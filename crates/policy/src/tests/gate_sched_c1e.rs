// SPDX-License-Identifier: AGPL-3.0-only

// #428 SCHED-C1e (register §1.299 SC1-12; the coordinator's point 8 and answers 3, 5 and 8 of
// 11-11, not Alice's words; Alice, 11-09 17:42: 「这个红线会随着时间由外部修改」; coordinator work
// order `rd-1111/brief-sched-c1e.md`; contracts/scheduling/README.md, "Pool lines", "Lines for one
// instance"): the lines one instance's `scheduling.pool_lines` gives a pool, as the evaluation
// reads them.
// - precedence: an instance's set over the catalog's lines over the defaults, for that instance
//   only; a set without `trigger_line` begins its episode at the capacity, whatever the catalog's
//   trigger line;
// - a change during an episode ends none, and the next evaluation reads the new lines; clearing
//   returns to the catalog's;
// - `lines_source` (`instance`, `catalog`, `default`) on `pool_line_held`, `capped_spend` and
//   `pool_lines_unspendable`; `S ≤ cap` and `T ≤ cap` checked at the evaluation (`lines_invalid`);
// - without a set every decision is as before but for the new value, and the example catalog keeps
//   its golden identities.
//
// The catalog is neutral and V2, written as the composer writes instance copies: per instance
// alias X, the capped pool `pool.neutral@X` of X, read from `resource.neutral` and its capacity
// from `resource.neutral.capacity` (180), valued 1000 milli a unit (Q = 1), no regeneration; the
// produced pool `pool.out@X`, 1000 milli a unit; and each task as `<task>@X` of X. The catalog's
// lines, where a test writes them, are the capacity less 40 (140) and less 76 (104). An instance's
// set names the pool `pool.neutral`, the id its `schedule.json` writes. A run is simulated: the
// pool is read again half a minute after the dispatch, and the next round is a minute after it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CandidateEligibility, CappedMemory, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DecisionReason, DispatchIntent, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactReference, FactRefreshInputs, FactRefreshSettings, HostResourceSnapshot,
    InstanceHolds, InstanceLineSet, InstancePoolLines, NoProgressGuards, OwedReruns, PolicyCode,
    PolicyEvaluation, PolicyEvaluationError, PoolLine, RemedyPlan, RunSlots,
    SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2, SchedulingDecisionState, TaskDecision,
    TaskRuntimeSnapshot, TaskTerminalState, compile_catalog, evaluate_with_capped_spending,
    evaluate_with_instance_lines, instance_valued_pool_ids, project_fact_pools,
};
use actingcommand_contract::outcome::Outcome;

const A: &str = "fixture-instance-a";
const B: &str = "fixture-instance-b";
const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// The capped pool, as an instance's `scheduling.pool_lines` names it; each instance's copy is
/// `pool.neutral@<alias>`.
const POOL: &str = "pool.neutral";
/// The produced pool; each instance's copy is `pool.out@<alias>`.
const OUT: &str = "pool.out";
const READING_KEY: &str = "resource.neutral";
/// The capacity fact of the capped pool, in its scope.
const CAPACITY_KEY: &str = "resource.neutral.capacity";
const OUT_KEY: &str = "resource.out";
/// The neutral example catalog's golden identities, computed before SS-2h (as in `gate_ss2h`).
const V1_EXAMPLE_HASH: &str =
    "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c";
const V2_EXAMPLE_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";

// Test 1, precedence. Each instance runs `task_p` (10 units of the pool a run, 30 of `pool.out`:
// positive at full price, `V ÷ c` 3000 in a capped round).
// - The catalog writes the lines (140, 104) and no trigger line. A's set {150, 110, trigger 170}:
//   read at 175 on both, A begins a capped episode (175 ≥ 170) and dispatches `task_p` with
//   `capped_spend` {`stop_line` 150, `hard_line` 110, `trigger_line` 170, `lines_source`
//   `instance`}; B, read at 112, keeps the catalog's lines: below its trigger (the capacity) its
//   run would leave 102 below the catalog's hard line 104, so it is held, `would_cross_hard_line`
//   {`hard_line` 104, `lines_source` `catalog`}, and B begins no episode.
// - Without the set, A read at 175 takes the catalog's lines: below the capacity, no episode, and
//   `task_p` runs at full price with no capped reason.
// - The catalog's trigger line 20 below the capacity (160), A's set {150, 110} without one: read
//   at 170 on both, A's trigger line is the capacity (180), not the catalog's, so A begins no
//   episode and runs `task_p` at full price; B begins one at 160 and its `capped_spend` names
//   `trigger_line` 160 and `lines_source` `catalog`.
// - A pool that writes no lines takes the defaults: read at the capacity (180) on A, its
//   `capped_spend` names `stop_line` 180, `hard_line` 0, no `trigger_line`, `lines_source`
//   `default`.
// - The pools an instance's set may name are its copies' ids without `@<alias>`, valued.
#[test]
fn sched_c1e_t1_an_instance_set_comes_before_the_catalog_for_that_instance_only() {
    let both = [A, B];
    let positive = || vec![task("task_p", 10, 30, Trigger::Enough)];

    let mut world = World::new(catalog(&both, &positive(), &catalog_lines(None)), 175);
    world.readings.insert(B, 112);
    world.lines.insert(A, POOL, set(150, 110, Some(170)));
    let evaluation = world.evaluate();
    assert_eq!(
        dispatched(&evaluation),
        [copy("task_p", A)],
        "{evaluation:#?}"
    );
    let head = spend(&evaluation, "task_p", A);
    assert_eq!(head.value("pool_id"), Some(&json!(copy(POOL, A))));
    assert_eq!(head.value("stop_line"), Some(&json!(150)));
    assert_eq!(head.value("hard_line"), Some(&json!(110)));
    assert_eq!(head.value("trigger_line"), Some(&json!(170)));
    assert_eq!(head.value("pool_capacity"), Some(&json!(180)));
    assert_eq!(head.value("lines_source"), Some(&json!("instance")));
    assert_eq!(world.memory.episode(A, &copy(POOL, A)), Some((1, 1)));
    let held = last(decision(&evaluation, "task_p", B));
    assert_eq!(hold_of(held).as_deref(), Some("would_cross_hard_line"));
    assert_eq!(held.outcome.value("hard_line"), Some(&json!(104)));
    assert_eq!(held.outcome.value("stop_line"), Some(&json!(140)));
    assert_eq!(held.outcome.value("amount_after"), Some(&json!(102)));
    assert_eq!(held.outcome.value("lines_source"), Some(&json!("catalog")));
    assert_eq!(world.memory.episode(B, &copy(POOL, B)), None);

    let mut plain = World::new(catalog(&both, &positive(), &catalog_lines(None)), 175);
    let evaluation = plain.evaluate();
    let ran = dispatched(&evaluation);
    assert!(ran.contains(&copy("task_p", A)), "{evaluation:#?}");
    assert!(!has(
        decision(&evaluation, "task_p", A),
        PolicyCode::CappedSpend
    ));
    assert!(plain.memory.is_empty());

    let mut triggered = World::new(
        catalog(&both, &positive(), &catalog_lines(Some(line(-20)))),
        170,
    );
    triggered.lines.insert(A, POOL, set(150, 110, None));
    let evaluation = triggered.evaluate();
    let ran = dispatched(&evaluation);
    assert!(ran.contains(&copy("task_p", A)), "{evaluation:#?}");
    assert!(ran.contains(&copy("task_p", B)), "{evaluation:#?}");
    assert!(
        !has(decision(&evaluation, "task_p", A), PolicyCode::CappedSpend),
        "A's trigger line is the capacity: {evaluation:#?}"
    );
    assert_eq!(triggered.memory.episode(A, &copy(POOL, A)), None);
    let b_head = spend(&evaluation, "task_p", B);
    assert_eq!(b_head.value("trigger_line"), Some(&json!(160)));
    assert_eq!(b_head.value("lines_source"), Some(&json!("catalog")));
    assert_eq!(triggered.memory.episode(B, &copy(POOL, B)), Some((1, 1)));

    let mut defaults = World::new(catalog(&[A], &positive(), &[]), 180);
    let evaluation = defaults.evaluate();
    assert_eq!(
        dispatched(&evaluation),
        [copy("task_p", A)],
        "{evaluation:#?}"
    );
    let head = spend(&evaluation, "task_p", A);
    assert_eq!(head.value("stop_line"), Some(&json!(180)));
    assert_eq!(head.value("hard_line"), Some(&json!(0)));
    assert_eq!(head.value("trigger_line"), None);
    assert_eq!(head.value("lines_source"), Some(&json!("default")));

    let mut unvalued = catalog_document(&both, &positive(), &catalog_lines(None));
    for pool in unvalued.1.as_array_mut().expect("the pools") {
        if pool["id"] == json!(copy(POOL, B)) {
            pool.as_object_mut()
                .expect("a pool object")
                .shift_remove("valuation");
        }
    }
    let compiled = compile(unvalued);
    assert_eq!(
        instance_valued_pool_ids(&compiled, A, SERVER, GAME),
        BTreeSet::from([OUT.to_owned(), POOL.to_owned()])
    );
    assert_eq!(
        instance_valued_pool_ids(&compiled, B, SERVER, GAME),
        BTreeSet::from([OUT.to_owned()])
    );
    assert!(instance_valued_pool_ids(&compiled, "fixture-instance-c", SERVER, GAME).is_empty());
}

// Test 2, a change during an episode. The catalog's lines (140, 104); `task_a` (30 units, `V ÷ c`
// 1000) and `task_b` (25 units, 800). Read at 200, episode 1 begins: A then B are frozen and
// `task_a` runs as the head (`lines_source` `catalog`). A's set then gives lines read from the
// capacity's fact (−80, −120: 100 and 60): the episode is not ended, and its next evaluation reads
// them: `task_b` runs as rank 2 of the same round with `stop_line` 100, `hard_line` 60 and
// `lines_source` `instance`; at 145, above the new stop line, round 2 begins with `task_a`
// (`lines_source` `instance`). Cleared at 115 (below the catalog's stop line, above the set's),
// the catalog's lines are read again and the episode ends by its own rule; kept, it goes on.
#[test]
fn sched_c1e_t2_a_change_ends_no_episode_and_clearing_returns_to_the_catalog() {
    let tasks = || {
        vec![
            task("task_a", 30, 30, Trigger::Enough),
            task("task_b", 25, 20, Trigger::Enough),
        ]
    };
    let mut world = World::new(catalog(&[A], &tasks(), &catalog_lines(None)), 200);
    let first = world.evaluate();
    assert_eq!(dispatched(&first), [copy("task_a", A)], "{first:#?}");
    let head = spend(&first, "task_a", A);
    assert_eq!(head.value("lines_source"), Some(&json!("catalog")));
    assert_eq!(head.value("stop_line"), Some(&json!(140)));
    world.run(&first, 30);

    world.lines.insert(
        A,
        POOL,
        InstanceLineSet {
            stop_line: reference(-80),
            hard_line: reference(-120),
            trigger_line: None,
        },
    );
    let second = world.evaluate();
    assert_eq!(
        world.memory.episode(A, &copy(POOL, A)),
        Some((1, 1)),
        "not ended"
    );
    assert_eq!(dispatched(&second), [copy("task_b", A)], "{second:#?}");
    let next = spend(&second, "task_b", A);
    assert_eq!(next.value("capped_episode"), Some(&json!(1)));
    assert_eq!(next.value("capped_round"), Some(&json!(1)));
    assert_eq!(next.value("capped_rank"), Some(&json!(2)));
    assert_eq!(next.value("stop_line"), Some(&json!(100)));
    assert_eq!(next.value("hard_line"), Some(&json!(60)));
    assert_eq!(next.value("lines_source"), Some(&json!("instance")));
    world.run(&second, 25);

    let third = world.evaluate();
    assert_eq!(dispatched(&third), [copy("task_a", A)], "{third:#?}");
    let round = spend(&third, "task_a", A);
    assert_eq!(round.value("capped_episode"), Some(&json!(1)));
    assert_eq!(round.value("capped_round"), Some(&json!(2)));
    assert_eq!(round.value("pool_reading"), Some(&json!(145)));
    assert_eq!(round.value("lines_source"), Some(&json!("instance")));
    world.run(&third, 30);

    let mut kept = world.clone();
    world.lines = InstancePoolLines::default();
    let cleared = world.evaluate();
    assert!(dispatched(&cleared).is_empty(), "{cleared:#?}");
    assert_eq!(
        world.memory.episode(A, &copy(POOL, A)),
        None,
        "spent at 115 by the catalog's lines"
    );
    let going_on = kept.evaluate();
    assert_eq!(dispatched(&going_on), [copy("task_b", A)], "{going_on:#?}");
    assert_eq!(kept.memory.episode(A, &copy(POOL, A)), Some((1, 2)));
}

// Test 3, `lines_source` on each reason, each of its values.
// - `capped_spend`: read at the capacity (180) with the instance's set {150, 110}, the catalog's
//   lines (140, 104) and none: `instance`, `catalog`, `default`.
// - `pool_line_held`: below the trigger line, `task_p` (10 units) would cross the hard line:
//   read at 115 under the set {150, 110} (`instance`), at 112 under the catalog's (`catalog`), and
//   `task_q` (the same on an hourly clock) at 5 under none (`default`, `hard_line` 0). A set read
//   above the capacity is refused at the evaluation with `lines_invalid` and `lines_source`
//   `instance`: its stop line (190) and its trigger line (190) each.
// - `pool_lines_unspendable`: read at 180, the lines {175, 170} leave 10 above the hard line, less
//   than the least run (25), from the set (`instance`) and from the catalog (`catalog`); without
//   lines a run of 190 does not fit under the capacity (`default`, read at 200).
#[test]
fn sched_c1e_t3_lines_source_names_where_the_lines_come_from() {
    let source = |outcome: &Outcome| outcome.value("lines_source").cloned();
    let pair = || {
        vec![
            task("task_a", 30, 30, Trigger::Enough),
            task("task_b", 25, 20, Trigger::Enough),
        ]
    };
    let positive = || vec![task("task_p", 10, 30, Trigger::Enough)];

    // capped_spend
    for (lines, own, expected) in [
        (catalog_lines(None), Some(set(150, 110, None)), "instance"),
        (catalog_lines(None), None, "catalog"),
        (Vec::new(), None, "default"),
    ] {
        let mut world = World::new(catalog(&[A], &pair(), &lines), 180);
        if let Some(own) = own {
            world.lines.insert(A, POOL, own);
        }
        let evaluation = world.evaluate();
        assert_eq!(dispatched(&evaluation), [copy("task_a", A)], "{expected}");
        let head = spend(&evaluation, "task_a", A);
        assert_eq!(source(head), Some(json!(expected)), "{expected}: {head:?}");
    }

    // pool_line_held, would_cross_hard_line
    let mut own = World::new(catalog(&[A], &positive(), &catalog_lines(None)), 115);
    own.lines.insert(A, POOL, set(150, 110, None));
    let mut by_catalog = World::new(catalog(&[A], &positive(), &catalog_lines(None)), 112);
    let mut by_default = World::new(
        catalog(&[A], &[task("task_q", 10, 30, Trigger::Hourly)], &[]),
        5,
    );
    for (world, task_id, expected, hard_line) in [
        (&mut own, "task_p", "instance", 110),
        (&mut by_catalog, "task_p", "catalog", 104),
        (&mut by_default, "task_q", "default", 0),
    ] {
        let evaluation = world.evaluate();
        assert!(
            dispatched(&evaluation).is_empty(),
            "{expected}: {evaluation:#?}"
        );
        let held = last(decision(&evaluation, task_id, A));
        assert_eq!(
            hold_of(held).as_deref(),
            Some("would_cross_hard_line"),
            "{expected}"
        );
        assert_eq!(held.outcome.value("hard_line"), Some(&json!(hard_line)));
        assert_eq!(source(&held.outcome), Some(json!(expected)), "{expected}");
    }

    // pool_line_held, lines_invalid: S ≤ cap and T ≤ cap at the evaluation.
    for (own, stop_line, trigger_line) in [
        (set(190, 104, None), 190, None),
        (set(140, 104, Some(190)), 140, Some(190)),
    ] {
        let mut world = World::new(catalog(&[A], &positive(), &catalog_lines(None)), 150);
        world.lines.insert(A, POOL, own);
        let evaluation = world.evaluate();
        assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
        let held = last(decision(&evaluation, "task_p", A));
        assert_eq!(hold_of(held).as_deref(), Some("lines_invalid"));
        assert_eq!(held.outcome.value("stop_line"), Some(&json!(stop_line)));
        assert_eq!(held.outcome.value("pool_capacity"), Some(&json!(180)));
        assert_eq!(
            held.outcome.value("trigger_line"),
            trigger_line.map(|line| json!(line)).as_ref()
        );
        assert_eq!(source(&held.outcome), Some(json!("instance")));
    }

    // pool_lines_unspendable
    let tight = vec![("stop_line", json!(175)), ("hard_line", json!(170))];
    let mut own = World::new(catalog(&[A], &pair(), &catalog_lines(None)), 180);
    own.lines.insert(A, POOL, set(175, 170, None));
    let mut by_catalog = World::new(catalog(&[A], &pair(), &tight), 180);
    for (world, expected) in [(&mut own, "instance"), (&mut by_catalog, "catalog")] {
        let evaluation = world.evaluate();
        assert!(
            dispatched(&evaluation).is_empty(),
            "{expected}: {evaluation:#?}"
        );
        assert!(world.memory.is_empty(), "{expected}: no episode begins");
        for task_id in ["task_a", "task_b"] {
            let held = decision(&evaluation, task_id, A);
            assert_eq!(held.state, SchedulingDecisionState::Deferred);
            let warning = &last(held).outcome;
            assert!(warning.is(PolicyCode::PoolLinesUnspendable), "{held:#?}");
            assert_eq!(warning.value("least_consumption"), Some(&json!(25)));
            assert_eq!(warning.value("hard_line"), Some(&json!(170)));
            assert_eq!(source(warning), Some(json!(expected)), "{expected}");
        }
    }
    let mut by_default = World::new(
        catalog(&[A], &[task("task_big", 190, 200, Trigger::Hourly)], &[]),
        200,
    );
    let evaluation = by_default.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let warning = &last(decision(&evaluation, "task_big", A)).outcome;
    assert!(warning.is(PolicyCode::PoolLinesUnspendable), "{warning:?}");
    assert_eq!(warning.value("least_consumption"), Some(&json!(190)));
    assert_eq!(warning.value("hard_line"), Some(&json!(0)));
    assert_eq!(source(warning), Some(json!("default")));
}

// Test 4, identity. The neutral example catalog keeps its V1 and V2 golden identities. Without a
// set, or with sets for another instance or another pool only, the Runtime's entry gives the
// evaluation, and the memory, of `evaluate_with_capped_spending` exactly; the decisions differ from
// before SCHED-C1e only by the new value, which the head's developer text ends with.
#[test]
fn sched_c1e_t4_without_a_set_every_decision_is_as_before_but_the_new_value() {
    for (version, golden) in [
        (SCHEDULING_SCHEMA_VERSION, V1_EXAMPLE_HASH),
        (SCHEDULING_SCHEMA_VERSION_V2, V2_EXAMPLE_HASH),
    ] {
        let example = compile_catalog(&example(version))
            .unwrap_or_else(|failure| panic!("{version}: {:?}", failure.diagnostics()));
        assert_eq!(example.catalog_hash(), golden, "{version}");
    }
    let tasks = || {
        vec![
            task("task_a", 30, 30, Trigger::Enough),
            task("task_b", 25, 20, Trigger::Enough),
        ]
    };
    let mut elsewhere = InstancePoolLines::default();
    elsewhere.insert(B, POOL, set(150, 110, None));
    elsewhere.insert(A, "pool.other", set(150, 110, None));
    for lines in [InstancePoolLines::default(), elsewhere] {
        let mut world = World::new(catalog(&[A], &tasks(), &catalog_lines(None)), 200);
        world.lines = lines;
        let mut before = CappedMemory::default();
        let facts = world.facts();
        let resources = resources(&world.catalog, &facts, &world.instances);
        let plain = evaluate_with_capped_spending::<PolicyEvaluationError>(
            &world.catalog,
            &facts,
            &resources,
            time(world.now),
            11,
            &BTreeSet::new(),
            &refresh_inputs(),
            &RemedyPlan::default(),
            &ChainHandoverInputs::default(),
            &RunSlots::default(),
            &OwedReruns::default(),
            &InstanceHolds::default(),
            &NoProgressGuards::default(),
            &mut before,
            |_, _, _| Ok(false),
            |_: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
                Ok(CandidateEligibility::Eligible)
            },
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"));
        let evaluation = world.evaluate();
        assert_eq!(evaluation, plain);
        assert_eq!(world.memory, before);
        let head = reason(decision(&evaluation, "task_a", A), PolicyCode::CappedSpend);
        assert_eq!(
            head.detail,
            format!(
                "pool_id={} pool_capacity=180 stop_line=140 hard_line=104 capped_episode=1 \
                 capped_round=1 capped_rank=1 pool_reading=200 lines_source=catalog",
                copy(POOL, A)
            )
        );
    }
}

// ---- The world a test drives ----

/// The catalog, the Runtime's memory of the capped episodes, the lines the instances set, the
/// instances, each one's reading of its capped pool, and the tasks' snapshots.
#[derive(Clone)]
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    lines: InstancePoolLines,
    instances: Vec<&'static str>,
    readings: BTreeMap<&'static str, i64>,
    read_at: u64,
    now: u64,
    states: Vec<TaskRuntimeSnapshot>,
}

impl World {
    /// The world at NOW over the instances `catalog` names, each capped pool read at `reading` a
    /// minute before; no instance sets lines.
    fn new(catalog: CompiledCatalog, reading: i64) -> Self {
        let instances = [A, B]
            .into_iter()
            .filter(|alias| {
                catalog
                    .catalog()
                    .pools
                    .pools
                    .iter()
                    .any(|pool| pool.id == copy(POOL, alias))
            })
            .collect::<Vec<_>>();
        let readings = instances.iter().map(|alias| (*alias, reading)).collect();
        Self {
            catalog,
            memory: CappedMemory::default(),
            lines: InstancePoolLines::default(),
            instances,
            readings,
            read_at: NOW - MINUTE_MS,
            now: NOW,
            states: Vec::new(),
        }
    }

    fn facts(&self) -> EvaluationFacts {
        let mut facts = Vec::new();
        for alias in &self.instances {
            let reading = self.readings.get(alias).copied().unwrap_or_default();
            facts.push(fact(alias, READING_KEY, reading, self.read_at, None));
            facts.push(fact(alias, CAPACITY_KEY, 180, NOW - HOUR_MS, None));
        }
        EvaluationFacts {
            ledger_position: 7,
            fact_snapshot_id: "snapshot:fixture-sched-c1e".to_owned(),
            facts,
            outcomes: Vec::new(),
            tasks: self.states.clone(),
            instances: self
                .instances
                .iter()
                .map(|alias| instance(alias, &[]))
                .collect(),
            priority_offsets: Vec::new(),
        }
    }

    /// One round by the Runtime's entry, with its memory and the instances' sets, every candidate
    /// admitted.
    fn evaluate(&mut self) -> PolicyEvaluation {
        let facts = self.facts();
        let resources = resources(&self.catalog, &facts, &self.instances);
        evaluate_with_instance_lines::<PolicyEvaluationError>(
            &self.catalog,
            &facts,
            &resources,
            time(self.now),
            11,
            &BTreeSet::new(),
            &refresh_inputs(),
            &RemedyPlan::default(),
            &ChainHandoverInputs::default(),
            &RunSlots::default(),
            &OwedReruns::default(),
            &InstanceHolds::default(),
            &NoProgressGuards::default(),
            &mut self.memory,
            &self.lines,
            |_, _, _| Ok(false),
            |_: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
                Ok(CandidateEligibility::Eligible)
            },
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
    }

    /// Runs the round's one dispatch: it succeeds having consumed `spent` of its instance's pool;
    /// the pools are read half a minute after the dispatch and the next round is a minute after.
    fn run(&mut self, evaluation: &PolicyEvaluation, spent: i64) {
        let [intent] = evaluation.dispatch_intents.as_slice() else {
            panic!("one dispatch: {evaluation:#?}");
        };
        self.states.retain(|state| {
            state.task_id != intent.task_id || state.instance_id != intent.instance_id
        });
        self.states.push(TaskRuntimeSnapshot {
            task_id: intent.task_id.clone(),
            instance_id: intent.instance_id.clone(),
            last_dispatched_unix_ms: Some(self.now),
            eligible_since_unix_ms: None,
            terminal_state: Some(TaskTerminalState::Succeeded),
            completed_window: None,
            last_duration_ms: None,
            failure_streak: None,
            expected_runtime_ms: None,
            expected_runtime_runs: None,
        });
        if let Some(reading) = self.readings.get_mut(intent.instance_id.as_str()) {
            *reading -= spent;
        }
        self.read_at = self.now + 30_000;
        self.now += MINUTE_MS;
    }
}

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

fn resources(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    instances: &[&str],
) -> EvaluationResources {
    project_fact_pools(
        catalog,
        facts,
        &EvaluationResources {
            pools: Vec::new(),
            hosts: instances
                .iter()
                .map(|alias| HostResourceSnapshot {
                    host_id: format!("host.{alias}"),
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
    )
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

// ---- The lines ----

/// A set of constant lines.
fn set(stop_line: u64, hard_line: u64, trigger_line: Option<u64>) -> InstanceLineSet {
    InstanceLineSet {
        stop_line: PoolLine::Constant(stop_line),
        hard_line: PoolLine::Constant(hard_line),
        trigger_line: trigger_line.map(PoolLine::Constant),
    }
}

/// A line of a set read from the capacity's fact plus `offset`.
fn reference(offset: i64) -> PoolLine {
    PoolLine::Fact(FactReference {
        fact_key: CAPACITY_KEY.to_owned(),
        offset,
        scope: None,
    })
}

/// The catalog's line: the capacity fact plus `offset`.
fn line(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

/// The catalog's lines of the capped pool: 40 and 76 below its capacity, and `trigger` its
/// trigger line when given.
fn catalog_lines(trigger: Option<Value>) -> Vec<(&'static str, Value)> {
    let mut lines = vec![("stop_line", line(-40)), ("hard_line", line(-76))];
    if let Some(trigger) = trigger {
        lines.push(("trigger_line", trigger));
    }
    lines
}

// ---- Assertions ----

/// The copy of `id` for the instance `alias`, as the composer names it.
fn copy(id: &str, alias: &str) -> String {
    format!("{id}@{alias}")
}

fn dispatched(evaluation: &PolicyEvaluation) -> Vec<String> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| intent.task_id.clone())
        .collect()
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task: &str, alias: &str) -> &'a TaskDecision {
    let task_id = copy(task, alias);
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(alias)
        })
        .unwrap_or_else(|| panic!("a decision for {task_id} on {alias}: {evaluation:#?}"))
}

fn has(decision: &TaskDecision, code: PolicyCode) -> bool {
    decision
        .reasons
        .iter()
        .any(|reason| reason.outcome.is(code))
}

fn reason(decision: &TaskDecision, code: PolicyCode) -> &DecisionReason {
    decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.is(code))
        .unwrap_or_else(|| panic!("a {} reason on {decision:#?}", code.as_str()))
}

fn last(decision: &TaskDecision) -> &DecisionReason {
    decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {decision:#?}"))
}

/// The `pool_line_hold` of `reason`, when it is `pool_line_held`.
fn hold_of(reason: &DecisionReason) -> Option<String> {
    if !reason.outcome.is(PolicyCode::PoolLineHeld) {
        return None;
    }
    reason
        .outcome
        .value("pool_line_hold")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The `capped_spend` of the dispatch of `task`'s copy on `alias`.
fn spend<'a>(evaluation: &'a PolicyEvaluation, task: &str, alias: &str) -> &'a Outcome {
    &reason(decision(evaluation, task, alias), PolicyCode::CappedSpend).outcome
}

// ---- The catalog ----

/// When a task is due: while its instance's capped pool holds what it consumes, or every hour.
#[derive(Clone, Copy)]
enum Trigger {
    Enough,
    Hourly,
}

/// A judged task of every instance: `consumed` units of the capped pool a run, `produced` units
/// of `pool.out`, due by `trigger`.
struct Rule {
    id: String,
    consumed: u64,
    produced: u64,
    trigger: Trigger,
}

fn task(id: &str, consumed: u64, produced: u64, trigger: Trigger) -> Rule {
    Rule {
        id: id.to_owned(),
        consumed,
        produced,
        trigger,
    }
}

fn effect(pool: &str, direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

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

/// The copy of the judged task `rule` for the instance `alias`.
fn task_copy(alias: &str, rule: &Rule) -> Value {
    let Rule {
        id,
        consumed,
        produced,
        trigger,
    } = rule;
    let task_id = copy(id, alias);
    let trigger = match trigger {
        Trigger::Enough => json!({
            "kind": "resource_projection",
            "pool_id": copy(POOL, alias),
            "comparison": "greater_than_or_equal",
            "value": consumed
        }),
        Trigger::Hourly => hourly(),
    };
    json!({
        "id": task_id,
        "scope": {"kind": "instance", "instance_id": alias},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
        "priority": 100,
        "trigger": trigger,
        "feedback_stop": {
            "kind": "outcome",
            "task_id": task_id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [effect(&copy(POOL, alias), "consume", *consumed)],
        "produces": [effect(&copy(OUT, alias), "produce", *produced)],
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
        "instance_overrides": [],
        "benefit_judged": true
    })
}

/// The copy of a pool for the instance `alias`, read from `fact_key`, of `capacity`, valued 1000
/// milli a unit.
fn pool_copy(id: &str, alias: &str, fact_key: &str, capacity: Value) -> Value {
    json!({
        "id": copy(id, alias),
        "scope": {"kind": "instance", "instance_id": alias},
        "capacity": capacity,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": fact_key},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000}
    })
}

/// The tasks and pools documents' entries for `instances`: each instance's task copies, its
/// produced pool and its capped pool (capacity read from its fact) writing `lines`.
fn catalog_document(instances: &[&str], tasks: &[Rule], lines: &[(&str, Value)]) -> (Value, Value) {
    let mut task_entries = Vec::new();
    let mut pool_entries = Vec::new();
    for alias in instances {
        task_entries.extend(tasks.iter().map(|task| task_copy(alias, task)));
        pool_entries.push(pool_copy(OUT, alias, OUT_KEY, json!(1_000_000)));
        let mut capped = pool_copy(POOL, alias, READING_KEY, line(0));
        for (member, value) in lines {
            capped[*member] = value.clone();
        }
        pool_entries.push(capped);
    }
    (Value::Array(task_entries), Value::Array(pool_entries))
}

fn catalog(instances: &[&str], tasks: &[Rule], lines: &[(&str, Value)]) -> CompiledCatalog {
    compile(catalog_document(instances, tasks, lines))
}

fn compile((tasks, pools): (Value, Value)) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.sched-c1e",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-sched-c1e"]
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
        tasks: source("tasks", &document("tasks", tasks)),
        pools: source("pools", &document("pools", pools)),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

/// The neutral example catalog `catalog-a` declared as `version` (V2: every timeline event valid
/// from 0, as its V2 golden was computed).
fn example(version: &str) -> CatalogSources {
    let parse = |bytes: &[u8]| -> Value { serde_json::from_slice(bytes).expect("example JSON") };
    let mut documents = [
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/tasks.json"
        )),
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/pools.json"
        )),
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/activity.json"
        )),
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/timeline.json"
        )),
    ];
    for declared in &mut documents {
        declared["schema_version"] = json!(version);
        if version == SCHEDULING_SCHEMA_VERSION_V2
            && let Some(events) = declared.get_mut("events").and_then(Value::as_array_mut)
        {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
    }
    let [tasks, pools, activity, timeline] = documents;
    CatalogSources {
        tasks: source("example-tasks", &tasks),
        pools: source("example-pools", &pools),
        activity: source("example-activity", &activity),
        timeline: source("example-timeline", &timeline),
        selection: None,
    }
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-sched-c1e/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}
