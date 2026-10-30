// SPDX-License-Identifier: AGPL-3.0-only

// SS-5 gate tests (register §1.223; design `rd-1029/design-sch-state.md` §4.6 and the SS-5 entry
// of §5; coordinator work order `rd-1029/brief-ss5-lnoff1.md` SS-5;
// `contracts/scheduling/README.md`, "Self-read fact probe"). A `fact` predicate with
// `on_unknown: probe` names a fact only the task's own package reads: while it is unknown the
// task is admitted once per unknown period (`fact_probe`), so its run reads it; a non-probe
// unknown still asks the maintenance package. A neutral V2 catalog on instance `instance-a` with
// one daily reset that clears `resource.neutral.`, whose latest occurrence is half an hour before
// the round. Deterministic: a fixed evaluation instant, no clock read. Every expected value comes
// from the work order and the design, never from what the code does.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{
    STANDARD_PACK_SCHEDULE_FILE, StandardPackProblem, StandardPackRootBytes,
    read_standard_pack_root,
};
use serde_json::{Map, Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::{
    CandidateEligibility, CatalogCompileFailure, CatalogDiagnosticCode, CatalogDocumentSource,
    CatalogSources, CompiledCatalog, CompletedActivityWindow, DecisionReason, EligibilityState,
    EvaluationFacts, EvaluationResources, EvaluationTime, FACT_REFRESH_TASK_ID,
    FACT_UNKNOWN_PROBE_SUBSYSTEM, FactRefreshInputs, FactRefreshReader, FactRefreshSettings,
    FactValue, HostResourceSnapshot, ObservedFact, PolicyEvaluation, PolicyEvaluationError,
    SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2, SchedulingDecisionState,
    ScopeSelector, StandardPackBinding, StandardPackComposeFailure, StandardPackComposeInput,
    StandardPackComposition, StandardPackGameFiles, TaskDecision, TaskRuntimeSnapshot,
    TaskTerminalState, compile_catalog, compose_standard_pack_rules, evaluate,
    evaluate_with_fact_refresh,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
// The task whose own package reads its gate fact, and a task of lower priority that is due at
// every whole hour: the instance's other work, which runs when the probe task yields.
const PROBE_TASK: &str = "task.probe";
const OTHER: &str = "task.other";
// The self-read key (a remaining count only the probe task's package reads) and a key the
// maintenance package reads.
const REMAINING: &str = "resource.neutral.remaining";
const STOCK: &str = "resource.neutral.stock";
const READER: &str = "fixture.reader";
const RESET_ID: &str = "reset.daily";
// The reset's latest occurrence, half an hour before the round; the next one a day after it.
const RESET_AT: u64 = NOW - 1_800_000;
// Before the reset: a run two hours before the round and its reading a minute later.
const BEFORE: u64 = NOW - 2 * HOUR_MS;
// After the reset: a run ten minutes before the round and its reading a minute later.
const AFTER: u64 = NOW - 600_000;
const SETTLED_AFTER_MS: u64 = 60_000;
const SHORT_COOLDOWN_MS: u64 = 1_000;
const FACT_PROBE: &str = "fact_probe";
const TRIGGER_FALSE: &str = "trigger_false";
const TRIGGER_UNKNOWN: &str = "trigger_unknown";
const REQUESTED: &str = "fact_refresh_requested";
// The standard-pack fixtures: one game root with one package on one server, one bound instance.
const SCHEDULE_INSTANCE: &str = "fixture-instance-a";
const SERVER: &str = "server-a";
const PACKAGE: &str = "fixture.neutral";

// Test 1: the self-read key is unknown (never read, the task never ran): the task is admitted
// once, with `fact_probe {fact_key, unknown_reason}` and no detection suggestion (no reading is
// asked for), ahead of the instance's other work.
#[test]
fn ss5_t1_an_unknown_self_read_key_admits_its_task_with_fact_probe() {
    let evaluation = evaluate_at(&probe_catalog(), Vec::new(), Vec::new());
    assert_eq!(dispatched(&evaluation), [PROBE_TASK]);
    let admitted = decision(&evaluation, PROBE_TASK);
    assert_eq!(admitted.eligibility, EligibilityState::True);
    assert_eq!(code_of(&admitted.reasons[0]), FACT_PROBE);
    assert_values(
        &admitted.reasons[0],
        &[
            ("fact_key", json!(REMAINING)),
            ("unknown_reason", json!("fact_missing")),
        ],
    );
    assert!(
        admitted.detection_suggestions.is_empty(),
        "a probe asks for no reading: {:?}",
        admitted.detection_suggestions
    );
}

// Test 2: the probe run publishes `remaining = 0`; the key is known and its gate false, so the
// task yields to the instance's other work, with no `fact_probe`.
#[test]
fn ss5_t2_a_probe_run_that_publishes_zero_yields() {
    let evaluation = evaluate_at(
        &probe_catalog(),
        vec![remaining(0, AFTER + SETTLED_AFTER_MS, None)],
        vec![ran(PROBE_TASK, AFTER, TaskTerminalState::Succeeded)],
    );
    assert_eq!(dispatched(&evaluation), [OTHER]);
    let yielded = decision(&evaluation, PROBE_TASK);
    assert_eq!(yielded.eligibility, EligibilityState::False);
    assert_eq!(codes(yielded), [TRIGGER_FALSE]);
}

// Test 3: a reset clears the key and admits the task again. The reading of 0 and the run that
// published it are from before the reset's latest occurrence, so the reset removed the reading
// and the run did not serve the new period. Likewise a reading past its own expiry, whose run was
// dispatched before the expiry, under a reset that does not clear the key: `fact_expired`.
#[test]
fn ss5_t3_a_reset_clears_the_key_and_admits_the_task_again() {
    let evaluation = evaluate_at(
        &probe_catalog(),
        vec![remaining(0, BEFORE + SETTLED_AFTER_MS, None)],
        vec![ran(PROBE_TASK, BEFORE, TaskTerminalState::Succeeded)],
    );
    assert_eq!(dispatched(&evaluation), [PROBE_TASK]);
    let admitted = decision(&evaluation, PROBE_TASK);
    assert_eq!(code_of(&admitted.reasons[0]), FACT_PROBE);
    assert_values(
        &admitted.reasons[0],
        &[
            ("fact_key", json!(REMAINING)),
            ("unknown_reason", json!("fact_missing")),
        ],
    );

    let expired = evaluate_at(
        &expiry_catalog(),
        vec![remaining(0, BEFORE + SETTLED_AFTER_MS, Some(NOW - HOUR_MS))],
        vec![ran(PROBE_TASK, BEFORE, TaskTerminalState::Succeeded)],
    );
    assert_eq!(dispatched(&expired), [PROBE_TASK]);
    assert_values(
        &decision(&expired, PROBE_TASK).reasons[0],
        &[
            ("fact_key", json!(REMAINING)),
            ("unknown_reason", json!("fact_expired")),
        ],
    );
}

// Test 4: a successful run dispatched in the key's current unknown period (after the reset) left
// the key unknown: the task is not admitted again, `fact_probe` with `served_by_dispatch_unix_ms`
// then `trigger_false`, no wake, and the instance's other work runs. A failed run served nothing,
// so the same period admits the task. Likewise after an expiry: a successful run dispatched after
// the reading expired serves the period.
#[test]
fn ss5_t4_after_a_run_in_the_same_period_the_task_is_not_admitted_again() {
    let catalog = probe_catalog();
    let served = evaluate_at(
        &catalog,
        Vec::new(),
        vec![ran(PROBE_TASK, AFTER, TaskTerminalState::Succeeded)],
    );
    assert_eq!(dispatched(&served), [OTHER]);
    let waiting = decision(&served, PROBE_TASK);
    assert_eq!(waiting.eligibility, EligibilityState::False);
    assert_eq!(waiting.state, SchedulingDecisionState::Blocked);
    assert_eq!(codes(waiting), [FACT_PROBE, TRIGGER_FALSE]);
    assert_values(
        &waiting.reasons[0],
        &[
            ("fact_key", json!(REMAINING)),
            ("unknown_reason", json!("fact_missing")),
            ("served_by_dispatch_unix_ms", json!(AFTER)),
        ],
    );
    assert_eq!(waiting.next_wake_unix_ms, None, "no wake");

    let failed = evaluate_at(
        &catalog,
        Vec::new(),
        vec![ran(PROBE_TASK, AFTER, TaskTerminalState::Failed)],
    );
    assert_eq!(
        dispatched(&failed),
        [PROBE_TASK],
        "a failed run served nothing"
    );

    let expired_at = NOW - HOUR_MS;
    let after_expiry = NOW - 1_200_000;
    let expired = evaluate_at(
        &expiry_catalog(),
        vec![remaining(0, BEFORE + SETTLED_AFTER_MS, Some(expired_at))],
        vec![ran(PROBE_TASK, after_expiry, TaskTerminalState::Succeeded)],
    );
    assert_eq!(dispatched(&expired), [OTHER]);
    assert_values(
        &decision(&expired, PROBE_TASK).reasons[0],
        &[
            ("fact_key", json!(REMAINING)),
            ("unknown_reason", json!("fact_expired")),
            ("served_by_dispatch_unix_ms", json!(after_expiry)),
        ],
    );
}

// Test 5: a probe on a key the task's package does not read is refused at compile. The catalog
// compiler refuses what no run of the task can read (a scope other than the instance's, a key no
// reading mark writes) and a probe where its truth would not admit the task (under `not`, in the
// feedback stop), each `predicate_uncomputable` at the member; a V1 document is refused at the
// member as `unknown_field`. The composer requires `fact_unknown_probe` of a `schedule.json` whose
// rule writes the member. Whether the key is one of the marks of the rule's own package is the
// resource tools' cross-check (`resource-tooling` `gate_ss5`).
#[test]
fn ss5_t5_a_probe_on_a_key_its_package_does_not_read_is_refused() {
    let accepted = compile(single_task(probe_trigger(REMAINING), outcome_stop()))
        .unwrap_or_else(|failure| panic!("a probe compiles: {:?}", failure.diagnostics()));
    assert_eq!(
        serde_json::to_value(&accepted.catalog().tasks.tasks[0].trigger)
            .expect("the trigger serializes")["on_unknown"],
        json!("probe")
    );

    let mut game_scope = probe_trigger(REMAINING);
    game_scope["scope"] = json!({"kind": "game", "game_id": GAME});
    assert_refused(
        compile(single_task(game_scope, outcome_stop())),
        CatalogDiagnosticCode::PredicateUncomputable,
        "/tasks/0/trigger/on_unknown",
        "a scope no reading writes",
    );
    assert_refused(
        compile(single_task(
            probe_trigger("session.neutral.flag"),
            outcome_stop(),
        )),
        CatalogDiagnosticCode::PredicateUncomputable,
        "/tasks/0/trigger/on_unknown",
        "a key no reading mark writes",
    );
    assert_refused(
        compile(single_task(
            json!({"kind": "not", "predicate": probe_trigger(REMAINING)}),
            outcome_stop(),
        )),
        CatalogDiagnosticCode::PredicateUncomputable,
        "/tasks/0/trigger/predicate/on_unknown",
        "under not",
    );
    assert_refused(
        compile(single_task(hourly(), probe_trigger(REMAINING))),
        CatalogDiagnosticCode::PredicateUncomputable,
        "/tasks/0/feedback_stop/on_unknown",
        "in the feedback stop",
    );
    let mut v1_trigger = probe_trigger(REMAINING);
    v1_trigger["scope"] = json!({"kind": "instance", "instance_id": SCHEDULE_INSTANCE});
    assert_refused(
        compile(example_sources(SCHEDULING_SCHEMA_VERSION, &v1_trigger)),
        CatalogDiagnosticCode::UnknownField,
        "/tasks/0/trigger/on_unknown",
        "a V1 document",
    );

    let composed = compose(&schedule(true))
        .unwrap_or_else(|failure| panic!("the schedule composes: {failure}"));
    assert_eq!(composed.games()[0].requires, [FACT_UNKNOWN_PROBE_SUBSYSTEM]);
    let trigger = serde_json::to_value(&composed.compiled().catalog().tasks.tasks[0].trigger)
        .expect("the trigger serializes");
    assert_eq!(trigger["on_unknown"], json!("probe"));
    assert_eq!(
        trigger["scope"],
        json!({"kind": "instance", "instance_id": SCHEDULE_INSTANCE})
    );
    assert_eq!(
        refused(compose(&schedule(false))),
        (
            StandardPackProblem::ScheduleInvalid,
            "/rules/0/trigger/on_unknown".to_owned()
        ),
        "without the requires entry"
    );
}

// Test 6: a non-probe unknown still asks the maintenance package. The probe task's gate reads
// its self-read key (probe) and a stock the maintenance package reads (no probe), both unknown:
// the task waits (`trigger_unknown`) and the maintenance package is requested for the stock
// alone, never for the probed key, though it marks both. Without `on_unknown` both keys are
// requested, as before. A probe alone is admitted and asks for nothing.
#[test]
fn ss5_t6_a_non_probe_unknown_still_requests_the_maintenance_package() {
    let mixed = refresh_evaluate(&single_catalog(all_of(vec![
        probe_trigger(REMAINING),
        stock_trigger(),
    ])));
    let waiting = decision(&mixed, PROBE_TASK);
    assert_eq!(waiting.eligibility, EligibilityState::Unknown);
    assert_eq!(code_of(&waiting.reasons[0]), FACT_PROBE);
    assert!(
        codes(waiting).contains(&TRIGGER_UNKNOWN),
        "the stock keeps the task waiting: {:?}",
        codes(waiting)
    );
    assert_eq!(requested_keys(&mixed), [STOCK]);

    let mut plain = probe_trigger(REMAINING);
    plain
        .as_object_mut()
        .expect("a predicate object")
        .shift_remove("on_unknown");
    let before = refresh_evaluate(&single_catalog(all_of(vec![plain, stock_trigger()])));
    assert_eq!(requested_keys(&before), [REMAINING, STOCK]);

    let alone = refresh_evaluate(&single_catalog(probe_trigger(REMAINING)));
    assert_eq!(dispatched(&alone), [PROBE_TASK]);
    assert!(
        alone
            .decisions
            .iter()
            .all(|decision| decision.task_id != FACT_REFRESH_TASK_ID),
        "a probe alone asks the maintenance package for nothing"
    );
}

// ---- Fixtures ----

// The probe task (priority 200) and the other task (priority 100) under the reset that clears
// `resource.neutral.`.
fn probe_catalog() -> CompiledCatalog {
    neutral_catalog(
        json!([
            task(PROBE_TASK, 200, probe_trigger(REMAINING), outcome_stop()),
            task(OTHER, 100, hourly(), outcome_stop_of(OTHER)),
        ]),
        reset(&["resource.neutral."]),
    )
}

// The same tasks under a reset that does not clear the key, so only the reading's expiry ends it.
fn expiry_catalog() -> CompiledCatalog {
    neutral_catalog(
        json!([
            task(PROBE_TASK, 200, probe_trigger(REMAINING), outcome_stop()),
            task(OTHER, 100, hourly(), outcome_stop_of(OTHER)),
        ]),
        reset(&["resource.other."]),
    )
}

// The probe task alone, due by `trigger`, under the reset.
fn single_catalog(trigger: Value) -> CompiledCatalog {
    neutral_catalog(
        json!([task(PROBE_TASK, 200, trigger, outcome_stop())]),
        reset(&["resource.neutral."]),
    )
}

// The V2 sources of the probe task alone, with `trigger` and `feedback_stop`.
fn single_task(trigger: Value, feedback_stop: Value) -> CatalogSources {
    neutral_sources(
        json!([task(PROBE_TASK, 200, trigger, feedback_stop)]),
        reset(&["resource.neutral."]),
    )
}

fn neutral_catalog(tasks: Value, event: Value) -> CompiledCatalog {
    compile(neutral_sources(tasks, event)).unwrap_or_else(|failure| {
        panic!("the fixture catalog compiles: {:?}", failure.diagnostics())
    })
}

// A V2 catalog of `tasks` with one activity profile and the one timeline event `event`.
fn neutral_sources(tasks: Value, event: Value) -> CatalogSources {
    let header = json!({
        "catalog_id": "fixture.fact-probe",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-fact-probe"]
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
            format!("memory://fixture-fact-probe/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    let mut profile = profile();
    profile.insert("scope".to_owned(), json!({"kind": "game", "game_id": GAME}));
    CatalogSources {
        tasks: source("tasks", &document("tasks", tasks)),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([event]))),
        selection: None,
    }
}

fn compile(sources: CatalogSources) -> Result<CompiledCatalog, CatalogCompileFailure> {
    compile_catalog(&sources)
}

// The neutral example catalog `catalog-a` declared as `version`, its first task due by
// `trigger` (CF-1's fixture: a V1 document of the same tasks).
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

fn example_document(version: &str, bytes: &[u8]) -> Value {
    let mut document: Value = serde_json::from_slice(bytes).expect("an example document");
    document["schema_version"] = json!(version);
    document
}

// The daily reset of instance A, clearing `invalidated`.
fn reset(invalidated: &[&str]) -> Value {
    json!({
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
    })
}

fn task(task_id: &str, priority: i16, trigger: Value, feedback_stop: Value) -> Value {
    let mut body = task_body(trigger, feedback_stop);
    body.insert("id".to_owned(), json!(task_id));
    body.insert("scope".to_owned(), json!({"kind": "game", "game_id": GAME}));
    body.insert(
        "entrypoint".to_owned(),
        json!({"operation_id": "operation.run"}),
    );
    body.insert(
        "procedure_ref".to_owned(),
        json!(format!("procedure.{task_id}")),
    );
    body.insert("priority".to_owned(), json!(priority));
    Value::Object(body)
}

// A task's members besides its identity, scope, entry point and priority.
fn task_body(trigger: Value, feedback_stop: Value) -> Map<String, Value> {
    let Value::Object(body) = json!({
        "trigger": trigger,
        "feedback_stop": feedback_stop,
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
        "cooldown_ms": SHORT_COOLDOWN_MS,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "loop_budget": {"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    }) else {
        unreachable!("a JSON object")
    };
    body
}

// Stops once the task's own result says it completed (never, in these rounds).
fn outcome_stop_of(task_id: &str) -> Value {
    json!({
        "kind": "outcome",
        "task_id": task_id,
        "outcome_key": "completed",
        "comparison": "eq",
        "value": {"type": "boolean", "value": true}
    })
}

fn outcome_stop() -> Value {
    outcome_stop_of(PROBE_TASK)
}

// Due while the self-read count `key` of instance A is at least 1; probed while it is unknown.
fn probe_trigger(key: &str) -> Value {
    json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": key,
        "comparison": "greater_than_or_equal",
        "value": {"type": "integer", "value": 1},
        "max_age_ms": null,
        "on_unknown": "probe"
    })
}

// Due while instance A's stock, which the maintenance package reads, is at least 1.
fn stock_trigger() -> Value {
    json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": STOCK,
        "comparison": "greater_than_or_equal",
        "value": {"type": "integer", "value": 1},
        "max_age_ms": null
    })
}

fn all_of(predicates: Vec<Value>) -> Value {
    json!({"kind": "all", "predicates": predicates})
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

// Instance A's self-read count `value`, read at `observed_at`, full confidence, with `expires_at`.
fn remaining(value: i64, observed_at: u64, expires_at: Option<u64>) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: REMAINING.to_owned(),
        value: FactValue::Integer(value),
        observed_at_unix_ms: observed_at,
        expires_at_unix_ms: expires_at,
        confidence_milli: 1_000,
    }
}

fn evaluation_facts(facts: Vec<ObservedFact>, tasks: Vec<TaskRuntimeSnapshot>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-fact-probe".to_owned(),
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

fn resources() -> EvaluationResources {
    EvaluationResources {
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
    }
}

// Evaluates instance A at NOW, without a fact refresh.
fn evaluate_at(
    catalog: &CompiledCatalog,
    facts: Vec<ObservedFact>,
    tasks: Vec<TaskRuntimeSnapshot>,
) -> PolicyEvaluation {
    evaluate(
        catalog,
        &evaluation_facts(facts, tasks),
        &resources(),
        round_time(),
        11,
    )
    .unwrap_or_else(|error| panic!("the evaluation succeeds: {error}"))
}

// Evaluates instance A at NOW with no reading and no run, with A's maintenance package, which
// marks both keys; the Runtime admits every candidate.
fn refresh_evaluate(catalog: &CompiledCatalog) -> PolicyEvaluation {
    let refresh = FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: true,
            min_interval_ms: 300_000,
            daily_limit: 96,
            failure_backoff_ms: 600_000,
            max_consecutive_failures: 3,
            expected_duration_ms: crate::FACT_REFRESH_EXPECTED_DURATION_MS,
        },
        readers: vec![FactRefreshReader {
            instance_id: A.to_owned(),
            package_id: READER.to_owned(),
            marked_keys: BTreeSet::from([REMAINING.to_owned(), STOCK.to_owned()]),
            dispatched_today: 0,
            last_failure_code: None,
            problem: None,
        }],
        instance_facts_loaded_at_unix_ms: Some(NOW - DAY_MS),
        instance_holds: BTreeMap::new(),
        idle_closed: BTreeMap::new(),
    };
    evaluate_with_fact_refresh::<PolicyEvaluationError>(
        catalog,
        &evaluation_facts(Vec::new(), Vec::new()),
        &resources(),
        round_time(),
        11,
        &BTreeSet::new(),
        &refresh,
        |_| Ok(CandidateEligibility::Eligible),
    )
    .unwrap_or_else(|error| panic!("the evaluation succeeds: {error}"))
}

// The keys the maintenance package's decision requests, in its reasons' order.
fn requested_keys(evaluation: &PolicyEvaluation) -> Vec<String> {
    evaluation
        .decisions
        .iter()
        .filter(|decision| decision.task_id == FACT_REFRESH_TASK_ID)
        .flat_map(|decision| decision.reasons.iter())
        .filter(|reason| code_of(reason) == REQUESTED)
        .filter_map(|reason| reason.outcome.value("fact_key"))
        .filter_map(|key| key.as_str().map(str::to_owned))
        .collect()
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

// The catalog is refused with `code` at `json_path`.
fn assert_refused(
    compiled: Result<CompiledCatalog, CatalogCompileFailure>,
    code: CatalogDiagnosticCode,
    json_path: &str,
    case: &str,
) {
    let Err(failure) = compiled else {
        panic!("{case}: the catalog is refused");
    };
    assert!(
        failure
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == code && diagnostic.json_path == json_path),
        "{case}: {:?}",
        failure.diagnostics()
    );
}

// The game root's `schedule.json`: one rule running the package, due while its self-read count is
// at least 1 and probed while it is unknown, with `requires` naming `fact_unknown_probe` when
// `declared`, and one profile.
fn schedule(declared: bool) -> Value {
    let mut predicate = probe_trigger(REMAINING);
    predicate
        .as_object_mut()
        .expect("a predicate object")
        .shift_remove("scope");
    let mut rule = task_body(predicate, outcome_stop());
    rule.shift_remove("instance_overrides");
    rule.insert("id".to_owned(), json!(PROBE_TASK));
    rule.insert("package_id".to_owned(), json!(PACKAGE));
    rule.insert("priority".to_owned(), json!(100));
    let requires = if declared {
        json!([FACT_UNKNOWN_PROBE_SUBSYSTEM])
    } else {
        json!([])
    };
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": [rule],
        "activity": [profile()]
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
