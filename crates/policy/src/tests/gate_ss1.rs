// SPDX-License-Identifier: AGPL-3.0-only

// SS-1 gate tests (0.12.30; register §1.211; coordinator work order `rd-1029/brief-wave3a.md`
// SS-1, design `rd-1029/design-sch-state.md` §3, §4.1, §4.2 and §5 SS-1; Alice's rulings 100 and
// 102): a pool's declared `valuation` weighs every producing task on every instance, with or
// without a resource target record; a goal only adds its shortfall term on top, and a met goal
// only stops adding it. The resource layer reads no task enablement, instance override,
// priority, offset or run state, so none of them changes a pool weight or another task's term.
// The expected values are the work order's: a task's resource term without a goal is
// `per-run yield × base / scale` (contracts/resource-targets.md, "Resource weights").
//
// The catalog is neutral: tasks on one instance, each triggered by an hourly clock that is due at
// NOW, and two valued pools of the instance that take their amount from readings of their keys.

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactValue, HostResourceSnapshot, ObservedFact, PolicyEvaluation,
    PriorityOffset, PriorityOffsetOrigin, SchedulingDecisionState, ScopeSelector, TaskDecision,
    check_resource_targets, compile_catalog, evaluate, parse_resource_targets, project_fact_pools,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const TASK_A: &str = "task_a";
const TASK_B: &str = "task_b";
/// Valued at `scale` 10, `base_weight_milli` 500, gap weight 100; targetable.
const POOL_A: &str = "pool_a";
const POOL_A_KEY: &str = "resource.pool_a";
/// Valued at `scale` 1, `base_weight_milli` 7, no gap block.
const POOL_B: &str = "pool_b";
const POOL_B_KEY: &str = "resource.pool_b";
const V1: &str = "actingcommand.resource-targets.v1";
const V2: &str = "actingcommand.resource-targets.v2";

/// Task A's base term: per-run yield 20 × base 500 / scale 10.
const TASK_A_BASE_TERM: u64 = 20 * 500 / 10;

// Work order test 1: with no target record, a task producing a valued pool gets the resource term
// per-run yield × base / scale. Task A yields 20 of pool A per run (base 500, scale 10): 1000.
// It takes part in the score stage like any weighed candidate: the term is its effective score,
// `score_computed` names it and the item shows the pool's weight with no gap part.
#[test]
fn ss1_t1_without_a_target_record_a_producing_task_gets_yield_times_base_over_scale() {
    let catalog = catalog(&[task_a(), task_b()]);
    let evaluation = round(&catalog, Vec::new(), Vec::new());
    let detail = weights(&evaluation, TASK_A);
    assert!(detail.starts_with("policy=none "), "{detail}");
    assert_eq!(term(&evaluation, TASK_A), TASK_A_BASE_TERM, "{detail}");
    assert_eq!(
        items(&evaluation, TASK_A),
        "pool_a:r=20,per=10,base=500,gap=0,effective=500,term=1000",
        "{detail}"
    );
    let task = decision(&evaluation, TASK_A);
    assert_eq!(
        task.rank.as_ref().map(|rank| rank.effective_milli),
        Some(1000),
        "{task:#?}"
    );
    assert!(
        task.reasons.iter().any(|reason| {
            reason.outcome.code().as_str() == "score_computed"
                && reason.detail.ends_with(" resources=1000")
        }),
        "{task:#?}"
    );
    // Task B yields 30 of pool A and 3 of pool B: 30 × 500 / 10 + 3 × 7 / 1.
    assert_eq!(term(&evaluation, TASK_B), 1500 + 21);
}

// Work order test 2: disabling, by instance override, another task that produces the same pool
// leaves the first task's resource term unchanged: without a record (the base alone), under a v2
// target on the pool with an open gap (base plus shortfall), and under a v1 target that names both
// tasks, in `adjust` and in `override` mode (whose score `s = ⌊g·I·u / (S·U)⌋` keeps the disabled
// task's yield in `U`).
#[test]
fn ss1_t2_disabling_another_producer_by_instance_override_leaves_the_resource_term_unchanged() {
    let enabled = catalog(&[task_a(), task_b()]);
    let disabled = catalog(&[task_a(), disabled_on_a(task_b())]);
    let inventory = || vec![reading(POOL_A_KEY, 50)];
    let v2 = stored(&enabled, v2_target(None), NOW - HOUR_MS);
    let v1 = stored(&enabled, v1_target(&[TASK_A, TASK_B]), NOW - HOUR_MS);
    let v1_override = stored(
        &enabled,
        v1_override_target(&[TASK_A, TASK_B]),
        NOW - HOUR_MS,
    );
    for (label, record) in [
        ("no record", None),
        ("v2", Some(v2)),
        ("v1", Some(v1)),
        ("v1 override", Some(v1_override)),
    ] {
        let facts = || {
            inventory()
                .into_iter()
                .chain(record.clone())
                .collect::<Vec<_>>()
        };
        let before = round(&enabled, facts(), Vec::new());
        let after = round(&disabled, facts(), Vec::new());
        assert_eq!(
            decision(&after, TASK_B).state,
            SchedulingDecisionState::Blocked,
            "{label}: {after:#?}"
        );
        assert_eq!(
            weights(&before, TASK_A),
            weights(&after, TASK_A),
            "{label}: task A's resource term and pool weights"
        );
        assert_eq!(
            effective(&before, TASK_A),
            effective(&after, TASK_A),
            "{label}"
        );
    }
    // The three cases weigh what the contract says: base 1000; base plus `Γ = ⌊100 · 50 / 10⌋ =
    // 500` on 20 per run (20 × 1000 / 10 = 2000); v1 `s = ⌊50 · 1000 · 20 / (10 · 30)⌋ = 3333`
    // on top of the base 1000, with the disabled task B's 30 still in `U`.
    let v2 = stored(&enabled, v2_target(None), NOW - HOUR_MS);
    let v1 = stored(&enabled, v1_target(&[TASK_A, TASK_B]), NOW - HOUR_MS);
    let after =
        |record: ObservedFact| round(&disabled, vec![reading(POOL_A_KEY, 50), record], Vec::new());
    assert_eq!(
        term(&round(&disabled, inventory(), Vec::new()), TASK_A),
        1000
    );
    assert_eq!(term(&after(v2), TASK_A), 2000);
    let v1_round = after(v1);
    assert_eq!(term(&v1_round, TASK_A), 1000);
    assert_eq!(effective(&v1_round, TASK_A), 3333 + 1000);
    // v1 `override`: `effective = utility + s + R` (contracts/resource-targets.md, "Base valuation
    // on every instance"). The override supersedes task A's manual offset of 250000 and the
    // utility term is 0 here, so the effective score is the same `s = 3333` plus the base 1000;
    // the disposition is `none`.
    let overridden = round(
        &disabled,
        vec![
            reading(POOL_A_KEY, 50),
            stored(
                &enabled,
                v1_override_target(&[TASK_A, TASK_B]),
                NOW - HOUR_MS,
            ),
        ],
        offset(TASK_A, 250_000),
    );
    assert_eq!(term(&overridden, TASK_A), 1000);
    assert_eq!(effective(&overridden, TASK_A), 3333 + 1000);
    assert_eq!(
        reason_detail(&overridden, TASK_A, "score_computed"),
        "score=none offset=250000 effective=4333 disposition=none \
         target=goal-a:override:3333 resources=1000"
    );
    assert_eq!(
        reason_detail(&overridden, TASK_A, "resource_target_override"),
        "superseded score=none offset=250000; utility kept"
    );
}

// Work order test 3: changing a task's priority or offset leaves every pool weight unchanged.
// Without a target record (the base term alone) and under a v2 target with an open gap on pool A,
// every item of every task (yield, step, base, gap, effective weight and term) and every task's
// term stay byte for byte as they were when task B's priority changes, when task B gets a manual
// offset and when task A gets one.
#[test]
fn ss1_t3_a_task_priority_or_offset_changes_no_pool_weight() {
    let base = catalog(&[task_a(), task_b()]);
    let reprioritized = catalog(&[task_a(), with_priority(task_b(), 900)]);
    let v2 = stored(&base, v2_target(None), NOW - HOUR_MS);
    // The weights each reference shows, task A's then task B's, with their terms: without a
    // record pool A `W = 500` and pool B `W = 7`; under the v2 target pool A `W = 500 + 500`.
    let without_record = [
        (
            "pool_a:r=20,per=10,base=500,gap=0,effective=500,term=1000",
            1000,
        ),
        (
            "pool_a:r=30,per=10,base=500,gap=0,effective=500,term=1500;\
             pool_b:r=3,per=1,base=7,gap=0,effective=7,term=21",
            1500 + 21,
        ),
    ];
    let under_v2 = [
        (
            "pool_a:r=20,per=10,base=500,gap=500,effective=1000,term=2000",
            2000,
        ),
        (
            "pool_a:r=30,per=10,base=500,gap=500,effective=1000,term=3000;\
             pool_b:r=3,per=1,base=7,gap=0,effective=7,term=21",
            3000 + 21,
        ),
    ];
    for (record_label, record, pinned) in [
        ("no record", None, without_record),
        ("v2", Some(v2), under_v2),
    ] {
        let facts = || {
            std::iter::once(reading(POOL_A_KEY, 50))
                .chain(record.clone())
                .collect::<Vec<_>>()
        };
        let reference = round(&base, facts(), Vec::new());
        for (task_id, (pinned_items, pinned_term)) in [TASK_A, TASK_B].into_iter().zip(pinned) {
            assert_eq!(
                items(&reference, task_id),
                pinned_items,
                "{record_label}: {task_id}"
            );
            assert_eq!(
                term(&reference, task_id),
                pinned_term,
                "{record_label}: {task_id}"
            );
        }
        let variants = [
            (
                "task B's priority",
                round(&reprioritized, facts(), Vec::new()),
            ),
            (
                "task B's offset",
                round(&base, facts(), offset(TASK_B, 250_000)),
            ),
            (
                "task A's offset",
                round(&base, facts(), offset(TASK_A, -250_000)),
            ),
        ];
        for (label, variant) in &variants {
            for task_id in [TASK_A, TASK_B] {
                assert_eq!(
                    items(&reference, task_id),
                    items(variant, task_id),
                    "{record_label}, {label}: the pool weights of {task_id}"
                );
                assert_eq!(
                    term(&reference, task_id),
                    term(variant, task_id),
                    "{record_label}, {label}: {task_id}"
                );
            }
        }
    }
}

// Work order test 4 (ruling 102): after a goal is met, its producing task stays a candidate;
// only the shortfall term is gone. Task A alone produces pool A under a v2 target `at_least`
// 100. At 50 (gap 50) its term is 20 × (500 + 500) / 10 = 2000; at 100 the target is satisfied,
// the term falls to the base 1000 and the task is still dispatched.
#[test]
fn ss1_t4_a_met_goal_keeps_its_producing_task_a_candidate_with_only_the_base_term() {
    let catalog = catalog(&[task_a()]);
    let record = stored(&catalog, v2_target(None), NOW - HOUR_MS);
    let open = round(
        &catalog,
        vec![reading(POOL_A_KEY, 50), record.clone()],
        Vec::new(),
    );
    let met = round(&catalog, vec![reading(POOL_A_KEY, 100), record], Vec::new());
    assert_eq!(term(&open, TASK_A), 2000);
    assert_eq!(term(&met, TASK_A), TASK_A_BASE_TERM);
    assert_eq!(
        items(&met, TASK_A),
        "pool_a:r=20,per=10,base=500,gap=0,effective=500,term=1000"
    );
    for evaluation in [&open, &met] {
        assert_eq!(dispatched(evaluation), [TASK_A], "{evaluation:#?}");
    }
    let task = decision(&met, TASK_A);
    assert!(
        task.reasons.iter().any(|reason| {
            reason.outcome.code().as_str() == "resource_targets"
                && reason.detail.contains("goal-a@pool_a:satisfied")
        }),
        "{task:#?}"
    );
}

// Work order test 5: an expired or withdrawn record keeps the base term. So do an unreadable
// record and an active v1 record whose target names another task (work order item 1: no goal
// record of any kind removes the base valuation).
#[test]
fn ss1_t5_an_expired_withdrawn_unreadable_or_v1_record_keeps_the_base_term() {
    let catalog = catalog(&[task_a(), task_b()]);
    let expired = {
        let document = json!({
            "schema_version": V2,
            "instance": A,
            "valid_until_unix_ms": NOW - HOUR_MS,
            "targets": [v2_target_entry(None)],
        });
        stored_document(&catalog, &document, NOW - 2 * HOUR_MS)
    };
    let withdrawn = stored_document(
        &catalog,
        &json!({"schema_version": V2, "instance": A, "targets": []}),
        NOW - HOUR_MS,
    );
    let unreadable = ObservedFact {
        value: FactValue::Integer(1),
        ..withdrawn.clone()
    };
    let v1 = stored(&catalog, v1_target(&[TASK_B]), NOW - HOUR_MS);
    for (label, record, policy_reason) in [
        ("expired", expired, Some("resource_target_policy_expired")),
        ("withdrawn", withdrawn, None),
        (
            "unreadable",
            unreadable,
            Some("resource_target_policy_unreadable"),
        ),
        ("v1", v1, None),
    ] {
        let evaluation = round(&catalog, vec![reading(POOL_A_KEY, 50), record], Vec::new());
        let detail = weights(&evaluation, TASK_A);
        assert!(detail.starts_with("policy=none "), "{label}: {detail}");
        assert_eq!(term(&evaluation, TASK_A), TASK_A_BASE_TERM, "{label}");
        assert_eq!(effective(&evaluation, TASK_A), 1000, "{label}");
        if let Some(code) = policy_reason {
            let task = decision(&evaluation, TASK_A);
            assert!(
                task.reasons
                    .iter()
                    .any(|reason| reason.outcome.code().as_str() == code),
                "{label}: {task:#?}"
            );
        }
    }
}

// ---- Fixture.

fn task_a() -> Value {
    task(TASK_A, 100, &[(POOL_A, 20)])
}

fn task_b() -> Value {
    task(TASK_B, 200, &[(POOL_A, 30), (POOL_B, 3)])
}

/// `task` with an instance override that disables it on the instance.
fn disabled_on_a(mut task: Value) -> Value {
    task["instance_overrides"] = json!([{
        "instance_id": A,
        "enabled": false,
        "priority": null,
        "strategic_weight_milli": null,
        "load_profile": null
    }]);
    task
}

fn with_priority(mut task: Value, priority: i16) -> Value {
    task["priority"] = json!(priority);
    task
}

/// The fixture catalog, `actingcommand.scheduling.v2`, with these tasks and both valued pools.
fn catalog(tasks: &[Value]) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.base-valuation",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-base-valuation"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let pools = json!([
        pool(
            POOL_A,
            POOL_A_KEY,
            json!({"name": "Material A", "unit": "piece", "scale": 10, "base_weight_milli": 500,
                "gap": {"rule": "shortfall_linear", "weight_milli": 100}})
        ),
        pool(
            POOL_B,
            POOL_B_KEY,
            json!({"name": "Material B", "unit": "piece", "scale": 1, "base_weight_milli": 7})
        ),
    ]);
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
        tasks: source("tasks", &document("tasks", Value::Array(tasks.to_vec()))),
        pools: source("pools", &document("pools", pools)),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

fn pool(id: &str, fact_key: &str, valuation: Value) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": 1000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": fact_key},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": valuation
    })
}

fn task(id: &str, priority: i16, produces: &[(&str, u64)]) -> Value {
    let produces = produces
        .iter()
        .map(|(pool_id, amount)| {
            json!({
                "pool_id": pool_id,
                "direction": "produce",
                "amount": amount,
                "observation_source": "scan_verified",
                "confidence_milli": 1000
            })
        })
        .collect::<Vec<_>>();
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
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
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
        "produces": produces,
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

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

/// A v2 target `goal-a` on pool A, `at_least` 100, step and importance from the valuation.
fn v2_target_entry(tasks: Option<&[&str]>) -> Value {
    let mut target = json!({
        "id": "goal-a",
        "resource": POOL_A,
        "condition": {"kind": "at_least", "amount": 100},
        "apply": {"mode": "adjust", "weight": "score_stage"}
    });
    if let Some(tasks) = tasks {
        target["tasks"] = json!(tasks);
    }
    target
}

fn v2_target(tasks: Option<&[&str]>) -> Value {
    json!({
        "schema_version": V2,
        "instance": A,
        "valid_until_unix_ms": NOW + DAY_MS,
        "targets": [v2_target_entry(tasks)],
    })
}

/// A v1 target `goal-a` on pool A naming `tasks`: `at_least` 100, scale 10, importance 1000.
fn v1_target(tasks: &[&str]) -> Value {
    json!({
        "schema_version": V1,
        "instance": A,
        "valid_until_unix_ms": NOW + DAY_MS,
        "targets": [{
            "id": "goal-a",
            "resource": POOL_A,
            "condition": {"kind": "at_least", "amount": 100},
            "scale": 10,
            "importance_milli": 1000,
            "rule": "shortfall_linear",
            "apply": {"mode": "adjust", "weight": "score_stage"},
            "tasks": tasks
        }]
    })
}

/// `v1_target` in `override` mode.
fn v1_override_target(tasks: &[&str]) -> Value {
    let mut document = v1_target(tasks);
    document["targets"][0]["apply"]["mode"] = json!("override");
    document
}

/// The record the formal entry stores for `document`, checked against `catalog` at `checked_at`.
fn stored(catalog: &CompiledCatalog, document: Value, checked_at: u64) -> ObservedFact {
    stored_document(catalog, &document, checked_at)
}

fn stored_document(catalog: &CompiledCatalog, document: &Value, checked_at: u64) -> ObservedFact {
    let parsed = parse_resource_targets(&serde_json::to_vec(document).expect("document bytes"))
        .unwrap_or_else(|error| panic!("the fixture document parses: {error:?}"));
    let checked = check_resource_targets(
        &parsed,
        catalog,
        &inputs(Vec::new(), Vec::new()),
        time(checked_at),
    )
    .unwrap_or_else(|error| panic!("the fixture document is accepted: {error:?}"));
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: actingcommand_contract::RESOURCE_TARGETS_FACT_KEY.to_owned(),
        value: FactValue::RecordList(
            serde_json::from_value(serde_json::to_value(&checked.rows).expect("rows"))
                .expect("record list"),
        ),
        observed_at_unix_ms: checked_at,
        expires_at_unix_ms: document["valid_until_unix_ms"].as_u64(),
        confidence_milli: 1000,
    }
}

/// A reading of a pool's key on the instance, a minute before NOW, fresh for a day.
fn reading(fact_key: &str, value: i64) -> ObservedFact {
    fact(A, fact_key, value, NOW - 60_000, Some(NOW + DAY_MS))
}

/// A person's manual offset on `task_id`, set an hour before NOW.
fn offset(task_id: &str, offset_milli: i32) -> Vec<PriorityOffset> {
    vec![PriorityOffset {
        task_id: task_id.to_owned(),
        instance_id: None,
        offset_milli,
        origin: PriorityOffsetOrigin::User,
        observed_at_unix_ms: NOW - HOUR_MS,
    }]
}

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

fn inputs(facts: Vec<ObservedFact>, priority_offsets: Vec<PriorityOffset>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-base-valuation".to_owned(),
        facts,
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[])],
        priority_offsets,
    }
}

/// One round at NOW. The pools' values are the ones the Runtime derives from the readings; the
/// instance has its own host and every candidate is admitted.
fn round(
    catalog: &CompiledCatalog,
    facts: Vec<ObservedFact>,
    priority_offsets: Vec<PriorityOffset>,
) -> PolicyEvaluation {
    let facts = inputs(facts, priority_offsets);
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
    evaluate(catalog, &facts, &resources, time(NOW), 11)
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

fn dispatched(evaluation: &PolicyEvaluation) -> Vec<&'static str> {
    [TASK_A, TASK_B]
        .into_iter()
        .filter(|task_id| {
            evaluation
                .dispatch_intents
                .iter()
                .any(|intent| intent.task_id == *task_id && intent.instance_id == A)
        })
        .collect()
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == task_id && decision.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}: {evaluation:#?}"))
}

/// The candidate's `resource_weights` detail: `policy=<…> term=<R>[ capped] items=<…>`.
fn weights<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a str {
    let task = decision(evaluation, task_id);
    task.reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == "resource_weights")
        .map(|reason| reason.detail.as_str())
        .unwrap_or_else(|| panic!("a resource_weights reason on {task:#?}"))
}

/// The detail of the candidate's first reason with `code`.
fn reason_detail<'a>(evaluation: &'a PolicyEvaluation, task_id: &str, code: &str) -> &'a str {
    let task = decision(evaluation, task_id);
    task.reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == code)
        .map(|reason| reason.detail.as_str())
        .unwrap_or_else(|| panic!("a {code} reason on {task:#?}"))
}

/// The candidate's resource term `R`.
fn term(evaluation: &PolicyEvaluation, task_id: &str) -> u64 {
    let detail = weights(evaluation, task_id);
    detail
        .split_once(" term=")
        .and_then(|(_, rest)| rest.split(' ').next())
        .and_then(|term| term.parse().ok())
        .unwrap_or_else(|| panic!("a term in {detail}"))
}

/// The candidate's weighed pools, each `<pool>:r=…,per=…,base=…,gap=…,effective=<W>,term=…`.
fn items<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a str {
    let detail = weights(evaluation, task_id);
    detail
        .split_once(" items=")
        .map(|(_, items)| items)
        .unwrap_or_else(|| panic!("items in {detail}"))
}

fn effective(evaluation: &PolicyEvaluation, task_id: &str) -> i64 {
    let task = decision(evaluation, task_id);
    task.rank
        .as_ref()
        .map(|rank| rank.effective_milli)
        .unwrap_or_else(|| panic!("a rank on {task:#?}"))
}
