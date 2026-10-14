// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #386 SR-2a, test row T6d (scheduling model v1.4.2 §4.2 item 0, §11 X2): a v0.12.0
// build opens a state root a later build wrote, holding a run that a condition ended (register
// S1, written from SR-2b on) and a ladder suppression `blackout` (register S3, written from SR-4a
// on). The start succeeds, the counts and the streak are rebuilt from the records, and both
// records read back. The host's test hooks stand in for the later writers.

use super::*;
use actingcommand_contract::{
    PolicyConditionSettlement, RecoveryLadderSuppression, RuntimeLifecyclePhase, RuntimePayload,
};

/// A task limited to one run a day and one per window (the shape of W2 v14's once-a-day tasks).
fn once_a_day_sources() -> CatalogSources {
    let mut sources = budget_policy_sources(1);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("task fixture");
    tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(1);
    tasks["tasks"][0]["loop_budget"]["window_iteration_limit"] = serde_json::json!(1);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("task bytes");
    sources
}

/// The run ran out of its sampling deadline (SCHED §4.1): a condition code, recoverable.
fn sampling_ended() -> PolicyExecutionInput {
    PolicyExecutionInput::Failed {
        outcome: actingcommand_contract::outcome::Outcome::new(
            actingcommand_execution_kernel::ExecutionKernelCode::RecognitionSampleDeadlineExceeded
                .code(),
        ),
        failure_identity: None,
        class: PolicyFailureClass::Recoverable,
    }
}

#[test]
fn t6d_a_start_reads_the_condition_endings_and_blackout_suppressions_a_later_build_wrote() {
    let root = TempDir::new().expect("tempdir");
    let registered_id = instance_id();
    let condition = PolicyConditionSettlement {
        refunded: true,
        count: 1,
        expires_at_unix_ms: POLICY_NOW_UNIX_MS + 3_600_000,
    };
    let suppression = RuntimeLifecyclePhase::RecoveryLadderSuppressed {
        reason: RecoveryLadderSuppression::Blackout,
        until_unix_ms: POLICY_NOW_UNIX_MS + 1_800_000,
        rule: None,
    };

    // The later build: one run of the pair ends by a condition, and a ladder is suppressed by a
    // blackout.
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the writing host");
    host.activate_policy_catalog(&once_a_day_sources())
        .expect("activate the catalog");
    let (_, first, reasons) =
        evaluated_policy_dispatch_at(&host, PolicyTrigger::Recovery, POLICY_NOW_UNIX_MS, 100);
    record_policy_approval(&host, &first);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&first, &reasons, &policy_context(&host, &first))
        .expect("the first admission")
    else {
        panic!("expected the first admission")
    };
    assert_eq!(context.admission().budget.task_daily_used, 1);
    clock.advance(75_000);
    host.settle_next_policy_run_by_condition_for_test(condition)
        .expect("the condition of the next settlement");
    let ended = host
        .record_policy_dispatch_outcome(&first.decision_id, &sampling_ended())
        .expect("the condition settlement");
    let PolicyExecutionOutcome::Failed { failure } = &ended.outcome else {
        panic!("expected a failure")
    };
    assert_eq!(failure.condition, Some(condition));
    host.append_lifecycle_observed_for_test(suppression.clone())
        .expect("the blackout suppression");
    host.close().expect("close the writing host");

    // The v0.12.0 build starts on the same root later the same day.
    let restart_unix_ms = POLICY_NOW_UNIX_MS + 600_000;
    let clock = Arc::new(ManualRuntimeClock::new(restart_unix_ms, 0));
    let reopened = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the start reads both records");
    let events = reopened
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    assert!(events.iter().any(|event| matches!(
        event.payload(),
        EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload))
            if matches!(
                payload.policy_result(),
                PolicyExecutionOutcome::Failed { failure } if failure.condition == Some(condition)
            )
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload(),
        EventPayload::Runtime(RuntimePayload::LifecycleObserved(observed))
            if observed.phase() == suppression
    )));

    // The counts: the refunded run gave its counts back, so the task is admitted again today.
    let (_, rerun, reasons) =
        evaluated_policy_dispatch_at(&reopened, PolicyTrigger::Recovery, restart_unix_ms, 101);
    let PolicyDispatchAdmission::Granted { context } = reopened
        .admit_policy_dispatch(&rerun, &reasons, &policy_context(&reopened, &rerun))
        .expect("the rerun admission")
    else {
        panic!("expected the rerun admission")
    };
    assert_eq!(context.admission().budget.task_daily_used, 1);

    // The streak: the condition ending added none, so the same code ending this run as an
    // ordinary failure is the pair's first.
    clock.advance(75_000);
    let failed = reopened
        .record_policy_dispatch_outcome(&rerun.decision_id, &sampling_ended())
        .expect("the ordinary failure");
    let PolicyExecutionOutcome::Failed { failure } = &failed.outcome else {
        panic!("expected a failure")
    };
    assert_eq!(failure.condition, None);
    assert_eq!(failure.consecutive_same_error, 1);
    assert_eq!(failure.escalation_streak, 1);
    reopened.close().expect("close the reading host");
}
