// SPDX-License-Identifier: AGPL-3.0-only

// #477 LF-1 (0.12.80; register §1.509; the coordinator's rulings 1 to 6 of 11-22, no words of the
// owner's behind them; coordinator work order `rd-1122/brief-477-lf1.md`;
// contracts/scheduling/README.md, "Task Reserves", "Capped spending order";
// contracts/remedies.md, section 4, "Spend values"): the turn cap a dispatch carries (`run_cap`)
// counts the lines of the pools its task consumes.
// - a valued pool whose lines are written takes part in a judged candidate's spend values whether
//   or not the task reserves it: its floor is the hard line in effect on the candidate's instance,
//   the larger of the hard line and the reserve where both are;
// - on the head of the pool's capped episode the turns are min(⌈(r − p − s) ÷ c⌉, ⌊allowance ÷ c⌋),
//   `s` the largest reading the episode's rule reads as spent (`r ≤ S` and `r < T`): the stop line,
//   or one below the trigger line when the stop line is the trigger line; everywhere else
//   ⌊allowance ÷ c⌋;
// - an unknown reading or line gives no `run_cap`; a pool without lines, a task the lines do not
//   constrain and a task with reserves only keep their values.
//
// The catalog is neutral and V2, written as SCHED-C1e's: per instance alias X, the pool
// `pool.neutral@X` of X, read from `resource.neutral`, its capacity read from
// `resource.neutral.capacity` (180), valued 1000 milli a unit (Q = 1), no regeneration; the produced
// pool `pool.out@X`, 1000 milli a unit; and each task as `<task>@X` of X, judged unless a test says
// otherwise, producing 30 of `pool.out` a run. The catalog's lines, where a test writes them, are
// the capacity's fact plus an offset. Every round is at NOW, the pool read a minute before.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::Outcome;
use serde_json::{Value, json};

use super::gate_ss6 as ss6;
use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::resource_targets::{LineView, PoolLines, ReserveView, SpendValues, spend_values_with};
use crate::{
    CandidateEligibility, CappedMemory, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DecisionReason, DispatchIntent, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactRefreshInputs, FactRefreshSettings, HostResourceSnapshot, InstanceHolds,
    InstanceLineSet, InstancePoolLines, NoProgressGuards, OwedReruns, PolicyCode, PolicyEvaluation,
    PolicyEvaluationError, PoolLine, RemedyPlan, RunSlots, SCHEDULING_SCHEMA_VERSION,
    SCHEDULING_SCHEMA_VERSION_V2, TaskDecision, TaskRuntimeSnapshot, TaskSpec, TaskTerminalState,
    compile_catalog, evaluate_with_instance_lines, project_fact_pools,
};

const A: &str = "fixture-instance-a";
const B: &str = "fixture-instance-b";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// The lined pool, as an instance's `scheduling.pool_lines` names it; each instance's copy is
/// `pool.neutral@<alias>`.
const POOL: &str = "pool.neutral";
/// The produced pool; each instance's copy is `pool.out@<alias>`.
const OUT: &str = "pool.out";
const READING_KEY: &str = "resource.neutral";
/// The capacity fact of the lined pool, in its scope.
const CAPACITY_KEY: &str = "resource.neutral.capacity";
const OUT_KEY: &str = "resource.out";
/// The neutral example catalog's golden identities, computed before SS-2h (as in `gate_ss2h`).
const V1_EXAMPLE_HASH: &str =
    "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c";
const V2_EXAMPLE_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";

// Test 1 (work order test 1), a lined pool and no reserve. The catalog's lines are 140 and 104 (the
// capacity less 40 and less 76), with no trigger line (the capacity, 180). `task_p` consumes 10 a
// run and reserves nothing. Read at 175 (below the trigger line: no episode) it runs at full price,
// and its `policy_dispatch_parameters` carries at its `consumes` entry 0 the allowance
// 175 − 104 = 71 and `run_cap` ⌊71 ÷ 10⌋ = 7. With `task_q` (20 a run, hourly) dispatched after the
// reading, its 20 are pending: 175 − 20 − 104 = 51, `run_cap` ⌊51 ÷ 10⌋ = 5.
#[test]
fn lf1_t1_a_lined_pool_without_a_reserve_floors_its_allowance_at_the_hard_line() {
    let alone = World::new(
        catalog(&[A], &[rule("task_p", 10)], &catalog_lines(None)),
        175,
    );
    let mut pending = World::new(
        catalog(
            &[A],
            &[
                rule("task_p", 10),
                Rule {
                    trigger: Trigger::Hourly,
                    ..rule("task_q", 20)
                },
            ],
            &catalog_lines(None),
        ),
        175,
    );
    pending.ran_after_the_reading("task_q", A);
    for (label, mut world, allowance, run_cap) in [
        ("alone", alone, 71, 7),
        ("task_q's 20 pending", pending, 51, 5),
    ] {
        let evaluation = world.evaluate();
        assert_eq!(
            dispatched(&evaluation),
            [copy("task_p", A)],
            "{label}: {evaluation:#?}"
        );
        assert!(
            !has(decision(&evaluation, "task_p", A), PolicyCode::CappedSpend),
            "{label}: no episode"
        );
        assert_eq!(
            carried(&evaluation, "task_p", A),
            (
                Some(json!([{"effect_index": 0, "spend_allowance": allowance}])),
                Some(json!(run_cap))
            ),
            "{label}"
        );
    }
}

// Test 2 (work order test 2), the head of a capped episode. `task_p` consumes 15 a run and is alone
// in its pool's order, so it is the head of the round (`capped_spend`, rank 1).
// - Lines 140 and 104, read at the capacity (180): to the stop line ⌈40 ÷ 15⌉ = 3 runs
//   (180 − 45 = 135 ≤ 140), where ⌊40 ÷ 15⌋ = 2 would stop at 150, above it, and the hard line
//   would allow ⌊76 ÷ 15⌋ = 5: `run_cap` 3; the allowance stays the hard line's, 76.
// - Lines 140 and 136, read at 180: three runs would leave 135, below the hard line, which allows
//   ⌊44 ÷ 15⌋ = 2: `run_cap` 2, the allowance 44.
// - The contract's rule ("Capped spending order": an episode is spent at `r ≤ S` and `r < T`): the
//   stop line at the trigger line (160 both) and the hard line 104, read at 175. One run leaves
//   160, not below the trigger line, so the episode needs ⌈(175 − 159) ÷ 15⌉ = 2 runs (145); the
//   hard line would allow ⌊71 ÷ 15⌋ = 4: `run_cap` 2, the allowance 71.
#[test]
fn lf1_t2_the_head_of_a_capped_episode_runs_to_its_stop_line_and_never_below_its_hard_line() {
    for (label, lines, reading, allowance, run_cap) in [
        ("past the stop line", catalog_lines(None), 180, 76, 3),
        (
            "the hard line binds",
            vec![("stop_line", line(-40)), ("hard_line", line(-44))],
            180,
            44,
            2,
        ),
        (
            "the stop line at the trigger line",
            vec![
                ("stop_line", line(-20)),
                ("hard_line", line(-76)),
                ("trigger_line", line(-20)),
            ],
            175,
            71,
            2,
        ),
    ] {
        let mut world = World::new(catalog(&[A], &[rule("task_p", 15)], &lines), reading);
        let evaluation = world.evaluate();
        assert_eq!(
            dispatched(&evaluation),
            [copy("task_p", A)],
            "{label}: {evaluation:#?}"
        );
        let head = spend(&evaluation, "task_p", A);
        assert_eq!(head.value("capped_rank"), Some(&json!(1)), "{label}");
        assert_eq!(
            world.memory.episode(A, &copy(POOL, A)),
            Some((1, 1)),
            "{label}"
        );
        assert_eq!(
            carried(&evaluation, "task_p", A),
            (
                Some(json!([{"effect_index": 0, "spend_allowance": allowance}])),
                Some(json!(run_cap))
            ),
            "{label}"
        );
    }
}

// Test 3 (work order test 3), below the trigger line. Lines 140 and 104 and the trigger line 160.
// Read at 155, below the trigger line with no episode under way, `task_p` (15 a run) runs at full
// price with no `capped_spend`, and its turns are the hard line's, ⌊(155 − 104) ÷ 15⌋ = 3, not the
// stop line's ⌈(155 − 140) ÷ 15⌉ = 1.
#[test]
fn lf1_t3_below_the_trigger_line_the_floor_is_the_hard_line() {
    let lines = catalog_lines(Some(line(-20)));
    let mut world = World::new(catalog(&[A], &[rule("task_p", 15)], &lines), 155);
    let evaluation = world.evaluate();
    assert_eq!(
        dispatched(&evaluation),
        [copy("task_p", A)],
        "{evaluation:#?}"
    );
    assert!(!has(
        decision(&evaluation, "task_p", A),
        PolicyCode::CappedSpend
    ));
    assert!(world.memory.is_empty());
    assert_eq!(
        carried(&evaluation, "task_p", A),
        (
            Some(json!([{"effect_index": 0, "spend_allowance": 51}])),
            Some(json!(3))
        )
    );
}

// Test 4 (work order test 4), a reserve beside the lines. Lines 140 and 104, read at 175; `task_r`
// consumes 10 a run and reserves the pool. A reserve of 120, above the hard line, is the floor:
// 175 − 120 = 55, `run_cap` 5. A reserve of 50, below it, gives way to the hard line: 71, `run_cap`
// 7.
#[test]
fn lf1_t4_the_larger_of_the_reserve_and_the_hard_line_is_the_floor() {
    for (at_least, allowance, run_cap) in [(120, 55, 5), (50, 71, 7)] {
        let reserving = Rule {
            reserve: Some(at_least),
            ..rule("task_r", 10)
        };
        let mut world = World::new(catalog(&[A], &[reserving], &catalog_lines(None)), 175);
        let evaluation = world.evaluate();
        assert_eq!(
            dispatched(&evaluation),
            [copy("task_r", A)],
            "reserve {at_least}: {evaluation:#?}"
        );
        assert_eq!(
            carried(&evaluation, "task_r", A),
            (
                Some(json!([{"effect_index": 0, "spend_allowance": allowance}])),
                Some(json!(run_cap))
            ),
            "reserve {at_least}"
        );
    }
}

// Test 5 (work order test 5), an instance's own lines. The catalog's lines are 140 and 104; A's
// `scheduling.pool_lines` gives the pool {150, 120}. Read at 175 on both, `task_p` (10 a run) runs
// on each: on A the floor is A's own hard line, 175 − 120 = 55, `run_cap` 5; on B the catalog's,
// 71, `run_cap` 7.
#[test]
fn lf1_t5_an_instances_own_lines_give_the_floor_on_that_instance() {
    let mut world = World::new(
        catalog(&[A, B], &[rule("task_p", 10)], &catalog_lines(None)),
        175,
    );
    world.lines.insert(A, POOL, set(150, 120));
    let evaluation = world.evaluate();
    let ran = dispatched(&evaluation);
    for (alias, allowance, run_cap) in [(A, 55, 5), (B, 71, 7)] {
        assert!(
            ran.contains(&copy("task_p", alias)),
            "{alias}: {evaluation:#?}"
        );
        assert_eq!(
            carried(&evaluation, "task_p", alias),
            (
                Some(json!([{"effect_index": 0, "spend_allowance": allowance}])),
                Some(json!(run_cap))
            ),
            "{alias}"
        );
    }
}

// Test 6 (work order test 6), an unknown value. The spend values of `task_p` (10 a run of the lined
// pool) and `task_r` (the same, reserving 50 of it), from the views the evaluation hands them. With
// the lines known (projected 175, hard line 104) `task_p` carries 71 and `run_cap` 7. While the
// pool's reading or one of its lines gives no value (`LineView::Unknown`; the lines' gate holds such
// a candidate) neither task carries an allowance or a `run_cap`, though `task_r`'s reserve view
// knows its projection: as a reserved pool whose projection is unknown gives none (beside it).
#[test]
fn lf1_t6_an_unknown_reading_or_line_gives_no_run_cap() {
    let compiled = catalog(
        &[A],
        &[
            rule("task_p", 10),
            Rule {
                reserve: Some(50),
                ..rule("task_r", 10)
            },
        ],
        &catalog_lines(None),
    );
    let pools = &compiled.catalog().pools.pools;
    let task_p = task_spec(&compiled, "task_p");
    let task_r = task_spec(&compiled, "task_r");
    let known = LineView::Known(PoolLines {
        projected: 175,
        consumes: 10,
        hard_line: 104,
        spent_at: None,
    });
    let lined = spend_values_with(task_p, None, |_| Some(known), |_| 0, pools, |_| None);
    assert_eq!(lined.spend_allowances, [(0, 71)], "{lined:?}");
    assert_eq!(lined.run_cap, Some(7), "{lined:?}");

    let unknown = spend_values_with(
        task_p,
        None,
        |_| Some(LineView::Unknown),
        |_| 0,
        pools,
        |_| None,
    );
    assert!(unknown.spend_allowances.is_empty(), "{unknown:?}");
    assert_eq!(unknown.run_cap, None, "{unknown:?}");

    let projected = [reserve_of(Some(175))];
    let reserved = spend_values_with(
        task_r,
        Some(projected.as_slice()),
        |_| Some(LineView::Unknown),
        |_| 0,
        pools,
        |_| None,
    );
    assert!(reserved.spend_allowances.is_empty(), "{reserved:?}");
    assert_eq!(reserved.run_cap, None, "{reserved:?}");

    let unprojected = [reserve_of(None)];
    let reserve_only = spend_values_with(
        task_r,
        Some(unprojected.as_slice()),
        |_| None,
        |_| 0,
        pools,
        |_| None,
    );
    assert!(reserve_only.spend_allowances.is_empty(), "{reserve_only:?}");
    assert_eq!(reserve_only.run_cap, None, "{reserve_only:?}");
}

// Test 7 (work order test 7), what does not change. The neutral example catalog keeps its V1 and V2
// golden identities. A valued pool that writes no lines (the defaults: its stop line the capacity,
// its hard line 0), read at its capacity (180): `task_p` (10 a run) runs as the head of its capped
// episode with the `capped_spend` of before, byte for byte, and its dispatch carries no
// `policy_dispatch_parameters`; `task_r` (the same, reserving 100) carries SS-6's values of its
// reserve, 180 − 100 = 80 and ⌊80 ÷ 10⌋ = 8, byte for byte the reason SS-6 builds from them. A task
// the lines do not constrain (`task_u`, not judged) on a lined pool carries none. SS-6's own fixture
// (pools without lines or a valuation): a task with reserves only carries, byte for byte, the reason
// of SS-6's values 900 and 30.
#[test]
fn lf1_t7_a_pool_without_lines_and_a_task_with_reserves_only_keep_their_values() {
    for (version, golden) in [
        (SCHEDULING_SCHEMA_VERSION, V1_EXAMPLE_HASH),
        (SCHEDULING_SCHEMA_VERSION_V2, V2_EXAMPLE_HASH),
    ] {
        let example = compile_catalog(&example(version))
            .unwrap_or_else(|failure| panic!("{version}: {:?}", failure.diagnostics()));
        assert_eq!(example.catalog_hash(), golden, "{version}");
    }

    let mut plain = World::new(catalog(&[A], &[rule("task_p", 10)], &[]), 180);
    let evaluation = plain.evaluate();
    assert_eq!(
        dispatched(&evaluation),
        [copy("task_p", A)],
        "{evaluation:#?}"
    );
    let head = reason(decision(&evaluation, "task_p", A), PolicyCode::CappedSpend);
    assert_eq!(
        head.detail,
        format!(
            "pool_id={} pool_capacity=180 stop_line=180 hard_line=0 capped_episode=1 \
             capped_round=1 capped_rank=1 pool_reading=180 lines_source=default",
            copy(POOL, A)
        )
    );
    assert!(
        !has(
            decision(&evaluation, "task_p", A),
            PolicyCode::PolicyDispatchParameters
        ),
        "{evaluation:#?}"
    );

    let reserving = Rule {
        reserve: Some(100),
        ..rule("task_r", 10)
    };
    let mut reserved = World::new(catalog(&[A], &[reserving], &[]), 180);
    let evaluation = reserved.evaluate();
    assert_eq!(
        dispatched(&evaluation),
        [copy("task_r", A)],
        "{evaluation:#?}"
    );
    assert!(has(
        decision(&evaluation, "task_r", A),
        PolicyCode::CappedSpend
    ));
    let given = reason(
        decision(&evaluation, "task_r", A),
        PolicyCode::PolicyDispatchParameters,
    );
    let expected = ss6_reason(vec![(0, 80)], 8);
    assert_eq!(given, &expected);
    assert_eq!(bytes(&given.outcome), bytes(&expected.outcome));

    let unmarked = Rule {
        judged: false,
        ..rule("task_u", 10)
    };
    let mut unjudged = World::new(catalog(&[A], &[unmarked], &catalog_lines(None)), 175);
    let evaluation = unjudged.evaluate();
    assert_eq!(
        dispatched(&evaluation),
        [copy("task_u", A)],
        "{evaluation:#?}"
    );
    assert!(
        !has(
            decision(&evaluation, "task_u", A),
            PolicyCode::PolicyDispatchParameters
        ),
        "{evaluation:#?}"
    );

    let declared = ss6::TaskDecl {
        consumes: &[(ss6::POOL_A, 30)],
        reserves: &[(ss6::POOL_A, 100)],
        ..ss6::TaskDecl::new(ss6::TASK_A, 100)
    };
    let fixture = ss6::catalog(&[ss6::task(&declared)]);
    let evaluation = ss6::round(&fixture, vec![ss6::reading_of(ss6::POOL_A_KEY, 1000)]);
    let given = ss6::parameters(&evaluation, ss6::TASK_A)
        .unwrap_or_else(|| panic!("SS-6's values: {evaluation:#?}"));
    let expected = ss6_reason(vec![(0, 900)], 30);
    assert_eq!(given, &expected);
    assert_eq!(bytes(&given.outcome), bytes(&expected.outcome));
}

// ---- The world a test drives ----

/// The catalog, the Runtime's memory of the capped episodes, the lines the instances set, the
/// instances, each one's reading of its lined pool, and the tasks' snapshots.
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    lines: InstancePoolLines,
    instances: Vec<&'static str>,
    readings: BTreeMap<&'static str, i64>,
    read_at: u64,
    states: Vec<TaskRuntimeSnapshot>,
}

impl World {
    /// The world at NOW over the instances `catalog` names, each lined pool read at `reading` a
    /// minute before; no instance sets lines and no task ran.
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
            states: Vec::new(),
        }
    }

    /// `task`'s copy on `alias` succeeded, dispatched half a minute after the pool's reading (so the
    /// reading does not show its consumption) and after its clock's latest occurrence.
    fn ran_after_the_reading(&mut self, task: &str, alias: &str) {
        self.states.push(TaskRuntimeSnapshot {
            task_id: copy(task, alias),
            instance_id: alias.to_owned(),
            last_dispatched_unix_ms: Some(self.read_at + 30_000),
            eligible_since_unix_ms: None,
            terminal_state: Some(TaskTerminalState::Succeeded),
            completed_window: None,
            last_duration_ms: None,
            failure_streak: None,
            expected_runtime_ms: None,
            expected_runtime_runs: None,
        });
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
            fact_snapshot_id: "snapshot:fixture-lf1".to_owned(),
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

    /// One round at NOW by the Runtime's entry, with its memory and the instances' sets, every
    /// candidate admitted.
    fn evaluate(&mut self) -> PolicyEvaluation {
        let facts = self.facts();
        let resources = resources(&self.catalog, &facts, &self.instances);
        evaluate_with_instance_lines::<PolicyEvaluationError>(
            &self.catalog,
            &facts,
            &resources,
            time(NOW),
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

/// An instance's set of constant lines, without a trigger line.
fn set(stop_line: u64, hard_line: u64) -> InstanceLineSet {
    InstanceLineSet {
        stop_line: PoolLine::Constant(stop_line),
        hard_line: PoolLine::Constant(hard_line),
        trigger_line: None,
    }
}

/// The catalog's line: the capacity fact plus `offset`.
fn line(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

/// The catalog's lines of the lined pool: 40 and 76 below its capacity, and `trigger` its trigger
/// line when given.
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

/// The `capped_spend` of the dispatch of `task`'s copy on `alias`.
fn spend<'a>(evaluation: &'a PolicyEvaluation, task: &str, alias: &str) -> &'a Outcome {
    &reason(decision(evaluation, task, alias), PolicyCode::CappedSpend).outcome
}

/// The `spend_allowances` and the `run_cap` the dispatch of `task`'s copy on `alias` carries on its
/// `policy_dispatch_parameters`.
fn carried(
    evaluation: &PolicyEvaluation,
    task: &str,
    alias: &str,
) -> (Option<Value>, Option<Value>) {
    let parameters = &reason(
        decision(evaluation, task, alias),
        PolicyCode::PolicyDispatchParameters,
    )
    .outcome;
    (
        parameters.value("spend_allowances").cloned(),
        parameters.value("run_cap").cloned(),
    )
}

/// The `policy_dispatch_parameters` SS-6 builds from these values alone, its detail the outcome's
/// text.
fn ss6_reason(spend_allowances: Vec<(usize, u64)>, run_cap: u64) -> DecisionReason {
    let outcome = SpendValues {
        spend_allowances,
        gaps_remaining: Vec::new(),
        run_cap: Some(run_cap),
    }
    .add_to(Outcome::new(PolicyCode::PolicyDispatchParameters));
    DecisionReason {
        detail: outcome.to_string(),
        outcome,
    }
}

fn bytes(outcome: &Outcome) -> Vec<u8> {
    serde_json::to_vec(outcome).expect("the outcome's bytes")
}

/// The compiled task of `task`'s copy on A.
fn task_spec<'a>(catalog: &'a CompiledCatalog, task: &str) -> &'a TaskSpec {
    let task_id = copy(task, A);
    catalog
        .catalog()
        .tasks
        .tasks
        .iter()
        .find(|spec| spec.id == task_id)
        .unwrap_or_else(|| panic!("the fixture task {task_id}"))
}

/// The reserve view of A's lined pool for a task that reserves 50 of it and consumes 10 a run,
/// projected at `projected`.
fn reserve_of(projected: Option<u64>) -> ReserveView {
    ReserveView {
        pool_id: copy(POOL, A),
        at_least: 50,
        consumes: 10,
        after: projected.and_then(|projected| i64::try_from(projected - 10).ok()),
        projected,
        next_change_unix_ms: None,
    }
}

// ---- The catalog ----

/// When a task is due: while its instance's lined pool holds what it consumes, or every hour.
#[derive(Clone, Copy)]
enum Trigger {
    Enough,
    Hourly,
}

/// A task of every instance: `consumed` units of the lined pool a run, 30 of `pool.out`, due by
/// `trigger`, reserving `reserve` of the lined pool when given, judged when `judged`.
struct Rule {
    id: &'static str,
    consumed: u64,
    trigger: Trigger,
    reserve: Option<u64>,
    judged: bool,
}

/// A judged task due while the pool holds what it consumes, reserving nothing.
fn rule(id: &'static str, consumed: u64) -> Rule {
    Rule {
        id,
        consumed,
        trigger: Trigger::Enough,
        reserve: None,
        judged: true,
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

/// The copy of `rule` for the instance `alias`.
fn task_copy(alias: &str, rule: &Rule) -> Value {
    let task_id = copy(rule.id, alias);
    let trigger = match rule.trigger {
        Trigger::Enough => json!({
            "kind": "resource_projection",
            "pool_id": copy(POOL, alias),
            "comparison": "greater_than_or_equal",
            "value": rule.consumed
        }),
        Trigger::Hourly => hourly(),
    };
    let mut task = json!({
        "id": task_id,
        "scope": {"kind": "instance", "instance_id": alias},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{}", rule.id),
        "priority": 100,
        "trigger": trigger,
        "feedback_stop": {
            "kind": "outcome",
            "task_id": task_id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [effect(&copy(POOL, alias), "consume", rule.consumed)],
        "produces": [effect(&copy(OUT, alias), "produce", 30)],
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
    });
    if rule.judged {
        task["benefit_judged"] = json!(true);
    }
    if let Some(at_least) = rule.reserve {
        task["reserves"] = json!([{"pool_id": copy(POOL, alias), "at_least": at_least}]);
    }
    task
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

/// The catalog of `instances`: each instance's task copies, its produced pool and its lined pool
/// (capacity read from its fact) writing `lines`.
fn catalog(instances: &[&str], tasks: &[Rule], lines: &[(&str, Value)]) -> CompiledCatalog {
    let mut task_entries = Vec::new();
    let mut pool_entries = Vec::new();
    for alias in instances {
        task_entries.extend(tasks.iter().map(|task| task_copy(alias, task)));
        pool_entries.push(pool_copy(OUT, alias, OUT_KEY, json!(1_000_000)));
        let mut lined = pool_copy(POOL, alias, READING_KEY, line(0));
        for (member, value) in lines {
            lined[*member] = value.clone();
        }
        pool_entries.push(lined);
    }
    let header = json!({
        "catalog_id": "fixture.lf1",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-lf1"]
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
        tasks: source("tasks", &document("tasks", Value::Array(task_entries))),
        pools: source("pools", &document("pools", Value::Array(pool_entries))),
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
        format!("memory://fixture-lf1/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}
