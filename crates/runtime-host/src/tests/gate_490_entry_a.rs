// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #490 ENTRY-A (0.12.77; register §1.494; `contracts/policy-suspension.md`, "A failure
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
//!   entry is a condition again;
//! - (e) (the ruling of 11-21 00:22 JST) another run starts first after the recovery: the task's next
//!   failure at the entry is a condition, the mark kept; once the task's run is the first after the
//!   next recovery, its failure at the entry is its own.
//!
//! The policy-level tests settle with the host's test hook that arms the next settlement as one of a
//! run that failed at its entry, judged as the live settlement judges it (the live settlement tells
//! that from the run's own records, which the replays exercise), or as one whose main package
//! started; a recovery's success is told to the blocks as the ladder's finished fact tells them
//! (the replays run real ladders). No clock is read in this file.
//!
//! Fix round 1 (the coordinator's work order of 11-21 after the independent review of #1853):
//!
//! - F1 (item 1): under the shipped defaults, whose `exclude_codes` name the prerequisite codes and
//!   `contained_task_return_home_entry_unmatched`, a scheduled run whose prerequisite's entry, or
//!   whose return-home layer's recheck, is unmatched fails at its entry: no immediate rerun, its
//!   admission given back, the instance blocked, and the block's own ladder (`pack_logic`, by the
//!   code's class) admitted and climbed; it cannot recover, so the instance stays blocked and one
//!   person is told by `instance_entry_blocked_recovery_failed` (the ladder's end and its trigger
//!   item; the bullet "the exhausted ladder" of item 2 too, as the first replay below);
//! - F2 (item 2): a blackout that covers the instance ends the block and drops its held recovery,
//!   and none comes after the blackout; business capacity's refusal keeps the recovery held,
//!   nothing recorded, until capacity returns, when it is admitted and recovers; a fence ends the
//!   block and drops its held recovery; a block that lasts
//!   `scheduling.unavailable_probe.attention_after_ms` tells a person once
//!   (`instance_entry_blocked_awaiting_person`, one code, one cause), its recovery still held;
//! - F3 (item 3): a store update package's task of an entry-blocked instance is dispatched;
//! - F4 (item 4, the bound): once a pair's conditions in a row reach its `escalation_threshold`, its
//!   next failure at the entry after a recovery is its own though another run started first.
//!
//! Fix round 2 (the coordinator's work order of 11-21 after the second independent review; the
//! policy-level tests tell a recovery whether its climb restarted the application, and a run's
//! main package that started, as the ladder's end and the settlement tell them):
//!
//! - R1 (ruling 1): a capture backend failure in a prerequisite layer of a scheduled run under the
//!   shipped defaults is no failure at the entry: settled as before this slice (no mark, no block,
//!   no refund), its ladder the one its trigger item starts;
//! - R2 (ruling 2): the first block climbs as configured; one set with nothing let in since the
//!   previous block ended climbs from the application restart; one after a run got in as
//!   configured;
//! - R3 (ruling 3; the production path, which F5 of fix round 1 replayed): after a recovery at the
//!   return-home rung, the same task's next failure at its entry is a condition and its block's
//!   recovery climbs from the application restart; after a recovery that restarted the
//!   application, it is the pair's own;
//! - R4 (ruling 4): the conditions in a row start again in a new activity window; the bound holds
//!   with another run let in and no recovery; it does not with neither;
//! - R5 (ruling 5): an exhausted climb tells a person once, waits, and climbs again by itself, and
//!   the second climb recovers; a panicked worker tells a person and nothing climbs again;
//! - R6 (ruling 6): two blocks chained by a recovery that let nothing in are one stretch: a person
//!   is told once, at the stretch's threshold;
//! - R7 (ruling 7): a claim the queue refuses for many ticks is tried again on the growing
//!   interval, its refusal recorded once.

use super::gate_373_nav3a::{
    AWAY, DONE, LEAD, MAIN, MENU, OPEN, Pack, RETURN_HOME, SHOP, StandardRoot, route, screens,
};
use super::gate_386_sr2b1::{admit, failure_of, start};
use super::gate_386_sr4a3::{once, silence_manifest};
use super::instance_workers::{
    admit_ladder_run, all_events, ladder_finished, ladder_setup_with_packages, phases,
    rung_outcomes, wait_for_ladder_end, wait_until,
};
use super::*;
use crate::codes::HostCode;
use crate::failure_identity::{FailureIdentity, IdentityFrame, IdentityLayer, IdentityPackage};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{LadderClass, Outcome};
use actingcommand_contract::{
    ConfigSettingChange, PolicyControlStateRecord, PolicyFailureRecord, RecoveryLadderOutcome,
    RecoveryLadderSuppression, RecoveryLadderTrigger, RecoveryRung, RecoveryRungOutcome,
    RecoveryRungPlan, RecoveryRungSkipReason, RecoveryRungState, RecoveryTriggerMatch,
    RuntimeConfigManifest, RuntimeLifecyclePhase, RuntimePayload,
};
use actingcommand_host_metrics::{
    CapacitySample, CapacityTarget, HostSample, HostSampler, ProcessLoadThresholds,
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
/// F1: two codes of class `pack_logic_mismatch` that the shipped `exclude_codes` name.
const PREREQUISITE_UNMATCHED: &str = "contained_task_prerequisite_entry_unmatched";
const RETURN_HOME_UNMATCHED: &str = "contained_task_return_home_entry_unmatched";
/// F1 (b): the scheduled run's bound recovery package, not the game's return-home package.
const BOUND: &str = "neutral.test.bound_return_home";
/// F2: the shipped `scheduling.unavailable_probe.attention_after_ms`.
const ATTENTION_AFTER_MS: u64 = 3_600_000;
/// F3: the store update task, its procedure and the ordinary task's procedure (gate_upd1's).
const STORE_TASK: &str = "t_store_update";
const STORE_PROCEDURE: &str = "procedure.store_update";
const UPDATE_KEY: &str = "health.application.update_required";
const MINUTE_MS: u64 = 60_000;
/// F3: past the longest interval the catalog fixture's activity profile samples after an
/// admission (five minutes) from the ordinary task's admission.
const ACTIVITY_INTERVAL_PAST_MS: u64 = 10 * MINUTE_MS;
/// R4 (ruling 4a): a day, so the next admission is in the next activity window of the budget
/// fixture's profile (one window a day).
const DAY_MS: u64 = 86_400_000;
/// R3, R5: the instance's own startup package.
const STARTUP: &str = "neutral.test.startup";
/// R3, R5: the shipped `scheduling.unavailable_probe.first_interval_ms`, the wait before a
/// block's climb after its first exhausted one.
const FIRST_INTERVAL_MS: u64 = 60_000;

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
    assert_settled_at_entry_with(failure, PAGE_UNKNOWN);
}

/// [`assert_settled_at_entry`] of a failure whose code is `code`.
fn assert_settled_at_entry_with(failure: &PolicyFailureRecord, code: &str) {
    assert_condition_at_entry_with(failure, code);
    assert_eq!(
        (failure.consecutive_same_error, failure.escalation_streak),
        (0, 0)
    );
}

/// [`assert_settled_at_entry`] but for the pair's counters, which a condition keeps as the pair's
/// latest real failure left them.
fn assert_a_condition_at_entry(failure: &PolicyFailureRecord) {
    assert_condition_at_entry_with(failure, PAGE_UNKNOWN);
}

/// [`assert_a_condition_at_entry`] of a failure whose code is `code`.
fn assert_condition_at_entry_with(failure: &PolicyFailureRecord, code: &str) {
    let condition = failure.condition.expect("a condition");
    assert!(condition.refunded, "{failure:?}");
    assert_eq!(condition.count, 1, "{failure:?}");
    assert_eq!(failure.disposition, PolicyFailureDisposition::Continue);
    assert_eq!(failure.effective_class, PolicyFailureClass::Recoverable);
    assert_eq!((failure.retry_attempt, failure.retry_at_unix_ms), (0, None));
    assert_eq!(crate::policy_control::pause_cause(failure), None);
    assert_eq!(failure.outcome.code().as_str(), code);
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

/// A recovery of the policy instance succeeded after restarting the application (fix round 2,
/// ruling 3: the recovery that is evidence against a pair): the instance is not blocked after it.
fn recover(host: &RuntimeHost) {
    recover_by(host, true);
}

/// Fix round 2 (ruling 3): a recovery of the policy instance succeeded at the return-home rung,
/// without restarting the application: it ends the block and is no evidence.
fn recover_at_return_home(host: &RuntimeHost) {
    recover_by(host, false);
}

fn recover_by(host: &RuntimeHost, restarted: bool) {
    host.recover_instance_for_test(POLICY_INSTANCE_ALIAS, restarted)
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

/// ENTRY-A2 (e; the coordinator's ruling of 11-21 00:22 JST): a failure at the entry marks the pair;
/// a recovery succeeds, and another task's run starts on the instance first (and succeeds); the
/// task's next failure at the entry did not start from a recovered instance, so it is a condition
/// as a first one is: no streak, its count given back, the instance blocked, the mark still set.
/// After the next recovery the task's run is the first on the instance, and its failure at the
/// entry is its own. (The settlement here is judged against the instance's marks at the
/// settlement, which stand for the run's start: only the other run's start is told.)
#[test]
fn entry_a2_e_a_failure_after_another_run_started_first_is_a_condition_and_the_mark_stays() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&sources(2))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    let settled = settle_at_entry(&host, &clock, &first);
    assert_settled_at_entry(&failure_of(&settled));
    assert_eq!(mark(&host), Some(0));
    recover(&host);
    assert_eq!(
        host.entry_runs_started_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the runs started"),
        0
    );

    host.start_run_for_test(POLICY_INSTANCE_ALIAS)
        .expect("another task's run starts first");
    let second = admit_at(&host, &clock, settled.observed_at_unix_ms + 1_000, 101);
    let settled = settle_at_entry(&host, &clock, &second);
    assert_settled_at_entry(&failure_of(&settled));
    let record = latest_value_record(&host);
    assert!(
        record.daily_counts.iter().all(|count| count.count == 0),
        "its count given back: {record:?}"
    );
    assert!(record.failure_streak.is_none(), "no streak: {record:?}");
    assert_eq!(blocked_by(&host).decision_id, second.decision_id);
    assert_eq!(mark(&host), Some(1), "the mark stays set");

    recover(&host);
    let third = admit_at(&host, &clock, settled.observed_at_unix_ms + 1_000, 102);
    let settled = settle_at_entry(&host, &clock, &third);
    let failure = failure_of(&settled);
    assert_own(&failure, 1);
    assert_eq!(
        failure.disposition,
        PolicyFailureDisposition::RetryScheduled,
        "the first own failure, below the threshold of 2"
    );
    assert_eq!(blocked_by(&host).decision_id, third.decision_id);
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
    /// Fix round 2: the fake device's state.
    state: Arc<FakeState>,
}

/// Fix round 2: what a replay's host is configured with beyond the ladder fixture's: the
/// configuration actingd assembles (`None`: the test host's), and the instance's startup package
/// when it is not the scheduled package.
#[derive(Default)]
struct Setup<'a> {
    manifest: Option<RuntimeConfigManifest>,
    startup: Option<&'a Pack<'a>>,
}

fn replay(return_home: &Pack<'_>, manual_clock: bool) -> Replay {
    replay_configured(return_home, manual_clock, Setup::default())
}

/// [`replay`] on a host `setup` configures (fix round 2).
fn replay_configured(return_home: &Pack<'_>, manual_clock: bool, setup: Setup<'_>) -> Replay {
    replay_set_up(
        &Scene {
            return_home,
            lead: &lead(),
            registry_return_home: None,
            screens: vec![AWAY, MENU],
        },
        manual_clock,
        setup,
    )
}

/// Fix round 2: the configuration actingd assembles from the shipped defaults, with the
/// `recovery_ladder` section's members `ladder` (no blackout of its own: the host list is set
/// while the Runtime runs).
fn shipped<'a>(ladder: Value) -> Setup<'a> {
    Setup {
        manifest: Some(silence_manifest(ladder, json!([]), &[])),
        startup: None,
    }
}

/// What a replay's game holds (fix round 1): the scheduled run's bound recovery package (the
/// ladder's return-home rung), the main package's prerequisite package `lead` and, when the game
/// names one, its return-home package, both listed in its standard pack, and the screens by input
/// count.
struct Scene<'a> {
    return_home: &'a Pack<'a>,
    lead: &'a Pack<'a>,
    registry_return_home: Option<&'a Pack<'a>>,
    screens: Vec<[u8; 3]>,
}

/// [`replay`] of `scene`.
fn replay_with(scene: &Scene<'_>, manual_clock: bool) -> Replay {
    replay_set_up(scene, manual_clock, Setup::default())
}

/// [`replay_with`] on a host `setup` configures (fix round 2).
fn replay_set_up(scene: &Scene<'_>, manual_clock: bool, setup: Setup<'_>) -> Replay {
    let root = TempDir::new().expect("tempdir");
    let main = route(MAIN, "menu", "done", OPEN, Some(LEAD));
    let (mut host_config, request) =
        ladder_setup_with_packages(root.path(), &main.zip(), &scene.return_home.zip());
    if let Some(startup) = setup.startup {
        let path = root.path().join("startup-package.zip");
        fs::write(&path, startup.zip()).expect("write the startup package");
        let startup = ContainedTaskRequest::new(path.to_string_lossy().into_owned())
            .expect("the startup package");
        host_config = host_config.with_startup_packages(BTreeMap::from([(
            POLICY_INSTANCE_ALIAS.to_owned(),
            startup,
        )]));
    }
    if let Some(manifest) = setup.manifest {
        host_config = host_config.with_config_manifest(manifest);
    }
    let mut standard = StandardRoot::new(root.path());
    scene.lead.write(&standard.list(LEAD));
    if let Some(registry_return_home) = scene.registry_return_home {
        registry_return_home.write(&standard.list(registry_return_home.package_id));
        standard.return_home(registry_return_home.package_id);
    }
    standard.write_bundle();
    let state = Arc::new(FakeState::default());
    screens(&state, scene.screens.clone());
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
        state,
    }
}

impl Replay {
    /// Runs the scheduled task on `away`: it fails in its prerequisite layer (its record names
    /// `contained_task_page_unknown`), before any input.
    fn fail_at_entry(&self) {
        self.fail_at_entry_of(&self.context);
    }

    /// Runs the scheduled task admitted as `context`; it fails at its entry.
    fn fail_at_entry_of(&self, context: &PolicyRunContext) {
        let error = self
            .host
            .run_scheduled_contained_task(context, &self.request)
            .expect_err("the scheduled run fails at its entry");
        assert!(!error.is_fatal(), "{}", error.code());
    }

    /// The run's execution record.
    fn execution(&self) -> PolicyFailureRecord {
        self.execution_of(self.context.decision_id())
    }

    /// The execution record of the dispatch `decision_id`, which failed.
    fn execution_of(&self, decision_id: &str) -> PolicyFailureRecord {
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
        self.ladder_triggers()
            .into_iter()
            .map(|trigger| trigger.rule)
            .collect()
    }

    /// The triggers of the ladders started for the run.
    fn ladder_triggers(&self) -> Vec<RecoveryLadderTrigger> {
        let run_id = self.context.run_id();
        phases(&all_events(&self.host))
            .into_iter()
            .filter_map(|(_, phase)| match phase {
                RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, .. }
                    if trigger.run_id == Some(run_id) =>
                {
                    Some(trigger)
                }
                _ => None,
            })
            .collect()
    }

    /// The instance's entry block, which must be set: its gate, whether a person was told and when
    /// its held recovery is due.
    fn block(&self) -> (crate::entry_block::EntryBlockGate, bool, Option<u64>) {
        self.host
            .entry_block_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's block")
            .expect("the instance is blocked")
    }

    /// Whether the instance is entry-blocked.
    fn blocked(&self) -> bool {
        self.host
            .entry_block_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's block")
            .is_some()
    }

    /// One tick of the entry blocks at the host clock, as the lease sweep runs it.
    fn tick(&self) {
        self.host
            .entry_block_tick_for_test()
            .expect("the sweep's tick");
    }

    /// Fix round 2 (manual clock): the clock moved to `at`, business capacity sampled there (so an
    /// admission reads a fresh sample), and one tick of the entry blocks.
    fn tick_at(&self, at: u64) {
        advance_manual_clock_to(self.manual_clock(), at);
        self.host
            .capacity_sampler_for_test()
            .expect("capacity owner")()
        .expect("capacity sample at the tick");
        self.tick();
    }

    /// Fix round 2: when the block's held recovery is due, which must be held.
    fn held_until(&self) -> u64 {
        let (_, _, held) = self.block();
        held.expect("the block holds a recovery")
    }

    /// Fix round 2 (ruling 2): whether the instance's block climbs from the application restart.
    fn restarts(&self) -> Option<bool> {
        self.host
            .entry_block_restarts_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's block")
    }

    /// Fix round 1, F2 (manual clock): the ladder's cool-down window has `window_ms` left when the
    /// run fails at its entry, so the block holds the run's recovery until the window's end, which
    /// it returns.
    fn hold_a_recovery(&self, window_ms: u64) -> u64 {
        self.manual_clock().advance(1);
        let until_unix_ms = self.now_unix_ms() + window_ms;
        self.host
            .open_recovery_cooldown_for_test(POLICY_INSTANCE_ALIAS, until_unix_ms)
            .expect("the ladder's cool-down window");
        self.fail_at_entry();
        assert_settled_at_entry(&self.execution());
        let (_, awaiting_person, held) = self.block();
        assert!(!awaiting_person);
        assert_eq!(held, Some(until_unix_ms), "the block holds the recovery");
        until_unix_ms
    }

    /// Fix round 1, F1: the scheduled run fails at its entry with `code`, of class
    /// `pack_logic_mismatch`, which the shipped `exclude_codes` name. Its record is a condition (no
    /// retry, so no immediate rerun; its admission given back; identity `…~fi…`), and the instance
    /// is blocked by it. The exclusion starts no ladder at the run's lease end; the block's own
    /// ladder is admitted at the lease sweep's next tick under the item that takes the code's
    /// class, `pack_logic`, its trigger the failed run's. No package of the instance knows the
    /// screen the run left, so it ends `exhausted`: the instance stays blocked, one person told
    /// (`instance_entry_blocked_recovery_failed`).
    fn assert_an_excluded_failure_blocks_and_starts_the_blocks_ladder(&self, code: &str) {
        self.fail_at_entry();
        let failure = self.execution();
        assert_settled_at_entry_with(&failure, code);
        let record = latest_value_record(&self.host);
        assert!(
            record.daily_counts.iter().all(|count| count.count == 0),
            "the admission is given back: {record:?}"
        );
        assert!(record.failure_streak.is_none(), "no streak: {record:?}");
        wait_until("the block's own ladder", || !self.ladders().is_empty());
        wait_for_ladder_end(&self.host, POLICY_INSTANCE_ALIAS);
        let triggers = self.ladder_triggers();
        let [trigger] = triggers.as_slice() else {
            panic!("one ladder for the run: {triggers:?}");
        };
        assert_eq!(trigger.rule.as_deref(), Some("pack_logic"), "{trigger:?}");
        assert_eq!(trigger.failure_code.as_str(), code, "{trigger:?}");
        assert_eq!(
            trigger.matched,
            Some(RecoveryTriggerMatch::Class {
                ladder_class: LadderClass::PackLogicMismatch
            }),
            "{trigger:?}"
        );
        assert!(
            matches!(
                ladder_finished(&all_events(&self.host)),
                Some((_, RecoveryLadderOutcome::Exhausted, _))
            ),
            "no package knows the screen"
        );
        let (gate, told, _) = self.block();
        assert_eq!(
            (gate.decision_id.as_str(), gate.failure_code.as_str()),
            (self.context.decision_id(), code)
        );
        assert!(told);
        let failed = the_exhausted_recovery(&self.host);
        assert_eq!(failed.value("failure_code"), Some(&json!(code)));
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

/// Fix round 1 (one code, one cause): the one `instance_entry_blocked_recovery_failed` of `host`,
/// the block's own `pack_logic` ladder ended `exhausted`; no `instance_entry_blocked_awaiting_person`
/// (the block has not lasted `attention_after_ms`).
fn the_exhausted_recovery(host: &RuntimeHost) -> Outcome {
    let failures = failed_outcomes(host, HostCode::InstanceEntryBlockedRecoveryFailed);
    let [failed] = failures.as_slice() else {
        panic!("one recovery that ended without recovering: {failures:#?}");
    };
    assert_eq!(
        failed.value("entry_recovery_end"),
        Some(&json!("exhausted"))
    );
    assert_eq!(failed.value("trigger_rule"), Some(&json!("pack_logic")));
    assert!(
        failed_outcomes(host, HostCode::InstanceEntryBlockedAwaitingPerson).is_empty(),
        "the threshold has not passed"
    );
    failed.clone()
}

/// Fix round 1: neither warning of an entry-blocked instance was raised on `host`.
fn assert_no_one_told(host: &RuntimeHost) {
    for code in [
        HostCode::InstanceEntryBlockedAwaitingPerson,
        HostCode::InstanceEntryBlockedRecoveryFailed,
    ] {
        let told = failed_outcomes(host, code);
        assert!(told.is_empty(), "{told:#?}");
    }
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
/// `exhausted`, the instance stays blocked and one `instance_entry_blocked_recovery_failed` names
/// the run, its failure, the ladder's end and its trigger item (fix round 1, one code, one cause;
/// no `instance_entry_blocked_awaiting_person`). A client's run on the blocked instance is not
/// refused; it leaves `away` for `menu` and succeeds, and its success ends the block.
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
    let (_, told, _) = replay
        .host
        .entry_block_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the instance's block")
        .expect("the instance stays blocked");
    assert!(told);
    let attention = the_exhausted_recovery(&replay.host);
    assert_eq!(attention.value("task_id"), Some(&json!(TASK)));
    assert_eq!(
        attention.value("decision_id"),
        Some(&json!(replay.context.decision_id()))
    );
    assert_eq!(attention.value("failure_code"), Some(&json!(PAGE_UNKNOWN)));

    let started_before = replay
        .host
        .entry_runs_started_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the runs started");
    let receipt = manual_run(&replay.host, replay.root.path());
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "a client's run on a blocked instance is not refused: {receipt:?}"
    );
    assert_eq!(
        replay
            .host
            .entry_runs_started_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the runs started"),
        started_before + 1,
        "ENTRY-A2: a client's run counts as a run started since the latest recovery"
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
        failed_outcomes(&replay.host, HostCode::InstanceEntryBlockedRecoveryFailed).len(),
        1,
        "the recovery's failure was raised once"
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
    assert_eq!(
        replay
            .host
            .entry_runs_started_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the runs started"),
        1,
        "fix round 2 (ruling 3): a recovery at the return-home rung is no evidence and leaves the \
         runs counted as they were (the scheduled run; its rung's run is the Runtime's own)"
    );
    assert_no_one_told(&replay.host);

    advance_manual_clock_to(clock, NEXT_DISPATCH_UNIX_MS);
    let evaluation = evaluate(&replay.host, NEXT_DISPATCH_UNIX_MS, 300);
    assert!(
        dispatched(&evaluation, TASK),
        "the due task is dispatched again: {evaluation:#?}"
    );
    assert_eq!(block_reason(&evaluation, TASK), None);
    replay.close();
}

// ---- Fix round 1.

/// `lead`'s step from `shop` to `done` (F1 a), and `back`'s from `away` to `done` (F1 b, F5).
const TO_DONE: &[(&str, &str, &str)] = &[("go", "shop", "done")];
const AWAY_TO_DONE: &[(&str, &str, &str)] = &[("back", "away", "done")];

/// Every `recovery_ladder_suppressed` of the host as (reason, until).
fn suppressions(host: &RuntimeHost) -> Vec<(RecoveryLadderSuppression, u64)> {
    phases(&all_events(host))
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderSuppressed {
                reason,
                until_unix_ms,
                ..
            } => Some((reason, until_unix_ms)),
            _ => None,
        })
        .collect()
}

/// `ConfigureSettings` of one key while the Runtime runs, as a client sends it.
fn configure(host: &RuntimeHost, key: &str, value: Value) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(
                &ids,
                RuntimeOperation::ConfigureSettings {
                    expected_version: None,
                    changes: vec![ConfigSettingChange {
                        key: key.to_owned(),
                        value: Some(value),
                    }],
                },
            ),
            ConnectionId::new(4_901).expect("connection"),
        )
        .expect("configure receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

/// F1 (a), item 1 of the fix round (the shipped defaults, system clock): the game is on `shop`.
/// `main`'s entry `menu` does not match, its declared prerequisite `lead` runs from `shop` and
/// ends on `done`, and `main`'s entry is still not seen within its step timeout:
/// `contained_task_prerequisite_entry_unmatched`, which the shipped `exclude_codes` name
/// (`contained_task_prerequisite_*`). The run failed at its entry: a condition (no immediate
/// rerun, its admission given back), the instance blocked, and the block's own ladder admitted
/// under `pack_logic` by the code's class; it cannot recover (the bound return-home package and
/// the startup package do not know `done`), so the instance stays blocked and one person is told.
#[test]
fn entry_a_f1_a_prerequisite_entry_unmatched_under_the_shipped_defaults_blocks_and_starts_its_ladder()
 {
    let replay = replay_with(
        &Scene {
            return_home: &route(RETURN_HOME, "away", "menu", BACK_STEPS, None),
            lead: &route(LEAD, "shop", "done", TO_DONE, None),
            registry_return_home: None,
            screens: vec![SHOP, DONE],
        },
        false,
    );
    replay.assert_an_excluded_failure_blocks_and_starts_the_blocks_ladder(PREREQUISITE_UNMATCHED);
    replay.close();
}

/// F1 (b), item 1 of the fix round (the shipped defaults, system clock): the game is on `away`.
/// `main`'s entry `menu` does not match; its declared prerequisite `lead` (entry `{shop}`, no
/// prerequisite of its own) does not match either, so the game's return-home package `back` runs
/// as `lead`'s layer, from `away` to `done`, and `lead`'s recheck does not see `shop`:
/// `contained_task_return_home_entry_unmatched`, which the shipped `exclude_codes` name. As F1 (a):
/// a condition, the instance blocked, the block's own ladder by `pack_logic`, exhausted, one
/// person told.
#[test]
fn entry_a_f1_b_return_home_entry_unmatched_under_the_shipped_defaults_blocks_and_starts_its_ladder()
 {
    let lead = Pack {
        package_id: LEAD,
        mode: "navigable_route",
        prerequisite: None,
        entry: json!(["shop"]),
        target: "menu",
        operations: LEAD_STEPS,
    };
    let replay = replay_with(
        &Scene {
            return_home: &route(BOUND, "away", "menu", BACK_STEPS, None),
            lead: &lead,
            registry_return_home: Some(&route(RETURN_HOME, "away", "done", AWAY_TO_DONE, None)),
            screens: vec![AWAY, DONE],
        },
        false,
    );
    replay.assert_an_excluded_failure_blocks_and_starts_the_blocks_ladder(RETURN_HOME_UNMATCHED);
    replay.close();
}

/// F2, a blackout (item 2, first bullet; manual clock; fix round 2: on the configuration actingd
/// assembles from the shipped defaults, whose key table holds `scheduling.blackout.host`): the
/// block holds the run's recovery for the cool-down's end; a blackout that covers the instance
/// then starts. The sweep's tick ends the block and drops the recovery: nothing is dispatched
/// inside the blackout, and after it no ladder of that trigger comes (SR-4a3), past the window's
/// end and the blackout's. Fix round 2 (ruling 9): the drop is recorded as AVAIL-1 records every
/// recovery it drops: the trigger's `recovery_ladder_suppressed` `cooldown` once more, until the
/// time it was held to.
#[test]
fn entry_a_f2_a_covering_blackout_ends_the_block_and_drops_its_held_recovery() {
    let replay = replay_configured(
        &route(RETURN_HOME, "away", "menu", BACK_STEPS, None),
        true,
        shipped(json!({})),
    );
    let until_unix_ms = replay.hold_a_recovery(COOLDOWN_LEFT_MS);
    let blackout_until = until_unix_ms + COOLDOWN_LEFT_MS;
    configure(
        &replay.host,
        "scheduling.blackout.host",
        json!([once("entry.blackout", replay.now_unix_ms(), blackout_until)]),
    );
    replay.tick();
    assert!(!replay.blocked(), "the blackout ends the block");
    assert_eq!(
        suppressions(&replay.host),
        vec![
            (RecoveryLadderSuppression::Cooldown, until_unix_ms),
            (RecoveryLadderSuppression::Cooldown, until_unix_ms),
        ],
        "the cool-down's suppression, and the drop's record (ruling 9)"
    );
    advance_manual_clock_to(replay.manual_clock(), blackout_until + 1);
    replay.tick();
    assert!(
        replay.ladders().is_empty(),
        "the dropped recovery never comes"
    );
    assert!(!replay.blocked());
    assert_no_one_told(&replay.host);
    replay.close();
}

/// Business capacity that refuses new work: every volume at zero bytes free.
struct PressureSampler {
    available_bytes: Arc<AtomicU64>,
}

impl HostSampler for PressureSampler {
    fn sample_capacity(&mut self, targets: &[CapacityTarget]) -> Vec<CapacitySample> {
        let available_bytes = self.available_bytes.load(Ordering::Acquire);
        actingcommand_host_metrics::sample_capacity(targets)
            .into_iter()
            .map(|mut sample| {
                if sample.available_bytes.is_ok() {
                    sample.available_bytes = Ok(available_bytes);
                }
                sample
            })
            .collect()
    }

    fn sample(
        &mut self,
        _observed_at_unix_ms: u64,
        _owned_processes: &BTreeMap<u32, String>,
        _top_process_count: usize,
        _thresholds: ProcessLoadThresholds,
    ) -> Result<HostSample, actingcommand_contract::outcome::Code> {
        panic!("the fixture enables no performance counters")
    }
}

/// F2, a transient refusal (item 2, second bullet; manual clock): business capacity refuses new
/// work when the cool-down that holds the run's recovery ends. The sweep's tick admits nothing and
/// records nothing (one suppression, the cool-down's): the recovery stays held and due, the block
/// stays, no person is told. Once capacity returns, the next tick admits it; its return-home rung
/// recovers and the block ends.
#[test]
fn entry_a_f2_a_capacity_refusal_is_tried_again_and_then_admitted() {
    let replay = replay(&route(RETURN_HOME, "away", "menu", BACK_STEPS, None), true);
    let until_unix_ms = replay.hold_a_recovery(COOLDOWN_LEFT_MS);
    let available_bytes = Arc::new(AtomicU64::new(0));
    replay
        .host
        .replace_capacity_sampler_for_test(Box::new(PressureSampler {
            available_bytes: Arc::clone(&available_bytes),
        }))
        .expect("hard pressure committed");
    advance_manual_clock_to(replay.manual_clock(), until_unix_ms);
    replay.tick();
    replay.tick();
    assert!(replay.ladders().is_empty(), "capacity refuses new work");
    let (_, awaiting_person, held) = replay.block();
    assert!(!awaiting_person, "a transient refusal tells no one");
    assert_eq!(held, Some(until_unix_ms), "the recovery stays held");
    assert_eq!(
        suppressions(&replay.host),
        vec![(RecoveryLadderSuppression::Cooldown, until_unix_ms)],
        "nothing recorded for the refusal"
    );

    available_bytes.store(
        actingcommand_contract::CapacityThresholds::default().soft_bytes,
        Ordering::Release,
    );
    replay.manual_clock().advance(1);
    replay
        .host
        .capacity_sampler_for_test()
        .expect("capacity owner")()
    .expect("capacity sample after the pressure");
    replay.tick();
    wait_until("the held recovery's ladder", || {
        !replay.ladders().is_empty()
    });
    wait_for_ladder_end(&replay.host, POLICY_INSTANCE_ALIAS);
    assert!(
        matches!(
            ladder_finished(&all_events(&replay.host)),
            Some((_, RecoveryLadderOutcome::Recovered, _))
        ),
        "the return-home rung recovers"
    );
    assert!(!replay.blocked(), "the recovery's success ends the block");
    assert_no_one_told(&replay.host);
    replay.close();
}

/// F2, a fence (item 2, fourth bullet; manual clock): the block holds the run's recovery for the
/// cool-down's end; a fence takes the instance. The fence ends the block and drops the recovery:
/// the fence and its own recheck own the instance from there; no ladder of the run's trigger
/// comes, and no person is told for the block.
#[test]
fn entry_a_f2_a_fence_ends_the_block_and_drops_its_held_recovery() {
    let replay = replay(&route(RETURN_HOME, "away", "menu", BACK_STEPS, None), true);
    replay.hold_a_recovery(COOLDOWN_LEFT_MS);
    replay
        .host
        .fence_instance_for_test(POLICY_INSTANCE_ALIAS)
        .expect("fence the instance");
    assert!(!replay.blocked(), "the fence ends the block");
    replay.tick();
    assert!(replay.ladders().is_empty(), "its held recovery was dropped");
    assert_no_one_told(&replay.host);
    replay.close();
}

/// F2, the catch-all (item 2, fifth bullet; manual clock): the cool-down holds the run's recovery
/// for twice `attention_after_ms`. A millisecond before the block has lasted
/// `scheduling.unavailable_probe.attention_after_ms` the sweep's tick tells no one; at that time
/// one `instance_entry_blocked_awaiting_person` names the run, how long the block lasted and the
/// threshold, with the recovery still held; a later tick tells no one again, and no
/// `instance_entry_blocked_recovery_failed` is raised (no recovery of the block ended). Fix round
/// 2 (ruling 6): the block is its stretch's first, and the stretch is measured on the Runtime's
/// clock from when the block was set.
#[test]
fn entry_a_f2_a_block_that_lasts_the_threshold_tells_a_person_once_with_its_recovery_held() {
    let replay = replay(&route(RETURN_HOME, "away", "menu", BACK_STEPS, None), true);
    let until_unix_ms = replay.hold_a_recovery(2 * ATTENTION_AFTER_MS);
    let (since, told) = replay
        .host
        .entry_stretch_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the instance's stretch")
        .expect("the block began a stretch");
    assert!(!told);
    advance_manual_clock_to(replay.manual_clock(), since + ATTENTION_AFTER_MS - 1);
    replay.tick();
    assert_no_one_told(&replay.host);
    replay.manual_clock().advance(1);
    replay.tick();
    let attentions = failed_outcomes(&replay.host, HostCode::InstanceEntryBlockedAwaitingPerson);
    let [attention] = attentions.as_slice() else {
        panic!("one attention: {attentions:#?}");
    };
    assert_eq!(attention.value("task_id"), Some(&json!(TASK)));
    assert_eq!(
        attention.value("decision_id"),
        Some(&json!(replay.context.decision_id()))
    );
    assert_eq!(
        attention.value("elapsed_ms"),
        Some(&json!(ATTENTION_AFTER_MS))
    );
    assert_eq!(
        attention.value("limit_ms"),
        Some(&json!(ATTENTION_AFTER_MS))
    );
    let (_, awaiting_person, held) = replay.block();
    assert!(awaiting_person);
    assert_eq!(held, Some(until_unix_ms), "the recovery is still held");
    replay.tick();
    assert_eq!(
        failed_outcomes(&replay.host, HostCode::InstanceEntryBlockedAwaitingPerson).len(),
        1,
        "once per block"
    );
    assert!(
        failed_outcomes(&replay.host, HostCode::InstanceEntryBlockedRecoveryFailed).is_empty(),
        "no recovery of the block ended: one code, one cause"
    );
    replay.close();
}

/// F3 (item 3, policy level, gate_upd1's two tasks): the ordinary task's run fails at its entry
/// and blocks the instance; the instance's game then asks for an update. The store update task is
/// dispatched although the instance is entry-blocked (its decision names no
/// `instance_entry_blocked`); the ordinary task waits; the block stays until a run succeeds.
#[test]
fn entry_a_f3_a_store_update_task_of_an_entry_blocked_instance_is_dispatched() {
    let root = TempDir::new().expect("tempdir");
    let ordinary = neutral_contained_task_package(true);
    let store = super::gate_store_fg2::store_package();
    let mut procedures = Vec::new();
    for (index, (procedure, package)) in [
        ("procedure.observe", ordinary.as_slice()),
        (STORE_PROCEDURE, store.as_slice()),
    ]
    .into_iter()
    .enumerate()
    {
        let path = root.path().join(format!("bound-{index}.zip"));
        fs::write(&path, package).expect("write the package");
        procedures.push(
            ProcedureBinding::new(
                procedure,
                "operation.observe",
                vec!["after_observation".to_owned()],
            )
            .expect("procedure binding")
            .with_package_path(path.to_string_lossy()),
        );
    }
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_procedure_manifest(ProcedureManifest::new(procedures).expect("manifest")),
        Arc::new(
            FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                instance_id(),
                Arc::new(FakeState::default()),
            )
            .fixture_simulation(),
        ),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&store_sources())
        .expect("activate the catalog");
    let evaluate_at = |at: u64, seed: u64| {
        advance_manual_clock_to(&clock, at);
        host.evaluate_policy_cycle_with_test_inputs(
            &update_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: at,
                monotonic_ms: clock.monotonic_ms(),
            },
            seed,
            PolicyTrigger::Recovery,
        )
        .expect("evaluate the policy")
        .evaluation
        .expect("a full evaluation")
    };

    let first = evaluate_at(POLICY_NOW_UNIX_MS + 1_000, 1);
    let intent = first
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == TASK)
        .cloned()
        .unwrap_or_else(|| panic!("the ordinary task: {first:#?}"));
    let chain = first
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .cloned()
        .expect("its reason chain");
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &chain, &policy_context(&host, &intent))
        .expect("the admission")
    else {
        panic!("the ordinary task is granted");
    };
    host.settle_next_policy_run_at_entry_for_test()
        .expect("arm the settlement");
    assert_settled_at_entry(&failure_of(
        &host
            .record_policy_dispatch_outcome(&intent.decision_id, &at_entry(&intent))
            .expect("the settlement"),
    ));
    host.release_policy_dispatch_lease_for_test(&context)
        .expect("end the dispatch's lease");
    assert!(
        host.entry_block_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's block")
            .is_some()
    );

    advance_manual_clock_to(&clock, POLICY_NOW_UNIX_MS + MINUTE_MS);
    host.record_application_update_for_test(POLICY_INSTANCE_ALIAS, true)
        .expect("the game asks for an update");
    // Fix round 2 (the cause of fix round 1's red run): the activity profile of the catalog
    // fixture samples an interval of one to five minutes after each admission on the instance
    // (`policy_activity_interval_active`), so the update is evaluated past it.
    let update = evaluate_at(POLICY_NOW_UNIX_MS + ACTIVITY_INTERVAL_PAST_MS, 2);
    assert!(
        dispatched(&update, STORE_TASK),
        "the store update task passes the entry block: {update:#?}"
    );
    assert_eq!(block_reason(&update, STORE_TASK), None);
    assert!(!dispatched(&update, TASK), "{update:#?}");
    assert!(
        host.entry_block_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's block")
            .is_some(),
        "the block stays until a run succeeds"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// F3: the budget fixture's task (procedure `procedure.observe`, due at every evaluation) and
/// gate_upd1's store update task, triggered while the instance's
/// `health.application.update_required` is true, on its own procedure.
fn store_sources() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("the task fixture");
    tasks["tasks"][0]["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1,
                     "anchor_ms": 0}
    });
    let mut store = tasks["tasks"][0].clone();
    store["id"] = json!(STORE_TASK);
    store["procedure_ref"] = json!(STORE_PROCEDURE);
    store["priority"] = json!(200);
    store["trigger"] = json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "fact_key": UPDATE_KEY,
        "comparison": "eq",
        "value": {"type": "boolean", "value": true},
        "max_age_ms": null
    });
    store["cooldown_ms"] = json!(30 * MINUTE_MS);
    store["produces"] = json!([]);
    store["instance_overrides"] = json!([]);
    tasks["tasks"]
        .as_array_mut()
        .expect("the tasks")
        .push(store);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("the task bytes");
    sources
}

/// F3: the fixture's facts without a scheduling outcome (the catalog references none).
fn update_facts() -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    facts
}

/// F4 (item 4, the bound; the settlement hook, threshold 2): the task's failure at the entry is a
/// condition; a recovery; another run starts first, and the task's next failure at the entry is a
/// condition again (its second in a row); a recovery; another run starts first again, and now the
/// pair's conditions in a row have reached its `escalation_threshold`: its failure at the entry is
/// its own (counters 1, `retry_scheduled`), whatever ran first, and blocks the instance.
#[test]
fn entry_a2_f4_conditions_in_a_row_at_the_threshold_make_the_next_failure_the_pairs_own() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&sources(2))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    let mut settled = settle_at_entry(&host, &clock, &first);
    assert_settled_at_entry(&failure_of(&settled));
    for seed in [101, 102] {
        recover(&host);
        host.start_run_for_test(POLICY_INSTANCE_ALIAS)
            .expect("another run starts first");
        let next = admit_at(&host, &clock, settled.observed_at_unix_ms + 1_000, seed);
        settled = settle_at_entry(&host, &clock, &next);
        if seed == 101 {
            assert_settled_at_entry(&failure_of(&settled));
            assert_eq!(mark(&host), Some(1), "the mark stays set");
        } else {
            let failure = failure_of(&settled);
            assert_own(&failure, 1);
            assert_eq!(
                failure.disposition,
                PolicyFailureDisposition::RetryScheduled
            );
            assert_eq!(blocked_by(&host).decision_id, next.decision_id);
        }
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// The clock moved to `at` (business capacity sampled there), the evaluation there, and `TASK`'s
/// dispatch admitted as a scheduled run of `request`: its context.
fn admit_scheduled_at(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    request: &ContainedTaskRequest,
    at: u64,
    seed: u64,
) -> Box<PolicyRunContext> {
    advance_manual_clock_to(clock, at);
    host.capacity_sampler_for_test().expect("capacity owner")()
        .expect("capacity sample at the admission time");
    let evaluation = evaluate(host, at, seed);
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == TASK && intent.instance_id == POLICY_INSTANCE_ALIAS)
        .cloned()
        .unwrap_or_else(|| panic!("the task is dispatched: {evaluation:#?}"));
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .cloned()
        .expect("its reason chain");
    record_policy_approval(host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_scheduled_policy_dispatch(&intent, &reasons, &policy_context(host, &intent), request)
        .expect("the admission")
    else {
        panic!("the task is granted");
    };
    context
}

// ---- Fix round 2.

/// The trigger and plan of each `recovery_ladder_started` of `host`, in order.
fn started_plans(host: &RuntimeHost) -> Vec<(RecoveryLadderTrigger, Vec<RecoveryRungPlan>)> {
    phases(&all_events(host))
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, rungs, .. } => {
                Some((trigger, rungs))
            }
            _ => None,
        })
        .collect()
}

/// The result of each `recovery_ladder_finished` of `host`, in order.
fn finished_ladders(host: &RuntimeHost) -> Vec<RecoveryLadderOutcome> {
    phases(&all_events(host))
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderFinished { ladder_result, .. } => {
                Some(ladder_result)
            }
            _ => None,
        })
        .collect()
}

/// `count` ladders finished and nothing holds the policy instance's key any more.
fn wait_for_ladders_finished(host: &RuntimeHost, count: usize) {
    wait_until("the ladders' ends and their releases", || {
        finished_ladders(host).len() >= count
            && host
                .instance_claims_for_test(POLICY_INSTANCE_ALIAS)
                .expect("instance claims")
                .0
                .is_none()
    });
}

/// One rung of a climb's plan: skipped for `reason`, else pending.
fn planned(rung: RecoveryRung, reason: Option<RecoveryRungSkipReason>) -> RecoveryRungPlan {
    RecoveryRungPlan {
        rung,
        state: if reason.is_some() {
            RecoveryRungState::Skipped
        } else {
            RecoveryRungState::Pending
        },
        reason,
    }
}

/// The plan of a climb from the application restart on a replay's instance (the shipped rungs, no
/// emulator control).
fn from_the_application_restart() -> Vec<RecoveryRungPlan> {
    vec![
        planned(
            RecoveryRung::ReturnHome,
            Some(RecoveryRungSkipReason::BeforeStartRung),
        ),
        planned(RecoveryRung::ApplicationRestart, None),
        planned(
            RecoveryRung::EmulatorRestart,
            Some(RecoveryRungSkipReason::NoEmulatorControl),
        ),
    ]
}

/// The `recovery_ladder` members of an instance whose `pack_logic` item starts its climbs at the
/// application restart (rung 2), the only trigger item.
fn restart_first() -> Value {
    json!({"triggers": [{
        "id": "pack_logic",
        "stages": ["task"],
        "classes": ["pack_logic_mismatch"],
        "start_rung": 2,
    }]})
}

/// The outcomes of the `runtime.failed` records whose code is spelled `code`.
fn failed_outcomes_coded(host: &RuntimeHost, code: &str) -> Vec<Outcome> {
    all_events(host)
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

/// The policy instance's block: whether it climbs from the application restart (ruling 2).
fn restarts(host: &RuntimeHost) -> Option<bool> {
    host.entry_block_restarts_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the instance's block")
}

/// The policy instance's stretch of blocks, if one runs: since when, and whether a person was told
/// (ruling 6).
fn stretch(host: &RuntimeHost) -> Option<(u64, bool)> {
    host.entry_stretch_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the instance's stretch")
}

/// R4: `TASK` dispatched and admitted at `at` fails at its entry: its execution record.
fn fail_at(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    at: u64,
    seed: u64,
) -> actingcommand_contract::PolicyExecutionEventData {
    let intent = admit_at(host, clock, at, seed);
    assert_eq!(intent.task_id, TASK);
    settle_at_entry(host, clock, &intent)
}

/// R1 (ruling 1; the shipped defaults, system clock): the game is on `shop`; `main`'s entry
/// `menu` is not seen, so its prerequisite `lead` runs as its layer, and the run's second capture,
/// in that layer, fails in the capture backend (`capture_backend_operation_failed`, class
/// `environment_fault`, which the shipped item `task_capture_backend` takes). It is no failure at
/// the entry, whatever the triggers say of its code: settled as before this slice, with no
/// condition (its identity's `F` unique, not `i…`), its admission kept, no mark and no block; its
/// lease end starts the ladder its trigger item names, as before.
#[test]
fn entry_r1_a_capture_backend_failure_in_a_prerequisite_layer_is_settled_as_before() {
    let replay = replay_with(
        &Scene {
            return_home: &route(RETURN_HOME, "away", "menu", BACK_STEPS, None),
            lead: &lead(),
            registry_return_home: None,
            screens: vec![SHOP, MENU, DONE],
        },
        false,
    );
    let captured = replay.state.capture_count.load(Ordering::Acquire);
    replay
        .state
        .fail_capture_on
        .store(captured + 2, Ordering::Release);
    replay.fail_at_entry();
    let failure = replay.execution();
    assert_eq!(
        failure.outcome.code().as_str(),
        SharedCode::CaptureBackendOperationFailed.as_str(),
        "{failure:?}"
    );
    assert_eq!(failure.condition, None, "{failure:?}");
    let identity = failure
        .failure_identity
        .as_deref()
        .and_then(FailureIdentity::parse)
        .expect("a v2 failure identity");
    assert!(
        !identity.at_entry() && !identity.accumulates(),
        "{identity:?}"
    );
    assert!(!crate::policy_control::entry_blocked(&failure));
    let record = latest_value_record(&replay.host);
    assert!(
        record.daily_counts.iter().any(|count| count.count > 0),
        "the admission is kept: {record:?}"
    );
    assert_eq!(mark(&replay.host), None, "no mark");
    assert!(!replay.blocked(), "no block");
    wait_for_ladder_end(&replay.host, POLICY_INSTANCE_ALIAS);
    let triggers = replay.ladder_triggers();
    let [trigger] = triggers.as_slice() else {
        panic!("the ladder its trigger item starts: {triggers:?}");
    };
    assert_eq!(trigger.rule.as_deref(), Some("task_capture_backend"));
    assert!(!replay.blocked());
    assert_no_one_told(&replay.host);
    replay.close();
}

/// R2 (ruling 2; the settlement hook, `escalation_threshold` 3): the first block since the Runtime
/// started climbs as its trigger item says. A recovery at the return-home rung ends it and lets
/// nothing in, so the task's next failure at the entry sets a block that climbs from the
/// application restart. That block ends the same way, a run's main package then starts on the
/// instance, and the task's next failure at the entry sets a block that climbs as configured.
#[test]
fn entry_r2_a_block_with_nothing_let_in_since_the_previous_one_climbs_from_the_application_restart()
{
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&sources(3))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    let mut settled = settle_at_entry(&host, &clock, &first);
    assert_eq!(
        restarts(&host),
        Some(false),
        "the first block since the Runtime started"
    );
    for (seed, let_in, from_restart) in [(101, false, true), (102, true, false)] {
        recover_at_return_home(&host);
        if let_in {
            host.let_run_in_for_test(POLICY_INSTANCE_ALIAS)
                .expect("a run's main package starts");
        }
        let next = admit_at(&host, &clock, settled.observed_at_unix_ms + 1_000, seed);
        settled = settle_at_entry(&host, &clock, &next);
        assert_settled_at_entry(&failure_of(&settled));
        assert_eq!(blocked_by(&host).decision_id, next.decision_id);
        assert_eq!(restarts(&host), Some(from_restart), "{seed}");
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// R3 (ruling 3; the live settlement on a fake device, manual clock; fix round 1's F5, moved): the
/// game is on `away`, which `lead` does not know. The scheduled run fails at its entry: a
/// condition, the pair marked. The first block since the Runtime started climbs as configured, from
/// the return-home rung: the bound return-home package leaves `away` for `done` and the ladder ends
/// `recovered` there, which ends the block and is no evidence (the runs counted since the latest
/// evidence stay as they were: the scheduled run). At the task's next occurrence (the cool-down
/// window opened so that the next ladder is held) the same task meets `done`, which `lead` does not
/// know, and fails at its entry again: a condition again, not the pair's own, the mark kept. Its
/// block was set with nothing let in since the previous one ended (ruling 2): at the window's end
/// its held recovery climbs from the application restart, the return-home rung skipped before its
/// start rung; the startup package does not know `done` either, the climb ends `exhausted`, and one
/// person is told, with the time of the block's next climb (ruling 5).
#[test]
fn entry_r3_a_a_recovery_at_the_return_home_rung_is_no_evidence_and_the_next_block_climbs_from_the_application_restart()
 {
    let replay = replay_with(
        &Scene {
            return_home: &route(RETURN_HOME, "away", "done", AWAY_TO_DONE, None),
            lead: &lead(),
            registry_return_home: None,
            screens: vec![AWAY, DONE],
        },
        true,
    );
    replay.fail_at_entry();
    assert_settled_at_entry(&replay.execution());
    assert_eq!(mark(&replay.host), Some(0), "the pair is marked");
    assert_eq!(replay.restarts(), Some(false), "the first block");
    wait_for_ladder_end(&replay.host, POLICY_INSTANCE_ALIAS);
    assert_eq!(
        finished_ladders(&replay.host),
        vec![RecoveryLadderOutcome::Recovered],
        "the bound return-home package recovers"
    );
    assert_eq!(
        rung_outcomes(&all_events(&replay.host)).first(),
        Some(&(
            RecoveryRung::ReturnHome,
            RecoveryRungOutcome::Recovered,
            None
        )),
        "at the return-home rung"
    );
    assert!(!replay.blocked(), "the recovery ends the block");
    assert_eq!(
        replay
            .host
            .entry_runs_started_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the runs started"),
        1,
        "no evidence: the scheduled run stays counted"
    );

    let window_until = NEXT_DISPATCH_UNIX_MS + COOLDOWN_LEFT_MS;
    replay
        .host
        .open_recovery_cooldown_for_test(POLICY_INSTANCE_ALIAS, window_until)
        .expect("the ladder's cool-down window");
    let second = admit_scheduled_at(
        &replay.host,
        replay.manual_clock(),
        &replay.request,
        NEXT_DISPATCH_UNIX_MS,
        301,
    );
    replay.fail_at_entry_of(&second);
    assert_settled_at_entry(&replay.execution_of(second.decision_id()));
    assert_eq!(mark(&replay.host), Some(0), "the mark stays");
    let (gate, _, held) = replay.block();
    assert_eq!(gate.decision_id, second.decision_id());
    assert_eq!(
        held,
        Some(window_until),
        "the block holds its run's recovery"
    );
    assert_eq!(
        replay.restarts(),
        Some(true),
        "nothing let in since the previous block ended"
    );

    replay.tick_at(window_until);
    wait_for_ladders_finished(&replay.host, 2);
    let plans = started_plans(&replay.host);
    let [(_, first_plan), (trigger, second_plan)] = plans.as_slice() else {
        panic!("two climbs: {plans:#?}");
    };
    assert_eq!(
        first_plan.first(),
        Some(&planned(RecoveryRung::ReturnHome, None)),
        "the first block's climb starts at the return-home rung"
    );
    assert_eq!(trigger.run_id, Some(second.run_id()));
    assert_eq!(*second_plan, from_the_application_restart());
    assert_eq!(
        finished_ladders(&replay.host),
        vec![
            RecoveryLadderOutcome::Recovered,
            RecoveryLadderOutcome::Exhausted
        ]
    );
    assert!(replay.blocked(), "the instance stays blocked");
    let failures = failed_outcomes(&replay.host, HostCode::InstanceEntryBlockedRecoveryFailed);
    let [failed] = failures.as_slice() else {
        panic!("one recovery that ended without recovering: {failures:#?}");
    };
    assert_eq!(
        failed.value("entry_recovery_end"),
        Some(&json!("exhausted"))
    );
    assert!(
        failed.value("next_occurrence_unix_ms").is_some(),
        "the block's next climb: {failed:?}"
    );
    replay.close();
}

/// R3 (ruling 3; the live settlement on a fake device, manual clock): as the replay above, but the
/// instance's `pack_logic` item starts its climbs at the application restart (rung 2) and its own
/// startup package leaves `away` for `done`: the first block's ladder stops the application, the
/// startup package recovers, and the ladder ends `recovered` having restarted the application: the
/// evidence against the pair. At the task's next occurrence the same task meets `done` and fails at
/// its entry again. The live settlement reads the marks its run's start read, before
/// `begin_contained_run` counted the run itself, so the run started from the instance that recovery
/// left: its failure is the pair's own (no condition, an identity that accumulates and names the
/// game root's data files, `consecutive_same_error` 1), and it blocks the instance, holding its
/// run's recovery. Were the marks read after the run counted itself, it would settle as a
/// condition.
#[test]
fn entry_r3_b_a_recovery_that_restarted_the_application_makes_the_next_failure_at_the_entry_the_pairs_own()
 {
    let startup = route(STARTUP, "away", "done", AWAY_TO_DONE, None);
    let replay = replay_set_up(
        &Scene {
            return_home: &route(RETURN_HOME, "away", "done", AWAY_TO_DONE, None),
            lead: &lead(),
            registry_return_home: None,
            screens: vec![AWAY, DONE],
        },
        true,
        Setup {
            startup: Some(&startup),
            ..shipped(restart_first())
        },
    );
    replay.fail_at_entry();
    assert_settled_at_entry(&replay.execution());
    assert_eq!(mark(&replay.host), Some(0), "the pair is marked");
    wait_for_ladder_end(&replay.host, POLICY_INSTANCE_ALIAS);
    assert_eq!(
        finished_ladders(&replay.host),
        vec![RecoveryLadderOutcome::Recovered]
    );
    assert!(
        rung_outcomes(&all_events(&replay.host)).contains(&(
            RecoveryRung::ApplicationRestart,
            RecoveryRungOutcome::Recovered,
            None
        )),
        "the startup package recovers after the application's stop"
    );
    assert!(!replay.blocked(), "the recovery ends the block");
    assert_eq!(
        replay
            .host
            .entry_runs_started_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the runs started"),
        0,
        "no run started since the recovery that is evidence"
    );

    let window_until = NEXT_DISPATCH_UNIX_MS + COOLDOWN_LEFT_MS;
    replay
        .host
        .open_recovery_cooldown_for_test(POLICY_INSTANCE_ALIAS, window_until)
        .expect("the ladder's cool-down window");
    let second = admit_scheduled_at(
        &replay.host,
        replay.manual_clock(),
        &replay.request,
        NEXT_DISPATCH_UNIX_MS,
        301,
    );
    replay.fail_at_entry_of(&second);
    let failure = replay.execution_of(second.decision_id());
    assert_eq!(failure.outcome.code().as_str(), PAGE_UNKNOWN);
    assert_eq!(failure.condition, None, "the pair's own: {failure:?}");
    assert_eq!(failure.consecutive_same_error, 1, "{failure:?}");
    let identity = failure
        .failure_identity
        .as_deref()
        .and_then(FailureIdentity::parse)
        .expect("a v2 failure identity");
    assert!(
        identity.accumulates() && !identity.at_entry(),
        "{identity:?}"
    );
    assert!(
        identity
            .layers
            .iter()
            .any(|layer| matches!(layer, IdentityLayer::StandardPackRoot(_))),
        "the game root's data files: {identity:?}"
    );
    assert!(!crate::policy_control::entry_blocked(&failure));
    let (gate, _, held) = replay.block();
    assert_eq!(gate.decision_id, second.decision_id());
    assert_eq!(
        held,
        Some(window_until),
        "the block holds its run's recovery"
    );
    replay.close();
}

/// R4 (ruling 4a; the settlement hook, `escalation_threshold` 2): the task's failure at the entry is
/// a condition; a recovery at the return-home rung (no evidence) and another run let in; its next
/// failure at the entry, the same day, is its second condition in a row; the same again; and its
/// next failure, the next day, is in another activity window of the profile: its conditions in a
/// row start again from one, so it is a condition, not the pair's own (the bound would have been
/// reached otherwise, another run having been let in).
#[test]
fn entry_r4_a_conditions_in_a_row_start_again_in_the_next_activity_window() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&sources(2))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    let settled = settle_at_entry(&host, &clock, &first);
    assert_settled_at_entry(&failure_of(&settled));
    recover_at_return_home(&host);
    host.let_run_in_for_test(POLICY_INSTANCE_ALIAS)
        .expect("a run's main package starts");
    let settled = fail_at(&host, &clock, settled.observed_at_unix_ms + 1_000, 101);
    assert_settled_at_entry(&failure_of(&settled));
    recover_at_return_home(&host);
    host.let_run_in_for_test(POLICY_INSTANCE_ALIAS)
        .expect("a run's main package starts");
    let settled = fail_at(&host, &clock, POLICY_NOW_UNIX_MS + DAY_MS, 102);
    assert_settled_at_entry(&failure_of(&settled));
    assert_eq!(blocked_by(&host).decision_id, settled.decision_id);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// R4 (ruling 4b; the settlement hook, `escalation_threshold` 2): no recovery that is evidence ever
/// comes (each block ends at the return-home rung), but after each failure at the entry another
/// run's main package starts on the instance. The second failure at the entry is a condition (its
/// conditions in a row below the threshold); the third, its conditions in a row at the threshold
/// and another run let in since its previous failure, is the pair's own (counters 1,
/// `retry_scheduled`), and blocks the instance.
#[test]
fn entry_r4_b_the_bound_holds_with_another_run_let_in_and_no_recovery_that_is_evidence() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&sources(2))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    let mut settled = settle_at_entry(&host, &clock, &first);
    assert_settled_at_entry(&failure_of(&settled));
    for (seed, own) in [(101, false), (102, true)] {
        recover_at_return_home(&host);
        host.let_run_in_for_test(POLICY_INSTANCE_ALIAS)
            .expect("a run's main package starts");
        settled = fail_at(&host, &clock, settled.observed_at_unix_ms + 1_000, seed);
        let failure = failure_of(&settled);
        if own {
            assert_own(&failure, 1);
            assert_eq!(
                failure.disposition,
                PolicyFailureDisposition::RetryScheduled
            );
        } else {
            assert_settled_at_entry(&failure);
        }
        assert_eq!(blocked_by(&host).decision_id, settled.decision_id);
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// R4 (ruling 4b; the settlement hook, `escalation_threshold` 2): each block ends at the
/// return-home rung (no evidence) and no other run is let in. The task's conditions in a row
/// reach the threshold at its second failure at the entry, but neither came since: its third
/// failure at the entry is a condition still, and so is its fourth.
#[test]
fn entry_r4_c_the_bound_is_not_reached_with_neither() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&sources(2))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    let mut settled = settle_at_entry(&host, &clock, &first);
    assert_settled_at_entry(&failure_of(&settled));
    for seed in [101, 102, 103] {
        recover_at_return_home(&host);
        settled = fail_at(&host, &clock, settled.observed_at_unix_ms + 1_000, seed);
        assert_settled_at_entry(&failure_of(&settled));
        assert_eq!(blocked_by(&host).decision_id, settled.decision_id);
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// R5 (ruling 5; the live path on a fake device, manual clock): the instance's `pack_logic` item
/// starts its climbs at the application restart, and its startup package runs from `menu` to
/// `done`; the game is on `away` until the test mends it. The run fails at its entry and blocks
/// the instance; its ladder stops the application, the startup package does not know `away`, and
/// the climb ends `exhausted`: one `instance_entry_blocked_recovery_failed` (`exhausted`, its next
/// climb's time), the instance stays blocked and the block holds its next climb for the shipped
/// first interval after that end. The game is mended. At that time the instance's cool-down
/// window, which the first climb opened when it got the key, still holds the climb back, to the
/// window's end; there the block climbs again by itself, from the application restart, of the
/// run's trigger: the startup package leaves `menu` for `done`, the climb ends `recovered` and the
/// block with it; no person is told again.
#[test]
fn entry_r5_a_an_exhausted_climb_tells_a_person_waits_and_climbs_again_by_itself() {
    let startup = route(STARTUP, "menu", "done", OPEN, None);
    let replay = replay_set_up(
        &Scene {
            return_home: &route(RETURN_HOME, "away", "menu", BACK_STEPS, None),
            lead: &lead(),
            registry_return_home: None,
            screens: vec![AWAY],
        },
        true,
        Setup {
            startup: Some(&startup),
            ..shipped(restart_first())
        },
    );
    let mended = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&mended);
    *replay.state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, inputs: usize| {
            let screen = if !seen.load(Ordering::Acquire) {
                AWAY
            } else if inputs == 0 {
                MENU
            } else {
                DONE
            };
            vec![(0, screen)]
        }));
    replay.fail_at_entry();
    assert_settled_at_entry(&replay.execution());
    wait_for_ladder_end(&replay.host, POLICY_INSTANCE_ALIAS);
    assert_eq!(
        finished_ladders(&replay.host),
        vec![RecoveryLadderOutcome::Exhausted],
        "the startup package does not know away"
    );
    assert!(replay.blocked(), "the instance stays blocked");
    let failures = failed_outcomes(&replay.host, HostCode::InstanceEntryBlockedRecoveryFailed);
    let [failed] = failures.as_slice() else {
        panic!("one recovery that ended without recovering: {failures:#?}");
    };
    assert_eq!(
        failed.value("entry_recovery_end"),
        Some(&json!("exhausted"))
    );
    let next = failed
        .value("next_occurrence_unix_ms")
        .and_then(Value::as_u64)
        .expect("the block's next climb");
    assert_eq!(replay.held_until(), next, "the block holds its next climb");
    assert_eq!(
        next,
        replay.now_unix_ms() + FIRST_INTERVAL_MS,
        "the shipped first interval after the climb's end"
    );

    mended.store(true, Ordering::Release);
    replay.tick_at(next);
    assert_eq!(
        started_plans(&replay.host).len(),
        1,
        "not before the window"
    );
    let window_end = replay.held_until();
    assert!(
        window_end > next,
        "the cool-down window holds the climb back"
    );
    replay.tick_at(window_end);
    wait_for_ladders_finished(&replay.host, 2);
    let plans = started_plans(&replay.host);
    let [_, (trigger, plan)] = plans.as_slice() else {
        panic!("two climbs: {plans:#?}");
    };
    assert_eq!(trigger.run_id, Some(replay.context.run_id()));
    assert_eq!(*plan, from_the_application_restart());
    assert_eq!(
        finished_ladders(&replay.host),
        vec![
            RecoveryLadderOutcome::Exhausted,
            RecoveryLadderOutcome::Recovered
        ]
    );
    assert!(!replay.blocked(), "the second climb ends the block");
    assert_eq!(
        failed_outcomes(&replay.host, HostCode::InstanceEntryBlockedRecoveryFailed).len(),
        1,
        "once per block"
    );
    replay.close();
}

/// R5 (ruling 5; manual clock): the block holds the run's recovery for the cool-down's end; the
/// next worker payload panics. At the window's end the recovery is admitted and its worker panics:
/// one `instance_entry_blocked_recovery_failed` (`worker_panicked`, no next climb), the instance
/// stays blocked holding nothing, and nothing climbs, then or two hours later.
#[test]
fn entry_r5_b_a_panicked_worker_tells_a_person_and_nothing_climbs_again() {
    let replay = replay(&route(RETURN_HOME, "away", "menu", BACK_STEPS, None), true);
    let until_unix_ms = replay.hold_a_recovery(COOLDOWN_LEFT_MS);
    replay
        .host
        .inject_instance_worker_panic_for_test()
        .expect("inject a worker panic");
    replay.tick_at(until_unix_ms);
    wait_until("the panicked recovery's warning", || {
        !failed_outcomes(&replay.host, HostCode::InstanceEntryBlockedRecoveryFailed).is_empty()
    });
    let failures = failed_outcomes(&replay.host, HostCode::InstanceEntryBlockedRecoveryFailed);
    let [failed] = failures.as_slice() else {
        panic!("one recovery that ended without recovering: {failures:#?}");
    };
    assert_eq!(
        failed.value("entry_recovery_end"),
        Some(&json!("worker_panicked"))
    );
    assert_eq!(
        failed.value("next_occurrence_unix_ms"),
        None,
        "no further climb"
    );
    let (_, told, held) = replay.block();
    assert!(told);
    assert_eq!(held, None, "nothing held");
    replay.tick_at(until_unix_ms + 2 * ATTENTION_AFTER_MS);
    assert!(
        replay.ladders().is_empty(),
        "no climb, then or later: {:?}",
        replay.ladders()
    );
    assert!(replay.blocked(), "the instance stays blocked");
    assert_eq!(
        failed_outcomes(&replay.host, HostCode::InstanceEntryBlockedRecoveryFailed).len(),
        1
    );
    replay.close();
}

/// R6 (ruling 6; the settlement hook, `escalation_threshold` 3): the task's failure at the entry
/// blocks the instance and begins its stretch; a recovery at the return-home rung ends the block
/// and lets nothing in, and the stretch runs on; the task's next failure at the entry blocks the
/// instance again, in the same stretch. A millisecond before the stretch has lasted
/// `attention_after_ms` the sweep's tick tells no one; at that time one
/// `instance_entry_blocked_awaiting_person` names the block's run and the stretch's length; later
/// ticks tell no one again.
#[test]
fn entry_r6_two_blocks_chained_by_a_recovery_that_let_nothing_in_tell_a_person_once_at_the_stretchs_threshold()
 {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&sources(3))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    let settled = settle_at_entry(&host, &clock, &first);
    let (since, told) = stretch(&host).expect("the first block begins a stretch");
    assert!(!told);
    recover_at_return_home(&host);
    assert_eq!(
        stretch(&host),
        Some((since, false)),
        "a recovery that let nothing in does not end the stretch"
    );
    let second = admit_at(&host, &clock, settled.observed_at_unix_ms + 1_000, 101);
    settle_at_entry(&host, &clock, &second);
    assert_eq!(blocked_by(&host).decision_id, second.decision_id);
    assert_eq!(stretch(&host), Some((since, false)), "the same stretch");

    advance_manual_clock_to(&clock, since + ATTENTION_AFTER_MS - 1);
    host.entry_block_tick_for_test().expect("the sweep's tick");
    assert!(
        failed_outcomes(&host, HostCode::InstanceEntryBlockedAwaitingPerson).is_empty(),
        "not before the stretch's threshold"
    );
    advance_manual_clock_to(&clock, since + ATTENTION_AFTER_MS);
    host.entry_block_tick_for_test().expect("the sweep's tick");
    let attentions = failed_outcomes(&host, HostCode::InstanceEntryBlockedAwaitingPerson);
    let [attention] = attentions.as_slice() else {
        panic!("one attention: {attentions:#?}");
    };
    assert_eq!(
        attention.value("decision_id"),
        Some(&json!(second.decision_id))
    );
    assert_eq!(
        attention.value("elapsed_ms"),
        Some(&json!(ATTENTION_AFTER_MS))
    );
    assert_eq!(
        attention.value("limit_ms"),
        Some(&json!(ATTENTION_AFTER_MS))
    );
    host.entry_block_tick_for_test().expect("the sweep's tick");
    assert_eq!(
        failed_outcomes(&host, HostCode::InstanceEntryBlockedAwaitingPerson).len(),
        1,
        "once per stretch"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// R7 (ruling 7; manual clock): the block holds the run's recovery for the cool-down's end, and
/// the queue refuses every ladder claim (as a full queue does, `lease_queue_full`). At the window's
/// end the recovery is admitted and its claim refused; the block holds it again for one second,
/// then two, four and eight, a tick a millisecond before each time trying nothing; the refusal is
/// recorded once, when it first occurred, none per retry. Once the queue takes claims again, the
/// next try's ladder climbs, its return-home rung recovers and the block ends.
#[test]
fn entry_r7_a_claim_the_queue_refuses_is_tried_again_on_a_growing_interval_and_recorded_once() {
    let replay = replay(&route(RETURN_HOME, "away", "menu", BACK_STEPS, None), true);
    let mut at = replay.hold_a_recovery(COOLDOWN_LEFT_MS);
    replay
        .host
        .refuse_ladder_claims_for_test(true)
        .expect("the queue refuses ladder claims");
    for (refusals, wait_ms) in [(1, 1_000), (2, 2_000), (3, 4_000), (4, 8_000)] {
        replay.tick_at(at);
        let next = at + wait_ms;
        // The lease sweep's own tick reads the same clock and may be the one that tried.
        wait_until("the refused claim held again", || {
            replay
                .host
                .ladder_claims_refused_for_test()
                .expect("the refusals")
                == refusals
                && replay
                    .host
                    .entry_block_for_test(POLICY_INSTANCE_ALIAS)
                    .expect("the instance's block")
                    .is_some_and(|(_, _, held)| held == Some(next))
        });
        replay.tick_at(next - 1);
        assert_eq!(
            replay
                .host
                .ladder_claims_refused_for_test()
                .expect("the refusals"),
            refusals,
            "not before its time"
        );
        at = next;
    }
    assert_eq!(
        failed_outcomes_coded(&replay.host, "lease_queue_full").len(),
        1,
        "recorded when it first occurred, none per retry"
    );
    assert!(replay.ladders().is_empty());
    assert!(replay.blocked());
    assert_no_one_told(&replay.host);

    replay
        .host
        .refuse_ladder_claims_for_test(false)
        .expect("the queue takes ladder claims again");
    replay.tick_at(at);
    wait_until("the held recovery's ladder", || {
        !replay.ladders().is_empty()
    });
    wait_for_ladder_end(&replay.host, POLICY_INSTANCE_ALIAS);
    assert!(
        matches!(
            ladder_finished(&all_events(&replay.host)),
            Some((_, RecoveryLadderOutcome::Recovered, _))
        ),
        "the return-home rung recovers"
    );
    assert!(!replay.blocked(), "the recovery's success ends the block");
    assert_eq!(
        failed_outcomes_coded(&replay.host, "lease_queue_full").len(),
        1
    );
    replay.close();
}
