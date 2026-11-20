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
//! ENTRY-A2's tests (a) to (e), fix round 1's F4, fix round 2's R3 (b) and R4 (a) to (c), and fix
//! round 3's K1 (a) to (d) and K2 (a) and (b) tested whose failure a failure at the entry is (a
//! recovery, what ran first, the conditions in a row, another task let in) and the pair's yield of
//! its turn; fix round 4 removes both (no failure at the entry counts against its rule), and they
//! are gone. None was on main.
//!
//! The policy-level tests settle with the host's test hook that arms the next settlement as one of a
//! run that failed at its entry (the live settlement tells that from the run's own records, which
//! the replays exercise), or as one whose main package started after its entry check; a recovery
//! is told to the blocks as the end of their block (the replays run real ladders). No clock is read
//! in this file, and every bounded wait takes `GATE_WAIT`.
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
//! - F3 (item 3): a store update package's task of an entry-blocked instance is dispatched.
//!
//! Fix round 2 (the coordinator's work order of 11-21 after the second independent review):
//!
//! - R1 (ruling 1): a capture backend failure in a prerequisite layer of a scheduled run under the
//!   shipped defaults is no failure at the entry: settled as before this slice (no count of
//!   failures at the entry, no block, no refund), its ladder the one its trigger item starts;
//! - R2 (ruling 2): the first block climbs as configured; one set with nothing let in since the
//!   previous block ended climbs from the application restart; one after a run got in as
//!   configured;
//! - R3 (a) (rulings 2 and 3; the production path): after a recovery at the return-home rung, the
//!   same task's next failure at its entry is a condition and its block's recovery climbs from the
//!   application restart (fix round 3: once the stretch's paced time and the window allow);
//! - R5 (ruling 5): an exhausted climb tells a person once, waits, and climbs again by itself, and
//!   the second climb recovers; a panicked worker tells a person and nothing climbs again;
//! - R6 (ruling 6): two blocks chained by a recovery that let nothing in are one stretch: a person
//!   is told once, at the stretch's threshold;
//! - R7 (ruling 7): a claim the queue refuses for many ticks is tried again on the growing
//!   interval, its refusal recorded once.
//!
//! Fix round 3 (the coordinator's work order of 11-21 after the third independent review):
//!
//! - K3 (ruling 3): a later climb of a stretch waits the interval after a `recovered` that let
//!   nothing in, and after an interruption by business capacity; after a run got in the next
//!   block's climb does not wait (and starts as configured, ruling 5);
//! - K4 (ruling 4): the game's update dialog ends the climb, a person is told once with its word,
//!   nothing climbs again in the stretch, and the store update task is dispatched on the blocked
//!   instance.
//!
//! Fix round 4 (the coordinator's work order of 11-21 after the fourth independent review; a
//! failure at the entry never counts against its rule):
//!
//! - M1 (ruling 1): a pair's second and third failures at the entry, another scheduled run let in
//!   before each, leave its rule's streak at zero and its rule not paused under a threshold of 1;
//! - M2 (ruling 2 and the coordinator's addition to it): after its first failure the pair waits the
//!   first interval from its block's end, so the healthy task runs first, after its second twice
//!   that, and a run of the pair that settles otherwise starts it from nothing;
//! - M3 (ruling 3): at the threshold the pair is set aside until its window's end and a person is
//!   told once, and it is dispatched again after the window; a failure with nothing let in since
//!   the previous one does not count; two broken pairs and a healthy task: the healthy task runs
//!   and both pairs are set aside in turn;
//! - M4 (ruling 4): the second climb of a stretch skips the emulator's rung, the first climb of a
//!   new stretch does not;
//! - M5 (ruling 5): a store update task's success between two failures at the entry clears the
//!   wait and counts toward nothing;
//! - M6 (the lease end's two in-memory flags): on a page graph with no prerequisite chain, a
//!   failure before its main package started is a failure at the entry at its lease end, one after
//!   it is not;
//! - M7 (the coordinator's addition to rulings 2 and 3): a catalogue that changes a set-aside pair's
//!   package frees it at once.
//!
//! Fix round 5 (the coordinator's work order of 11-21 after the fifth independent review):
//!
//! - N1 (ruling 1): a sensitive task's failure at the entry, under a threshold of 1, settles as a
//!   condition, its admission given back, the instance blocked, the rule not paused, no streak;
//! - N3 (a) (ruling 3, case (a)): after an outage the pairs at the maximum no longer come due
//!   together (the broken one waits one first interval more), the healthy task runs in the round
//!   after the broken one's failure, its run let in ends every wait at once and keeps what counts
//!   against a pair; N3 (b) (ruling 3 (i)): past the maximum, one first interval more per failure;
//! - N4 (ruling 4): a store update task whose main package started and failed raises nothing and
//!   ends no wait;
//! - ruling 5: the first replay asserts that after an exhausted climb the block's later climbs
//!   start at the application restart, whichever path builds them.
//!
//! M1, M2, M3 (a), M3 (c), M7 and R2 change with ruling 3 (ii) (a scheduled run let in ends every
//! pair's wait), and every policy-level admission comes after the previous run's lease lapsed
//! (`LEASE_LAPSED_MS`: M1 failed on fix round 4's head admitting a run two seconds after another);
//! M4 ends its first block with a recovery (the review's M4) and waits for each climb's end as the
//! other replays do.

use super::gate_373_nav3a::{
    AWAY, DONE, LEAD, MAIN, MENU, OPEN, Pack, RETURN_HOME, SHOP, StandardRoot, route, screens,
};
use super::gate_373_nav6::{BLACK, Overlay, RED, entry_away_to_home, frame, script, update_root};
use super::gate_386_sr2b1::{admit, failure_of, start};
use super::gate_386_sr4a3::{once, silence_manifest};
use super::gate_480_pg2b::hold_next_rung_step;
use super::instance_workers::{
    admit_ladder_run, all_events, ladder_end_code, ladder_finished, ladder_setup_with_package,
    ladder_setup_with_packages, phases, rung_outcomes, wait_for_ladder_end, wait_until,
};
use super::*;
use crate::codes::HostCode;
use crate::failure_identity::{FailureIdentity, IdentityFrame, IdentityPackage};
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
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
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
/// The second task of the same rule on the policy instance, of a lower priority.
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
/// M1: a day, so the next admission is in the next activity window of the budget fixture's
/// profile (one window a day).
const DAY_MS: u64 = 86_400_000;
/// K3 (fix round 3, ruling 3): `scheduling.unavailable_probe`'s first and longest interval in the
/// configuration the pacing tests run on, two hours: longer than the ten-minute cool-down window,
/// so the wait a later climb keeps is the stretch's interval, not the window.
const PACED_MS: u64 = 7_200_000;
/// R3, R5: the instance's own startup package.
const STARTUP: &str = "neutral.test.startup";
/// R3, R5: the shipped `scheduling.unavailable_probe.first_interval_ms`, the wait before a
/// block's climb after its first exhausted one; fix round 4 (ruling 2): a pair's wait after its
/// first failure at the entry in a row, doubling (the shipped `factor` 2) after each further one.
const FIRST_INTERVAL_MS: u64 = 60_000;
/// Fix round 4 (ruling 7): the bound of every wait on a channel in this file, as `gate_480_pg2b`'s;
/// it only bounds a broken run.
const GATE_WAIT: Duration = Duration::from_secs(120);
/// Fix round 4: a run that fails at its entry at once (the settlement hook's policy-level runs).
const ENTRY_RUN_MS: u64 = 1_000;
/// Fix round 4 (the coordinator's addition to ruling 2): how long a block lasts in the policy-level
/// tests (its climb), from the failure's settlement to the recovery that ends it; longer than a
/// pair's first two waits (one and two minutes), so a wait counted from the settlement would have
/// passed when the block ends.
const BLOCK_MS: u64 = 3 * MINUTE_MS;
/// Fix round 4 (K3 a): an hour, `scheduling.unavailable_probe`'s first interval there.
const HOUR_MS: u64 = 3_600_000;
/// Fix round 4 (M3 c): a third task of the budget fixture's rule, between `TASK` and `OTHER`.
const BROKEN: &str = "fixture.broken";
/// Fix round 5 (CI on fix round 4's head: M1 admitted `OTHER` two seconds after `TASK`): past the
/// test configuration's lease TTL (5 s, `config`), so the lease of the run before, which the
/// settlement hook does not release, has lapsed when the next run is admitted.
const LEASE_LAPSED_MS: u64 = 10_000;

// ---- The policy-level fixture.

/// The budget fixture (one task, due at every evaluation) with `on_failure.escalation_threshold`.
fn sources(escalation_threshold: u16) -> CatalogSources {
    sources_at(1, escalation_threshold)
}

/// Fix round 4 (M7): [`sources`] as the catalog version `version`.
fn sources_at(version: u64, escalation_threshold: u16) -> CatalogSources {
    let mut sources = budget_policy_sources(version);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("the task fixture");
    tasks["tasks"][0]["on_failure"]["escalation_threshold"] = json!(escalation_threshold);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("the task bytes");
    sources
}

/// [`sources`] with a second task, `OTHER`: the same rule under its own procedure and a lower
/// priority (50; `TASK`'s is 100), due at every evaluation too, so it is dispatched when `TASK`
/// is not.
fn two_tasks(escalation_threshold: u16) -> CatalogSources {
    two_tasks_at(1, escalation_threshold)
}

/// Fix round 4 (M7): [`two_tasks`] as the catalog version `version`.
fn two_tasks_at(version: u64, escalation_threshold: u16) -> CatalogSources {
    let mut sources = sources_at(version, escalation_threshold);
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

/// Fix round 4 (M3 c): [`two_tasks`] with a third task, `BROKEN`, of the same rule under its own
/// procedure (the fixture manifest's third binding), ranked between them (priority 75), due at
/// every evaluation too.
fn three_tasks(escalation_threshold: u16) -> CatalogSources {
    let mut sources = two_tasks(escalation_threshold);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("the task fixture");
    let mut broken = tasks["tasks"][1].clone();
    broken["id"] = json!(BROKEN);
    broken["procedure_ref"] = json!("procedure.detect");
    broken["priority"] = json!(75);
    tasks["tasks"]
        .as_array_mut()
        .expect("the tasks")
        .push(broken);
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

/// Fix round 4 (rulings 2 and 3): the reason `code` of `task_id` on the policy instance in
/// `evaluation`.
fn reason_of(
    evaluation: &actingcommand_policy::PolicyEvaluation,
    task_id: &str,
    code: HostCode,
) -> Option<Outcome> {
    evaluation
        .decisions
        .iter()
        .filter(|decision| {
            decision.task_id == task_id
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .flat_map(|decision| decision.reasons.iter())
        .find(|reason| reason.outcome.is(code))
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

/// [`assert_settled_at_entry_with`] but for the rule's counters, which a condition keeps as the
/// pair's latest failure of another kind left them.
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

// ---- The pairs' runs at the policy level.

/// The clock moved to `at` and the next dispatch on the policy instance evaluated and admitted
/// there: its intent.
fn admit_at(host: &RuntimeHost, clock: &ManualRuntimeClock, at: u64, seed: u64) -> DispatchIntent {
    advance_manual_clock_to(clock, at);
    admit(host, at, seed, false).0
}

/// `intent`'s run, `RUN_MS` long, settled as one that failed at its entry in its prerequisite
/// layer (the input the live settlement builds): its execution record.
fn settle_at_entry(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    intent: &DispatchIntent,
) -> actingcommand_contract::PolicyExecutionEventData {
    settle_at_entry_after(host, clock, intent, RUN_MS)
}

/// Fix round 4: [`settle_at_entry`] of a run `run_ms` long.
fn settle_at_entry_after(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    intent: &DispatchIntent,
    run_ms: u64,
) -> actingcommand_contract::PolicyExecutionEventData {
    clock.advance(run_ms);
    host.settle_next_policy_run_at_entry_for_test()
        .expect("arm the settlement");
    host.record_policy_dispatch_outcome(&intent.decision_id, &at_entry(intent))
        .expect("the settlement")
}

/// Fix round 4: `intent`'s run, `ENTRY_RUN_MS` long, succeeded: its execution record.
fn succeed(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    intent: &DispatchIntent,
) -> actingcommand_contract::PolicyExecutionEventData {
    clock.advance(ENTRY_RUN_MS);
    host.record_policy_dispatch_outcome(&intent.decision_id, &PolicyExecutionInput::Succeeded)
        .expect("the success")
}

/// The policy instance's entry block, which must be set: its gate.
fn blocked_by(host: &RuntimeHost) -> crate::entry_block::EntryBlockGate {
    host.entry_block_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the instance's block")
        .expect("the instance is blocked")
        .0
}

/// A recovery of the policy instance ended its block (at whichever rung): the instance is not
/// blocked after it.
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

/// Fix round 4 (the coordinator's addition to ruling 2): [`recover`] with the clock moved to `at`,
/// the block's end.
fn recover_at(host: &RuntimeHost, clock: &ManualRuntimeClock, at: u64) {
    advance_manual_clock_to(clock, at);
    recover(host);
}

/// Fix round 4 (rulings 2 and 3): `task_id`'s pair on the policy instance, if its latest scheduled
/// run failed at its entry: its failures at the entry in a row, those against it, the time before
/// which it is not dispatched and whether it is set aside.
fn pair(host: &RuntimeHost, task_id: &str) -> Option<(u32, u32, u64, bool)> {
    host.entry_pair_for_test(POLICY_INSTANCE_ALIAS, task_id)
        .expect("the pair")
}

/// Fix round 4 (ruling 3): the `instance_entry_task_set_aside_reported` warnings of `host`.
fn set_aside_reports(host: &RuntimeHost) -> Vec<Outcome> {
    failed_outcomes(host, HostCode::InstanceEntryTaskSetAsideReported)
}

/// M1 (ruling 1; the settlement hook, two tasks, `escalation_threshold` 1): `TASK` fails at its
/// entry; after its block's end, while it waits, `OTHER` is dispatched and succeeds (a scheduled
/// run let in, which since fix round 5 ends `TASK`'s wait at once, ruling 3 (ii)); `TASK`'s second
/// failure at the entry comes once `OTHER`'s lease has lapsed; after that block's end `OTHER` is
/// let in again (the pair being set aside for the window, ruling 3), and `TASK`'s third failure at
/// the entry comes the next day. Each is a condition of the instance: the rule's counters 0,
/// `continue`, no retry, no pause cause, its count given back, no failure streak in the pair's
/// value record, the instance blocked by it. No failure at the entry counts against its rule,
/// whatever got in between. Fix round 5 (CI on fix round 4's head, where this test failed): every
/// admission comes after the previous run's lease lapsed (`LEASE_LAPSED_MS`; the settlement hook
/// releases none), each block lasting `BLOCK_MS`.
#[test]
fn entry_m1_failures_at_the_entry_with_another_run_let_in_before_each_never_count_against_the_rule()
{
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&two_tasks(1))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    assert_eq!(first.task_id, TASK, "the task comes first");
    let mut settled = settle_at_entry_after(&host, &clock, &first, ENTRY_RUN_MS);
    assert_settled_at_entry(&failure_of(&settled));
    for (seed, next_day) in [(101, false), (103, true)] {
        let block_end = settled.observed_at_unix_ms + BLOCK_MS;
        recover_at(&host, &clock, block_end);
        let other = admit_at(&host, &clock, block_end + 1_000, seed);
        assert_eq!(other.task_id, OTHER, "the task waits, the other runs");
        let succeeded = succeed(&host, &clock, &other);
        let at = if next_day {
            POLICY_NOW_UNIX_MS + DAY_MS
        } else {
            succeeded.observed_at_unix_ms + LEASE_LAPSED_MS
        };
        let next = admit_at(&host, &clock, at, seed + 1);
        assert_eq!(next.task_id, TASK, "{seed}");
        settled = settle_at_entry_after(&host, &clock, &next, ENTRY_RUN_MS);
        let failure = failure_of(&settled);
        assert_settled_at_entry(&failure);
        let record = latest_value_record(&host);
        assert!(record.failure_streak.is_none(), "no streak: {record:?}");
        assert_eq!(blocked_by(&host).decision_id, next.decision_id);
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// M2 (ruling 2 and the coordinator's addition to it; the settlement hook, two tasks,
/// `escalation_threshold` 3): `TASK`, sorted first, fails at its entry; while its block lasts
/// (`BLOCK_MS`, longer than the interval) its wait counts from that settlement, and once a recovery
/// ends the block, from the block's end. A second after that end `TASK` waits
/// `instance_entry_task_waiting` (count 1, `next_eligible_unix_ms` the wait's end, the failing run
/// named) and `OTHER`, the healthy task, is dispatched and succeeds before `TASK`'s second attempt.
/// Fix round 5 (ruling 3 (ii)): that scheduled run let in ends `TASK`'s wait at once and sets its
/// count in a row back to none, so once `OTHER`'s lease has lapsed `TASK` is dispatched with no
/// reason of its pair, long before its old wait's end, and fails at its entry again: its first in a
/// row (the first interval again), one against it (`OTHER` got in), its wait counted from that
/// block's end. At that wait's end `TASK`'s next run's main package starts after its entry check and
/// fails in a step: the pair starts from nothing, and after the step failure's backoff `TASK` is
/// dispatched with no wait.
#[test]
fn entry_m2_the_pair_waits_from_its_blocks_end_so_the_healthy_task_runs_first_and_a_run_past_its_entry_clears_it()
 {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&two_tasks(3))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    assert_eq!(first.task_id, TASK);
    let settled = settle_at_entry_after(&host, &clock, &first, ENTRY_RUN_MS);
    assert_eq!(
        pair(&host, TASK),
        Some((1, 0, settled.observed_at_unix_ms + FIRST_INTERVAL_MS, false)),
        "from the settlement while the block lasts"
    );
    let block_end = settled.observed_at_unix_ms + BLOCK_MS;
    recover_at(&host, &clock, block_end);
    let until = block_end + FIRST_INTERVAL_MS;
    assert_eq!(
        pair(&host, TASK),
        Some((1, 0, until, false)),
        "from the block's end"
    );
    let at = block_end + 1_000;
    advance_manual_clock_to(&clock, at);
    let evaluation = evaluate(&host, at, 101);
    assert!(
        dispatched(&evaluation, OTHER),
        "another due task runs: {evaluation:#?}"
    );
    assert!(!dispatched(&evaluation, TASK), "{evaluation:#?}");
    let waiting = reason_of(&evaluation, TASK, HostCode::InstanceEntryTaskWaiting)
        .unwrap_or_else(|| panic!("the task waits: {evaluation:#?}"));
    assert_eq!(
        waiting.value("instance_id"),
        Some(&json!(POLICY_INSTANCE_ALIAS))
    );
    assert_eq!(waiting.value("task_id"), Some(&json!(TASK)));
    assert_eq!(
        waiting.value("decision_id"),
        Some(&json!(first.decision_id))
    );
    assert_eq!(waiting.value("count"), Some(&json!(1)));
    assert_eq!(waiting.value("next_eligible_unix_ms"), Some(&json!(until)));
    let other = admit_at(&host, &clock, at, 151);
    assert_eq!(
        other.task_id, OTHER,
        "the healthy task runs before the pair's next attempt"
    );
    let succeeded = succeed(&host, &clock, &other);
    assert_eq!(
        pair(&host, TASK),
        Some((0, 0, 0, false)),
        "a scheduled run let in ends the wait at once (ruling 3 (ii))"
    );
    let at = succeeded.observed_at_unix_ms + LEASE_LAPSED_MS;
    assert!(at < until, "long before the old wait's end");
    advance_manual_clock_to(&clock, at);
    let evaluation = evaluate(&host, at, 102);
    assert!(dispatched(&evaluation, TASK), "{evaluation:#?}");
    assert_eq!(
        reason_of(&evaluation, TASK, HostCode::InstanceEntryTaskWaiting),
        None
    );
    let second = admit_at(&host, &clock, at, 152);
    assert_eq!(second.task_id, TASK);
    let settled = settle_at_entry_after(&host, &clock, &second, ENTRY_RUN_MS);
    assert_settled_at_entry(&failure_of(&settled));
    let block_end = settled.observed_at_unix_ms + BLOCK_MS;
    recover_at(&host, &clock, block_end);
    let until = block_end + FIRST_INTERVAL_MS;
    assert_eq!(
        pair(&host, TASK),
        Some((1, 1, until, false)),
        "its first in a row again, one against it"
    );
    let intent = admit_at(&host, &clock, until, 103);
    assert_eq!(intent.task_id, TASK);
    clock.advance(ENTRY_RUN_MS);
    host.settle_next_policy_run_past_its_entry_for_test()
        .expect("arm the settlement");
    let settled = host
        .record_policy_dispatch_outcome(&intent.decision_id, &in_a_step())
        .expect("the settlement");
    let failure = failure_of(&settled);
    assert_eq!(failure.condition, None);
    assert_eq!(
        failure.retry_at_unix_ms,
        Some(settled.observed_at_unix_ms + BACKOFF_MS),
        "the rule's backoff"
    );
    assert_eq!(pair(&host, TASK), None, "the run started its main package");
    let retry_at = settled.observed_at_unix_ms + BACKOFF_MS;
    advance_manual_clock_to(&clock, retry_at);
    let evaluation = evaluate(&host, retry_at, 200);
    assert!(
        dispatched(&evaluation, TASK),
        "no wait of its entry: {evaluation:#?}"
    );
    assert_eq!(
        reason_of(&evaluation, TASK, HostCode::InstanceEntryTaskWaiting),
        None
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// M3 (a) (ruling 3; the settlement hook, two tasks, `escalation_threshold` 2): `TASK` fails at its
/// entry three times, each block lasting `BLOCK_MS`, `OTHER` dispatched and succeeding after each
/// block's end while `TASK` waits (from that end) before the second and the third; fix round 5
/// (ruling 3 (ii)): each such success ends `TASK`'s wait and its count in a row and keeps what
/// counts against it, and `TASK` is dispatched once `OTHER`'s lease has lapsed, each failure its
/// first in a row. The second counts once against it, the third twice: it reaches the threshold,
/// so `TASK` is set aside until
/// its activity window's end and one `instance_entry_task_set_aside_reported` names the task, the
/// count, the threshold and the window's end. An hour later `TASK` waits
/// `instance_entry_task_set_aside` and `OTHER` is dispatched; after the window's end `TASK` is
/// dispatched again; no person is told again.
#[test]
fn entry_m3_a_at_the_threshold_the_pair_is_set_aside_until_its_windows_end_and_a_person_is_told_once()
 {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&two_tasks(2))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    assert_eq!(first.task_id, TASK);
    let mut settled = settle_at_entry_after(&host, &clock, &first, ENTRY_RUN_MS);
    let window_end = failure_of(&settled)
        .condition
        .expect("a condition")
        .expires_at_unix_ms;
    let mut last = first;
    for (against, seed) in [(1_u32, 101), (2, 103)] {
        let block_end = settled.observed_at_unix_ms + BLOCK_MS;
        recover_at(&host, &clock, block_end);
        let other = admit_at(&host, &clock, block_end + 1_000, seed);
        assert_eq!(other.task_id, OTHER);
        let succeeded = succeed(&host, &clock, &other);
        // Fix round 5 (ruling 3 (ii)): the run let in ends the pair's wait and its count in a
        // row, and leaves what counts against it.
        assert_eq!(
            pair(&host, TASK),
            Some((0, against - 1, 0, false)),
            "the wait ended, the count against it kept"
        );
        last = admit_at(
            &host,
            &clock,
            succeeded.observed_at_unix_ms + LEASE_LAPSED_MS,
            seed + 1,
        );
        assert_eq!(last.task_id, TASK);
        settled = settle_at_entry_after(&host, &clock, &last, ENTRY_RUN_MS);
        assert_settled_at_entry(&failure_of(&settled));
        let (in_a_row, counted, _, set_aside) = pair(&host, TASK).expect("the pair");
        assert_eq!((in_a_row, counted, set_aside), (1, against, against == 2));
    }
    let reports = set_aside_reports(&host);
    let [report] = reports.as_slice() else {
        panic!("one person told: {reports:#?}");
    };
    assert_eq!(report.value("task_id"), Some(&json!(TASK)));
    assert_eq!(report.value("decision_id"), Some(&json!(last.decision_id)));
    assert_eq!(report.value("count"), Some(&json!(2)));
    assert_eq!(report.value("limit_count"), Some(&json!(2)));
    assert_eq!(report.value("window_end_unix_ms"), Some(&json!(window_end)));

    recover(&host);
    let later = settled.observed_at_unix_ms + 3_600_000;
    assert!(later < window_end);
    advance_manual_clock_to(&clock, later);
    let evaluation = evaluate(&host, later, 200);
    assert!(!dispatched(&evaluation, TASK), "{evaluation:#?}");
    assert!(dispatched(&evaluation, OTHER), "{evaluation:#?}");
    let aside = reason_of(&evaluation, TASK, HostCode::InstanceEntryTaskSetAside)
        .unwrap_or_else(|| panic!("the task is set aside: {evaluation:#?}"));
    assert_eq!(aside.value("window_end_unix_ms"), Some(&json!(window_end)));
    assert_eq!(aside.value("count"), Some(&json!(2)));
    assert_eq!(aside.value("limit_count"), Some(&json!(2)));

    let next_window = window_end + 1_000;
    advance_manual_clock_to(&clock, next_window);
    let evaluation = evaluate(&host, next_window, 201);
    assert!(
        dispatched(&evaluation, TASK),
        "dispatched again after the window: {evaluation:#?}"
    );
    assert_eq!(set_aside_reports(&host).len(), 1, "told once");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// M3 (b) (ruling 3; the settlement hook, one task, `escalation_threshold` 1): `TASK` fails at its
/// entry three times in a row, each once its wait from its block's end has passed, nothing let in
/// on the instance between them: none counts against it, it is never set aside and no person is
/// told.
#[test]
fn entry_m3_b_a_failure_with_nothing_let_in_since_the_previous_one_does_not_count() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&sources(1))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    let mut settled = settle_at_entry_after(&host, &clock, &first, ENTRY_RUN_MS);
    for (in_a_row, seed) in [(2_u32, 101), (3, 102)] {
        let block_end = settled.observed_at_unix_ms + BLOCK_MS;
        recover_at(&host, &clock, block_end);
        let (_, _, until, _) = pair(&host, TASK).expect("the pair waits");
        assert_eq!(
            until,
            block_end + (FIRST_INTERVAL_MS << (in_a_row - 2)),
            "from the block's end"
        );
        let next = admit_at(&host, &clock, until, seed);
        settled = settle_at_entry_after(&host, &clock, &next, ENTRY_RUN_MS);
        let (counted_in_a_row, against, _, set_aside) = pair(&host, TASK).expect("the pair");
        assert_eq!((counted_in_a_row, against, set_aside), (in_a_row, 0, false));
    }
    assert!(set_aside_reports(&host).is_empty(), "no one told");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// M3 (c) (ruling 3 and the coordinator's addition to ruling 2; the settlement hook, three tasks,
/// `escalation_threshold` 1): `TASK` and `BROKEN` fail at their entry each time; `OTHER` succeeds;
/// each block lasts `BLOCK_MS`. `TASK` fails; after its block's end, while it waits, `BROKEN` is
/// dispatched and fails. `TASK`'s wait passed during `BROKEN`'s block, but that block's end starts
/// both pairs' waits again: `OTHER` runs and is let in. `TASK`'s next failure, then `BROKEN`'s,
/// each came after a run let in: each pair is set aside for the window in turn, a person told once
/// of each, and `OTHER` is dispatched and runs again.
#[test]
fn entry_m3_c_two_broken_pairs_and_a_healthy_task_the_healthy_task_runs_and_both_are_set_aside_in_turn()
 {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&three_tasks(1))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    assert_eq!(first.task_id, TASK);
    let task_settled = settle_at_entry_after(&host, &clock, &first, ENTRY_RUN_MS);
    let first_end = task_settled.observed_at_unix_ms + BLOCK_MS;
    recover_at(&host, &clock, first_end);
    let broken = admit_at(&host, &clock, first_end + 1_000, 101);
    assert_eq!(
        broken.task_id, BROKEN,
        "the task waits, the broken one goes"
    );
    let broken_settled = settle_at_entry_after(&host, &clock, &broken, ENTRY_RUN_MS);
    let mut block_end = broken_settled.observed_at_unix_ms + BLOCK_MS;
    assert!(
        block_end > first_end + FIRST_INTERVAL_MS,
        "the task's wait passes during the broken one's block"
    );
    recover_at(&host, &clock, block_end);
    for task_id in [TASK, BROKEN] {
        assert_eq!(
            pair(&host, task_id).map(|(_, _, until, _)| until),
            Some(block_end + FIRST_INTERVAL_MS),
            "{task_id} waits from the latest block's end"
        );
    }
    let other = admit_at(&host, &clock, block_end + 1_000, 102);
    assert_eq!(other.task_id, OTHER, "both wait, the healthy task runs");
    let succeeded = succeed(&host, &clock, &other);
    // Fix round 5 (ruling 3 (ii)): the run let in ends both pairs' waits at once; `TASK`, first in
    // the order, goes once `OTHER`'s lease has lapsed, then `BROKEN` after `TASK`'s block's end
    // (its count in a row none, so that end sets it no wait).
    for (task_id, seed) in [(TASK, 103), (BROKEN, 104)] {
        assert_eq!(
            pair(&host, task_id).map(|(in_a_row, _, until, _)| (in_a_row, until)),
            Some((0, 0)),
            "{task_id}: no wait"
        );
        let at = if task_id == TASK {
            succeeded.observed_at_unix_ms + LEASE_LAPSED_MS
        } else {
            block_end + 1_000
        };
        let next = admit_at(&host, &clock, at, seed);
        assert_eq!(next.task_id, task_id);
        let settled = settle_at_entry_after(&host, &clock, &next, ENTRY_RUN_MS);
        let (_, against, _, set_aside) = pair(&host, task_id).expect("the pair");
        assert_eq!((against, set_aside), (1, true), "{task_id}");
        block_end = settled.observed_at_unix_ms + BLOCK_MS;
        recover_at(&host, &clock, block_end);
    }
    let reports = set_aside_reports(&host);
    let told = reports
        .iter()
        .filter_map(|report| report.value("task_id").cloned())
        .collect::<Vec<_>>();
    assert_eq!(told, vec![json!(TASK), json!(BROKEN)], "one each, in turn");
    let other = admit_at(&host, &clock, block_end + 1_000, 105);
    assert_eq!(other.task_id, OTHER, "the healthy task runs");
    succeed(&host, &clock, &other);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// Fix round 4 (M7): [`two_tasks`] as the catalog version 2 in which `TASK` runs another package
/// (the fixture manifest's third binding, `procedure.detect`).
fn changed_package(escalation_threshold: u16) -> CatalogSources {
    let mut sources = two_tasks_at(2, escalation_threshold);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("the task fixture");
    tasks["tasks"][0]["procedure_ref"] = json!("procedure.detect");
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("the task bytes");
    sources
}

/// M7 (the coordinator's addition to rulings 2 and 3; the settlement hook, two tasks,
/// `escalation_threshold` 1): `TASK` fails at its entry, `OTHER` gets in after its block's end, and
/// `TASK`'s second failure at the entry sets it aside until its window's end: after that block's
/// end it waits `instance_entry_task_set_aside`. A catalogue in which `TASK` runs another package
/// is activated: the pair's count, wait and being set aside are cleared then, and a second later,
/// long before the window's end, `TASK` is dispatched with no reason of its pair.
#[test]
fn entry_m7_a_catalogue_with_a_changed_package_frees_a_set_aside_pair_at_once() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&two_tasks(1))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    assert_eq!(first.task_id, TASK);
    let settled = settle_at_entry_after(&host, &clock, &first, ENTRY_RUN_MS);
    let window_end = failure_of(&settled)
        .condition
        .expect("a condition")
        .expires_at_unix_ms;
    let block_end = settled.observed_at_unix_ms + BLOCK_MS;
    recover_at(&host, &clock, block_end);
    let other = admit_at(&host, &clock, block_end + 1_000, 101);
    assert_eq!(other.task_id, OTHER);
    let succeeded = succeed(&host, &clock, &other);
    // Fix round 5 (ruling 3 (ii)): `OTHER`'s success ended `TASK`'s wait; it goes once `OTHER`'s
    // lease has lapsed.
    let second = admit_at(
        &host,
        &clock,
        succeeded.observed_at_unix_ms + LEASE_LAPSED_MS,
        102,
    );
    assert_eq!(second.task_id, TASK);
    let settled = settle_at_entry_after(&host, &clock, &second, ENTRY_RUN_MS);
    assert_eq!(
        pair(&host, TASK).map(|(_, against, _, set_aside)| (against, set_aside)),
        Some((1, true)),
        "set aside"
    );
    let block_end = settled.observed_at_unix_ms + BLOCK_MS;
    recover_at(&host, &clock, block_end);
    let at = block_end + 1_000;
    advance_manual_clock_to(&clock, at);
    let evaluation = evaluate(&host, at, 200);
    assert!(!dispatched(&evaluation, TASK), "{evaluation:#?}");
    assert!(
        reason_of(&evaluation, TASK, HostCode::InstanceEntryTaskSetAside).is_some(),
        "the task is set aside: {evaluation:#?}"
    );

    host.activate_policy_catalog(&changed_package(1))
        .expect("activate the catalogue with the changed package");
    assert_eq!(pair(&host, TASK), None, "the pair starts from nothing");
    let at = at + 1_000;
    assert!(at < window_end);
    advance_manual_clock_to(&clock, at);
    let evaluation = evaluate(&host, at, 201);
    assert!(
        dispatched(&evaluation, TASK),
        "dispatched before the window's end: {evaluation:#?}"
    );
    assert_eq!(
        reason_of(&evaluation, TASK, HostCode::InstanceEntryTaskSetAside),
        None
    );
    assert_eq!(
        reason_of(&evaluation, TASK, HostCode::InstanceEntryTaskWaiting),
        None
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// ---- Fix round 5 at the policy level.

/// N1: [`sources`] with the task marked `sensitive`.
fn sensitive_sources(escalation_threshold: u16) -> CatalogSources {
    let mut sources = sources(escalation_threshold);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("the task fixture");
    tasks["tasks"][0]["sensitive"] = json!(true);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("the task bytes");
    sources
}

/// N1 (fix round 5, ruling 1; the settlement hook, a sensitive task, `escalation_threshold` 1): its
/// run fails at its entry. Its main package started nothing, so nothing sensitive was begun: the
/// record is a condition of the instance as any task's (refunded, count 1, `continue`, counters 0,
/// no retry, no pause cause, `F` `i…`), and says so (`sensitive` false, the sensitive rule not
/// applied); the pair's value record holds no count for the day and no streak, and the instance is
/// blocked by the run. The rule is not paused.
#[test]
fn entry_n1_a_sensitive_tasks_failure_at_the_entry_settles_as_a_condition_and_never_pauses_its_rule()
 {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&sensitive_sources(1))
        .expect("activate the catalog");
    let (intent, admission) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    assert_eq!(admission.budget.task_daily_used, Some(1));
    let settled = settle_at_entry_after(&host, &clock, &intent, ENTRY_RUN_MS);
    let failure = failure_of(&settled);
    assert_settled_at_entry(&failure);
    assert!(!failure.sensitive, "settled as any task's: {failure:?}");
    let record = latest_value_record(&host);
    assert!(
        record.daily_counts.iter().all(|count| count.count == 0),
        "the run's count is given back: {record:?}"
    );
    assert!(record.failure_streak.is_none(), "no streak: {record:?}");
    assert_eq!(blocked_by(&host).decision_id, intent.decision_id);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// N3 (a) (fix round 5, ruling 3, case (a); the settlement hook, three tasks, `escalation_threshold`
/// 2, `scheduling.unavailable_probe`'s first and longest interval one minute, so a pair is at the
/// maximum from its first failure): an outage. `TASK`, first in the order, fails at its entry, then
/// `BROKEN` and `OTHER` (both healthy here) fail too while it waits, each block `BLOCK_MS` long:
/// every pair waits the maximum from the last block's end, all due together. The outage ends:
/// `TASK`, whose entry stays broken, is dispatched first and fails again, its second in a row, and
/// past the maximum it waits one first interval more (ruling 3 (i)), so in the round after that
/// block's end it waits (`count` 2) and the healthy `BROKEN` is dispatched and succeeds. That run
/// let in ends every pair's wait at once (ruling 3 (ii)): no reason holds `TASK` or `OTHER`, `TASK`
/// (first in the order) is dispatched once `BROKEN`'s lease has lapsed and its failure counts
/// against it; after that block's end the healthy `BROKEN` runs again, which keeps that count
/// (ruling 3 (ii) touches no count against a pair), and `TASK`'s next failure is the second against
/// it: set aside for its window, one person told.
#[test]
fn entry_n3_a_after_an_outage_the_pairs_at_the_maximum_come_due_apart_and_a_run_let_in_ends_every_wait()
 {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(
        &root,
        instance_id(),
        POLICY_NOW_UNIX_MS,
        paced_between(FIRST_INTERVAL_MS, FIRST_INTERVAL_MS, json!({})).manifest,
    );
    host.activate_policy_catalog(&three_tasks(2))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    assert_eq!(first.task_id, TASK);
    let settled = settle_at_entry_after(&host, &clock, &first, ENTRY_RUN_MS);
    let mut block_end = settled.observed_at_unix_ms + BLOCK_MS;
    recover_at(&host, &clock, block_end);
    for (task_id, seed) in [(BROKEN, 101), (OTHER, 102)] {
        let intent = admit_at(&host, &clock, block_end + 1_000, seed);
        assert_eq!(intent.task_id, task_id, "the outage");
        let settled = settle_at_entry_after(&host, &clock, &intent, ENTRY_RUN_MS);
        block_end = settled.observed_at_unix_ms + BLOCK_MS;
        recover_at(&host, &clock, block_end);
    }
    for task_id in [TASK, BROKEN, OTHER] {
        assert_eq!(
            pair(&host, task_id),
            Some((1, 0, block_end + FIRST_INTERVAL_MS, false)),
            "{task_id}: at the maximum, due with the others"
        );
    }

    let again = admit_at(&host, &clock, block_end + FIRST_INTERVAL_MS, 103);
    assert_eq!(again.task_id, TASK, "first in the order");
    let settled = settle_at_entry_after(&host, &clock, &again, ENTRY_RUN_MS);
    block_end = settled.observed_at_unix_ms + BLOCK_MS;
    recover_at(&host, &clock, block_end);
    assert_eq!(
        pair(&host, TASK),
        Some((2, 0, block_end + 2 * FIRST_INTERVAL_MS, false)),
        "past the maximum, one first interval more"
    );
    assert_eq!(
        pair(&host, BROKEN),
        Some((1, 0, block_end + FIRST_INTERVAL_MS, false))
    );
    let at = block_end + FIRST_INTERVAL_MS;
    advance_manual_clock_to(&clock, at);
    let round = evaluate(&host, at, 104);
    assert!(dispatched(&round, BROKEN), "{round:#?}");
    let waiting = reason_of(&round, TASK, HostCode::InstanceEntryTaskWaiting)
        .unwrap_or_else(|| panic!("the broken task waits: {round:#?}"));
    assert_eq!(waiting.value("count"), Some(&json!(2)));
    assert_eq!(
        waiting.value("next_eligible_unix_ms"),
        Some(&json!(block_end + 2 * FIRST_INTERVAL_MS))
    );
    let healthy = admit_at(&host, &clock, at, 105);
    assert_eq!(
        healthy.task_id, BROKEN,
        "the healthy task, in the round after"
    );
    let let_in = succeed(&host, &clock, &healthy);
    for task_id in [TASK, OTHER] {
        assert_eq!(
            pair(&host, task_id),
            Some((0, 0, 0, false)),
            "{task_id}: its wait ended at once"
        );
    }
    assert_eq!(pair(&host, BROKEN), None, "its own run got in");

    let at = let_in.observed_at_unix_ms + LEASE_LAPSED_MS;
    advance_manual_clock_to(&clock, at);
    let round = evaluate(&host, at, 106);
    for task_id in [TASK, OTHER] {
        assert_eq!(
            reason_of(&round, task_id, HostCode::InstanceEntryTaskWaiting),
            None,
            "{task_id}: {round:#?}"
        );
    }
    let broken = admit_at(&host, &clock, at, 107);
    assert_eq!(broken.task_id, TASK, "first in the order, no longer held");
    let settled = settle_at_entry_after(&host, &clock, &broken, ENTRY_RUN_MS);
    assert_eq!(
        pair(&host, TASK),
        Some((1, 1, settled.observed_at_unix_ms + FIRST_INTERVAL_MS, false)),
        "it came although a run was let in: one against it"
    );
    block_end = settled.observed_at_unix_ms + BLOCK_MS;
    recover_at(&host, &clock, block_end);
    let healthy = admit_at(&host, &clock, block_end + 1_000, 108);
    assert_eq!(healthy.task_id, BROKEN);
    let let_in = succeed(&host, &clock, &healthy);
    assert_eq!(
        pair(&host, TASK),
        Some((0, 1, 0, false)),
        "the wait ended, the count against it kept"
    );
    let last = admit_at(
        &host,
        &clock,
        let_in.observed_at_unix_ms + LEASE_LAPSED_MS,
        109,
    );
    assert_eq!(last.task_id, TASK);
    settle_at_entry_after(&host, &clock, &last, ENTRY_RUN_MS);
    assert_eq!(
        pair(&host, TASK).map(|(_, against, _, set_aside)| (against, set_aside)),
        Some((2, true)),
        "set aside at the threshold"
    );
    let reports = set_aside_reports(&host);
    let [report] = reports.as_slice() else {
        panic!("one person told: {reports:#?}");
    };
    assert_eq!(report.value("task_id"), Some(&json!(TASK)));
    assert_eq!(report.value("count"), Some(&json!(2)));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// N3 (b) (fix round 5, ruling 3 (i); the settlement hook, one task, `escalation_threshold` 3,
/// `scheduling.unavailable_probe`'s first interval one minute, its longest two, factor 2): four
/// failures at the entry in a row with nothing let in, each block `BLOCK_MS`, each next run at its
/// wait's end: after the first the pair waits one minute from its block's end, after the second
/// two (the maximum), after the third three and after the fourth four (past the maximum, one first
/// interval more for each further failure in a row).
#[test]
fn entry_n3_b_past_the_maximum_each_further_failure_in_a_row_waits_one_first_interval_more() {
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(
        &root,
        instance_id(),
        POLICY_NOW_UNIX_MS,
        paced_between(FIRST_INTERVAL_MS, 2 * FIRST_INTERVAL_MS, json!({})).manifest,
    );
    host.activate_policy_catalog(&sources(3))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    let mut settled = settle_at_entry_after(&host, &clock, &first, ENTRY_RUN_MS);
    for (in_a_row, minutes, seed) in [(1_u32, 1_u64, 101), (2, 2, 102), (3, 3, 103), (4, 4, 104)] {
        let block_end = settled.observed_at_unix_ms + BLOCK_MS;
        recover_at(&host, &clock, block_end);
        let until = block_end + minutes * FIRST_INTERVAL_MS;
        assert_eq!(
            pair(&host, TASK),
            Some((in_a_row, 0, until, false)),
            "{in_a_row}"
        );
        if in_a_row < 4 {
            let next = admit_at(&host, &clock, until, seed);
            settled = settle_at_entry_after(&host, &clock, &next, ENTRY_RUN_MS);
        }
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// N4 (fix round 5, ruling 4; policy level, gate_upd1's store update task, the ordinary task's
/// `escalation_threshold` 1): the ordinary task fails at its entry and waits its first interval.
/// The game asks for an update; the store update task passes the block, its main package starts
/// after its entry check and it fails (severe, so it is not rerun): that is no run let in for the
/// pairs, so the ordinary task's pair is as it was, its wait not ended. A recovery ends the block;
/// the ordinary task's next failure at the entry is its second in a row and none against it: not
/// set aside, no person told.
#[test]
fn entry_n4_a_store_update_task_whose_main_package_started_and_failed_is_no_run_let_in() {
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
    host.activate_policy_catalog(&store_sources_threshold_one())
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
    let grant = |evaluation: &actingcommand_policy::PolicyEvaluation, task_id: &str| {
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == task_id)
            .cloned()
            .unwrap_or_else(|| panic!("{task_id} is dispatched: {evaluation:#?}"));
        let chain = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .cloned()
            .expect("its reason chain");
        let PolicyDispatchAdmission::Granted { context } = host
            .admit_policy_dispatch(&intent, &chain, &policy_context(&host, &intent))
            .expect("the admission")
        else {
            panic!("{task_id} is granted");
        };
        (intent, context)
    };

    let first = evaluate_at(POLICY_NOW_UNIX_MS + 1_000, 1);
    let ordinary_intent = first
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == TASK)
        .cloned()
        .unwrap_or_else(|| panic!("the ordinary task: {first:#?}"));
    record_policy_approval(&host, &ordinary_intent);
    let (intent, context) = grant(&first, TASK);
    let settled = settle_at_entry_after(&host, &clock, &intent, ENTRY_RUN_MS);
    assert_settled_at_entry(&failure_of(&settled));
    host.release_policy_dispatch_lease_for_test(&context)
        .expect("end the dispatch's lease");
    let waiting = (1, 0, settled.observed_at_unix_ms + FIRST_INTERVAL_MS, false);
    assert_eq!(pair(&host, TASK), Some(waiting));

    advance_manual_clock_to(&clock, POLICY_NOW_UNIX_MS + MINUTE_MS);
    host.record_application_update_for_test(POLICY_INSTANCE_ALIAS, true)
        .expect("the game asks for an update");
    let store_at = POLICY_NOW_UNIX_MS + ACTIVITY_INTERVAL_PAST_MS;
    let update = evaluate_at(store_at, 2);
    let (store_intent, store_context) = grant(&update, STORE_TASK);
    clock.advance(ENTRY_RUN_MS);
    host.settle_next_policy_run_past_its_entry_for_test()
        .expect("arm the settlement");
    let store_failure = failure_of(
        &host
            .record_policy_dispatch_outcome(
                &store_intent.decision_id,
                &PolicyExecutionInput::Failed {
                    outcome: Outcome::new(
                        actingcommand_execution_kernel::ExecutionKernelCode::PageConfirmationFailed
                            .code(),
                    ),
                    failure_identity: None,
                    class: PolicyFailureClass::Severe,
                },
            )
            .expect("the store update task's settlement"),
    );
    assert_eq!(store_failure.condition, None);
    host.release_policy_dispatch_lease_for_test(&store_context)
        .expect("end the dispatch's lease");
    assert_eq!(
        pair(&host, TASK),
        Some(waiting),
        "no run let in for the pairs: no wait ended"
    );
    host.record_application_update_for_test(POLICY_INSTANCE_ALIAS, false)
        .expect("the game is up to date");
    recover(&host);

    let again = evaluate_at(store_at + ACTIVITY_INTERVAL_PAST_MS, 3);
    let (intent, context) = grant(&again, TASK);
    let settled = settle_at_entry_after(&host, &clock, &intent, ENTRY_RUN_MS);
    assert_settled_at_entry(&failure_of(&settled));
    host.release_policy_dispatch_lease_for_test(&context)
        .expect("end the dispatch's lease");
    let (in_a_row, against, _, set_aside) = pair(&host, TASK).expect("the pair");
    assert_eq!(
        (in_a_row, against, set_aside),
        (2, 0, false),
        "nothing counts against it"
    );
    assert!(set_aside_reports(&host).is_empty(), "no one told");
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
/// when it is not the scheduled package. Fix round 4: `unchained`, a `main` that declares no
/// prerequisite (a page graph whose entry check is its own, M6); `emulator`, the instance has
/// emulator control and a discovery binding, so a ladder's emulator rung runs (M4).
#[derive(Default)]
struct Setup<'a> {
    manifest: Option<RuntimeConfigManifest>,
    startup: Option<&'a Pack<'a>>,
    unchained: bool,
    emulator: bool,
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
        ..Setup::default()
    }
}

/// Fix round 3 (K3): [`shipped`] but for `scheduling.unavailable_probe`'s first and longest
/// interval, `PACED_MS`.
fn paced<'a>(ladder: Value) -> Setup<'a> {
    paced_between(PACED_MS, PACED_MS, ladder)
}

/// Fix round 4 (K3 a): [`shipped`] but for `scheduling.unavailable_probe`'s first interval
/// `first_ms` and longest `max_ms` (the shipped factor 2).
fn paced_between<'a>(first_ms: u64, max_ms: u64, ladder: Value) -> Setup<'a> {
    let file = json!({
        "recovery_ladder": ladder,
        "scheduling": {
            "blackout": {"host": []},
            "unavailable_probe": {"first_interval_ms": first_ms, "max_interval_ms": max_ms}
        },
        "instances": [],
    });
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(&mechanism_rows(), &[], &defaults, &file, &|_| None)
        .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    Setup {
        manifest: Some(RuntimeConfigManifest {
            subsystems: Vec::new(),
            parameters,
        }),
        ..Setup::default()
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
    let main = route(
        MAIN,
        "menu",
        "done",
        OPEN,
        (!setup.unchained).then_some(LEAD),
    );
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
    if setup.emulator {
        // The emulator rung's readiness passes once its Start ran (Android boots, the opens pass).
        state.observed_open.store(true, Ordering::Release);
        bind_fake_emulator(&host, POLICY_INSTANCE_ALIAS, instance_id(), &state);
    }
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
        failed_execution(&self.host, decision_id)
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

    /// Fix round 3 (ruling 3): the climbs of the instance's stretch, which must run, and when the
    /// latest ended.
    fn climbs(&self) -> (u32, u64) {
        self.host
            .entry_climbs_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's stretch")
            .expect("a stretch runs")
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

/// The execution record of the dispatch `decision_id` on `host`, which failed.
fn failed_execution(host: &RuntimeHost, decision_id: &str) -> PolicyFailureRecord {
    all_events(host)
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
    assert_eq!(
        replay.restarts(),
        Some(true),
        "fix round 5 (ruling 5): after an exhausted climb every later climb of the block, \
         whichever path builds it, starts at the application restart"
    );
    let attention = the_exhausted_recovery(&replay.host);
    assert_eq!(attention.value("task_id"), Some(&json!(TASK)));
    assert_eq!(
        attention.value("decision_id"),
        Some(&json!(replay.context.decision_id()))
    );
    assert_eq!(attention.value("failure_code"), Some(&json!(PAGE_UNKNOWN)));

    assert_eq!(
        replay
            .host
            .entry_climbs_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's stretch")
            .map(|(climbs, _)| climbs),
        Some(1),
        "fix round 3: the exhausted climb counts in the block's stretch"
    );
    let receipt = manual_run(&replay.host, replay.root.path());
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "a client's run on a blocked instance is not refused: {receipt:?}"
    );
    assert_eq!(
        replay
            .host
            .entry_climbs_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's stretch"),
        None,
        "fix round 3: a client's run that succeeded let a run in, which ends the stretch"
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
    store_sources_at(1)
}

/// [`store_sources`] as the catalog version `version` (fix round 3, K4: activated after the
/// catalog fixture's version 1).
fn store_sources_at(version: u64) -> CatalogSources {
    let mut sources = policy_sources(version);
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

/// R1 (ruling 1; the shipped defaults, system clock): the game is on `shop`; `main`'s entry
/// `menu` is not seen, so its prerequisite `lead` runs as its layer, and the run's second capture,
/// in that layer, fails in the capture backend (`capture_backend_operation_failed`, class
/// `environment_fault`, which the shipped item `task_capture_backend` takes). It is no failure at
/// the entry, whatever the triggers say of its code: settled as before this slice, with no
/// condition (its identity's `F` unique, not `i…`), its admission kept, no count of failures at
/// the entry and no block; its lease end starts the ladder its trigger item names, as before.
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
    assert_eq!(
        pair(&replay.host, TASK),
        None,
        "no count of failures at the entry"
    );
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

/// R2 (ruling 2, kept by fix round 3's ruling 5; the settlement hook, `escalation_threshold` 3):
/// the first block since the Runtime started climbs as its trigger item says. A recovery ends it
/// and lets nothing in, so the task's next failure at the entry (once its wait has passed, fix
/// round 4) sets a block that climbs from the application restart. That block ends the same way,
/// a scheduled run's main package then starts on the instance after its entry check, and the
/// task's next failure at the entry sets a block that climbs as configured; each failure is a
/// condition (fix round 4, ruling 1: never the rule's failure).
#[test]
fn entry_r2_a_block_with_nothing_let_in_since_the_previous_one_climbs_from_the_application_restart()
{
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&sources(3))
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    settle_at_entry(&host, &clock, &first);
    assert_eq!(
        restarts(&host),
        Some(false),
        "the first block since the Runtime started"
    );
    for (seed, let_in, from_restart) in [(101, false, true), (102, true, false)] {
        recover(&host);
        if let_in {
            host.let_run_in_for_test(POLICY_INSTANCE_ALIAS)
                .expect("a run's main package starts");
        }
        // Fix round 5 (ruling 3 (ii)): a run let in ends the pair's wait (`until` 0); the next
        // run then goes once the lease of the one before has lapsed.
        let (_, _, until, _) = pair(&host, TASK).expect("the pair");
        let now = clock.sample().expect("the manual clock").unix_ms;
        let next = admit_at(&host, &clock, until.max(now + LEASE_LAPSED_MS), seed);
        let settled = settle_at_entry(&host, &clock, &next);
        assert_settled_at_entry(&failure_of(&settled));
        assert_eq!(blocked_by(&host).decision_id, next.decision_id);
        assert_eq!(restarts(&host), Some(from_restart), "{seed}");
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// R3 (a) (rulings 2 and 3 of fix round 2, kept by fix round 3; the live settlement on a fake
/// device, manual clock): the game is on `away`, which `lead` does not know. The scheduled run
/// fails at its entry: a condition, the pair's first in a row. The first block since the Runtime
/// started climbs as configured, from the return-home rung: the bound return-home package leaves
/// `away` for `done` and the ladder ends `recovered` there, which ends the block. At the task's
/// next occurrence (the cool-down window opened again) the same task meets `done`, which `lead`
/// does not know, and fails at its entry again: a condition again, its second in a row. Its block
/// was set with nothing let in since the previous one ended: its run's recovery, a later climb of
/// the stretch (fix round 3, ruling 3), is held, due no later than the cool-down window's end (fix
/// round 4: the shipped first interval after the first climb's end has passed long since, so the
/// lease sweep's own tick may already have moved it to the window's end, which the window asks
/// anyway: the cause of fix round 3's red run, which read the earlier time); at the window's end it
/// climbs from the application restart, the return-home rung skipped before its start rung; the
/// startup package does not know `done` either, the climb ends `exhausted`, and one person is
/// told, with the time of the block's next climb.
#[test]
fn entry_r3_a_after_a_recovery_at_the_return_home_rung_the_next_block_climbs_from_the_application_restart()
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
    assert_eq!(
        pair(&replay.host, TASK).map(|(in_a_row, against, _, _)| (in_a_row, against)),
        Some((1, 0)),
        "the pair's first failure at the entry"
    );
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
    let (climbs, first_end) = replay.climbs();
    assert_eq!(climbs, 1, "the first climb of the stretch");

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
    assert_eq!(
        pair(&replay.host, TASK).map(|(in_a_row, against, _, _)| (in_a_row, against)),
        Some((2, 0)),
        "its second in a row, nothing let in between"
    );
    let (gate, _, held) = replay.block();
    assert_eq!(gate.decision_id, second.decision_id());
    assert!(
        held.is_some_and(|until| until == first_end + FIRST_INTERVAL_MS || until == window_until),
        "the block holds its run's recovery, paced from the first climb's end and kept to the \
         cool-down window's end: {held:?}"
    );
    assert_eq!(
        started_plans(&replay.host).len(),
        1,
        "not climbed before the window's end"
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
/// blocks the instance and begins its stretch; a recovery ends the block and lets nothing in, and
/// the stretch runs on; the task's next failure at the entry (once its wait has passed, fix round
/// 4) blocks the instance again, in the same stretch. A millisecond before the stretch has lasted
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
    settle_at_entry(&host, &clock, &first);
    let (since, told) = stretch(&host).expect("the first block begins a stretch");
    assert!(!told);
    recover(&host);
    assert_eq!(
        stretch(&host),
        Some((since, false)),
        "a recovery that let nothing in does not end the stretch"
    );
    let (_, _, until, _) = pair(&host, TASK).expect("the pair waits");
    let second = admit_at(&host, &clock, until, 101);
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

// ---- Fix round 3.

/// K3 (a) (ruling 3; the live path on a fake device, manual clock, `scheduling.unavailable_probe`'s
/// first interval an hour and its longest two; fix round 4: a pair's first wait is an hour too,
/// from its block's end): the instance's `pack_logic` item starts its climbs at the application
/// restart and its startup package leaves `menu` for `done`; the game is on `away` until the test
/// mends it. The scheduled run fails at its entry; its block's first climb ends `exhausted` and the
/// block holds the next climb for the first interval after that end. The game is mended; there
/// the block climbs again, the startup package recovers and the ladder ends `recovered` with
/// nothing let in: two climbs of the stretch, the second ended then, and the pair waits its hour
/// from that end. When that wait has passed the task meets `done` and fails at its entry again;
/// its lease end, a later climb of the same stretch, admits no ladder: the block holds the recovery
/// until the stretch's second interval (two hours) after the second climb's end, an hour after the
/// pair's wait. A millisecond before, the sweep's tick takes up nothing; at that time it climbs,
/// from the application restart (ruling 5).
#[test]
fn entry_k3_a_a_later_climb_after_a_recovery_that_let_nothing_in_waits_its_interval() {
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
            ..paced_between(HOUR_MS, PACED_MS, restart_first())
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
    wait_for_ladders_finished(&replay.host, 1);
    assert_eq!(
        finished_ladders(&replay.host),
        vec![RecoveryLadderOutcome::Exhausted],
        "the startup package does not know away"
    );
    let (climbs, first_end) = replay.climbs();
    assert_eq!(climbs, 1);
    let next = replay.held_until();
    assert_eq!(next, first_end + HOUR_MS, "the first interval after it");

    mended.store(true, Ordering::Release);
    replay.tick_at(next);
    wait_for_ladders_finished(&replay.host, 2);
    assert_eq!(
        finished_ladders(&replay.host),
        vec![
            RecoveryLadderOutcome::Exhausted,
            RecoveryLadderOutcome::Recovered
        ]
    );
    assert!(!replay.blocked());
    let (climbs, second_end) = replay.climbs();
    assert_eq!(climbs, 2);
    let paced_until = second_end + PACED_MS;
    let (_, _, until, _) = pair(&replay.host, TASK).expect("the pair waits");
    assert_eq!(until, second_end + HOUR_MS, "from the block's end");

    let second = admit_scheduled_at(
        &replay.host,
        replay.manual_clock(),
        &replay.request,
        until,
        301,
    );
    replay.fail_at_entry_of(&second);
    assert_settled_at_entry(&replay.execution_of(second.decision_id()));
    assert_eq!(
        started_plans(&replay.host).len(),
        2,
        "the later climb is not admitted at the lease end"
    );
    assert_eq!(replay.held_until(), paced_until);
    replay.tick_at(paced_until - 1);
    assert_eq!(started_plans(&replay.host).len(), 2, "not before its time");
    replay.tick_at(paced_until);
    wait_for_ladders_finished(&replay.host, 3);
    let plans = started_plans(&replay.host);
    let [_, _, (trigger, plan)] = plans.as_slice() else {
        panic!("three climbs: {plans:#?}");
    };
    assert_eq!(trigger.run_id, Some(second.run_id()));
    assert_eq!(*plan, from_the_application_restart());
    replay.close();
}

/// K3 (b) (ruling 3; the live path on a fake device, manual clock, the first interval two hours):
/// the instance's `pack_logic` item starts its climbs at the application restart. The scheduled
/// run fails at its entry and blocks the instance; just before its climb's first step, business
/// capacity starts refusing new work, so the climb ends interrupted (`exhausted` at Warning,
/// `recovery_ladder_interrupted`) and no person is told. Capacity returns. Twenty minutes later,
/// past the cool-down window the climb opened, the sweep's tick takes up nothing; a millisecond
/// before the interval after the interrupted climb's end, nothing either; at that time the block
/// climbs again.
#[test]
fn entry_k3_b_after_a_climb_business_capacity_interrupted_the_next_waits_its_interval() {
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
            ..paced(restart_first())
        },
    );
    let (at_check, go) = hold_next_rung_step(&replay.host);
    replay.fail_at_entry();
    assert_settled_at_entry(&replay.execution());
    at_check
        .recv_timeout(GATE_WAIT)
        .expect("the climb's holding check before its first step");
    let available_bytes = Arc::new(AtomicU64::new(0));
    replay
        .host
        .replace_capacity_sampler_for_test(Box::new(PressureSampler {
            available_bytes: Arc::clone(&available_bytes),
        }))
        .expect("hard pressure committed");
    go.send(()).expect("the climb goes on");
    wait_for_ladder_end(&replay.host, POLICY_INSTANCE_ALIAS);
    let events = all_events(&replay.host);
    let (finished, result, _) = ladder_finished(&events).expect("the climb's end");
    assert_eq!(result, RecoveryLadderOutcome::Exhausted);
    assert_eq!(
        ladder_end_code(finished),
        Some("recovery_ladder_interrupted")
    );
    assert_no_one_told(&replay.host);
    let (climbs, end) = replay.climbs();
    assert_eq!(climbs, 1, "the interrupted climb counts in the stretch");
    available_bytes.store(
        actingcommand_contract::CapacityThresholds::default().soft_bytes,
        Ordering::Release,
    );

    replay.tick_at(end + 2 * COOLDOWN_LEFT_MS);
    assert_eq!(
        started_plans(&replay.host).len(),
        1,
        "past the cool-down window, not past the interval"
    );
    replay.tick_at(end + PACED_MS - 1);
    assert_eq!(started_plans(&replay.host).len(), 1, "not before its time");
    replay.tick_at(end + PACED_MS);
    wait_for_ladders_finished(&replay.host, 2);
    let plans = started_plans(&replay.host);
    let [_, (trigger, _)] = plans.as_slice() else {
        panic!("two climbs: {plans:#?}");
    };
    assert_eq!(trigger.run_id, Some(replay.context.run_id()));
    replay.close();
}

/// K3 (c) (rulings 3 and 5; the live path on a fake device, manual clock, the first interval two
/// hours): the game is on `away`, which no package of the instance knows. The scheduled run fails
/// at its entry, its block's first climb ends `exhausted`: one climb of the stretch. A client's
/// run leaves `away` for `menu` and succeeds: it ends the block and lets a run in, which ends the
/// stretch. The game is on `away` again; once the task's own wait has passed (fix round 4) it
/// fails at its entry: its block, the first of a new stretch, climbs as configured, from the
/// return-home rung, at once from its lease end, with nothing held.
#[test]
fn entry_k3_c_after_a_run_got_in_the_next_blocks_climb_does_not_wait_and_starts_as_configured() {
    let replay = replay_set_up(
        &Scene {
            return_home: &route(RETURN_HOME, "shop", "done", NOWHERE_STEPS, None),
            lead: &lead(),
            registry_return_home: None,
            screens: vec![AWAY],
        },
        true,
        paced(json!({})),
    );
    let away = Arc::new(AtomicBool::new(true));
    let from = Arc::new(AtomicUsize::new(0));
    let (seen_away, seen_from) = (Arc::clone(&away), Arc::clone(&from));
    *replay.state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, inputs: usize| {
            let screen = if !seen_away.load(Ordering::Acquire)
                && inputs > seen_from.load(Ordering::Acquire)
            {
                MENU
            } else {
                AWAY
            };
            vec![(0, screen)]
        }));
    replay.fail_at_entry();
    assert_settled_at_entry(&replay.execution());
    wait_for_ladder_end(&replay.host, POLICY_INSTANCE_ALIAS);
    assert_eq!(
        finished_ladders(&replay.host),
        vec![RecoveryLadderOutcome::Exhausted]
    );
    assert_eq!(replay.climbs().0, 1);

    from.store(
        replay.state.input_count.load(Ordering::Acquire),
        Ordering::Release,
    );
    away.store(false, Ordering::Release);
    let receipt = manual_run(&replay.host, replay.root.path());
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
    assert!(!replay.blocked(), "the client's run ends the block");
    assert_eq!(
        replay
            .host
            .entry_climbs_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's stretch"),
        None,
        "and lets a run in: the stretch ends"
    );

    away.store(true, Ordering::Release);
    // Fix round 4 (ruling 2): the task is dispatched once its own wait (two hours here) has
    // passed; the client's run does not end it.
    let (_, _, until, _) = pair(&replay.host, TASK).expect("the pair waits");
    let second = admit_scheduled_at(
        &replay.host,
        replay.manual_clock(),
        &replay.request,
        until,
        301,
    );
    replay.fail_at_entry_of(&second);
    assert_settled_at_entry(&replay.execution_of(second.decision_id()));
    assert_eq!(
        replay.restarts(),
        Some(false),
        "a run got in since the previous block ended: as configured"
    );
    wait_until("the next block's climb", || {
        started_plans(&replay.host).len() == 2
    });
    let plans = started_plans(&replay.host);
    let [_, (trigger, plan)] = plans.as_slice() else {
        panic!("two climbs: {plans:#?}");
    };
    assert_eq!(trigger.run_id, Some(second.run_id()));
    assert_eq!(
        plan.first(),
        Some(&planned(RecoveryRung::ReturnHome, None)),
        "from the first rung"
    );
    wait_for_ladders_finished(&replay.host, 2);
    replay.close();
}

/// K4 (ruling 4; the live path on a fake device, manual clock, gate_373_nav6's update dialog): the
/// game root's `notice` is the game's update dialog. The scheduled run (a page graph whose entry is
/// `away`) meets a screen no package knows and fails at its entry (`contained_task_home_recovery_
/// binding_missing`: no recovery package): a condition, the instance blocked. Its ladder skips the
/// return-home rung (no package), restarts the application, and its startup package meets the
/// update dialog, which ends the climb (`exhausted` at Warning, `recovery_ladder_interrupted`). One
/// `instance_entry_blocked_recovery_failed` names it (`entry_recovery_end` `update_dialog`, no next
/// climb), the block holds nothing, and twenty minutes later, past the cool-down window, the
/// sweep's tick climbs nothing. The store update task is dispatched on the blocked instance.
#[test]
fn entry_k4_the_update_dialog_ends_the_climb_tells_a_person_once_and_nothing_climbs_again() {
    let state = Arc::new(FakeState::default());
    let temp = TempDir::new().expect("tempdir");
    let root = update_root(temp.path());
    let (host_config, bound) = ladder_setup_with_package(temp.path(), &entry_away_to_home(false));
    // The scheduled run binds no return-home package, and the game names none.
    let request = ContainedTaskRequest::new(bound.package_path().to_owned())
        .expect("scheduled request without a return-home binding");
    let store_path = temp.path().join("store-update.zip");
    fs::write(&store_path, super::gate_store_fg2::store_package()).expect("write the package");
    let procedures = ProcedureManifest::new(vec![
        ProcedureBinding::new(
            "procedure.observe",
            "operation.observe",
            vec!["after_observation".to_owned()],
        )
        .expect("procedure binding"),
        ProcedureBinding::new(
            STORE_PROCEDURE,
            "operation.observe",
            vec!["after_observation".to_owned()],
        )
        .expect("procedure binding")
        .with_package_path(store_path.to_string_lossy()),
    ])
    .expect("manifest");
    // Before the application restart an unknown screen; after it the update dialog until the next
    // input (its handler's tap), then `away`.
    let device = Arc::downgrade(&state);
    let restarted_at = std::sync::Mutex::new(None::<usize>);
    script(&state, move |_, inputs| {
        let applications = device
            .upgrade()
            .map_or(0, |state| state.application_count.load(Ordering::Acquire));
        if applications == 0 {
            return frame(BLACK, None);
        }
        let mut restarted_at = restarted_at.lock().expect("the restart's inputs");
        if *restarted_at.get_or_insert(inputs) == inputs {
            frame(RED, Some(Overlay::Notice))
        } else {
            frame(RED, None)
        }
    });
    state.physical_task_geometry.store(true, Ordering::Release);
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        host_config
            .with_procedure_manifest(procedures)
            .with_standard_packs_root(Some(root.root.clone()))
            .with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let context = admit_ladder_run(&host, &request);
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails at its entry");
    assert!(!error.is_fatal(), "{}", error.code());
    let failure = failed_execution(&host, context.decision_id());
    assert!(
        crate::policy_control::entry_blocked(&failure),
        "a failure at the entry, settled as a condition: {failure:?}"
    );
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);
    let events = all_events(&host);
    let (finished, result, _) = ladder_finished(&events).expect("the climb's end");
    assert_eq!(result, RecoveryLadderOutcome::Exhausted);
    assert_eq!(
        ladder_end_code(finished),
        Some("recovery_ladder_interrupted")
    );
    assert!(
        rung_outcomes(&events).contains(&(
            RecoveryRung::ApplicationRestart,
            RecoveryRungOutcome::Failed,
            Some("contained_task_application_update_required".to_owned())
        )),
        "the update dialog ended the rung's run: {:?}",
        rung_outcomes(&events)
    );
    let failures = failed_outcomes(&host, HostCode::InstanceEntryBlockedRecoveryFailed);
    let [told] = failures.as_slice() else {
        panic!("one person told: {failures:#?}");
    };
    assert_eq!(
        told.value("entry_recovery_end"),
        Some(&json!("update_dialog"))
    );
    assert_eq!(told.value("next_occurrence_unix_ms"), None, "no next climb");
    let (_, awaiting, held) = host
        .entry_block_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the instance's block")
        .expect("the instance stays blocked");
    assert!(awaiting);
    assert_eq!(held, None, "nothing held");

    let later = POLICY_NOW_UNIX_MS + 2 * COOLDOWN_LEFT_MS;
    advance_manual_clock_to(&clock, later);
    host.capacity_sampler_for_test().expect("capacity owner")()
        .expect("capacity sample at the tick");
    host.entry_block_tick_for_test().expect("the sweep's tick");
    let started = phases(&all_events(&host))
        .iter()
        .filter(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::RecoveryLadderStarted { .. }))
        .count();
    assert_eq!(started, 1, "nothing climbs again in the stretch");
    assert_eq!(
        failed_outcomes(&host, HostCode::InstanceEntryBlockedRecoveryFailed).len(),
        1,
        "told once"
    );

    host.activate_policy_catalog(&store_sources_at(2))
        .expect("activate the store update catalog");
    let update = host
        .evaluate_policy_cycle_with_test_inputs(
            &update_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: later,
                monotonic_ms: clock.monotonic_ms(),
            },
            2,
            PolicyTrigger::Recovery,
        )
        .expect("evaluate the policy")
        .evaluation
        .expect("a full evaluation");
    assert!(
        dispatched(&update, STORE_TASK),
        "the store update task passes the entry block: {update:#?}"
    );
    assert_eq!(block_reason(&update, STORE_TASK), None);
    assert!(!dispatched(&update, TASK), "{update:#?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// ---- Fix round 4.

/// M4: the emulator's Stops and Starts so far.
fn emulator_actions(state: &FakeState) -> Vec<actingcommand_contract::EmulatorInstanceAction> {
    state
        .emulator_actions
        .lock()
        .expect("fake emulator actions lock")
        .clone()
}

/// M4 (ruling 4; the live path on a fake device with emulator control, manual clock): the game is
/// on `away`, which no package of the instance knows. The scheduled run fails at its entry; its
/// block's first climb runs every rung, the emulator's Stop and Start included. At the task's next
/// occurrence it fails at its entry again, nothing let in between: the stretch's second climb starts
/// at the application restart and skips the emulator's rung (planned pending, as a plan drill's;
/// recorded `skipped` with `recovery_rung_skipped_emulator_restarted_in_stretch`); the emulator is
/// not stopped again. A client's run then leaves `away` and succeeds, which lets a run in and ends
/// the stretch; the task's next failure at the entry begins a new stretch, whose first climb plans
/// the emulator's rung as
/// configured and restarts the emulator once more.
#[test]
fn entry_m4_the_emulator_is_restarted_once_in_a_stretch_and_again_in_the_next() {
    use actingcommand_contract::EmulatorInstanceAction::{Start, Stop};
    let replay = replay_set_up(
        &Scene {
            return_home: &route(RETURN_HOME, "shop", "done", NOWHERE_STEPS, None),
            lead: &lead(),
            registry_return_home: None,
            screens: vec![AWAY],
        },
        true,
        Setup {
            emulator: true,
            ..Setup::default()
        },
    );
    let away = Arc::new(AtomicBool::new(true));
    let from = Arc::new(AtomicUsize::new(0));
    let (seen_away, seen_from) = (Arc::clone(&away), Arc::clone(&from));
    *replay.state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, inputs: usize| {
            let screen = if !seen_away.load(Ordering::Acquire)
                && inputs > seen_from.load(Ordering::Acquire)
            {
                MENU
            } else {
                AWAY
            };
            vec![(0, screen)]
        }));
    replay.fail_at_entry();
    assert_settled_at_entry(&replay.execution());
    wait_for_ladders_finished(&replay.host, 1);
    assert_eq!(
        emulator_actions(&replay.state),
        [Stop, Start],
        "the stretch's first climb restarts the emulator"
    );
    let plans = started_plans(&replay.host);
    assert_eq!(
        plans.first().and_then(|(_, plan)| plan.last()),
        Some(&planned(RecoveryRung::EmulatorRestart, None))
    );
    // Fix round 5 (ruling 2, the fifth review's M4): the instance's startup package is `main`,
    // which fails on `away`, so the first climb ends `exhausted` and the block stands; a
    // recovery ends it with nothing let in, as M6 does, so the task is dispatched at its next
    // occurrence and its next block is in the same stretch.
    recover(&replay.host);

    let second = admit_scheduled_at(
        &replay.host,
        replay.manual_clock(),
        &replay.request,
        NEXT_DISPATCH_UNIX_MS,
        301,
    );
    replay.fail_at_entry_of(&second);
    assert_settled_at_entry(&replay.execution_of(second.decision_id()));
    wait_until("the stretch's second climb", || {
        replay.tick();
        started_plans(&replay.host).len() == 2
    });
    wait_for_ladders_finished(&replay.host, 2);
    let plans = started_plans(&replay.host);
    let (trigger, plan) = &plans[1];
    assert_eq!(trigger.run_id, Some(second.run_id()));
    assert_eq!(
        *plan,
        vec![
            planned(
                RecoveryRung::ReturnHome,
                Some(RecoveryRungSkipReason::BeforeStartRung),
            ),
            planned(RecoveryRung::ApplicationRestart, None),
            planned(RecoveryRung::EmulatorRestart, None),
        ],
        "from the application restart, the emulator's rung planned as a plan drill's is"
    );
    assert!(
        rung_outcomes(&all_events(&replay.host)).contains(&(
            RecoveryRung::EmulatorRestart,
            RecoveryRungOutcome::Skipped,
            Some(
                HostCode::RecoveryRungSkippedEmulatorRestartedInStretch
                    .as_str()
                    .to_owned()
            )
        )),
        "{:?}",
        rung_outcomes(&all_events(&replay.host))
    );
    assert_eq!(
        emulator_actions(&replay.state),
        [Stop, Start],
        "not restarted again in the stretch"
    );

    from.store(
        replay.state.input_count.load(Ordering::Acquire),
        Ordering::Release,
    );
    away.store(false, Ordering::Release);
    let receipt = manual_run(&replay.host, replay.root.path());
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
            .entry_climbs_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's stretch"),
        None,
        "a run let in ends the stretch"
    );
    away.store(true, Ordering::Release);
    let third = admit_scheduled_at(
        &replay.host,
        replay.manual_clock(),
        &replay.request,
        NEXT_DISPATCH_UNIX_MS + 2 * 3_600_000,
        302,
    );
    replay.fail_at_entry_of(&third);
    assert_settled_at_entry(&replay.execution_of(third.decision_id()));
    wait_until("the new stretch's first climb", || {
        started_plans(&replay.host).len() == 3
    });
    let plans = started_plans(&replay.host);
    let (trigger, plan) = &plans[2];
    assert_eq!(trigger.run_id, Some(third.run_id()));
    assert_eq!(
        plan.first(),
        Some(&planned(RecoveryRung::ReturnHome, None)),
        "as configured"
    );
    assert_eq!(
        plan.last(),
        Some(&planned(RecoveryRung::EmulatorRestart, None)),
        "a new stretch may restart the emulator once again"
    );
    wait_for_ladders_finished(&replay.host, 3);
    assert_eq!(emulator_actions(&replay.state), [Stop, Start, Stop, Start]);
    replay.close();
}

/// M5: [`store_sources`] with the ordinary task's `on_failure.escalation_threshold` 1.
fn store_sources_threshold_one() -> CatalogSources {
    let mut sources = store_sources();
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("the task fixture");
    tasks["tasks"][0]["on_failure"]["escalation_threshold"] = json!(1);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("the task bytes");
    sources
}

/// M5 (ruling 5; policy level, gate_upd1's store update task, `scheduling.unavailable_probe`'s first
/// interval two hours, the ordinary task's `escalation_threshold` 1): the ordinary task fails at its
/// entry and waits two hours. The game asks for an update; the store update task passes the block,
/// is dispatched and succeeds: every pair of the instance starts from nothing, so the ordinary
/// task's wait is gone, and the success is no run let in for the pairs. Its run having ended the
/// block, the ordinary task is dispatched twenty minutes later, well inside its old wait, and fails
/// at its entry again: its first in a row, none against it, not set aside, no person told.
#[test]
fn entry_m5_a_store_update_success_between_two_failures_at_the_entry_clears_the_wait_and_counts_toward_nothing()
 {
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
    let manifest = paced(json!({})).manifest.expect("the configuration");
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_procedure_manifest(ProcedureManifest::new(procedures).expect("manifest"))
            .with_config_manifest(manifest),
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
    host.activate_policy_catalog(&store_sources_threshold_one())
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
    let grant = |evaluation: &actingcommand_policy::PolicyEvaluation, task_id: &str| {
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == task_id)
            .cloned()
            .unwrap_or_else(|| panic!("{task_id} is dispatched: {evaluation:#?}"));
        let chain = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .cloned()
            .expect("its reason chain");
        let PolicyDispatchAdmission::Granted { context } = host
            .admit_policy_dispatch(&intent, &chain, &policy_context(&host, &intent))
            .expect("the admission")
        else {
            panic!("{task_id} is granted");
        };
        (intent, context)
    };

    let first = evaluate_at(POLICY_NOW_UNIX_MS + 1_000, 1);
    let ordinary_intent = first
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == TASK)
        .cloned()
        .unwrap_or_else(|| panic!("the ordinary task: {first:#?}"));
    record_policy_approval(&host, &ordinary_intent);
    let (intent, context) = grant(&first, TASK);
    let settled = settle_at_entry_after(&host, &clock, &intent, ENTRY_RUN_MS);
    assert_settled_at_entry(&failure_of(&settled));
    host.release_policy_dispatch_lease_for_test(&context)
        .expect("end the dispatch's lease");
    assert_eq!(
        pair(&host, TASK),
        Some((1, 0, settled.observed_at_unix_ms + PACED_MS, false)),
        "it waits two hours"
    );

    advance_manual_clock_to(&clock, POLICY_NOW_UNIX_MS + MINUTE_MS);
    host.record_application_update_for_test(POLICY_INSTANCE_ALIAS, true)
        .expect("the game asks for an update");
    let store_at = POLICY_NOW_UNIX_MS + ACTIVITY_INTERVAL_PAST_MS;
    let update = evaluate_at(store_at, 2);
    let (store_intent, store_context) = grant(&update, STORE_TASK);
    succeed(&host, &clock, &store_intent);
    host.release_policy_dispatch_lease_for_test(&store_context)
        .expect("end the dispatch's lease");
    assert_eq!(
        pair(&host, TASK),
        None,
        "every pair of the instance starts again"
    );
    host.record_application_update_for_test(POLICY_INSTANCE_ALIAS, false)
        .expect("the game is up to date");
    // The store update task's run succeeded on the instance, which ends its block
    // (`append_contained_task_terminal`); the hook settles no run.
    recover(&host);

    let again = evaluate_at(store_at + ACTIVITY_INTERVAL_PAST_MS, 3);
    assert!(
        settled.observed_at_unix_ms + PACED_MS > store_at + ACTIVITY_INTERVAL_PAST_MS,
        "inside the old wait"
    );
    let (intent, context) = grant(&again, TASK);
    let settled = settle_at_entry_after(&host, &clock, &intent, ENTRY_RUN_MS);
    assert_settled_at_entry(&failure_of(&settled));
    host.release_policy_dispatch_lease_for_test(&context)
        .expect("end the dispatch's lease");
    let (in_a_row, against, _, set_aside) = pair(&host, TASK).expect("the pair");
    assert_eq!((in_a_row, against, set_aside), (1, 0, false));
    assert!(set_aside_reports(&host).is_empty(), "no one told");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// M6 (the lease end's in-memory flags, `RunEntry`'s `checked` and `started`; the live path on a
/// fake device, manual clock): `main` declares no prerequisite, so its page graph's own entry check
/// is its gate, and the run's bound return-home package leaves `away` for `menu`. The game is on
/// `shop`. A first run fails at its entry while the cool-down window holds its recovery, and a
/// recovery ends its block with nothing let in. The task's next run checks its entry, cannot reach
/// it and fails before its main package started: its lease end judges it a failure at the entry
/// (`checked`), so its block's climb starts at once from the application restart. A recovery ends
/// that block. The game is on `away`: the next run checks its entry, its return-home package brings
/// it to `menu`, its main package starts and fails on `shop`: no failure at the entry, settled
/// with no condition, and its lease end (`started`) admits its ladder at once, as configured.
#[test]
fn entry_m6_on_a_page_graph_without_a_chain_the_lease_end_tells_a_failure_before_its_main_package_from_one_after()
 {
    let replay = replay_set_up(
        &Scene {
            return_home: &route(RETURN_HOME, "away", "menu", BACK_STEPS, None),
            lead: &lead(),
            registry_return_home: None,
            screens: vec![SHOP],
        },
        true,
        Setup {
            unchained: true,
            ..Setup::default()
        },
    );
    let started = Arc::new(AtomicBool::new(false));
    let base = Arc::new(AtomicUsize::new(0));
    let (seen_started, seen_base) = (Arc::clone(&started), Arc::clone(&base));
    *replay.state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, inputs: usize| {
            let screen = if seen_started.load(Ordering::Acquire) {
                match inputs.saturating_sub(seen_base.load(Ordering::Acquire)) {
                    0 => AWAY,
                    1 => MENU,
                    _ => SHOP,
                }
            } else {
                SHOP
            };
            vec![(0, screen)]
        }));
    replay.manual_clock().advance(1);
    replay
        .host
        .open_recovery_cooldown_for_test(
            POLICY_INSTANCE_ALIAS,
            replay.now_unix_ms() + COOLDOWN_LEFT_MS,
        )
        .expect("the ladder's cool-down window");
    replay.fail_at_entry();
    assert!(
        crate::policy_control::entry_blocked(&replay.execution()),
        "a failure at the entry: {:?}",
        replay.execution()
    );
    recover(&replay.host);
    assert!(replay.ladders().is_empty(), "the cool-down held its ladder");

    let second = admit_scheduled_at(
        &replay.host,
        replay.manual_clock(),
        &replay.request,
        NEXT_DISPATCH_UNIX_MS,
        301,
    );
    replay.fail_at_entry_of(&second);
    assert!(crate::policy_control::entry_blocked(
        &replay.execution_of(second.decision_id())
    ));
    wait_until("the block's climb", || {
        replay.tick();
        !started_plans(&replay.host).is_empty()
    });
    let plans = started_plans(&replay.host);
    let [(trigger, plan)] = plans.as_slice() else {
        panic!("one climb: {plans:#?}");
    };
    assert_eq!(trigger.run_id, Some(second.run_id()));
    assert_eq!(
        *plan,
        from_the_application_restart(),
        "judged at the entry at its lease end"
    );
    wait_for_ladders_finished(&replay.host, 1);
    recover(&replay.host);

    base.store(
        replay.state.input_count.load(Ordering::Acquire),
        Ordering::Release,
    );
    started.store(true, Ordering::Release);
    let third = admit_scheduled_at(
        &replay.host,
        replay.manual_clock(),
        &replay.request,
        NEXT_DISPATCH_UNIX_MS + 2 * 3_600_000,
        302,
    );
    replay.fail_at_entry_of(&third);
    let failure = replay.execution_of(third.decision_id());
    assert!(
        !crate::policy_control::entry_blocked(&failure),
        "after its main package started: {failure:?}"
    );
    assert_eq!(failure.condition, None, "{failure:?}");
    wait_until("the run's own climb", || {
        started_plans(&replay.host).len() == 2
    });
    let plans = started_plans(&replay.host);
    let (trigger, plan) = &plans[1];
    assert_eq!(trigger.run_id, Some(third.run_id()));
    assert_eq!(
        plan.first(),
        Some(&planned(RecoveryRung::ReturnHome, None)),
        "as configured, at once from its lease end"
    );
    wait_for_ladders_finished(&replay.host, 2);
    replay.close();
}
