// SPDX-License-Identifier: AGPL-3.0-only

// P-2a gate tests of the policy crate: the first half of test plan v2.2 row P-2
// (`rd-1009/tests/test-ci-plan.md:163`; `inventory-policy.md` gaps G8 and G9, fakes I2 and I3;
// `req-basis-cp.md` P-2). Every expected value comes from the cited requirement, not from what
// the code does today.
//
// I2 `FakeAdmission` stands in for the Runtime's read-only admission predicate
// (contracts/scheduling/README.md, "Scope And Overrides"): it answers the eligibility callback of
// `evaluate_with_eligibility` from held, cooldown and budget tables, can fail the read of one
// instance, and records each call. With activity profiles it also gates on their windows and
// counts admissions against the daily limit of the window's day.
//
// I3 is the day replay: minute steps over two days with a restart gap, the pair's last dispatch
// carried from step to step and every selected intent admitted through the fake.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use actingcommand_contract::outcome::Outcome;
use serde_json::{Value, json};

use crate::{
    ActivityProfile, CandidateEligibility, CatalogDocumentSource, CatalogSources, CompiledCatalog,
    DecisionReason, DispatchIntent, EvaluationFacts, EvaluationResources, EvaluationTime,
    HostResourceSnapshot, InstanceSnapshot, PolicyEvaluation, PolicyEvaluationError,
    SchedulingDecisionState, TaskDecision, TaskRuntimeSnapshot, activity_window_at,
    clock_slot_view, compile_catalog, evaluate_with_eligibility,
};

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 86_400_000;
const SEED: u64 = 17;

const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const OPERATION: &str = "operation.run";
const FIRST: &str = "task.first";
const SECOND: &str = "task.second";
const INSTANCE_A: &str = "instance-a";
const INSTANCE_B: &str = "instance-b";
const INSTANCE_C: &str = "instance-c";
const INSTANCE_D: &str = "instance-d";
// Instances A and D share this host; it has room for one run of a task (cpu 200 of 300).
const SHARED_HOST: &str = "host.shared";

// The codes with which the Runtime's admission defers a candidate, registered by runtime-host
// (contracts/scheduling/README.md, "Scope And Overrides" and the gate table of "Clock-slot
// records"; model-369-queue.md W-1).
const INSTANCE_HELD: &str = "dispatch_instance_held";
const INSTANCE_COOLDOWN: &str = "dispatch_instance_cooldown";
const BUDGET_EXHAUSTED: &str = "policy_budget_exhausted";
const WINDOW_CLOSED: &str = "policy_activity_window_closed";

// G8: the evaluation instant, one minute past a whole hour, so the hourly trigger of every task
// is due and its next occurrence lies 59 minutes ahead.
const G8_NOW: u64 = 1_000 * HOUR_MS + MINUTE_MS;
// G8: when the takeover cooldown of instance B ends, and when the refused budget of the first
// task on instance C admits it again; both earlier than any trigger's next occurrence.
const COOLDOWN_END: u64 = G8_NOW + 1_000;
const BUDGET_NEXT: u64 = G8_NOW + 2_000;

// G9: midnight of the first replayed day, its activity window (06:00 to 22:00), the two daily
// clock slots (07:00 and 19:00) and the restart gap: no Runtime runs from 18:00 of day 1 until
// 06:30 of day 2.
const START: u64 = 400 * DAY_MS;
const WINDOW_START_MINUTE: u16 = 360;
const WINDOW_END_MINUTE: u16 = 1_320;
const MORNING: u64 = 7 * HOUR_MS;
const EVENING: u64 = 19 * HOUR_MS;
const STOPPED_AT: u64 = START + 18 * HOUR_MS;
const RESTARTED_AT: u64 = START + DAY_MS + 6 * HOUR_MS + 30 * MINUTE_MS;
const END: u64 = START + 2 * DAY_MS;
// G9: the daily limit of the replayed task.
const DAILY_LIMIT: u32 = 2;

// P-2 G8 (test-ci-plan.md:163; inventory-policy.md key 29, gap G8; req-basis-cp.md:110;
// contracts/scheduling/README.md:258-267; model-369-queue.md L347-350; #369 S4 acceptance).
// Admission defers every candidate of a held instance (none is selected), every candidate of an
// instance in its takeover cooldown, and the first task on instance C, whose budget refuses it;
// each deferred decision's last reason is the admission's code. A deferred candidate leaves its
// instance and its host to the next ranked candidate: the second task on C is selected, and D,
// which shares A's host with room for one run, is selected although both of A's candidates
// ranked above it. The earliest wake of the round is the wake of the cooldown deferral.
#[test]
fn gate_p2a_g8_admission_deferrals_carry_their_reason_and_leave_instance_and_host_to_the_next_candidate()
 {
    let catalog = g8_catalog();
    let mut admission = FakeAdmission::default();
    admission.held.insert(INSTANCE_A.to_owned());
    admission
        .cooldown
        .insert(INSTANCE_B.to_owned(), COOLDOWN_END);
    admission
        .refused
        .insert((FIRST.to_owned(), INSTANCE_C.to_owned()), BUDGET_NEXT);

    let evaluation = evaluate_with_eligibility::<Refusal>(
        &catalog,
        &facts(g8_instances(), Vec::new()),
        &resources(&g8_instances()),
        at(G8_NOW),
        SEED,
        |intent| admission.preview(intent),
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error:?}"));

    for task_id in [FIRST, SECOND] {
        let held = decision(&evaluation, task_id, INSTANCE_A);
        assert_eq!(
            held.state,
            SchedulingDecisionState::Deferred,
            "{task_id} on A"
        );
        assert_eq!(last_reason(held), INSTANCE_HELD, "{task_id} on A");
        let cooling = decision(&evaluation, task_id, INSTANCE_B);
        assert_eq!(
            cooling.state,
            SchedulingDecisionState::Deferred,
            "{task_id} on B"
        );
        assert_eq!(last_reason(cooling), INSTANCE_COOLDOWN, "{task_id} on B");
    }
    let refused = decision(&evaluation, FIRST, INSTANCE_C);
    assert_eq!(refused.state, SchedulingDecisionState::Deferred);
    assert_eq!(last_reason(refused), BUDGET_EXHAUSTED);
    assert_eq!(
        decision(&evaluation, SECOND, INSTANCE_C).state,
        SchedulingDecisionState::Selected,
        "the instance a budget refusal leaves is the next ranked task's"
    );
    let selected_on_d = [FIRST, SECOND]
        .into_iter()
        .filter(|task_id| {
            decision(&evaluation, task_id, INSTANCE_D).state == SchedulingDecisionState::Selected
        })
        .count();
    assert_eq!(
        selected_on_d, 1,
        "the deferred candidates of A consume none of the host D shares with it"
    );
    let dispatched = evaluation
        .dispatch_intents
        .iter()
        .map(|intent| intent.instance_id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(dispatched, BTreeSet::from([INSTANCE_C, INSTANCE_D]));
    assert_eq!(
        evaluation.next_wake_unix_ms,
        Some(COOLDOWN_END),
        "the round wakes at the earliest deferral wake"
    );
}

// P-2 G8, the budget wake (test-ci-plan.md:163: a budget deferral carries its wake;
// contracts/scheduling/README.md:258-267, "the next possible admission time"): when the only
// deferral of a round is a budget refusal, the round wakes at the refusal's next eligible time,
// earlier than the next occurrence of any trigger.
#[test]
fn gate_p2a_g8_a_budget_deferral_wakes_the_round_at_its_next_eligible_time() {
    let catalog = g8_catalog();
    let mut admission = FakeAdmission::default();
    admission
        .refused
        .insert((FIRST.to_owned(), INSTANCE_C.to_owned()), BUDGET_NEXT);

    let evaluation = evaluate_with_eligibility::<Refusal>(
        &catalog,
        &facts(g8_instances(), Vec::new()),
        &resources(&g8_instances()),
        at(G8_NOW),
        SEED,
        |intent| admission.preview(intent),
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error:?}"));

    let refused = decision(&evaluation, FIRST, INSTANCE_C);
    assert_eq!(refused.state, SchedulingDecisionState::Deferred);
    assert_eq!(last_reason(refused), BUDGET_EXHAUSTED);
    assert_eq!(evaluation.next_wake_unix_ms, Some(BUDGET_NEXT));
}

// P-2 G8, the failing read (inventory-policy.md key 29: "a callback error propagates";
// contracts/scheduling/README.md:258-267; Fail Loud): when the admission read of one candidate
// fails, the evaluation fails with that very error and decides nothing.
#[test]
fn gate_p2a_g8_an_admission_read_failure_fails_the_evaluation_with_that_error() {
    let catalog = g8_catalog();
    let instances = g8_instances()
        .into_iter()
        .filter(|instance| instance.instance_id != INSTANCE_A)
        .collect::<Vec<_>>();
    let mut admission = FakeAdmission::default();
    admission.failing.insert(INSTANCE_D.to_owned());

    let result = evaluate_with_eligibility::<Refusal>(
        &catalog,
        &facts(instances.clone(), Vec::new()),
        &resources(&instances),
        at(G8_NOW),
        SEED,
        |intent| admission.preview(intent),
    );

    assert_eq!(
        result.map(|evaluation| evaluation.dispatch_intents.len()),
        Err(Refusal::AdmissionRead(INSTANCE_D.to_owned()))
    );
    assert_eq!(
        admission
            .calls
            .last()
            .map(|(_, instance_id, time)| (instance_id.as_str(), *time)),
        Some((INSTANCE_D, G8_NOW)),
        "the evaluation ends at the failed read"
    );
}

// P-2 G9 (test-ci-plan.md:163; inventory-policy.md gap G9, fake I3; req-basis-cp.md:111;
// contracts/scheduling/README.md:734 (Workflow #361) and "Clock-slot records"; model-372-skip.md
// §0, §3.1). Two days in minute steps of a task with daily clock slots at 07:00 and 19:00, a
// daily limit of 2 and an activity window from 06:00 to 22:00; no Runtime runs from 18:00 of day
// 1 until 06:30 of day 2. At every evaluation at which `clock_slot_view` gives the pair a due
// occurrence, the pair has a decision, and it is either selected or held back by an admission
// gate (its last reason the gate's code), never missing. The occurrence before the first window
// runs at the window's opening; day 1's evening slot, missed while no Runtime ran, runs once at
// the first admissible evaluation of day 2 and counts against day 2's limit; so day 2's own
// evening slot meets the exhausted budget, visibly, at its own time.
#[test]
fn gate_p2a_g9_a_two_day_replay_serves_or_gates_every_due_slot_and_counts_an_owed_run_on_its_day() {
    let catalog = g9_catalog();
    let task = catalog
        .catalog()
        .tasks
        .tasks
        .iter()
        .find(|task| task.id == FIRST)
        .expect("the replayed task");
    let mut admission = FakeAdmission {
        profiles: catalog.catalog().activity.profiles.clone(),
        ..FakeAdmission::default()
    };
    let instances = vec![instance(INSTANCE_A, "host.a")];
    let hosts = resources(&instances);
    let mut last_dispatched = None;
    let mut dispatched = Vec::new();
    // Per due occurrence: when it was served, and every gate that held it back (time, code).
    let mut served = BTreeMap::<u64, u64>::new();
    let mut gated = BTreeMap::<u64, Vec<(u64, String)>>::new();

    let mut now = START;
    while now < END {
        if (STOPPED_AT..RESTARTED_AT).contains(&now) {
            now += MINUTE_MS;
            continue;
        }
        let state = task_state(last_dispatched);
        let view = clock_slot_view(task, Some(&state), at(now))
            .unwrap_or_else(|error| panic!("at {now}: the clock view reads: {error}"))
            .unwrap_or_else(|| panic!("at {now}: the daily clock of the task counts"));
        let evaluation = evaluate_with_eligibility::<Refusal>(
            &catalog,
            &facts(instances.clone(), vec![state]),
            &hosts,
            at(now),
            SEED,
            |intent| admission.preview(intent),
        )
        .unwrap_or_else(|error| panic!("at {now}: the replay evaluates: {error:?}"));
        let pair = evaluation
            .decisions
            .iter()
            .find(|decision| {
                decision.task_id == FIRST && decision.instance_id.as_deref() == Some(INSTANCE_A)
            })
            .unwrap_or_else(|| panic!("at {now}: the pair has a decision"));
        match view.due_occurrence {
            Some(occurrence) if pair.state == SchedulingDecisionState::Selected => {
                served.entry(occurrence).or_insert(now);
            }
            Some(occurrence) => {
                let code = last_reason(pair);
                assert!(
                    [WINDOW_CLOSED, BUDGET_EXHAUSTED].contains(&code),
                    "at {now}: the due occurrence {occurrence} is held back by an admission gate, \
                     not by {code}"
                );
                gated
                    .entry(occurrence)
                    .or_default()
                    .push((now, code.to_owned()));
            }
            None => assert_ne!(
                pair.state,
                SchedulingDecisionState::Selected,
                "at {now}: no occurrence is due, so nothing is selected"
            ),
        }
        for intent in &evaluation.dispatch_intents {
            admission.admit(intent);
            last_dispatched = Some(now);
            dispatched.push(now);
        }
        now += MINUTE_MS;
    }

    let day_1 = START;
    let day_2 = START + DAY_MS;
    let first_slot = day_1 - DAY_MS + EVENING;
    assert_eq!(
        dispatched,
        [
            day_1 + u64::from(WINDOW_START_MINUTE) * MINUTE_MS,
            day_1 + MORNING,
            RESTARTED_AT,
            day_2 + MORNING,
        ],
        "four runs: the slot before the first window at its opening, day 1's morning, day 1's \
         missed evening at the restart, day 2's morning"
    );
    assert_eq!(
        gated.get(&first_slot).and_then(|gates| gates.first()),
        Some(&(day_1, WINDOW_CLOSED.to_owned())),
        "the slot before the first window waits for the window"
    );
    assert_eq!(
        served.get(&first_slot),
        Some(&(day_1 + u64::from(WINDOW_START_MINUTE) * MINUTE_MS))
    );
    assert_eq!(served.get(&(day_1 + MORNING)), Some(&(day_1 + MORNING)));
    assert_eq!(
        served.get(&(day_1 + EVENING)),
        Some(&RESTARTED_AT),
        "the owed evening slot of day 1 runs once, at the first admissible evaluation of day 2"
    );
    assert_eq!(served.get(&(day_2 + MORNING)), Some(&(day_2 + MORNING)));
    assert_eq!(admission.used(FIRST, INSTANCE_A, local_day(day_1)), 2);
    assert_eq!(
        admission.used(FIRST, INSTANCE_A, local_day(day_2)),
        2,
        "the owed run counts against the day it ran in"
    );
    let refused_slot = day_2 + EVENING;
    assert!(
        !served.contains_key(&refused_slot),
        "day 2's evening slot does not run"
    );
    assert_eq!(
        gated.get(&refused_slot).and_then(|gates| gates.first()),
        Some(&(refused_slot, BUDGET_EXHAUSTED.to_owned())),
        "day 2's evening slot is refused by the exhausted budget at its own time"
    );
}

// The error type of the callback: the evaluation's own error, or the fake's failed read of an
// instance.
#[derive(PartialEq)]
enum Refusal {
    Evaluation(PolicyEvaluationError),
    AdmissionRead(String),
}

impl From<PolicyEvaluationError> for Refusal {
    fn from(error: PolicyEvaluationError) -> Self {
        Self::Evaluation(error)
    }
}

impl fmt::Debug for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evaluation(error) => write!(formatter, "evaluation: {error}"),
            Self::AdmissionRead(instance_id) => {
                write!(formatter, "admission read of {instance_id} failed")
            }
        }
    }
}

// I2: the Runtime's read-only admission predicate as tables, and the admissions it counted.
#[derive(Default)]
struct FakeAdmission {
    // Instances another holder keeps: deferred with no wake (model-369-queue.md W-1).
    held: BTreeSet<String>,
    // Instances in a takeover cooldown, with its end: deferred with a wake at that end.
    cooldown: BTreeMap<String, u64>,
    // (task, instance) pairs whose budget refuses them, with their next eligible time.
    refused: BTreeMap<(String, String), u64>,
    // Instances whose admission read fails.
    failing: BTreeSet<String>,
    // Activity profiles whose windows gate admission and whose days carry the daily limit;
    // empty when the fake applies neither.
    profiles: Vec<ActivityProfile>,
    // Admissions per (task, instance, local day of the activity window they ran in).
    admitted: BTreeMap<(String, String, i64), u32>,
    // Every call, in order: (task, instance, evaluation time).
    calls: Vec<(String, String, u64)>,
}

impl FakeAdmission {
    // The callback's answer for one ranked candidate.
    fn preview(&mut self, intent: &DispatchIntent) -> Result<CandidateEligibility, Refusal> {
        let time = intent.prerequisites.evaluated_at_unix_ms;
        self.calls
            .push((intent.task_id.clone(), intent.instance_id.clone(), time));
        if self.failing.contains(&intent.instance_id) {
            return Err(Refusal::AdmissionRead(intent.instance_id.clone()));
        }
        if self.held.contains(&intent.instance_id) {
            return Ok(deferred(INSTANCE_HELD, None));
        }
        if let Some(until) = self.cooldown.get(&intent.instance_id) {
            return Ok(deferred(INSTANCE_COOLDOWN, Some(*until)));
        }
        let pair = (intent.task_id.clone(), intent.instance_id.clone());
        if let Some(next) = self.refused.get(&pair) {
            return Ok(deferred(BUDGET_EXHAUSTED, Some(*next)));
        }
        if self.profile(intent).is_some() {
            let Some(day) = self.window_day(intent) else {
                return Ok(deferred(WINDOW_CLOSED, Some(next_window_start(time))));
            };
            if self.used(&intent.task_id, &intent.instance_id, day)
                >= intent.prerequisites.daily_limit
            {
                return Ok(deferred(BUDGET_EXHAUSTED, Some(next_window_start(time))));
            }
        }
        Ok(CandidateEligibility::Eligible)
    }

    // The final admission of a selected intent: it counts against the day of the activity
    // window open when it runs.
    fn admit(&mut self, intent: &DispatchIntent) {
        let day = self.window_day(intent).unwrap_or_else(|| {
            panic!(
                "{} on {} is admitted inside its activity window",
                intent.task_id, intent.instance_id
            )
        });
        *self
            .admitted
            .entry((intent.task_id.clone(), intent.instance_id.clone(), day))
            .or_default() += 1;
    }

    // The admissions of a pair on one local day.
    fn used(&self, task_id: &str, instance_id: &str, day: i64) -> u32 {
        self.admitted
            .get(&(task_id.to_owned(), instance_id.to_owned(), day))
            .copied()
            .unwrap_or(0)
    }

    fn profile(&self, intent: &DispatchIntent) -> Option<&ActivityProfile> {
        self.profiles
            .iter()
            .find(|profile| profile.id == intent.prerequisites.activity_profile_id)
    }

    // The local day of the activity window open when the intent was evaluated; `None` when
    // none is open.
    fn window_day(&self, intent: &DispatchIntent) -> Option<i64> {
        let profile = self.profile(intent)?;
        activity_window_at(profile, intent.prerequisites.evaluated_at_unix_ms)
            .unwrap_or_else(|error| panic!("the activity window reads: {error}"))
            .map(|window| window.local_day)
    }
}

fn deferred(code: &str, next_wake_unix_ms: Option<u64>) -> CandidateEligibility {
    CandidateEligibility::Deferred {
        reason: DecisionReason {
            outcome: Outcome::relayed(code),
            detail: format!("fixture admission: {code}"),
        },
        next_wake_unix_ms,
    }
}

// The next opening of the G9 activity window after `time`.
fn next_window_start(time: u64) -> u64 {
    let opening = (time / DAY_MS) * DAY_MS + u64::from(WINDOW_START_MINUTE) * MINUTE_MS;
    if time < opening {
        opening
    } else {
        opening + DAY_MS
    }
}

// The local day of a G9 time: its windows are in UTC.
fn local_day(time: u64) -> i64 {
    i64::try_from(time / DAY_MS).expect("a day index")
}

fn at(unix_ms: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms,
        monotonic_ms: unix_ms,
    }
}

fn decision<'a>(
    evaluation: &'a PolicyEvaluation,
    task_id: &str,
    instance_id: &str,
) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(instance_id)
        })
        .unwrap_or_else(|| panic!("a decision for {task_id} on {instance_id}"))
}

// The reason that settled a decision: its last.
fn last_reason(decision: &TaskDecision) -> &str {
    decision
        .reasons
        .last()
        .map(|reason| reason.outcome.code().as_str())
        .unwrap_or_else(|| panic!("{} carries a reason", decision.task_id))
}

fn task_state(last_dispatched_unix_ms: Option<u64>) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: FIRST.to_owned(),
        instance_id: INSTANCE_A.to_owned(),
        last_dispatched_unix_ms,
        eligible_since_unix_ms: Some(START),
        terminal_state: None,
        completed_window: None,
        last_duration_ms: None,
        failure_streak: None,
    }
}

fn instance(instance_id: &str, host_id: &str) -> InstanceSnapshot {
    InstanceSnapshot {
        instance_id: instance_id.to_owned(),
        server_id: SERVER.to_owned(),
        game_id: GAME.to_owned(),
        host_id: host_id.to_owned(),
        available: true,
        capability_operation_ids: vec![OPERATION.to_owned()],
        preferred_task_ids: Vec::new(),
    }
}

// A and D on the shared host, B and C on hosts of their own.
fn g8_instances() -> Vec<InstanceSnapshot> {
    vec![
        instance(INSTANCE_A, SHARED_HOST),
        instance(INSTANCE_B, "host.b"),
        instance(INSTANCE_C, "host.c"),
        instance(INSTANCE_D, SHARED_HOST),
    ]
}

fn facts(instances: Vec<InstanceSnapshot>, tasks: Vec<TaskRuntimeSnapshot>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-p2a".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks,
        instances,
        priority_offsets: Vec::new(),
    }
}

// Every host of the instances, each with full capacity, except the shared host, which has room
// for one run of a task.
fn resources(instances: &[InstanceSnapshot]) -> EvaluationResources {
    let hosts = instances
        .iter()
        .map(|instance| instance.host_id.as_str())
        .collect::<BTreeSet<_>>();
    EvaluationResources {
        pools: Vec::new(),
        hosts: hosts
            .into_iter()
            .map(|host_id| HostResourceSnapshot {
                host_id: host_id.to_owned(),
                cpu_available_milli: if host_id == SHARED_HOST { 300 } else { 1_000 },
                gpu_available_milli: 1_000,
                io_available_milli: 1_000,
                host_responsiveness_basis_points: 10_000,
                third_party_pressure_basis_points: 0,
                heavy_dispatch_limit: 1,
                active_heavy_dispatches: 0,
            })
            .collect(),
    }
}

// G8: two tasks on every instance of the game, each due by an hourly clock; on A both rank
// first, on C the first task ranks above the second; one activity profile open all day.
fn g8_catalog() -> CompiledCatalog {
    let hourly = json!({
        "kind": "clock",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": HOUR_MS,
            "anchor_ms": 0
        }
    });
    catalog(
        vec![
            task(
                FIRST,
                hourly.clone(),
                24,
                json!([
                    override_priority(INSTANCE_A, 150),
                    override_priority(INSTANCE_C, 140)
                ]),
            ),
            task(
                SECOND,
                hourly,
                24,
                json!([override_priority(INSTANCE_A, 150)]),
            ),
        ],
        0,
        0,
    )
}

// G9: one task with daily clock slots at 07:00 and 19:00 (UTC) and a daily limit of 2; its
// activity window is open from 06:00 to 22:00.
fn g9_catalog() -> CompiledCatalog {
    let daily = json!({
        "kind": "clock",
        "schedule": {
            "kind": "daily",
            "clock_source": {
                "kind": "server",
                "timezone_id": "etc/utc",
                "utc_offset_minutes": 0,
                "dst_offset_minutes": 0,
                "maintenance_drift_ms": 0
            },
            "minutes_of_day": [MORNING / MINUTE_MS, EVENING / MINUTE_MS]
        }
    });
    catalog(
        vec![task(FIRST, daily, DAILY_LIMIT, json!([]))],
        WINDOW_START_MINUTE,
        WINDOW_END_MINUTE,
    )
}

fn override_priority(instance_id: &str, priority: i16) -> Value {
    json!({
        "instance_id": instance_id,
        "enabled": null,
        "priority": priority,
        "strategic_weight_milli": null,
        "load_profile": null
    })
}

fn task(task_id: &str, trigger: Value, daily_limit: u32, overrides: Value) -> Value {
    json!({
        "id": task_id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{task_id}"),
        "priority": 100,
        "trigger": trigger,
        "feedback_stop": {
            "kind": "outcome",
            "task_id": task_id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
        "produces": [],
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": DAY_MS,
        "yield_points": [],
        "expected_duration_ms": 60000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "loop_budget": {
            "daily_limit": daily_limit,
            "window_iteration_limit": 4,
            "max_runtime_ms": 300000
        },
        "strategic_weight_milli": 1000,
        "instance_overrides": overrides
    })
}

// Compiles the tasks with one activity profile for the whole game, open every day from
// `window_start` to `window_end` (minutes of the UTC day; equal values mean all day), and no
// pools or timeline events.
fn catalog(tasks: Vec<Value>, window_start: u16, window_end: u16) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.p2a-catalog",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-p2a"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let profile = json!({
        "id": "fixture-activity",
        "scope": {"kind": "game", "game_id": GAME},
        "windows": [{
            "weekdays": [1, 2, 3, 4, 5, 6, 7],
            "utc_offset_minutes": 0,
            "start_minute_of_day": window_start,
            "end_minute_of_day": window_end
        }],
        "daily_budget": 24,
        "max_window_iterations": 4,
        "session_max_ms": 7200000,
        "detection_budget": {
            "window_dispatch_limit": 4,
            "window_runtime_ms": 300000,
            "expected_duration_ms": 60000
        },
        "minimum_interval_ms": 60000,
        "maximum_interval_ms": 300000,
        "seed_source": "ledger",
        "resample_policy": "same_round_stable",
        "importance_milli": 1000,
        "goals": []
    });
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-p2a/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}
