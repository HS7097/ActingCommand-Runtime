// SPDX-License-Identifier: AGPL-3.0-only

// #386 slice CF-4 gate tests (`contracts/scheduling/README.md`, "Fact references"; Alice 10-26: a
// red line is the current cap less a margin, never a fixed number): a V2 `fact` predicate whose
// `value` is another fact of its own scope plus an integer offset, `{"type": "fact", "fact_key",
// "offset"}`. A neutral V2 catalog on instance `instance-a` whose one task `task.amount` is due
// while `resource.neutral.amount` >= `resource.neutral.capacity` - 1000. Every expected value
// comes from the work order and the contract text, never from what the code does.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{
    STANDARD_PACK_SCHEDULE_FILE, StandardPackProblem, StandardPackRootBytes,
    read_standard_pack_root,
};
use serde_json::{Map, Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::{
    CatalogCompileFailure, CatalogDiagnosticCode, CatalogDocumentSource, CatalogSources,
    CompiledCatalog, DecisionReason, DetectionSuggestion, EligibilityState, EvaluationFacts,
    EvaluationResources, EvaluationTime, FACT_REFERENCE_SUBSYSTEM, FactValue, HostResourceSnapshot,
    ObservedFact, PolicyEvaluation, SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2,
    SchedulingDecisionState, ScopeSelector, StandardPackBinding, StandardPackComposeFailure,
    StandardPackComposeInput, StandardPackComposition, StandardPackGameFiles, TaskDecision,
    compile_catalog, compose_standard_pack_rules, evaluate,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const TASK: &str = "task.amount";
const AMOUNT: &str = "resource.neutral.amount";
const CAPACITY: &str = "resource.neutral.capacity";
const MARGIN: i64 = -1_000;
const CAPACITY_VALUE: i64 = 5_000;
const MAX_AGE_MS: u64 = 600_000;
// The amount read one minute before the round, the capacity two minutes before it: the
// capacity's freshness ends first.
const AMOUNT_OBSERVED: u64 = NOW - 60_000;
const CAPACITY_OBSERVED: u64 = NOW - 120_000;
const REFERENCE_UNAVAILABLE: &str = "fact_reference_unavailable";
const TRIGGER_FALSE: &str = "trigger_false";
const TRIGGER_UNKNOWN: &str = "trigger_unknown";
// The golden identity of the neutral example catalog `contracts/scheduling/examples/catalog-a`
// declared V2 (its trigger holds a `fact` predicate with a typed value), computed before CF-4
// apart from the code under test (Workflow #361 R7, row R7-T6).
const PLAIN_V2_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";
// The standard-pack fixtures: one game root with one package on one server, one bound instance.
const SCHEDULE_INSTANCE: &str = "fixture-instance-a";
const SERVER: &str = "server-a";
const PACKAGE: &str = "fixture.amount";
const PRESENT_SUBSYSTEM: &str = "standard_pack_schedule";

// Work order, `a >= b - 1000`: true when a = b - 1000; false when a = b - 1001, with only
// `trigger_false`. Both facts obey the predicate's `max_age_ms`: the false predicate wakes one
// millisecond after the earlier end of the two readings' freshness, here the capacity's.
#[test]
fn cf4_a_fact_at_least_its_reference_less_1000_is_true_at_the_bound_and_false_one_below() {
    let catalog = neutral_catalog(reference_predicate(A));

    let at_bound = evaluate_at(
        &catalog,
        vec![amount(CAPACITY_VALUE - 1_000), capacity(CAPACITY_VALUE)],
    );
    assert_eq!(dispatched(&at_bound), [TASK], "a = b - 1000 is true");

    let below = evaluate_at(
        &catalog,
        vec![amount(CAPACITY_VALUE - 1_001), capacity(CAPACITY_VALUE)],
    );
    assert!(
        below.dispatch_intents.is_empty(),
        "a = b - 1001 is false: {:#?}",
        below.dispatch_intents
    );
    let waiting = decision(&below, TASK);
    assert_eq!(waiting.state, SchedulingDecisionState::Blocked);
    assert_eq!(waiting.eligibility, EligibilityState::False);
    assert_eq!(codes(waiting), [TRIGGER_FALSE]);
    assert!(waiting.detection_suggestions.is_empty());
    assert_eq!(
        waiting.next_wake_unix_ms,
        Some(CAPACITY_OBSERVED + MAX_AGE_MS + 1),
        "the referenced fact's freshness bounds the predicate's"
    );
}

// Work order: a missing or a stale reference is false (not unknown) with its reason,
// `fact_reference_unavailable {fact_key, unknown_reason}`, then `trigger_false`; no fact refresh
// is asked for and no wake is given. A reference that holds no integer (the non-integer pair at
// evaluation) is the same reason with `type_mismatch`; the predicate's own fact holding no
// integer is Unknown with `type_mismatch`, as for any fact of a kind its predicate does not
// compare.
#[test]
fn cf4_a_reference_that_gives_no_integer_is_false_with_its_reason() {
    let catalog = neutral_catalog(reference_predicate(A));
    let stale = NOW - MAX_AGE_MS - 1;
    let text = |fact_key: &str, observed_at: u64| ObservedFact {
        value: FactValue::String("text".to_owned()),
        ..integer_fact(fact_key, 0, observed_at)
    };
    for (case, facts, unknown_reason) in [
        ("missing", vec![amount(4_000)], "fact_missing"),
        (
            "stale",
            vec![amount(4_000), integer_fact(CAPACITY, CAPACITY_VALUE, stale)],
            "fact_stale",
        ),
        (
            "no integer",
            vec![amount(4_000), text(CAPACITY, CAPACITY_OBSERVED)],
            "type_mismatch",
        ),
    ] {
        let evaluation = evaluate_at(&catalog, facts);
        assert!(evaluation.dispatch_intents.is_empty(), "{case}");
        let waiting = decision(&evaluation, TASK);
        assert_eq!(waiting.state, SchedulingDecisionState::Blocked, "{case}");
        assert_eq!(
            waiting.eligibility,
            EligibilityState::False,
            "{case}: false, not unknown"
        );
        assert_eq!(
            codes(waiting),
            [REFERENCE_UNAVAILABLE, TRIGGER_FALSE],
            "{case}"
        );
        assert_values(
            &waiting.reasons[0],
            &[
                ("fact_key", json!(CAPACITY)),
                ("unknown_reason", json!(unknown_reason)),
            ],
        );
        assert!(
            waiting.detection_suggestions.is_empty(),
            "{case}: no fact refresh"
        );
        assert_eq!(waiting.next_wake_unix_ms, None, "{case}: no wake");
    }

    let evaluation = evaluate_at(
        &catalog,
        vec![text(AMOUNT, AMOUNT_OBSERVED), capacity(CAPACITY_VALUE)],
    );
    let unknown = decision(&evaluation, TASK);
    assert_eq!(unknown.eligibility, EligibilityState::Unknown);
    assert_eq!(
        unknown.detection_suggestions,
        [suggestion(AMOUNT, "type_mismatch")]
    );
    assert!(!codes(unknown).contains(&REFERENCE_UNAVAILABLE));
}

// Work order: a document without the new form keeps its catalog hash (the golden computed before
// CF-4) and its bytes, and a typed value evaluates exactly as `fact` always did: true at its
// threshold; false one below with only `trigger_false`, woken by its own fact's freshness alone
// though the other fact is present; Unknown with a detection suggestion when its fact is missing.
#[test]
fn cf4_a_document_without_a_reference_keeps_its_hash_bytes_and_evaluation() {
    let written = example_trigger();
    let plain = compile(example_sources(SCHEDULING_SCHEMA_VERSION_V2, &written))
        .unwrap_or_else(|failure| panic!("the catalog compiles: {:?}", failure.diagnostics()));
    assert_eq!(plain.catalog_hash(), PLAIN_V2_HASH);
    assert_eq!(trigger_of(&plain), written, "the trigger keeps its bytes");

    let literal = neutral_catalog(json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": AMOUNT,
        "comparison": "greater_than_or_equal",
        "value": {"type": "integer", "value": CAPACITY_VALUE - 1_000},
        "max_age_ms": MAX_AGE_MS
    }));
    let at_threshold = evaluate_at(
        &literal,
        vec![amount(CAPACITY_VALUE - 1_000), capacity(CAPACITY_VALUE)],
    );
    assert_eq!(dispatched(&at_threshold), [TASK]);

    let below = evaluate_at(
        &literal,
        vec![amount(CAPACITY_VALUE - 1_001), capacity(CAPACITY_VALUE)],
    );
    let waiting = decision(&below, TASK);
    assert_eq!(codes(waiting), [TRIGGER_FALSE]);
    assert_eq!(
        waiting.next_wake_unix_ms,
        Some(AMOUNT_OBSERVED + MAX_AGE_MS + 1),
        "a typed value reads no other fact"
    );

    let missing = evaluate_at(&literal, vec![capacity(CAPACITY_VALUE)]);
    let unknown = decision(&missing, TASK);
    assert_eq!(unknown.eligibility, EligibilityState::Unknown);
    assert_eq!(
        unknown.detection_suggestions,
        [suggestion(AMOUNT, "fact_missing")]
    );
    assert!(codes(unknown).contains(&TRIGGER_UNKNOWN));
    assert!(!codes(unknown).contains(&REFERENCE_UNAVAILABLE));
}

// Work order, validation: a reference refused with its pointer. V1 knows no reference kind
// (`unknown_field` at `value/type`); an unknown kind and a non-integer offset are refused by the
// reader (`type_mismatch`, inside the predicate); `contains` is no integer comparison
// (`predicate_uncomputable` at `comparison`); an offset beyond the canonical integers is
// `limit_exceeded` at `value/offset`; a reference key that is no identifier is `type_mismatch`
// at `value/fact_key`; a reference that names a scope, another one here, is a cross-scope
// reference (`unknown_field` at `value/scope`).
#[test]
fn cf4_an_unknown_kind_a_non_integer_pair_or_a_cross_scope_reference_is_refused() {
    let base =
        || example_reference(json!({"type": "fact", "fact_key": CAPACITY, "offset": MARGIN}));
    let accepted = compile(example_sources(SCHEDULING_SCHEMA_VERSION_V2, &base()));
    assert!(
        accepted.is_ok(),
        "the V2 reference compiles: {:?}",
        accepted.err().map(|failure| failure.diagnostics().to_vec())
    );

    assert!(
        refused_at(
            compile(example_sources(SCHEDULING_SCHEMA_VERSION, &base())),
            CatalogDiagnosticCode::UnknownField,
            "/tasks/0/trigger/value/type",
        ),
        "V1"
    );

    let mut unknown_kind = base();
    unknown_kind["value"]["type"] = json!("fact_sum");
    let mut fractional = base();
    fractional["value"]["offset"] = json!(1.5);
    for (case, trigger) in [
        ("unknown kind", unknown_kind),
        ("fractional offset", fractional),
    ] {
        assert!(
            refused_within(
                compile(example_sources(SCHEDULING_SCHEMA_VERSION_V2, &trigger)),
                CatalogDiagnosticCode::TypeMismatch,
                "/tasks/0/trigger",
            ),
            "{case}"
        );
    }

    let mut contains = base();
    contains["comparison"] = json!("contains");
    let mut beyond = base();
    beyond["value"]["offset"] = json!(9_007_199_254_740_992_i64);
    let mut key = base();
    key["value"]["fact_key"] = json!("Not A Key");
    let mut cross_scope = base();
    cross_scope["value"]["scope"] =
        json!({"kind": "instance", "instance_id": "fixture-instance-b"});
    for (case, trigger, code, pointer) in [
        (
            "contains",
            contains,
            CatalogDiagnosticCode::PredicateUncomputable,
            "/tasks/0/trigger/comparison",
        ),
        (
            "offset beyond the canonical integers",
            beyond,
            CatalogDiagnosticCode::LimitExceeded,
            "/tasks/0/trigger/value/offset",
        ),
        (
            "reference key",
            key,
            CatalogDiagnosticCode::TypeMismatch,
            "/tasks/0/trigger/value/fact_key",
        ),
        (
            "cross-scope reference",
            cross_scope,
            CatalogDiagnosticCode::UnknownField,
            "/tasks/0/trigger/value/scope",
        ),
    ] {
        assert!(
            refused_at(
                compile(example_sources(SCHEDULING_SCHEMA_VERSION_V2, &trigger)),
                code,
                pointer,
            ),
            "{case}"
        );
    }
}

// Work order, composer: a `schedule.json` whose rule writes a reference composes when `requires`
// names `fact_reference`, with or without a Runtime manifest at hand, and keeps the reference as
// written while the predicate node gets the instance's scope. Without the entry it is refused,
// `schedule_invalid` at the reference; a Runtime that does not name the subsystem refuses the
// file at its `requires` entry (`schedule_requires_missing`) instead of misreading it.
#[test]
fn cf4_the_composer_accepts_a_reference_its_requires_names_and_refuses_one_it_does_not() {
    let present = BTreeSet::from([
        PRESENT_SUBSYSTEM.to_owned(),
        FACT_REFERENCE_SUBSYSTEM.to_owned(),
    ]);
    for subsystems in [Some(&present), None] {
        let composition = compose(&schedule(true), subsystems)
            .unwrap_or_else(|failure| panic!("the schedule composes: {failure}"));
        assert_eq!(composition.games()[0].requires, [FACT_REFERENCE_SUBSYSTEM]);
        let trigger =
            serde_json::to_value(&composition.compiled().catalog().tasks.tasks[0].trigger)
                .expect("the composed trigger serializes");
        assert_eq!(
            trigger["predicates"][1]["value"],
            json!({"type": "fact", "fact_key": CAPACITY, "offset": MARGIN}),
            "the reference is composed as written"
        );
        assert_eq!(
            trigger["predicates"][1]["scope"],
            json!({"kind": "instance", "instance_id": SCHEDULE_INSTANCE})
        );

        assert_eq!(
            refused(compose(&schedule(false), subsystems)),
            (
                StandardPackProblem::ScheduleInvalid,
                "/rules/0/trigger/predicates/1/value".to_owned()
            ),
            "without the requires entry"
        );
    }

    let older = BTreeSet::from([PRESENT_SUBSYSTEM.to_owned()]);
    assert_eq!(
        refused(compose(&schedule(true), Some(&older))),
        (
            StandardPackProblem::ScheduleRequiresMissing,
            "/requires/0".to_owned()
        ),
        "a Runtime that does not read references"
    );
}

// ---- Fixtures ----

// `a >= b - 1000` on instance `alias`.
fn reference_predicate(alias: &str) -> Value {
    json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": alias},
        "fact_key": AMOUNT,
        "comparison": "greater_than_or_equal",
        "value": {"type": "fact", "fact_key": CAPACITY, "offset": MARGIN},
        "max_age_ms": MAX_AGE_MS
    })
}

fn neutral_catalog(trigger: Value) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.fact-reference",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-fact-reference"]
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
            format!("memory://fixture-fact-reference/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    let mut task = task_body(trigger);
    task.insert("id".to_owned(), json!(TASK));
    task.insert("scope".to_owned(), json!({"kind": "game", "game_id": GAME}));
    task.insert(
        "entrypoint".to_owned(),
        json!({"operation_id": "operation.run"}),
    );
    task.insert("procedure_ref".to_owned(), json!("procedure.amount"));
    let mut activity = profile();
    activity.insert("scope".to_owned(), json!({"kind": "game", "game_id": GAME}));
    compile(CatalogSources {
        tasks: source("tasks", &document("tasks", json!([task]))),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([activity]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

// A task's members besides its id, scope, entrypoint and procedure: `trigger`, a feedback stop on
// its own outcome, no effects, a one-second cooldown.
fn task_body(trigger: Value) -> Map<String, Value> {
    let Value::Object(body) = json!({
        "priority": 100,
        "trigger": trigger,
        "feedback_stop": {
            "kind": "outcome",
            "task_id": TASK,
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
        "loop_budget": {"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    }) else {
        unreachable!("a JSON object")
    };
    body
}

// One profile without its scope, active all day on every weekday.
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

// An integer fact of instance A, read at `observed_at`, full confidence, no own expiry.
fn integer_fact(fact_key: &str, value: i64, observed_at: u64) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: fact_key.to_owned(),
        value: FactValue::Integer(value),
        observed_at_unix_ms: observed_at,
        expires_at_unix_ms: None,
        confidence_milli: 1_000,
    }
}

fn amount(value: i64) -> ObservedFact {
    integer_fact(AMOUNT, value, AMOUNT_OBSERVED)
}

fn capacity(value: i64) -> ObservedFact {
    integer_fact(CAPACITY, value, CAPACITY_OBSERVED)
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

// catalog-a's task trigger, as written.
fn example_trigger() -> Value {
    let tasks: Value = serde_json::from_slice(include_bytes!(
        "../../../../contracts/scheduling/examples/catalog-a/tasks.json"
    ))
    .expect("the example tasks document");
    tasks["tasks"][0]["trigger"].clone()
}

// catalog-a's `fact` predicate with `value` instead of its typed value, as the task's trigger.
fn example_reference(value: Value) -> Value {
    let mut predicate = example_trigger()["predicates"][1].clone();
    assert_eq!(predicate["kind"], "fact", "catalog-a's second predicate");
    predicate["value"] = value;
    predicate
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

// Whether the catalog is refused with `code` at `json_path`.
fn refused_at(
    compiled: Result<CompiledCatalog, CatalogCompileFailure>,
    code: CatalogDiagnosticCode,
    json_path: &str,
) -> bool {
    let Err(failure) = compiled else {
        return false;
    };
    failure
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == code && diagnostic.json_path == json_path)
}

// Whether the catalog is refused with `code` at a pointer inside `within`: the reader points at
// the member nearest to where it stopped.
fn refused_within(
    compiled: Result<CompiledCatalog, CatalogCompileFailure>,
    code: CatalogDiagnosticCode,
    within: &str,
) -> bool {
    let Err(failure) = compiled else {
        return false;
    };
    failure
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == code && diagnostic.json_path.starts_with(within))
}

// Evaluates instance A at NOW with no task history; the host has room for one heavy dispatch.
fn evaluate_at(catalog: &CompiledCatalog, facts: Vec<ObservedFact>) -> PolicyEvaluation {
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-fact-reference".to_owned(),
        facts,
        outcomes: Vec::new(),
        tasks: Vec::new(),
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

// The game root's `schedule.json`: one rule running the package, due by an hourly clock or
// while `a >= b - 1000`, with `requires` naming `fact_reference` when `declared`, and one profile.
fn schedule(declared: bool) -> Value {
    let mut predicate = reference_predicate(SCHEDULE_INSTANCE);
    predicate
        .as_object_mut()
        .expect("a predicate object")
        .shift_remove("scope");
    let trigger = json!({
        "kind": "any",
        "predicates": [
            {
                "kind": "clock",
                "schedule": {
                    "kind": "interval",
                    "clock_source": {"kind": "local"},
                    "every_ms": HOUR_MS,
                    "anchor_ms": 0
                }
            },
            predicate
        ]
    });
    let mut rule = task_body(trigger);
    rule.shift_remove("instance_overrides");
    rule.insert("id".to_owned(), json!(TASK));
    rule.insert("package_id".to_owned(), json!(PACKAGE));
    let requires = if declared {
        json!([FACT_REFERENCE_SUBSYSTEM])
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
// bound instance.
fn compose(
    schedule: &Value,
    subsystems: Option<&BTreeSet<String>>,
) -> Result<StandardPackComposition, StandardPackComposeFailure> {
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
        subsystems,
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
