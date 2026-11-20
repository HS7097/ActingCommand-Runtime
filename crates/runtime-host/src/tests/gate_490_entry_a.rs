// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #490 ENTRY-A (0.12.76; register §1.494; `contracts/policy-suspension.md`, "A failure
//! at the entry"; `contracts/scheduling-blackout.md`, "Order of the deferrals", position 4b). The
//! 11-20 production defect: a package left a game on a screen no package knows; every task due
//! after it failed at its entry (0 steps of its main package), was dispatched again within seconds
//! by the immediate rerun until its runs of the day were spent, a rule whose
//! `escalation_threshold` is 1 was paused by one such failure, and once the runs were spent nothing
//! triggered a recovery. Requirements (the coordinator's work order of 11-20, tests 1 to 5):
//!
//! - T1: a failure at the entry settles as a condition of the instance: its counts given back, no
//!   streak, no retry, no rerun owed; the instance is blocked, and the next evaluation dispatches
//!   nothing there, its candidates waiting `instance_entry_blocked`, which the not-running row says;
//! - T2: a rule whose `escalation_threshold` is 1 is not paused by a failure at its entry, and still
//!   is by a failure in a step of its main package;
//! - T1, T3, T4 replayed on a fake device: a scheduled run on a screen its packages do not know fails
//!   in its prerequisite layer with `contained_task_page_unknown`; it blocks its instance and enters
//!   the ladder by `pack_logic`; the ladder ends without success, the instance stays blocked and a
//!   person is told once; a client's run on the instance is not refused and its success unblocks it;
//! - T3: a recovery the ladder's cool-down held back is taken up at the cool-down's end, its success
//!   unblocks the instance, and the due task is dispatched again.
//! - T5 (a step failure as before) is the existing tests, unchanged.
//!
//! ENTRY-A2 (the coordinator's ruling of 11-20 23:57 JST, tests (a) to (d)): a failure at the entry
//! is the instance's condition only while nothing says it is the pair's own.
//!
//! - (a) a failure at the entry, a successful recovery, and the same task's next failure at the
//!   entry: the second is the pair's own, settled as a failure that accumulates (not refunded, its
//!   backoff set) and blocking the instance again; after the next recovery another due task is
//!   dispatched before it;
//! - (b) with `escalation_threshold` 2 the pair is paused by the own failure that reaches it, and
//!   the instance it blocked is unblocked by the next recovery and stays so;
//! - (c) a run of the task whose main package started clears the pair's mark: its next failure at
//!   the entry, after a recovery, is a condition again;
//! - (d) an instance unblocked by a pause set and lifted, not by a recovery: the next failure at the
//!   entry is a condition again.
//!
//! The policy-level tests settle with the host's test hook that arms the next settlement as one of a
//! run that failed at its entry, judged as the live settlement judges it (the live settlement tells
//! that from the run's own records, which the replays exercise), or as one whose main package
//! started; a recovery's success is told to the blocks as the ladder's finished fact tells them
//! (the replays run real ladders). No clock is read in this file.

use super::gate_373_nav3a::{
    AWAY, LEAD, MAIN, MENU, OPEN, Pack, RETURN_HOME, StandardRoot, route, screens,
};
use super::gate_386_sr2b1::{admit, failure_of, start};
use super::instance_workers::{
    admit_ladder_run, all_events, ladder_finished, ladder_setup_with_packages, phases,
    wait_for_ladder_end, wait_until,
};
use super::*;
use crate::codes::HostCode;
use crate::failure_identity::{FailureIdentity, IdentityFrame, IdentityPackage};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    PolicyControlStateRecord, PolicyFailureRecord, RecoveryLadderOutcome,
    RecoveryLadderSuppression, RuntimeLifecyclePhase, RuntimePayload,
};
use serde_json::{Value, json};

const TASK: &str = "fixture.observe";
const PAGE_UNKNOWN: &str = "contained_task_page_unknown";
const BLOCKED: &str = "instance_entry_blocked";
/// A run's runtime, within the budget fixture's runtime budget.
const RUN_MS: u64 = 75_000;
/// The catalog fixture task's next occurrence after a run at the fixture's instant, plus one
/// default policy cadence (gate_489_avail1's).
const NEXT_DISPATCH_UNIX_MS: u64 = POLICY_NOW_UNIX_MS + 3_600_000 + 1_000;
/// The ladder's cool-down window left when the replayed run fails (T3).
const COOLDOWN_LEFT_MS: u64 = 600_000;
/// ENTRY-A2: the second task of the same rule on the policy instance, of a lower priority.
const OTHER: &str = "fixture.other";
/// The catalog fixture's `on_failure.retry_backoff_ms`: a first failure's backoff.
const BACKOFF_MS: u64 = 30_000;

// ---- The policy-level fixture.

/// The budget fixture (one task, due at every evaluation) with `on_failure.escalation_threshold`.
fn sources(escalation_threshold: u16) -> CatalogSources {
    let mut sources = budget_policy_sources(1);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("the task fixture");
    tasks["tasks"][0]["on_failure"]["escalation_threshold"] = json!(escalation_threshold);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("the task bytes");
    sources
}

/// [`sources`] with a second task, `OTHER`: the same rule under its own procedure and a lower
/// priority (50; `TASK`'s is 100), due at every evaluation too, so it is dispatched when `TASK`
/// is not.
fn two_tasks(escalation_threshold: u16) -> CatalogSources {
    let mut sources = sources(escalation_threshold);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("the task fixture");
    let mut other = tasks["tasks"][0].clone();
    other["id"] = json!(OTHER);
    other["procedure_ref"] = json!("procedure.observe-b");
    other["priority"] = json!(50);
    other["produces"] = json!([]);
    other["instance_overrides"] = json!([]);
    tasks["tasks"]
        .as_array_mut()
        .expect("the tasks")
        .push(other);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("the task bytes");
    sources
}

/// A failure identity whose base is `code` (its main package not measured) with the frame mark
/// `frame`.
fn identity(code: &str, frame: IdentityFrame) -> String {
    FailureIdentity {
        base: code.to_owned(),
        key: "0".repeat(12),
        main: IdentityPackage {
            id_hash: "0a1b2c3d".to_owned(),
            hash: "x".to_owned(),
        },
        layers: Vec::new(),
        frame,
    }
    .encode()
}

/// `intent`'s run failed in its prerequisite layer on a screen the layer does not know: the input
/// the live settlement builds for it (a failure that does not accumulate, its `F` unique).
fn at_entry(intent: &DispatchIntent) -> PolicyExecutionInput {
    PolicyExecutionInput::Failed {
        outcome: Outcome::new(SharedCode::ContainedTaskPageUnknown),
        failure_identity: Some(identity(
            PAGE_UNKNOWN,
            IdentityFrame::unique(&intent.decision_id),
        )),
        class: PolicyFailureClass::Recoverable,
    }
}

/// A step of the main package did not reach its page: a failure that accumulates.
fn in_a_step() -> PolicyExecutionInput {
    PolicyExecutionInput::Failed {
        outcome: Outcome::new(
            actingcommand_execution_kernel::ExecutionKernelCode::PageConfirmationFailed.code(),
        ),
        failure_identity: Some(identity("page_confirmation_failed", IdentityFrame::None)),
        class: PolicyFailureClass::Recoverable,
    }
}

/// The pair's latest L7 value record in the ledger.
fn latest_value_record(host: &RuntimeHost) -> PolicyControlStateRecord {
    all_events(host)
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Policy(PolicyPayload::ControlStateRecorded(payload))
                if payload.state().task_id == TASK =>
            {
                Some(payload.state().clone())
            }
            _ => None,
        })
        .next_back()
        .expect("the pair's L7 record")
}

/// The policy's evaluation of the policy instance at `at`.
fn evaluate(host: &RuntimeHost, at: u64, seed: u64) -> actingcommand_policy::PolicyEvaluation {
    host.evaluate_policy_cycle_with_test_inputs(
        &policy_facts(),
        &policy_resources(),
        EvaluationTime {
            unix_ms: at,
            monotonic_ms: at,
        },
        seed,
        PolicyTrigger::FactsChanged,
    )
    .expect("evaluate the policy")
    .evaluation
    .expect("an evaluation")
}

/// Whether `evaluation` dispatched `task_id` on the policy instance.
fn dispatched(evaluation: &actingcommand_policy::PolicyEvaluation, task_id: &str) -> bool {
    evaluation
        .dispatch_intents
        .iter()
        .any(|intent| intent.task_id == task_id && intent.instance_id == POLICY_INSTANCE_ALIAS)
}

/// The `instance_entry_blocked` reason of `task_id` on the policy instance in `evaluation`.
fn block_reason(
    evaluation: &actingcommand_policy::PolicyEvaluation,
    task_id: &str,
) -> Option<Outcome> {
    evaluation
        .decisions
        .iter()
        .filter(|decision| {
            decision.task_id == task_id
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .flat_map(|decision| decision.reasons.iter())
        .find(|reason| reason.outcome.is(HostCode::InstanceEntryBlocked))
        .map(|reason| reason.outcome.clone())
}

/// The policy instance's not-running row as `ScheduleExplanation` answers it: its `why` and since
/// when.
fn not_running(host: &RuntimeHost) -> (Outcome, u64) {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::ScheduleExplanation {
        instance: Some(POLICY_INSTANCE_ALIAS.to_owned()),
        candidates: false,
    });
    let receipt = client.send(&request);
    let Some(RuntimeResult::ScheduleExplanation { explanation }) = receipt.result() else {
        panic!("a schedule explanation: {receipt:?}");
    };
    let entry = explanation
        .instances
        .first()
        .unwrap_or_else(|| panic!("the instance's entry: {explanation:?}"));
    (
        entry.not_running.why.clone(),
        entry.not_running.since_unix_ms,
    )
}

/// The record a failure at the entry settles to: a condition, its counts given back, the pair's
/// counters as they were (none), no retry, no pause, its identity's `F` an entry failure's.
fn assert_settled_at_entry(failure: &PolicyFailureRecord) {
    assert_a_condition_at_entry(failure);
    assert_eq!(
        (failure.consecutive_same_error, failure.escalation_streak),
        (0, 0)
    );
}

/// [`assert_settled_at_entry`] but for the pair's counters, which a condition keeps as the pair's
/// latest real failure left them.
fn assert_a_condition_at_entry(failure: &PolicyFailureRecord) {
    let condition = failure.condition.expect("a condition");
    assert!(condition.refunded, "{failure:?}");
    assert_eq!(condition.count, 1, "{failure:?}");
    assert_eq!(failure.disposition, PolicyFailureDisposition::Continue);
    assert_eq!(failure.effective_class, PolicyFailureClass::Recoverable);
    assert_eq!((failure.retry_attempt, failure.retry_at_unix_ms), (0, None));
    assert_eq!(crate::policy_control::pause_cause(failure), None);
    assert_eq!(failure.outcome.code().as_str(), PAGE_UNKNOWN);
    let identity = failure
        .failure_identity
        .as_deref()
        .and_then(FailureIdentity::parse)
        .expect("a v2 failure identity");
    assert!(
        identity.at_entry() && !identity.accumulates(),
        "{identity:?}"
    );
    assert!(crate::policy_control::entry_blocked(failure));
}

// ---- T1, T2.

/// T1: the run of the budget fixture's task (its daily limit 4) fails at its entry. Its record is a
/// condition of the instance; the pair's value record holds no count for the day, no failure streak
/// and no owed rerun; the instance is blocked by that run. An evaluation a second later dispatches
/// nothing: the task, due at every evaluation, waits `instance_entry_blocked` naming the run, its
/// failure and since when, with no immediate rerun; the not-running row says it, since that
/// evaluation.
#[test]
fn entry_a_t1_a_failure_at_the_entry_settles_as_a_condition_and_blocks_the_instance() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&sources(2))
        .expect("activate the catalog");
    let (intent, admission) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    assert_eq!(admission.budget.task_daily_used, Some(1));
    clock.advance(RUN_MS);
    host.settle_next_policy_run_at_entry_for_test()
        .expect("arm the settlement");
    let settled = host
        .record_policy_dispatch_outcome(&intent.decision_id, &at_entry(&intent))
        .expect("the settlement");
    let failure = failure_of(&settled);
    assert_settled_at_entry(&failure);

    let record = latest_value_record(&host);
    assert!(
        record.daily_counts.iter().all(|count| count.count == 0),
        "the run's count is given back: {record:?}"
    );
    assert!(record.failure_streak.is_none(), "no streak: {record:?}");
    assert!(
        record.condition_reruns.is_none(),
        "no rerun owed: {record:?}"
    );

    let (gate, awaiting_person, held) = host
        .entry_block_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the instance's block")
        .expect("the instance is blocked");
    assert_eq!(
        (
            gate.task_id.as_str(),
            gate.decision_id.as_str(),
            gate.failure_code.as_str(),
            gate.since_unix_ms
        ),
        (
            TASK,
            intent.decision_id.as_str(),
            PAGE_UNKNOWN,
            settled.observed_at_unix_ms
        )
    );
    assert!(!awaiting_person);
    assert_eq!(held, None);

    let at = settled.observed_at_unix_ms + 1_000;
    advance_manual_clock_to(&clock, at);
    let evaluation = evaluate(&host, at, 200);
    assert!(
        !dispatched(&evaluation, TASK),
        "nothing is dispatched to the blocked instance: {evaluation:#?}"
    );
    assert!(
        evaluation
            .dispatch_intents
            .iter()
            .all(|intent| intent.instance_id != POLICY_INSTANCE_ALIAS),
        "no candidate of the instance: {evaluation:#?}"
    );
    let reason = block_reason(&evaluation, TASK)
        .unwrap_or_else(|| panic!("the task waits for the block: {evaluation:#?}"));
    assert_eq!(
        reason.value("instance_id"),
        Some(&json!(POLICY_INSTANCE_ALIAS))
    );
    assert_eq!(reason.value("task_id"), Some(&json!(TASK)));
    assert_eq!(
        reason.value("decision_id"),
        Some(&json!(intent.decision_id))
    );
    assert_eq!(reason.value("failure_code"), Some(&json!(PAGE_UNKNOWN)));
    assert_eq!(
        reason.value("observed_at_unix_ms"),
        Some(&json!(settled.observed_at_unix_ms))
    );
    assert!(
        !evaluation
            .decisions
            .iter()
            .flat_map(|decision| decision.reasons.iter())
            .any(|reason| reason.outcome.code().as_str() == "failure_retry_immediate"),
        "no immediate rerun: {evaluation:#?}"
    );
    let (why, since_unix_ms) = not_running(&host);
    assert_eq!(why.code().as_str(), BLOCKED, "{why:?}");
    assert_eq!(since_unix_ms, at);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// T2: a rule whose `escalation_threshold` is 1. A failure at its entry settles as a condition and
/// does not pause its pair; the same rule's failure in a step of its main package (on a fresh root)
/// still pauses it at once (`repeated_failure`) and blocks nothing.
#[test]
fn entry_a_t2_a_threshold_of_one_pauses_a_step_failure_and_never_a_failure_at_the_entry() {
    for case in ["at_entry", "in_a_step"] {
        let root = TempDir::new().expect("tempdir");
        let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
        host.activate_policy_catalog(&sources(1))
            .expect("activate the catalog");
        let (intent, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
        clock.advance(RUN_MS);
        let input = if case == "at_entry" {
            host.settle_next_policy_run_at_entry_for_test()
                .expect("arm the settlement");
            at_entry(&intent)
        } else {
            in_a_step()
        };
        let failure = failure_of(
            &host
                .record_policy_dispatch_outcome(&intent.decision_id, &input)
                .expect("the settlement"),
        );
        let blocked = host
            .entry_block_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's block");
        if case == "at_entry" {
            assert_settled_at_entry(&failure);
            assert!(blocked.is_some(), "{case}");
        } else {
            assert_eq!(failure.condition, None, "{case}");
            assert_eq!(
                failure.disposition,
                PolicyFailureDisposition::PausedTask,
                "{case}"
            );
            assert_eq!(
                crate::policy_control::pause_cause(&failure).map(crate::codes::PauseCause::as_str),
                Some("repeated_failure"),
                "{case}"
            );
            assert!(blocked.is_none(), "{case}");
        }
        assert!(host.fatal_error().expect("runtime health").is_none());
        host.close().expect("close the host");
    }
}

// ---- ENTRY-A2: whose failure.

/// The clock moved to `at` and the next dispatch on the policy instance evaluated and admitted
/// there: its intent.
fn admit_at(host: &RuntimeHost, clock: &ManualRuntimeClock, at: u64, seed: u64) -> DispatchIntent {
    advance_manual_clock_to(clock, at);
    admit(host, at, seed, false).0
}

/// `intent`'s run, `RUN_MS` long, settled as one that failed at its entry in its prerequisite
/// layer (the input the live settlement builds before it judges whose failure it is): its
/// execution record.
fn settle_at_entry(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    intent: &DispatchIntent,
) -> actingcommand_contract::PolicyExecutionEventData {
    clock.advance(RUN_MS);
    host.settle_next_policy_run_at_entry_for_test()
        .expect("arm the settlement");
    host.record_policy_dispatch_outcome(&intent.decision_id, &at_entry(intent))
        .expect("the settlement")
}

/// The record of a failure at the entry that is its pair's own: a failure that accumulates, the
/// `streak`-th of its identity in a row, with no condition (its counts stay taken), its identity's
/// `F` neither unique nor an entry failure's (no error frame here: `na`).
fn assert_own(failure: &PolicyFailureRecord, streak: u16) {
    assert_eq!(failure.condition, None, "{failure:?}");
    assert_eq!(
        (failure.consecutive_same_error, failure.escalation_streak),
        (streak, streak),
        "{failure:?}"
    );
    assert_eq!(failure.outcome.code().as_str(), PAGE_UNKNOWN);
    let identity = failure
        .failure_identity
        .as_deref()
        .and_then(FailureIdentity::parse)
        .expect("a v2 failure identity");
    assert!(
        identity.accumulates() && !identity.at_entry(),
        "{identity:?}"
    );
    assert_eq!(identity.frame, IdentityFrame::None);
    assert!(!crate::policy_control::entry_blocked(failure));
}

/// The policy instance's entry block, which must be set: its gate.
fn blocked_by(host: &RuntimeHost) -> crate::entry_block::EntryBlockGate {
    host.entry_block_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the instance's block")
        .expect("the instance is blocked")
        .0
}

/// `TASK`'s mark on the policy instance, if it is set: the instance's successful recoveries when
/// the run that set it began.
fn mark(host: &RuntimeHost) -> Option<u64> {
    host.entry_mark_for_test(POLICY_INSTANCE_ALIAS, TASK)
        .expect("the pair's mark")
}

/// A recovery of the policy instance succeeded: the instance is not blocked after it.
fn recover(host: &RuntimeHost) {
    host.recover_instance_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the recovery's success");
    assert_eq!(
        host.entry_block_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's block"),
        None,
        "no block after a recovery's success"
    );
}

/// A pause of every instance set and lifted by a client, as a person does.
fn pause_and_resume(host: &RuntimeHost) {
    let mut client = TestClient::connect(host);
    client.set_receipt_read_timeout();
    let scope = actingcommand_contract::SchedulingPauseScope::Global;
    let pause = client.request(RuntimeOperation::PauseScheduling {
        scope: scope.clone(),
        pause_reason: actingcommand_contract::SchedulingPauseReason::new("entry_check")
            .expect("pause reason"),
        drain_timeout_ms: 1_000,
        pause_mode: actingcommand_contract::SchedulingPauseMode::Stand,
    });
    let receipt = client.send(&pause);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let resume = client.request(RuntimeOperation::ResumeScheduling {
        scope,
        expected: None,
    });
    let receipt = client.send(&resume);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

/// ENTRY-A2 (a): two tasks of the rule on the policy instance, `TASK` first. Its run fails at its
/// entry: a condition (its count given back), the pair marked, the instance blocked. A recovery
/// succeeds and ends the block; `TASK` is the first candidate again and its run fails at its entry
/// again: now the pair's own, settled as a failure that accumulates (counters 1, `retry_scheduled`
/// with the rule's backoff, no condition), its count for the day kept, the instance blocked again
/// by it. After the next recovery an evaluation within the backoff dispatches `OTHER`, not `TASK`.
#[test]
fn entry_a2_a_after_a_recovery_the_pairs_own_failure_lets_another_task_run_first() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&two_tasks(3))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    assert_eq!(first.task_id, TASK, "the task comes first");
    let settled = settle_at_entry(&host, &clock, &first);
    assert_settled_at_entry(&failure_of(&settled));
    assert_eq!(blocked_by(&host).decision_id, first.decision_id);
    assert_eq!(mark(&host), Some(0), "the pair is marked");
    recover(&host);

    let second = admit_at(&host, &clock, settled.observed_at_unix_ms + 1_000, 101);
    assert_eq!(
        second.task_id, TASK,
        "the task is the first candidate again"
    );
    let settled = settle_at_entry(&host, &clock, &second);
    let failure = failure_of(&settled);
    assert_own(&failure, 1);
    assert_eq!(
        failure.disposition,
        PolicyFailureDisposition::RetryScheduled
    );
    assert_eq!(
        failure.retry_at_unix_ms,
        Some(settled.observed_at_unix_ms + BACKOFF_MS),
        "the rule's backoff"
    );
    let record = latest_value_record(&host);
    assert_eq!(
        record
            .daily_counts
            .iter()
            .map(|count| count.count)
            .sum::<u32>(),
        1,
        "the first run's count was given back, the second's is kept: {record:?}"
    );
    assert!(record.failure_streak.is_some(), "a streak: {record:?}");
    assert_eq!(
        blocked_by(&host).decision_id,
        second.decision_id,
        "the own failure blocks the instance again"
    );
    assert_eq!(mark(&host), Some(1));

    recover(&host);
    let at = settled.observed_at_unix_ms + 1_000;
    advance_manual_clock_to(&clock, at);
    let evaluation = evaluate(&host, at, 102);
    assert!(
        dispatched(&evaluation, OTHER),
        "another due task runs first: {evaluation:#?}"
    );
    assert!(
        !dispatched(&evaluation, TASK),
        "the task waits for its backoff: {evaluation:#?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// ENTRY-A2 (b): `escalation_threshold` 2. A failure at the entry (a condition), a recovery, the
/// pair's own failure (its first: `retry_scheduled`), a recovery, and at the end of its backoff the
/// pair's own failure again: its identity repeats, so its second in a row reaches the threshold and
/// pauses the pair (`repeated_failure`), which clears the mark; it blocks the instance all the same.
/// The next recovery ends that block, and an evaluation then dispatches nothing of the paused pair
/// and finds the instance unblocked.
#[test]
fn entry_a2_b_own_failures_pause_the_pair_at_its_threshold_and_a_recovery_unblocks() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&sources(2))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    let settled = settle_at_entry(&host, &clock, &first);
    assert_settled_at_entry(&failure_of(&settled));
    recover(&host);

    let second = admit_at(&host, &clock, settled.observed_at_unix_ms + 1_000, 101);
    let settled = settle_at_entry(&host, &clock, &second);
    let failure = failure_of(&settled);
    assert_own(&failure, 1);
    assert_eq!(
        failure.disposition,
        PolicyFailureDisposition::RetryScheduled
    );
    let retry_at = failure.retry_at_unix_ms.expect("its backoff");
    recover(&host);

    let third = admit_at(&host, &clock, retry_at, 102);
    assert_eq!(third.task_id, TASK);
    let settled = settle_at_entry(&host, &clock, &third);
    let paused = failure_of(&settled);
    assert_own(&paused, 2);
    assert_eq!(
        paused.failure_identity, failure.failure_identity,
        "the identity repeats"
    );
    assert_eq!(paused.disposition, PolicyFailureDisposition::PausedTask);
    assert_eq!(
        crate::policy_control::pause_cause(&paused).map(crate::codes::PauseCause::as_str),
        Some("repeated_failure")
    );
    assert_eq!(mark(&host), None, "the pause clears the mark");
    assert_eq!(
        blocked_by(&host).decision_id,
        third.decision_id,
        "the failure that pauses the pair blocks the instance all the same"
    );

    recover(&host);
    let at = settled.observed_at_unix_ms + 1_000;
    advance_manual_clock_to(&clock, at);
    let evaluation = evaluate(&host, at, 103);
    assert!(
        !dispatched(&evaluation, TASK),
        "the pair is paused: {evaluation:#?}"
    );
    assert_eq!(
        block_reason(&evaluation, TASK),
        None,
        "the instance is not blocked: {evaluation:#?}"
    );
    assert_eq!(
        host.entry_block_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's block"),
        None
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// ENTRY-A2 (c): a failure at the entry marks the pair; a recovery succeeds; the task's next run
/// starts its main package and fails in a step (a failure that accumulates, settled as before),
/// which clears the mark and blocks nothing. After another recovery and the step failure's backoff,
/// the task's next failure at the entry is a condition again (keeping the step failure's
/// counters): no mark says its entry is broken.
#[test]
fn entry_a2_c_a_run_that_started_its_main_package_clears_the_mark() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&sources(3))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    let settled = settle_at_entry(&host, &clock, &first);
    assert_settled_at_entry(&failure_of(&settled));
    assert_eq!(mark(&host), Some(0));
    recover(&host);

    let second = admit_at(&host, &clock, settled.observed_at_unix_ms + 1_000, 101);
    clock.advance(RUN_MS);
    host.settle_next_policy_run_past_its_entry_for_test()
        .expect("arm the settlement");
    let failure = failure_of(
        &host
            .record_policy_dispatch_outcome(&second.decision_id, &in_a_step())
            .expect("the settlement"),
    );
    assert_eq!(failure.condition, None);
    assert_eq!(
        failure.disposition,
        PolicyFailureDisposition::RetryScheduled
    );
    assert_eq!(mark(&host), None, "the run started its main package");
    assert_eq!(
        host.entry_block_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's block"),
        None,
        "a step failure blocks nothing"
    );
    let retry_at = failure.retry_at_unix_ms.expect("its backoff");
    recover(&host);

    let third = admit_at(&host, &clock, retry_at, 102);
    let settled = settle_at_entry(&host, &clock, &third);
    let failure = failure_of(&settled);
    assert_a_condition_at_entry(&failure);
    assert_eq!(
        (failure.consecutive_same_error, failure.escalation_streak),
        (1, 1),
        "the step failure's counters, as they were"
    );
    assert_eq!(blocked_by(&host).decision_id, third.decision_id);
    assert_eq!(mark(&host), Some(2), "marked again");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// ENTRY-A2 (d): a failure at the entry marks the pair and blocks the instance; a person's pause of
/// every instance, set and lifted, ends the block, and no recovery ran. The task's next failure at
/// the entry is a condition again (its count given back, the pair's counters none): only a
/// recovery's success is evidence against the pair.
#[test]
fn entry_a2_d_an_instance_unblocked_by_a_lifted_pause_meets_a_condition_again() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&sources(3))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    let settled = settle_at_entry(&host, &clock, &first);
    assert_settled_at_entry(&failure_of(&settled));
    assert_eq!(blocked_by(&host).decision_id, first.decision_id);
    assert_eq!(mark(&host), Some(0));

    pause_and_resume(&host);
    assert_eq!(
        host.entry_block_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's block"),
        None,
        "the lifted pause ends the block"
    );

    let second = admit_at(&host, &clock, settled.observed_at_unix_ms + 1_000, 101);
    assert_eq!(second.task_id, TASK);
    let settled = settle_at_entry(&host, &clock, &second);
    assert_settled_at_entry(&failure_of(&settled));
    assert_eq!(blocked_by(&host).decision_id, second.decision_id);
    assert_eq!(mark(&host), Some(0));
    let record = latest_value_record(&host);
    assert!(
        record.daily_counts.iter().all(|count| count.count == 0),
        "both counts given back: {record:?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// ---- The replays on a fake device.

const LEAD_STEPS: &[(&str, &str, &str)] = &[("go", "shop", "menu")];
const NOWHERE_STEPS: &[(&str, &str, &str)] = &[("leave", "shop", "done")];
const BACK_STEPS: &[(&str, &str, &str)] = &[("back", "away", "menu")];
const MANUAL: &str = "neutral.test.manual";

/// `lead`, a page graph from `shop` to `menu` that declares nothing: the main package's
/// prerequisite layer. On `away`, a screen none of the instance's packages knows, its run meets an
/// unknown page.
fn lead() -> Pack<'static> {
    route(LEAD, "shop", "menu", LEAD_STEPS, None)
}

/// A replay's host: the instance-workers ladder fixture's scheduled run (its request binding
/// `return_home`, the scheduled package its startup package) of `main`, a page graph from `menu` to
/// `done` declaring `lead`, listed in the game's standard pack (no return-home package named); the
/// fake device shows `away` until its first input and `menu` after it. The catalog fixture's run is
/// admitted. `clock`: on a manual Runtime clock at the fixture's instant (each capture moves it a
/// millisecond), else on the system clock.
struct Replay {
    root: TempDir,
    host: RuntimeHost,
    clock: Option<Arc<ManualRuntimeClock>>,
    context: Box<PolicyRunContext>,
    request: ContainedTaskRequest,
}

fn replay(return_home: &Pack<'_>, manual_clock: bool) -> Replay {
    let root = TempDir::new().expect("tempdir");
    let main = route(MAIN, "menu", "done", OPEN, Some(LEAD));
    let (host_config, request) =
        ladder_setup_with_packages(root.path(), &main.zip(), &return_home.zip());
    let mut standard = StandardRoot::new(root.path());
    lead().write(&standard.list(LEAD));
    standard.write_bundle();
    let state = Arc::new(FakeState::default());
    screens(&state, vec![AWAY, MENU]);
    state.physical_task_geometry.store(true, Ordering::Release);
    let mut host_config = host_config.with_standard_packs_root(Some(standard.root.clone()));
    let clock = manual_clock.then(|| Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0)));
    if let Some(clock) = &clock {
        host_config = host_config.with_runtime_clock(clock.clone());
        let stepped = Arc::clone(clock);
        *state.capture_hook.lock().expect("capture hook") =
            Some(Box::new(move || stepped.advance(1)));
    }
    let host = RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("physical runtime host");
    let context = admit_ladder_run(&host, &request);
    Replay {
        root,
        host,
        clock,
        context,
        request,
    }
}

impl Replay {
    /// Runs the scheduled task on `away`: it fails in its prerequisite layer (its record names
    /// `contained_task_page_unknown`), before any input.
    fn fail_at_entry(&self) {
        let error = self
            .host
            .run_scheduled_contained_task(&self.context, &self.request)
            .expect_err("the scheduled run fails at its entry");
        assert!(!error.is_fatal(), "{}", error.code());
    }

    /// The run's execution record.
    fn execution(&self) -> PolicyFailureRecord {
        let decision_id = self.context.decision_id();
        all_events(&self.host)
            .iter()
            .find_map(|event| match event.payload() {
                EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload))
                    if payload.decision_id() == decision_id =>
                {
                    match payload.policy_result() {
                        PolicyExecutionOutcome::Failed { failure } => Some(failure.clone()),
                        PolicyExecutionOutcome::Succeeded { .. }
                        | PolicyExecutionOutcome::Stopped { .. } => None,
                    }
                }
                _ => None,
            })
            .expect("the run's failed execution record")
    }

    /// The ladders started for the run, with their trigger item.
    fn ladders(&self) -> Vec<Option<String>> {
        let run_id = self.context.run_id();
        phases(&all_events(&self.host))
            .into_iter()
            .filter_map(|(_, phase)| match phase {
                RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, .. }
                    if trigger.run_id == Some(run_id) =>
                {
                    Some(trigger.rule)
                }
                _ => None,
            })
            .collect()
    }

    fn manual_clock(&self) -> &ManualRuntimeClock {
        self.clock.as_deref().expect("a manual clock")
    }

    fn now_unix_ms(&self) -> u64 {
        self.manual_clock()
            .sample()
            .expect("the manual clock")
            .unix_ms
    }

    fn close(self) {
        assert!(self.host.fatal_error().expect("runtime health").is_none());
        self.host.close().expect("close the host");
        drop(self.root);
    }
}

/// The outcomes of the `runtime.failed` records whose code is `code`.
fn failed_outcomes(host: &RuntimeHost, code: HostCode) -> Vec<Outcome> {
    all_events(host)
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.outcome().code().as_str() == code.as_str() =>
            {
                Some(record.outcome().clone())
            }
            _ => None,
        })
        .collect()
}

/// A client's run of a package from `away` to `menu` on the policy instance: its receipt.
fn manual_run(host: &RuntimeHost, root: &Path) -> RuntimeReceipt {
    let path = root.join("manual");
    route(MANUAL, "away", "menu", BACK_STEPS, None).write(&path);
    let task_request =
        ContainedTaskRequest::new(path.display().to_string()).expect("the task request");
    let mut client = TestClient::connect(host);
    client.set_receipt_read_timeout();
    let holder = client.ids.mint_holder_id().expect("holder");
    let request = client.request(RuntimeOperation::run_contained_task(
        POLICY_INSTANCE_ALIAS,
        holder,
        task_request,
    ));
    client.send(&request)
}

/// T1, T3, T4, the replay on a fake device (system clock): the game is on `away`, which no package
/// of the instance knows. The scheduled run fails in its prerequisite layer with
/// `contained_task_page_unknown`, its main package having executed no step: its record is a
/// condition of the instance, and the instance is blocked by it. Its lease end enters the ladder as
/// a failed run's does (`pack_logic`, once). The ladder cannot recover (the return-home package does
/// not know `away` either, the startup package fails as the run did, no emulator control): it ends
/// `exhausted`, the instance stays blocked and one `instance_entry_blocked_awaiting_person` names
/// the run, its failure and since when. A client's run on the blocked instance is not refused; it
/// leaves `away` for `menu` and succeeds, and its success ends the block.
#[test]
fn entry_a_t1_t3_t4_a_failure_at_the_entry_blocks_until_a_run_succeeds_and_a_failed_ladder_tells_a_person()
 {
    let replay = replay(
        &route(RETURN_HOME, "shop", "done", NOWHERE_STEPS, None),
        false,
    );
    replay.fail_at_entry();
    let failure = replay.execution();
    assert_settled_at_entry(&failure);
    let (gate, _, held) = replay
        .host
        .entry_block_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the instance's block")
        .expect("the instance is blocked");
    assert_eq!(
        (gate.task_id.as_str(), gate.decision_id.as_str()),
        (TASK, replay.context.decision_id())
    );
    assert_eq!(gate.failure_code.as_str(), PAGE_UNKNOWN);
    assert_eq!(held, None, "the ladder was admitted, nothing held");

    wait_for_ladder_end(&replay.host, POLICY_INSTANCE_ALIAS);
    assert_eq!(
        replay.ladders(),
        vec![Some("pack_logic".to_owned())],
        "the failure enters the ladder as a failed run's does"
    );
    let events = all_events(&replay.host);
    assert!(
        matches!(
            ladder_finished(&events),
            Some((_, RecoveryLadderOutcome::Exhausted, _))
        ),
        "the ladder cannot recover"
    );
    let (_, awaiting_person, _) = replay
        .host
        .entry_block_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the instance's block")
        .expect("the instance stays blocked");
    assert!(awaiting_person);
    let attentions = failed_outcomes(&replay.host, HostCode::InstanceEntryBlockedAwaitingPerson);
    let [attention] = attentions.as_slice() else {
        panic!("one attention: {attentions:#?}");
    };
    assert_eq!(attention.value("task_id"), Some(&json!(TASK)));
    assert_eq!(
        attention.value("decision_id"),
        Some(&json!(replay.context.decision_id()))
    );
    assert_eq!(attention.value("failure_code"), Some(&json!(PAGE_UNKNOWN)));

    let receipt = manual_run(&replay.host, replay.root.path());
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "a client's run on a blocked instance is not refused: {receipt:?}"
    );
    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::ContainedTaskCompleted {
                task_result: TaskOutcome::Success,
                ..
            })
        ),
        "{receipt:?}"
    );
    assert_eq!(
        replay
            .host
            .entry_block_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's block"),
        None,
        "the run's success ends the block"
    );
    assert_eq!(
        failed_outcomes(&replay.host, HostCode::InstanceEntryBlockedAwaitingPerson).len(),
        1,
        "the attention was raised once"
    );
    replay.close();
}

/// T3 (manual Runtime clock): the ladder's cool-down window has ten minutes left when the run fails
/// at its entry. Its ladder is suppressed (`cooldown`) and not dropped: the block holds it until
/// the window's end. A millisecond before, the sweep's tick takes up nothing; at the end it is
/// admitted as without the cool-down, its trigger the failed run's; its return-home rung leaves
/// `away` for `menu` and recovers, the ladder ends `recovered` and the block with it. The due task
/// is dispatched at its next occurrence.
#[test]
fn entry_a_t3_a_recovery_the_cooldown_held_back_is_taken_up_at_its_end_and_unblocks() {
    let replay = replay(&route(RETURN_HOME, "away", "menu", BACK_STEPS, None), true);
    let clock = replay.manual_clock();
    clock.advance(1);
    let until_unix_ms = replay.now_unix_ms() + COOLDOWN_LEFT_MS;
    replay
        .host
        .open_recovery_cooldown_for_test(POLICY_INSTANCE_ALIAS, until_unix_ms)
        .expect("the ladder's cool-down window");
    replay.fail_at_entry();
    assert_settled_at_entry(&replay.execution());
    let suppressed = phases(&all_events(&replay.host))
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderSuppressed {
                reason,
                until_unix_ms,
                ..
            } => Some((reason, until_unix_ms)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        suppressed,
        vec![(RecoveryLadderSuppression::Cooldown, until_unix_ms)]
    );
    assert!(replay.ladders().is_empty(), "the cool-down held the ladder");
    let (_, _, held) = replay
        .host
        .entry_block_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the instance's block")
        .expect("the instance is blocked");
    assert_eq!(held, Some(until_unix_ms), "the block holds the recovery");

    // Business capacity is sampled a millisecond before the window's end, at the moved clock, so
    // the held recovery's admission reads a fresh sample.
    clock.advance(
        until_unix_ms
            .saturating_sub(1)
            .saturating_sub(replay.now_unix_ms()),
    );
    let sample_capacity = replay
        .host
        .capacity_sampler_for_test()
        .expect("capacity owner");
    sample_capacity().expect("capacity sample before the window's end");
    replay
        .host
        .entry_block_tick_for_test()
        .expect("the sweep's tick");
    assert!(
        replay.ladders().is_empty(),
        "nothing before the window's end"
    );
    clock.advance(1);
    replay
        .host
        .entry_block_tick_for_test()
        .expect("the sweep's tick");
    wait_until("the held recovery's ladder", || {
        !replay.ladders().is_empty()
    });
    wait_for_ladder_end(&replay.host, POLICY_INSTANCE_ALIAS);
    assert_eq!(replay.ladders(), vec![Some("pack_logic".to_owned())]);
    let events = all_events(&replay.host);
    assert!(
        matches!(
            ladder_finished(&events),
            Some((_, RecoveryLadderOutcome::Recovered, _))
        ),
        "the return-home rung recovers"
    );
    assert_eq!(
        replay
            .host
            .entry_block_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's block"),
        None,
        "the recovery's success ends the block"
    );
    assert!(failed_outcomes(&replay.host, HostCode::InstanceEntryBlockedAwaitingPerson).is_empty());

    advance_manual_clock_to(clock, NEXT_DISPATCH_UNIX_MS);
    let evaluation = evaluate(&replay.host, NEXT_DISPATCH_UNIX_MS, 300);
    assert!(
        dispatched(&evaluation, TASK),
        "the due task is dispatched again: {evaluation:#?}"
    );
    assert_eq!(block_reason(&evaluation, TASK), None);
    replay.close();
}
