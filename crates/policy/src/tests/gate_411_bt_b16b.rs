// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B16 part 2 gate tests (slice BT-B16b; plan `rd-1021/train-0129/b-plan/plan.md`
// §2.12 and §5.3 BT-B16b T1 to T3; policy gate keys 64 to 66; register §1.167): a V2 catalog
// task may declare `reserves: [{pool_id, at_least}]`, the amount each pool must still hold after
// the task. The score stage computes `after = projected − consumes` per pool and hands the
// selection document `task.consumes.<pool>`, `task.after.<pool>` and `task.reserve_gap.<pool>`;
// `after < at_least` is a hard gate with the reason `resource_reserve_insufficient` (info). Two
// tasks that fit alone but not together: the higher `effective_milli` runs this round, and the
// other is scored again after a reading refreshes the pool. Every expected value comes from §2.12
// and the draft it quotes ("做完后某种资源至少留多少", "剩得不够保留额的任务，这一轮不派",
// "两个任务单做都够、同做不够时，分高的先做；它做完、重新读到资源后，再给另一个打分",
// "保留额是硬门槛").
//
// The catalog is neutral: tasks on one instance, each triggered by an hourly clock that is due at
// NOW, and one pool of the instance that takes its amount from readings of its key
// (`ledger_fact`) and does not regenerate.

use actingcommand_contract::outcome::Category;
use serde_json::{Value, json};

use super::selection_catalog::{
    DAY_MS, HOUR_MS, NOW, drop_candidate, fact, field_ref, identity_term, instance, integer_field,
    selection,
};
use crate::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, DecisionReason, EvaluationFacts,
    EvaluationResources, EvaluationTime, HostResourceSnapshot, ObservedFact, PolicyEvaluation,
    PriorityOffset, PriorityOffsetOrigin, SchedulingDecisionState, TaskDecision,
    TaskRuntimeSnapshot, TaskTerminalState, compile_catalog, evaluate, project_fact_pools,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const TASK_A: &str = "task_a";
const TASK_B: &str = "task_b";
const POOL: &str = "pool_a";
/// The pool's key: an instance fact that a reading writes.
const POOL_KEY: &str = "resource.pool_a";
const RESERVE_INSUFFICIENT: &str = "resource_reserve_insufficient";

// §2.12 and T1: `after = projected − consumes`, and `after < at_least` is a hard gate. Task A
// consumes 60 and must leave at least 50. A reading of 100 leaves 40 after it: the task is not
// admitted this round, with `resource_reserve_insufficient` naming the pool, the reserve and what
// the task would leave. A reading of 110 leaves exactly 50, which is not below the reserve, and
// one of 120 leaves 60: the task runs. No selection document is needed for the gate.
#[test]
fn bt_b16b_t1_a_task_that_would_leave_its_pool_below_the_reserve_is_not_admitted() {
    let catalog = catalog(
        &[TaskDecl {
            id: TASK_A,
            priority: 100,
            consumes: 60,
            at_least: 50,
        }],
        None,
    );
    for (reading, admitted) in [(100, false), (110, true), (120, true)] {
        let evaluation = round(
            &catalog,
            &[],
            vec![pool_reading(reading, NOW - 60_000)],
            Vec::new(),
            NOW,
        );
        assert_eq!(
            dispatched(&evaluation),
            if admitted { vec![TASK_A] } else { Vec::new() },
            "reading {reading}: {evaluation:#?}"
        );
        if admitted {
            continue;
        }
        let task = decision(&evaluation, TASK_A);
        assert_ne!(task.state, SchedulingDecisionState::Selected, "{task:#?}");
        let why = last_reason(task);
        assert_eq!(
            why.outcome.code().as_str(),
            RESERVE_INSUFFICIENT,
            "{task:#?}"
        );
        assert_eq!(why.outcome.category(), Category::Info, "{why:?}");
        assert_eq!(why.outcome.value("pool_id"), Some(&json!(POOL)), "{why:?}");
        assert_eq!(
            why.outcome.value("reserve_at_least"),
            Some(&json!(50)),
            "{why:?}"
        );
        assert_eq!(
            why.outcome.value("amount_after"),
            Some(&json!(40)),
            "{why:?}"
        );
        assert_eq!(why.outcome.value("task_id"), None, "no other task: {why:?}");
    }
}

// §2.12 and T2: task A (consumes 60) and task B (consumes 50) each must leave at least 20 in the
// pool. A reading of 100 lets either run alone (A leaves 40, B leaves 50) but not both (they would
// leave −10). Task A has the higher priority, task B the higher `effective_milli` (a manual offset
// of 50 milli). Each round is fed the dispatches of the rounds before it:
// - round 1: the higher `effective_milli` runs this round, so B is dispatched and A waits with
//   `resource_reserve_insufficient`, naming B and the −10 the pool would hold after both;
// - round 2, B's run is over and no reading has come since: the pool's 100 does not show B's
//   consumption yet, so A still waits for B (−10);
// - round 3, a reading of 85 taken after B's dispatch refreshes the pool: A is scored again on it,
//   leaves 25, and runs.
// Before the refresh, never both run.
#[test]
fn bt_b16b_t2_of_two_tasks_that_fit_alone_but_not_together_the_higher_score_runs_first() {
    let catalog = catalog(
        &[
            TaskDecl {
                id: TASK_A,
                priority: 200,
                consumes: 60,
                at_least: 20,
            },
            TaskDecl {
                id: TASK_B,
                priority: 100,
                consumes: 50,
                at_least: 20,
            },
        ],
        None,
    );
    let first_reading = pool_reading(100, NOW - 60_000);
    let refresh_at = NOW + 8 * 60_000;
    let rounds = [
        (NOW, first_reading.clone()),
        (NOW + 5 * 60_000, first_reading),
        (NOW + 10 * 60_000, pool_reading(85, refresh_at)),
    ];
    let mut history: Vec<TaskRuntimeSnapshot> = Vec::new();
    let mut evaluations = Vec::new();
    for (at, reading) in rounds {
        let evaluation = round(&catalog, &history, vec![reading], offset_of_b(), at);
        for task_id in dispatched(&evaluation) {
            history.retain(|state| state.task_id != task_id);
            history.push(dispatched_state(task_id, at));
        }
        evaluations.push(evaluation);
    }
    assert_eq!(
        evaluations.iter().map(dispatched).collect::<Vec<_>>(),
        [vec![TASK_B], Vec::new(), vec![TASK_A]],
        "{evaluations:#?}"
    );

    for (index, evaluation) in evaluations[..2].iter().enumerate() {
        let task = decision(evaluation, TASK_A);
        let why = last_reason(task);
        assert_eq!(
            why.outcome.code().as_str(),
            RESERVE_INSUFFICIENT,
            "round {}: {task:#?}",
            index + 1
        );
        assert_eq!(why.outcome.value("pool_id"), Some(&json!(POOL)), "{why:?}");
        assert_eq!(
            why.outcome.value("reserve_at_least"),
            Some(&json!(20)),
            "{why:?}"
        );
        assert_eq!(
            why.outcome.value("amount_after"),
            Some(&json!(-10)),
            "{why:?}"
        );
        assert_eq!(
            why.outcome.value("task_id"),
            Some(&json!(TASK_B)),
            "{why:?}"
        );
    }
}

// §2.12 and T3: the score stage hands the selection document `task.after.<pool>`. A document
// whose one term scores that field as it is ranks task A, which consumes 30 of a pool read at
// 100, with an effective value of 70 milli.
#[test]
fn bt_b16b_t3_the_selection_document_reads_what_a_task_leaves_in_its_pool() {
    let after = format!("task.after.{POOL}");
    let document = selection(
        vec![integer_field(&after)],
        Vec::new(),
        Vec::new(),
        vec![identity_term(
            "term.after",
            field_ref(&after),
            drop_candidate(),
        )],
    );
    let catalog = catalog(
        &[TaskDecl {
            id: TASK_A,
            priority: 100,
            consumes: 30,
            at_least: 10,
        }],
        Some(&document),
    );
    let evaluation = round(
        &catalog,
        &[],
        vec![pool_reading(100, NOW - 60_000)],
        Vec::new(),
        NOW,
    );
    let task = decision(&evaluation, TASK_A);
    assert_eq!(
        task.rank.as_ref().map(|rank| rank.effective_milli),
        Some(70),
        "{task:#?}"
    );
    assert_eq!(dispatched(&evaluation), [TASK_A], "{evaluation:#?}");
}

// ---- Fixture.

/// One task of the fixture: its id, priority, consumption of the pool and reserve on it.
struct TaskDecl {
    id: &'static str,
    priority: i16,
    consumes: u64,
    at_least: u64,
}

/// The fixture catalog, `actingcommand.scheduling.v2`, with these tasks, the pool and, when given,
/// the selection document.
fn catalog(tasks: &[TaskDecl], selection: Option<&Value>) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.resource-reserves",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-resource-reserves"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let tasks = tasks.iter().map(task).collect::<Vec<_>>();
    let pool = json!({
        "id": POOL,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": 1000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": POOL_KEY},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    });
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
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", json!([pool]))),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: selection.map(|selection| source("selection", selection)),
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

fn task(declared: &TaskDecl) -> Value {
    json!({
        "id": declared.id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{}", declared.id),
        "priority": declared.priority,
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
            "task_id": declared.id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [{
            "pool_id": POOL,
            "direction": "consume",
            "amount": declared.consumes,
            "observation_source": "scan_verified",
            "confidence_milli": 1000
        }],
        "produces": [],
        "reserves": [{"pool_id": POOL, "at_least": declared.at_least}],
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

/// A reading of the pool's key on the instance, observed at `observed_at`.
fn pool_reading(value: i64, observed_at: u64) -> ObservedFact {
    fact(A, POOL_KEY, value, observed_at, None)
}

/// Task B's manual offset of 50 milli, which a person set an hour before NOW.
fn offset_of_b() -> Vec<PriorityOffset> {
    vec![PriorityOffset {
        task_id: TASK_B.to_owned(),
        instance_id: None,
        offset_milli: 50,
        origin: PriorityOffsetOrigin::User,
        observed_at_unix_ms: NOW - HOUR_MS,
    }]
}

/// A task whose run, dispatched at `at`, has ended.
fn dispatched_state(task_id: &str, at: u64) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: task_id.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(at),
        eligible_since_unix_ms: None,
        terminal_state: Some(TaskTerminalState::Succeeded),
        completed_window: None,
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
    }
}

/// One round at `at`. The pool's value is the one the Runtime derives from the readings; the
/// instance has its own host and every candidate is admitted.
fn round(
    catalog: &CompiledCatalog,
    tasks: &[TaskRuntimeSnapshot],
    readings: Vec<ObservedFact>,
    priority_offsets: Vec<PriorityOffset>,
    at: u64,
) -> PolicyEvaluation {
    let instance = instance(A, &[]);
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-resource-reserves".to_owned(),
        facts: readings,
        outcomes: Vec::new(),
        tasks: tasks.to_vec(),
        instances: vec![instance.clone()],
        priority_offsets,
    };
    let resources = project_fact_pools(
        catalog,
        &facts,
        &EvaluationResources {
            pools: Vec::new(),
            hosts: vec![HostResourceSnapshot {
                host_id: instance.host_id,
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
    .unwrap_or_else(|error| panic!("the round at {at} evaluates: {error}"))
}

/// The tasks the round dispatches on the instance, by task id.
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

/// The reason that settled a decision: its last one.
fn last_reason(decision: &TaskDecision) -> &DecisionReason {
    decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {decision:#?}"))
}
