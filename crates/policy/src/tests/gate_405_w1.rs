// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #405 W1 gate tests (policy gate key 53; register section 1.134;
// `contracts/scheduling/README.md`, "Fact-time wake"): the V2 catalog predicate
// `fact_time_reached`, true once the `timestamp_ms` value of a fact plus a signed offset has come,
// waking the evaluation at that instant. A neutral catalog: task `task_a` on instance
// `instance-a`, the instance fact `resource.neutral.slot_ready_at`, an offset of 60 000 ms unless
// a case says otherwise. Every expected value comes from the slice's requirement named on each
// test, never from what the code does.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::{
    CandidateEligibility, CatalogCompileFailure, CatalogDiagnosticCode, CatalogDocumentSource,
    CatalogSources, CompiledCatalog, DecisionReason, EvaluationFacts, EvaluationResources,
    EvaluationTime, FACT_REFRESH_TASK_ID, FactRefreshInputs, FactRefreshReader,
    FactRefreshSettings, FactValue, HostResourceSnapshot, MAX_FACT_MAX_AGE_MS,
    MAX_FACT_TIME_OFFSET_MS, MIN_FACT_TIME_OFFSET_MS, ObservedFact, PolicyEvaluation,
    PolicyEvaluationError, SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2,
    SchedulingDecisionState, ScopeSelector, TaskDecision, TaskRuntimeSnapshot, TaskTerminalState,
    compile_catalog, evaluate_with_fact_refresh,
};

const TASK_A: &str = "task_a";
const INSTANCE_A: &str = "instance-a";
const SLOT_READY_AT: &str = "resource.neutral.slot_ready_at";
// Instance A's recognition maintenance package; its marks write the key, so a predicate that
// asked for a fact refresh of it would be served.
const READER: &str = "package.reader-a";
const OFFSET_MS: i64 = 60_000;
// The predicate's `max_age_ms` unless a case says otherwise: two hours.
const MAX_AGE_MS: u64 = 7_200_000;
const PENDING: &str = "fact_time_pending";
const UNAVAILABLE: &str = "fact_time_unavailable";
const TRIGGER_FALSE: &str = "trigger_false";
// The hourly clock of the fallback: its occurrences are whole hours, so at NOW (one minute past
// a whole hour) the latest occurred one minute ago and the next comes 59 minutes ahead.
const LATEST_SLOT: u64 = NOW - 60_000;
const NEXT_SLOT: u64 = LATEST_SLOT + HOUR_MS;

// Requirements 2 and 3: the predicate is true from `T + offset_ms` on and false one millisecond
// before, for a positive and a negative offset. While true, its freshness and its wake are the
// fact's: the freshness end, and one millisecond after it.
#[test]
fn w1_a_reached_time_is_true_from_value_plus_offset_and_false_one_ms_before() {
    let observed_at = NOW - HOUR_MS;
    let fresh_end = observed_at + MAX_AGE_MS;
    let held = NOW;
    for (offset_ms, target) in [(OFFSET_MS, held + 60_000), (-OFFSET_MS, held - 60_000)] {
        let catalog = catalog(fact_time_reached(offset_ms, json!(MAX_AGE_MS)));
        let facts = || vec![slot_ready_at(FactValue::TimestampMs(held), observed_at)];

        let before = evaluate_at(&catalog, facts(), Vec::new(), target - 1);
        assert!(
            before.dispatch_intents.is_empty(),
            "offset {offset_ms}: one millisecond before T + offset"
        );
        let waiting = decision(&before);
        assert_eq!(
            waiting.state,
            SchedulingDecisionState::Blocked,
            "{offset_ms}"
        );
        assert_eq!(last_code(waiting), TRIGGER_FALSE, "{offset_ms}");

        let reached = evaluate_at(&catalog, facts(), Vec::new(), target);
        assert_eq!(
            reached.dispatch_intents.len(),
            1,
            "offset {offset_ms}: at T + offset"
        );
        let intent = &reached.dispatch_intents[0];
        assert_eq!(intent.task_id, TASK_A);
        assert_eq!(intent.instance_id, INSTANCE_A);
        assert_eq!(
            intent.prerequisites.facts_fresh_until_unix_ms,
            Some(fresh_end),
            "offset {offset_ms}: the freshness is the fact's"
        );
        assert_eq!(decision(&reached).state, SchedulingDecisionState::Selected);
        assert_eq!(
            reached.next_wake_unix_ms,
            Some(fresh_end + 1),
            "offset {offset_ms}: the wake is one millisecond after the freshness end"
        );
    }
}

// Requirement 3: before `T + offset_ms` the predicate is false; the decision carries
// `fact_time_pending {fact_key, next_eligible_unix_ms: T + offset_ms}` and then `trigger_false`,
// and the evaluation wakes at `min(T + offset_ms, freshness end + 1)`.
#[test]
fn w1_a_future_time_defers_with_fact_time_pending_and_wakes_at_value_plus_offset() {
    let observed_at = NOW - 60_000;
    let held = NOW + 600_000;
    let target = held + 60_000;
    for (max_age_ms, wake) in [
        // The fact stays fresh past T + offset: the evaluation wakes at T + offset.
        (MAX_AGE_MS, target),
        // Its freshness ends first: the evaluation wakes one millisecond after that end.
        (300_000, observed_at + 300_000 + 1),
    ] {
        let evaluation = evaluate_at(
            &catalog(fact_time_reached(OFFSET_MS, json!(max_age_ms))),
            vec![slot_ready_at(FactValue::TimestampMs(held), observed_at)],
            Vec::new(),
            NOW,
        );
        assert!(evaluation.dispatch_intents.is_empty(), "{max_age_ms}");
        let waiting = decision(&evaluation);
        assert_eq!(waiting.state, SchedulingDecisionState::Blocked);
        assert_eq!(codes(waiting), [PENDING, TRIGGER_FALSE], "{max_age_ms}");
        assert_values(
            &waiting.reasons[0],
            &[
                ("fact_key", json!(SLOT_READY_AT)),
                ("next_eligible_unix_ms", json!(target)),
            ],
        );
        assert_eq!(evaluation.next_wake_unix_ms, Some(wake), "{max_age_ms}");
    }
}

// Requirement 4: a fact that gives no time makes the predicate false, with
// `fact_time_unavailable {fact_key, fact_time_problem}` and then `trigger_false`. It gives no
// detection suggestion, so no fact refresh is requested although instance A's reader marks the
// key, and no wake. Each unusable fact holds a time that would otherwise have been reached.
#[test]
fn w1_an_unusable_fact_is_false_with_its_problem_and_requests_no_refresh() {
    let reached = FactValue::TimestampMs(NOW - 600_000);
    let usable = || slot_ready_at(reached.clone(), NOW - 60_000);
    for (problem, facts) in [
        ("missing", Vec::new()),
        (
            "low_confidence",
            vec![ObservedFact {
                confidence_milli: 0,
                ..usable()
            }],
        ),
        (
            "stale",
            vec![slot_ready_at(reached.clone(), NOW - MAX_AGE_MS - 1)],
        ),
        (
            "expired",
            vec![ObservedFact {
                expires_at_unix_ms: Some(NOW - 1),
                ..usable()
            }],
        ),
        (
            "not_a_time",
            vec![ObservedFact {
                value: FactValue::Integer(5),
                ..usable()
            }],
        ),
    ] {
        let evaluation = evaluate_at(
            &catalog(fact_time_reached(OFFSET_MS, json!(MAX_AGE_MS))),
            facts,
            Vec::new(),
            NOW,
        );
        assert!(
            evaluation.dispatch_intents.is_empty(),
            "{problem}: nothing is dispatched, no fact refresh either: {:#?}",
            evaluation.dispatch_intents
        );
        assert!(
            evaluation
                .decisions
                .iter()
                .all(|decision| decision.task_id != FACT_REFRESH_TASK_ID),
            "{problem}: the reader is not requested"
        );
        let waiting = decision(&evaluation);
        assert_eq!(waiting.state, SchedulingDecisionState::Blocked, "{problem}");
        assert!(waiting.detection_suggestions.is_empty(), "{problem}");
        assert_eq!(codes(waiting), [UNAVAILABLE, TRIGGER_FALSE], "{problem}");
        assert_values(
            &waiting.reasons[0],
            &[
                ("fact_key", json!(SLOT_READY_AT)),
                ("fact_time_problem", json!(problem)),
            ],
        );
        assert_eq!(evaluation.next_wake_unix_ms, None, "{problem}: no wake");
    }
}

// Requirement 5: `any [fact_time_reached, clock]` dispatches at whichever comes first. The pair
// was served at the clock's latest occurrence. (a) The fact's time plus the offset comes before
// the clock's next occurrence: the evaluation wakes at it and dispatches there. (b) The fact is
// missing: the task waits for the clock (`trigger_false`, no fact refresh) and dispatches when
// the clock is due, with the missing fact's reason in its chain.
#[test]
fn w1_any_with_a_clock_dispatches_whichever_comes_first() {
    let catalog = catalog(json!({
        "kind": "any",
        "predicates": [fact_time_reached(OFFSET_MS, json!(MAX_AGE_MS)), hourly_clock()]
    }));
    let served = || vec![served_at(LATEST_SLOT + 30_000)];

    // (a) The time comes first.
    // T + offset is 11 minutes ahead, the clock's next occurrence 59 minutes ahead.
    let held = NOW + 600_000;
    let target = held + 60_000;
    let facts = || vec![slot_ready_at(FactValue::TimestampMs(held), NOW - 60_000)];
    let before = evaluate_at(&catalog, facts(), served(), target - 1);
    assert!(before.dispatch_intents.is_empty());
    assert_eq!(last_code(decision(&before)), TRIGGER_FALSE);
    assert_eq!(before.next_wake_unix_ms, Some(target));
    let reached = evaluate_at(&catalog, facts(), served(), target);
    assert_eq!(dispatched(&reached), [TASK_A], "the time came first");

    // (b) The fact is missing: the clock comes first.
    let before = evaluate_at(&catalog, Vec::new(), served(), NEXT_SLOT - 1);
    assert!(
        before.dispatch_intents.is_empty(),
        "no dispatch and no fact refresh before the clock: {:#?}",
        before.dispatch_intents
    );
    let waiting = decision(&before);
    assert!(waiting.detection_suggestions.is_empty());
    assert_eq!(codes(waiting), [UNAVAILABLE, TRIGGER_FALSE]);
    assert_eq!(before.next_wake_unix_ms, Some(NEXT_SLOT));
    let due = evaluate_at(&catalog, Vec::new(), served(), NEXT_SLOT);
    assert_eq!(dispatched(&due), [TASK_A], "the clock runs the task");
    let chain = &due.reason_chains[0].reasons;
    let unavailable = chain
        .iter()
        .find(|reason| code_of(reason) == UNAVAILABLE)
        .expect("the missing fact's reason is in the chain");
    assert_values(
        unavailable,
        &[
            ("fact_key", json!(SLOT_READY_AT)),
            ("fact_time_problem", json!("missing")),
        ],
    );
}

// Requirement 1: V2 only; `offset_ms` within -86 400 000..=86 400 000 and `max_age_ms` null or
// 1..=MAX_FACT_MAX_AGE_MS, else `limit_exceeded` at the member; every member required. A V1
// document that names the kind is refused `unknown_field` at the predicate's `/kind`.
#[test]
fn w1_a_v1_catalog_or_out_of_range_members_are_refused() {
    for (offset_ms, max_age_ms) in [
        (-86_400_000, Value::Null),
        (86_400_000, json!(1)),
        (0, json!(MAX_FACT_MAX_AGE_MS)),
    ] {
        compile(
            SCHEDULING_SCHEMA_VERSION_V2,
            fact_time_reached(offset_ms, max_age_ms),
        )
        .unwrap_or_else(|failure| {
            panic!(
                "offset {offset_ms} is admitted: {:?}",
                failure.diagnostics()
            )
        });
    }

    assert!(refused_at(
        compile(
            SCHEDULING_SCHEMA_VERSION,
            fact_time_reached(OFFSET_MS, json!(MAX_AGE_MS))
        ),
        CatalogDiagnosticCode::UnknownField,
        Some("/tasks/0/trigger/kind"),
    ));
    for offset_ms in [86_400_001, -86_400_001] {
        assert!(
            refused_at(
                compile(
                    SCHEDULING_SCHEMA_VERSION_V2,
                    fact_time_reached(offset_ms, json!(MAX_AGE_MS))
                ),
                CatalogDiagnosticCode::LimitExceeded,
                Some("/tasks/0/trigger/offset_ms"),
            ),
            "offset {offset_ms}"
        );
    }
    assert!(refused_at(
        compile(
            SCHEDULING_SCHEMA_VERSION_V2,
            fact_time_reached(OFFSET_MS, json!(0))
        ),
        CatalogDiagnosticCode::LimitExceeded,
        Some("/tasks/0/trigger/max_age_ms"),
    ));
    let mut without_max_age = fact_time_reached(OFFSET_MS, Value::Null);
    without_max_age
        .as_object_mut()
        .expect("the predicate object")
        .remove("max_age_ms");
    assert!(refused_at(
        compile(SCHEDULING_SCHEMA_VERSION_V2, without_max_age),
        CatalogDiagnosticCode::MissingRequiredField,
        None,
    ));
}

// Requirement 7: the V2 schema declares the predicate with every member required and the
// Runtime's bounds, as `published_schema_bounds_match_compiler_constants` does for `fact`; the V1
// schema does not declare it.
#[test]
fn w1_the_v2_schema_declares_fact_time_reached_with_the_runtime_bounds() {
    let variant = |text: &str| {
        serde_json::from_str::<Value>(text).expect("the schema is JSON")["$defs"]["predicate"]
            ["oneOf"]
            .as_array()
            .expect("predicate variants")
            .iter()
            .find(|variant| variant["properties"]["kind"]["const"] == "fact_time_reached")
            .cloned()
    };
    assert!(
        variant(include_str!(
            "../../../../contracts/scheduling/common.schema.json"
        ))
        .is_none(),
        "V1 does not declare it"
    );
    let v2 = variant(include_str!(
        "../../../../contracts/scheduling/v2/common.schema.json"
    ))
    .expect("V2 declares fact_time_reached");
    assert_eq!(
        v2["required"],
        json!(["kind", "scope", "fact_key", "offset_ms", "max_age_ms"])
    );
    assert_eq!(v2["additionalProperties"], false);
    let properties = &v2["properties"];
    assert_eq!(properties["scope"]["$ref"], "#/$defs/scope");
    assert_eq!(properties["fact_key"]["$ref"], "#/$defs/identifier");
    assert_eq!(properties["offset_ms"]["type"], "integer");
    assert_eq!(properties["offset_ms"]["minimum"], MIN_FACT_TIME_OFFSET_MS);
    assert_eq!(properties["offset_ms"]["maximum"], MAX_FACT_TIME_OFFSET_MS);
    assert_eq!(properties["max_age_ms"]["type"], json!(["integer", "null"]));
    assert_eq!(properties["max_age_ms"]["minimum"], 1);
    assert_eq!(properties["max_age_ms"]["maximum"], MAX_FACT_MAX_AGE_MS);
}

// The predicate on instance A's key with this offset and `max_age_ms`.
fn fact_time_reached(offset_ms: i64, max_age_ms: Value) -> Value {
    json!({
        "kind": "fact_time_reached",
        "scope": {"kind": "instance", "instance_id": INSTANCE_A},
        "fact_key": SLOT_READY_AT,
        "offset_ms": offset_ms,
        "max_age_ms": max_age_ms
    })
}

// A clock due at every whole hour (the instance's monotonic clock equals Unix time here).
fn hourly_clock() -> Value {
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

// Instance A's key with this value, observed at `observed_at`, full confidence, no own expiry.
fn slot_ready_at(value: FactValue, observed_at: u64) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: INSTANCE_A.to_owned(),
        },
        fact_key: SLOT_READY_AT.to_owned(),
        value,
        observed_at_unix_ms: observed_at,
        expires_at_unix_ms: None,
        confidence_milli: 1_000,
    }
}

// Task A on instance A, last dispatched at `at` and settled.
fn served_at(at: u64) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: TASK_A.to_owned(),
        instance_id: INSTANCE_A.to_owned(),
        last_dispatched_unix_ms: Some(at),
        eligible_since_unix_ms: None,
        terminal_state: Some(TaskTerminalState::Succeeded),
        completed_window: None,
        last_duration_ms: None,
        failure_streak: None,
    }
}

fn catalog(trigger: Value) -> CompiledCatalog {
    compile(SCHEDULING_SCHEMA_VERSION_V2, trigger).unwrap_or_else(|failure| {
        panic!("the fixture catalog compiles: {:?}", failure.diagnostics())
    })
}

// The neutral catalog of task A alone, every document declared as `version`, triggered by
// `trigger`.
fn compile(version: &str, trigger: Value) -> Result<CompiledCatalog, CatalogCompileFailure> {
    let header = json!({
        "catalog_id": "fixture.fact-time",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-fact-time"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": version,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let task = json!({
        "id": TASK_A,
        "scope": {"kind": "game", "game_id": "game-a"},
        "entrypoint": {"operation_id": "operation.run"},
        "procedure_ref": "procedure.task-a",
        "priority": 100,
        "trigger": trigger,
        "feedback_stop": {
            "kind": "outcome",
            "task_id": TASK_A,
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
    });
    let profile = json!({
        "id": "fixture-activity",
        "scope": {"kind": "game", "game_id": "game-a"},
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
    });
    let source = |name: &str, document: &Value| {
        CatalogDocumentSource::new(
            format!("memory://fixture-fact-time/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", json!([task]))),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
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

// Evaluates instance A at `now` with the fact refresh on and A's reader marking the key; the
// Runtime admits every candidate.
fn evaluate_at(
    catalog: &CompiledCatalog,
    facts: Vec<ObservedFact>,
    tasks: Vec<TaskRuntimeSnapshot>,
    now: u64,
) -> PolicyEvaluation {
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-fact-time".to_owned(),
        facts,
        outcomes: Vec::new(),
        tasks,
        instances: vec![instance(INSTANCE_A, &[])],
        priority_offsets: Vec::new(),
    };
    let resources = EvaluationResources {
        pools: Vec::new(),
        hosts: vec![HostResourceSnapshot {
            host_id: format!("host.{INSTANCE_A}"),
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
    let refresh = FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: true,
            min_interval_ms: 300_000,
            daily_limit: 96,
            failure_backoff_ms: 600_000,
            max_consecutive_failures: 3,
        },
        readers: vec![FactRefreshReader {
            instance_id: INSTANCE_A.to_owned(),
            package_id: READER.to_owned(),
            marked_keys: BTreeSet::from([SLOT_READY_AT.to_owned()]),
            dispatched_today: 0,
            last_failure_code: None,
            problem: None,
        }],
        instance_facts_loaded_at_unix_ms: Some(NOW - DAY_MS),
        instance_holds: BTreeMap::new(),
    };
    evaluate_with_fact_refresh::<PolicyEvaluationError>(
        catalog,
        &facts,
        &resources,
        EvaluationTime {
            unix_ms: now,
            monotonic_ms: now,
        },
        11,
        &BTreeSet::new(),
        &refresh,
        |_| Ok(CandidateEligibility::Eligible),
    )
    .unwrap_or_else(|error| panic!("the evaluation succeeds: {error}"))
}

fn decision(evaluation: &PolicyEvaluation) -> &TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == TASK_A && decision.instance_id.as_deref() == Some(INSTANCE_A)
        })
        .expect("task A's decision on instance A")
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
