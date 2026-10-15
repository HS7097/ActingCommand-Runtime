// SPDX-License-Identifier: AGPL-3.0-only

// POOL-1 gate tests (Workflow #390; forced-compromises mapping row POOL-1 and compromise S-04;
// register §1.76; policy gate key 49): a `ledger_fact` pool takes its data from readings. A
// reading of the pool's key is its base, which the next reading resets, and the pool's value is
// that base projected by the pool's regeneration rule: each complete period since the reading
// adds `amount`, up to `capacity` (contracts/scheduling/README.md, "Pool Projection"). A pool
// without a usable reading asks for one: its predicate raises the fact refresh's demand for the
// pool's key in the instance's scope, as a predicate on the key itself does (D2 model v1.4.2
// §3.3). A resource target on the pool reads the same projected inventory. Every expected value
// comes from the row, the compromise and the contracts named here.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CandidateEligibility, CatalogDocumentSource, CatalogSources, CompiledCatalog, DecisionReason,
    EligibilityState, EvaluationFacts, EvaluationResources, EvaluationTime, FACT_REFRESH_TASK_ID,
    FactRefreshInputs, FactRefreshReader, FactRefreshSettings, HostResourceSnapshot, ObservedFact,
    PolicyEvaluation, PolicyEvaluationError, SchedulingDecisionState, TaskDecision,
    compile_catalog, evaluate_with_fact_refresh, project_fact_pools, targetable_resources,
};

const A: &str = "instance-a";
const X: &str = "task.x";
const POOL: &str = "pool.stock";
// The pool's key: a realistic instance fact that recognition writes (D2 model §3.1).
const STOCK: &str = "resource.stock";
// A's recognition maintenance package; its marks write STOCK.
const READER: &str = "package.reader-a";
const CAPACITY: u64 = 100;
// The pool regenerates one unit per complete period of ten seconds.
const PERIOD_MS: u64 = 10_000;
// The task's trigger: the pool's projected value is at least this.
const THRESHOLD: i64 = 38;
// The pool's minimum confidence.
const MINIMUM_CONFIDENCE: u16 = 900;
const PENDING: &str = "fact_refresh_pending";
const REQUESTED: &str = "fact_refresh_requested";
const TRIGGER_UNKNOWN: &str = "trigger_unknown";

// A reading resets the pool's base and the pool regenerates from it: 35 read at T1 is 37 one
// millisecond before the third period ends and 38, the trigger's threshold, when it ends. A newer
// reading of 20 at T2 replaces the base: ten seconds later the pool is 21, not 35 plus five
// periods, and it reaches the threshold only after eighteen periods from T2.
#[test]
fn pool1_a_reading_is_the_live_pool_s_base_and_regenerates_by_the_pool_s_rule() {
    let catalog = pool_catalog(true);
    let first = reading(35, NOW, NOW + HOUR_MS);
    for (at, eligibility) in [
        (NOW + 3 * PERIOD_MS - 1, EligibilityState::False),
        (NOW + 3 * PERIOD_MS, EligibilityState::True),
    ] {
        let evaluation = evaluate(&catalog, vec![first.clone()], &no_reader(), at);
        assert_eq!(decision(&evaluation).eligibility, eligibility, "at {at}");
        assert_eq!(
            has_task_intent(&evaluation),
            eligibility == EligibilityState::True,
            "at {at}"
        );
    }

    let reset_at = NOW + 4 * PERIOD_MS;
    let newer = reading(20, reset_at, reset_at + HOUR_MS);
    for (at, eligibility) in [
        (reset_at + PERIOD_MS, EligibilityState::False),
        (reset_at + 18 * PERIOD_MS - 1, EligibilityState::False),
        (reset_at + 18 * PERIOD_MS, EligibilityState::True),
    ] {
        let evaluation = evaluate(&catalog, vec![newer.clone()], &no_reader(), at);
        assert_eq!(decision(&evaluation).eligibility, eligibility, "at {at}");
    }
}

// A live pool without a usable reading asks for one: the task its predicate gates waits under
// `fact_refresh_pending` for the pool's key, with the key's own unknown reason (no record, an
// expired one, one below the pool's minimum confidence, one outside 0..=capacity), and A's
// recognition maintenance package is requested for it. A static snapshot pool, which no reading
// supplies, raises no demand: its task stays `trigger_unknown` alone.
#[test]
fn pool1_a_live_pool_without_a_usable_reading_requests_the_reader_for_its_key() {
    let catalog = pool_catalog(true);
    let mut low_confidence = reading(35, NOW - 60_000, NOW + HOUR_MS);
    low_confidence.confidence_milli = MINIMUM_CONFIDENCE - 1;
    for (case, facts, unknown_reason) in [
        ("no reading", Vec::new(), "fact_missing"),
        (
            "an expired reading",
            vec![reading(35, NOW - 2 * HOUR_MS, NOW - HOUR_MS)],
            "fact_expired",
        ),
        (
            "a reading below the pool's minimum confidence",
            vec![low_confidence],
            "fact_low_confidence",
        ),
        (
            "a reading above the pool's capacity",
            vec![reading(150, NOW - 60_000, NOW + HOUR_MS)],
            "type_mismatch",
        ),
    ] {
        let evaluation = evaluate(&catalog, facts, &with_reader(), NOW);
        let waiting = decision(&evaluation);
        assert_eq!(waiting.state, SchedulingDecisionState::Deferred, "{case}");
        assert_eq!(refresh_codes(waiting), [PENDING], "{case}");
        let pending = coded(waiting, PENDING);
        assert_eq!(
            pending[0].outcome.value("fact_key"),
            Some(&json!(STOCK)),
            "{case}"
        );
        assert_eq!(
            pending[0].outcome.value("unknown_reason"),
            Some(&json!(unknown_reason)),
            "{case}"
        );
        assert_eq!(
            pending[0].outcome.value("package_id"),
            Some(&json!(READER)),
            "{case}"
        );
        let refresh = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == FACT_REFRESH_TASK_ID && intent.instance_id == A)
            .unwrap_or_else(|| panic!("{case}: the reader is requested"));
        let chain = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == refresh.reason_chain_id)
            .expect("the reader's reason chain");
        let requested = chain
            .reasons
            .iter()
            .filter(|reason| code_of(reason) == REQUESTED)
            .collect::<Vec<_>>();
        assert_eq!(requested.len(), 1, "{case}: the key set is the pool's key");
        assert_eq!(
            requested[0].outcome.value("fact_key"),
            Some(&json!(STOCK)),
            "{case}"
        );
        assert_eq!(
            requested[0].outcome.value("unknown_reason"),
            Some(&json!(unknown_reason)),
            "{case}"
        );
    }

    let evaluation = evaluate(&pool_catalog(false), Vec::new(), &with_reader(), NOW);
    let waiting = decision(&evaluation);
    assert_eq!(codes(waiting), [TRIGGER_UNKNOWN]);
    assert!(evaluation.dispatch_intents.is_empty());
    assert!(
        evaluation
            .decisions
            .iter()
            .all(|decision| decision.task_id != FACT_REFRESH_TASK_ID),
        "nothing requests the reader for a static snapshot pool"
    );
}

// A resource target on a live pool reads the pool's projected inventory: 35 read at T1 is 38
// after three periods and the capacity, 100, after an hour; its observation time stays the
// reading's. A static snapshot pool's target reads the record as observed.
#[test]
fn pool1_a_target_on_a_live_pool_reads_the_projected_inventory() {
    let base = reading(35, NOW, NOW + DAY_MS);
    for (live, at, current) in [
        (true, NOW + 3 * PERIOD_MS, 38),
        (true, NOW + HOUR_MS, 100),
        (false, NOW + 3 * PERIOD_MS, 35),
    ] {
        let catalog = pool_catalog(live);
        let facts = evaluation_facts(vec![base.clone()]);
        let (targetable, not_targetable) = targetable_resources(
            &catalog,
            &facts,
            &instance(A, &[]),
            EvaluationTime {
                unix_ms: at,
                monotonic_ms: at,
            },
        )
        .expect("the pool's targetable resources");
        assert!(not_targetable.is_empty(), "{not_targetable:?}");
        let [resource] = targetable.as_slice() else {
            panic!("one targetable resource: {targetable:?}");
        };
        assert_eq!(resource.resource, POOL);
        assert_eq!(
            resource.observation.current,
            Some(current),
            "live {live} at {at}"
        );
        assert_eq!(resource.observation.observed_at_unix_ms, Some(NOW));
        assert_eq!(resource.observation.pending, None);
    }
}

// A reading of the pool's key in A's scope, at a confidence above the pool's minimum.
fn reading(value: i64, observed_at: u64, expires_at: u64) -> ObservedFact {
    let mut reading = fact(A, STOCK, value, observed_at, Some(expires_at));
    reading.confidence_milli = 990;
    reading
}

// Task X, which produces the pool, waits for its projected value to reach THRESHOLD. The pool
// observes STOCK in A's scope; `live` makes it a `ledger_fact` pool, otherwise it is a static
// snapshot pool.
fn pool_catalog(live: bool) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.live-pool",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-live-pool"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let task = json!({
        "id": X,
        "scope": {"kind": "game", "game_id": "game-a"},
        "entrypoint": {"operation_id": "operation.run"},
        "procedure_ref": "procedure.x",
        "priority": 100,
        "trigger": {
            "kind": "resource_projection",
            "pool_id": POOL,
            "comparison": "greater_than_or_equal",
            "value": THRESHOLD
        },
        "feedback_stop": {
            "kind": "outcome",
            "task_id": X,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
        "produces": [{
            "pool_id": POOL,
            "direction": "produce",
            "amount": 1,
            "observation_source": "scan_verified",
            "confidence_milli": 1000
        }],
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
    let mut pool = json!({
        "id": POOL,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": CAPACITY,
        "projection": {"amount": 1, "per_ms": PERIOD_MS},
        "observation": {"kind": "fact", "fact_key": STOCK},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    });
    if live {
        pool["value_source"] = json!({
            "kind": "ledger_fact",
            "minimum_confidence_milli": MINIMUM_CONFIDENCE
        });
    }
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
            format!("memory://fixture/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", json!([task]))),
        pools: source("pools", &document("pools", json!([pool]))),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the pool catalog compiles: {:?}", failure.diagnostics()))
}

fn evaluation_facts(facts: Vec<ObservedFact>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-live-pool".to_owned(),
        facts,
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

// The fact refresh's starting defaults (D2 model §3.4) with `readers`; the facts were loaded a
// day before NOW.
fn refresh_inputs(readers: Vec<FactRefreshReader>) -> FactRefreshInputs {
    FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: true,
            min_interval_ms: 300_000,
            daily_limit: 96,
            failure_backoff_ms: 600_000,
            max_consecutive_failures: 3,
        },
        readers,
        instance_facts_loaded_at_unix_ms: Some(NOW - DAY_MS),
        instance_holds: BTreeMap::new(),
    }
}

fn no_reader() -> FactRefreshInputs {
    refresh_inputs(Vec::new())
}

// A's recognition maintenance package, which marks the pool's key.
fn with_reader() -> FactRefreshInputs {
    refresh_inputs(vec![FactRefreshReader {
        instance_id: A.to_owned(),
        package_id: READER.to_owned(),
        marked_keys: BTreeSet::from([STOCK.to_owned()]),
        dispatched_today: 0,
        last_failure_code: None,
        problem: None,
    }])
}

// One round at `now`. The pool values are those the Runtime derives from the facts (the host's
// projection of a `ledger_fact` pool); A has its own host and the Runtime admits every candidate.
fn evaluate(
    catalog: &CompiledCatalog,
    observed: Vec<ObservedFact>,
    inputs: &FactRefreshInputs,
    now: u64,
) -> PolicyEvaluation {
    let facts = evaluation_facts(observed);
    let resources = project_fact_pools(
        catalog,
        &facts,
        &EvaluationResources {
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
        },
    );
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
        inputs,
        |_| Ok(CandidateEligibility::Eligible),
    )
    .unwrap_or_else(|error| panic!("the round never fails: {error}"))
}

// Task X's decision on A.
fn decision(evaluation: &PolicyEvaluation) -> &TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == X && decision.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {X} on {A}: {evaluation:#?}"))
}

fn has_task_intent(evaluation: &PolicyEvaluation) -> bool {
    evaluation
        .dispatch_intents
        .iter()
        .any(|intent| intent.task_id == X && intent.instance_id == A)
}

fn code_of(reason: &DecisionReason) -> &str {
    reason.outcome.code().as_str()
}

fn codes(decision: &TaskDecision) -> Vec<&str> {
    decision.reasons.iter().map(code_of).collect()
}

fn refresh_codes(decision: &TaskDecision) -> Vec<&str> {
    codes(decision)
        .into_iter()
        .filter(|code| code.starts_with("fact_refresh_"))
        .collect()
}

fn coded<'a>(decision: &'a TaskDecision, code: &str) -> Vec<&'a DecisionReason> {
    decision
        .reasons
        .iter()
        .filter(|reason| code_of(reason) == code)
        .collect()
}
