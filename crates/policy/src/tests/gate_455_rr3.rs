// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 RR3 (0.12.95; register §1.538; the coordinator's rulings 1 to 11 of 11-28 in its
// work order, on the read-only study of derived training targets; contracts/resource-targets.md,
// "Needs computed from the records"), the evaluator's half. The host computes, before each
// evaluation, what the characters an instance's stored records name still need, and gives it to
// the evaluator in memory beside the left-out facts; here the needs are given as the host gives
// them:
// - the two layers stack: `max(0, need + stock − held)`, what is in hand going to the characters
//   first (the study's answer 五: need 30, stock 200, held 50 gives 180, held 10 gives 220; the
//   lower item of a recipe, need 24, stock 200, held 5, gives 219), with `goal_records_need` (t1);
// - an instance with needs and no stored goal is judged for goals all the same: the pool's
//   producer gains the shortfall weight, judged with the pool's own step and importance, and the
//   notes of the needs are said (t2);
// - the needs are part of each candidate's decision identity; an instance without them is
//   evaluated byte for byte as before (t3).
//
// The pools, tasks and ids are made up; nothing reads a clock.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::{
    CandidateEligibility, CappedMemory, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DispatchIntent, EvaluationFacts, EvaluationResources, EvaluationTime,
    FactRefreshInputs, FactRefreshSettings, FactValue, GivenPoolReadings, HostResourceSnapshot,
    InstanceHolds, InstancePoolLines, InstanceRecordNeeds, LeftOutFacts, NoProgressGuards,
    ObservedFact, OwedReruns, PolicyEvaluation, PolicyEvaluationError, PoolNeed, PoolNeedKind,
    RecordNeeds, RecordNeedsNote, RecordRef, RemedyPlan, RunSlots, SCHEDULING_SCHEMA_VERSION_V2,
    ScopeSelector, TaskDecision, check_resource_targets, compile_catalog,
    evaluate_with_left_out_facts, parse_resource_targets, project_fact_pools_with_readings,
};
use actingcommand_contract::RESOURCE_TARGETS_FACT_KEY;

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
/// The pool the needs land on, read from its own fact [`GOLD_FACT`].
const GOLD: &str = "pool.gold";
const GOLD_FACT: &str = "resource.gold";
/// The task that produces 20 of [`GOLD`] a run.
const TASK: &str = "task.gold";
/// The sha256 the fixture's needs name as their tables'.
const SHA: &str = "fixture-tables-a";
const V2_TARGETS: &str = "actingcommand.resource-targets.v2";

// Test 1, the stacking (rulings 4 and 5; the study's answer 五). With a stored goal of 200 on the
// pool and a computed need of 30, the stored goal asks 230 and keeps its own step and importance:
// held 50 the gap is 180 (30 of the 50 go to the characters, the stock is counted on the 20 left),
// held 10 it is 220. The lower item of a recipe, of which 24 are needed, stocked at 200 and held
// 5, has a gap of 219. Each covering candidate says the numbers in `goal_records_need`, naming the
// pool.
#[test]
fn rr3_t1_the_two_layers_stack_and_the_holding_goes_to_the_characters_first() {
    let catalog = gold_catalog();
    for (need, held, gap) in [(30_u64, 50_u64, 180_u64), (30, 10, 220), (24, 5, 219)] {
        let facts = facts_with(vec![stored_target(&catalog, 200), held_fact(held)]);
        let evaluation = round(
            &catalog,
            &facts,
            Some(needs(vec![(GOLD, need, PoolNeedKind::Lands)], Vec::new())),
        );
        let decided = decision(&evaluation, TASK);
        let applied = format!(
            "goal-g@{GOLD}:applied current={held} at_least={} gap={gap}",
            200 + need
        );
        assert!(
            detail_of(decided, "resource_targets").contains(&applied),
            "{applied}: {decided:#?}"
        );
        let reason = reason_of(decided, "goal_records_need");
        assert_eq!(
            reason.outcome.value("pool_id").and_then(Value::as_str),
            Some(GOLD)
        );
        assert_eq!(
            reason.detail,
            format!(
                "needed={need} stock=200 held={held} to_characters={} gap={gap} tables={SHA}",
                held.min(need)
            )
        );
    }
}

// Test 2, no stored goal (ruling 5). The instance holds no stored target: without computed needs
// its producer carries no `resource_targets` reason and its pool weighs its base alone (`gap=0`);
// with a need of 30 on a holding of 10 the pool is judged as if it held a goal of 30 with the
// pool's own step and importance (gap 20, the shortfall weight 100 × 20 / 1 = 2000), the producer
// carries `goal_records_need` with no stock, and the notes of the needs are said on it.
#[test]
fn rr3_t2_an_instance_with_records_and_no_stored_goal_is_judged_for_goals_and_pushes_the_producers()
{
    let catalog = gold_catalog();
    let facts = facts_with(vec![held_fact(10)]);
    let without = round(&catalog, &facts, None);
    let plain = decision(&without, TASK);
    assert!(
        plain
            .reasons
            .iter()
            .all(|reason| reason.outcome.code().as_str() != "resource_targets"),
        "{plain:#?}"
    );
    assert!(
        detail_of(plain, "resource_weights")
            .contains(&format!("{GOLD}:r=20,per=1,base=1000,gap=0")),
        "{plain:#?}"
    );
    let record = RecordRef {
        entry: "raise".to_owned(),
        key: "hero_a".to_owned(),
    };
    let notes = vec![RecordNeedsNote::PositionUnknown {
        record,
        column: "grade".to_owned(),
    }];
    let with = round(
        &catalog,
        &facts,
        Some(needs(vec![(GOLD, 30, PoolNeedKind::Lands)], notes)),
    );
    let pushed = decision(&with, TASK);
    let applied = format!("goal_records@{GOLD}:applied current=10 at_least=30 gap=20");
    assert!(
        detail_of(pushed, "resource_targets").contains(&applied),
        "{pushed:#?}"
    );
    assert!(
        detail_of(pushed, "resource_weights")
            .contains(&format!("{GOLD}:r=20,per=1,base=1000,gap=2000")),
        "{pushed:#?}"
    );
    assert_eq!(
        reason_of(pushed, "goal_records_need").detail,
        format!("needed=30 stock=0 held=10 to_characters=10 gap=20 tables={SHA}")
    );
    assert_eq!(
        detail_of(pushed, "goal_records_position_unknown"),
        "raise/hero_a/grade"
    );
}

// Test 3, the identity (ruling 6) and nothing moved (ruling 11). Given no needs, or an empty set,
// the evaluation is byte for byte the one without this slice's input. Given needs, the intent's
// decision identity is another; the same needs give the same identity, and another need, or the
// same need from other tables, gives another.
#[test]
fn rr3_t3_the_computed_set_is_in_the_decision_identity_and_an_instance_without_needs_is_as_before()
{
    let catalog = gold_catalog();
    let facts = facts_with(vec![held_fact(10)]);
    let plain = round(&catalog, &facts, None);
    assert_eq!(round(&catalog, &facts, Some(RecordNeeds::default())), plain);
    let given = |need: u64, sha: &str| {
        let mut computed = RecordNeeds::default();
        computed.give(
            A,
            InstanceRecordNeeds::new(
                Some(sha.to_owned()),
                NOW - HOUR_MS,
                BTreeMap::from([(
                    GOLD.to_owned(),
                    PoolNeed {
                        need,
                        kind: PoolNeedKind::Lands,
                    },
                )]),
                Vec::new(),
                None,
            ),
        );
        round(&catalog, &facts, Some(computed))
    };
    let first = intent_id(&given(30, SHA));
    assert_eq!(intent_id(&given(30, SHA)), first);
    assert_ne!(first, intent_id(&plain));
    assert_ne!(intent_id(&given(31, SHA)), first);
    assert_ne!(intent_id(&given(30, "fixture-tables-b")), first);
}

// ---- The fixture: V2, one instance A of the game

/// The needs of A: `pools` (pool, need, kind) and `notes`, of the tables [`SHA`], from records set
/// an hour before NOW, of no expiry.
fn needs(pools: Vec<(&str, u64, PoolNeedKind)>, notes: Vec<RecordNeedsNote>) -> RecordNeeds {
    let mut computed = RecordNeeds::default();
    computed.give(
        A,
        InstanceRecordNeeds::new(
            Some(SHA.to_owned()),
            NOW - HOUR_MS,
            pools
                .into_iter()
                .map(|(pool, need, kind)| (pool.to_owned(), PoolNeed { need, kind }))
                .collect(),
            notes,
            None,
        ),
    );
    computed
}

fn clock() -> Value {
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

fn task() -> Value {
    json!({
        "id": TASK,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{TASK}"),
        "priority": 100,
        "trigger": clock(),
        "feedback_stop": {
            "kind": "outcome",
            "task_id": TASK,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
        "produces": [{
            "pool_id": GOLD,
            "direction": "produce",
            "amount": 20,
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
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// [`GOLD`], A's, read from its fact, valued with the step 1, the base 1000 and the gap weight
/// 100.
fn gold_pool() -> Value {
    json!({
        "id": GOLD,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": 1_000_000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": GOLD_FACT},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {
            "name": "Fixture",
            "unit": "unit",
            "scale": 1,
            "base_weight_milli": 1000,
            "gap": {"rule": "shortfall_linear", "weight_milli": 100}
        }
    })
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-455-rr3/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

fn gold_catalog() -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.455-rr3",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-rr3"]
    });
    let document = |member: &str, entries: Vec<Value>| {
        let mut document = json!({
            "schema_version": SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = Value::Array(entries);
        document
    };
    let profile = json!({
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
    });
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", vec![task()])),
        pools: source("pools", &document("pools", vec![gold_pool()])),
        activity: source("activity", &document("profiles", vec![profile])),
        timeline: source("timeline", &document("events", Vec::new())),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the catalog compiles: {:?}", failure.diagnostics()))
}

/// A's reading of [`GOLD`]: `held`, read ten minutes before NOW, fresh for a day.
fn held_fact(held: u64) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: GOLD_FACT.to_owned(),
        value: FactValue::Integer(i64::try_from(held).expect("a small reading")),
        observed_at_unix_ms: NOW - 600_000,
        expires_at_unix_ms: Some(NOW + DAY_MS),
        confidence_milli: 1000,
    }
}

/// The record the formal entry stores for a v2 target `goal-g` of at least `stock` on [`GOLD`],
/// checked an hour before NOW.
fn stored_target(catalog: &CompiledCatalog, stock: u64) -> ObservedFact {
    let document = json!({
        "schema_version": V2_TARGETS,
        "instance": A,
        "valid_until_unix_ms": NOW + DAY_MS,
        "targets": [{
            "id": "goal-g",
            "resource": GOLD,
            "condition": {"kind": "at_least", "amount": stock},
            "apply": {"mode": "adjust", "weight": "score_stage"}
        }]
    });
    let parsed = parse_resource_targets(&serde_json::to_vec(&document).expect("bytes"))
        .expect("the document parses");
    let checked = check_resource_targets(
        &parsed,
        catalog,
        &facts_with(Vec::new()),
        time(NOW - HOUR_MS),
    )
    .expect("the document is accepted");
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: RESOURCE_TARGETS_FACT_KEY.to_owned(),
        value: FactValue::RecordList(
            serde_json::from_value(serde_json::to_value(&checked.rows).expect("rows"))
                .expect("record list"),
        ),
        observed_at_unix_ms: NOW - HOUR_MS,
        expires_at_unix_ms: document["valid_until_unix_ms"].as_u64(),
        confidence_milli: 1000,
    }
}

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

fn host() -> HostResourceSnapshot {
    HostResourceSnapshot {
        host_id: instance(A, &[]).host_id,
        cpu_available_milli: 1_000,
        gpu_available_milli: 1_000,
        io_available_milli: 1_000,
        host_responsiveness_basis_points: 10_000,
        third_party_pressure_basis_points: 0,
        heavy_dispatch_limit: 1,
        active_heavy_dispatches: 0,
        heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
    }
}

fn facts_with(facts: Vec<ObservedFact>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-455-rr3".to_owned(),
        facts,
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

/// One round at NOW, the fact refresh at its starting defaults with no reader, every candidate
/// admitted, with the needs `given` beside the left-out facts when there are some.
fn round(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    given: Option<RecordNeeds>,
) -> PolicyEvaluation {
    let resources = project_fact_pools_with_readings(
        catalog,
        facts,
        &EvaluationResources {
            pools: Vec::new(),
            hosts: vec![host()],
        },
        &GivenPoolReadings::default(),
    );
    let mut left_out = LeftOutFacts::default();
    if let Some(computed) = given {
        left_out.give_record_needs(computed);
    }
    let refresh = FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: true,
            min_interval_ms: 300_000,
            daily_limit: 96,
            failure_backoff_ms: 600_000,
            max_consecutive_failures: 3,
            expected_duration_ms: crate::FACT_REFRESH_EXPECTED_DURATION_MS,
        },
        readers: Vec::new(),
        instance_facts_loaded_at_unix_ms: Some(NOW - DAY_MS),
        instance_holds: BTreeMap::new(),
        idle_closed: BTreeMap::new(),
    };
    evaluate_with_left_out_facts::<PolicyEvaluationError>(
        catalog,
        facts,
        &resources,
        time(NOW),
        11,
        &BTreeSet::new(),
        &refresh,
        &RemedyPlan::default(),
        &ChainHandoverInputs::default(),
        &RunSlots::default(),
        &OwedReruns::default(),
        &InstanceHolds::default(),
        &NoProgressGuards::default(),
        &mut CappedMemory::default(),
        &InstancePoolLines::default(),
        &left_out,
        |_, _, _| Ok(false),
        |_: &DispatchIntent| Ok(CandidateEligibility::Eligible),
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == task_id && decision.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}: {evaluation:#?}"))
}

/// `decision`'s first reason of `code`.
fn reason_of<'a>(decision: &'a TaskDecision, code: &str) -> &'a crate::DecisionReason {
    decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == code)
        .unwrap_or_else(|| panic!("a {code} reason on {decision:#?}"))
}

/// The detail of `decision`'s first reason of `code`.
fn detail_of<'a>(decision: &'a TaskDecision, code: &str) -> &'a str {
    reason_of(decision, code).detail.as_str()
}

/// The decision identity of the round's intent of [`TASK`] on A.
fn intent_id(evaluation: &PolicyEvaluation) -> String {
    evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == TASK && intent.instance_id == A)
        .map(|intent| intent.decision_id.clone())
        .unwrap_or_else(|| panic!("an intent of {TASK} on {A}: {evaluation:#?}"))
}
