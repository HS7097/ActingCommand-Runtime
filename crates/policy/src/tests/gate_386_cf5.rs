// SPDX-License-Identifier: AGPL-3.0-only

// #386 slice CF-5 gate tests (`contracts/scheduling/README.md`, "Pool Projection" and "Fact
// references"; Alice 10-26: an account's cap is read, never a constant): a V2 pool's `capacity`
// and a `resource_projection`'s `value` may each be a fact of the pool's scope plus an integer
// offset, `{"type": "fact", "fact_key", "offset"}`. A neutral V2 catalog on instance
// `instance-a`: one `ledger_fact` pool `pool.neutral` that reads `resource.neutral.amount` and
// regenerates one unit per ten-second period, and one task `task.neutral` whose trigger is a
// `resource_projection` on it. Every expected value comes from the work order and the contract
// text, never from what the code does.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{
    STANDARD_PACK_SCHEDULE_FILE, StandardPackProblem, StandardPackRootBytes,
    read_standard_pack_root,
};
use serde_json::{Map, Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, DecisionReason, DetectionSuggestion,
    EligibilityState, EvaluationFacts, EvaluationResources, EvaluationTime, FactValue,
    HostResourceSnapshot, ObservedFact, POOL_FACT_REFERENCE_SUBSYSTEM, PolicyEvaluation,
    SCHEDULING_SCHEMA_VERSION_V2, SchedulingDecisionState, ScopeSelector, StandardPackBinding,
    StandardPackComposeFailure, StandardPackComposeInput, StandardPackComposition,
    StandardPackGameFiles, TaskDecision, TaskRank, compile_catalog, compose_standard_pack_rules,
    evaluate, project_fact_pools,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const TASK: &str = "task.neutral";
const POOL: &str = "pool.neutral";
// The pool's own reading and its capacity: realistic instance facts a reading writes together.
const AMOUNT: &str = "resource.neutral.amount";
const CAPACITY: &str = "resource.neutral.capacity";
// One unit regenerates per complete period of ten seconds.
const PERIOD_MS: u64 = 10_000;
const MINIMUM_CONFIDENCE: u16 = 900;
const REFERENCE_UNAVAILABLE: &str = "fact_reference_unavailable";
const TRIGGER_FALSE: &str = "trigger_false";
const TRIGGER_UNKNOWN: &str = "trigger_unknown";
// The golden identities of the neutral example catalog `contracts/scheduling/examples/catalog-a`
// declared V2 (every timeline event with validity {from 0, until null}), computed apart from the
// code under test: the canonical JSON (keys in UTF-16 order, no whitespace) of its four documents
// under `activity`, `pools`, `tasks` and `timeline`, hashed with SHA-256 by a Python script that
// first reproduces the V1 golden (`neutral_catalog_compiles_to_stable_hash_and_dry_run`) and
// R7-T6's V2 golden. The first is the plain catalog (its pool's `capacity` is the number 120);
// the second has as its trigger's second predicate
// `{"kind": "resource_projection", "pool_id": "fixture-pool-a", "comparison":
// "greater_than_or_equal", "value": 100}`.
const PLAIN_V2_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";
const CONSTANT_PROJECTION_V2_HASH: &str =
    "sha256:15b408ec0e50312eae5efcd9223951a2bc928f63c4e9bd86101a6e871921fe48";
// The standard-pack fixtures: one game root with one package on one server, one bound instance.
const SCHEDULE_INSTANCE: &str = "fixture-instance-a";
const SERVER: &str = "server-a";
const PACKAGE: &str = "fixture.neutral";
const PRESENT_SUBSYSTEM: &str = "standard_pack_schedule";

// Work order: a pool with a fact capacity projects as one with the same constant while the fact
// holds, and the projection follows when the fact changes. The amount 90 read at NOW regenerates
// one unit per period up to the capacity: 95 after five periods, 100 after ten and still after
// forty under a capacity of 100; under a newer capacity reading of 120 it is 110 after twenty
// periods and 120 after thirty. Each round decides exactly as the same catalog with that number
// as its constant capacity.
#[test]
fn cf5_a_fact_capacity_projects_as_the_same_constant_and_follows_the_fact() {
    let read_capacity = || fact_capacity(0);
    for (capacity, read_at, periods, projected) in [
        (100, NOW, 0, 90),
        (100, NOW, 5, 95),
        (100, NOW, 10, 100),
        (100, NOW, 40, 100),
        (120, NOW + PERIOD_MS, 20, 110),
        (120, NOW + PERIOD_MS, 30, 120),
        (120, NOW + PERIOD_MS, 40, 120),
    ] {
        let at = NOW + periods * PERIOD_MS;
        let case = format!("capacity {capacity} after {periods} periods");
        let facts = || vec![amount(90, NOW), capacity_reading(capacity, read_at)];
        for (comparison, value) in [
            ("eq", projected),
            ("eq", projected + 1),
            ("greater_than_or_equal", projected + 1),
        ] {
            let read = evaluate_at(
                &pool_catalog(read_capacity(), comparison, json!(value)),
                facts(),
                at,
            );
            let constant = evaluate_at(
                &pool_catalog(json!(capacity), comparison, json!(value)),
                facts(),
                at,
            );
            assert_eq!(
                summary(&read),
                summary(&constant),
                "{case}: {comparison} {value} decides as the constant capacity"
            );
            assert_eq!(
                dispatched(&read),
                if value == projected && comparison == "eq" {
                    vec![TASK]
                } else {
                    Vec::new()
                },
                "{case}: the projection is {projected}, {comparison} {value}"
            );
            assert_ne!(
                decision(&read, TASK).eligibility,
                EligibilityState::Unknown,
                "{case}"
            );
        }
    }
}

// Work order: an unknown or stale capacity fact defers the dependent predicate with its reason,
// and no constant stands in for it. The predicate is Unknown (`trigger_unknown`) with one
// detection suggestion for the capacity fact in the pool's scope with that fact's unknown
// reason: none (`fact_missing`), past its own expiry (`fact_expired`), below the pool's minimum
// confidence (`fact_low_confidence`), no integer or no positive capacity (`type_mismatch`). The
// amount alone would make the trigger true.
#[test]
fn cf5_a_capacity_fact_that_gives_no_capacity_defers_its_projection_with_its_reason() {
    let catalog = pool_catalog(fact_capacity(0), "greater_than_or_equal", json!(1));
    let at = NOW + 2 * PERIOD_MS;
    let mut expired = capacity_reading(100, NOW);
    expired.expires_at_unix_ms = Some(NOW + PERIOD_MS);
    let mut low_confidence = capacity_reading(100, NOW);
    low_confidence.confidence_milli = MINIMUM_CONFIDENCE - 1;
    let text = ObservedFact {
        value: FactValue::String("text".to_owned()),
        ..capacity_reading(100, NOW)
    };
    for (case, capacity, unknown_reason) in [
        ("missing", None, "fact_missing"),
        ("stale: past its own expiry", Some(expired), "fact_expired"),
        (
            "below the pool's minimum confidence",
            Some(low_confidence),
            "fact_low_confidence",
        ),
        ("no integer", Some(text), "type_mismatch"),
        (
            "no positive capacity",
            Some(capacity_reading(0, NOW)),
            "type_mismatch",
        ),
    ] {
        let facts = std::iter::once(amount(90, NOW)).chain(capacity).collect();
        let evaluation = evaluate_at(&catalog, facts, at);
        assert!(evaluation.dispatch_intents.is_empty(), "{case}");
        let waiting = decision(&evaluation, TASK);
        assert_eq!(waiting.eligibility, EligibilityState::Unknown, "{case}");
        assert_eq!(
            waiting.detection_suggestions,
            [suggestion(CAPACITY, unknown_reason)],
            "{case}"
        );
        assert!(codes(waiting).contains(&TRIGGER_UNKNOWN), "{case}");
    }
}

// Work order: a reading above the capacity is kept, and regeneration stops at the capacity. 130
// read under a capacity read as 100 stays 130 as time passes (it neither falls to the capacity
// nor grows); 95 grows to 98 after three periods and stops at 100. A pool at or above its
// capacity gives no regeneration wake.
#[test]
fn cf5_a_reading_above_the_capacity_is_kept_and_regeneration_stops_at_the_capacity() {
    let facts = |value| vec![amount(value, NOW), capacity_reading(100, NOW)];
    let projected = project_fact_pools(
        &pool_catalog(fact_capacity(0), "eq", json!(130)),
        &evaluation_facts(facts(130)),
        &resources(),
    );
    assert_eq!(
        projected
            .pools
            .iter()
            .map(|pool| (pool.pool_id.as_str(), pool.value))
            .collect::<Vec<_>>(),
        [(POOL, 130)],
        "the reading above the capacity is the pool's value"
    );

    for (value, periods, expected) in [
        (130, 0, 130),
        (130, 50, 130),
        (95, 3, 98),
        (95, 5, 100),
        (95, 50, 100),
    ] {
        let at = NOW + periods * PERIOD_MS;
        let case = format!("{value} after {periods} periods");
        for (candidate, due) in [
            (expected, true),
            (expected + 1, false),
            (100, expected == 100),
        ] {
            let evaluation = evaluate_at(
                &pool_catalog(fact_capacity(0), "eq", json!(candidate)),
                facts(value),
                at,
            );
            assert_eq!(
                dispatched(&evaluation),
                if due { vec![TASK] } else { Vec::new() },
                "{case}: projected {expected}, eq {candidate}"
            );
        }
        if expected >= 100 {
            let above = evaluate_at(
                &pool_catalog(
                    fact_capacity(0),
                    "greater_than_or_equal",
                    json!(expected + 1),
                ),
                facts(value),
                at,
            );
            let waiting = decision(&above, TASK);
            assert_eq!(codes(waiting), [TRIGGER_FALSE], "{case}");
            assert_eq!(
                waiting.next_wake_unix_ms, None,
                "{case}: nothing regenerates at or above the capacity"
            );
        }
    }
}

// Work order: a projection compared with `capacity - offset` evaluates as the constant form with
// the same number. A pool whose capacity is read as 100 and a trigger `projected >= capacity - 20`
// decide every round as a pool of constant capacity 100 and the trigger `projected >= 80`: false
// at 79 (waking at the next period), true at 80 one period later and at 81. A referenced fact
// that gives no integer makes the predicate false with `fact_reference_unavailable {fact_key,
// unknown_reason}` and then `trigger_false`, with no fact refresh and no wake, as for a `fact`
// predicate (CF-4).
#[test]
fn cf5_a_projection_compared_with_the_capacity_less_an_offset_evaluates_as_the_constant() {
    let relative = pool_catalog(
        fact_capacity(0),
        "greater_than_or_equal",
        fact_comparand(-20),
    );
    let constant = pool_catalog(json!(100), "greater_than_or_equal", json!(80));
    for (value, at, due) in [
        (79, NOW, false),
        (79, NOW + PERIOD_MS, true),
        (81, NOW, true),
    ] {
        let facts = || vec![amount(value, NOW), capacity_reading(100, NOW)];
        let read = evaluate_at(&relative, facts(), at);
        assert_eq!(
            summary(&read),
            summary(&evaluate_at(&constant, facts(), at)),
            "{value} at {at}"
        );
        assert_eq!(
            dispatched(&read),
            if due { vec![TASK] } else { Vec::new() },
            "{value} at {at}"
        );
        if !due {
            let waiting = decision(&read, TASK);
            assert_eq!(codes(waiting), [TRIGGER_FALSE]);
            assert_eq!(waiting.next_wake_unix_ms, Some(NOW + PERIOD_MS));
        }
    }

    let unavailable = pool_catalog(json!(100), "greater_than_or_equal", fact_comparand(-20));
    let evaluation = evaluate_at(&unavailable, vec![amount(90, NOW)], NOW);
    assert!(evaluation.dispatch_intents.is_empty());
    let waiting = decision(&evaluation, TASK);
    assert_eq!(waiting.state, SchedulingDecisionState::Blocked);
    assert_eq!(
        waiting.eligibility,
        EligibilityState::False,
        "false, not unknown"
    );
    assert_eq!(codes(waiting), [REFERENCE_UNAVAILABLE, TRIGGER_FALSE]);
    assert_values(
        &waiting.reasons[0],
        &[
            ("fact_key", json!(CAPACITY)),
            ("unknown_reason", json!("fact_missing")),
        ],
    );
    assert!(waiting.detection_suggestions.is_empty(), "no fact refresh");
    assert_eq!(waiting.next_wake_unix_ms, None, "no wake");
}

// Work order: a document that uses neither form evaluates byte-identically to today. The neutral
// example catalog declared V2 keeps its golden hash and its pool document's bytes; with a
// constant `resource_projection` it has the golden computed for it and keeps the predicate's
// bytes. A constant capacity and a constant comparand decide as the contract says: 95 read at NOW
// reaches 100 after five periods and stops there; one period earlier the predicate is false and
// wakes at the next period; a reading above a constant capacity leaves the pool without a value
// (Unknown, `type_mismatch` on the pool's own key).
#[test]
fn cf5_a_document_without_either_form_keeps_its_hash_bytes_and_evaluation() {
    let plain = compile_catalog(&example_sources(None))
        .unwrap_or_else(|failure| panic!("the catalog compiles: {:?}", failure.diagnostics()));
    assert_eq!(plain.catalog_hash(), PLAIN_V2_HASH);
    assert_eq!(
        serde_json::to_value(&plain.catalog().pools).expect("the pools serialize"),
        example_document("pools"),
        "the pool document keeps its bytes"
    );

    let projection = json!({
        "kind": "resource_projection",
        "pool_id": "fixture-pool-a",
        "comparison": "greater_than_or_equal",
        "value": 100
    });
    let constant = compile_catalog(&example_sources(Some(&projection)))
        .unwrap_or_else(|failure| panic!("the catalog compiles: {:?}", failure.diagnostics()));
    assert_eq!(constant.catalog_hash(), CONSTANT_PROJECTION_V2_HASH);
    assert_eq!(
        serde_json::to_value(&constant.catalog().tasks.tasks[0].trigger)
            .expect("the trigger serializes")["predicates"][1],
        projection,
        "the predicate keeps its bytes"
    );

    let catalog = pool_catalog(json!(100), "greater_than_or_equal", json!(100));
    let facts = || vec![amount(95, NOW), capacity_reading(120, NOW)];
    assert_eq!(
        dispatched(&evaluate_at(&catalog, facts(), NOW + 5 * PERIOD_MS)),
        [TASK]
    );
    assert_eq!(
        dispatched(&evaluate_at(&catalog, facts(), NOW + 50 * PERIOD_MS)),
        [TASK]
    );
    let below = evaluate_at(&catalog, facts(), NOW + 4 * PERIOD_MS);
    let waiting = decision(&below, TASK);
    assert_eq!(codes(waiting), [TRIGGER_FALSE]);
    assert_eq!(waiting.next_wake_unix_ms, Some(NOW + 5 * PERIOD_MS));
    let full = pool_catalog(json!(100), "eq", json!(100));
    assert_eq!(
        dispatched(&evaluate_at(&full, facts(), NOW + 50 * PERIOD_MS)),
        [TASK],
        "a constant capacity holds the regeneration at 100"
    );

    let above = evaluate_at(
        &catalog,
        vec![amount(130, NOW), capacity_reading(120, NOW)],
        NOW,
    );
    let unknown = decision(&above, TASK);
    assert_eq!(unknown.eligibility, EligibilityState::Unknown);
    assert_eq!(
        unknown.detection_suggestions,
        [suggestion(AMOUNT, "type_mismatch")]
    );
}

// Work order: `requires` declares the new forms, and the composer refuses a document that uses
// them without declaring them. A `schedule.json` whose pool reads its capacity from a fact and
// whose rule compares the projection with that capacity less 20 composes when `requires` names
// `pool_fact_reference`, with or without a Runtime manifest at hand, and keeps both references as
// written while the pool gets the instance's scope. Without the entry it is refused,
// `schedule_invalid` at the first reference (the rule's, then the pool's when the rule writes
// none); a Runtime that does not name the subsystem refuses the file at its `requires` entry
// (`schedule_requires_missing`) instead of misreading it.
#[test]
fn cf5_the_composer_accepts_both_forms_its_requires_names_and_refuses_them_without() {
    let present = BTreeSet::from([
        PRESENT_SUBSYSTEM.to_owned(),
        POOL_FACT_REFERENCE_SUBSYSTEM.to_owned(),
    ]);
    for subsystems in [Some(&present), None] {
        let composition = compose(&schedule(true, fact_comparand(-20)), subsystems)
            .unwrap_or_else(|failure| panic!("the schedule composes: {failure}"));
        assert_eq!(
            composition.games()[0].requires,
            [POOL_FACT_REFERENCE_SUBSYSTEM]
        );
        let catalog = composition.compiled().catalog();
        let trigger =
            serde_json::to_value(&catalog.tasks.tasks[0].trigger).expect("the trigger serializes");
        assert_eq!(
            trigger["predicates"][1]["value"],
            fact_comparand(-20),
            "the comparand is composed as written"
        );
        let pool = serde_json::to_value(&catalog.pools.pools[0]).expect("the pool serializes");
        assert_eq!(
            pool["capacity"],
            fact_capacity(0),
            "the capacity is composed as written"
        );
        assert_eq!(
            pool["scope"],
            json!({"kind": "instance", "instance_id": SCHEDULE_INSTANCE})
        );

        assert_eq!(
            refused(compose(&schedule(false, fact_comparand(-20)), subsystems)),
            (
                StandardPackProblem::ScheduleInvalid,
                "/rules/0/trigger/predicates/1/value".to_owned()
            ),
            "without the requires entry"
        );
        assert_eq!(
            refused(compose(&schedule(false, json!(80)), subsystems)),
            (
                StandardPackProblem::ScheduleInvalid,
                "/pools/0/capacity".to_owned()
            ),
            "a fact capacity alone, without the requires entry"
        );
    }

    let older = BTreeSet::from([PRESENT_SUBSYSTEM.to_owned()]);
    assert_eq!(
        refused(compose(&schedule(true, fact_comparand(-20)), Some(&older))),
        (
            StandardPackProblem::ScheduleRequiresMissing,
            "/requires/0".to_owned()
        ),
        "a Runtime that does not read the forms"
    );
}

// ---- Fixtures ----

// The pool's capacity read from CAPACITY in its scope plus `offset`.
fn fact_capacity(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY, "offset": offset})
}

// A projection's comparand: CAPACITY in the pool's scope plus `offset`.
fn fact_comparand(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY, "offset": offset})
}

// The pool's members besides its id and scope, with `capacity`.
fn pool_body(capacity: Value) -> Map<String, Value> {
    let Value::Object(body) = json!({
        "capacity": capacity,
        "projection": {"amount": 1, "per_ms": PERIOD_MS},
        "observation": {"kind": "fact", "fact_key": AMOUNT},
        "value_source": {
            "kind": "ledger_fact",
            "minimum_confidence_milli": MINIMUM_CONFIDENCE
        },
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    }) else {
        unreachable!("a JSON object")
    };
    body
}

// The task's trigger: the pool's projection `comparison` `value`.
fn projection(comparison: &str, value: Value) -> Value {
    json!({
        "kind": "resource_projection",
        "pool_id": POOL,
        "comparison": comparison,
        "value": value
    })
}

// The neutral V2 catalog: the pool on instance A with `capacity`, and the task due while the
// pool's projection `comparison` `value`.
fn pool_catalog(capacity: Value, comparison: &str, value: Value) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.pool-fact-reference",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-pool-fact-reference"]
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
            format!("memory://fixture-pool-fact-reference/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    let mut task = task_body(projection(comparison, value));
    task.insert("id".to_owned(), json!(TASK));
    task.insert("scope".to_owned(), json!({"kind": "game", "game_id": GAME}));
    task.insert(
        "entrypoint".to_owned(),
        json!({"operation_id": "operation.run"}),
    );
    task.insert("procedure_ref".to_owned(), json!("procedure.neutral"));
    let mut pool = pool_body(capacity);
    pool.insert("id".to_owned(), json!(POOL));
    pool.insert(
        "scope".to_owned(),
        json!({"kind": "instance", "instance_id": A}),
    );
    let mut activity = profile();
    activity.insert("scope".to_owned(), json!({"kind": "game", "game_id": GAME}));
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", json!([task]))),
        pools: source("pools", &document("pools", json!([pool]))),
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

// An integer fact of instance A, read at `observed_at` above the pool's minimum confidence, with
// no expiry of its own.
fn integer_fact(fact_key: &str, value: i64, observed_at: u64) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: fact_key.to_owned(),
        value: FactValue::Integer(value),
        observed_at_unix_ms: observed_at,
        expires_at_unix_ms: None,
        confidence_milli: 990,
    }
}

fn amount(value: i64, observed_at: u64) -> ObservedFact {
    integer_fact(AMOUNT, value, observed_at)
}

fn capacity_reading(value: i64, observed_at: u64) -> ObservedFact {
    integer_fact(CAPACITY, value, observed_at)
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

fn evaluation_facts(facts: Vec<ObservedFact>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-pool-fact-reference".to_owned(),
        facts,
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

// No static pool value; A's host has room for one heavy dispatch.
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

// One round at `at` with no task history. The pool values are those the Runtime derives from the
// facts (the host's projection of a `ledger_fact` pool).
fn evaluate_at(
    catalog: &CompiledCatalog,
    observed: Vec<ObservedFact>,
    at: u64,
) -> PolicyEvaluation {
    let facts = evaluation_facts(observed);
    let resources = project_fact_pools(catalog, &facts, &resources());
    evaluate(
        catalog,
        &facts,
        &resources,
        EvaluationTime {
            unix_ms: at,
            monotonic_ms: at,
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

/// What a round decided for the task, without the catalog's identity: its eligibility, state,
/// reason codes, fact refresh suggestions, own wake and rank, and what it dispatched.
type Summary = (
    EligibilityState,
    SchedulingDecisionState,
    Vec<String>,
    Vec<DetectionSuggestion>,
    Option<u64>,
    Option<TaskRank>,
    Vec<String>,
);

fn summary(evaluation: &PolicyEvaluation) -> Summary {
    let decision = decision(evaluation, TASK);
    (
        decision.eligibility,
        decision.state,
        codes(decision).into_iter().map(str::to_owned).collect(),
        decision.detection_suggestions.clone(),
        decision.next_wake_unix_ms,
        decision.rank.clone(),
        dispatched(evaluation)
            .into_iter()
            .map(str::to_owned)
            .collect(),
    )
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

// One neutral example document of catalog-a declared V2 (a timeline event gets validity
// {from 0, until null}).
fn example_document(name: &str) -> Value {
    let bytes: &[u8] = match name {
        "tasks" => include_bytes!("../../../../contracts/scheduling/examples/catalog-a/tasks.json"),
        "pools" => include_bytes!("../../../../contracts/scheduling/examples/catalog-a/pools.json"),
        "activity" => {
            include_bytes!("../../../../contracts/scheduling/examples/catalog-a/activity.json")
        }
        "timeline" => {
            include_bytes!("../../../../contracts/scheduling/examples/catalog-a/timeline.json")
        }
        other => unreachable!("no example document {other}"),
    };
    let mut document: Value = serde_json::from_slice(bytes).expect("an example document");
    document["schema_version"] = json!(SCHEDULING_SCHEMA_VERSION_V2);
    if let Some(events) = document.get_mut("events").and_then(Value::as_array_mut) {
        for event in events {
            event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
        }
    }
    document
}

// The four neutral example documents declared V2, with `predicate` as the second predicate of
// the task's trigger when given.
fn example_sources(predicate: Option<&Value>) -> CatalogSources {
    let mut tasks = example_document("tasks");
    if let Some(predicate) = predicate {
        tasks["tasks"][0]["trigger"]["predicates"][1] = predicate.clone();
    }
    let source = |name: &str, document: &Value| {
        CatalogDocumentSource::new(
            format!("memory://catalog-a/{name}.json"),
            serde_json::to_vec(document).expect("document bytes"),
        )
    };
    CatalogSources {
        tasks: source("tasks", &tasks),
        pools: source("pools", &example_document("pools")),
        activity: source("activity", &example_document("activity")),
        timeline: source("timeline", &example_document("timeline")),
        selection: None,
    }
}

// The game root's `schedule.json`: one rule running the package while the pool's projection is
// at least `value` (or every hour), and the pool, which reads its capacity from CAPACITY, with
// `requires` naming `pool_fact_reference` when `declared`, and one profile.
fn schedule(declared: bool, value: Value) -> Value {
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
            projection("greater_than_or_equal", value)
        ]
    });
    let mut rule = task_body(trigger);
    rule.shift_remove("instance_overrides");
    rule.insert("id".to_owned(), json!(TASK));
    rule.insert("package_id".to_owned(), json!(PACKAGE));
    let mut pool = pool_body(fact_capacity(0));
    pool.insert("id".to_owned(), json!(POOL));
    let requires = if declared {
        json!([POOL_FACT_REFERENCE_SUBSYSTEM])
    } else {
        json!([])
    };
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": [rule],
        "pools": [pool],
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
