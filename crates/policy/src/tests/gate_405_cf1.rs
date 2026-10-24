// SPDX-License-Identifier: AGPL-3.0-only

// SCH-M slice CF-1 gate tests (frozen model `rd-1024/sched-design/model.md` §3.3 D-8 and its §6
// row CF-1; `contracts/scheduling/README.md`, "Condition edges"): the edge members
// `dependency_completed.since_own_run` and `fact_time_reached.once_per_reading`. A neutral V2
// catalog on instance `instance-a`. For `since_own_run`: the predecessor `task.lead` (an hourly
// clock, held by its two-hour cooldown after each of its runs, so it never competes) and its
// successor `task.next`, due on the lead's success, with a ten-minute cooldown. For
// `once_per_reading`: the single task `task.slot`, due at the time the instance fact
// `resource.neutral.slot_ready_at` holds plus one minute. Every expected value comes from D-8 and
// the §6 test row, never from what the code does.

use serde_json::{Map, Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::{
    CatalogCompileFailure, CatalogDiagnosticCode, CatalogDocumentSource, CatalogSources,
    CompiledCatalog, DecisionReason, EvaluationFacts, EvaluationResources, EvaluationTime,
    FactValue, HostResourceSnapshot, ObservedFact, PolicyEvaluation, SCHEDULING_SCHEMA_VERSION,
    SCHEDULING_SCHEMA_VERSION_V2, SchedulingDecisionState, ScopeSelector, TaskDecision,
    TaskRuntimeSnapshot, TaskTerminalState, compile_catalog, evaluate,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const LEAD: &str = "task.lead";
const NEXT: &str = "task.next";
const SLOT: &str = "task.slot";
const SLOT_READY_AT: &str = "resource.neutral.slot_ready_at";
const OFFSET_MS: i64 = 60_000;
const MAX_AGE_MS: u64 = 7_200_000;
const NEXT_COOLDOWN_MS: u64 = 600_000;
const ALREADY_SERVED: &str = "dependency_already_served";
const FACT_TIME_SERVED: &str = "fact_time_served";
const TRIGGER_FALSE: &str = "trigger_false";
const COOLDOWN: &str = "task_cooldown_active";
// The lead's first success 50 minutes before the round, the successor's run 40 minutes before it
// (its cooldown long over), and the lead's second success 20 minutes before it.
const LEAD_FIRST: u64 = NOW - 3_000_000;
const NEXT_RAN: u64 = NOW - 2_400_000;
const LEAD_AGAIN: u64 = NOW - 1_200_000;
// The golden identity of the neutral example catalog declared V2 whose task trigger also names
// `dependency_completed` and `fact_time_reached` without the CF-1 members. Computed apart from the
// code under test, as R7-T6's golden was: the canonical JSON (keys in UTF-16 order, no whitespace)
// of the four documents of `contracts/scheduling/examples/catalog-a` under `activity`, `pools`,
// `tasks` and `timeline`, each declared V2, every timeline event with validity {from 0, until
// null}, the trigger as `edge_trigger` writes it, hashed with SHA-256 by a Python script that
// first reproduces R7-T6's golden of the plain V2 catalog.
const ABSENT_MEMBERS_HASH: &str =
    "sha256:08341a021385026921a4bf5e2dca7d8fc579e3f29934a643b46ede6741fed7f8";

// D-8, row item 1: a predecessor that succeeded once makes its successor due exactly once. Before
// the successor's own run it is dispatched; after its own successful run, dispatched after that
// success, it is false with `dependency_already_served {task_id, predecessor_task_id}` and then
// `trigger_false`, although its cooldown is long over.
#[test]
fn cf1_a_predecessor_success_makes_its_successor_due_exactly_once() {
    let catalog = chain_catalog();

    let due = evaluate_at(&catalog, Vec::new(), vec![lead(LEAD_FIRST)]);
    assert_eq!(dispatched(&due), [NEXT], "never run: due");

    let served = evaluate_at(
        &catalog,
        Vec::new(),
        vec![
            lead(LEAD_FIRST),
            next(NEXT_RAN, TaskTerminalState::Succeeded),
        ],
    );
    assert!(
        served.dispatch_intents.is_empty(),
        "run once since the lead's success: not due again: {:#?}",
        served.dispatch_intents
    );
    let waiting = decision(&served, NEXT);
    assert_eq!(waiting.state, SchedulingDecisionState::Blocked);
    assert_eq!(codes(waiting), [ALREADY_SERVED, TRIGGER_FALSE]);
    assert_values(
        &waiting.reasons[0],
        &[
            ("task_id", json!(NEXT)),
            ("predecessor_task_id", json!(LEAD)),
        ],
    );
}

// D-8, row item 2: the predecessor succeeds again, dispatched after the successor's run: the
// successor is due again.
#[test]
fn cf1_a_new_predecessor_success_makes_the_successor_due_again() {
    let evaluation = evaluate_at(
        &chain_catalog(),
        Vec::new(),
        vec![
            lead(LEAD_AGAIN),
            next(NEXT_RAN, TaskTerminalState::Succeeded),
        ],
    );
    assert_eq!(dispatched(&evaluation), [NEXT]);
}

// D-8, row item 3: a successor whose own run after the predecessor's success failed stays due,
// bounded by its cooldown: within it the candidate is deferred with `task_cooldown_active` and
// wakes at its end, never `dependency_already_served`; after it the successor is dispatched.
#[test]
fn cf1_a_failed_successor_stays_due_under_its_cooldown() {
    let catalog = chain_catalog();
    let failed_at = NOW - 300_000;
    let within = evaluate_at(
        &catalog,
        Vec::new(),
        vec![lead(LEAD_FIRST), next(failed_at, TaskTerminalState::Failed)],
    );
    assert!(within.dispatch_intents.is_empty());
    let waiting = decision(&within, NEXT);
    assert_eq!(waiting.state, SchedulingDecisionState::Deferred);
    assert_eq!(last_code(waiting), COOLDOWN);
    assert!(!codes(waiting).contains(&ALREADY_SERVED));
    assert_eq!(
        waiting.next_wake_unix_ms,
        Some(failed_at + NEXT_COOLDOWN_MS)
    );

    let after = evaluate_at(
        &catalog,
        Vec::new(),
        vec![lead(LEAD_FIRST), next(NEXT_RAN, TaskTerminalState::Failed)],
    );
    assert_eq!(dispatched(&after), [NEXT], "past its cooldown: dispatched");
}

// D-8, row item 4: with `once_per_reading`, a reached time T counts as served once the task's
// latest run succeeded and was dispatched at or after max(T + offset, the reading's
// `observed_at`): the predicate is then false with `fact_time_served {fact_key,
// served_by_dispatch_unix_ms}` and then `trigger_false`, and gives no wake. A newer reading,
// observed after that dispatch, makes it true again.
#[test]
fn cf1_a_served_reading_is_false_until_a_newer_reading() {
    let catalog = slot_catalog();
    let observed_at = NOW - 1_800_000;
    let held = NOW - 1_200_000;
    let reading = || vec![slot_ready_at(held, observed_at)];

    let unserved = evaluate_at(&catalog, reading(), Vec::new());
    assert_eq!(
        dispatched(&unserved),
        [SLOT],
        "a reached time not yet served"
    );

    let dispatched_at = NOW - 600_000;
    let served = evaluate_at(&catalog, reading(), vec![slot_ran(dispatched_at)]);
    assert!(
        served.dispatch_intents.is_empty(),
        "served: {:#?}",
        served.dispatch_intents
    );
    let waiting = decision(&served, SLOT);
    assert_eq!(waiting.state, SchedulingDecisionState::Blocked);
    assert_eq!(codes(waiting), [FACT_TIME_SERVED, TRIGGER_FALSE]);
    assert_values(
        &waiting.reasons[0],
        &[
            ("fact_key", json!(SLOT_READY_AT)),
            ("served_by_dispatch_unix_ms", json!(dispatched_at)),
        ],
    );
    assert_eq!(waiting.next_wake_unix_ms, None, "no wake");

    // A newer reading, observed after the dispatch, whose time plus the offset has come.
    let newer = evaluate_at(
        &catalog,
        vec![slot_ready_at(NOW - 60_000, NOW - 120_000)],
        vec![slot_ran(dispatched_at)],
    );
    assert_eq!(dispatched(&newer), [SLOT], "a newer reading: due again");
}

// D-8 and the row's identity item: a catalog that names `dependency_completed` and
// `fact_time_reached` without the members keeps its bytes and its hash (the golden above); the
// same catalog with both members has another hash, the same on every compile, and its trigger
// re-serializes as it was written.
#[test]
fn cf1_absent_members_keep_the_catalog_bytes_and_hash() {
    let written = edge_trigger(None);
    let plain = compile(example_sources(SCHEDULING_SCHEMA_VERSION_V2, &written))
        .unwrap_or_else(|failure| panic!("the catalog compiles: {:?}", failure.diagnostics()));
    assert_eq!(plain.catalog_hash(), ABSENT_MEMBERS_HASH);
    assert_eq!(trigger_of(&plain), written, "the trigger keeps its bytes");

    let with_members = edge_trigger(Some(true));
    let sources = || example_sources(SCHEDULING_SCHEMA_VERSION_V2, &with_members);
    let edged = compile(sources()).unwrap_or_else(|failure| {
        panic!(
            "the catalog with both members compiles: {:?}",
            failure.diagnostics()
        )
    });
    assert_ne!(edged.catalog_hash(), plain.catalog_hash());
    assert_eq!(
        compile(sources())
            .expect("the catalog with both members compiles again")
            .catalog_hash(),
        edged.catalog_hash()
    );
    assert_eq!(trigger_of(&edged), with_members);
}

// D-8: a V1 catalog that writes either member, with either value, is refused `unknown_field`;
// `since_own_run` at the member.
#[test]
fn cf1_a_v1_catalog_naming_either_member_is_refused() {
    for value in [true, false] {
        let trigger = json!({
            "kind": "any",
            "predicates": [
                hourly(),
                {
                    "kind": "dependency_completed",
                    "task_id": "fixture.observe",
                    "terminal_states": ["succeeded"],
                    "since_own_run": value
                }
            ]
        });
        assert!(
            refused_at(
                compile(example_sources(SCHEDULING_SCHEMA_VERSION, &trigger)),
                CatalogDiagnosticCode::UnknownField,
                Some("/tasks/0/trigger/predicates/1/since_own_run"),
            ),
            "since_own_run {value}"
        );
        let trigger = json!({
            "kind": "fact_time_reached",
            "scope": {"kind": "instance", "instance_id": "fixture-instance-a"},
            "fact_key": "resource.primary_ready_at",
            "offset_ms": 0,
            "max_age_ms": MAX_AGE_MS,
            "once_per_reading": value
        });
        assert!(
            refused_at(
                compile(example_sources(SCHEDULING_SCHEMA_VERSION, &trigger)),
                CatalogDiagnosticCode::UnknownField,
                None,
            ),
            "once_per_reading {value}"
        );
    }
}

// ---- Fixtures ----

// The lead (hourly clock, two-hour cooldown) and its successor, due by the edge on its success.
fn chain_catalog() -> CompiledCatalog {
    let edge = json!({
        "kind": "dependency_completed",
        "task_id": LEAD,
        "terminal_states": ["succeeded"],
        "since_own_run": true
    });
    neutral_catalog(json!([
        task(LEAD, 200, hourly(), 2 * HOUR_MS),
        task(NEXT, 100, edge, NEXT_COOLDOWN_MS),
    ]))
}

// The slot task, due once per reading of its completion time.
fn slot_catalog() -> CompiledCatalog {
    let trigger = json!({
        "kind": "fact_time_reached",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": SLOT_READY_AT,
        "offset_ms": OFFSET_MS,
        "max_age_ms": MAX_AGE_MS,
        "once_per_reading": true
    });
    neutral_catalog(json!([task(SLOT, 100, trigger, 1_000)]))
}

fn neutral_catalog(tasks: Value) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.condition-edges",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-condition-edges"]
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
            format!("memory://fixture-condition-edges/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    compile(CatalogSources {
        tasks: source("tasks", &document("tasks", tasks)),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([profile()]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
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

// One profile for the whole game, active all day on every weekday.
fn profile() -> Value {
    json!({
        "id": "fixture-activity",
        "scope": {"kind": "game", "game_id": GAME},
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
    })
}

// The lead's latest run on the instance, dispatched at `at` and succeeded.
fn lead(at: u64) -> TaskRuntimeSnapshot {
    ran(LEAD, at, TaskTerminalState::Succeeded)
}

// The successor's latest run on the instance, dispatched at `at`.
fn next(at: u64, terminal_state: TaskTerminalState) -> TaskRuntimeSnapshot {
    ran(NEXT, at, terminal_state)
}

// The slot task's latest run on the instance, dispatched at `at` and succeeded.
fn slot_ran(at: u64) -> TaskRuntimeSnapshot {
    ran(SLOT, at, TaskTerminalState::Succeeded)
}

fn ran(task_id: &str, at: u64, terminal_state: TaskTerminalState) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: task_id.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(at),
        eligible_since_unix_ms: None,
        terminal_state: Some(terminal_state),
        completed_window: None,
        last_duration_ms: None,
        failure_streak: None,
    }
}

// The instance's completion time `held`, read at `observed_at`, full confidence, no own expiry.
fn slot_ready_at(held: u64, observed_at: u64) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: SLOT_READY_AT.to_owned(),
        value: FactValue::TimestampMs(held),
        observed_at_unix_ms: observed_at,
        expires_at_unix_ms: None,
        confidence_milli: 1_000,
    }
}

// The example task's trigger: its clock and its fact, plus `dependency_completed` and
// `fact_time_reached`, with both CF-1 members set to `members` when given.
fn edge_trigger(members: Option<bool>) -> Value {
    let tasks: Value = serde_json::from_slice(include_bytes!(
        "../../../../contracts/scheduling/examples/catalog-a/tasks.json"
    ))
    .expect("the example tasks document");
    let mut trigger = tasks["tasks"][0]["trigger"].clone();
    let mut dependency = json!({
        "kind": "dependency_completed",
        "task_id": "fixture.observe",
        "terminal_states": ["succeeded"]
    });
    let mut fact_time = json!({
        "kind": "fact_time_reached",
        "scope": {"kind": "instance", "instance_id": "fixture-instance-a"},
        "fact_key": "resource.primary_ready_at",
        "offset_ms": 0,
        "max_age_ms": MAX_AGE_MS
    });
    if let Some(members) = members {
        dependency["since_own_run"] = json!(members);
        fact_time["once_per_reading"] = json!(members);
    }
    let predicates = trigger["predicates"]
        .as_array_mut()
        .expect("the example trigger is an any");
    predicates.push(dependency);
    predicates.push(fact_time);
    trigger
}

// The four neutral example documents declared as `version` (a V2 timeline event gets validity
// {from 0, until null}), with `trigger` as the trigger of their one task.
fn example_sources(version: &str, trigger: &Value) -> CatalogSources {
    let mut tasks = example_document(
        version,
        include_bytes!("../../../../contracts/scheduling/examples/catalog-a/tasks.json"),
    );
    tasks["tasks"][0]["trigger"] = trigger.clone();
    let source = |name: &str, document: &Value| {
        CatalogDocumentSource::new(
            format!("memory://catalog-a/{name}.json"),
            serde_json::to_vec(document).expect("document bytes"),
        )
    };
    CatalogSources {
        tasks: source("tasks", &tasks),
        pools: source(
            "pools",
            &example_document(
                version,
                include_bytes!("../../../../contracts/scheduling/examples/catalog-a/pools.json"),
            ),
        ),
        activity: source(
            "activity",
            &example_document(
                version,
                include_bytes!("../../../../contracts/scheduling/examples/catalog-a/activity.json"),
            ),
        ),
        timeline: source(
            "timeline",
            &example_document(
                version,
                include_bytes!("../../../../contracts/scheduling/examples/catalog-a/timeline.json"),
            ),
        ),
        selection: None,
    }
}

// One neutral example document declared as `version`; a V2 timeline event gets validity
// {from 0, until null}.
fn example_document(version: &str, bytes: &[u8]) -> Value {
    let mut document: Value = serde_json::from_slice(bytes).expect("an example document");
    document["schema_version"] = json!(version);
    if version == SCHEDULING_SCHEMA_VERSION_V2
        && let Some(events) = document.get_mut("events").and_then(Value::as_array_mut)
    {
        for event in events {
            event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
        }
    }
    document
}

fn compile(sources: CatalogSources) -> Result<CompiledCatalog, CatalogCompileFailure> {
    compile_catalog(&sources)
}

fn trigger_of(catalog: &CompiledCatalog) -> Value {
    serde_json::to_value(&catalog.catalog().tasks.tasks[0].trigger).expect("the trigger serializes")
}

// Whether the catalog is refused with `code`, at `json_path` when one is given.
fn refused_at(
    compiled: Result<CompiledCatalog, CatalogCompileFailure>,
    code: CatalogDiagnosticCode,
    json_path: Option<&str>,
) -> bool {
    let Err(failure) = compiled else {
        return false;
    };
    failure.diagnostics().iter().any(|diagnostic| {
        diagnostic.code == code
            && json_path.is_none_or(|json_path| diagnostic.json_path == json_path)
    })
}

// Evaluates instance A at NOW; the host has room for one heavy dispatch.
fn evaluate_at(
    catalog: &CompiledCatalog,
    facts: Vec<ObservedFact>,
    tasks: Vec<TaskRuntimeSnapshot>,
) -> PolicyEvaluation {
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-condition-edges".to_owned(),
        facts,
        outcomes: Vec::new(),
        tasks,
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    };
    let resources = EvaluationResources {
        pools: Vec::new(),
        hosts: vec![HostResourceSnapshot {
            host_id: format!("host.{A}"),
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
        &facts,
        &resources,
        EvaluationTime {
            unix_ms: NOW,
            monotonic_ms: NOW,
        },
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

fn last_code(decision: &TaskDecision) -> &str {
    decision.reasons.last().map_or("", code_of)
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
