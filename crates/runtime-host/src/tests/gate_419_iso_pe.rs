// SPDX-License-Identifier: AGPL-3.0-only

// ISO-PE (#419; brief `rd-1029/brief-iso-pe.md`, investigation `rd-1029/inv-1119.md`; production
// v0.12.26, 10-29 11:19): a policy evaluation that fails with a failure that is not global no
// longer stops the Runtime. The host records the failure with its detail
// (`policy_evaluation_skipped`, its cause `policy_evaluation_rejected` and the policy's
// `policy_evaluation_input_invalid` naming the refused input) and answers a deferred cycle that
// evaluates again `scheduling.evaluation.retry_ms` later; `scheduling.evaluation.escalate_after`
// skips in a row record `policy_evaluation_awaiting_person` once; a global failure is still
// returned. The evaluation's time is sampled under the gates its inputs are read under, so an
// input committed while it waited for them is never later than its time. Every expected value is
// the work order's; the Runtime clock is a manual one.

use super::gate_386_sr2b1::{rerun_manifest, start};
use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::host::skips_policy_evaluation;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{FactScalar, RuntimeConfigManifest, RuntimePayload};
use actingcommand_runtime_config::scheduling::{EVALUATION_ESCALATE_AFTER, EVALUATION_RETRY_MS};
use serde_json::{Value, json};

const SKIPPED: &str = "policy_evaluation_skipped";
const AWAITING: &str = "policy_evaluation_awaiting_person";
const REJECTED: &str = "policy_evaluation_rejected";
const INPUT_INVALID: &str = "policy_evaluation_input_invalid";
/// The shipped `scheduling.evaluation.retry_ms` and `.escalate_after`.
const DEFAULT_RETRY_MS: u64 = 15_000;
const DEFAULT_ESCALATE_AFTER: u64 = 3;

/// The `runtime.failed` outcomes of `code`, in ledger order.
fn failed(host: &RuntimeHost, code: &str) -> Vec<Outcome> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::RuntimeFailed),
        ..EventQuery::default()
    })
    .expect("the runtime.failed records")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Runtime(RuntimePayload::Failed(record))
            if record.outcome().code().as_str() == code =>
        {
            Some(record.outcome().clone())
        }
        _ => None,
    })
    .collect()
}

/// The first outcome of `code` in `outcome`'s tree, itself included.
fn find<'a>(outcome: &'a Outcome, code: &str) -> Option<&'a Outcome> {
    if outcome.code().as_str() == code {
        return Some(outcome);
    }
    outcome
        .causes()
        .iter()
        .find_map(|link| find(link.outcome(), code))
}

fn value(outcome: &Outcome, key: &str) -> Value {
    outcome.value(key).cloned().unwrap_or(Value::Null)
}

/// Whether some link of `outcome`'s tree carries `text` in its `raw_text`.
fn raw_text_holds(outcome: &Outcome, text: &str) -> bool {
    value(outcome, "raw_text")
        .as_str()
        .is_some_and(|raw| raw.contains(text))
        || outcome
            .causes()
            .iter()
            .any(|link| raw_text_holds(link.outcome(), text))
}

/// The code of the failure each `policy_evaluation_skipped` record names (its first `caused_by`
/// link), in ledger order. Shared with the tests whose non-global evaluation failure is skipped
/// since ISO-PE.
pub(super) fn skipped_failure_codes(host: &RuntimeHost) -> Vec<String> {
    failed(host, SKIPPED)
        .iter()
        .map(|skipped| {
            skipped
                .causes()
                .first()
                .map(|link| link.outcome().code().as_str().to_owned())
                .unwrap_or_default()
        })
        .collect()
}

/// The Runtime still runs: nothing latched and no stop requested.
fn assert_running(host: &RuntimeHost) {
    assert!(
        host.fatal_error().expect("the health").is_none(),
        "nothing latched"
    );
    assert!(
        !host.is_shutdown_requested().expect("the shutdown flag"),
        "no stop requested"
    );
}

/// A fixture host whose manual clock reads `at`; the fixture catalog active.
fn host_at(
    root: &TempDir,
    at: u64,
    manifest: Option<RuntimeConfigManifest>,
) -> (RuntimeHost, Arc<ManualRuntimeClock>) {
    let (host, clock) = start(root, instance_id(), at, manifest);
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate the catalog");
    (host, clock)
}

#[test]
fn iso_pe_t1_an_input_invalid_evaluation_is_skipped_with_its_detail_and_evaluated_again_after_the_wait()
 {
    // The fixture's outcome and pool were observed at POLICY_NOW_UNIX_MS; a minute before it the
    // evaluation refuses its input as later than its time.
    let root = TempDir::new().expect("tempdir");
    let at = POLICY_NOW_UNIX_MS - 60_000;
    let (host, clock) = host_at(&root, at, None);

    let cycle = host
        .evaluate_policy_cycle(PolicyTrigger::Recovery)
        .expect("a skipped evaluation is no error");
    assert!(cycle.evaluation.is_none(), "nothing was evaluated");
    assert!(
        cycle.pending_dispatch_intents.is_empty(),
        "nothing dispatched"
    );
    assert_eq!(cycle.directive.kind, PolicyRecomputeKind::Deferred);
    assert_eq!(
        cycle.directive.eligible_at_unix_ms,
        at + DEFAULT_RETRY_MS,
        "evaluated again scheduling.evaluation.retry_ms later"
    );
    assert_running(&host);

    // The skip, with the failure and its detail all the way down.
    let skipped = failed(&host, SKIPPED);
    assert_eq!(skipped.len(), 1, "{skipped:#?}");
    let skip = &skipped[0];
    assert_eq!(value(skip, "count"), json!(1));
    assert_eq!(value(skip, "limit_count"), json!(DEFAULT_ESCALATE_AFTER));
    assert_eq!(value(skip, "retry_after_ms"), json!(DEFAULT_RETRY_MS));
    assert_eq!(
        value(skip, "next_eligible_unix_ms"),
        json!(at + DEFAULT_RETRY_MS)
    );
    let rejected = find(skip, REJECTED).expect("the rejection is the skip's cause");
    assert_eq!(value(rejected, "operation"), json!("evaluate_policy_cycle"));
    let invalid =
        find(rejected, INPUT_INVALID).expect("the policy's code is the rejection's cause");
    assert_eq!(value(invalid, "policy_input"), json!("outcome"));
    assert_eq!(
        value(invalid, "policy_input_problem"),
        json!("observed_after_evaluation")
    );
    assert_eq!(value(invalid, "task_id"), json!("fixture.observe"));
    assert_eq!(value(invalid, "instance_id"), json!(POLICY_INSTANCE_ALIAS));
    assert_eq!(
        value(invalid, "observed_at_unix_ms"),
        json!(POLICY_NOW_UNIX_MS)
    );
    assert_eq!(value(invalid, "evaluated_at_unix_ms"), json!(at));
    assert!(
        raw_text_holds(skip, "is later than evaluation time"),
        "the policy's own text is kept: {skip:#?}"
    );
    assert!(
        failed(&host, AWAITING).is_empty(),
        "one skip escalates nothing"
    );

    // After the wait, with the input no longer later than the evaluation, it evaluates and
    // dispatches.
    clock.set_unix_ms(POLICY_NOW_UNIX_MS);
    let cycle = host
        .evaluate_policy_cycle(PolicyTrigger::Recovery)
        .expect("the evaluation after the wait");
    let evaluation = cycle.evaluation.expect("an evaluation");
    assert!(
        !evaluation.dispatch_intents.is_empty(),
        "the instance dispatches again: {evaluation:#?}"
    );
    assert_eq!(failed(&host, SKIPPED).len(), 1, "no second skip");
    assert_running(&host);
    host.close().expect("close the host");
}

#[test]
fn iso_pe_t2_n_skips_in_a_row_escalate_to_a_person_once_and_never_stop_the_runtime() {
    // Both values come from the configuration: retry 5 s, escalate after 2.
    let retry_ms = 5_000;
    let manifest = rerun_manifest(vec![
        (EVALUATION_RETRY_MS, FactScalar::DurationMs(retry_ms)),
        (EVALUATION_ESCALATE_AFTER, FactScalar::Integer(2)),
    ]);
    let root = TempDir::new().expect("tempdir");
    let at = POLICY_NOW_UNIX_MS - 600_000;
    let (host, clock) = host_at(&root, at, Some(manifest));

    for skip in 1..=4_u64 {
        let cycle = host
            .evaluate_policy_cycle(PolicyTrigger::Recovery)
            .unwrap_or_else(|error| panic!("skip {skip} is no error: {error:?}"));
        assert!(cycle.evaluation.is_none());
        assert_eq!(
            cycle.directive.eligible_at_unix_ms,
            at + (skip - 1) * retry_ms + retry_ms
        );
        assert_running(&host);
        clock.advance(retry_ms);
    }
    let counts = failed(&host, SKIPPED)
        .iter()
        .map(|skip| value(skip, "count"))
        .collect::<Vec<_>>();
    assert_eq!(counts, vec![json!(1), json!(2), json!(3), json!(4)]);
    let awaiting = failed(&host, AWAITING);
    assert_eq!(awaiting.len(), 1, "escalated once per run of skips");
    assert_eq!(value(&awaiting[0], "count"), json!(2));
    assert_eq!(value(&awaiting[0], "limit_count"), json!(2));
    assert_eq!(value(&awaiting[0], "retry_after_ms"), json!(retry_ms));
    assert!(
        find(&awaiting[0], INPUT_INVALID).is_some(),
        "the escalation names the failure: {awaiting:#?}"
    );

    // An evaluation that evaluates ends the run of skips; the next skip counts from one again
    // and escalates nothing yet.
    clock.set_unix_ms(POLICY_NOW_UNIX_MS);
    assert!(
        host.evaluate_policy_cycle(PolicyTrigger::Recovery)
            .expect("the evaluation")
            .evaluation
            .is_some()
    );
    clock.set_unix_ms(POLICY_NOW_UNIX_MS - 1);
    host.evaluate_policy_cycle(PolicyTrigger::Recovery)
        .expect("a skip after the run ended");
    assert_eq!(
        failed(&host, SKIPPED)
            .last()
            .map(|skip| value(skip, "count")),
        Some(json!(1))
    );
    assert_eq!(failed(&host, AWAITING).len(), 1);
    assert_running(&host);
    host.close().expect("close the host");
}

#[test]
fn iso_pe_t3_an_input_observed_while_the_evaluation_waits_for_its_gates_is_not_later_than_its_time()
{
    // The evaluation starts a second before the fixture's outcome and pool were observed; before
    // it takes the gates it reads them under, the Runtime clock reaches their observation (as when
    // a settlement on another thread committed them meanwhile). Sampled as it started, the
    // evaluation's time was earlier than its inputs and it refused them (production 10-29 11:19);
    // sampled under the gates, it evaluates.
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = host_at(&root, POLICY_NOW_UNIX_MS - 1_000, None);
    let moved = Arc::clone(&clock);
    host.run_before_policy_inputs_read_for_test(Box::new(move || {
        moved.set_unix_ms(POLICY_NOW_UNIX_MS);
    }))
    .expect("the hook");

    let cycle = host
        .evaluate_policy_cycle(PolicyTrigger::Recovery)
        .expect("the evaluation");
    let evaluation = cycle.evaluation.expect("evaluated, not skipped");
    let intent = evaluation
        .dispatch_intents
        .first()
        .expect("a dispatch intent");
    assert_eq!(
        intent.prerequisites.evaluated_at_unix_ms, POLICY_NOW_UNIX_MS,
        "the evaluation's time is the one sampled under the gates"
    );
    assert!(failed(&host, SKIPPED).is_empty(), "nothing skipped");
    host.close().expect("close the host");
}

#[test]
fn iso_pe_t4_a_global_failure_is_returned_and_only_a_non_global_one_is_skipped() {
    let instance = instance_id();
    let cases = [
        // Global: the ledger, a poisoned lock, a fatal failure on no instance.
        (
            "ledger read",
            RuntimeHostError::fatal(
                RuntimeErrorCode::LedgerFailure,
                HostLocation::ReadPolicyFactPosition,
                RuntimeErrorCode::LedgerFailure,
            ),
            false,
        ),
        (
            "poisoned lock",
            RuntimeHostError::fatal(
                HostCode::RuntimeStatePoisoned,
                HostLocation::EvaluatePolicyCycle,
                RuntimeErrorCode::RuntimeHalted,
            ),
            false,
        ),
        // Not global, with its own bounded handling in actingd: returned.
        (
            "snapshot changed",
            RuntimeHostError::request(
                HostCode::PolicyOutcomeSnapshotChanged,
                HostLocation::ValidatePolicyOutcomeKeySnapshot,
                RuntimeErrorCode::InvalidRequest,
            ),
            false,
        ),
        (
            "projection not ready",
            RuntimeHostError::request(
                SharedCode::OutcomeProjectionNotReady,
                HostLocation::EvaluatePolicyCycle,
                RuntimeErrorCode::RuntimeUnavailable,
            ),
            false,
        ),
        // Not global: skipped.
        (
            "evaluation rejected",
            RuntimeHostError::request(
                HostCode::PolicyEvaluationRejected,
                HostLocation::EvaluatePolicyCycle,
                RuntimeErrorCode::InvalidRequest,
            ),
            true,
        ),
        (
            "one instance's fault",
            RuntimeHostError::instance_fault(
                HostCode::PolicyEvaluationRejected,
                HostLocation::EvaluatePolicyCycle,
                RuntimeErrorCode::RuntimeHalted,
                instance,
            ),
            true,
        ),
    ];
    for (case, error, skipped) in cases {
        assert_eq!(
            skips_policy_evaluation(&error),
            skipped,
            "{case}: {error:?}"
        );
        assert_eq!(
            error.halts_runtime(),
            !skipped && error.is_fatal(),
            "{case}: only a global failure halts"
        );
    }
}
