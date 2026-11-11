// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-6b1 (register §1.314 R6B-8, §1.315): the frame retention matches a run's
// settlement with its terminal's branch. A stopped run's `stopped` record matches the `task.failed`
// terminal every scheduled cancel writes and no other branch, so, like a failed run's, it never
// sources an eviction intent (only a success does). Nothing writes the kind yet, so the record is
// built with the contract's own types.

use super::settlement_matches;
use crate::{PersistedEvent, Sha256SecretFingerprinter};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventSeverity, EventSource,
    IdentifierIssuer, OriginModule, PerformanceContext, PolicyExecutionEventData,
    PolicyExecutionOutcome, PolicyFailureClass, PolicyFailureDisposition, PolicyFailureRecord,
    PolicyPayloadDraft, RuntimeErrorCode, TaskOutcome,
};

const AT_MS: u64 = 1_752_147_200_400;

/// A stored `policy.execution_recorded` whose result is `outcome`.
fn settlement(outcome: PolicyExecutionOutcome) -> PersistedEvent {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let draft = EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        AT_MS,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Scheduler,
            OriginModule::Policy,
            EventActor::Scheduler,
        ),
        EventLinksDraft::default(),
        PolicyPayloadDraft::execution_recorded(
            PolicyExecutionEventData {
                decision_id: "decision:retention-fixture".to_owned(),
                task_id: "task:retention-fixture".to_owned(),
                instance_id: "instance:retention-fixture".to_owned(),
                observed_at_unix_ms: AT_MS,
                outcome,
            },
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"retention-r6b1-test").expect("fingerprinter"))
    .expect("a settlement fixture");
    PersistedEvent::from_sanitized(1, draft).expect("a stored settlement")
}

fn failure() -> PolicyFailureRecord {
    PolicyFailureRecord {
        outcome: Outcome::new(RuntimeErrorCode::CaptureFailed),
        failure_identity: None,
        reported_success: false,
        original_class: PolicyFailureClass::Recoverable,
        effective_class: PolicyFailureClass::Recoverable,
        consecutive_same_error: 1,
        escalation_streak: 1,
        performance_tax_exempt: false,
        retry_attempt: 0,
        disposition: PolicyFailureDisposition::Continue,
        retry_at_unix_ms: None,
        runtime_ms: 1_000,
        sensitive: false,
        perf_context: Box::new(PerformanceContext::unavailable(AT_MS)),
        condition: None,
    }
}

#[test]
fn r6b1_l2_the_retention_matches_a_stopped_record_with_its_failed_terminal_only() {
    let stopped = settlement(PolicyExecutionOutcome::Stopped {
        outcome: Outcome::new(RuntimeErrorCode::ContainedTaskCancelled),
        runtime_ms: 1_000,
    });
    assert!(
        settlement_matches(&stopped, TaskOutcome::Failure),
        "a stopped record matches its task.failed terminal"
    );
    assert!(
        !settlement_matches(&stopped, TaskOutcome::Success),
        "a stopped record never matches a success, so its frames source no eviction intent"
    );
    assert!(
        !settlement_matches(&stopped, TaskOutcome::Cancelled),
        "a scheduled run's stop is no task.cancelled"
    );

    // The two existing kinds match as before.
    let succeeded = settlement(PolicyExecutionOutcome::Succeeded { runtime_ms: 1_000 });
    assert!(settlement_matches(&succeeded, TaskOutcome::Success));
    assert!(!settlement_matches(&succeeded, TaskOutcome::Failure));
    let failed = settlement(PolicyExecutionOutcome::Failed { failure: failure() });
    assert!(settlement_matches(&failed, TaskOutcome::Failure));
    assert!(settlement_matches(&failed, TaskOutcome::Cancelled));
    assert!(!settlement_matches(&failed, TaskOutcome::Success));
}
