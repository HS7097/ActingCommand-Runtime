// SPDX-License-Identifier: AGPL-3.0-only

// SS-2 (register §1.222; Alice's ruling 104 points 2, 4 and 5, approved 2026-10-30, Workflow
// #386 comment 6523113979; brief `rd-1029/brief-ss2.md` tests 1 to 4 and 6; test 5 is
// `gate_rl1a::ss2_t5_three_priority_values_warn_and_two_do_not`):
// - a chain successor ranks with its chain head's priority and strategic weight while it is in
//   the head's run window, and with its own outside it; the resource layer reads no chain;
// - of candidates equal in the ranking, the one closer to losing its eligibility goes first (a
//   window it requires closes sooner, or fewer runs of its own consumption are left above its
//   threshold); without a measure, the one with more conditions; then the one declared first.
//
// The catalog is neutral and V2: tasks on one instance with an hourly clock due at NOW, one pool
// of the instance read from its key (no regeneration), and one valued pool. Every candidate's
// host is idle, so no contention term separates two tasks of equal priority and weight.

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, evaluate_fixture, fact, instance};
use crate::{
    CandidateEligibility, CatalogDocumentSource, CatalogSources, CompiledCatalog, DecisionReason,
    DispatchIntent, EvaluationFacts, EvaluationResources, EvaluationTime, HostResourceSnapshot,
    ObservedFact, PolicyEvaluation, PolicyEvaluationError, TaskDecision, TaskRuntimeSnapshot,
    TaskTerminalState, compile_catalog, evaluate_with_eligibility, project_fact_pools,
};
use actingcommand_contract::outcome::Outcome;

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
/// The pool the tasks of test 1 consume, read from its key.
const POOL: &str = "pool.neutral";
const POOL_KEY: &str = "resource.neutral";
/// A key no task consumes.
const OTHER_KEY: &str = "resource.other";
/// A pool with a base valuation (scale 1, base weight 7 milli per unit).
const VALUED: &str = "pool.valued";
const VALUED_KEY: &str = "resource.valued";
const MINUTE_MS: u64 = 60_000;
/// The deferral the fixture's admission answers when it holds every candidate, so that the round
/// asks it about every candidate in allocation order.
const NOT_LOADED: &str = "pack_registry_not_loaded";

// Test 1 (ruling 104 point 4, Alice's example in neutral form): A and B tie in the ranking (same
// priority, same weight, same total), and B is declared first, so declared order alone would pick
// B. A loses its eligibility sooner, and A runs:
// - a window: A's clock occurrence is due only for 30 minutes (`missed: skip_after`), 29 minutes
//   are left; B's clock has no grace and stays due;
// - a threshold: both need the pool at 9000 or more, which holds 10000; A uses 600 a run, so two
//   runs are left above its threshold, B uses 100, so eleven.
// Without A's bound B runs (declared order), and a total one point higher wins whatever the
// measure says: the tie-break only orders ties.
#[test]
fn ss2_t1_of_two_tied_candidates_the_one_closer_to_losing_its_eligibility_runs() {
    // A window.
    let window = catalog(vec![
        task("task_b", 100, hourly(), 0, None),
        task("task_a", 100, graced(30 * MINUTE_MS), 0, None),
    ]);
    let (evaluation, _) = round(&window, Vec::new(), Vec::new(), 11, true);
    assert_eq!(total(&evaluation, "task_a"), total(&evaluation, "task_b"));
    assert_eq!(dispatched(&evaluation), ["task_a"], "{evaluation:#?}");
    let detail = breakdown(&evaluation, "task_a");
    assert!(
        detail.contains(
            " lead=0 tie=bounded chances=29 time_left_ms=1740000 runs_left=none conditions=1 declared=1"
        ),
        "{detail}"
    );
    let other = related(&evaluation, "task_a", "task_b");
    assert!(
        other.contains(" behind_by=0 ")
            && other.ends_with(
                " tie=none chances=none time_left_ms=none runs_left=none conditions=1 declared=0"
            ),
        "{other}"
    );

    // A threshold.
    let threshold = all(vec![hourly(), pool_at_least(9_000)]);
    let consuming = catalog(vec![
        task("task_b", 100, threshold.clone(), 100, None),
        task("task_a", 100, threshold, 600, None),
    ]);
    let readings = vec![fact(A, POOL_KEY, 10_000, NOW - MINUTE_MS, None)];
    let (evaluation, _) = round(&consuming, Vec::new(), readings, 11, true);
    assert_eq!(total(&evaluation, "task_a"), total(&evaluation, "task_b"));
    assert_eq!(dispatched(&evaluation), ["task_a"], "{evaluation:#?}");
    let detail = breakdown(&evaluation, "task_a");
    assert!(detail.contains(" tie=bounded chances=2 "), "{detail}");
    assert!(detail.contains(" runs_left=2 conditions=2 "), "{detail}");
    let other = related(&evaluation, "task_a", "task_b");
    assert!(other.contains(" tie=bounded chances=11 "), "{other}");

    // Without A's bound, declared order: B.
    let catalog_plain = catalog(vec![
        task("task_b", 100, hourly(), 0, None),
        task("task_a", 100, hourly(), 0, None),
    ]);
    let (evaluation, _) = round(&catalog_plain, Vec::new(), Vec::new(), 11, true);
    assert_eq!(dispatched(&evaluation), ["task_b"], "{evaluation:#?}");

    // No tie: the higher total runs, and its breakdown carries no tie-break.
    let catalog_apart = catalog(vec![
        task("task_b", 101, hourly(), 0, None),
        task("task_a", 100, graced(30 * MINUTE_MS), 0, None),
    ]);
    let (evaluation, _) = round(&catalog_apart, Vec::new(), Vec::new(), 11, true);
    assert_eq!(dispatched(&evaluation), ["task_b"], "{evaluation:#?}");
    let detail = breakdown(&evaluation, "task_b");
    assert!(!detail.contains(" tie="), "{detail}");
}

// Test 2 (ruling 104 point 4, fallback): no candidate has a measure (no window it requires closes
// and no threshold its own consumption lowers), so the one with more conditions goes first, and
// of two with as many, the one declared first. Declared: B (one condition), A and C (two each).
// The admission holds every candidate, so the round asks it about each in allocation order.
#[test]
fn ss2_t2_without_a_measure_more_conditions_go_first_then_declared_order() {
    let two = all(vec![hourly(), other_at_least(0)]);
    let catalog = catalog(vec![
        task("task_b", 100, hourly(), 0, None),
        task("task_a", 100, two.clone(), 0, None),
        task("task_c", 100, two, 0, None),
    ]);
    let readings = vec![fact(A, OTHER_KEY, 5, NOW - MINUTE_MS, None)];
    let (evaluation, order) = round(&catalog, Vec::new(), readings.clone(), 11, false);
    assert_eq!(order, ["task_a", "task_c", "task_b"], "{evaluation:#?}");
    let (evaluation, _) = round(&catalog, Vec::new(), readings, 11, true);
    assert_eq!(dispatched(&evaluation), ["task_a"], "{evaluation:#?}");
    let detail = breakdown(&evaluation, "task_a");
    assert!(
        detail.ends_with(
            " tie=none chances=none time_left_ms=none runs_left=none conditions=2 declared=1"
        ),
        "{detail}"
    );
}

// Test 3 (ruling 104 point 5): a full tie (same priority, weight, total, measure and conditions)
// goes to the task declared first, whatever the ids and whatever the evaluation's seed, in every
// evaluation: before SS-2 a seeded hash decided it.
#[test]
fn ss2_t3_declared_order_decides_a_full_tie_in_every_evaluation() {
    let catalog = catalog(vec![
        task("task_z", 100, hourly(), 0, None),
        task("task_a", 100, hourly(), 0, None),
        task("task_m", 100, hourly(), 0, None),
    ]);
    for seed in 1..=8 {
        let (evaluation, order) = round(&catalog, Vec::new(), Vec::new(), seed, false);
        assert_eq!(
            order,
            ["task_z", "task_a", "task_m"],
            "seed {seed}: {evaluation:#?}"
        );
        let (evaluation, _) = round(&catalog, Vec::new(), Vec::new(), seed, true);
        assert_eq!(dispatched(&evaluation), ["task_z"], "seed {seed}");
    }
}

// Test 4 (ruling 104 point 2): the head (priority 300, produces 10 of the valued pool) ran a
// minute ago and succeeded; its clock occurrence is served. The successor (priority 100,
// produces 3) requires the head's success; a third task has priority 200.
// - In the head's run window (the successor has not run since) the successor ranks with 300 and
//   runs; its breakdown names the head.
// - Once it served that run (it ran after it and succeeded) it ranks with its own 100 and the
//   third task runs. A dependency under an `any` is no chain link: own priority too.
// - The resource layer reads no chain: the successor's resource term is its own yield × base /
//   scale = 21 in and out of the window, never the head's 70, and its pool weights are the same.
#[test]
fn ss2_t4_a_successor_ranks_with_its_chain_head_in_the_head_run_window() {
    let tasks = |successor_trigger: Value| {
        vec![
            task("task_x", 200, hourly(), 0, None),
            task("task_head", 300, hourly(), 0, Some(10)),
            task("task_next", 100, successor_trigger, 0, Some(3)),
        ]
    };
    let chain = catalog(tasks(dependency("task_head")));
    let head_ran = state("task_head", NOW - 30_000, TaskTerminalState::Succeeded);

    let (open, _) = round(&chain, vec![head_ran.clone()], Vec::new(), 11, true);
    assert_eq!(dispatched(&open), ["task_next"], "{open:#?}");
    assert_eq!(priority(&open, "task_next"), 300);
    let detail = breakdown(&open, "task_next");
    assert!(detail.contains(" chain_head=task_head"), "{detail}");

    let served = state("task_next", NOW - 10_000, TaskTerminalState::Succeeded);
    let (closed, _) = round(&chain, vec![head_ran.clone(), served], Vec::new(), 11, true);
    assert_eq!(dispatched(&closed), ["task_x"], "{closed:#?}");
    assert_eq!(priority(&closed, "task_next"), 100);

    let loose = catalog(tasks(json!({
        "kind": "any",
        "predicates": [dependency("task_head"), hourly()]
    })));
    let (unlinked, _) = round(&loose, vec![head_ran], Vec::new(), 11, true);
    assert_eq!(dispatched(&unlinked), ["task_x"], "{unlinked:#?}");
    assert_eq!(priority(&unlinked, "task_next"), 100);

    for (label, evaluation) in [
        ("open", &open),
        ("closed", &closed),
        ("unlinked", &unlinked),
    ] {
        assert_eq!(
            term(evaluation, "task_next"),
            21,
            "{label}: never the head's 70"
        );
    }
    assert_eq!(
        weights(&open, "task_next"),
        weights(&closed, "task_next"),
        "the pool weights do not depend on the chain"
    );
}

// Test 6 (no other change): on the selection fixture (tasks of priorities 300, 200 and 100 on two
// instances, no tie) each instance runs its highest total, every selected breakdown is the one of
// before byte for byte (no chain head, no tie-break), and the decisions are byte-identical over
// two evaluations of the same inputs.
#[test]
fn ss2_t6_without_ties_the_decisions_are_those_of_before() {
    let catalog = super::selection_catalog::catalog(
        &[("task.high", 300), ("task.mid", 200), ("task.low", 100)],
        None,
    );
    let instances = [instance("instance-a", &[]), instance("instance-b", &[])];
    let evaluation = evaluate_fixture(&catalog, &instances, Vec::new()).expect("the fixture");
    let again = evaluate_fixture(&catalog, &instances, Vec::new()).expect("the fixture");
    assert_eq!(
        serde_json::to_vec(&evaluation).expect("bytes"),
        serde_json::to_vec(&again).expect("bytes")
    );
    let text = serde_json::to_string(&evaluation).expect("text");
    assert!(!text.contains(" tie="), "{text}");
    assert!(!text.contains("chain_head="), "{text}");
    assert_eq!(evaluation.dispatch_intents.len(), 2, "{evaluation:#?}");
    for intent in &evaluation.dispatch_intents {
        assert_eq!(intent.task_id, "task.high");
        let ranked = evaluation
            .decisions
            .iter()
            .filter(|decision| decision.instance_id.as_deref() == Some(intent.instance_id.as_str()))
            .filter_map(|decision| decision.rank.as_ref())
            .collect::<Vec<_>>();
        let winner = ranked
            .iter()
            .map(|rank| rank.total_score)
            .max()
            .expect("ranked candidates");
        let runner_up = ranked
            .iter()
            .map(|rank| rank.total_score)
            .filter(|total| *total < winner)
            .max()
            .expect("a second candidate");
        let selected = evaluation
            .decisions
            .iter()
            .find(|decision| {
                decision.task_id == "task.high"
                    && decision.instance_id.as_deref() == Some(intent.instance_id.as_str())
            })
            .expect("the selected decision");
        let rank = selected.rank.as_ref().expect("its rank");
        assert_eq!(rank.total_score, winner);
        let detail = reason_with_code(selected, "rank_breakdown");
        let tie_breaker = detail
            .split_once(" tie_breaker=")
            .and_then(|(_, rest)| rest.split(' ').next())
            .expect("the hash tie-breaker");
        let before = format!(
            "total={} base={} priority={} aging_ms={} strategic_milli={} urgency_milli={} contention={} effective=0 score=none utility=0 offset=0 target=none target_id=- mode=- superseded=0 promoted=0 affinity=0 tie_breaker={tie_breaker} saturated=0 lead={}",
            rank.total_score,
            rank.total_score,
            rank.priority,
            rank.aging_ms,
            rank.strategic_weight_milli,
            rank.urgency_milli,
            rank.contention_penalty,
            winner - runner_up,
        );
        assert_eq!(detail, before);
    }
}

// ---- Fixtures ----

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

/// The hourly clock whose occurrence is due only for `grace_ms` after it.
fn graced(grace_ms: u64) -> Value {
    let mut clock = hourly();
    clock["missed"] = json!({"kind": "skip_after", "grace_ms": grace_ms});
    clock
}

fn all(predicates: Vec<Value>) -> Value {
    json!({"kind": "all", "predicates": predicates})
}

/// True while the pool's key holds at least `threshold`.
fn pool_at_least(threshold: i64) -> Value {
    at_least(POOL_KEY, threshold)
}

/// True while the key no task consumes holds at least `threshold`.
fn other_at_least(threshold: i64) -> Value {
    at_least(OTHER_KEY, threshold)
}

fn at_least(fact_key: &str, threshold: i64) -> Value {
    json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": fact_key,
        "comparison": "greater_than_or_equal",
        "value": {"type": "integer", "value": threshold},
        "max_age_ms": HOUR_MS
    })
}

/// True once `task_id`'s latest run on the instance succeeded.
fn dependency(task_id: &str) -> Value {
    json!({"kind": "dependency_completed", "task_id": task_id, "terminal_states": ["succeeded"]})
}

/// A task: `consumes` of the pool a run (none at 0), `produces` of the valued pool.
fn task(id: &str, priority: i16, trigger: Value, consumes: u64, produces: Option<u64>) -> Value {
    let consumes = if consumes == 0 {
        Vec::new()
    } else {
        vec![json!({
            "pool_id": POOL,
            "direction": "consume",
            "amount": consumes,
            "observation_source": "scan_verified",
            "confidence_milli": 1000
        })]
    };
    let produces = produces
        .map(|amount| {
            vec![json!({
                "pool_id": VALUED,
                "direction": "produce",
                "amount": amount,
                "observation_source": "scan_verified",
                "confidence_milli": 1000
            })]
        })
        .unwrap_or_default();
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
        "priority": priority,
        "trigger": trigger,
        "feedback_stop": {
            "kind": "outcome",
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": consumes,
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

fn pool(id: &str, fact_key: &str, capacity: u64, valuation: Option<Value>) -> Value {
    let mut pool = json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": capacity,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": fact_key},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    });
    if let Some(valuation) = valuation {
        pool["valuation"] = valuation;
    }
    pool
}

/// The fixture catalog, `actingcommand.scheduling.v2`, with these tasks in this order.
fn catalog(tasks: Vec<Value>) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.ties",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-ties"]
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
        pool(POOL, POOL_KEY, 20_000, None),
        pool(
            VALUED,
            VALUED_KEY,
            1_000,
            Some(json!({"name": "Material", "unit": "piece", "scale": 1, "base_weight_milli": 7}))
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
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", pools)),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

/// A task whose run, dispatched at `at`, ended in `terminal`.
fn state(task_id: &str, at: u64, terminal: TaskTerminalState) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: task_id.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(at),
        eligible_since_unix_ms: None,
        terminal_state: Some(terminal),
        completed_window: None,
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
    }
}

/// One round at NOW with `seed`. With `admit` every candidate is admitted; without it the
/// admission holds every candidate, and the second value lists the task ids it was asked about,
/// in allocation order.
fn round(
    catalog: &CompiledCatalog,
    tasks: Vec<TaskRuntimeSnapshot>,
    readings: Vec<ObservedFact>,
    seed: u64,
    admit: bool,
) -> (PolicyEvaluation, Vec<String>) {
    let instance = instance(A, &[]);
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-ties".to_owned(),
        facts: readings,
        outcomes: Vec::new(),
        tasks,
        instances: vec![instance.clone()],
        priority_offsets: Vec::new(),
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
    let mut asked = Vec::new();
    let evaluation = evaluate_with_eligibility(
        catalog,
        &facts,
        &resources,
        EvaluationTime {
            unix_ms: NOW,
            monotonic_ms: NOW,
        },
        seed,
        |intent: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
            asked.push(intent.task_id.clone());
            if admit {
                return Ok(CandidateEligibility::Eligible);
            }
            Ok(CandidateEligibility::Deferred {
                reason: DecisionReason {
                    outcome: Outcome::relayed(NOT_LOADED),
                    detail: format!("fixture admission: {NOT_LOADED}"),
                },
                next_wake_unix_ms: None,
            })
        },
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"));
    (evaluation, asked)
}

fn dispatched(evaluation: &PolicyEvaluation) -> Vec<&str> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| intent.task_id.as_str())
        .collect()
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == task_id && decision.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}: {evaluation:#?}"))
}

fn total(evaluation: &PolicyEvaluation, task_id: &str) -> i64 {
    let decision = decision(evaluation, task_id);
    decision
        .rank
        .as_ref()
        .unwrap_or_else(|| panic!("a rank on {decision:#?}"))
        .total_score
}

fn priority(evaluation: &PolicyEvaluation, task_id: &str) -> i16 {
    let decision = decision(evaluation, task_id);
    decision
        .rank
        .as_ref()
        .unwrap_or_else(|| panic!("a rank on {decision:#?}"))
        .priority
}

fn reason_with_code<'a>(decision: &'a TaskDecision, code: &str) -> &'a str {
    decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == code)
        .map(|reason| reason.detail.as_str())
        .unwrap_or_else(|| panic!("a {code} reason on {decision:#?}"))
}

/// The selected candidate's `rank_breakdown` detail.
fn breakdown<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a str {
    reason_with_code(decision(evaluation, task_id), "rank_breakdown")
}

/// The detail with which `winner`'s decision record lists `other`.
fn related<'a>(evaluation: &'a PolicyEvaluation, winner: &str, other: &str) -> &'a str {
    let decision = decision(evaluation, winner);
    decision
        .reasons
        .iter()
        .find(|reason| {
            reason.outcome.code().as_str() == "decision_candidate_not_selected"
                && reason.outcome.value("task_id") == Some(&json!(other))
        })
        .map(|reason| reason.detail.as_str())
        .unwrap_or_else(|| panic!("{other} listed on {decision:#?}"))
}

/// The candidate's `resource_weights` detail.
fn weights<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a str {
    reason_with_code(decision(evaluation, task_id), "resource_weights")
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
