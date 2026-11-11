// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-6b1 (register §1.314 R6B-3, §1.315; `contracts/policy-suspension.md`, "A
// stopped run"): `policy.execution_recorded` gains the result kind `stopped` `{outcome,
// runtime_ms}`. Nothing writes it yet, so each record here is built with the contract's own
// types. A stopped record whose `outcome` is a code is valid, its severity is its outcome's
// category and it reads back unchanged; one without `outcome`, or naming a member of a failure,
// does not decode; the public projection carries its outcome codes only; and the two existing
// kinds serialize as before.

use super::*;
use crate::codes::RuntimeErrorCode;

const AT_MS: u64 = 1_752_147_201_000;

fn execution_draft(outcome: PolicyExecutionOutcome) -> EventPayloadDraft {
    PolicyPayloadDraft::execution_recorded(
        PolicyExecutionEventData {
            decision_id: "decision:fixture-a".to_owned(),
            task_id: "task:fixture-a".to_owned(),
            instance_id: "instance:fixture-a".to_owned(),
            observed_at_unix_ms: AT_MS,
            outcome,
        },
        AuditInput::new(),
    )
    .into()
}

/// The stop's terminal outcome, with a value the codes-only projection leaves out (a duration
/// is not a registered name).
fn stop_outcome() -> Outcome {
    Outcome::new(RuntimeErrorCode::ContainedTaskCancelled).retry_after_ms(1_000_u64)
}

fn stopped() -> PolicyExecutionOutcome {
    PolicyExecutionOutcome::Stopped {
        outcome: stop_outcome(),
        runtime_ms: 4_000,
    }
}

/// The stored record's result object, `policy_result`.
fn result_of(stored: &mut serde_json::Value) -> &mut serde_json::Map<String, serde_json::Value> {
    super::sh1::holder(stored, "policy_result")
        .and_then(|data| data.get_mut("policy_result"))
        .and_then(serde_json::Value::as_object_mut)
        .expect("the record's policy_result")
}

#[test]
fn r6b1_c1_a_stopped_record_validates_and_reads_back_and_a_malformed_one_is_refused() {
    let event = sanitize(execution_draft(stopped()), 1);
    event
        .payload()
        .validate()
        .expect("a stopped record validates");
    let EventPayload::Policy(PolicyPayload::ExecutionRecorded(recorded)) = event.payload() else {
        panic!("an execution record");
    };
    assert_eq!(recorded.policy_result(), &stopped());
    // Its stored severity is its outcome's category, as for every result-bearing record.
    assert_eq!(
        event.severity(),
        EventSeverity::from_category(stop_outcome().category())
    );

    let stored = serde_json::to_value(event.payload()).expect("the record encodes");
    let decoded: EventPayload =
        serde_json::from_value(stored.clone()).expect("the stopped record decodes");
    decoded.validate().expect("the decoded record validates");
    assert_eq!(
        serde_json::to_value(&decoded).expect("the record encodes again"),
        stored,
        "the stopped record reads back unchanged"
    );
    let mut written = stored.clone();
    let result = result_of(&mut written);
    assert_eq!(
        result.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["kind", "outcome", "runtime_ms"],
        "a stopped result has exactly its two members beside its kind"
    );
    assert_eq!(
        result.get("kind"),
        Some(&serde_json::Value::String("stopped".to_owned()))
    );

    let mut without_outcome = stored.clone();
    result_of(&mut without_outcome).remove("outcome");
    assert!(
        serde_json::from_value::<EventPayload>(without_outcome).is_err(),
        "a stopped record without its outcome does not decode"
    );
    for member in [
        "failure",
        "failure_identity",
        "reported_success",
        "original_class",
        "effective_class",
        "consecutive_same_error",
        "escalation_streak",
        "performance_tax_exempt",
        "retry_attempt",
        "disposition",
        "retry_at_unix_ms",
        "sensitive",
        "perf_context",
        "condition",
    ] {
        let mut with_member = stored.clone();
        result_of(&mut with_member).insert(member.to_owned(), serde_json::Value::Null);
        assert!(
            serde_json::from_value::<EventPayload>(with_member).is_err(),
            "a stopped record naming {member} does not decode"
        );
    }
}

#[test]
fn r6b1_c2_the_public_projection_carries_a_stopped_outcome_codes_only() {
    let event = sanitize(execution_draft(stopped()), 2);
    let EventPayload::Policy(PolicyPayload::ExecutionRecorded(recorded)) = event.payload() else {
        panic!("an execution record");
    };
    let PolicyExecutionOutcome::Stopped {
        outcome: stored, ..
    } = recorded.policy_result()
    else {
        panic!("a stopped result");
    };
    assert!(
        stored.value("retry_after_ms").is_some(),
        "the full record keeps every value of the outcome"
    );
    let PublicEventPayload::Policy(public) = event.payload().public_projection() else {
        panic!("a policy projection");
    };
    let Some(PolicyExecutionOutcome::Stopped {
        outcome,
        runtime_ms,
    }) = public.policy_execution_outcome()
    else {
        panic!("the projection carries the stopped result");
    };
    assert_eq!(*runtime_ms, 4_000);
    assert!(outcome.is(RuntimeErrorCode::ContainedTaskCancelled));
    assert_eq!(outcome.category(), stored.category());
    assert_eq!(outcome.value("retry_after_ms"), None, "codes only");
    assert_eq!(outcome, &stored.codes_only());
}

#[test]
fn r6b1_c3_the_two_existing_kinds_serialize_as_before() {
    assert_eq!(
        serde_json::to_string(&PolicyExecutionOutcome::Succeeded { runtime_ms: 1_000 })
            .expect("a success encodes"),
        r#"{"kind":"succeeded","runtime_ms":1000}"#
    );
    let failure = PolicyFailureRecord {
        outcome: Outcome::new(RuntimeErrorCode::CaptureFailed),
        failure_identity: None,
        reported_success: false,
        original_class: PolicyFailureClass::Recoverable,
        effective_class: PolicyFailureClass::Recoverable,
        consecutive_same_error: 1,
        escalation_streak: 1,
        performance_tax_exempt: false,
        retry_attempt: 1,
        disposition: PolicyFailureDisposition::RetryScheduled,
        retry_at_unix_ms: Some(AT_MS + 100),
        runtime_ms: 1_000,
        sensitive: false,
        perf_context: Box::new(PerformanceContext::unavailable(AT_MS)),
        condition: None,
    };
    assert_eq!(
        serde_json::to_string(&PolicyExecutionOutcome::Failed {
            failure: failure.clone()
        })
        .expect("a failure encodes"),
        format!(
            r#"{{"kind":"failed","failure":{}}}"#,
            serde_json::to_string(&failure).expect("the failure record encodes")
        )
    );
    // Both still decode as themselves.
    for outcome in [
        PolicyExecutionOutcome::Succeeded { runtime_ms: 1_000 },
        PolicyExecutionOutcome::Failed { failure },
    ] {
        let encoded = serde_json::to_string(&outcome).expect("the result encodes");
        let decoded: PolicyExecutionOutcome =
            serde_json::from_str(&encoded).expect("the result decodes");
        assert_eq!(decoded, outcome);
    }
}
