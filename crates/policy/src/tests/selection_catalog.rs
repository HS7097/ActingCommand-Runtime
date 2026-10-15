// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan fixture I4, "catalog with a selection document" (`inventory-policy.md:122`, test
//! plan P-2): a compiled catalog that carries a selection document, the parts that document is
//! written from, and the evaluation inputs that run it.
//!
//! Every task runs on every instance of one game, its hourly clock trigger is due at [`NOW`] and
//! its feedback stop stays false. Every instance has its own host, so instances never share a
//! budget. Built once for the D2 S1 gate tests; P-2b's G13 rows reuse it.

use serde_json::{Value, json};

use crate::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactValue, HostResourceSnapshot, InstanceSnapshot, ObservedFact,
    PolicyEvaluation, PolicyEvaluationResult, ScopeSelector, compile_catalog,
};

pub(crate) const HOUR_MS: u64 = 3_600_000;
pub(crate) const DAY_MS: u64 = 86_400_000;
/// The evaluation instant: one minute past a whole hour, so the hourly trigger of every task is
/// due and its next occurrence lies 59 minutes ahead.
pub(crate) const NOW: u64 = 1_000 * HOUR_MS + 60_000;

const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";

/// Compiles the fixture catalog: each task with its priority, and the selection document as
/// the fifth document when one is given.
pub(crate) fn catalog(tasks: &[(&str, i16)], selection: Option<&Value>) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.selection-catalog",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-selection"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let tasks = tasks
        .iter()
        .map(|(task_id, priority)| task(task_id, *priority))
        .collect::<Vec<_>>();
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([profile()]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: selection.map(|selection| source("selection", selection)),
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

fn task(task_id: &str, priority: i16) -> Value {
    json!({
        "id": task_id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{task_id}"),
        "priority": priority,
        "trigger": {
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": HOUR_MS,
                "anchor_ms": 0
            }
        },
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
        "loop_budget": {"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// One profile for the whole game, active all day on every weekday.
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

/// A selection document with these declarations, gates and terms. The scheduling layer reads
/// only the scores, so the requirement and the tie-break keys are fixed.
pub(crate) fn selection(
    fields: Vec<Value>,
    facts: Vec<Value>,
    gates: Vec<Value>,
    scoring: Vec<Value>,
) -> Value {
    json!({
        "schema_version": actingcommand_selection_policy::SELECTION_POLICY_SCHEMA_VERSION,
        "policy_id": "policy.fixture",
        "applies_to": {
            "candidate_layout_id": "layout.fixture",
            "outcome_keys": {
                "selected": "outcome.selected",
                "empty": "outcome.empty",
                "insufficient": "outcome.insufficient",
                "ambiguous": "outcome.ambiguous",
                "unknown": "outcome.unknown"
            }
        },
        "fields": fields,
        "facts": facts,
        "gates": gates,
        "scoring": scoring,
        "selection": {"mode": "none_allowed", "required_count": 0},
        "tie_break": []
    })
}

/// A declared integer candidate field.
pub(crate) fn integer_field(name: &str) -> Value {
    json!({"name": name, "value_type": {"type": "integer"}})
}

/// A declared integer fact with the document's own freshness bound.
pub(crate) fn integer_fact(fact_key: &str, max_age_ms: u64) -> Value {
    json!({
        "fact_key": fact_key,
        "value_type": {"type": "integer"},
        "max_age_ms": max_age_ms,
        "minimum_confidence_milli": 500
    })
}

pub(crate) fn field_ref(name: &str) -> Value {
    json!({"source": "field", "field": name})
}

pub(crate) fn fact_ref(fact_key: &str) -> Value {
    json!({"source": "fact", "fact_key": fact_key})
}

pub(crate) fn at_least(value: Value, threshold: i64) -> Value {
    json!({"kind": "integer_at_least", "value": value, "threshold": threshold})
}

pub(crate) fn gate(gate_id: &str, predicate: Value, on_unknown: Value) -> Value {
    json!({"gate_id": gate_id, "predicate": predicate, "on_unknown": on_unknown})
}

/// A term that scores an integer as it is, at weight one.
pub(crate) fn identity_term(term_id: &str, value: Value, on_unknown: Value) -> Value {
    json!({
        "term_id": term_id,
        "value": value,
        "transform": {"kind": "identity"},
        "weight_milli": 1000,
        "on_unknown": on_unknown
    })
}

pub(crate) fn drop_candidate() -> Value {
    json!({"kind": "drop_candidate"})
}

pub(crate) fn abort_evaluation() -> Value {
    json!({"kind": "abort_evaluation"})
}

/// An available instance of the fixture game on its own host, with affinity for `preferred`.
pub(crate) fn instance(instance_id: &str, preferred: &[&str]) -> InstanceSnapshot {
    InstanceSnapshot {
        instance_id: instance_id.to_owned(),
        server_id: "server-a".to_owned(),
        game_id: GAME.to_owned(),
        host_id: format!("host.{instance_id}"),
        available: true,
        capability_operation_ids: vec![OPERATION.to_owned()],
        preferred_task_ids: preferred.iter().map(|task| (*task).to_owned()).collect(),
    }
}

/// An integer fact of one instance, at full confidence.
pub(crate) fn fact(
    instance_id: &str,
    fact_key: &str,
    value: i64,
    observed_at_unix_ms: u64,
    expires_at_unix_ms: Option<u64>,
) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: instance_id.to_owned(),
        },
        fact_key: fact_key.to_owned(),
        value: FactValue::Integer(value),
        observed_at_unix_ms,
        expires_at_unix_ms,
        confidence_milli: 1_000,
    }
}

/// Evaluates the catalog at [`NOW`] over these instances and facts, with no admission callback.
pub(crate) fn evaluate_fixture(
    catalog: &CompiledCatalog,
    instances: &[InstanceSnapshot],
    facts: Vec<ObservedFact>,
) -> PolicyEvaluationResult<PolicyEvaluation> {
    let inputs = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-selection".to_owned(),
        facts,
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: instances.to_vec(),
        priority_offsets: Vec::new(),
    };
    let resources = EvaluationResources {
        pools: Vec::new(),
        hosts: instances
            .iter()
            .map(|instance| HostResourceSnapshot {
                host_id: instance.host_id.clone(),
                cpu_available_milli: 1_000,
                gpu_available_milli: 1_000,
                io_available_milli: 1_000,
                host_responsiveness_basis_points: 10_000,
                third_party_pressure_basis_points: 0,
                heavy_dispatch_limit: 1,
                active_heavy_dispatches: 0,
                heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
            })
            .collect(),
    };
    let time = EvaluationTime {
        unix_ms: NOW,
        monotonic_ms: NOW,
    };
    crate::evaluate(catalog, &inputs, &resources, time, 11)
}
