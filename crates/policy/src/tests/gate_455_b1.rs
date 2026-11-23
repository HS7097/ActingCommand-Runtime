// SPDX-License-Identifier: AGPL-3.0-only

// #455 B1 (0.12.82; register §1.518; the coordinator's rulings of 11-23, no words of the owner's
// behind them; coordinator work order `rd-1123/brief-455-b1.md`; contracts/scheduling/README.md,
// "Fixed values written in the pack", "Net benefit within a tier", "Capped spending order";
// contracts/resource-targets.md, "Resource weights (v2)"): a task with a fixed value counts a
// goal's gap on the pools it produces that declare no valuation.
// - each such pool a goal in force covers with an open gap adds its production term `r·Γ/(1000·S)`
//   to `B = V − cost`, at the goal's own scale `S` and importance (`W = B + Γ`, `B` 0 without a
//   valuation), so of two stages of one value and one cost the one that produces the lacking pool
//   goes first, in the business tier and in a capped round alike (tests 1 and 4);
// - a met goal and an unknown reading add nothing (test 2); without a goal every decision is byte
//   for byte the one of a task that declares no such production (test 3);
// - an expected yield brings its rounding bound, and the judgement's items name the pool (test 5);
// - the goal's spend values (`gap_remaining`, its part in `run_cap`) are SS-6's and LF-1's (test 7).
//
// The catalog is neutral and V2, as SCHED-C1a's gate: judged tasks on one instance consuming 30 a
// run of the capped pool `pool.neutral` read from `resource.neutral`, its capacity read from
// `resource.neutral.capacity` (180), valued 1000 milli a unit (Q = 1), no regeneration, its stop
// line the capacity less 40 (140) and its hard line the capacity less 76 (104); the lacking pool
// `pool.lacking`, read from `resource.lacking`, declares no valuation. The goal is a v3 record on
// the lacking pool, at least 40, its own scale 10 and importance 1000: read at 10, the gap is 30
// and `Γ = ⌊1000·30/10⌋ = 3000`; a stage producing 2 a run adds `2000·3000/(10·1000) = 600`.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{Initiator, RESOURCE_TARGETS_FACT_KEY, RuntimeChannel};
use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::resource_targets::SpendValues;
use crate::{
    CandidateEligibility, CappedMemory, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DecisionReason, DispatchIntent, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactRefreshInputs, FactRefreshSettings, FactValue, HostResourceSnapshot,
    InstanceHolds, NoProgressGuards, ObservedFact, OwedReruns, PolicyCode, PolicyEvaluation,
    PolicyEvaluationError, RemedyPlan, RunSlots, SCHEDULING_SCHEMA_VERSION,
    SCHEDULING_SCHEMA_VERSION_V2, ScopeSelector, TargetSetter, TaskDecision,
    check_resource_targets, compile_catalog, evaluate_with_capped_spending, parse_resource_targets,
    project_fact_pools,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// The capped pool, read from `READING_KEY`.
const POOL: &str = "pool.neutral";
const READING_KEY: &str = "resource.neutral";
/// The capacity fact of the capped pool, in its scope.
const CAPACITY_KEY: &str = "resource.neutral.capacity";
/// The lacking pool, without a valuation, read from `LACKING_KEY`.
const LACKING: &str = "pool.lacking";
const LACKING_KEY: &str = "resource.lacking";
/// The goal on the lacking pool: at least `AT_LEAST`, its own scale and importance.
const AT_LEAST: i64 = 40;
const GOAL_SCALE: u64 = 10;
const IMPORTANCE_MILLI: u64 = 1000;
/// The declared duration of every task, `E` without a snapshot.
const E: u64 = 60_000;
/// The neutral example catalog's golden identities, computed before SS-2h (as in `gate_ss2h`).
const V1_EXAMPLE_HASH: &str =
    "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c";
const V2_EXAMPLE_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";

// Test 1, the goal counts. Read at 150 (below the trigger line), `task_a` and, declared after it,
// `task_b` each consume 30 at W = 1000 with `V` 40 000, so each is `B = 40 000 − 30 000 = 10 000`;
// `task_b` also produces 2 of the lacking pool a run. With the goal (gap 30) `task_b` is
// `B = 10 000 + 600 = 10 600` and is selected, its `rank_breakdown` naming B and V; `task_a` keeps
// 10 000 and waits `instance_already_selected`. Without the goal both are 10 000 and the one
// declared first, `task_a`, is selected, as before.
#[test]
fn b1_t1_a_goal_on_a_lacking_pool_adds_its_term_and_its_stage_goes_first() {
    let mut goal = World::new(stages(40_000), 150, Some(10)).with_goal();
    let evaluation = goal.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_b"], "{evaluation:#?}");
    let winner = decision(&evaluation, "task_b");
    fixed_first(winner, 10_600, 40_000, E);
    let breakdown = reason(winner, PolicyCode::RankBreakdown);
    assert_eq!(
        breakdown.outcome.value("benefit_milli"),
        Some(&json!(10_600))
    );
    assert!(
        breakdown
            .detail
            .ends_with(" benefit_milli=10600 expected_duration_ms=60000 fixed_value_milli=40000"),
        "{}",
        breakdown.detail
    );
    let other = decision(&evaluation, "task_a");
    assert!(
        last(other).outcome.is(PolicyCode::InstanceAlreadySelected),
        "{other:#?}"
    );
    fixed_first(other, 10_000, 40_000, E);

    let mut plain = World::new(stages(40_000), 150, Some(10));
    let evaluation = plain.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_a"], "{evaluation:#?}");
    fixed_first(decision(&evaluation, "task_a"), 10_000, 40_000, E);
    fixed_first(decision(&evaluation, "task_b"), 10_000, 40_000, E);
}

// Test 2, nothing to count. With the goal met (the lacking pool read at 40) or its reading unknown
// (no reading: the goal is pending, `Γ = 0`), both stages are `B = 10 000` and `task_a`, declared
// first, is selected, as without the goal; the resource layer says why (`satisfied`, `pending`).
#[test]
fn b1_t2_a_met_goal_or_an_unknown_reading_leaves_both_stages_as_before() {
    for (case, lacking, said) in [
        (
            "met",
            Some(AT_LEAST),
            "goal-lacking@pool.lacking:satisfied current=40 at_least=40 gap=0 gap_weight=0",
        ),
        (
            "unknown",
            None,
            "goal-lacking@pool.lacking:pending reason=missing gap_weight=pending",
        ),
    ] {
        let mut world = World::new(stages(40_000), 150, lacking).with_goal();
        let evaluation = world.evaluate();
        assert_eq!(
            dispatched(&evaluation),
            ["task_a"],
            "{case}: {evaluation:#?}"
        );
        fixed_first(decision(&evaluation, "task_a"), 10_000, 40_000, E);
        let other = decision(&evaluation, "task_b");
        assert!(
            last(other).outcome.is(PolicyCode::InstanceAlreadySelected),
            "{case}: {other:#?}"
        );
        fixed_first(other, 10_000, 40_000, E);
        let targets = reason(other, PolicyCode::ResourceTargets);
        assert!(targets.detail.contains(said), "{case}: {}", targets.detail);
    }
}

// Test 3, no goal, no change. The neutral example catalog keeps its V1 and V2 golden identities.
// Without a goal, the two stages of test 1 decide byte for byte as the same two stages of which
// neither declares a production (whose decisions nothing of this slice reaches), the lacking pool's
// reading known or unknown: the same decisions' bytes, the same dispatch and the same wake.
#[test]
fn b1_t3_without_a_goal_every_decision_is_byte_for_byte_the_one_of_before() {
    for (version, golden) in [
        (SCHEDULING_SCHEMA_VERSION, V1_EXAMPLE_HASH),
        (SCHEDULING_SCHEMA_VERSION_V2, V2_EXAMPLE_HASH),
    ] {
        let compiled = compile_catalog(&example(version))
            .unwrap_or_else(|failure| panic!("{version}: {:?}", failure.diagnostics()));
        assert_eq!(compiled.catalog_hash(), golden, "{version}");
    }
    let round = |compiled: CompiledCatalog, lacking: Option<i64>| {
        let mut world = World::new(compiled, 150, lacking);
        let evaluation = world.evaluate();
        (
            serde_json::to_vec(&evaluation.decisions).expect("decisions"),
            dispatched(&evaluation)
                .iter()
                .map(|task_id| (*task_id).to_owned())
                .collect::<Vec<_>>(),
            evaluation.next_wake_unix_ms,
        )
    };
    let unproducing = || {
        catalog(
            vec![
                fixed(judged("task_a", 30), 40_000),
                fixed(judged("task_b", 30), 40_000),
            ],
            pools(),
        )
    };
    let before = round(unproducing(), Some(10));
    assert_eq!(before.1, ["task_a"]);
    assert_eq!(round(stages(40_000), Some(10)), before, "a known reading");
    assert_eq!(round(stages(40_000), None), before, "an unknown reading");
}

// Test 4, the capped spending order. Read at 200 (above the trigger line, the capacity 180), both
// stages with `V` 36 000 are listed by their value without the pool's cost over that cost: with
// the goal `task_b`'s 36 600 ÷ 30 against `task_a`'s 36 000 ÷ 30, so `task_b` is frozen first and
// spent as the head (`capped_spend`, rank 1), its B = 36 600 − 1000·(30 − 30), while `task_a`
// waits in the block (`capped_not_head`) with its own 36 000. Without the goal the two tie and the
// order takes the task ids: `task_a` first, `task_b` waiting with 36 000, as before.
#[test]
fn b1_t4_in_a_capped_round_the_stage_that_produces_the_lacking_pool_is_frozen_first() {
    let mut goal = World::new(stages(36_000), 200, Some(10)).with_goal();
    let evaluation = goal.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_b"], "{evaluation:#?}");
    let head = spend(&evaluation, "task_b");
    assert_eq!(head.value("capped_rank"), Some(&json!(1)));
    assert_eq!(head.value("capped_round"), Some(&json!(1)));
    fixed_first(decision(&evaluation, "task_b"), 36_600, 36_000, E);
    let waits = decision(&evaluation, "task_a");
    assert_eq!(
        hold(waits).as_deref(),
        Some("capped_not_head"),
        "{waits:#?}"
    );
    fixed_first(waits, 36_000, 36_000, E);

    let mut plain = World::new(stages(36_000), 200, Some(10));
    let evaluation = plain.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_a"], "{evaluation:#?}");
    assert_eq!(
        spend(&evaluation, "task_a").value("capped_rank"),
        Some(&json!(1))
    );
    let waits = decision(&evaluation, "task_b");
    assert_eq!(
        hold(waits).as_deref(),
        Some("capped_not_head"),
        "{waits:#?}"
    );
    fixed_first(waits, 36_000, 36_000, E);
}

// Test 5, an expected yield. `task_e` consumes 30 at W = 1000 and expects 2.5 of the lacking pool a
// run (`expected_amount_milli` 2500). With the goal its term is 2500·3000/(10·1000) = 750 and its
// rounding bound [2·2500 + 1·(2·3000 + 1)]/(4000·10) = 11001/40000 beside the consumption's 15: with
// `V` 29 250, S = 29 250 + 750 − 30 000 = 0 within ε = 611001/40000, `benefit_within_tolerance`
// (⌈ε⌉ 16, `unmarked`) whose items name the lacking pool first; with `V` 40 000, B = 10 750 and it
// is dispatched. Without the goal the lacking pool is not read: with `V` 29 250, B = −750,
// `policy_benefit_nonpositive`, its items the consumption's alone.
#[test]
fn b1_t5_an_expected_yield_brings_its_rounding_bound_and_its_item_names_the_pool() {
    let stage = |value: i64| {
        catalog(
            vec![fixed(
                with(
                    judged("task_e", 30),
                    "produces",
                    json!([produce_expected(LACKING, 2_500)]),
                ),
                value,
            )],
            pools(),
        )
    };
    let mut zero = World::new(stage(29_250), 150, Some(10)).with_goal();
    let evaluation = zero.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let within = decision(&evaluation, "task_e");
    let band = last(within);
    assert!(
        band.outcome.is(PolicyCode::BenefitWithinTolerance),
        "{within:#?}"
    );
    assert_eq!(band.outcome.value("benefit_milli"), Some(&json!(0)));
    assert_eq!(band.outcome.value("tolerance_milli"), Some(&json!(16)));
    assert_eq!(band.outcome.value("zero_tier"), Some(&json!("unmarked")));
    assert_eq!(
        band.detail,
        "benefit_milli=0 tolerance_milli=16 zero_tier=unmarked sum=0/1 \
         tolerance=611001/40000 items=pool.lacking:+750,pool.neutral:-30000"
    );
    fixed_first(within, 0, 29_250, E);

    let mut positive = World::new(stage(40_000), 150, Some(10)).with_goal();
    let evaluation = positive.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_e"], "{evaluation:#?}");
    fixed_first(decision(&evaluation, "task_e"), 10_750, 40_000, E);

    let mut plain = World::new(stage(29_250), 150, Some(10));
    let evaluation = plain.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, "task_e");
    let negative = last(held);
    assert!(
        negative.outcome.is(PolicyCode::PolicyBenefitNonpositive),
        "{held:#?}"
    );
    assert_eq!(
        negative.detail,
        "benefit_milli=-750 expected_duration_ms=60000 items=pool.neutral:-30000"
    );
    fixed_first(held, -750, 29_250, E);
}

// Test 7, the goal's spend values. Below the trigger line (test 1's round) `task_b` is handed the
// gap 30 at its production entry and `run_cap` 1: the hard line allows ⌊(150 − 104) ÷ 30⌋ = 1 run,
// fewer than the ⌈30 ÷ 2⌉ = 15 the goal still needs. As the head of a capped round (read at 200)
// with the lacking pool read at 38, the gap is 2 (`Γ` 200, its term 40, B = 36 040) and the goal's
// ⌈2 ÷ 2⌉ = 1 run is fewer than the lines allow: `run_cap` 1. Each value is the one SS-6 and LF-1
// build from the same inputs.
#[test]
fn b1_t7_the_goals_spend_values_are_those_of_before() {
    let mut below = World::new(stages(40_000), 150, Some(10)).with_goal();
    let evaluation = below.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_b"], "{evaluation:#?}");
    spend_values_are(&evaluation, "task_b", 30, 1);

    let mut capped = World::new(stages(36_000), 200, Some(38)).with_goal();
    let evaluation = capped.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_b"], "{evaluation:#?}");
    fixed_first(decision(&evaluation, "task_b"), 36_040, 36_000, E);
    spend_values_are(&evaluation, "task_b", 2, 1);
}

// ---- The world a test drives ----

/// The catalog, the Runtime's memory of the capped episodes, the two pools' readings and the
/// instance's goal record.
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    /// The capped pool's reading.
    reading: i64,
    /// The lacking pool's reading; `None` while it is unknown.
    lacking: Option<i64>,
    /// The instance's `session.resource_targets` record; `None` without a goal.
    goal: Option<ObservedFact>,
}

impl World {
    /// The world at NOW without a goal, the capped pool read at `reading` and the lacking pool at
    /// `lacking`, each a minute before.
    fn new(compiled: CompiledCatalog, reading: i64, lacking: Option<i64>) -> Self {
        Self {
            catalog: compiled,
            memory: CappedMemory::default(),
            reading,
            lacking,
            goal: None,
        }
    }

    /// The same world with the goal on the lacking pool, set an hour before.
    fn with_goal(mut self) -> Self {
        self.goal = Some(goal_record(&self.catalog));
        self
    }

    fn facts(&self) -> EvaluationFacts {
        let mut observed = vec![
            fact(A, READING_KEY, self.reading, NOW - MINUTE_MS, None),
            fact(A, CAPACITY_KEY, 180, NOW - HOUR_MS, None),
        ];
        if let Some(lacking) = self.lacking {
            observed.push(fact(A, LACKING_KEY, lacking, NOW - MINUTE_MS, None));
        }
        observed.extend(self.goal.clone());
        EvaluationFacts {
            ledger_position: 7,
            fact_snapshot_id: "snapshot:fixture-goal-gap".to_owned(),
            facts: observed,
            outcomes: Vec::new(),
            tasks: Vec::new(),
            instances: vec![instance(A, &[])],
            priority_offsets: Vec::new(),
        }
    }

    /// One round by the Runtime's entry, with its memory, every candidate admitted.
    fn evaluate(&mut self) -> PolicyEvaluation {
        let facts = self.facts();
        let resource_snapshot = resources(&self.catalog, &facts);
        evaluate_with_capped_spending::<PolicyEvaluationError>(
            &self.catalog,
            &facts,
            &resource_snapshot,
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
            |_, _, _| Ok(false),
            |_: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
                Ok(CandidateEligibility::Eligible)
            },
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
    }
}

/// The v3 goal on the lacking pool (at least `AT_LEAST`, scale `GOAL_SCALE`, importance
/// `IMPORTANCE_MILLI`, no tasks named, no lifetime), checked against `compiled` and stored as the
/// formal entry stores it for a Lab connection, an hour before NOW.
fn goal_record(compiled: &CompiledCatalog) -> ObservedFact {
    let document = json!({
        "schema_version": "actingcommand.resource-targets.v3",
        "instance": A,
        "targets": [{
            "id": "goal-lacking",
            "resource": LACKING,
            "condition": {"kind": "at_least", "amount": AT_LEAST},
            "scale": GOAL_SCALE,
            "importance_milli": IMPORTANCE_MILLI,
            "apply": {"mode": "adjust", "weight": "score_stage"}
        }]
    });
    let parsed = parse_resource_targets(&serde_json::to_vec(&document).expect("document bytes"))
        .unwrap_or_else(|error| panic!("the goal parses: {error:?}"));
    let set_at = NOW - HOUR_MS;
    let checked = check_resource_targets(
        &parsed,
        compiled,
        &EvaluationFacts {
            ledger_position: 7,
            fact_snapshot_id: "snapshot:fixture-goal-gap".to_owned(),
            facts: Vec::new(),
            outcomes: Vec::new(),
            tasks: Vec::new(),
            instances: vec![instance(A, &[])],
            priority_offsets: Vec::new(),
        },
        time(set_at),
    )
    .unwrap_or_else(|error| panic!("the goal is accepted: {error:?}"));
    let composed = checked
        .compose(
            None,
            Some(TargetSetter {
                set_unix_ms: set_at,
                channel: RuntimeChannel::Lab,
                initiator: Some(Initiator::LabCli),
            }),
        )
        .unwrap_or_else(|error| panic!("the goal composes: {error:?}"))
        .expect("with a setter every document composes");
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: RESOURCE_TARGETS_FACT_KEY.to_owned(),
        value: FactValue::RecordList(
            serde_json::from_value(serde_json::to_value(&composed.rows).expect("rows"))
                .expect("record list"),
        ),
        observed_at_unix_ms: set_at,
        expires_at_unix_ms: composed.expires_at_unix_ms,
        confidence_milli: 1000,
    }
}

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

fn resources(compiled: &CompiledCatalog, facts: &EvaluationFacts) -> EvaluationResources {
    project_fact_pools(
        compiled,
        facts,
        &EvaluationResources {
            pools: Vec::new(),
            hosts: vec![HostResourceSnapshot {
                host_id: format!("host.{A}"),
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

// ---- Assertions ----

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
        .find(|entry| entry.task_id == task_id && entry.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}: {evaluation:#?}"))
}

fn reason(entry: &TaskDecision, code: PolicyCode) -> &DecisionReason {
    entry
        .reasons
        .iter()
        .find(|given| given.outcome.is(code))
        .unwrap_or_else(|| panic!("a {} reason on {entry:#?}", code.as_str()))
}

fn last(entry: &TaskDecision) -> &DecisionReason {
    entry
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {entry:#?}"))
}

/// The decision opens with exactly one `benefit_fixed_value`, of `B`, `V` and `E`.
fn fixed_first(entry: &TaskDecision, benefit: i64, value: i64, expected_ms: u64) {
    let first = entry
        .reasons
        .first()
        .unwrap_or_else(|| panic!("a reason on {entry:#?}"));
    assert!(
        first.outcome.is(PolicyCode::BenefitFixedValue),
        "{entry:#?}"
    );
    assert_eq!(
        first.outcome.value("benefit_milli"),
        Some(&json!(benefit)),
        "{entry:#?}"
    );
    assert_eq!(
        first.outcome.value("fixed_value_milli"),
        Some(&json!(value))
    );
    assert_eq!(
        first.outcome.value("expected_duration_ms"),
        Some(&json!(expected_ms))
    );
    let count = entry
        .reasons
        .iter()
        .filter(|given| given.outcome.is(PolicyCode::BenefitFixedValue))
        .count();
    assert_eq!(count, 1, "{entry:#?}");
}

/// The `pool_line_hold` of the decision's last reason, when it is `pool_line_held`.
fn hold(entry: &TaskDecision) -> Option<String> {
    let held = last(entry);
    if !held.outcome.is(PolicyCode::PoolLineHeld) {
        return None;
    }
    held.outcome
        .value("pool_line_hold")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The `capped_spend` reason of `task_id`'s dispatch.
fn spend<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a Outcome {
    &reason(decision(evaluation, task_id), PolicyCode::CappedSpend).outcome
}

/// `task_id`'s dispatch is handed `gap` at its production entry 0 and `run_cap`, as SS-6 builds
/// them (`policy_dispatch_parameters`).
fn spend_values_are(evaluation: &PolicyEvaluation, task_id: &str, gap: u64, run_cap: u64) {
    let given = &reason(
        decision(evaluation, task_id),
        PolicyCode::PolicyDispatchParameters,
    )
    .outcome;
    let built = SpendValues {
        spend_allowances: Vec::new(),
        gaps_remaining: vec![(0, gap)],
        run_cap: Some(run_cap),
    }
    .add_to(Outcome::new(PolicyCode::PolicyDispatchParameters));
    for key in ["gaps_remaining", "run_cap"] {
        assert_eq!(given.value(key), built.value(key), "{key}: {given:?}");
    }
}

// ---- The catalog ----

fn effect(pool: &str, direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

fn produce(pool: &str, amount: u64) -> Value {
    effect(pool, "produce", amount)
}

fn consume(pool: &str, amount: u64) -> Value {
    effect(pool, "consume", amount)
}

/// A production of `milli` thousandths of a unit expected per run.
fn produce_expected(pool: &str, milli: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": "produce",
        "expected_amount_milli": milli,
        "observation_source": "self_reported",
        "confidence_milli": 1000
    })
}

/// Due while the capped pool holds `amount`.
fn enough(amount: u64) -> Value {
    json!({
        "kind": "resource_projection",
        "pool_id": POOL,
        "comparison": "greater_than_or_equal",
        "value": amount
    })
}

/// A judged task of the game consuming `consumed` units of the capped pool a run, due while the
/// pool holds them, of priority 100.
fn judged(id: &str, consumed: u64) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
        "priority": 100,
        "trigger": enough(consumed),
        "feedback_stop": {
            "kind": "outcome",
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [consume(POOL, consumed)],
        "produces": [],
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": DAY_MS,
        "yield_points": [],
        "expected_duration_ms": E,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": [],
        "benefit_judged": true
    })
}

/// `task` with the fixed value `value`.
fn fixed(task: Value, value: i64) -> Value {
    with(task, "benefit_fixed_value_milli", json!(value))
}

fn with(mut task: Value, name: &str, value: Value) -> Value {
    task[name] = value;
    task
}

/// `task_a` and, declared after it, `task_b`: each judged, consuming 30 of the capped pool a run,
/// with the fixed value `value`; `task_b` also produces 2 of the lacking pool a run.
fn stages(value: i64) -> CompiledCatalog {
    catalog(
        vec![
            fixed(judged("task_a", 30), value),
            fixed(
                with(
                    judged("task_b", 30),
                    "produces",
                    json!([produce(LACKING, 2)]),
                ),
                value,
            ),
        ],
        pools(),
    )
}

/// A pool of the instance that does not regenerate, read from `resource.<the id without
/// "pool.">`, of constant `capacity`, weighed `weight` milli a unit.
fn valued(id: &str, weight: u32, capacity: u64) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": capacity,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": weight}
    })
}

/// The capacity fact plus `offset`.
fn line(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

/// The capped pool, its capacity read from its fact, its stop line 40 below the cap and its hard
/// line 76 below it; and the lacking pool, [`valued`] without a valuation.
fn pools() -> Value {
    let mut capped = valued(POOL, 1000, 1);
    capped["capacity"] = line(0);
    capped["stop_line"] = line(-40);
    capped["hard_line"] = line(-76);
    let mut lacking = valued(LACKING, 1000, 1000);
    lacking
        .as_object_mut()
        .expect("a pool object")
        .shift_remove("valuation");
    json!([capped, lacking])
}

fn catalog(tasks: Vec<Value>, pool_list: Value) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.goal-gap",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-goal-gap"]
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
        pools: source("pools", &document("pools", pool_list)),
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
    let [tasks, example_pools, activity, timeline] = documents;
    CatalogSources {
        tasks: source("example-tasks", &tasks),
        pools: source("example-pools", &example_pools),
        activity: source("example-activity", &activity),
        timeline: source("example-timeline", &timeline),
        selection: None,
    }
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-goal-gap/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}
