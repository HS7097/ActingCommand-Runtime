// SPDX-License-Identifier: AGPL-3.0-only

// #503 FIXV (0.12.101; register §1.550; the coordinator's rulings on the owner's direction of
// 11-28, in its work order; contracts/scheduling/README.md, "Fixed values written in the pack";
// contracts/standard-pack-schedule.md, "The fixed value", "`requires`"): a task's fixed value may
// be 0 or negative and may stand beside productions into priced pools; with a fixed value the base
// value is that value alone, and the productions feed only the goal layer.
// - a value of any sign compiles and is written as it is in the canonical bytes (test 1);
// - the gate reads a value of any sign: 0 is not positive, a negative value with a consumption is
//   `V − W·c`, a goal term larger than `|V|` makes it positive where goal terms count, a small one
//   leaves it in the band (test 2);
// - beside a priced product the base value is `V` alone, and a goal on the product adds its gap
//   term once, the product's base valuation nowhere; a pending product leaves `B` known (test 3);
// - the capped order lists a consumer of no value without the pool's cost nowhere, and a negative
//   value lifted by its goal term by its value (test 4);
// - the goal's runs bound `run_cap` beside a priced product, and the run told enough turns leaves
//   the goal's pool unread (test 5);
// - the case the slice is for: a chain head with a fixed value, a priced consumption and a priced
//   product a goal asks for (test 6);
// - the composer requires `fixed_value_any` for either freedom, and a Runtime that does not name
//   it refuses the file (test 7).
//
// The catalog is neutral and V2, as #455 B1's gate: judged tasks on one instance; the capped pool
// `pool.neutral` read from `resource.neutral`, its capacity read from `resource.neutral.capacity`
// (180), valued 1000 milli a unit (Q = 1), no regeneration, its stop line the capacity less 40
// (140) and its hard line the capacity less 76 (104); `pool.lacking` without a valuation;
// `pool.priced` valued 5000 milli a step of 10 units (Q = 10); `pool.later` whose valuation is the
// pending marker. A goal is a v3 record, at least 40, its own scale 10 and importance 1000: read at
// 10, the gap is 30 and `Γ = ⌊1000·30/10⌋ = 3000`; a production of 2 a run adds
// `2000·3000/(10·1000) = 600`, of 4 a run 1200 and of 6 a run 1800. Names are made up; no clock is
// read.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    Initiator, RESOURCE_TARGETS_FACT_KEY, RuntimeChannel, StandardPackProblem,
    StandardPackRootBytes, read_standard_pack_root,
};
use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::evaluator::produced_unread;
use crate::resource_targets::SpendValues;
use crate::{
    BENEFIT_FIXED_VALUE_SUBSYSTEM, CandidateEligibility, CappedMemory, CatalogDocumentSource,
    CatalogSources, ChainHandoverInputs, CompiledCatalog, DecisionReason, DispatchIntent,
    EvaluationFacts, EvaluationResources, EvaluationTime, FIXED_VALUE_ANY_SUBSYSTEM,
    FactRefreshInputs, FactRefreshSettings, FactValue, GivenPoolReadings, HandedRuns,
    HostResourceSnapshot, InstanceHolds, NoProgressGuards, ObservedFact, OwedReruns, PolicyCode,
    PolicyEvaluation, PolicyEvaluationError, RemedyPlan, RunSlots, SCHEDULING_SCHEMA_VERSION_V2,
    ScopeSelector, StandardPackBinding, StandardPackComposeInput, StandardPackComposition,
    StandardPackGameFiles, TargetSetter, TaskDecision, check_resource_targets, compile_catalog,
    compose_standard_pack_rules, evaluate_with_capped_spending, parse_resource_targets,
    project_fact_pools,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// How long a run lasts: it ends half a minute after the round that dispatched it.
const RUN_MS: u64 = 30_000;
/// The capped pool, read from `READING_KEY`.
const POOL: &str = "pool.neutral";
const READING_KEY: &str = "resource.neutral";
/// The capacity fact of the capped pool, in its scope.
const CAPACITY_KEY: &str = "resource.neutral.capacity";
/// A pool without a valuation, read from `LACKING_KEY`.
const LACKING: &str = "pool.lacking";
const LACKING_KEY: &str = "resource.lacking";
/// A priced pool (5000 milli a step of 10), read from `PRICED_KEY`.
const PRICED: &str = "pool.priced";
const PRICED_KEY: &str = "resource.priced";
/// A pool whose valuation is the pending marker.
const LATER: &str = "pool.later";
/// Ruling 11 of #455 B1: how the detail of a reason judged without the goal terms ends.
const UNCOUNTED: &str = " goal_terms=not_counted_below_trigger_line";
/// Every goal: at least `AT_LEAST`, its own scale and importance.
const AT_LEAST: i64 = 40;
const GOAL_SCALE: u64 = 10;
const IMPORTANCE_MILLI: u64 = 1000;
/// The declared duration of every task, `E` without a snapshot.
const E: u64 = 60_000;
/// The largest and the least value, ±(2^53 − 1).
const MAX: i64 = 9_007_199_254_740_991;

// Test 1, the compiler (the work order's study, section 5 item 1, with sched_c1a_t6 adapted in
// place). Tasks of the values 0, −1 and −(2^53 − 1) compile, each held as written; the canonical
// bytes of the tasks write them as the integers they are, and the same documents compile to the
// same catalog hash, which a value of another sign changes.
#[test]
fn fixv_t1_a_value_of_any_sign_compiles_and_keeps_its_canonical_bytes() {
    let tasks = || {
        vec![
            fixed(judged("task_zero", 0), 0),
            fixed(judged("task_minus", 0), -1),
            fixed(judged("task_least", 0), -MAX),
        ]
    };
    let compiled = catalog(tasks(), pools());
    let values = compiled
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| task.benefit_fixed_value_milli)
        .collect::<Vec<_>>();
    assert_eq!(values, [Some(0), Some(-1), Some(-MAX)]);
    let bytes = crate::canonical::canonical_serialized(&compiled.catalog().tasks.tasks)
        .expect("the canonical bytes");
    let text = String::from_utf8(bytes).expect("UTF-8");
    for written in [
        "\"benefit_fixed_value_milli\":0,",
        "\"benefit_fixed_value_milli\":-1,",
        "\"benefit_fixed_value_milli\":-9007199254740991,",
    ] {
        assert!(text.contains(written), "{written}: {text}");
    }
    let again = catalog(tasks(), pools());
    assert_eq!(compiled.catalog_hash(), again.catalog_hash());
    let other = catalog(
        vec![
            fixed(judged("task_zero", 0), 0),
            fixed(judged("task_minus", 0), 1),
            fixed(judged("task_least", 0), -MAX),
        ],
        pools(),
    );
    assert_ne!(compiled.catalog_hash(), other.catalog_hash());
}

// Test 2, the gate (study item 2).
// - `V` 0 with no effect: `B = 0` exactly, `policy_benefit_nonpositive` with `benefit_milli` 0,
//   after `benefit_fixed_value` with `fixed_value_milli` 0.
// - `V` −1 000 consuming 30 of the capped pool read at 150: `B = −1 000 − 30 000 = −31 000`, not
//   dispatched.
// - `V` −500 consuming 1 of the capped pool (it does not regenerate) and producing 6 of the lacking
//   pool: `B = −1 500` without the goal, `−1 500 + 1 800 = 300` with it, dispatched.
// - The capped pool regenerating with its trigger line at 160: read at 150 the same task waits
//   `policy_benefit_nonpositive` −1 500, its detail saying the goal terms are not counted; with the
//   lacking pool writing `goal_tiers` its term counts below the line too and it is dispatched with
//   `B` 300; read at 160 it heads the capped episode, `B = −500 + 1 800 − 1000·(1 − 1) = 1 300`.
// - `V` −600 producing 2 of the lacking pool: with the goal `S = 0` within `ε = 1/10`,
//   `benefit_within_tolerance` `unmarked`.
#[test]
fn fixv_t2_the_gate_reads_a_value_of_any_sign() {
    let mut zero = World::new(
        catalog(vec![fixed(judged("task_zero", 0), 0)], pools()),
        150,
    );
    let evaluation = zero.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, "task_zero");
    let nonpositive = reason(held, PolicyCode::PolicyBenefitNonpositive);
    assert_eq!(nonpositive.outcome.value("benefit_milli"), Some(&json!(0)));
    fixed_first(held, 0, 0, E);

    let mut costly = World::new(
        catalog(vec![fixed(judged("task_cost", 30), -1_000)], pools()),
        150,
    );
    let evaluation = costly.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, "task_cost");
    let nonpositive = reason(held, PolicyCode::PolicyBenefitNonpositive);
    assert_eq!(
        nonpositive.outcome.value("benefit_milli"),
        Some(&json!(-31_000))
    );
    fixed_first(held, -31_000, -1_000, E);

    let lifted = |pool_list: Value| {
        catalog(
            vec![fixed(
                with(
                    judged("task_lift", 1),
                    "produces",
                    json!([produce(LACKING, 6)]),
                ),
                -500,
            )],
            pool_list,
        )
    };
    let mut plain = World::new(lifted(pools()), 150).with_lacking(10);
    let evaluation = plain.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    fixed_first(decision(&evaluation, "task_lift"), -1_500, -500, E);
    let mut goal = World::new(lifted(pools()), 150)
        .with_lacking(10)
        .with_goals(&[LACKING]);
    let evaluation = goal.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_lift"], "{evaluation:#?}");
    fixed_first(decision(&evaluation, "task_lift"), 300, -500, E);

    let mut below = World::new(lifted(triggered(pools())), 150)
        .with_lacking(10)
        .with_goals(&[LACKING]);
    let evaluation = below.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let waits = decision(&evaluation, "task_lift");
    let held = last(waits);
    assert!(
        held.outcome.is(PolicyCode::PolicyBenefitNonpositive),
        "{waits:#?}"
    );
    assert_eq!(held.outcome.value("benefit_milli"), Some(&json!(-1_500)));
    assert!(held.detail.ends_with(UNCOUNTED), "{}", held.detail);
    fixed_first(waits, -1_500, -500, E);
    let mut tiered = World::new(lifted(with_tiers(triggered(pools()))), 150)
        .with_lacking(10)
        .with_goals(&[LACKING]);
    let evaluation = tiered.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_lift"], "{evaluation:#?}");
    fixed_first(decision(&evaluation, "task_lift"), 300, -500, E);
    let mut at_line = World::new(lifted(triggered(pools())), 160)
        .with_lacking(10)
        .with_goals(&[LACKING]);
    let evaluation = at_line.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_lift"], "{evaluation:#?}");
    assert_eq!(
        spend(&evaluation, "task_lift").value("capped_rank"),
        Some(&json!(1))
    );
    fixed_first(decision(&evaluation, "task_lift"), 1_300, -500, E);

    let mut band = World::new(
        catalog(
            vec![fixed(
                with(
                    judged("task_band", 0),
                    "produces",
                    json!([produce(LACKING, 2)]),
                ),
                -600,
            )],
            pools(),
        ),
        150,
    )
    .with_lacking(10)
    .with_goals(&[LACKING]);
    let evaluation = band.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let within = decision(&evaluation, "task_band");
    let tolerance = reason(within, PolicyCode::BenefitWithinTolerance);
    assert_eq!(tolerance.outcome.value("benefit_milli"), Some(&json!(0)));
    assert_eq!(tolerance.outcome.value("tolerance_milli"), Some(&json!(1)));
    assert_eq!(
        tolerance.outcome.value("zero_tier"),
        Some(&json!("unmarked"))
    );
    assert!(
        tolerance.detail.ends_with(" items=pool.lacking:+600"),
        "{}",
        tolerance.detail
    );
    fixed_first(within, 0, -600, E);
}

// Test 3, beside a priced product (study item 2, its last point). `task_p` consumes 30 of the
// capped pool (read at 150) and produces 2 of the priced pool (read at 10): with `V` 29 000 and no
// goal `B = 29 000 − 30 000 = −1 000`, its items the consumption's alone (the product's base
// valuation, 1 000 a run, is not added); with a goal on the priced pool `B = −1 000 + 600 = −400`,
// the gap term once, at the pool's own step 10, and its item `pool.priced:+600` (`W = B + Γ`
// would give 1 600). With `V` 40 000 it is dispatched with `B` 10 000 without the goal and
// 10 600 with it. A production of the pool whose valuation is pending leaves `B` known: 10 000.
#[test]
fn fixv_t3_beside_a_priced_product_the_base_value_is_v_alone() {
    let stage = |value: i64, produced: &str| {
        catalog(
            vec![fixed(
                with(
                    judged("task_p", 30),
                    "produces",
                    json!([produce(produced, 2)]),
                ),
                value,
            )],
            pools(),
        )
    };
    let mut plain = World::new(stage(29_000, PRICED), 150).with_priced(10);
    let evaluation = plain.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, "task_p");
    assert_eq!(
        last(held).detail,
        "benefit_milli=-1000 expected_duration_ms=60000 items=pool.neutral:-30000"
    );
    fixed_first(held, -1_000, 29_000, E);
    let mut goal = World::new(stage(29_000, PRICED), 150)
        .with_priced(10)
        .with_goals(&[PRICED]);
    let evaluation = goal.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, "task_p");
    assert_eq!(
        last(held).detail,
        "benefit_milli=-400 expected_duration_ms=60000 items=pool.priced:+600,pool.neutral:-30000"
    );
    fixed_first(held, -400, 29_000, E);

    for (case, goals, benefit) in [
        ("no goal", Vec::new(), 10_000),
        ("a goal", vec![PRICED], 10_600),
    ] {
        let mut world = World::new(stage(40_000, PRICED), 150)
            .with_priced(10)
            .with_goals(&goals);
        let evaluation = world.evaluate();
        assert_eq!(
            dispatched(&evaluation),
            ["task_p"],
            "{case}: {evaluation:#?}"
        );
        let winner = decision(&evaluation, "task_p");
        fixed_first(winner, benefit, 40_000, E);
        assert_eq!(
            reason(winner, PolicyCode::RankBreakdown)
                .outcome
                .value("benefit_milli"),
            Some(&json!(benefit)),
            "{case}"
        );
    }

    let mut pending = World::new(stage(40_000, LATER), 150);
    let evaluation = pending.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_p"], "{evaluation:#?}");
    fixed_first(decision(&evaluation, "task_p"), 10_000, 40_000, E);
}

// Test 4, the capped order (study item 3). Read at 200 (above the capacity 180), three consumers of
// 30 a run: `task_a` of `V` −1 000 and `task_c` of `V` 0, whose values without the pool's cost are
// −1 000 and 0, are not listed and are judged by their whole `B` (−31 000 and −30 000,
// `policy_benefit_nonpositive`); `task_b` of `V` −1 000 producing 4 of the lacking pool, its goal
// term 1 200, is listed by its value 200 and spent as the head (rank 1), its `B` −1 000 + 1 200 −
// 1000·(30 − 30) = 200.
#[test]
fn fixv_t4_the_capped_order_lists_a_consumer_by_its_value_of_any_sign() {
    let compiled = catalog(
        vec![
            fixed(judged("task_a", 30), -1_000),
            fixed(
                with(
                    judged("task_b", 30),
                    "produces",
                    json!([produce(LACKING, 4)]),
                ),
                -1_000,
            ),
            fixed(judged("task_c", 30), 0),
        ],
        pools(),
    );
    let mut world = World::new(compiled, 200)
        .with_lacking(10)
        .with_goals(&[LACKING]);
    let evaluation = world.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_b"], "{evaluation:#?}");
    let head = spend(&evaluation, "task_b");
    assert_eq!(head.value("capped_rank"), Some(&json!(1)));
    assert_eq!(head.value("capped_round"), Some(&json!(1)));
    fixed_first(decision(&evaluation, "task_b"), 200, -1_000, E);
    for (task_id, value, benefit) in [("task_a", -1_000, -31_000), ("task_c", 0, -30_000)] {
        let entry = decision(&evaluation, task_id);
        assert!(
            !has(entry, PolicyCode::CappedSpend),
            "{task_id}: {entry:#?}"
        );
        assert_eq!(
            reason(entry, PolicyCode::PolicyBenefitNonpositive)
                .outcome
                .value("benefit_milli"),
            Some(&json!(benefit)),
            "{task_id}"
        );
        fixed_first(entry, benefit, value, E);
    }
}

// Test 5, the turns (study item 4, the coordinator's ruling 2). `task_t` of `V` 40 000 consumes 2
// of the capped pool (read at 150) and produces 2 of the priced pool and 2 of the lacking pool, a
// goal on the lacking one (gap 30). Its run is told `run_cap` min(⌊(150 − 104) ÷ 2⌋ = 23,
// ⌈30 ÷ 2⌉ = 15) = 15 and the gap 30 at its production entry 1: the priced product, which carries a
// base weight, no longer removes the goal's bound. That run, told enough turns to close the gap,
// leaves the lacking pool unread until a reading observed at or after its end (#455 RR3c); the
// priced pool, which no goal asks for, is read.
#[test]
fn fixv_t5_the_goal_s_runs_bound_the_turns_beside_a_priced_product() {
    let compiled = catalog(
        vec![fixed(
            with(
                judged("task_t", 2),
                "produces",
                json!([produce(PRICED, 2), produce(LACKING, 2)]),
            ),
            40_000,
        )],
        pools(),
    );
    let mut world = World::new(compiled, 150)
        .with_lacking(10)
        .with_priced(10)
        .with_goals(&[LACKING]);
    let evaluation = world.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_t"], "{evaluation:#?}");
    fixed_first(decision(&evaluation, "task_t"), 38_600, 40_000, E);
    spend_values_are(&evaluation, "task_t", &[(1, 30)], 15);

    let handed = world.handed(&evaluation, "task_t");
    let facts = world.facts();
    let bundle = world.catalog.catalog();
    let unread = produced_unread(
        &bundle.pools.pools,
        &facts,
        &GivenPoolReadings::default(),
        &bundle.tasks.tasks,
        &handed,
    );
    assert!(unread.contains(LACKING), "{unread:?}");
    assert!(!unread.contains(PRICED), "{unread:?}");
}

// Test 6, the case the slice is for (the work order's test 1). A chain head `task_head` of `V`
// 10 000 consumes 2 of the capped pool (a priced consumption, 2 000) and produces 2 of the priced
// pool; its follower `task_next` requires its success and declares nothing. Without a goal the head
// is `B = 10 000 − 2 000 = 8 000` exactly and dispatched, its `rank_breakdown` naming `B` and `V`;
// with a goal on the priced pool (gap 30) it is `8 000 + 600 = 8 600`: the gap term once, the
// pool's base valuation (1 000 a run) nowhere, and its run is told the goal's turns,
// min(⌊(150 − 104) ÷ 2⌋ = 23, ⌈30 ÷ 2⌉ = 15) = 15, and the gap 30 at its production entry 0. The
// follower is not due before its head's run.
#[test]
fn fixv_t6_a_chain_head_with_its_true_products_reaches_its_goal() {
    let chain = || {
        catalog(
            vec![
                fixed(
                    with(
                        judged("task_head", 2),
                        "produces",
                        json!([produce(PRICED, 2)]),
                    ),
                    10_000,
                ),
                with(judged("task_next", 0), "trigger", after("task_head")),
            ],
            pools(),
        )
    };
    let mut plain = World::new(chain(), 150).with_priced(10);
    let evaluation = plain.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_head"], "{evaluation:#?}");
    let head = decision(&evaluation, "task_head");
    fixed_first(head, 8_000, 10_000, E);
    let breakdown = reason(head, PolicyCode::RankBreakdown);
    assert!(
        breakdown
            .detail
            .ends_with(" benefit_milli=8000 expected_duration_ms=60000 fixed_value_milli=10000"),
        "{}",
        breakdown.detail
    );

    let mut goal = World::new(chain(), 150)
        .with_priced(10)
        .with_goals(&[PRICED]);
    let evaluation = goal.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_head"], "{evaluation:#?}");
    let head = decision(&evaluation, "task_head");
    fixed_first(head, 8_600, 10_000, E);
    assert!(
        reason(head, PolicyCode::RankBreakdown)
            .detail
            .ends_with(" benefit_milli=8600 expected_duration_ms=60000 fixed_value_milli=10000"),
        "{head:#?}"
    );
    spend_values_are(&evaluation, "task_head", &[(0, 30)], 15);
}

// Test 7, the composer and an older Runtime (study item 5; the work order's test 2, rollback's
// guard). With `requires` naming `benefit_fixed_value` alone, a rule whose fixed value is 0, one
// whose value is negative, one beside a production of a priced pool and one beside a production of
// a pool whose valuation is pending are each refused `schedule_invalid` at the first such rule's
// `fixed_value_milli`; a rule of a positive value beside no priced product does not need the word.
// With `fixed_value_any` named too, the file composes and each task holds its value as written. A
// Runtime whose subsystems do not name the word (as none before 0.12.101 does) refuses that file at
// its entry, `schedule_requires_missing`.
#[test]
fn fixv_t7_the_composer_requires_fixed_value_any_and_an_older_runtime_refuses_the_file() {
    let older = [BENEFIT_FIXED_VALUE_SUBSYSTEM];
    let both = [BENEFIT_FIXED_VALUE_SUBSYSTEM, FIXED_VALUE_ANY_SUBSYSTEM];
    let plain = || schedule_rule("rule.plain", 12_000, None);
    for (case, freedom) in [
        ("a value of 0", schedule_rule("rule.free", 0, None)),
        ("a negative value", schedule_rule("rule.free", -1_500, None)),
        (
            "beside a priced product",
            schedule_rule("rule.free", 12_000, Some(PRICED)),
        ),
        (
            "beside a pending product",
            schedule_rule("rule.free", 12_000, Some(LATER)),
        ),
    ] {
        let file = schedule(vec![plain(), freedom], &older);
        let failure = compose(&file, None).expect_err(case);
        let issue = &failure.issues()[0].issue;
        assert_eq!(
            issue.problem,
            StandardPackProblem::ScheduleInvalid,
            "{case}"
        );
        assert_eq!(issue.pointer, "/rules/1/fixed_value_milli", "{case}");
    }
    compose(&schedule(vec![plain()], &older), None)
        .unwrap_or_else(|failure| panic!("a file of neither freedom composes: {failure}"));

    let free = || {
        vec![
            plain(),
            schedule_rule("rule.zero", 0, None),
            schedule_rule("rule.minus", -1_500, Some(PRICED)),
            schedule_rule("rule.later", 12_000, Some(LATER)),
        ]
    };
    let composed = compose(&schedule(free(), &both), None)
        .unwrap_or_else(|failure| panic!("the file composes: {failure}"));
    let values = composed
        .compiled()
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| (task.id.clone(), task.benefit_fixed_value_milli))
        .collect::<Vec<_>>();
    assert_eq!(
        values,
        [
            ("rule.plain".to_owned(), Some(12_000)),
            ("rule.zero".to_owned(), Some(0)),
            ("rule.minus".to_owned(), Some(-1_500)),
            ("rule.later".to_owned(), Some(12_000)),
        ]
    );

    let subsystems = BTreeSet::from([
        "standard_pack_schedule".to_owned(),
        BENEFIT_FIXED_VALUE_SUBSYSTEM.to_owned(),
    ]);
    let failure = compose(&schedule(free(), &both), Some(&subsystems))
        .expect_err("a Runtime that does not name the subsystem");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleRequiresMissing);
    assert_eq!(issue.pointer, "/requires/1");
}

// ---- The world a test drives ----

/// The catalog, the Runtime's memory of the capped episodes, the pools' readings and the
/// instance's goal record.
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    /// The capped pool's reading.
    reading: i64,
    /// The lacking pool's reading; `None` while it is unknown.
    lacking: Option<i64>,
    /// The priced pool's reading; `None` while it is unknown.
    priced: Option<i64>,
    /// The instance's `session.resource_targets` record; `None` without a goal.
    goal: Option<ObservedFact>,
}

impl World {
    /// The world at NOW without a goal, the capped pool read at `reading` a minute before.
    fn new(compiled: CompiledCatalog, reading: i64) -> Self {
        Self {
            catalog: compiled,
            memory: CappedMemory::default(),
            reading,
            lacking: None,
            priced: None,
            goal: None,
        }
    }

    /// The same world with the lacking pool read at `reading` a minute before.
    fn with_lacking(mut self, reading: i64) -> Self {
        self.lacking = Some(reading);
        self
    }

    /// The same world with the priced pool read at `reading` a minute before.
    fn with_priced(mut self, reading: i64) -> Self {
        self.priced = Some(reading);
        self
    }

    /// The same world with a goal on each of `targeted`, set an hour before; none without one.
    fn with_goals(mut self, targeted: &[&str]) -> Self {
        if !targeted.is_empty() {
            self.goal = Some(goal_record(&self.catalog, targeted));
        }
        self
    }

    fn facts(&self) -> EvaluationFacts {
        let mut observed = vec![
            fact(A, CAPACITY_KEY, 180, NOW - HOUR_MS, None),
            fact(A, READING_KEY, self.reading, NOW - MINUTE_MS, None),
        ];
        for (key, reading) in [(LACKING_KEY, self.lacking), (PRICED_KEY, self.priced)] {
            if let Some(reading) = reading {
                observed.push(fact(A, key, reading, NOW - MINUTE_MS, None));
            }
        }
        observed.extend(self.goal.clone());
        EvaluationFacts {
            ledger_position: 7,
            fact_snapshot_id: "snapshot:fixture-503-fixv".to_owned(),
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

    /// The runs the host notes once the round's dispatch of `task_id` ran: it ends half a minute
    /// after the round, noted from its recorded reasons as the host notes it.
    fn handed(&self, evaluation: &PolicyEvaluation, task_id: &str) -> HandedRuns {
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == task_id && intent.instance_id == A)
            .unwrap_or_else(|| panic!("a dispatch of {task_id}: {evaluation:#?}"));
        let chain = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .unwrap_or_else(|| panic!("the dispatch's reason chain: {evaluation:#?}"));
        let mut handed = HandedRuns::default();
        handed.note_dispatch(
            &intent.task_id,
            &intent.instance_id,
            Some(NOW + RUN_MS),
            chain.reasons.iter().map(|given| &given.outcome),
        );
        handed
    }
}

/// The v3 goal of a target on each of `targeted` (at least `AT_LEAST`, scale `GOAL_SCALE`,
/// importance `IMPORTANCE_MILLI`, no tasks named, no lifetime), checked against `compiled` and
/// stored as the formal entry stores it for a Lab connection, an hour before NOW.
fn goal_record(compiled: &CompiledCatalog, targeted: &[&str]) -> ObservedFact {
    let targets = targeted
        .iter()
        .map(|pool| {
            json!({
                "id": format!("goal-{}", &pool["pool.".len()..]),
                "resource": pool,
                "condition": {"kind": "at_least", "amount": AT_LEAST},
                "scale": GOAL_SCALE,
                "importance_milli": IMPORTANCE_MILLI,
                "apply": {"mode": "adjust", "weight": "score_stage"}
            })
        })
        .collect::<Vec<_>>();
    let document = json!({
        "schema_version": "actingcommand.resource-targets.v3",
        "instance": A,
        "targets": targets
    });
    let parsed = parse_resource_targets(&serde_json::to_vec(&document).expect("document bytes"))
        .unwrap_or_else(|error| panic!("the goal parses: {error:?}"));
    let set_at = NOW - HOUR_MS;
    let checked = check_resource_targets(
        &parsed,
        compiled,
        &EvaluationFacts {
            ledger_position: 7,
            fact_snapshot_id: "snapshot:fixture-503-fixv".to_owned(),
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

fn has(entry: &TaskDecision, code: PolicyCode) -> bool {
    entry.reasons.iter().any(|given| given.outcome.is(code))
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
        Some(&json!(value)),
        "{entry:#?}"
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

/// The `capped_spend` reason of `task_id`'s dispatch.
fn spend<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a Outcome {
    &reason(decision(evaluation, task_id), PolicyCode::CappedSpend).outcome
}

/// `task_id`'s dispatch is handed the gaps `gaps` (by production entry) and `run_cap`, as SS-6
/// builds them (`policy_dispatch_parameters`).
fn spend_values_are(
    evaluation: &PolicyEvaluation,
    task_id: &str,
    gaps: &[(usize, u64)],
    run_cap: u64,
) {
    let given = &reason(
        decision(evaluation, task_id),
        PolicyCode::PolicyDispatchParameters,
    )
    .outcome;
    let built = SpendValues {
        spend_allowances: Vec::new(),
        gaps_remaining: gaps.to_vec(),
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

/// Due while the capped pool holds `amount`.
fn enough(amount: u64) -> Value {
    json!({
        "kind": "resource_projection",
        "pool_id": POOL,
        "comparison": "greater_than_or_equal",
        "value": amount
    })
}

/// Due every hour (one minute past the hour at NOW).
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

/// The hourly clock and `task_id`'s success, both required: a chain link to `task_id`.
fn after(task_id: &str) -> Value {
    json!({
        "kind": "all",
        "predicates": [
            hourly(),
            {"kind": "dependency_completed", "task_id": task_id, "terminal_states": ["succeeded"]}
        ]
    })
}

/// A task of the game of priority 100 without a member of a schedule rule's: `consumes` and
/// `produces` as given, on `trigger`.
fn task_body(id: &str, trigger: Value, consumes: Vec<Value>, produces: Vec<Value>) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
        "priority": 100,
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
        "expected_duration_ms": E,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// A judged task consuming `consumed` units of the capped pool a run, due while the pool holds
/// them; with `consumed` 0 it consumes nothing and is due every hour.
fn judged(id: &str, consumed: u64) -> Value {
    let mut task = if consumed == 0 {
        task_body(id, hourly(), Vec::new(), Vec::new())
    } else {
        task_body(
            id,
            enough(consumed),
            vec![consume(POOL, consumed)],
            Vec::new(),
        )
    };
    task["benefit_judged"] = json!(true);
    task
}

/// `task` with the fixed value `value`.
fn fixed(task: Value, value: i64) -> Value {
    with(task, "benefit_fixed_value_milli", json!(value))
}

fn with(mut task: Value, name: &str, value: Value) -> Value {
    task[name] = value;
    task
}

/// A pool of the instance that does not regenerate, read from `resource.<the id without
/// "pool.">`, of constant `capacity`, weighed `weight` milli a step of `scale` units.
fn valued(id: &str, weight: u32, scale: u64, capacity: u64) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": capacity,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {"name": "Fixture", "unit": "unit", "scale": scale, "base_weight_milli": weight}
    })
}

/// The capacity fact plus `offset`.
fn line(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

/// The capped pool, its capacity read from its fact, its stop line 40 below the cap and its hard
/// line 76 below it; the lacking pool without a valuation; the priced pool; the pending one.
fn pools() -> Value {
    let mut capped = valued(POOL, 1000, 1, 1);
    capped["capacity"] = line(0);
    capped["stop_line"] = line(-40);
    capped["hard_line"] = line(-76);
    let mut lacking = valued(LACKING, 1000, 1, 1000);
    lacking
        .as_object_mut()
        .expect("a pool object")
        .shift_remove("valuation");
    let mut later = valued(LATER, 1000, 1, 1000);
    later["valuation"] = json!("pending");
    json!([capped, lacking, valued(PRICED, 5000, 10, 1000), later])
}

/// `pool_list` with its capped pool (the first) regenerating one unit in six minutes and its
/// trigger line 20 below its capacity (160).
fn triggered(mut pool_list: Value) -> Value {
    pool_list[0]["projection"] = json!({"amount": 1, "per_ms": 6 * MINUTE_MS});
    pool_list[0]["trigger_line"] = line(-20);
    pool_list
}

/// `pool_list` with its lacking pool (the second) writing `goal_tiers`, its stock's cap the
/// largest, so a goal's term on it is the one it has without the member.
fn with_tiers(mut pool_list: Value) -> Value {
    pool_list[1]["goal_tiers"] =
        json!({"need_importance_milli": 100_000, "stock_cap_milli": 1_000_000});
    pool_list
}

fn catalog(tasks: Vec<Value>, pool_list: Value) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.503-fixv",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-503-fixv"]
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

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-503-fixv/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

// ---- The standard-pack fixture of test 7 ----

/// A `schedule.json` rule running the package of its own id every hour, consuming 10 of the capped
/// pool, with the fixed value `value`, producing 1 of `produced` when given.
fn schedule_rule(id: &str, value: i64, produced: Option<&str>) -> Value {
    let produces = produced.map(|pool| produce(pool, 1)).into_iter().collect();
    let mut rule = task_body(id, hourly(), vec![consume(POOL, 10)], produces);
    let members = rule.as_object_mut().expect("a task object");
    for member in ["scope", "entrypoint", "procedure_ref", "instance_overrides"] {
        members.shift_remove(member);
    }
    members.insert("package_id".to_owned(), json!(id));
    members.insert("fixed_value_milli".to_owned(), json!(value));
    rule
}

/// A `schedule.json` pool: `id` of constant capacity 240, valued 1000 milli a unit, without a
/// scope.
fn schedule_pool(id: &str) -> Value {
    let mut pool = valued(id, 1000, 1, 240);
    pool.as_object_mut()
        .expect("a pool object")
        .shift_remove("scope");
    pool
}

/// `schedule.json` with these rules, the capped pool, the priced pool and the pending one, naming
/// `requires`.
fn schedule(rules: Vec<Value>, requires: &[&str]) -> Value {
    let mut later = schedule_pool(LATER);
    later["valuation"] = json!("pending");
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": rules,
        "pools": [schedule_pool(POOL), schedule_pool(PRICED), later],
        "activity": [],
        "timeline": [],
        "held": []
    })
}

/// Composes the `schedule.json` `file` for one instance of the game, whose registry holds a package
/// per rule and no maintenance package, with the Runtime's `subsystems` when given.
fn compose(
    file: &Value,
    subsystems: Option<&BTreeSet<String>>,
) -> Result<StandardPackComposition, crate::StandardPackComposeFailure> {
    let packages = file["rules"]
        .as_array()
        .expect("the rules")
        .iter()
        .map(|rule| rule["package_id"].as_str().expect("a package").to_owned())
        .collect::<BTreeSet<_>>();
    let bundle = json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": packages
            .iter()
            .map(|package_id| json!({
                "package_id": package_id,
                "server": SERVER,
                "entry_task_id": "task",
                "path": format!("packs/{package_id}"),
                "file_count": 1,
                "byte_count": 64
            }))
            .collect::<Vec<_>>()
    });
    let bundle = serde_json::to_vec(&bundle).expect("bundle bytes");
    let root = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle.as_slice()),
            ..StandardPackRootBytes::default()
        },
        GAME,
    )
    .unwrap_or_else(|issue| panic!("the root reads: {issue}"))
    .expect("the root holds bundle.json");
    let bytes = serde_json::to_vec(file).expect("schedule bytes");
    let irreversible = super::readable_root(&root);
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &bytes,
            root: &root,
            maintenance: &[],
            irreversible: &irreversible,
            catalog: None,
        },
    )]);
    let instances = BTreeMap::from([(
        A.to_owned(),
        StandardPackBinding {
            game_id: GAME.to_owned(),
            server_id: SERVER.to_owned(),
        },
    )]);
    compose_standard_pack_rules(&StandardPackComposeInput {
        instances: &instances,
        games: &games,
        subsystems,
        catalog_version: 1,
    })
}
