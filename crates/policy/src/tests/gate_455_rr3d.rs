// SPDX-License-Identifier: AGPL-3.0-only

// #455 RR3d (0.12.101; register §1.548; the coordinator's rulings 1 to 6 of 11-28 in its work order
// `rd-1123/brief-455-rr3d.md`; contracts/scheduling/README.md, "Pool lines";
// contracts/resource-targets.md, "A pool a run was told enough to close the gap on"), the policy's
// half: the production told under a pool's gap adds up since the pool's reading. Of the noted
// dispatches that were told a gap on the pool and whose run has not ended or ended after the
// reading, in the order noted, the pool is unread once for one of them the sum of turns times the
// production per turn, over it and those before it, reaches the gap it was told.
// - two dispatches of one pair each short of the gap close it together, each compared with the
//   gap it was itself told (test 1);
// - a reading observed at or after a dispatch's end counts only the dispatches after it (test 2);
// - a later dispatch of the pair told no gap on the pool does not take the place of one that
//   closed it (test 3);
// - two tasks told three each of a gap of six close it together (test 4);
// - a stock of 150 at three a dispatch is read through the 49th dispatch and unread at the 50th
//   (test 5).
// RR3c's own gates (`gate_455_rr3c`, one dispatch each) and LF-1's (`gate_477_lf1`) pass as they
// are. The host notes the dispatches in the order of their admission, a run that failed at its
// entry passed over (the host's gate of this slice); here they are noted as the host would. The
// judgement is the one every reader takes, `produced_unread`. Names are made up; no clock is read.

use actingcommand_contract::outcome::Outcome;
use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::evaluator::produced_unread;
use crate::resource_targets::SpendValues;
use crate::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, EvaluationFacts, FactValue,
    GivenPoolReadings, HandedRuns, ObservedFact, PolicyCode, SCHEDULING_SCHEMA_VERSION_V2,
    ScopeSelector, compile_catalog,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// How long a run lasts.
const RUN_MS: u64 = 30_000;
/// The pool a goal is on, of `instance-a`, read from `TIERED_KEY`.
const TIERED: &str = "pool.tiered";
const TIERED_KEY: &str = "resource.tiered";
/// A second pool of `instance-a`, read from `OTHER_KEY` (test 3).
const OTHER: &str = "pool.other";
const OTHER_KEY: &str = "resource.other";
/// When both pools were read: a minute before NOW, before every dispatch.
const READ_AT: u64 = NOW - MINUTE_MS;

/// One noting of a table: the gap the dispatch was told at its production entry 0, its `run_cap`
/// (none: one turn), and whether the pool is unread once it is noted.
type Noting = (u64, Option<u64>, bool);

// Test 1, two dispatches of one pair each short of the gap. `task_one` produces 1 of the pool a
// run; each dispatch since the pool's reading, its run ended after it, is noted in turn:
// - a gap of 6 told 3 and 3: read after the first, unread after the second;
// - a gap of 7 told 3, 3 and one turn: read, read, unread at the seventh unit;
// - each dispatch against the gap it was itself told: told 3 of 4 (read), then one turn of a gap of
//   2 (4 ≥ 2: unread); told 3 of 4 (read), then 4 of 8 (7 < 8: read), then one turn of 8 (unread).
#[test]
fn rr3d_t1_two_dispatches_short_of_the_gap_close_it_together() {
    let compiled = catalog(vec![producer("task_one", json!([produce(TIERED, 1)]))]);
    let facts = facts_of(vec![reading(TIERED_KEY, READ_AT)]);
    let cases: [(&str, Vec<Noting>); 4] = [
        (
            "a gap of 6 told 3 and 3",
            vec![(6, Some(3), false), (6, Some(3), true)],
        ),
        (
            "a gap of 7 told 3, 3 and 1",
            vec![(7, Some(3), false), (7, Some(3), false), (7, None, true)],
        ),
        (
            "a smaller gap told later",
            vec![(4, Some(3), false), (2, None, true)],
        ),
        (
            "a larger gap told later",
            vec![(4, Some(3), false), (8, Some(4), false), (8, None, true)],
        ),
    ];
    for (case, notings) in cases {
        let mut handed = HandedRuns::default();
        for (place, (gap, run_cap, unread)) in (1_u64..).zip(notings) {
            handed.note_dispatch(
                "task_one",
                A,
                Some(NOW + place * MINUTE_MS),
                [&parameters(&[(0, gap)], run_cap)],
            );
            assert_eq!(
                unread_pools(&compiled, &facts, &handed).contains(TIERED),
                unread,
                "{case}, dispatch {place}"
            );
        }
    }
}

// Test 2, a reading between the dispatches. A gap of 6 told 3 by a dispatch whose run ended at
// NOW + 30 s and then 3 by one that still runs: a reading from before the first dispatch, or one
// observed while its run ran, counts both (unread); one observed at the first run's end or after
// counts only the second (3 < 6: read).
#[test]
fn rr3d_t2_a_reading_after_a_runs_end_counts_only_the_dispatches_after_it() {
    let compiled = catalog(vec![producer("task_one", json!([produce(TIERED, 1)]))]);
    let ended = NOW + RUN_MS;
    let mut handed = HandedRuns::default();
    handed.note_dispatch(
        "task_one",
        A,
        Some(ended),
        [&parameters(&[(0, 6)], Some(3))],
    );
    handed.note_dispatch("task_one", A, None, [&parameters(&[(0, 6)], Some(3))]);
    for (case, observed_at, unread) in [
        ("read before both", READ_AT, true),
        ("read while the first ran", ended - 1, true),
        ("read at the first run's end", ended, false),
        ("read after the first run's end", ended + 1, false),
    ] {
        let facts = facts_of(vec![reading(TIERED_KEY, observed_at)]);
        assert_eq!(
            unread_pools(&compiled, &facts, &handed).contains(TIERED),
            unread,
            "{case}"
        );
    }
}

// Test 3, a later dispatch of the pair told no gap on the pool (RR3c's limit, closed). `task_both`
// produces 1 of `pool.tiered` (entry 0) and 1 of `pool.other` (entry 1) a run. Its first dispatch,
// its run ended at NOW + 30 s, was told a gap of 2 on `pool.tiered` and 2 turns: it closed it. A
// later dispatch of the same pair, still running, carries no `gaps_remaining` (its `run_cap`
// alone), no member at all, or a gap on `pool.other` only: `pool.tiered` stays unread with a
// reading from before the first dispatch (where RR3c read it again on that reading), and is read
// with one observed at the first run's end. The gap on `pool.other` (2 told 2) leaves that pool
// unread.
#[test]
fn rr3d_t3_a_later_dispatch_told_no_gap_there_does_not_take_the_place_of_one_that_closed_it() {
    let compiled = catalog(vec![producer(
        "task_both",
        json!([produce(TIERED, 1), produce(OTHER, 1)]),
    )]);
    let ended = NOW + RUN_MS;
    for (case, later, other_unread) in [
        ("its run_cap alone", parameters(&[], Some(5)), false),
        ("no member", parameters(&[], None), false),
        (
            "a gap on the other pool",
            parameters(&[(1, 2)], Some(2)),
            true,
        ),
    ] {
        let mut handed = HandedRuns::default();
        handed.note_dispatch(
            "task_both",
            A,
            Some(ended),
            [&parameters(&[(0, 2)], Some(2))],
        );
        handed.note_dispatch("task_both", A, None, [&later]);
        let before = facts_of(vec![
            reading(TIERED_KEY, READ_AT),
            reading(OTHER_KEY, READ_AT),
        ]);
        let unread = unread_pools(&compiled, &before, &handed);
        assert!(unread.contains(TIERED), "{case}: {unread:?}");
        assert_eq!(unread.contains(OTHER), other_unread, "{case}: {unread:?}");
        let after = facts_of(vec![
            reading(TIERED_KEY, ended),
            reading(OTHER_KEY, READ_AT),
        ]);
        assert!(
            !unread_pools(&compiled, &after, &handed).contains(TIERED),
            "{case}"
        );
    }
}

// Test 4, two tasks. `task_one` and `task_two` each produce 1 of the pool a run; each is sent told
// 3 of a gap of 6 since the pool's reading, in either order: read after the first, unread after the
// second.
#[test]
fn rr3d_t4_two_tasks_told_three_each_of_a_gap_of_six_close_it_together() {
    let compiled = catalog(vec![
        producer("task_one", json!([produce(TIERED, 1)])),
        producer("task_two", json!([produce(TIERED, 1)])),
    ]);
    let facts = facts_of(vec![reading(TIERED_KEY, READ_AT)]);
    for (first, second) in [("task_one", "task_two"), ("task_two", "task_one")] {
        let mut handed = HandedRuns::default();
        handed.note_dispatch(
            first,
            A,
            Some(NOW + RUN_MS),
            [&parameters(&[(0, 6)], Some(3))],
        );
        assert!(
            !unread_pools(&compiled, &facts, &handed).contains(TIERED),
            "{first} alone"
        );
        handed.note_dispatch(second, A, None, [&parameters(&[(0, 6)], Some(3))]);
        assert!(
            unread_pools(&compiled, &facts, &handed).contains(TIERED),
            "{first} then {second}"
        );
    }
}

// Test 5, a stock of 150 at three a dispatch (RR3c's test 5 sent it again and again on one
// reading). Fifty dispatches of `task_one`, each told 150 and 3 turns, each run ended a minute
// after the one before, the pool not read again: read through the 49th (147 < 150), unread at the
// 50th.
#[test]
fn rr3d_t5_a_stock_of_150_at_three_a_dispatch_is_unread_at_the_fiftieth() {
    let compiled = catalog(vec![producer("task_one", json!([produce(TIERED, 1)]))]);
    let facts = facts_of(vec![reading(TIERED_KEY, READ_AT)]);
    let mut handed = HandedRuns::default();
    for dispatch in 1..=50_u64 {
        handed.note_dispatch(
            "task_one",
            A,
            Some(NOW + dispatch * MINUTE_MS),
            [&parameters(&[(0, 150)], Some(3))],
        );
        assert_eq!(
            unread_pools(&compiled, &facts, &handed).contains(TIERED),
            dispatch == 50,
            "dispatch {dispatch}"
        );
    }
}

// ---- The fixture ----

/// The facts of `instance-a`, at the fixture's ledger position, holding `observed`.
fn facts_of(observed: Vec<ObservedFact>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-455-rr3d".to_owned(),
        facts: observed,
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

/// `key` of `instance-a` read as 0, observed at `at`, of no expiry, at full confidence.
fn reading(key: &str, at: u64) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: key.to_owned(),
        value: FactValue::Integer(0),
        observed_at_unix_ms: at,
        expires_at_unix_ms: None,
        confidence_milli: 1_000,
    }
}

/// The pools [`produced_unread`] judges unread in `facts` for the runs `handed`, as the evaluator
/// and the needs' projection read them.
fn unread_pools(
    compiled: &CompiledCatalog,
    facts: &EvaluationFacts,
    handed: &HandedRuns,
) -> std::collections::BTreeSet<String> {
    let bundle = compiled.catalog();
    produced_unread(
        &bundle.pools.pools,
        facts,
        &GivenPoolReadings::default(),
        &bundle.tasks.tasks,
        handed,
    )
}

/// The `policy_dispatch_parameters` SS-6 builds of the gaps `gaps` (`(effect_index, gap)`) and
/// `run_cap`.
fn parameters(gaps: &[(usize, u64)], run_cap: Option<u64>) -> Outcome {
    SpendValues {
        spend_allowances: Vec::new(),
        gaps_remaining: gaps.to_vec(),
        run_cap,
    }
    .add_to(Outcome::new(PolicyCode::PolicyDispatchParameters))
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

/// A task `id` of the game of priority 100, due every hour, consuming nothing and producing
/// `produces`.
fn producer(id: &str, produces: Value) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
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
        "expected_duration_ms": 60_000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// A pool of `instance-a` without a valuation, read from `resource.<the id without "pool.">`, of
/// capacity 1 000 000, writing `target_defaults` and `goal_tiers`.
fn goal_pool(id: &str) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": 1_000_000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "target_defaults": {"scale": 10, "importance_milli": 1000},
        "goal_tiers": {"need_importance_milli": 100_000, "stock_cap_milli": 40}
    })
}

fn catalog(tasks: Vec<Value>) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.455-rr3d",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-rr3d"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
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
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source(
            "pools",
            &document("pools", json!([goal_pool(TIERED), goal_pool(OTHER)])),
        ),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the catalog compiles: {:?}", failure.diagnostics()))
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-455-rr3d/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}
