// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 P19 gate tests (comment 6455013768 items 1 and 2; coordinator work order
// `rd-1028/brief-wave1.md` W1-4, the declarative way; `contracts/scheduling/README.md`, "Reset
// exclusions and the task's own last result"): (1) a timeline reset may declare key prefixes it
// does not clear, `excludes_fact_prefixes`, so a ready time keeps its reading through the reset;
// (2) a reset never clears a task's own last result, which its own `dependency_completed`,
// `since_own_run`'s own run and `once_per_reading`'s served check read. A neutral V2 catalog on
// instance `instance-a` with one daily reset, whose latest occurrence is half an hour before the
// round and after every reading and run of the fixtures. Every expected value comes from the work
// order and the contract text, never from what the code does.

use std::collections::BTreeMap;

use actingcommand_contract::{
    STANDARD_PACK_SCHEDULE_FILE, StandardPackProblem, StandardPackRootBytes,
    read_standard_pack_root,
};
use serde_json::{Map, Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::evaluator::project_time_validity;
use crate::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, CompletedActivityWindow,
    DecisionReason, DetectionSuggestion, EligibilityState, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactValue, HostResourceSnapshot, ObservedFact, PolicyEvaluation,
    RESET_EXCLUSIONS_SUBSYSTEM, SCHEDULING_SCHEMA_VERSION_V2, SchedulingDecisionState,
    ScopeSelector, StandardPackBinding, StandardPackComposeFailure, StandardPackComposeInput,
    StandardPackComposition, StandardPackGameFiles, TaskDecision, TaskRuntimeSnapshot,
    TaskTerminalState, compile_catalog, compose_standard_pack_rules, evaluate,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
// The tasks: two that read readings below one reset prefix, a chain step with its own success
// gate and a follower of it, an edge pair, and a task due once per reading.
const READY_TASK: &str = "task.ready";
const AMOUNT_TASK: &str = "task.amount";
const STEP: &str = "task.step";
const FOLLOWER: &str = "task.follower";
const LEAD: &str = "task.lead";
const NEXT: &str = "task.next";
const SLOT: &str = "task.slot";
// The readings: a ready time (an absolute time that expires by itself) below an excluded prefix,
// and an amount beside it.
const READY_PREFIX: &str = "resource.neutral.ready.";
const READY_AT: &str = "resource.neutral.ready.slot";
const AMOUNT: &str = "resource.neutral.amount";
const MAX_AGE_MS: u64 = 4 * HOUR_MS;
const RESET_ID: &str = "reset.daily";
// The reset's latest occurrence, half an hour before the round; the next one a day after it.
const RESET_AT: u64 = NOW - 1_800_000;
// Every reading and run of the fixtures is from before the reset: the readings two hours before
// the round, the lead's run 100 minutes before it and every other run 90 minutes before it, each
// settled a minute after its dispatch.
const READ_AT: u64 = NOW - 2 * HOUR_MS;
const LEAD_RAN: u64 = NOW - 6_000_000;
const RAN: u64 = NOW - 5_400_000;
const SETTLED_AFTER_MS: u64 = 60_000;
const SHORT_COOLDOWN_MS: u64 = 1_000;
const TRIGGER_FALSE: &str = "trigger_false";
// The standard-pack fixtures: one game root with one package on one server, one bound instance.
const SCHEDULE_INSTANCE: &str = "fixture-instance-a";
const SERVER: &str = "server-a";
const PACKAGE: &str = "fixture.neutral";

// Item 1: a reset that invalidates `resource.` and excludes `resource.neutral.ready.` keeps the
// ready time's reading through the reset (no expiry cap at its next occurrence), so the task due
// at that time is dispatched; the amount beside it is cleared as before (its task is Unknown with
// `fact_missing`). Without the member the ready time is cleared too (`fact_time_unavailable`
// with `missing`). A `schedule.json` whose event writes the member composes when `requires` names
// `reset_exclusions`, keeping the member, and is refused at the member without it.
#[test]
fn p19_an_excluded_prefix_survives_a_reset() {
    let tasks = || {
        json!([
            task(READY_TASK, 200, ready_trigger(false), SHORT_COOLDOWN_MS),
            task(AMOUNT_TASK, 100, amount_trigger(), SHORT_COOLDOWN_MS),
        ])
    };
    let readings = || vec![ready_at(NOW - 600_000, READ_AT), amount(5)];

    let plain = neutral_catalog(tasks(), reset(&["resource."], None));
    let cleared = evaluate_at(&plain, readings(), Vec::new());
    assert!(
        cleared.dispatch_intents.is_empty(),
        "without the member the reset clears the ready time: {:#?}",
        cleared.dispatch_intents
    );
    let waiting = decision(&cleared, READY_TASK);
    assert_eq!(codes(waiting), ["fact_time_unavailable", TRIGGER_FALSE]);
    assert_values(
        &waiting.reasons[0],
        &[
            ("fact_key", json!(READY_AT)),
            ("fact_time_problem", json!("missing")),
        ],
    );

    let excluded = neutral_catalog(tasks(), reset(&["resource."], Some(&[READY_PREFIX][..])));
    let kept = evaluate_at(&excluded, readings(), Vec::new());
    assert_eq!(dispatched(&kept), [READY_TASK], "the ready time survives");
    let other = decision(&kept, AMOUNT_TASK);
    assert_eq!(other.eligibility, EligibilityState::Unknown);
    assert_eq!(
        other.detection_suggestions,
        [suggestion(AMOUNT, "fact_missing")],
        "every other key below the invalidated prefix is still cleared"
    );
    let projected = project_time_validity(
        &excluded,
        &evaluation_facts(readings(), Vec::new()),
        round_time(),
    )
    .unwrap_or_else(|error| panic!("the projection succeeds: {error}"));
    let survivors: Vec<(&str, Option<u64>)> = projected
        .facts
        .iter()
        .map(|fact| (fact.fact_key.as_str(), fact.expires_at_unix_ms))
        .collect();
    assert_eq!(
        survivors,
        [(READY_AT, None::<u64>)],
        "only the excluded key survives, its expiry not capped at the next reset"
    );

    let composed = compose(&schedule(true))
        .unwrap_or_else(|failure| panic!("the schedule composes: {failure}"));
    assert_eq!(composed.games()[0].requires, [RESET_EXCLUSIONS_SUBSYSTEM]);
    let event = serde_json::to_value(&composed.compiled().catalog().timeline.events[0])
        .expect("the event serializes");
    assert_eq!(event["excludes_fact_prefixes"], json!([READY_PREFIX]));
    assert_eq!(
        refused(compose(&schedule(false))),
        (
            StandardPackProblem::ScheduleInvalid,
            "/timeline/0/excludes_fact_prefixes".to_owned()
        ),
        "without the requires entry"
    );
}

// Item 2: a reset that clears `task.` state leaves the step's own last result. The step's own
// success gate (`dependency_completed` on itself) reads it: a last success makes the step due, a
// last failure makes the gate false (`trigger_false`), never Unknown. Another task that names the
// step still reads it as the reset left it (Unknown, `dependency_state_missing`). And a reset
// that clears the successor's state alone leaves an edge it already served served
// (`dependency_already_served`): `since_own_run` reads the successor's own last run.
#[test]
fn p19_the_steps_own_last_result_survives_a_reset() {
    let own_gate = || dependency(STEP, None);
    let catalog = neutral_catalog(
        json!([
            task(STEP, 200, own_gate(), SHORT_COOLDOWN_MS),
            task(FOLLOWER, 100, own_gate(), SHORT_COOLDOWN_MS),
        ]),
        reset(&["task."], None),
    );

    let succeeded = evaluate_at(
        &catalog,
        Vec::new(),
        vec![ran(STEP, RAN, TaskTerminalState::Succeeded)],
    );
    assert_eq!(
        dispatched(&succeeded),
        [STEP],
        "the own success gate reads the last success"
    );
    let follower = decision(&succeeded, FOLLOWER);
    assert_eq!(follower.eligibility, EligibilityState::Unknown);
    assert_eq!(
        follower.detection_suggestions,
        [suggestion(
            &format!("task.{STEP}.terminal_state"),
            "dependency_state_missing"
        )],
        "another task reads the step as the reset left it"
    );

    let failed = evaluate_at(
        &catalog,
        Vec::new(),
        vec![ran(STEP, RAN, TaskTerminalState::Failed)],
    );
    assert!(failed.dispatch_intents.is_empty());
    let gated = decision(&failed, STEP);
    assert_eq!(gated.eligibility, EligibilityState::False);
    assert_eq!(gated.state, SchedulingDecisionState::Blocked);
    assert_eq!(codes(gated), [TRIGGER_FALSE]);

    // The reset clears the successor's state alone; the lead's stays.
    let successor_state = format!("task.{NEXT}.");
    let edge = neutral_catalog(
        json!([
            task(LEAD, 200, hourly(), 2 * HOUR_MS),
            task(NEXT, 100, dependency(LEAD, Some(true)), SHORT_COOLDOWN_MS),
        ]),
        reset(&[successor_state.as_str()], None),
    );
    let served = evaluate_at(
        &edge,
        Vec::new(),
        vec![
            ran(LEAD, LEAD_RAN, TaskTerminalState::Succeeded),
            ran(NEXT, RAN, TaskTerminalState::Succeeded),
        ],
    );
    assert!(
        served.dispatch_intents.is_empty(),
        "an edge served before the reset stays served: {:#?}",
        served.dispatch_intents
    );
    let next = decision(&served, NEXT);
    assert_eq!(codes(next), ["dependency_already_served", TRIGGER_FALSE]);
    assert_values(
        &next.reasons[0],
        &[
            ("task_id", json!(NEXT)),
            ("predecessor_task_id", json!(LEAD)),
        ],
    );
}

// Item 2: `once_per_reading`'s served check reads the task's own last result. A reading the task
// served before a reset that clears `task.` state stays served after it: false with
// `fact_time_served {fact_key, served_by_dispatch_unix_ms}`, then `trigger_false`, no wake. A last
// run that failed served nothing, so the same reading makes the task due.
#[test]
fn p19_the_served_check_reads_the_own_last_result() {
    let catalog = neutral_catalog(
        json!([task(SLOT, 100, ready_trigger(true), SHORT_COOLDOWN_MS)]),
        reset(&["task."], None),
    );
    let reading = || vec![ready_at(NOW - 3 * HOUR_MS, NOW - 3 * HOUR_MS)];

    let served = evaluate_at(
        &catalog,
        reading(),
        vec![ran(SLOT, RAN, TaskTerminalState::Succeeded)],
    );
    assert!(
        served.dispatch_intents.is_empty(),
        "a reading served before the reset stays served: {:#?}",
        served.dispatch_intents
    );
    let waiting = decision(&served, SLOT);
    assert_eq!(waiting.state, SchedulingDecisionState::Blocked);
    assert_eq!(codes(waiting), ["fact_time_served", TRIGGER_FALSE]);
    assert_values(
        &waiting.reasons[0],
        &[
            ("fact_key", json!(READY_AT)),
            ("served_by_dispatch_unix_ms", json!(RAN)),
        ],
    );
    assert_eq!(waiting.next_wake_unix_ms, None, "no wake");

    let failed = evaluate_at(
        &catalog,
        reading(),
        vec![ran(SLOT, RAN, TaskTerminalState::Failed)],
    );
    assert_eq!(dispatched(&failed), [SLOT], "a failed run served nothing");
}

// ---- Fixtures ----

// A V2 catalog of `tasks` with one activity profile and the one timeline event `event`.
fn neutral_catalog(tasks: Value, event: Value) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.reset-exclusions",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-reset-exclusions"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let source = |name: &str, document: &Value| {
        CatalogDocumentSource::new(
            format!("memory://fixture-reset-exclusions/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    let mut profile = profile();
    profile.insert("scope".to_owned(), json!({"kind": "game", "game_id": GAME}));
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", tasks)),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([event]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

// The daily reset of instance A, invalidating `invalidated` and, when given, excluding
// `excluded`.
fn reset(invalidated: &[&str], excluded: Option<&[&str]>) -> Value {
    let mut event = json!({
        "id": RESET_ID,
        "scope": {"kind": "instance", "instance_id": A},
        "event_kind": "reset",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": DAY_MS,
            "anchor_ms": RESET_AT
        },
        "duration_ms": 0,
        "invalidates_fact_prefixes": invalidated,
        "validity": {"from_unix_ms": 0, "until_unix_ms": null}
    });
    if let Some(excluded) = excluded {
        event["excludes_fact_prefixes"] = json!(excluded);
    }
    event
}

fn task(task_id: &str, priority: i16, trigger: Value, cooldown_ms: u64) -> Value {
    json!({
        "id": task_id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": "operation.run"},
        "procedure_ref": format!("procedure.{task_id}"),
        "priority": priority,
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
        "cooldown_ms": cooldown_ms,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "loop_budget": {"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

// Due once the ready time has come; once per reading when `once`.
fn ready_trigger(once: bool) -> Value {
    let mut trigger = json!({
        "kind": "fact_time_reached",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": READY_AT,
        "offset_ms": 0,
        "max_age_ms": MAX_AGE_MS
    });
    if once {
        trigger["once_per_reading"] = json!(true);
    }
    trigger
}

// Due while the amount is at least 1.
fn amount_trigger() -> Value {
    json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": AMOUNT,
        "comparison": "greater_than_or_equal",
        "value": {"type": "integer", "value": 1},
        "max_age_ms": MAX_AGE_MS
    })
}

// Due on `task_id`'s success, as an edge when `since_own_run` is given.
fn dependency(task_id: &str, since_own_run: Option<bool>) -> Value {
    let mut predicate = json!({
        "kind": "dependency_completed",
        "task_id": task_id,
        "terminal_states": ["succeeded"]
    });
    if let Some(since_own_run) = since_own_run {
        predicate["since_own_run"] = json!(since_own_run);
    }
    predicate
}

// A clock due at every whole hour (the instance's monotonic clock equals Unix time here).
fn hourly() -> Value {
    json!({
        "kind": "clock",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": HOUR_MS,
            "anchor_ms": 0
        }
    })
}

// One profile, active all day on every weekday, without its scope.
fn profile() -> Map<String, Value> {
    let Value::Object(profile) = json!({
        "id": "fixture-activity",
        "windows": [{
            "weekdays": [1, 2, 3, 4, 5, 6, 7],
            "utc_offset_minutes": 0,
            "start_minute_of_day": 0,
            "end_minute_of_day": 0
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
    }) else {
        unreachable!("a JSON object")
    };
    profile
}

// `task_id`'s latest run on instance A, dispatched at `at`, settled a minute later as
// `terminal_state`.
fn ran(task_id: &str, at: u64, terminal_state: TaskTerminalState) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: task_id.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(at),
        eligible_since_unix_ms: None,
        terminal_state: Some(terminal_state),
        completed_window: Some(CompletedActivityWindow {
            window_id: "fixture-window".to_owned(),
            completed_at_unix_ms: at + SETTLED_AFTER_MS,
        }),
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
    }
}

// Instance A's ready time `held`, read at `observed_at`, full confidence, no own expiry.
fn ready_at(held: u64, observed_at: u64) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: READY_AT.to_owned(),
        value: FactValue::TimestampMs(held),
        observed_at_unix_ms: observed_at,
        expires_at_unix_ms: None,
        confidence_milli: 1_000,
    }
}

// Instance A's amount, read with the ready time, full confidence, no own expiry.
fn amount(value: i64) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: AMOUNT.to_owned(),
        value: FactValue::Integer(value),
        observed_at_unix_ms: READ_AT,
        expires_at_unix_ms: None,
        confidence_milli: 1_000,
    }
}

fn suggestion(fact_key: &str, reason: &str) -> DetectionSuggestion {
    DetectionSuggestion {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: fact_key.to_owned(),
        reason: reason.to_owned(),
    }
}

fn evaluation_facts(facts: Vec<ObservedFact>, tasks: Vec<TaskRuntimeSnapshot>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-reset-exclusions".to_owned(),
        facts,
        outcomes: Vec::new(),
        tasks,
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

fn round_time() -> EvaluationTime {
    EvaluationTime {
        unix_ms: NOW,
        monotonic_ms: NOW,
    }
}

// Evaluates instance A at NOW; the host has room for one heavy dispatch.
fn evaluate_at(
    catalog: &CompiledCatalog,
    facts: Vec<ObservedFact>,
    tasks: Vec<TaskRuntimeSnapshot>,
) -> PolicyEvaluation {
    let resources = EvaluationResources {
        pools: Vec::new(),
        hosts: vec![HostResourceSnapshot {
            host_id: instance(A, &[]).host_id,
            cpu_available_milli: 1_000,
            gpu_available_milli: 1_000,
            io_available_milli: 1_000,
            host_responsiveness_basis_points: 10_000,
            third_party_pressure_basis_points: 0,
            heavy_dispatch_limit: 1,
            active_heavy_dispatches: 0,
            heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
        }],
    };
    evaluate(
        catalog,
        &evaluation_facts(facts, tasks),
        &resources,
        round_time(),
        11,
    )
    .unwrap_or_else(|error| panic!("the evaluation succeeds: {error}"))
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == task_id && decision.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("{task_id}'s decision on instance A"))
}

fn dispatched(evaluation: &PolicyEvaluation) -> Vec<&str> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| intent.task_id.as_str())
        .collect()
}

fn code_of(reason: &DecisionReason) -> &str {
    reason.outcome.code().as_str()
}

fn codes(decision: &TaskDecision) -> Vec<&str> {
    decision.reasons.iter().map(code_of).collect()
}

// The reason's values are exactly these.
fn assert_values(reason: &DecisionReason, expected: &[(&str, Value)]) {
    let expected = expected
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect::<Map<String, Value>>();
    assert_eq!(
        reason.outcome.values(),
        &expected,
        "{}: {}",
        code_of(reason),
        reason.detail
    );
}

// The game root's `schedule.json`: the daily reset without its scope, excluding the ready times,
// with `requires` naming `reset_exclusions` when `declared`.
fn schedule(declared: bool) -> Value {
    let mut event = reset(&["resource."], Some(&[READY_PREFIX][..]));
    event
        .as_object_mut()
        .expect("the event is an object")
        .shift_remove("scope");
    let requires = if declared {
        json!([RESET_EXCLUSIONS_SUBSYSTEM])
    } else {
        json!([])
    };
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "timeline": [event]
    })
}

// Composes `schedule` in the fixture game root (its bundle lists the one package) for the one
// bound instance, without a Runtime manifest at hand.
fn compose(schedule: &Value) -> Result<StandardPackComposition, StandardPackComposeFailure> {
    let bundle = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": [{
            "package_id": PACKAGE,
            "server": SERVER,
            "entry_task_id": "task",
            "path": format!("packs/{PACKAGE}"),
            "file_count": 1,
            "byte_count": 64
        }]
    }))
    .expect("bundle bytes");
    let root = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle.as_slice()),
            ..StandardPackRootBytes::default()
        },
        GAME,
    )
    .unwrap_or_else(|issue| panic!("the root reads: {issue}"))
    .expect("the root holds bundle.json");
    let schedule = serde_json::to_vec_pretty(schedule).expect("schedule bytes");
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: schedule.as_slice(),
            root: &root,
            maintenance: &[],
        },
    )]);
    let instances = BTreeMap::from([(
        SCHEDULE_INSTANCE.to_owned(),
        StandardPackBinding {
            game_id: GAME.to_owned(),
            server_id: SERVER.to_owned(),
        },
    )]);
    compose_standard_pack_rules(&StandardPackComposeInput {
        instances: &instances,
        games: &games,
        subsystems: None,
        catalog_version: 1,
    })
}

// The one refusal of a composition that must fail: its problem and pointer in the game's file.
fn refused(
    composed: Result<StandardPackComposition, StandardPackComposeFailure>,
) -> (StandardPackProblem, String) {
    let failure = match composed {
        Ok(_) => panic!("the schedule is refused"),
        Err(failure) => failure,
    };
    assert_eq!(failure.issues().len(), 1, "one refusal: {failure}");
    let issue = &failure.issues()[0];
    assert_eq!(issue.game_id.as_deref(), Some(GAME));
    assert_eq!(issue.issue.file, STANDARD_PACK_SCHEDULE_FILE);
    (issue.issue.problem, issue.issue.pointer.clone())
}
