// SPDX-License-Identifier: AGPL-3.0-only

//! #503 PACE (0.12.100; register §1.545; `contracts/policy-suspension.md`, "A pair whose runs keep
//! failing"; `contracts/scheduling-blackout.md`, "Order of the deferrals", position 4b). A failure
//! that does not accumulate (its identity's `F` unique) keeps its count of the same error at 1, so
//! a pair whose runs keep failing so was rerun at once after every failure, bounded only by the
//! counts of its rule and its activity. From its second failure in a row in a window on, the pair
//! now waits `scheduling.unavailable_probe`'s interval for one less than its count since that
//! failure's settlement (one minute, two, four, … thirty with the shipped values), never past the
//! window's end, and each failure that sets a wait writes one record of it under the instance (the
//! owner's ruling on this slice: the situation has its code, which says the result and the
//! operation that follows); once its failures that came although another scheduled run succeeded
//! on the instance reach its rule's `escalation_threshold`, it is set aside for the window and a
//! person is told once, in place of that record.
//!
//! The host-level tests run on `gate_372k2`'s fixture instance (the activity window 06:00 to
//! 23:30 UTC, its interval 1 ms, the no-progress guard off, a manual Runtime clock) with tasks due
//! at every evaluation (an interval clock of 1 ms, no cooldown, twenty runs a day) under the
//! catalog fixture's `on_failure` (`continue`, a backoff of 30 s) with the `retry_limit` and
//! `escalation_threshold` each test names. A run is evaluated, admitted, settled at once as the
//! test says (a failure is `contained_task_page_unknown` with a unique `F`, as the live settlement
//! writes a failure that does not accumulate) and its lease ended. The memory, the gate and a store
//! update task's runs are also tested on their own (`crate::failure_pacing`). No clock is read in
//! this file.

use super::gate_372k2::{SlotTask, Slots, at, slot_sources};
use super::gate_386_sr2b1::failure_of;
use super::instance_workers::all_events;
use super::*;
use crate::codes::HostCode;
use crate::failure_identity::{FailureIdentity, IdentityFrame, IdentityPackage};
use crate::failure_pacing::{
    FailurePacingGate, FailurePacingGates, FailurePairs, PacedRecord, PacedSettlement, PacingRule,
};
use crate::idle_action::IdlePlan;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{PolicyConditionSettlement, PolicyFailureRecord, RuntimePayload};
use actingcommand_policy::{
    CandidateEligibility, PolicyEvaluation, SchedulingDecisionState, TaskDecision,
};
use actingcommand_runtime_config::UnavailableProbeConfig;
use serde_json::{Value, json};

const TASK: &str = "fixture.paced";
/// A second task of the fixture instance, ranked below `TASK`.
const OTHER: &str = "fixture.healthy";
/// A store update package's task, in the tests of the memory alone.
const STORE: &str = "fixture.store_update";
const ALIAS: &str = POLICY_INSTANCE_ALIAS;
/// The failure every failed run settles with.
const FAILURE: &str = "contained_task_page_unknown";
const IMMEDIATE: &str = "failure_retry_immediate";
const OWED: &str = "policy_rerun_owed";
const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
/// The catalog fixture's `on_failure.retry_backoff_ms`: a first failure's backoff.
const BACKOFF_MS: u64 = 30_000;
/// The pair's wait after its second, third, … eighth failure in a row: the shipped
/// `scheduling.unavailable_probe` intervals for one to seven (`first_interval_ms` 60 000, `factor`
/// 2, `max_interval_ms` 1 800 000).
const WAITS_MS: [u64; 7] = [
    MINUTE_MS,
    2 * MINUTE_MS,
    4 * MINUTE_MS,
    8 * MINUTE_MS,
    16 * MINUTE_MS,
    30 * MINUTE_MS,
    30 * MINUTE_MS,
];
const WAITING: HostCode = HostCode::InstanceTaskFailureWaiting;
const SET_ASIDE: HostCode = HostCode::InstanceTaskFailureSetAside;
const REPORTED: HostCode = HostCode::InstanceTaskFailureSetAsideReported;

// ---- The fixture.

/// The task `id`, ranked `priority`, due at every evaluation: an interval clock of 1 ms, twenty
/// runs a day.
fn due(id: &'static str, priority: u32) -> SlotTask {
    SlotTask {
        id,
        priority,
        trigger: json!({
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": 1,
                "anchor_ms": 0
            }
        }),
        daily_limit: 20,
    }
}

/// The catalog (version `version`) of `tasks` on the fixture instance, each with `on_failure`'s
/// `retry_limit` and `escalation_threshold` as given.
fn sources_at(
    version: u64,
    tasks: &[SlotTask],
    retry_limit: u16,
    escalation_threshold: u16,
) -> CatalogSources {
    let mut catalog = slot_sources(version, tasks, None);
    let mut document: Value =
        serde_json::from_slice(&catalog.tasks.bytes).expect("the task fixture");
    for task in document["tasks"].as_array_mut().expect("the tasks") {
        task["on_failure"]["retry_limit"] = json!(retry_limit);
        task["on_failure"]["escalation_threshold"] = json!(escalation_threshold);
    }
    catalog.tasks.bytes = serde_json::to_vec_pretty(&document).expect("the task bytes");
    catalog
}

/// [`sources_at`] as the catalog version 1.
fn sources(tasks: &[SlotTask], retry_limit: u16, escalation_threshold: u16) -> CatalogSources {
    sources_at(1, tasks, retry_limit, escalation_threshold)
}

/// What a run of `intent` that keeps failing settles with: `contained_task_page_unknown` in a step
/// of the main package, a failure that does not accumulate, its identity's `F` unique to the run.
fn failure(intent: &DispatchIntent) -> PolicyExecutionInput {
    PolicyExecutionInput::Failed {
        outcome: Outcome::new(SharedCode::ContainedTaskPageUnknown),
        failure_identity: Some(
            FailureIdentity {
                base: FAILURE.to_owned(),
                key: "0".repeat(12),
                main: IdentityPackage {
                    id_hash: "0a1b2c3d".to_owned(),
                    hash: "x".to_owned(),
                },
                layers: Vec::new(),
                frame: IdentityFrame::unique(&intent.decision_id),
            }
            .encode(),
        ),
        class: PolicyFailureClass::Recoverable,
    }
}

/// One run admitted and settled: its intent, its execution record and the end of the activity
/// window its admission recorded.
struct Ran {
    intent: DispatchIntent,
    settled: PolicyExecutionEventData,
    window_end: u64,
}

/// The fixture host, and the catalogues approved on it.
struct Pace {
    slots: Slots,
    approved: BTreeSet<String>,
}

impl Pace {
    /// A host at `unix_ms` with `catalog` active.
    fn start(catalog: &CatalogSources, unix_ms: u64) -> Self {
        Self {
            slots: Slots::start_without_no_progress_guard(catalog, unix_ms),
            approved: BTreeSet::new(),
        }
    }

    fn host(&self) -> &RuntimeHost {
        &self.slots.host
    }

    /// One full evaluation at `unix_ms`, the Runtime clock moved there.
    fn evaluate(&mut self, unix_ms: u64) -> PolicyEvaluation {
        self.slots
            .evaluate(unix_ms)
            .evaluation
            .unwrap_or_else(|| panic!("an evaluated cycle at {unix_ms}"))
    }

    /// Evaluates at `unix_ms`, admits the dispatch the cycle selected for `task_id` (its catalogue
    /// approved first) and settles its run at once as `settle` gives it; its lease ends.
    fn run(
        &mut self,
        unix_ms: u64,
        task_id: &str,
        settle: impl FnOnce(&DispatchIntent) -> PolicyExecutionInput,
    ) -> Ran {
        let evaluation = self.evaluate(unix_ms);
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == task_id)
            .unwrap_or_else(|| panic!("{task_id} is selected at {unix_ms}: {evaluation:#?}"))
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("the dispatch's reason chain")
            .clone();
        if self.approved.insert(intent.catalog_hash.clone()) {
            record_policy_approval(&self.slots.host, &intent);
        }
        let PolicyDispatchAdmission::Granted { context } = self
            .slots
            .host
            .admit_policy_dispatch(
                &intent,
                &reasons,
                &policy_context(&self.slots.host, &intent),
            )
            .unwrap_or_else(|error| panic!("admit {task_id} at {unix_ms}: {error}"))
        else {
            panic!("{task_id} is granted at {unix_ms}");
        };
        let window_end = context
            .admission()
            .activity
            .window_until_unix_ms
            .expect("the admission records its window's end");
        let settled = self
            .slots
            .host
            .record_policy_dispatch_outcome(&intent.decision_id, &settle(&intent))
            .expect("the settlement");
        self.slots
            .host
            .release_policy_dispatch_lease_for_test(&context)
            .expect("end the dispatch's lease");
        Ran {
            intent,
            settled,
            window_end,
        }
    }

    /// [`Pace::run`] of a run that fails ([`failure`]).
    fn fail(&mut self, unix_ms: u64, task_id: &str) -> Ran {
        self.run(unix_ms, task_id, failure)
    }

    /// [`Pace::run`] of a run that succeeds.
    fn succeed(&mut self, unix_ms: u64, task_id: &str) -> Ran {
        self.run(unix_ms, task_id, |_| PolicyExecutionInput::Succeeded)
    }

    /// The pair of `task_id` on the fixture instance: its failures in a row, those against it,
    /// the time before which it is not dispatched and whether it is set aside.
    fn pair(&self, task_id: &str) -> Option<(u32, u32, u64, bool)> {
        self.host()
            .failure_pair_for_test(ALIAS, task_id)
            .expect("the pair")
    }

    /// A clean stop and a new start on the same state root at `unix_ms`.
    fn restart(self, unix_ms: u64) -> Self {
        let Self { slots, approved } = self;
        Self {
            slots: slots.restart(unix_ms),
            approved,
        }
    }

    fn close(self) {
        assert!(self.host().fatal_error().expect("runtime health").is_none());
        self.slots.close();
    }
}

/// The decision of `task_id` on the fixture instance in `evaluation`.
fn decision_of<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(ALIAS)
        })
        .unwrap_or_else(|| panic!("{task_id}'s decision: {evaluation:#?}"))
}

/// Whether `evaluation` dispatched `task_id` on the fixture instance.
fn dispatched(evaluation: &PolicyEvaluation, task_id: &str) -> bool {
    evaluation
        .dispatch_intents
        .iter()
        .any(|intent| intent.task_id == task_id && intent.instance_id == ALIAS)
}

/// Whether a reason of `task_id` on the fixture instance in `evaluation` has the code `code`.
fn has_reason(evaluation: &PolicyEvaluation, task_id: &str, code: &str) -> bool {
    evaluation
        .decisions
        .iter()
        .filter(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(ALIAS)
        })
        .flat_map(|decision| decision.reasons.iter())
        .any(|reason| reason.outcome.code().as_str() == code)
}

/// Whether a reason of `task_id` in `evaluation` names this slice's wait or its being set aside.
fn paced(evaluation: &PolicyEvaluation, task_id: &str) -> bool {
    has_reason(evaluation, task_id, WAITING.as_str())
        || has_reason(evaluation, task_id, SET_ASIDE.as_str())
}

/// `task_id` is not dispatched and its decision is deferred with `code` as its last reason: that
/// reason's outcome and the decision's wake.
fn deferred_by(
    evaluation: &PolicyEvaluation,
    task_id: &str,
    code: HostCode,
) -> (Outcome, Option<u64>) {
    assert!(
        !dispatched(evaluation, task_id),
        "{task_id} is not dispatched: {evaluation:#?}"
    );
    let decision = decision_of(evaluation, task_id);
    assert_eq!(
        decision.state,
        SchedulingDecisionState::Deferred,
        "{decision:#?}"
    );
    let last = decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason: {decision:#?}"));
    assert!(
        last.outcome.is(code),
        "deferred by {}: {decision:#?}",
        code.as_str()
    );
    (last.outcome.clone(), decision.next_wake_unix_ms)
}

/// The wait's outcome `outcome` names `latest`, the pair's `count`-th failure in a row, and its
/// end `until`.
fn assert_waits(outcome: &Outcome, latest: &Ran, count: u32, until: u64) {
    assert_eq!(outcome.value("instance_id"), Some(&json!(ALIAS)));
    assert_eq!(
        outcome.value("task_id"),
        Some(&json!(latest.intent.task_id.as_str()))
    );
    assert_eq!(
        outcome.value("decision_id"),
        Some(&json!(latest.intent.decision_id.as_str()))
    );
    assert_eq!(outcome.value("failure_code"), Some(&json!(FAILURE)));
    assert_eq!(
        outcome.value("observed_at_unix_ms"),
        Some(&json!(latest.settled.observed_at_unix_ms))
    );
    assert_eq!(outcome.value("count"), Some(&json!(count)));
    assert_eq!(outcome.value("next_eligible_unix_ms"), Some(&json!(until)));
}

/// The outcomes of `host`'s `runtime.failed` records whose code is `code`, in ledger order.
fn failed_records(host: &RuntimeHost, code: HostCode) -> Vec<Outcome> {
    all_events(host)
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record)) if record.outcome().is(code) => {
                Some(record.outcome().clone())
            }
            _ => None,
        })
        .collect()
}

/// The `instance_task_failure_set_aside_reported` warnings of `host`.
fn set_aside_reports(host: &RuntimeHost) -> Vec<Outcome> {
    failed_records(host, REPORTED)
}

/// The records of `host` that say a failure set a wait (`instance_task_failure_waiting`).
fn wait_records(host: &RuntimeHost) -> Vec<Outcome> {
    failed_records(host, WAITING)
}

/// The records an evaluation or a settlement could write for a pair: the policy's own and the
/// `runtime.failed` records.
fn policy_records(host: &RuntimeHost) -> usize {
    all_events(host)
        .iter()
        .filter(|event| {
            matches!(
                event.event_type(),
                EventType::PolicyDispatchIntent
                    | EventType::PolicyDispatchAdmitted
                    | EventType::PolicyDispatchRejected
                    | EventType::PolicyDispatchCompleted
                    | EventType::PolicyExecutionRecorded
                    | EventType::PolicyPlanningSignalObserved
                    | EventType::PolicyControlStateRecorded
                    | EventType::PolicyActivityStateRecorded
                    | EventType::PolicyNotRunChanged
                    | EventType::RuntimeFailed
            )
        })
        .count()
}

/// The fixture instance's not-running row as `ScheduleExplanation` answers it: its `why`.
fn not_running(host: &RuntimeHost) -> Outcome {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::ScheduleExplanation {
        instance: Some(ALIAS.to_owned()),
        candidates: false,
    });
    let receipt = client.send(&request);
    let Some(RuntimeResult::ScheduleExplanation { explanation }) = receipt.result() else {
        panic!("a schedule explanation: {receipt:?}");
    };
    explanation
        .instances
        .first()
        .unwrap_or_else(|| panic!("the instance's entry: {explanation:?}"))
        .not_running
        .why
        .clone()
}

/// The fixture instance's idle plan, as the idle check reads it.
fn idle_plan(host: &RuntimeHost) -> IdlePlan {
    host.idle_plan_and_runs_counted_for_test(ALIAS)
        .expect("the idle plan")
        .0
        .expect("an evaluated plan")
}

/// The members of a failed run's record that its time and its run do not set: all but its
/// failure identity (unique to the run), its retry time and its performance context.
fn timeless(record: &PolicyFailureRecord) -> Value {
    let mut value = serde_json::to_value(record).expect("the record as JSON");
    let members = value.as_object_mut().expect("the record's members");
    for member in ["failure_identity", "retry_at_unix_ms", "perf_context"] {
        members.remove(member);
    }
    value
}

/// TASK (an `escalation_threshold` of 2) fails three times in a row, OTHER succeeding before its
/// second and its third: the second counts once against it, the third twice, which sets it aside
/// for its window. The third failure.
fn set_aside_by_evidence(pace: &mut Pace) -> Ran {
    let first = pace.fail(at(1, 7, 0), TASK);
    let first_at = first.settled.observed_at_unix_ms;
    pace.succeed(first_at + 1_000, OTHER);
    let second = pace.fail(first_at + BACKOFF_MS, TASK);
    let second_at = second.settled.observed_at_unix_ms;
    let until = second_at + MINUTE_MS;
    assert_eq!(
        pace.pair(TASK),
        Some((2, 1, until, false)),
        "one against it, below the threshold"
    );
    assert_eq!(wait_records(pace.host()).len(), 1, "the second set a wait");
    pace.succeed(second_at + 1_000, OTHER);
    let third = pace.fail(until, TASK);
    assert_eq!(
        pace.pair(TASK),
        Some((
            3,
            2,
            third.settled.observed_at_unix_ms + 2 * MINUTE_MS,
            true
        )),
        "set aside at the threshold"
    );
    assert_eq!(
        wait_records(pace.host()).len(),
        1,
        "the third says it is set aside in place of a wait"
    );
    third
}

/// The shipped `scheduling.unavailable_probe`.
fn shipped() -> UnavailableProbeConfig {
    UnavailableProbeConfig {
        first_interval_ms: MINUTE_MS,
        factor: 2,
        max_interval_ms: 30 * MINUTE_MS,
        attention_after_ms: HOUR_MS,
    }
}

/// A failure that counts, of the dispatch `decision_id` settled at `unix_ms`, `exempt` when its
/// record is `performance_tax_exempt`.
fn failed(decision_id: &str, unix_ms: u64, exempt: bool) -> PacedSettlement {
    PacedSettlement::Failed {
        decision_id: decision_id.to_owned(),
        failure_code: actingcommand_contract::scheduled_failure_code_str(&Outcome::new(
            SharedCode::ContainedTaskPageUnknown,
        )),
        observed_at_unix_ms: unix_ms,
        exempt,
    }
}

/// A rule whose `escalation_threshold` is 2, in the window ending at `window_end` (`0`: unknown).
const fn rule(window_end: u64, store_task: bool) -> PacingRule {
    PacingRule {
        window_end,
        threshold: 2,
        store_task,
    }
}

// ---- The tests.

/// Test 1: the first failure in a row adds nothing. With `retry_limit` 1 its record schedules a
/// retry after the backoff, and once the backoff has passed the pair is offered as an immediate
/// rerun and dispatched, as on main; with `retry_limit` 0 (`continue`, no cooldown) its record
/// continues and the next evaluation dispatches it by its trigger, as on main. Neither names this
/// slice's codes.
#[test]
fn pace_p1_the_first_failure_is_rerun_as_on_main_with_a_retry_and_without_one() {
    let mut pace = Pace::start(&sources(&[due(TASK, 100)], 1, 3), at(1, 6, 0));
    let first = pace.fail(at(1, 7, 0), TASK);
    let record = failure_of(&first.settled);
    assert_eq!(record.disposition, PolicyFailureDisposition::RetryScheduled);
    assert_eq!(
        record.retry_at_unix_ms,
        Some(first.settled.observed_at_unix_ms + BACKOFF_MS)
    );
    assert_eq!(
        pace.pair(TASK),
        Some((1, 0, 0, false)),
        "the first in a row waits for nothing"
    );
    assert!(
        wait_records(pace.host()).is_empty(),
        "the first failure writes no record of a wait"
    );
    let retry = pace.evaluate(first.settled.observed_at_unix_ms + BACKOFF_MS);
    assert!(dispatched(&retry, TASK), "{retry:#?}");
    assert!(has_reason(&retry, TASK, IMMEDIATE), "{retry:#?}");
    assert!(!paced(&retry, TASK), "{retry:#?}");
    pace.close();

    let mut pace = Pace::start(&sources(&[due(TASK, 100)], 0, 3), at(1, 6, 0));
    let first = pace.fail(at(1, 7, 0), TASK);
    let record = failure_of(&first.settled);
    assert_eq!(record.disposition, PolicyFailureDisposition::Continue);
    assert_eq!(record.retry_at_unix_ms, None);
    assert_eq!(pace.pair(TASK), Some((1, 0, 0, false)));
    assert!(wait_records(pace.host()).is_empty());
    let next = pace.evaluate(first.settled.observed_at_unix_ms + 1_000);
    assert!(dispatched(&next, TASK), "{next:#?}");
    assert!(!has_reason(&next, TASK, IMMEDIATE), "{next:#?}");
    assert!(!paced(&next, TASK), "{next:#?}");
    pace.close();
}

/// Test 2: the second to the eighth failure in a row (`retry_limit` 1, each dispatched as an
/// immediate rerun at the end of its wait): after the n-th the pair waits
/// `instance_task_failure_waiting` (the instance, the task, the latest dispatch, its failure and
/// settlement, `count` n, `next_eligible_unix_ms`) one, two, four, eight, sixteen, thirty and
/// thirty minutes from that settlement, its decision waking then, and still waits a millisecond
/// before. Each of those failures writes, at its settlement, one `instance_task_failure_waiting`
/// record under the instance with the same values (the owner's ruling on this slice), and no
/// evaluation writes one. A wait that would pass the window's end (seeded) ends there, and at the
/// window's end nothing of this slice holds it.
#[test]
fn pace_p2_from_its_second_failure_in_a_row_the_pair_waits_one_two_four_minutes_to_the_longest_and_never_past_its_window()
 {
    let mut pace = Pace::start(&sources(&[due(TASK, 100)], 1, 3), at(1, 6, 0));
    let first = pace.fail(at(1, 7, 0), TASK);
    let window_end = first.window_end;
    let mut due_at = first.settled.observed_at_unix_ms + BACKOFF_MS;
    for (index, wait_ms) in WAITS_MS.iter().enumerate() {
        let count = u32::try_from(index + 2).expect("a count");
        let ran = pace.fail(due_at, TASK);
        let settled_at = ran.settled.observed_at_unix_ms;
        let until = settled_at + *wait_ms;
        assert_eq!(pace.pair(TASK), Some((count, 0, until, false)), "{count}");
        // The owner's ruling on this slice: the failure that sets the wait writes one record of
        // it, with the wait's code and values, at its settlement.
        let records = wait_records(pace.host());
        assert_eq!(records.len(), index + 1, "{count}: one record per wait");
        assert_waits(&records[index], &ran, count, until);
        let held = pace.evaluate(settled_at + 1_000);
        let (waiting, wake) = deferred_by(&held, TASK, WAITING);
        assert_waits(&waiting, &ran, count, until);
        assert_eq!(wake, Some(until), "{count}: it wakes at the wait's end");
        let (_, wake) = deferred_by(&pace.evaluate(until - 1), TASK, WAITING);
        assert_eq!(wake, Some(until), "{count}: a millisecond before its end");
        assert_eq!(
            wait_records(pace.host()).len(),
            index + 1,
            "{count}: no evaluation writes one"
        );
        due_at = until;
    }
    pace.host()
        .seed_failure_pair_for_test(
            ALIAS,
            TASK,
            (9, due_at, window_end + HOUR_MS, window_end, false),
        )
        .expect("a wait past the window's end");
    let (cut, wake) = deferred_by(&pace.evaluate(at(1, 23, 0)), TASK, WAITING);
    assert_eq!(cut.value("next_eligible_unix_ms"), Some(&json!(window_end)));
    assert_eq!(wake, Some(window_end), "cut at the window's end");
    let ended = pace.evaluate(window_end);
    assert!(
        !paced(&ended, TASK),
        "the window's end ends the wait: {ended:#?}"
    );
    pace.close();
}

/// Test 3 (a): two tasks, `retry_limit` 1. After `TASK`'s second failure in a row, its backoff
/// passed, it is offered both by its trigger and as an immediate rerun (`failure_retry_immediate`)
/// and waits `instance_task_failure_waiting` until its wait's end, while `OTHER`, the instance's
/// other task, is dispatched with no reason of this slice. At the wait's end `TASK` runs and is
/// settled by a configured condition (the test's seam), which leaves its pair as it was and owes a
/// rerun; with its pair waiting again (seeded), the owed rerun (`policy_rerun_owed`) waits too.
#[test]
fn pace_p3_a_every_way_to_be_a_candidate_waits_and_the_instances_other_task_runs() {
    let mut pace = Pace::start(
        &sources(&[due(TASK, 100), due(OTHER, 50)], 1, 3),
        at(1, 6, 0),
    );
    let first = pace.fail(at(1, 7, 0), TASK);
    let second = pace.fail(first.settled.observed_at_unix_ms + BACKOFF_MS, TASK);
    let settled_at = second.settled.observed_at_unix_ms;
    let until = settled_at + MINUTE_MS;
    let held = pace.evaluate(settled_at + BACKOFF_MS + 1_000);
    let (_, wake) = deferred_by(&held, TASK, WAITING);
    assert_eq!(wake, Some(until));
    assert!(
        has_reason(&held, TASK, IMMEDIATE),
        "offered as an immediate rerun: {held:#?}"
    );
    assert!(
        dispatched(&held, OTHER),
        "the instance's other task is not held: {held:#?}"
    );
    assert!(!paced(&held, OTHER), "{held:#?}");

    pace.host()
        .settle_next_policy_run_by_condition_for_test(PolicyConditionSettlement {
            refunded: true,
            count: 1,
            expires_at_unix_ms: second.window_end,
        })
        .expect("arm the condition");
    let ended = pace.fail(until, TASK);
    assert!(
        failure_of(&ended.settled).condition.is_some(),
        "settled by a condition"
    );
    assert_eq!(
        pace.pair(TASK),
        Some((2, 0, until, false)),
        "a condition leaves the pair as it was"
    );
    let ended_at = ended.settled.observed_at_unix_ms;
    let again = ended_at + 2 * MINUTE_MS;
    pace.host()
        .seed_failure_pair_for_test(ALIAS, TASK, (3, ended_at, again, second.window_end, false))
        .expect("the pair waits again");
    let owed = pace.evaluate(ended_at + 1_000);
    let (_, wake) = deferred_by(&owed, TASK, WAITING);
    assert_eq!(wake, Some(again));
    assert!(has_reason(&owed, TASK, OWED), "an owed rerun: {owed:#?}");
    pace.close();
}

/// Test 3 (b), the gate on its own: a waiting pair holds a fact refresh of it, which names
/// `instance_task_failure_waiting` as its gate and wakes at the wait's end, and any other candidate
/// of it; it holds neither the instance's other task nor the same task on another instance. A wait
/// past the window's end ends there, and at the window's end nothing holds. A pair set aside holds
/// its candidates until the window's end (`instance_task_failure_set_aside`, `count`,
/// `limit_count`, `window_end_unix_ms`), a fact refresh naming that code; in a window the policy
/// does not know it only waits.
#[test]
fn pace_p3_b_the_gate_holds_its_pair_alone_and_names_itself_to_a_fact_refresh() {
    let now = at(1, 8, 0);
    let window_end = at(1, 23, 30);
    let waiting = FailurePacingGate {
        decision_id: "decision-latest".to_owned(),
        failure_code: actingcommand_contract::scheduled_failure_code_str(&Outcome::new(
            SharedCode::ContainedTaskPageUnknown,
        )),
        observed_at_unix_ms: now - 1_000,
        in_a_row: 2,
        until_unix_ms: now + MINUTE_MS,
        window_end_unix_ms: window_end,
        set_aside: None,
    };
    let gates = |gate: FailurePacingGate| {
        FailurePacingGates::new(BTreeMap::from([(
            (ALIAS.to_owned(), TASK.to_owned()),
            gate,
        )]))
    };
    let held = gates(waiting.clone());
    match held.deferral(ALIAS, TASK, now, true) {
        Some(CandidateEligibility::Refresh {
            reason,
            next_wake_unix_ms,
            gate,
            limit,
        }) => {
            assert_eq!(gate, WAITING.code());
            assert_eq!(next_wake_unix_ms, Some(now + MINUTE_MS));
            assert_eq!(limit, None);
            assert!(reason.outcome.is(WAITING), "{reason:?}");
            assert_eq!(reason.outcome.value("count"), Some(&json!(2)));
            assert_eq!(reason.outcome.value("failure_code"), Some(&json!(FAILURE)));
        }
        other => panic!("a fact refresh's deferral: {other:?}"),
    }
    match held.deferral(ALIAS, TASK, now, false) {
        Some(CandidateEligibility::Deferred {
            reason,
            next_wake_unix_ms,
        }) => {
            assert_eq!(next_wake_unix_ms, Some(now + MINUTE_MS));
            assert_eq!(
                reason.outcome.value("next_eligible_unix_ms"),
                Some(&json!(now + MINUTE_MS))
            );
        }
        other => panic!("a candidate's deferral: {other:?}"),
    }
    assert_eq!(held.deferral(ALIAS, OTHER, now, false), None);
    assert_eq!(
        held.deferral(POLICY_INSTANCE_ALIAS_B, TASK, now, false),
        None
    );
    assert_eq!(held.deferral(ALIAS, TASK, now + MINUTE_MS, false), None);

    let long = gates(FailurePacingGate {
        until_unix_ms: window_end + HOUR_MS,
        ..waiting.clone()
    });
    match long.deferral(ALIAS, TASK, now, false) {
        Some(CandidateEligibility::Deferred {
            next_wake_unix_ms, ..
        }) => assert_eq!(next_wake_unix_ms, Some(window_end)),
        other => panic!("a wait cut at the window's end: {other:?}"),
    }
    assert_eq!(long.deferral(ALIAS, TASK, window_end, false), None);

    let aside = gates(FailurePacingGate {
        set_aside: Some((2, 2)),
        ..waiting.clone()
    });
    match aside.deferral(ALIAS, TASK, now + HOUR_MS, true) {
        Some(CandidateEligibility::Refresh {
            reason,
            next_wake_unix_ms,
            gate,
            ..
        }) => {
            assert_eq!(gate, SET_ASIDE.code());
            assert_eq!(next_wake_unix_ms, Some(window_end));
            assert_eq!(reason.outcome.value("count"), Some(&json!(2)));
            assert_eq!(reason.outcome.value("limit_count"), Some(&json!(2)));
            assert_eq!(
                reason.outcome.value("window_end_unix_ms"),
                Some(&json!(window_end))
            );
        }
        other => panic!("a pair set aside: {other:?}"),
    }
    assert_eq!(aside.deferral(ALIAS, TASK, window_end, false), None);
    let unknown = gates(FailurePacingGate {
        set_aside: Some((2, 2)),
        window_end_unix_ms: 0,
        ..waiting
    });
    match unknown.deferral(ALIAS, TASK, now, false) {
        Some(CandidateEligibility::Deferred { reason, .. }) => {
            assert!(reason.outcome.is(WAITING), "it only waits: {reason:?}");
        }
        other => panic!("a pair in an unknown window: {other:?}"),
    }
}

/// Tests 4 and 12: one task, `retry_limit` 1. After its second failure in a row the pair waits a
/// minute: the decision and the evaluation (nothing else is due) wake at the wait's end, and the
/// schedule explanation's not-running row names `instance_task_failure_waiting` with its count and
/// end. Four more evaluations before the end, one after the backoff (an immediate rerun offered),
/// each wake at the same end and write no record of the policy and no `runtime.failed`; at the end
/// the task is dispatched.
#[test]
fn pace_p4_a_waiting_pair_wakes_the_cycle_at_its_end_writes_no_record_and_names_its_code_in_the_explanation()
 {
    let mut pace = Pace::start(&sources(&[due(TASK, 100)], 1, 3), at(1, 6, 0));
    let first = pace.fail(at(1, 7, 0), TASK);
    let second = pace.fail(first.settled.observed_at_unix_ms + BACKOFF_MS, TASK);
    let settled_at = second.settled.observed_at_unix_ms;
    let until = settled_at + MINUTE_MS;
    let first_held = pace.evaluate(settled_at + 1_000);
    let (_, wake) = deferred_by(&first_held, TASK, WAITING);
    assert_eq!(wake, Some(until));
    assert_eq!(
        first_held.next_wake_unix_ms,
        Some(until),
        "with nothing else due the cycle wakes at the wait's end"
    );
    let why = not_running(pace.host());
    assert!(why.is(WAITING), "{why:?}");
    assert_eq!(why.value("task_id"), Some(&json!(TASK)));
    assert_eq!(why.value("count"), Some(&json!(2)));
    assert_eq!(why.value("next_eligible_unix_ms"), Some(&json!(until)));
    let records = policy_records(pace.host());
    for offset in [10_000, BACKOFF_MS + 1_000, 45_000, MINUTE_MS - 1] {
        let held = pace.evaluate(settled_at + offset);
        let (_, wake) = deferred_by(&held, TASK, WAITING);
        assert_eq!(wake, Some(until), "{offset}");
        assert_eq!(held.next_wake_unix_ms, Some(until), "{offset}");
    }
    assert_eq!(
        policy_records(pace.host()),
        records,
        "a wait writes nothing"
    );
    let woken = pace.evaluate(until);
    assert!(
        dispatched(&woken, TASK),
        "dispatched at the wait's end: {woken:#?}"
    );
    pace.close();
}

/// Test 5: one task, `retry_limit` 1. After two failures in a row the pair's run at the wait's end
/// succeeds: the pair is gone. Its next run fails: a first again, no wait, and once its backoff has
/// passed it is offered as an immediate rerun and dispatched.
#[test]
fn pace_p5_a_success_of_the_pair_makes_its_next_failure_a_first() {
    let mut pace = Pace::start(&sources(&[due(TASK, 100)], 1, 3), at(1, 6, 0));
    let first = pace.fail(at(1, 7, 0), TASK);
    let second = pace.fail(first.settled.observed_at_unix_ms + BACKOFF_MS, TASK);
    let until = second.settled.observed_at_unix_ms + MINUTE_MS;
    let success = pace.succeed(until, TASK);
    assert_eq!(
        pace.pair(TASK),
        None,
        "a success of the pair starts it from nothing"
    );
    let third = pace.fail(success.settled.observed_at_unix_ms + 1_000, TASK);
    assert_eq!(
        pace.pair(TASK),
        Some((1, 0, 0, false)),
        "its next failure is a first"
    );
    let retry = pace.evaluate(third.settled.observed_at_unix_ms + BACKOFF_MS);
    assert!(dispatched(&retry, TASK), "{retry:#?}");
    assert!(has_reason(&retry, TASK, IMMEDIATE), "{retry:#?}");
    assert!(!paced(&retry, TASK), "{retry:#?}");
    pace.close();
}

/// Test 6 (a): two tasks, `TASK`'s `escalation_threshold` 2. Its second and third failures in a
/// row come after a success of `OTHER` each: they count once and twice against it, and the third
/// sets it aside. The second writes the record of its wait; the third, in place of one, the one
/// `instance_task_failure_set_aside_reported`, which names the instance, the task, the third run,
/// its failure and settlement, `count` 2, `limit_count` 2 and the window's end. An hour later `TASK`
/// waits `instance_task_failure_set_aside` (its count, limit and window's end) and wakes at the
/// window's end while `OTHER` is dispatched; nobody is told again.
#[test]
fn pace_p6_a_failures_with_another_success_between_them_set_the_pair_aside_and_tell_a_person_once()
{
    let mut pace = Pace::start(
        &sources(&[due(TASK, 100), due(OTHER, 50)], 1, 2),
        at(1, 6, 0),
    );
    let third = set_aside_by_evidence(&mut pace);
    let reports = set_aside_reports(pace.host());
    let [report] = reports.as_slice() else {
        panic!("one person told: {reports:#?}");
    };
    assert_eq!(report.value("instance_id"), Some(&json!(ALIAS)));
    assert_eq!(report.value("task_id"), Some(&json!(TASK)));
    assert_eq!(
        report.value("decision_id"),
        Some(&json!(third.intent.decision_id.as_str()))
    );
    assert_eq!(report.value("failure_code"), Some(&json!(FAILURE)));
    assert_eq!(
        report.value("observed_at_unix_ms"),
        Some(&json!(third.settled.observed_at_unix_ms))
    );
    assert_eq!(report.value("count"), Some(&json!(2)));
    assert_eq!(report.value("limit_count"), Some(&json!(2)));
    assert_eq!(
        report.value("window_end_unix_ms"),
        Some(&json!(third.window_end))
    );
    let held = pace.evaluate(third.settled.observed_at_unix_ms + HOUR_MS);
    let (aside, wake) = deferred_by(&held, TASK, SET_ASIDE);
    assert_eq!(aside.value("count"), Some(&json!(2)));
    assert_eq!(aside.value("limit_count"), Some(&json!(2)));
    assert_eq!(
        aside.value("window_end_unix_ms"),
        Some(&json!(third.window_end))
    );
    assert_eq!(wake, Some(third.window_end), "it wakes at the window's end");
    assert!(dispatched(&held, OTHER), "the other task runs: {held:#?}");
    assert_eq!(set_aside_reports(pace.host()).len(), 1, "told once");
    pace.close();
}

/// Test 6 (b): one task, `escalation_threshold` 2, its second to fifth failures in a row with no
/// other run between them: nothing counts against it, it is never set aside and nobody is told; it
/// waits.
#[test]
fn pace_p6_b_failures_with_no_success_between_them_never_set_the_pair_aside() {
    let mut pace = Pace::start(&sources(&[due(TASK, 100)], 1, 2), at(1, 6, 0));
    let first = pace.fail(at(1, 7, 0), TASK);
    let mut due_at = first.settled.observed_at_unix_ms + BACKOFF_MS;
    for (index, wait_ms) in WAITS_MS.iter().take(4).enumerate() {
        let count = u32::try_from(index + 2).expect("a count");
        let ran = pace.fail(due_at, TASK);
        due_at = ran.settled.observed_at_unix_ms + *wait_ms;
        assert_eq!(
            pace.pair(TASK),
            Some((count, 0, due_at, false)),
            "{count}: nothing against it"
        );
    }
    assert!(set_aside_reports(pace.host()).is_empty(), "no one told");
    let (_, wake) = deferred_by(&pace.evaluate(due_at - 1), TASK, WAITING);
    assert_eq!(wake, Some(due_at));
    pace.close();
}

/// Test 6 (c), the memory on its own (`escalation_threshold` 2): failures with a success of
/// another task before each, in a window the policy does not know, count against the pair but never
/// set it aside; the same in a known window with every failure's record `performance_tax_exempt`
/// count nothing against it. Either way the pair waits after its latest failure.
#[test]
fn pace_p6_c_an_unknown_window_never_sets_a_pair_aside_and_an_exempt_failure_never_counts_against_it()
 {
    let instance = instance_id();
    let start = at(1, 7, 0);
    for (window_end, exempt) in [(0, false), (at(1, 23, 30), true)] {
        let mut pairs = FailurePairs::default();
        let mut latest = start;
        for step in 0..4_u64 {
            latest = start + step * HOUR_MS;
            assert_eq!(
                pairs.settle(
                    instance,
                    OTHER,
                    PacedSettlement::Succeeded,
                    rule(window_end, false),
                    shipped()
                ),
                None
            );
            let told = pairs.settle(
                instance,
                TASK,
                failed(&format!("decision-{step}"), latest, exempt),
                rule(window_end, false),
                shipped(),
            );
            assert!(
                !matches!(told, Some(PacedRecord::SetAside(_))),
                "never set aside: {window_end} {exempt}"
            );
            assert_eq!(
                matches!(told, Some(PacedRecord::Waits(_))),
                step > 0,
                "a record of each wait from the second failure on: {step}"
            );
        }
        let pair = pairs.pair(instance, TASK).expect("the pair");
        let against = if exempt { 0 } else { 3 };
        assert_eq!(
            (pair.in_a_row, pair.against, pair.set_aside),
            (4, against, false),
            "{window_end} {exempt}"
        );
        assert_eq!(pair.until_unix_ms, latest + 4 * MINUTE_MS, "it still waits");
    }
}

/// Test 7: two tasks, `TASK` set aside as in 6 (a). A millisecond before its window's end it is
/// still set aside, waking at the end; at the end nothing of this slice holds it; its first failure
/// of the next day's window is a first there (no wait, nothing against it).
#[test]
fn pace_p7_the_windows_end_ends_the_set_aside_and_the_next_windows_first_failure_is_a_first() {
    let mut pace = Pace::start(
        &sources(&[due(TASK, 100), due(OTHER, 50)], 1, 2),
        at(1, 6, 0),
    );
    let third = set_aside_by_evidence(&mut pace);
    let window_end = third.window_end;
    let (_, wake) = deferred_by(&pace.evaluate(window_end - 1), TASK, SET_ASIDE);
    assert_eq!(wake, Some(window_end));
    let ended = pace.evaluate(window_end);
    assert!(!paced(&ended, TASK), "the window's end ends it: {ended:#?}");
    let next = pace.fail(at(2, 7, 0), TASK);
    assert!(next.window_end > window_end, "the next window");
    assert_eq!(
        pace.pair(TASK),
        Some((1, 0, 0, false)),
        "its first in the next window"
    );
    pace.close();
}

/// Test 8 (a): one task, `retry_limit` 1, waiting after its second failure in a row. A catalogue
/// that becomes active (version 2 of the same rules) clears the pair at once, and the task is
/// dispatched before the old wait's end with no reason of this slice. Waiting again after two more
/// failures, a restart starts with no pair, and its first evaluation names none.
#[test]
fn pace_p8_a_an_activation_and_a_restart_clear_every_pair() {
    let tasks = [due(TASK, 100)];
    let mut pace = Pace::start(&sources(&tasks, 1, 3), at(1, 6, 0));
    let first = pace.fail(at(1, 7, 0), TASK);
    let second = pace.fail(first.settled.observed_at_unix_ms + BACKOFF_MS, TASK);
    let settled_at = second.settled.observed_at_unix_ms;
    assert_eq!(pace.pair(TASK), Some((2, 0, settled_at + MINUTE_MS, false)));
    pace.host()
        .activate_policy_catalog(&sources_at(2, &tasks, 1, 3))
        .expect("activate the next catalogue");
    assert_eq!(pace.pair(TASK), None, "an activation clears every pair");
    let free = pace.evaluate(settled_at + BACKOFF_MS);
    assert!(
        dispatched(&free, TASK),
        "dispatched before the old wait's end: {free:#?}"
    );
    assert!(!paced(&free, TASK), "{free:#?}");
    let third = pace.fail(settled_at + BACKOFF_MS + 1_000, TASK);
    let fourth = pace.fail(third.settled.observed_at_unix_ms + BACKOFF_MS, TASK);
    let fourth_at = fourth.settled.observed_at_unix_ms;
    assert_eq!(pace.pair(TASK), Some((2, 0, fourth_at + MINUTE_MS, false)));
    let mut pace = pace.restart(fourth_at + 1_000);
    assert_eq!(pace.pair(TASK), None, "a fresh host has none");
    let after = pace.evaluate(fourth_at + 2_000);
    assert!(!paced(&after, TASK), "{after:#?}");
    pace.close();
}

/// The modification time `seconds` after a fixed instant (no clock is read) given to `path`.
fn touch(path: &Path, seconds: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .and_then(|file| {
            file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + seconds))
        })
        .expect("set the file's modification time");
}

/// Test 8 (b), in composer mode (`gate_490_entry_a`'s P4 with one game): after the first check
/// (it activates the first catalogue) a pair of the game's rule waits (seeded); the next check,
/// nothing changed, leaves it; the rule's control file placed again (its modification time moved)
/// frees it at the check after.
#[test]
fn pace_p8_b_a_changed_package_frees_the_pairs_at_the_workbenchs_check() {
    const PLACED: &str = "fixture.placed";
    let packs = TempDir::new().expect("standard-pack root");
    let game = packs.path().join(super::gate_rl1b::GAME_A);
    super::gate_rl1b::write_json(
        &game.join("bundle.json"),
        &super::gate_rl1b::bundle(
            super::gate_rl1b::GAME_A,
            super::gate_rl1b::SERVER_A,
            &[PLACED],
        ),
    );
    super::gate_rl1b::write_json(
        &game.join("schedule.json"),
        &super::gate_rl1b::schedule_a(&[(PLACED, PLACED, 100)]),
    );
    super::gate_442_rem1::write_package(&game.join(PLACED), PLACED, None, None);
    let control = game.join(PLACED).join("control.json");
    touch(&control, 0);
    let mut facts = policy_facts();
    facts.outcomes.clear();
    facts.instances = vec![InstanceSnapshot {
        instance_id: ALIAS.to_owned(),
        server_id: super::gate_rl1b::SERVER_A.to_owned(),
        game_id: super::gate_rl1b::GAME_A.to_owned(),
        host_id: "fixture-host-a".to_owned(),
        available: true,
        capability_operation_ids: vec![actingcommand_policy::STANDARD_PACK_OPERATION_ID.to_owned()],
        preferred_task_ids: Vec::new(),
    }];
    let state = TempDir::new().expect("state root");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&state)
            .with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), policy_resources()))
            .with_runtime_clock(clock.clone())
            .with_standard_packs_root(Some(packs.path().to_owned()))
            .with_standard_pack_schedule(true),
        Arc::new(FakeProvider::one(
            ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("start the composer-mode host");
    // One policy cycle with the monotonic clock at `checks` check intervals (the shipped
    // `standard_packs.check_interval_ms`, a minute): its check runs first.
    let check = |checks: u64| {
        clock.set_unix_ms(POLICY_NOW_UNIX_MS);
        clock.set_monotonic_ms(checks * MINUTE_MS);
        host.capacity_sampler_for_test().expect("capacity owner")().expect("capacity sample");
        host.evaluate_policy_cycle_with_test_inputs(
            &facts,
            &super::gate_rl1b::resources(&[super::gate_rl1b::POOL_A]),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::Recovery,
        )
        .unwrap_or_else(|error| panic!("check {checks}: {error}"));
    };
    let until = POLICY_NOW_UNIX_MS + HOUR_MS;
    check(0);
    host.seed_failure_pair_for_test(ALIAS, PLACED, (2, POLICY_NOW_UNIX_MS, until, 0, false))
        .expect("the pair waits");
    check(1);
    assert_eq!(
        host.failure_pair_for_test(ALIAS, PLACED).expect("the pair"),
        Some((2, 0, until, false)),
        "nothing changed since the first check, nothing freed"
    );
    touch(&control, 10);
    check(2);
    assert_eq!(
        host.failure_pair_for_test(ALIAS, PLACED).expect("the pair"),
        None,
        "the changed package frees it"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the composer-mode host");
}

/// Tests 8 (c) and 10, the memory on its own: a store update package's task's failures, however
/// many, give it no pair (no wait, never set aside). Its success clears every pair of its instance
/// and none of another, and it is no success the pairs count: after it the pair's second failure in
/// a row counts nothing against it.
#[test]
fn pace_p8_c_p10_a_store_update_tasks_runs_wait_for_nothing_and_its_success_clears_its_instances_pairs()
 {
    let instance = instance_id();
    let elsewhere = instance_id();
    let window_end = at(1, 23, 30);
    let start = at(1, 7, 0);
    let mut pairs = FailurePairs::default();
    for step in 0..4_u64 {
        assert_eq!(
            pairs.settle(
                instance,
                STORE,
                failed(&format!("store-{step}"), start + step * MINUTE_MS, false),
                rule(window_end, true),
                shipped()
            ),
            None
        );
    }
    assert_eq!(
        pairs.pair(instance, STORE),
        None,
        "a store update task has no pair"
    );
    for (on, decision_id, unix_ms, waits) in [
        (instance, "task-1", start, false),
        (instance, "task-2", start + BACKOFF_MS, true),
        (elsewhere, "elsewhere-1", start, false),
    ] {
        let told = pairs.settle(
            on,
            TASK,
            failed(decision_id, unix_ms, false),
            rule(window_end, false),
            shipped(),
        );
        assert_eq!(
            matches!(told, Some(PacedRecord::Waits(_))),
            waits,
            "{decision_id}"
        );
    }
    assert_eq!(
        pairs.settle(
            instance,
            STORE,
            PacedSettlement::Succeeded,
            rule(window_end, true),
            shipped()
        ),
        None
    );
    assert_eq!(
        pairs.pair(instance, TASK),
        None,
        "its instance's pairs start from nothing"
    );
    assert!(
        pairs.pair(elsewhere, TASK).is_some(),
        "another instance's pair stays"
    );
    for (decision_id, unix_ms, waits) in [
        ("task-3", start + HOUR_MS, false),
        ("task-4", start + HOUR_MS + BACKOFF_MS, true),
    ] {
        let told = pairs.settle(
            instance,
            TASK,
            failed(decision_id, unix_ms, false),
            rule(window_end, false),
            shipped(),
        );
        assert_eq!(
            matches!(told, Some(PacedRecord::Waits(_))),
            waits,
            "{decision_id}: a first after the success"
        );
    }
    let pair = pairs.pair(instance, TASK).expect("the pair");
    assert_eq!(
        (pair.in_a_row, pair.against, pair.set_aside),
        (2, 0, false),
        "a store update task's success is no evidence"
    );
}

/// Test 9 (a): one task, `retry_limit` 1, waiting after its second failure in a row. At the wait's
/// end its run is stopped: the pair stays as it was; its next run is settled by a configured
/// condition (the test's seam): the pair stays as it was; its owed rerun fails severely, which
/// pauses the pair: the pair is gone.
#[test]
fn pace_p9_a_a_stop_and_a_condition_leave_the_pair_as_it_was_and_a_pausing_settlement_removes_it() {
    let mut pace = Pace::start(&sources(&[due(TASK, 100)], 1, 3), at(1, 6, 0));
    let first = pace.fail(at(1, 7, 0), TASK);
    let second = pace.fail(first.settled.observed_at_unix_ms + BACKOFF_MS, TASK);
    let until = second.settled.observed_at_unix_ms + MINUTE_MS;
    let waiting = Some((2, 0, until, false));
    assert_eq!(pace.pair(TASK), waiting);
    let stopped = pace.run(until, TASK, |_| PolicyExecutionInput::Stopped {
        outcome: Outcome::new(RuntimeErrorCode::ContainedTaskCancelled),
    });
    assert!(
        matches!(
            stopped.settled.outcome,
            PolicyExecutionOutcome::Stopped { .. }
        ),
        "{:?}",
        stopped.settled
    );
    assert_eq!(pace.pair(TASK), waiting, "a stop leaves the pair as it was");
    pace.host()
        .settle_next_policy_run_by_condition_for_test(PolicyConditionSettlement {
            refunded: true,
            count: 1,
            expires_at_unix_ms: second.window_end,
        })
        .expect("arm the condition");
    let ended = pace.fail(stopped.settled.observed_at_unix_ms + 1_000, TASK);
    assert!(failure_of(&ended.settled).condition.is_some());
    assert_eq!(
        pace.pair(TASK),
        waiting,
        "a condition leaves the pair as it was"
    );
    let paused = pace.run(ended.settled.observed_at_unix_ms + 1_000, TASK, |_| {
        PolicyExecutionInput::Failed {
            outcome: Outcome::new(HostCode::ArtifactStoreFailure),
            failure_identity: None,
            class: PolicyFailureClass::Severe,
        }
    });
    assert_eq!(
        failure_of(&paused.settled).disposition,
        PolicyFailureDisposition::PausedTask
    );
    assert_eq!(
        pace.pair(TASK),
        None,
        "a settlement that pauses the pair removes it"
    );
    pace.close();
}

/// Test 9 (b): one task, `retry_limit` 1, waiting after its second failure in a row. At the wait's
/// end its run fails at its entry (the settlement armed so, as `gate_490_entry_a` arms it): the
/// record is a condition of the instance, this slice's pair stays as it was, and ENTRY-A's pair and
/// block are as on main for the same inputs (one in a row, none against it, waiting the first
/// interval from the settlement; the instance blocked).
#[test]
fn pace_p9_b_a_failure_at_the_entry_leaves_the_pair_as_it_was_and_entry_a_counts_it_as_on_main() {
    let mut pace = Pace::start(&sources(&[due(TASK, 100)], 1, 3), at(1, 6, 0));
    let first = pace.fail(at(1, 7, 0), TASK);
    let second = pace.fail(first.settled.observed_at_unix_ms + BACKOFF_MS, TASK);
    let until = second.settled.observed_at_unix_ms + MINUTE_MS;
    pace.host()
        .settle_next_policy_run_at_entry_for_test()
        .expect("arm the settlement");
    let at_entry = pace.fail(until, TASK);
    let record = failure_of(&at_entry.settled);
    assert!(record.condition.is_some(), "a condition of the instance");
    assert!(crate::policy_control::entry_blocked(&record));
    assert_eq!(
        pace.pair(TASK),
        Some((2, 0, until, false)),
        "the pair stays as it was"
    );
    assert_eq!(
        pace.host()
            .entry_pair_for_test(ALIAS, TASK)
            .expect("ENTRY-A's pair"),
        Some((
            1,
            0,
            at_entry.settled.observed_at_unix_ms + MINUTE_MS,
            false
        )),
        "ENTRY-A counts it as on main"
    );
    assert!(
        pace.host()
            .entry_block_for_test(ALIAS)
            .expect("ENTRY-A's block")
            .is_some(),
        "the instance is entry-blocked"
    );
    pace.close();
}

/// Test 11: one task, `retry_limit` 1. The records of its second to fourth failures in a row, each
/// followed by a wait, are the first failure's in every member their time and their run do not set
/// (as main writes them for the same inputs: `retry_scheduled`, its count of the same error 1, its
/// retry one backoff after its settlement). Then, behind an operator's pause of the instance (no
/// wake of its own), the idle plan counts the waiting task not due with the wait's end as its
/// release; seeded set aside, with the window's end.
#[test]
fn pace_p11_the_failures_records_are_mains_and_the_idle_plan_reads_the_wait_and_the_window_as_its_release()
 {
    let mut pace = Pace::start(&sources(&[due(TASK, 100)], 1, 3), at(1, 6, 0));
    let first = pace.fail(at(1, 7, 0), TASK);
    let main = failure_of(&first.settled);
    assert_eq!(
        (
            main.disposition,
            main.consecutive_same_error,
            main.retry_attempt
        ),
        (PolicyFailureDisposition::RetryScheduled, 1, 1)
    );
    let mut due_at = first.settled.observed_at_unix_ms + BACKOFF_MS;
    let mut latest_at = due_at;
    for wait_ms in WAITS_MS.iter().take(3) {
        let ran = pace.fail(due_at, TASK);
        let record = failure_of(&ran.settled);
        latest_at = ran.settled.observed_at_unix_ms;
        assert_eq!(timeless(&record), timeless(&main), "as main writes it");
        assert_eq!(record.retry_at_unix_ms, Some(latest_at + BACKOFF_MS));
        due_at = latest_at + *wait_ms;
    }
    pace.slots.pause();
    let (_, wake) = deferred_by(
        &pace.evaluate(latest_at + 1_000),
        TASK,
        HostCode::DispatchPausedInstance,
    );
    assert_eq!(wake, None, "the pause names no wake");
    let plan = idle_plan(pace.host());
    assert!(!plan.due_now, "not due behind the pause: {plan:?}");
    assert_eq!(
        plan.next_wake_unix_ms,
        Some(due_at),
        "the wait's end is its release: {plan:?}"
    );
    pace.host()
        .seed_failure_pair_for_test(ALIAS, TASK, (4, latest_at, 0, first.window_end, true))
        .expect("set the pair aside");
    let (_, wake) = deferred_by(
        &pace.evaluate(latest_at + 2_000),
        TASK,
        HostCode::DispatchPausedInstance,
    );
    assert_eq!(wake, None, "the pause names no wake");
    let plan = idle_plan(pace.host());
    assert!(!plan.due_now, "not due behind the pause: {plan:?}");
    assert_eq!(
        plan.next_wake_unix_ms,
        Some(first.window_end),
        "the window's end is its release: {plan:?}"
    );
    pace.close();
}
