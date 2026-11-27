// SPDX-License-Identifier: AGPL-3.0-only

// #503 GOAL-1, the policy half (0.12.88; register §1.531; contracts/resource-targets.md, "A
// target's pool"): a target names its pool as the game root's files write it, and the one function
// that gives its pool (`target_pool`) gives, for one instance, the catalog pool of that id or the
// instance's composed copy `<id>@<alias>`. The catalog is neutral and V2, written as the composer
// writes it: for one instance of the game its pool and its task keep the file's ids; for two, each
// instance has its copies `<id>@<alias>`. The pool is valued (step 1, base 100, gap weight 700),
// of capacity 1000, read from its fact (10 on each instance); the task produces 2 of it a run,
// hourly, due at NOW. Every name is made up; no clock is read.

use std::collections::BTreeMap;

use actingcommand_contract::{
    FactScalar as ContractFactScalar, Initiator, RESOURCE_TARGETS_FACT_KEY,
    ResourceTargetCondition, ResourceTargetConditionState, ResourceTargetsRejectionReason,
    RuntimeChannel,
};
use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactValue, GivenPoolReadings, GoalRecordEdit, HostResourceSnapshot,
    ObservedFact, PolicyEvaluation, ResourceTargetsError, ScopeSelector, StoredResourceTargets,
    TargetSetter, active_resource_targets, check_resource_targets, compile_catalog,
    edit_goal_record, evaluate, parse_resource_targets, pool_capacity, project_fact_pools,
    targetable_resources,
};

const A: &str = "instance-a";
const B: &str = "instance-b";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
/// The pool as the game root's files write it.
const POOL: &str = "pool.neutral.stock";
const READING: &str = "resource.neutral.stock";
/// The task as the game root's files write it.
const TASK: &str = "task.neutral.produce";
/// The goal's target id.
const GOAL: &str = "stock";

type Rows = Vec<BTreeMap<String, ContractFactScalar>>;

/// The id the composer gives `id` on the instance `alias`: the file's id for one instance of the
/// game (`copies` false), `<id>@<alias>` for two.
fn on(id: &str, alias: &str, copies: bool) -> String {
    if copies {
        format!("{id}@{alias}")
    } else {
        id.to_owned()
    }
}

/// The composed catalog for `instances` of the game.
fn catalog(instances: &[&str]) -> CompiledCatalog {
    let copies = instances.len() > 1;
    let header = json!({
        "catalog_id": "fixture.503-goal1",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-503-goal1"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let mut tasks = Vec::new();
    let mut pools = Vec::new();
    for alias in instances {
        let task_id = on(TASK, alias, copies);
        tasks.push(json!({
            "id": task_id,
            "scope": {"kind": "instance", "instance_id": alias},
            "entrypoint": {"operation_id": OPERATION},
            "procedure_ref": format!("procedure.{TASK}"),
            "priority": 100,
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
            "produces": [{
                "pool_id": on(POOL, alias, copies),
                "direction": "produce",
                "amount": 2,
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
        }));
        pools.push(json!({
            "id": on(POOL, alias, copies),
            "scope": {"kind": "instance", "instance_id": alias},
            "capacity": 1000,
            "projection": {"kind": "none"},
            "observation": {"kind": "fact", "fact_key": READING},
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
            "valuation": {"name": "Stock", "unit": "piece", "scale": 1, "base_weight_milli": 100,
                          "gap": {"rule": "shortfall_linear", "weight_milli": 700}}
        }));
    }
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
    let source = |name: &str, document: &Value| {
        CatalogDocumentSource::new(
            format!("memory://fixture-503-goal1/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", Value::Array(pools))),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

/// The rows of instance A's record that one `EditGoal` of the target [`GOAL`] on [`POOL`], at least
/// 50, writes over no record: the same bytes whatever the catalog.
fn stored_rows() -> Rows {
    edit_goal_record(
        None,
        None,
        A,
        &GoalRecordEdit::SetTarget {
            id: GOAL.to_owned(),
            resource: POOL.to_owned(),
            at_least: 50,
        },
        TargetSetter {
            set_unix_ms: NOW - HOUR_MS,
            channel: RuntimeChannel::Lab,
            initiator: Some(Initiator::LabCli),
        },
    )
    .unwrap_or_else(|refusal| panic!("the edit is taken: {refusal:?}"))
    .composed
    .rows
}

/// Instance A's `session.resource_targets` record of `rows`, observed an hour before NOW.
fn record(rows: &Rows) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: RESOURCE_TARGETS_FACT_KEY.to_owned(),
        value: FactValue::RecordList(
            serde_json::from_value(serde_json::to_value(rows).expect("rows")).expect("record list"),
        ),
        observed_at_unix_ms: NOW - HOUR_MS,
        expires_at_unix_ms: None,
        confidence_milli: 1000,
    }
}

/// The projection for `instances`: each instance's reading of the pool, 10 a minute before NOW,
/// and the `extra` facts.
fn facts(instances: &[&str], extra: Vec<ObservedFact>) -> EvaluationFacts {
    let mut observed = instances
        .iter()
        .map(|alias| fact(alias, READING, 10, NOW - 60_000, Some(NOW + DAY_MS)))
        .collect::<Vec<_>>();
    observed.extend(extra);
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-503-goal1".to_owned(),
        facts: observed,
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: instances.iter().map(|alias| instance(alias, &[])).collect(),
        priority_offsets: Vec::new(),
    }
}

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

/// One round at NOW, each instance on its own host.
fn round(compiled: &CompiledCatalog, projection: &EvaluationFacts) -> PolicyEvaluation {
    let resources = project_fact_pools(
        compiled,
        projection,
        &EvaluationResources {
            pools: Vec::new(),
            hosts: projection
                .instances
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
        },
    );
    evaluate(compiled, projection, &resources, time(NOW), 11)
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

/// The `resource_targets` reason of the task's decision on `alias`, if it has one.
fn targets_reason(evaluation: &PolicyEvaluation, alias: &str, copies: bool) -> Option<String> {
    let task_id = on(TASK, alias, copies);
    let decision = evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(alias)
        })
        .unwrap_or_else(|| panic!("a decision of {task_id} on {alias}: {evaluation:#?}"));
    decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == "resource_targets")
        .map(|reason| reason.detail.clone())
}

/// The pools the instance `alias` can target, with their producing tasks, and the ones it cannot.
fn targetable(
    compiled: &CompiledCatalog,
    projection: &EvaluationFacts,
    alias: &str,
) -> (Vec<(String, Vec<String>)>, Vec<String>) {
    let (offered, refused) =
        targetable_resources(compiled, projection, &instance(alias, &[]), time(NOW))
            .unwrap_or_else(|error| panic!("the targetable list: {error:?}"));
    (
        offered
            .into_iter()
            .map(|pool| (pool.resource, pool.producing_tasks))
            .collect(),
        refused.into_iter().map(|pool| pool.resource).collect(),
    )
}

// The work order's fourth test, and ruling 2. The same stored rows (a v3 record of instance A whose
// target names the pool by the file's id) under the catalog composed for one instance of the game
// and under the one composed for two: on each, A's view of the record computes the target on A's
// pool (the file's id for one instance, A's copy for two), with its gap (at least 50 over the
// reading 10: 40); the capacity the goal page reads under the file's id is that pool's; the
// targetable list of A names the pool once by the file's id, with A's task, and lists nothing it
// cannot target (B's copy is no second pool of A); the evaluation applies the target to A's task
// with that gap. For two, B's list names the same pool by the same id with B's task, B has no
// record and B's task no target.
#[test]
fn goal1_t4_the_same_rows_give_the_instances_pool_for_one_instance_and_for_two() {
    let rows = stored_rows();
    for instances in [&[A][..], &[A, B][..]] {
        let copies = instances.len() > 1;
        let compiled = catalog(instances);
        let projection = facts(instances, vec![record(&rows)]);
        let label = instances.len();

        let StoredResourceTargets::Policy(policy) = active_resource_targets(
            Some(&compiled),
            &projection,
            &GivenPoolReadings::default(),
            &instance(A, &[]),
            time(NOW),
        )
        .unwrap_or_else(|error| panic!("{label}: the view: {error:?}")) else {
            panic!("{label}: a policy on A");
        };
        assert_eq!(
            policy.conditions,
            // Fix round 1 (the coordinator's ruling): a condition names its pool as the target
            // does, the file's id, for one instance and for two.
            [ResourceTargetCondition {
                target_id: GOAL.to_owned(),
                resource: POOL.to_owned(),
                fact_key: READING.to_owned(),
                state: ResourceTargetConditionState::Computed {
                    current: 10,
                    observed_at_unix_ms: NOW - 60_000,
                    gap: 40,
                },
            }],
            "{label}"
        );
        assert_eq!(policy.targets[0]["resource"], json!(POOL), "{label}");
        assert_eq!(
            pool_capacity(&compiled, &projection, &instance(A, &[]), POOL, time(NOW)),
            Some(1000),
            "{label}"
        );
        assert_eq!(
            targetable(&compiled, &projection, A),
            (
                vec![(POOL.to_owned(), vec![on(TASK, A, copies)])],
                Vec::new()
            ),
            "{label}"
        );

        let evaluation = round(&compiled, &projection);
        let applied = targets_reason(&evaluation, A, copies)
            .unwrap_or_else(|| panic!("{label}: A's task carries the target: {evaluation:#?}"));
        let expected = format!(
            "{GOAL}@{}:applied current=10 at_least=50 gap=40",
            on(POOL, A, copies)
        );
        assert!(applied.starts_with(&expected), "{label}: {applied}");

        if copies {
            assert_eq!(
                targetable(&compiled, &projection, B),
                (
                    vec![(POOL.to_owned(), vec![on(TASK, B, copies)])],
                    Vec::new()
                )
            );
            assert!(matches!(
                active_resource_targets(
                    Some(&compiled),
                    &projection,
                    &GivenPoolReadings::default(),
                    &instance(B, &[]),
                    time(NOW),
                ),
                Ok(StoredResourceTargets::None)
            ));
            assert_eq!(targets_reason(&evaluation, B, copies), None);
        }
    }
}

// The work order's fifth test, the policy half: under the catalog composed for two instances, a v3
// document of instance A that names the pool by the file's id is taken, its target on A's copy;
// one that names A's copy id or B's is refused `unknown_resource` at `/targets/0/resource` (no
// pool identifier), and so is a task named by a copy id `unknown_task` at `/targets/0/tasks/0`.
#[test]
fn goal1_t5_a_document_names_the_files_id_and_a_copy_id_is_refused() {
    let compiled = catalog(&[A, B]);
    let projection = facts(&[A, B], Vec::new());
    let check = |target: Value| {
        let document = json!({
            "schema_version": "actingcommand.resource-targets.v3",
            "instance": A,
            "targets": [target]
        });
        let parsed = parse_resource_targets(&serde_json::to_vec(&document).expect("bytes"))
            .unwrap_or_else(|error| panic!("the document parses: {error:?}"));
        check_resource_targets(&parsed, &compiled, &projection, time(NOW))
    };
    let target = |resource: &str| {
        json!({
            "id": "floor",
            "resource": resource,
            "condition": {"kind": "at_least", "amount": 50},
            "apply": {"mode": "adjust", "weight": "score_stage"}
        })
    };

    let checked = check(target(POOL)).unwrap_or_else(|error| panic!("taken: {error:?}"));
    assert_eq!(
        checked
            .conditions
            .iter()
            .map(|condition| (condition.target_id.as_str(), condition.resource.clone()))
            .collect::<Vec<_>>(),
        // Fix round 1: the receipt's condition names the pool as the document does.
        [("floor", POOL.to_owned())]
    );

    for refused in [on(POOL, A, true), on(POOL, B, true)] {
        let Err(ResourceTargetsError::Rejected(rejection)) = check(target(refused.as_str())) else {
            panic!("{refused} is refused");
        };
        assert_eq!(
            (rejection.field_path.as_str(), rejection.reason),
            (
                "/targets/0/resource",
                ResourceTargetsRejectionReason::UnknownResource
            ),
            "{refused}"
        );
    }

    let mut named = target(POOL);
    named["tasks"] = json!([on(TASK, A, true)]);
    let Err(ResourceTargetsError::Rejected(rejection)) = check(named) else {
        panic!("a task named by a copy id is refused");
    };
    assert_eq!(
        (rejection.field_path.as_str(), rejection.reason),
        (
            "/targets/0/tasks/0",
            ResourceTargetsRejectionReason::UnknownTask
        )
    );
}
