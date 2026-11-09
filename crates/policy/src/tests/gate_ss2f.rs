// SPDX-License-Identifier: AGPL-3.0-only

// SS-2f (register §1.288; Alice's answer K2 to question 137 of 11-02 19:4x; design
// `rd-1031/design-acm-price-addendum2.1.md` §5.4; coordinator work order `rd-1109/brief-ss2f.md`
// tests 1 to 4; contracts/scheduling/README.md, "Net benefit within a tier", "Turns across
// instances"):
// - in the business tier the instances take turns: the one whose most recent policy dispatch is
//   the earliest goes first, one never dispatched first of all, then the smaller instance id,
//   whatever the net benefit per millisecond `B ÷ E` of their candidates;
// - within an instance the order of before stays: unjudged first, then judged by `B ÷ E`;
// - the maintenance tier orders as before;
// - a catalog without the `benefit_judged` mark, or a round with no business candidate, decides
//   byte for byte alike whatever the instances' dispatch history.
//
// The catalog is neutral and V2: two instances of two games, each on its own idle host, with tasks
// scoped to one instance on the hourly clock, due at NOW (its occurrence is at NOW − 1 minute; a
// run after it serves it). Each game values its own pool, so the `B` of one instance is in a unit
// of its own. X's id sorts after Y's, so the order by id and the order by `B ÷ E` disagree.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact};
use crate::{
    CandidateEligibility, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DecisionReason, DispatchIntent, EvaluationFacts, EvaluationResources,
    EvaluationTime, FACT_REFRESH_TASK_ID, FactRefreshInputs, FactRefreshSettings,
    HostResourceSnapshot, InstanceHolds, InstanceSnapshot, NoProgressGuards, ObservedFact,
    OwedReruns, PolicyEvaluation, PolicyEvaluationError, RemedyPlan, RunSlots, TaskDecision,
    TaskRuntimeSnapshot, TaskTerminalState, compile_catalog, evaluate_with_no_progress,
    project_fact_pools,
};
use actingcommand_contract::outcome::Outcome;

/// The instance whose candidates have the higher `B ÷ E`.
const X: &str = "instance-x";
/// The other instance, of another game; its id sorts before X's.
const Y: &str = "instance-w";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// X's game values this pool 1000 milli a unit (scale 1).
const OUT_X: &str = "pool.out_x";
/// Y's game values this pool 1000 milli a unit (scale 1).
const OUT_Y: &str = "pool.out_y";
const TASK_X: &str = "task_x";
const TASK_Y: &str = "task_y";
/// The deferral the fixture's admission answers when it holds every candidate, so that the round
/// asks it about every candidate in allocation order.
const NOT_LOADED: &str = "pack_registry_not_loaded";
const CONCURRENT: &str = "policy_concurrent_runs_deferred";

// Test 1 (fails before, passes after): X's candidate produces three units of its game's pool and
// Y's one of its own, in the same expected 60 000 ms, so `B ÷ E` favours X (3000 against 1000
// milli). X was dispatched 10 minutes ago, Y 50 minutes ago: Y is ordered first and takes the one
// shared run slot, X waits `policy_concurrent_runs_deferred`. Before SS-2f, `B ÷ E` ordered X
// first (main eb1e946d, `crates/policy/src/evaluator.rs`, `allocation_order`). With Y the more
// recent, X goes first. Never dispatched, or dispatched at the same instant, the smaller id goes
// first: Y, though X's `B ÷ E` is higher.
#[test]
fn ss2f_t1_the_least_recently_dispatched_instance_goes_first_whatever_b_per_e() {
    let catalog = catalog(vec![
        task(TASK_Y, Y, 100, vec![produce(OUT_Y, 1)], true),
        task(TASK_X, X, 100, vec![produce(OUT_X, 3)], true),
    ]);
    let x_recent = vec![
        succeeded(X, TASK_X, NOW - 10 * MINUTE_MS),
        succeeded(Y, TASK_Y, NOW - 50 * MINUTE_MS),
    ];
    let (held, asked) = round(
        &catalog,
        &[X, Y],
        x_recent.clone(),
        Vec::new(),
        &RunSlots::default(),
        false,
    );
    assert_eq!(asked, [TASK_Y, TASK_X], "{held:#?}");
    let (evaluation, _) = round(&catalog, &[X, Y], x_recent, Vec::new(), &one_slot(), true);
    assert_eq!(dispatched(&evaluation), [TASK_Y], "{evaluation:#?}");
    assert_eq!(benefit_milli(&evaluation, Y, TASK_Y), json!(1000));
    let waiting = decision(&evaluation, X, TASK_X);
    assert_eq!(last_code(waiting), CONCURRENT, "{waiting:#?}");

    let y_recent = vec![
        succeeded(X, TASK_X, NOW - 50 * MINUTE_MS),
        succeeded(Y, TASK_Y, NOW - 10 * MINUTE_MS),
    ];
    let (held, asked) = round(
        &catalog,
        &[X, Y],
        y_recent.clone(),
        Vec::new(),
        &RunSlots::default(),
        false,
    );
    assert_eq!(asked, [TASK_X, TASK_Y], "{held:#?}");
    let (evaluation, _) = round(&catalog, &[X, Y], y_recent, Vec::new(), &one_slot(), true);
    assert_eq!(dispatched(&evaluation), [TASK_X], "{evaluation:#?}");
    assert_eq!(benefit_milli(&evaluation, X, TASK_X), json!(3000));
    let waiting = decision(&evaluation, Y, TASK_Y);
    assert_eq!(last_code(waiting), CONCURRENT, "{waiting:#?}");

    let same_instant = vec![
        succeeded(X, TASK_X, NOW - 30 * MINUTE_MS),
        succeeded(Y, TASK_Y, NOW - 30 * MINUTE_MS),
    ];
    for history in [Vec::new(), same_instant] {
        let (held, asked) = round(
            &catalog,
            &[X, Y],
            history,
            Vec::new(),
            &RunSlots::default(),
            false,
        );
        assert_eq!(asked, [TASK_Y, TASK_X], "{held:#?}");
    }
}

// Test 2: within one instance the order is unchanged. Each instance has a manual request (its
// trigger requires a `session.` instruction: a business task that is not judged) and two judged
// tasks producing one and two units, declared low, high, request. X was dispatched more recently.
// Y's three go first, then X's; on each instance the request first, then the judged ones by
// `B ÷ E` (2000 before 1000 milli), exactly the order the instance gets when it is alone.
#[test]
fn ss2f_t2_within_an_instance_the_order_is_unchanged() {
    let mut tasks = Vec::new();
    for (alias, out) in [(X, OUT_X), (Y, OUT_Y)] {
        let tagged = tag(alias);
        tasks.push(task(
            &format!("task_low_{tagged}"),
            alias,
            100,
            vec![produce(out, 1)],
            true,
        ));
        tasks.push(task(
            &format!("task_high_{tagged}"),
            alias,
            100,
            vec![produce(out, 2)],
            true,
        ));
        let request = format!("task_request_{tagged}");
        tasks.push(with(
            task(&request, alias, 100, Vec::new(), true),
            "trigger",
            all(vec![
                hourly(),
                at_least(alias, &format!("session.request.{request}"), 1),
            ]),
        ));
    }
    let catalog = catalog(tasks);
    let history = vec![
        succeeded(X, "task_low_x", NOW - 10 * MINUTE_MS),
        succeeded(Y, "task_low_y", NOW - 50 * MINUTE_MS),
    ];
    let (held, asked) = round(
        &catalog,
        &[X, Y],
        history.clone(),
        vec![request_reading(X), request_reading(Y)],
        &RunSlots::default(),
        false,
    );
    assert_eq!(
        asked,
        [
            "task_request_y",
            "task_high_y",
            "task_low_y",
            "task_request_x",
            "task_high_x",
            "task_low_x",
        ],
        "{held:#?}"
    );
    for alias in [X, Y] {
        let (alone, alone_asked) = round(
            &catalog,
            &[alias],
            of(alias, &history),
            vec![request_reading(alias)],
            &RunSlots::default(),
            false,
        );
        let suffix = format!("_{}", tag(alias));
        let mine = asked
            .iter()
            .filter(|task_id| task_id.ends_with(&suffix))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(mine.len(), 3, "{asked:?}");
        assert_eq!(mine, alone_asked, "{alias}: {alone:#?}");
    }
}

// Test 3: the maintenance tier is unchanged. Each instance has an unmarked (maintenance) task, X's
// of priority 200 and Y's of 100, and a judged business task. X was dispatched more recently. The
// maintenance tier goes first and keeps its order, X's first by its priority; then the business
// tier, Y's first. With one shared run slot X's maintenance task takes it.
#[test]
fn ss2f_t3_the_maintenance_tier_is_unchanged() {
    let catalog = catalog(vec![
        task("task_maint_y", Y, 100, Vec::new(), false),
        task("task_maint_x", X, 200, Vec::new(), false),
        task(TASK_Y, Y, 100, vec![produce(OUT_Y, 1)], true),
        task(TASK_X, X, 100, vec![produce(OUT_X, 3)], true),
    ]);
    let history = vec![
        succeeded(X, TASK_X, NOW - 10 * MINUTE_MS),
        succeeded(Y, TASK_Y, NOW - 50 * MINUTE_MS),
    ];
    let (held, asked) = round(
        &catalog,
        &[X, Y],
        history.clone(),
        Vec::new(),
        &RunSlots::default(),
        false,
    );
    assert_eq!(
        asked,
        ["task_maint_x", "task_maint_y", TASK_Y, TASK_X],
        "{held:#?}"
    );
    let (evaluation, _) = round(&catalog, &[X, Y], history, Vec::new(), &one_slot(), true);
    assert_eq!(dispatched(&evaluation), ["task_maint_x"], "{evaluation:#?}");
    let waiting = decision(&evaluation, Y, "task_maint_y");
    assert_eq!(last_code(waiting), CONCURRENT, "{waiting:#?}");
}

// Test 4: without a business candidate nothing changes. The dispatch history is the fact refresh's
// (a policy dispatch of each instance that no catalog task reads): X refreshed 10 s ago, or Y did,
// or neither. A catalog without the mark (X's task of priority 200, Y's of 100) and a marked
// catalog whose business tasks both ran after the occurrence (so only its maintenance tasks are
// candidates) decide byte for byte alike under the three histories, X's task first by its
// priority. The control: the same histories order a marked catalog's due business tasks.
#[test]
fn ss2f_t4_without_a_business_candidate_the_decisions_are_byte_identical() {
    let unmarked = catalog(vec![
        task(TASK_Y, Y, 100, vec![produce(OUT_Y, 1)], false),
        task(TASK_X, X, 200, vec![produce(OUT_X, 3)], false),
    ]);
    let marked = catalog(vec![
        task("task_maint_y", Y, 100, Vec::new(), false),
        task("task_maint_x", X, 200, Vec::new(), false),
        task(TASK_Y, Y, 100, vec![produce(OUT_Y, 1)], true),
        task(TASK_X, X, 100, vec![produce(OUT_X, 3)], true),
    ]);
    let served = vec![
        succeeded(X, TASK_X, NOW - 30_000),
        succeeded(Y, TASK_Y, NOW - 30_000),
    ];
    for (compiled, served, first) in [
        (&unmarked, Vec::new(), [TASK_X, TASK_Y]),
        (&marked, served, ["task_maint_x", "task_maint_y"]),
    ] {
        let mut decisions = Vec::new();
        for history in histories(served.as_slice()) {
            let (held, asked) = round(
                compiled,
                &[X, Y],
                history,
                Vec::new(),
                &RunSlots::default(),
                false,
            );
            assert_eq!(asked, first, "{held:#?}");
            decisions.push(serde_json::to_vec(&held.decisions).expect("decisions"));
        }
        assert!(
            decisions.windows(2).all(|pair| pair[0] == pair[1]),
            "the dispatch history changes nothing outside the business tier: {first:?}"
        );
    }

    // The control: the fact refresh's dispatch is the instance's most recent policy dispatch.
    let due = catalog(vec![
        task(TASK_Y, Y, 100, vec![produce(OUT_Y, 1)], true),
        task(TASK_X, X, 100, vec![produce(OUT_X, 3)], true),
    ]);
    let [never, x_refreshed, y_refreshed] = histories(&[]);
    for (history, order) in [
        (never, [TASK_Y, TASK_X]),
        (x_refreshed, [TASK_Y, TASK_X]),
        (y_refreshed, [TASK_X, TASK_Y]),
    ] {
        let (held, asked) = round(
            &due,
            &[X, Y],
            history,
            Vec::new(),
            &RunSlots::default(),
            false,
        );
        assert_eq!(asked, order, "{held:#?}");
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

fn all(predicates: Vec<Value>) -> Value {
    json!({"kind": "all", "predicates": predicates})
}

/// The integer fact `fact_key` of `alias` is at least `threshold`.
fn at_least(alias: &str, fact_key: &str, threshold: i64) -> Value {
    json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": alias},
        "fact_key": fact_key,
        "comparison": "greater_than_or_equal",
        "value": {"type": "integer", "value": threshold},
        "max_age_ms": HOUR_MS
    })
}

fn produce(pool: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": "produce",
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// A task of the instance `alias` on its hourly clock, marked `benefit_judged` when `judged`.
fn task(id: &str, alias: &str, priority: i16, produces: Vec<Value>, judged: bool) -> Value {
    let mut task = json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": alias},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
        "priority": priority,
        "trigger": hourly(),
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
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    });
    if judged {
        task["benefit_judged"] = json!(true);
    }
    task
}

/// `task` with its member `name` set to `value`.
fn with(mut task: Value, name: &str, value: Value) -> Value {
    task[name] = value;
    task
}

/// A pool of `alias` that does not regenerate, valued 1000 milli a unit by its game.
fn pool(alias: &str, id: &str) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": alias},
        "capacity": 1_000_000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000}
    })
}

/// The activity profile of the game `game_id`.
fn profile(id: &str, game_id: &str) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": game_id},
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

/// The fixture catalog, `actingcommand.scheduling.v2`, with these tasks in this order.
fn catalog(tasks: Vec<Value>) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.turns",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-turns"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let pools = json!([pool(X, OUT_X), pool(Y, OUT_Y)]);
    let profiles = json!([
        profile("activity-x", "game-x"),
        profile("activity-y", "game-w")
    ]);
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", pools)),
        activity: source("activity", &document("profiles", profiles)),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-turns/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

/// The available instance `alias` of its game, on its own host.
fn instance(alias: &str) -> InstanceSnapshot {
    let game = if alias == X { "x" } else { "w" };
    InstanceSnapshot {
        instance_id: alias.to_owned(),
        server_id: format!("server-{game}"),
        game_id: format!("game-{game}"),
        host_id: format!("host.{alias}"),
        available: true,
        capability_operation_ids: vec![OPERATION.to_owned()],
        preferred_task_ids: Vec::new(),
    }
}

/// `task_id`'s latest run on `alias`, dispatched at `at`, succeeded.
fn succeeded(alias: &str, task_id: &str, at: u64) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: task_id.to_owned(),
        instance_id: alias.to_owned(),
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

/// The fact refresh's latest run on `alias`, dispatched at `at`, succeeded.
fn refresh(alias: &str, at: u64) -> TaskRuntimeSnapshot {
    succeeded(alias, FACT_REFRESH_TASK_ID, at)
}

/// Test 4's three histories: `served`, then with the fact refresh's run on neither instance, on X
/// or on Y, 10 s before NOW.
fn histories(served: &[TaskRuntimeSnapshot]) -> [Vec<TaskRuntimeSnapshot>; 3] {
    [None, Some(X), Some(Y)].map(|refreshed| {
        let mut history = served.to_vec();
        history.extend(refreshed.map(|alias| refresh(alias, NOW - 10_000)));
        history
    })
}

/// The tag of `alias`'s task ids in test 2.
fn tag(alias: &str) -> &'static str {
    if alias == X { "x" } else { "y" }
}

/// Test 2's manual request instruction on `alias`, read a minute before NOW.
fn request_reading(alias: &str) -> ObservedFact {
    fact(
        alias,
        &format!("session.request.task_request_{}", tag(alias)),
        1,
        NOW - MINUTE_MS,
        None,
    )
}

/// The snapshots of `alias` among `history`.
fn of(alias: &str, history: &[TaskRuntimeSnapshot]) -> Vec<TaskRuntimeSnapshot> {
    history
        .iter()
        .filter(|state| state.instance_id == alias)
        .cloned()
        .collect()
}

/// `performance.pacing.max_concurrent_runs` 1, nothing admitted or running.
fn one_slot() -> RunSlots {
    RunSlots {
        admitted_load_profiles: Vec::new(),
        max_concurrent_runs: Some(1),
    }
}

/// The fact refresh off: no reader is bound.
fn refresh_inputs() -> FactRefreshInputs {
    FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: false,
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
    }
}

/// One round at NOW over the instances `aliases`, under `run_slots`. With `admit` the admission
/// takes every candidate; without it the admission holds every candidate, so the round asks it
/// about every candidate, and the second value lists their task ids in allocation order.
fn round(
    catalog: &CompiledCatalog,
    aliases: &[&str],
    tasks: Vec<TaskRuntimeSnapshot>,
    readings: Vec<ObservedFact>,
    run_slots: &RunSlots,
    admit: bool,
) -> (PolicyEvaluation, Vec<String>) {
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-turns".to_owned(),
        facts: readings,
        outcomes: Vec::new(),
        tasks,
        instances: aliases.iter().copied().map(instance).collect(),
        priority_offsets: Vec::new(),
    };
    let resources = project_fact_pools(
        catalog,
        &facts,
        &EvaluationResources {
            pools: Vec::new(),
            hosts: facts
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
    let mut asked = Vec::new();
    let evaluation = evaluate_with_no_progress::<PolicyEvaluationError>(
        catalog,
        &facts,
        &resources,
        EvaluationTime {
            unix_ms: NOW,
            monotonic_ms: NOW,
        },
        11,
        &BTreeSet::new(),
        &refresh_inputs(),
        &RemedyPlan::default(),
        &ChainHandoverInputs::default(),
        run_slots,
        &OwedReruns::default(),
        &InstanceHolds::default(),
        &NoProgressGuards::default(),
        |_, _, _| Ok(false),
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

fn decision<'a>(evaluation: &'a PolicyEvaluation, alias: &str, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(alias)
        })
        .unwrap_or_else(|| panic!("a decision for {task_id} on {alias}: {evaluation:#?}"))
}

fn last_code(decision: &TaskDecision) -> &str {
    decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {decision:#?}"))
        .outcome
        .code()
        .as_str()
}

/// The `benefit_milli` value of the selected candidate's `rank_breakdown`.
fn benefit_milli(evaluation: &PolicyEvaluation, alias: &str, task_id: &str) -> Value {
    let selected = decision(evaluation, alias, task_id);
    selected
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == "rank_breakdown")
        .and_then(|reason| reason.outcome.value("benefit_milli"))
        .cloned()
        .unwrap_or_else(|| panic!("a rank_breakdown with benefit_milli on {selected:#?}"))
}
