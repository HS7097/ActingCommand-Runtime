// SPDX-License-Identifier: AGPL-3.0-only

// SS-2d (register §1.235; Alice's ruling 127 of 10-31 13:19, the review's version; design
// `rd-1031/design-acm-price-addendum2.1.md` §7, tests 1 to 20 of §7.6 but 17, which is the
// runtime host's idle plan; contracts/scheduling/README.md, "Net benefit within a tier", "Within
// tolerance"):
// - a judged candidate whose exact net benefit `S` is within its rounding bound
//   `ε = Σ_produces [R/(2000·Q) + n·(2W+1)/(4000·Q)] + Σ_consumes σ·[C/(2000·Q) +
//   m·(2W+1)/(4000·Q)]` is `benefit_within_tolerance`, neither positive nor
//   `policy_benefit_nonpositive`;
// - a task marked `benefit_realization` (the model's verdict) whose produced pools include one
//   weighed above 0 and whose latest run did not fail is released into the last place of the
//   business tier, behind every positive candidate of every instance, every later gate applying;
//   every other candidate within tolerance is deferred (`unmarked`, `no_downstream_value`,
//   `latest_failed`);
// - outside the band the gate is SS-2b's, and a catalog with no candidate in the band decides as
//   before;
// - the compiler holds the mark to a judged task with a consumption and a production, a reserve
//   on and a fact reading of each consumed pool, and no cycle; the composer writes it from the
//   `schedule.json` rule member `realization`.
//
// The catalog is neutral and V2: tasks on the hourly clock due at NOW (its occurrence is at
// NOW − 1 minute; a task that ran after it is no candidate), pools of the instance read from their
// keys. Every quantity below is in milli of value unless it says otherwise; every expected value
// comes from the design's formula, worked out in the comment of its test.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    BenefitCoverage, CandidateEligibility, CatalogCompileFailure, CatalogDiagnosticCode,
    CatalogDocumentSource, CatalogSources, ChainHandoverInputs, CompiledCatalog, DecisionReason,
    EligibilityState, EvaluationFacts, EvaluationResources, EvaluationTime, FactRefreshInputs,
    FactRefreshSettings, FactValue, HostResourceSnapshot, InstanceHolds, InstanceSnapshot,
    NoProgressGuards, NoProgressMemory, ObservedFact, OwedReruns, PolicyCode, PolicyEvaluation,
    PolicyEvaluationError, RemedyPlan, RunSlots, SchedulingDecisionState, ScopeSelector,
    StandardPackBinding, StandardPackComposeInput, StandardPackComposition, StandardPackGameFiles,
    TaskDecision, TaskRuntimeSnapshot, TaskTerminalState, check_resource_targets, compile_catalog,
    compose_standard_pack_rules, evaluate_with_no_progress, parse_resource_targets,
    project_fact_pools,
};
use actingcommand_contract::{
    BundleIndex, SCHEDULE_RULE_REALIZATION_MEMBER, STANDARD_PACK_SCHEDULE_FILE,
    StandardPackProblem, StandardPackRootBytes, parse_standard_pack_schedule,
    read_standard_pack_root,
};

const A: &str = "instance-a";
const X: &str = "instance-x";
const Y: &str = "instance-y";
const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// Produced; its weight is each test's.
const OUT: &str = "pool.out";
/// Consumed: 333 milli a unit (scale 1).
const IN: &str = "pool.in";
/// Consumed: 333 milli per 1000 units (scale 1000).
const IN_FINE: &str = "pool.in_fine";
/// Consumed: 334 milli per 1000 units (scale 1000).
const IN_EXTRA: &str = "pool.in_extra";
const ZERO: &str = "task_zero";
const POSITIVE: &str = "task_positive";
const WITHIN: &str = "benefit_within_tolerance";
const NONPOSITIVE: &str = "policy_benefit_nonpositive";
/// The run before the hourly occurrence: the clock is due again at NOW.
const EARLIER: u64 = NOW - 10 * MINUTE_MS;

// Test 1: produce one unit at W 1000, consume three at W 333: S = 1000 − 999 = +1 milli, ε =
// 1000/2000 + 3000/2000 = 2 (the design's EX1). Not dispatched: `benefit_within_tolerance` with
// `zero_tier: unmarked`. Before SS-2d the same candidate was dispatched with B = ⌊1⌋ = 1 (main
// f64b3a12, `crates/policy/src/evaluator/benefit.rs:447`, `if benefit_milli > 0`).
#[test]
fn ss2d_t1_the_zero_band_holds_an_unmarked_candidate() {
    let catalog = catalog(
        vec![task(ZERO, vec![produce(OUT, 1)], vec![consume(IN, 3)])],
        pools(1000),
    );
    let evaluation = round(&catalog, Vec::new(), readings());
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, A, ZERO);
    assert_eq!(held.state, SchedulingDecisionState::Deferred);
    assert_eq!(held.eligibility, EligibilityState::True);
    assert_eq!(last_code(held), WITHIN, "{held:#?}");
    assert_tolerance(last(held), 1, 2, "unmarked");
    assert!(
        last(held)
            .detail
            .contains(" sum=1/1 tolerance=2/1 items=pool.out:+1000,pool.in:-999"),
        "{}",
        last(held).detail
    );
}

// Test 2: outside the band, as before. S = 1002 − 999 = 3 > ε = 2 is dispatched with B = 3; S =
// 996 − 999 = −3 < −ε is `policy_benefit_nonpositive`. Neither is within tolerance.
#[test]
fn ss2d_t2_outside_the_band_the_gate_is_unchanged() {
    let tasks = || vec![task(ZERO, vec![produce(OUT, 1)], vec![consume(IN, 3)])];
    let positive = round(&catalog(tasks(), pools(1002)), Vec::new(), readings());
    assert_eq!(dispatched(&positive), [ZERO], "{positive:#?}");
    let breakdown = reason(decision(&positive, A, ZERO), "rank_breakdown");
    assert_eq!(breakdown.outcome.value("benefit_milli"), Some(&json!(3)));

    let negative = round(&catalog(tasks(), pools(996)), Vec::new(), readings());
    assert!(dispatched(&negative).is_empty(), "{negative:#?}");
    let held = decision(&negative, A, ZERO);
    assert_eq!(last_code(held), NONPOSITIVE, "{held:#?}");
    assert_eq!(last(held).outcome.value("benefit_milli"), Some(&json!(-3)));
    for evaluation in [&positive, &negative] {
        assert!(no_tolerance(evaluation), "{evaluation:#?}");
    }
}

// Test 3: released. Marked, S = 998 − 999 = −1, ε = 2; it consumes a pool read at 100 above its
// reserve line 10, produces a pool weighed 998, its latest run (before the occurrence) succeeded,
// and nothing else is a candidate: dispatched, its reasons carry `zero_tier: released`.
#[test]
fn ss2d_t3_a_marked_candidate_within_tolerance_is_released() {
    let catalog = catalog(vec![zero_task()], pools(998));
    let evaluation = round(
        &catalog,
        vec![state(A, ZERO, EARLIER, TaskTerminalState::Succeeded)],
        readings(),
    );
    assert_eq!(dispatched(&evaluation), [ZERO], "{evaluation:#?}");
    let selected = decision(&evaluation, A, ZERO);
    assert_eq!(selected.state, SchedulingDecisionState::Selected);
    assert_tolerance(reason(selected, WITHIN), -1, 2, "released");
}

// Test 4: the band's edge, ε = 2. S = 997 − 999 = −2 is within it and released. Three pools give
// S = 997 − (2999·333 + 1·334)/1000 = −2.001 with ε = 1/2 + 2999/2000 + 1/2000 = 2 exactly
// (each pool's term rounded up alone would give 4 and hold it): outside, not positive, B = −3.
#[test]
fn ss2d_t4_the_band_is_closed_at_its_bound() {
    let edge = round(
        &catalog(vec![zero_task()], pools(997)),
        Vec::new(),
        readings(),
    );
    assert_eq!(dispatched(&edge), [ZERO], "{edge:#?}");
    assert_tolerance(reason(decision(&edge, A, ZERO), WITHIN), -2, 2, "released");

    let beyond = catalog(vec![three_pool_task()], pools(997));
    let evaluation = round(&beyond, Vec::new(), readings());
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, A, ZERO);
    assert_eq!(last_code(held), NONPOSITIVE, "{held:#?}");
    assert_eq!(last(held).outcome.value("benefit_milli"), Some(&json!(-3)));
    assert!(no_tolerance(&evaluation), "{evaluation:#?}");
}

// Test 5: positive first. The instance also has a positive candidate (two units at 998, S =
// 1996): it is selected; the released candidate waits behind it, `instance_already_selected`.
#[test]
fn ss2d_t5_a_positive_candidate_goes_before_the_zero_tier() {
    let catalog = catalog(
        vec![
            zero_task(),
            task(POSITIVE, vec![produce(OUT, 2)], Vec::new()),
        ],
        pools(998),
    );
    let evaluation = round(&catalog, Vec::new(), readings());
    assert_eq!(dispatched(&evaluation), [POSITIVE], "{evaluation:#?}");
    let waiting = decision(&evaluation, A, ZERO);
    assert_tolerance(reason(waiting, WITHIN), -1, 2, "released");
    assert_eq!(
        last_code(waiting),
        "instance_already_selected",
        "{waiting:#?}"
    );
}

// Test 6: the shared run slot. `max_concurrent_runs` 1. Instance X has only a released candidate,
// declared first (its tie-break comes first) and, were it positive, 10 times Y's benefit per
// millisecond (S = +1 in 60 000 ms against 3000 in 600 000 ms); Y has a positive one. Y takes the
// slot; X waits, `policy_concurrent_runs_deferred`.
#[test]
fn ss2d_t6_the_zero_tier_takes_no_shared_run_slot_before_a_positive_candidate() {
    let zero_x = on(
        marked(
            task(
                "task_zero_x",
                vec![produce("pool.out_x", 1)],
                vec![consume("pool.in_x", 3)],
            ),
            &[("pool.in_x", 10)],
        ),
        X,
    );
    let positive_y = on(
        with(
            task(
                "task_positive_y",
                vec![produce("pool.out_y", 3)],
                Vec::new(),
            ),
            "expected_duration_ms",
            json!(600_000),
        ),
        Y,
    );
    let catalog = catalog(
        vec![zero_x, positive_y],
        vec![
            valued_on(X, "pool.out_x", 1000, 1),
            valued_on(X, "pool.in_x", 333, 1),
            valued_on(Y, "pool.out_y", 1000, 1),
        ],
    );
    let facts = facts(
        vec![instance(X, &[]), instance(Y, &[])],
        Vec::new(),
        vec![fact(X, "resource.in_x", 100, NOW - MINUTE_MS, None)],
    );
    let slots = RunSlots {
        admitted_load_profiles: Vec::new(),
        max_concurrent_runs: Some(1),
    };
    let evaluation = evaluate(&catalog, &facts, &slots, &NoProgressGuards::default(), NOW);
    assert_eq!(
        dispatched(&evaluation),
        ["task_positive_y"],
        "{evaluation:#?}"
    );
    let waiting = decision(&evaluation, X, "task_zero_x");
    assert_tolerance(reason(waiting, WITHIN), 1, 2, "released");
    assert_eq!(
        last_code(waiting),
        "policy_concurrent_runs_deferred",
        "{waiting:#?}"
    );
}

// Test 7: the reserve line holds. Released, but the consumed pool reads 12: 12 − 3 = 9 is below
// its reserve 10, `resource_reserve_insufficient`.
#[test]
fn ss2d_t7_the_reserve_gate_still_holds_a_released_candidate() {
    let catalog = catalog(vec![zero_task()], pools(998));
    let evaluation = round(&catalog, Vec::new(), vec![reading(IN, 12)]);
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, A, ZERO);
    assert_tolerance(reason(held, WITHIN), -1, 2, "released");
    assert_eq!(
        last_code(held),
        "resource_reserve_insufficient",
        "{held:#?}"
    );
}

// Test 8: no downstream value. The only produced pool is weighed 0: S = 0 − 334/1000 = −0.334, ε
// = 1/2 + 1/2000 = 1001/2000; deferred, `zero_tier: no_downstream_value`.
#[test]
fn ss2d_t8_a_candidate_whose_produce_is_worth_nothing_is_not_released() {
    let mut pools = pools(998);
    pools.push(valued("pool.out_nil", 0, 1));
    let catalog = catalog(
        vec![marked(
            task(
                ZERO,
                vec![produce("pool.out_nil", 1)],
                vec![consume(IN_EXTRA, 1)],
            ),
            &[(IN_EXTRA, 0)],
        )],
        pools,
    );
    let evaluation = round(&catalog, Vec::new(), readings());
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, A, ZERO);
    assert_eq!(held.state, SchedulingDecisionState::Deferred);
    assert_eq!(last_code(held), WITHIN, "{held:#?}");
    assert_tolerance(last(held), -1, 1, "no_downstream_value");
    assert!(
        last(held)
            .detail
            .contains(" sum=-167/500 tolerance=1001/2000 "),
        "{}",
        last(held).detail
    );
}

// Test 9: no replay after a failure. Its latest run failed: deferred, `zero_tier: latest_failed`.
// The failure's own path is unchanged: the same task at S = 1002 − 999 = +3, outside the band, is
// dispatched after the same failure as before SS-2d.
#[test]
fn ss2d_t9_a_failed_latest_run_is_not_released() {
    let failed = vec![state(A, ZERO, EARLIER, TaskTerminalState::Failed)];
    let evaluation = round(
        &catalog(vec![zero_task()], pools(998)),
        failed.clone(),
        readings(),
    );
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, A, ZERO);
    assert_eq!(last_code(held), WITHIN, "{held:#?}");
    assert_tolerance(last(held), -1, 2, "latest_failed");

    let positive = round(&catalog(vec![zero_task()], pools(1002)), failed, readings());
    assert_eq!(dispatched(&positive), [ZERO], "{positive:#?}");
    assert!(no_tolerance(&positive), "{positive:#?}");
}

// Test 10: unknown stays unknown. Marked, but it produces a pool whose valuation is the pending
// marker: `benefit_unknown` (`valuation_pending`), never the zero tier.
#[test]
fn ss2d_t10_an_unknown_value_is_never_the_zero_tier() {
    let mut pools = pools(998);
    pools.push(pool(
        A,
        "pool.pending",
        json!({"kind": "none"}),
        Some(json!("pending")),
    ));
    let catalog = catalog(
        vec![marked(
            task(ZERO, vec![produce("pool.pending", 1)], vec![consume(IN, 3)]),
            &[(IN, 10)],
        )],
        pools,
    );
    let evaluation = round(&catalog, Vec::new(), readings());
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, A, ZERO);
    assert_eq!(last_code(held), "benefit_unknown", "{held:#?}");
    assert_eq!(
        last(held).outcome.value("benefit_gap"),
        Some(&json!("valuation_pending"))
    );
    assert!(no_tolerance(&evaluation), "{evaluation:#?}");
}

// Test 11: a goal's gap takes it out of the band. OUT has base 998 and a gap weight of 100 a
// step; a goal holds it at least 100. Read at 90, Γ = ⌊100·10/1⌋ = 1000, W = 1998: S = 999 > ε,
// dispatched with B = 999. Read at 100 the goal is met, Γ = 0, S = −1: back in the zero tier,
// released.
#[test]
fn ss2d_t11_a_goal_gap_lifts_the_candidate_out_of_the_band() {
    let mut pools = pools(998);
    pools[0]["valuation"]["gap"] = json!({"rule": "shortfall_linear", "weight_milli": 100});
    let catalog = catalog(vec![zero_task()], pools);
    let goal = stored_goal(&catalog);
    for (out, expected) in [(90, Some(999)), (100, None)] {
        let mut facts = readings();
        facts.push(reading(OUT, out));
        facts.push(goal.clone());
        let evaluation = round(&catalog, Vec::new(), facts);
        assert_eq!(dispatched(&evaluation), [ZERO], "{out}: {evaluation:#?}");
        let selected = decision(&evaluation, A, ZERO);
        match expected {
            Some(benefit) => {
                let breakdown = reason(selected, "rank_breakdown");
                assert_eq!(
                    breakdown.outcome.value("benefit_milli"),
                    Some(&json!(benefit)),
                    "{out}"
                );
                assert!(no_tolerance(&evaluation), "{out}: {evaluation:#?}");
            }
            None => assert_tolerance(reason(selected, WITHIN), -1, 2, "released"),
        }
    }
}

// Test 12: two complementary consumed pools, summed exactly. S = 999 − (2999·333 + 334)/1000 =
// −0.001 (each pool's cost rounded alone, 999 + 1, would give −1 exactly), ε = 1/2 + 2999/2000 +
// 1/2000 = 2 (the two consumed terms added as fractions, not rounded up one by one): released.
#[test]
fn ss2d_t12_two_consumed_pools_add_their_terms_exactly() {
    let catalog = catalog(vec![three_pool_task()], pools(999));
    let evaluation = round(&catalog, Vec::new(), readings());
    assert_eq!(dispatched(&evaluation), [ZERO], "{evaluation:#?}");
    let released = reason(decision(&evaluation, A, ZERO), WITHIN);
    assert_tolerance(released, -1, 2, "released");
    assert!(
        released.detail.contains(
            " sum=-1/1000 tolerance=2/1 items=pool.out:+999,pool.in_fine:-999,pool.in_extra:-1"
        ),
        "{}",
        released.detail
    );
}

// Test 13: the joint plan's example (#424 6516719468: material worth 10, acquired for 2, an
// upgrade worth 10). Acquiring is B = 10 − 2 = 8 > ε = 1 and runs first. Upgrading is S = 10 − 10
// = 0 within ε = 1: marked, it runs when its instance has nothing else (the acquisition ran after
// the occurrence), for 8 in all; unmarked it waits for ever, `zero_tier: unmarked`.
#[test]
fn ss2d_t13_a_marked_realization_runs_in_idle_time_and_an_unmarked_one_never() {
    let pools = vec![
        valued("pool.cost", 2, 1),
        valued("pool.material", 10, 1),
        valued("pool.upgrade", 10, 1),
    ];
    let acquire = task(
        "task_acquire",
        vec![produce("pool.material", 1)],
        vec![consume("pool.cost", 1)],
    );
    let upgrade = task(
        "task_upgrade",
        vec![produce("pool.upgrade", 1)],
        vec![consume("pool.material", 1)],
    );
    let readings = || vec![reading("pool.cost", 100), reading("pool.material", 5)];
    let marked_catalog = catalog(
        vec![
            acquire.clone(),
            marked(upgrade.clone(), &[("pool.material", 0)]),
        ],
        pools.clone(),
    );
    let first = round(&marked_catalog, Vec::new(), readings());
    assert_eq!(dispatched(&first), ["task_acquire"], "{first:#?}");
    let breakdown = reason(decision(&first, A, "task_acquire"), "rank_breakdown");
    assert_eq!(breakdown.outcome.value("benefit_milli"), Some(&json!(8)));
    let waiting = decision(&first, A, "task_upgrade");
    assert_tolerance(reason(waiting, WITHIN), 0, 1, "released");
    assert_eq!(last_code(waiting), "instance_already_selected");

    let acquired = vec![state(
        A,
        "task_acquire",
        NOW - 30_000,
        TaskTerminalState::Succeeded,
    )];
    let idle = round(&marked_catalog, acquired.clone(), readings());
    assert_eq!(dispatched(&idle), ["task_upgrade"], "{idle:#?}");
    assert_tolerance(
        reason(decision(&idle, A, "task_upgrade"), WITHIN),
        0,
        1,
        "released",
    );

    let unmarked_catalog = catalog(vec![acquire, upgrade], pools);
    let never = round(&unmarked_catalog, acquired, readings());
    assert!(dispatched(&never).is_empty(), "{never:#?}");
    let held = decision(&never, A, "task_upgrade");
    assert_eq!(last_code(held), WITHIN, "{held:#?}");
    assert_tolerance(last(held), 0, 1, "unmarked");
}

// Test 14: the compiler's checks, each refused at its member, and the composer's member. A
// consumed pool without a reserve (`missing_required_field` at `reserves`); a mark on a task that
// is not judged (at `benefit_judged`); no production or no consumption (at the empty list); a
// consumed pool read from an outcome, not a fact (`effect_incompatible` at the entry); two marked
// tasks that convert into each other, and one that produces what it consumes
// (`realization_cycle` at each one's `benefit_realization`); the mark in V1 (`unknown_field`). A
// chain that does not close compiles. The composer writes `benefit_realization: true` from the
// rule member `"realization": true` and counts it; refuses it on a maintenance-role rule, beside
// `"maintenance": true`, of another value, and the task member written by a rule; and maps a
// composed cycle back to the rule's `realization`.
#[test]
fn ss2d_t14_the_compiler_holds_the_mark_to_its_conditions() {
    let plain = || task(ZERO, vec![produce(OUT, 1)], vec![consume(IN, 3)]);
    let refusals = [
        (
            "no reserve",
            vec![with(plain(), "benefit_realization", json!(true))],
            CatalogDiagnosticCode::MissingRequiredField,
            vec!["/tasks/0/reserves"],
        ),
        (
            "not judged",
            vec![without(marked(plain(), &[(IN, 10)]), "benefit_judged")],
            CatalogDiagnosticCode::MissingRequiredField,
            vec!["/tasks/0/benefit_judged"],
        ),
        (
            "no production",
            vec![marked(
                task(ZERO, Vec::new(), vec![consume(IN, 3)]),
                &[(IN, 10)],
            )],
            CatalogDiagnosticCode::MissingRequiredField,
            vec!["/tasks/0/produces"],
        ),
        (
            "no consumption",
            vec![with(
                task(ZERO, vec![produce(OUT, 1)], Vec::new()),
                "benefit_realization",
                json!(true),
            )],
            CatalogDiagnosticCode::MissingRequiredField,
            vec!["/tasks/0/consumes"],
        ),
        (
            "a consumed pool read from an outcome",
            vec![marked(
                task(
                    ZERO,
                    vec![produce(OUT, 1)],
                    vec![consume("pool.counted", 1)],
                ),
                &[("pool.counted", 0)],
            )],
            CatalogDiagnosticCode::EffectIncompatible,
            vec!["/tasks/0/consumes/0"],
        ),
        (
            "two tasks converting into each other",
            vec![
                zero_task(),
                marked(
                    task("task_back", vec![produce(IN, 3)], vec![consume(OUT, 1)]),
                    &[(OUT, 0)],
                ),
            ],
            CatalogDiagnosticCode::RealizationCycle,
            vec![
                "/tasks/0/benefit_realization",
                "/tasks/1/benefit_realization",
            ],
        ),
        (
            "a task producing what it consumes",
            vec![marked(
                task(ZERO, vec![produce(IN, 1)], vec![consume(IN, 3)]),
                &[(IN, 10)],
            )],
            CatalogDiagnosticCode::RealizationCycle,
            vec!["/tasks/0/benefit_realization"],
        ),
    ];
    for (case, tasks, code, paths) in refusals {
        let mut pools = pools(998);
        pools.push(outcome_pool("pool.counted", ZERO));
        let failure = compile_v2(tasks, pools).expect_err(case);
        for path in paths {
            assert!(
                refused_at(&failure, code, path),
                "{case}: {path}: {:?}",
                failure.diagnostics()
            );
        }
    }

    // A chain that does not close compiles, and the member serializes as written.
    let chain = catalog(
        vec![
            zero_task(),
            marked(
                task(
                    "task_onward",
                    vec![produce(IN_EXTRA, 1)],
                    vec![consume(OUT, 1)],
                ),
                &[(OUT, 0)],
            ),
        ],
        pools(998),
    );
    let marks = chain
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| task.benefit_realization)
        .collect::<Vec<_>>();
    assert_eq!(marks, [Some(true), Some(true)]);

    // V1: the member is unknown.
    let failure = compile_v1_example(|tasks| {
        tasks["tasks"][0]["benefit_realization"] = json!(true);
    })
    .expect_err("V1 has no realization mark");
    assert!(
        refused_at(
            &failure,
            CatalogDiagnosticCode::UnknownField,
            "/tasks/0/benefit_realization"
        ),
        "{:?}",
        failure.diagnostics()
    );

    // The composer: the rule member becomes the task member on a judged rule, and is counted.
    let composition = compose(schedule(vec![
        realization_rule("rule.zero"),
        rule("rule.other", vec![produce(OUT, 1)], Vec::new()),
    ]))
    .unwrap_or_else(|failure| panic!("the fixture composes: {failure}"));
    let marks = composition
        .compiled()
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| {
            (
                task.id.clone(),
                task.benefit_judged,
                task.benefit_realization,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        marks,
        [
            ("rule.zero".to_owned(), Some(true), Some(true)),
            ("rule.other".to_owned(), Some(true), None),
        ]
    );
    let text = String::from_utf8(composition.sources().tasks.bytes.clone()).expect("UTF-8");
    assert!(
        !text.contains(&format!("\"{SCHEDULE_RULE_REALIZATION_MEMBER}\"")),
        "the composed task does not carry the rule member: {text}"
    );
    assert_eq!(
        composition.benefit_coverage(),
        [(
            GAME.to_owned(),
            BenefitCoverage {
                complete: 2,
                pending: 0,
                undeclared: 0,
                maintenance_declared: 0,
                realization: 1,
            }
        )]
    );

    // Refused at the rule: on a maintenance-role package, beside the maintenance declaration, of
    // another value, the task member written by the rule, and a composed cycle.
    let mut on_maintenance = realization_rule("rule.maint");
    on_maintenance["package_id"] = json!("rule.maint");
    let mut declared = realization_rule("rule.zero");
    declared["maintenance"] = json!(true);
    let mut other_value = realization_rule("rule.zero");
    other_value[SCHEDULE_RULE_REALIZATION_MEMBER] = json!(false);
    let mut written = rule("rule.zero", vec![produce(OUT, 1)], vec![consume(IN, 3)]);
    written["benefit_realization"] = json!(true);
    let mut looping = rule("rule.zero", vec![produce(IN, 1)], vec![consume(IN, 3)]);
    looping["reserves"] = json!([{"pool_id": IN, "at_least": 10}]);
    looping[SCHEDULE_RULE_REALIZATION_MEMBER] = json!(true);
    for (case, refused_rule, pointer, by_reader) in [
        (
            "a maintenance-role package",
            on_maintenance,
            "/rules/0/realization",
            false,
        ),
        (
            "a declared maintenance rule",
            declared,
            "/rules/0/realization",
            true,
        ),
        ("false", other_value, "/rules/0/realization", true),
        (
            "the task member",
            written,
            "/rules/0/benefit_realization",
            false,
        ),
        ("a cycle", looping, "/rules/0/realization", false),
    ] {
        let schedule = schedule(vec![refused_rule]);
        let bytes = serde_json::to_vec(&schedule).expect("schedule bytes");
        match parse_standard_pack_schedule(&bytes) {
            Err(issue) => {
                assert!(by_reader, "{case}: {issue:?}");
                assert_eq!(issue.file, STANDARD_PACK_SCHEDULE_FILE, "{case}");
                assert_eq!(
                    issue.problem,
                    StandardPackProblem::ScheduleInvalid,
                    "{case}"
                );
                assert_eq!(issue.pointer, pointer, "{case}");
            }
            Ok(_) => assert!(!by_reader, "{case}: the reader takes it"),
        }
        let failure = compose(schedule).expect_err(case);
        assert!(
            failure.issues().iter().any(|refused| {
                refused.issue.problem == StandardPackProblem::ScheduleInvalid
                    && refused.issue.pointer == pointer
            }),
            "{case}: {failure}"
        );
    }
}

// Test 15: ruling 127 (i), the cost formula unchanged. A pool that does not regenerate, full (100
// of 100) above its reserve line, costs its full weight: S = 1000 − 400 = 600, so does a scarce
// material whatever its fill. A regenerating pool costs nothing at capacity (S = 1000) and its
// weight at the reserve line (S = 1000 − 10·1000 = −9000).
#[test]
fn ss2d_t15_the_cost_of_a_pool_is_as_before() {
    let of_100 = |mut pool: Value| {
        pool["capacity"] = json!(100);
        pool
    };
    let mut pools = pools(1000);
    pools.push(of_100(pool(
        A,
        "pool.stock",
        json!({"kind": "none"}),
        Some(valuation(400, 1)),
    )));
    pools.push(of_100(pool(
        A,
        "pool.stamina",
        json!({"amount": 1, "per_ms": 6 * MINUTE_MS}),
        Some(valuation(1000, 1)),
    )));
    let full_stock = catalog(
        vec![with(
            task(
                POSITIVE,
                vec![produce(OUT, 1)],
                vec![consume("pool.stock", 1)],
            ),
            "reserves",
            json!([{"pool_id": "pool.stock", "at_least": 10}]),
        )],
        pools.clone(),
    );
    let evaluation = round(&full_stock, Vec::new(), vec![reading("pool.stock", 100)]);
    assert_eq!(dispatched(&evaluation), [POSITIVE], "{evaluation:#?}");
    let breakdown = reason(decision(&evaluation, A, POSITIVE), "rank_breakdown");
    assert_eq!(breakdown.outcome.value("benefit_milli"), Some(&json!(600)));

    let regenerating = catalog(
        vec![with(
            task(
                POSITIVE,
                vec![produce(OUT, 1)],
                vec![consume("pool.stamina", 10)],
            ),
            "reserves",
            json!([{"pool_id": "pool.stamina", "at_least": 20}]),
        )],
        pools,
    );
    let full = round(
        &regenerating,
        Vec::new(),
        vec![reading("pool.stamina", 100)],
    );
    let breakdown = reason(decision(&full, A, POSITIVE), "rank_breakdown");
    assert_eq!(breakdown.outcome.value("benefit_milli"), Some(&json!(1000)));
    let at_reserve = round(&regenerating, Vec::new(), vec![reading("pool.stamina", 20)]);
    let held = decision(&at_reserve, A, POSITIVE);
    assert_eq!(last_code(held), NONPOSITIVE, "{held:#?}");
    assert_eq!(
        last(held).outcome.value("benefit_milli"),
        Some(&json!(-9000))
    );
}

// Test 16: the no-progress guard bounds the zero tier. Released and dispatched once on a reading
// of 100; the run succeeded and the consumed pool has no newer reading of another value: the next
// round is SS-4's `policy_no_progress`, not dispatched. A reading of another value (97) releases
// the guard and the zero tier runs again.
#[test]
fn ss2d_t16_a_released_run_without_progress_waits_for_a_new_reading() {
    let catalog = catalog(vec![zero_task()], pools(998));
    let read_at = NOW - HOUR_MS;
    let first_at = NOW - 700_000;
    let dispatched_at = NOW - 600_000;
    let before = vec![fact(A, "resource.in", 100, read_at, None)];
    let first = evaluate(
        &catalog,
        &facts(vec![instance(A, &[])], Vec::new(), before.clone()),
        &RunSlots::default(),
        &NoProgressGuards::default(),
        first_at,
    );
    assert_eq!(dispatched(&first), [ZERO], "{first:#?}");
    assert_tolerance(reason(decision(&first, A, ZERO), WITHIN), -1, 2, "released");
    let mut memory = NoProgressMemory::default();
    memory
        .capture(
            &catalog,
            &facts(vec![instance(A, &[])], Vec::new(), before.clone()),
            &resources(
                &catalog,
                &facts(vec![instance(A, &[])], Vec::new(), before.clone()),
            ),
            time(first_at),
            &first.dispatch_intents,
            |alias| alias == A,
        )
        .unwrap_or_else(|error| panic!("the memory captures: {error}"));
    memory.admit(
        ZERO,
        A,
        &first.dispatch_intents[0].decision_id,
        dispatched_at,
    );
    let ran = vec![state(A, ZERO, dispatched_at, TaskTerminalState::Succeeded)];
    for (case, readings, held) in [
        ("no newer reading", before, true),
        (
            "a reading of another value",
            vec![fact(A, "resource.in", 97, dispatched_at + MINUTE_MS, None)],
            false,
        ),
    ] {
        let mut memory = memory.clone();
        let next = facts(vec![instance(A, &[])], ran.clone(), readings);
        memory
            .observe(&catalog, &next, time(NOW), |alias| alias == A)
            .unwrap_or_else(|error| panic!("{case}: the memory observes: {error}"));
        let evaluation = evaluate(&catalog, &next, &RunSlots::default(), &memory.guards(), NOW);
        let decided = decision(&evaluation, A, ZERO);
        if held {
            assert!(
                dispatched(&evaluation).is_empty(),
                "{case}: {evaluation:#?}"
            );
            assert_eq!(
                last_code(decided),
                "policy_no_progress",
                "{case}: {decided:#?}"
            );
        } else {
            assert_eq!(dispatched(&evaluation), [ZERO], "{case}: {evaluation:#?}");
            assert_tolerance(reason(decided, WITHIN), -1, 2, "released");
        }
    }
}

// Test 18: unchanged. A catalog with no candidate in the band: S = 3 > ε = 2 dispatched with B =
// 3; S = 1002 − 1332 = −330 not positive; S = 600/1000 = 0.6 above its ε = 1/2000 but B = ⌊0.6⌋ =
// 0, not positive as before; no effect, unknown. With the realization mark on the positive task,
// every decision is byte for byte the one without it; no record names the new code or values, and
// the catalog without the mark does not carry the member.
#[test]
fn ss2d_t18_a_catalog_with_nothing_in_the_band_decides_as_before() {
    let mut pools = pools(1002);
    pools.push(valued("pool.out_fine", 600, 1000));
    let tasks = |mark: bool| {
        let positive = with(
            task(POSITIVE, vec![produce(OUT, 1)], vec![consume(IN, 3)]),
            "reserves",
            json!([{"pool_id": IN, "at_least": 10}]),
        );
        vec![
            if mark {
                with(positive, "benefit_realization", json!(true))
            } else {
                positive
            },
            task("task_negative", vec![produce(OUT, 1)], vec![consume(IN, 4)]),
            task("task_small", vec![produce("pool.out_fine", 1)], Vec::new()),
            task("task_none", Vec::new(), Vec::new()),
        ]
    };
    let unmarked = catalog(tasks(false), pools.clone());
    let marked_catalog = catalog(tasks(true), pools);
    let before = round(&unmarked, Vec::new(), readings());
    let after = round(&marked_catalog, Vec::new(), readings());
    assert_eq!(
        serde_json::to_vec(&before.decisions).expect("decisions"),
        serde_json::to_vec(&after.decisions).expect("decisions"),
        "the mark changes nothing outside the band"
    );
    assert_eq!(dispatched(&before), [POSITIVE], "{before:#?}");
    assert_eq!(dispatched(&after), [POSITIVE], "{after:#?}");
    let breakdown = reason(decision(&before, A, POSITIVE), "rank_breakdown");
    assert!(
        breakdown
            .detail
            .ends_with(" benefit_milli=3 expected_duration_ms=60000"),
        "{}",
        breakdown.detail
    );
    for (task_id, code, benefit) in [
        ("task_negative", NONPOSITIVE, Some(-330)),
        ("task_small", NONPOSITIVE, Some(0)),
        ("task_none", "benefit_unknown", None),
    ] {
        let held = decision(&before, A, task_id);
        assert_eq!(last_code(held), code, "{task_id}: {held:#?}");
        assert_eq!(
            last(held).outcome.value("benefit_milli"),
            benefit.map(|benefit| json!(benefit)).as_ref(),
            "{task_id}"
        );
    }
    for evaluation in [&before, &after] {
        let text = serde_json::to_string(evaluation).expect("text");
        for absent in [WITHIN, "tolerance_milli", "zero_tier"] {
            assert!(!text.contains(absent), "{absent}: {text}");
        }
    }
    let compiled = serde_json::to_string(unmarked.catalog().tasks.tasks.as_slice()).expect("tasks");
    assert!(!compiled.contains("benefit_realization"), "{compiled}");
}

// Test 19: the production cross term (appendix A5, case 1). Two thousandths of a unit expected at
// W = 2, Q = 1: that pool's ε is 1/1000 + 5/4000 = 9/4000, not the second version's 1/500. With one
// unit of a pool at W = 6100 per 10^6 consumed (ε 1/2 000 000): S = 4/1000 − 6100/10^6 = −21/10000
// and ε = 4501/2000000. |S| = 4200/2000000 is within it, and outside the 4001/2000000 that 1/500
// would give.
#[test]
fn ss2d_t19_an_expected_production_carries_its_cross_term() {
    let catalog = catalog(
        vec![task(
            ZERO,
            vec![expected("pool.trace", "produce", 2)],
            vec![consume("pool.dust", 1)],
        )],
        vec![
            valued("pool.trace", 2, 1),
            valued("pool.dust", 6100, 1_000_000),
        ],
    );
    let evaluation = round(&catalog, Vec::new(), Vec::new());
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, A, ZERO);
    assert_eq!(last_code(held), WITHIN, "{held:#?}");
    assert_tolerance(last(held), -1, 1, "unmarked");
    assert!(
        last(held)
            .detail
            .contains(" sum=-21/10000 tolerance=4501/2000000 "),
        "{}",
        last(held).detail
    );
}

// Test 20: an expected consumption in ε (SS-2g). 26 thousandths of a pool at W = 2000, Q = 1 cost
// 52; that pool's ε is 26/2000 + 4001/4000 = 4053/4000, and one produced unit at Q = 1 adds 1/2:
// ε = 6053/4000. Produced at 54, S = 2 > ε is dispatched with B = 2; at 53, S = 1 is within
// tolerance; at 50, S = −2 < −ε is not positive.
#[test]
fn ss2d_t20_an_expected_consumption_enters_the_bound() {
    for (weight, band) in [(54, "positive"), (53, "within"), (50, "nonpositive")] {
        let catalog = catalog(
            vec![task(
                ZERO,
                vec![produce(OUT, 1)],
                vec![expected("pool.cube", "consume", 26)],
            )],
            vec![valued(OUT, weight, 1), valued("pool.cube", 2000, 1)],
        );
        let evaluation = round(&catalog, Vec::new(), Vec::new());
        let decided = decision(&evaluation, A, ZERO);
        match band {
            "positive" => {
                assert_eq!(dispatched(&evaluation), [ZERO], "{evaluation:#?}");
                let breakdown = reason(decided, "rank_breakdown");
                assert_eq!(breakdown.outcome.value("benefit_milli"), Some(&json!(2)));
            }
            "within" => {
                assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
                assert_eq!(last_code(decided), WITHIN, "{decided:#?}");
                assert_tolerance(last(decided), 1, 2, "unmarked");
                assert!(
                    last(decided)
                        .detail
                        .contains(" sum=1/1 tolerance=6053/4000 "),
                    "{}",
                    last(decided).detail
                );
            }
            _ => {
                assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
                assert_eq!(last_code(decided), NONPOSITIVE, "{decided:#?}");
                assert_eq!(
                    last(decided).outcome.value("benefit_milli"),
                    Some(&json!(-2))
                );
            }
        }
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

/// An entry of `milli` thousandths of a unit expected per run.
fn expected(pool: &str, direction: &str, milli: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "expected_amount_milli": milli,
        "observation_source": "self_reported",
        "confidence_milli": 1000
    })
}

/// The members of a task of the game on its hourly clock, but `id`, its effects and its scope.
fn task_body(id: &str, produces: Vec<Value>, consumes: Vec<Value>) -> Value {
    json!({
        "id": id,
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
        "priority": 100,
        "trigger": hourly(),
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
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// A judged task of the game (marked `benefit_judged`), not marked as a realization.
fn task(id: &str, produces: Vec<Value>, consumes: Vec<Value>) -> Value {
    let mut task = task_body(id, produces, consumes);
    task["scope"] = json!({"kind": "game", "game_id": GAME});
    task["benefit_judged"] = json!(true);
    task
}

/// `task` marked `benefit_realization`, with these reserves.
fn marked(task: Value, reserves: &[(&str, u64)]) -> Value {
    let reserves = reserves
        .iter()
        .map(|(pool_id, at_least)| json!({"pool_id": pool_id, "at_least": at_least}))
        .collect::<Vec<_>>();
    with(
        with(task, "reserves", Value::Array(reserves)),
        "benefit_realization",
        json!(true),
    )
}

/// The marked task of tests 3 to 12: one unit of OUT for three of IN, IN's reserve line at 10.
fn zero_task() -> Value {
    marked(
        task(ZERO, vec![produce(OUT, 1)], vec![consume(IN, 3)]),
        &[(IN, 10)],
    )
}

/// The marked task of tests 4 and 12: one unit of OUT for 2999 of IN_FINE and one of IN_EXTRA.
fn three_pool_task() -> Value {
    marked(
        task(
            ZERO,
            vec![produce(OUT, 1)],
            vec![consume(IN_FINE, 2999), consume(IN_EXTRA, 1)],
        ),
        &[(IN_FINE, 10), (IN_EXTRA, 0)],
    )
}

/// `task` scoped to the instance `alias`.
fn on(task: Value, alias: &str) -> Value {
    with(
        task,
        "scope",
        json!({"kind": "instance", "instance_id": alias}),
    )
}

fn with(mut task: Value, name: &str, value: Value) -> Value {
    task[name] = value;
    task
}

fn without(mut task: Value, name: &str) -> Value {
    task.as_object_mut()
        .expect("a task object")
        .shift_remove(name);
    task
}

/// A pool of `alias` read from `resource.<the id without "pool.">`.
fn pool(alias: &str, id: &str, projection: Value, valuation: Option<Value>) -> Value {
    let mut pool = json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": alias},
        "capacity": 1_000_000,
        "projection": projection,
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    });
    if let Some(valuation) = valuation {
        pool["valuation"] = valuation;
    }
    pool
}

/// A pool of `alias` that does not regenerate, weighed `weight` milli per `scale` units.
fn valued_on(alias: &str, id: &str, weight: u32, scale: u64) -> Value {
    pool(
        alias,
        id,
        json!({"kind": "none"}),
        Some(valuation(weight, scale)),
    )
}

fn valued(id: &str, weight: u32, scale: u64) -> Value {
    valued_on(A, id, weight, scale)
}

fn valuation(weight: u32, scale: u64) -> Value {
    json!({"name": "Fixture", "unit": "unit", "scale": scale, "base_weight_milli": weight})
}

/// A valued pool of A read from the outcome `count` of `task_id`, not from a fact.
fn outcome_pool(id: &str, task_id: &str) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": 1_000_000,
        "projection": {"kind": "none"},
        "observation": {"kind": "outcome", "task_id": task_id, "outcome_key": "count"},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": valuation(100, 1)
    })
}

/// OUT at `out_weight`, IN, IN_FINE and IN_EXTRA, in this order.
fn pools(out_weight: u32) -> Vec<Value> {
    vec![
        valued(OUT, out_weight, 1),
        valued(IN, 333, 1),
        valued(IN_FINE, 333, 1000),
        valued(IN_EXTRA, 334, 1000),
    ]
}

/// A reading of `pool` on A, a minute before NOW.
fn reading(pool: &str, value: i64) -> ObservedFact {
    fact(
        A,
        &format!("resource.{}", &pool["pool.".len()..]),
        value,
        NOW - MINUTE_MS,
        None,
    )
}

/// The consumed pools read well above every reserve line: IN 100, IN_FINE 100 000, IN_EXTRA 100.
fn readings() -> Vec<ObservedFact> {
    vec![
        reading(IN, 100),
        reading(IN_FINE, 100_000),
        reading(IN_EXTRA, 100),
    ]
}

fn catalog(tasks: Vec<Value>, pools: Vec<Value>) -> CompiledCatalog {
    compile_v2(tasks, pools).unwrap_or_else(|failure| {
        panic!("the fixture catalog compiles: {:?}", failure.diagnostics())
    })
}

fn compile_v2(
    tasks: Vec<Value>,
    pools: Vec<Value>,
) -> Result<CompiledCatalog, CatalogCompileFailure> {
    let header = json!({
        "catalog_id": "fixture.zero-band",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-zero-band"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION_V2,
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
        pools: source("pools", &document("pools", Value::Array(pools))),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
}

/// The V1 example catalog `catalog-a`, its tasks document changed by `mutate`.
fn compile_v1_example(
    mutate: impl FnOnce(&mut Value),
) -> Result<CompiledCatalog, CatalogCompileFailure> {
    let parse = |text: &str| serde_json::from_str::<Value>(text).expect("an example document");
    let mut tasks = parse(include_str!(
        "../../../../contracts/scheduling/examples/catalog-a/tasks.json"
    ));
    mutate(&mut tasks);
    compile_catalog(&CatalogSources {
        tasks: source("v1-tasks", &tasks),
        pools: source(
            "v1-pools",
            &parse(include_str!(
                "../../../../contracts/scheduling/examples/catalog-a/pools.json"
            )),
        ),
        activity: source(
            "v1-activity",
            &parse(include_str!(
                "../../../../contracts/scheduling/examples/catalog-a/activity.json"
            )),
        ),
        timeline: source(
            "v1-timeline",
            &parse(include_str!(
                "../../../../contracts/scheduling/examples/catalog-a/timeline.json"
            )),
        ),
        selection: None,
    })
}

fn refused_at(failure: &CatalogCompileFailure, code: CatalogDiagnosticCode, path: &str) -> bool {
    failure
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == code && diagnostic.json_path == path)
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-zero-band/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

/// `task_id`'s latest run on `alias`, dispatched at `at`, ended in `terminal`.
fn state(alias: &str, task_id: &str, at: u64, terminal: TaskTerminalState) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: task_id.to_owned(),
        instance_id: alias.to_owned(),
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

/// A v2 goal on OUT, at least 100, stored for A as the formal entry stores it.
fn stored_goal(catalog: &CompiledCatalog) -> ObservedFact {
    let document = json!({
        "schema_version": "actingcommand.resource-targets.v2",
        "instance": A,
        "valid_until_unix_ms": NOW + DAY_MS,
        "targets": [{
            "id": "goal-out",
            "resource": OUT,
            "condition": {"kind": "at_least", "amount": 100},
            "apply": {"mode": "adjust", "weight": "score_stage"}
        }]
    });
    let parsed = parse_resource_targets(&serde_json::to_vec(&document).expect("document bytes"))
        .unwrap_or_else(|error| panic!("the goal parses: {error:?}"));
    let checked = check_resource_targets(
        &parsed,
        catalog,
        &facts(vec![instance(A, &[])], Vec::new(), Vec::new()),
        time(NOW - HOUR_MS),
    )
    .unwrap_or_else(|error| panic!("the goal is accepted: {error:?}"));
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: actingcommand_contract::RESOURCE_TARGETS_FACT_KEY.to_owned(),
        value: FactValue::RecordList(
            serde_json::from_value(serde_json::to_value(&checked.rows).expect("rows"))
                .expect("record list"),
        ),
        observed_at_unix_ms: NOW - HOUR_MS,
        expires_at_unix_ms: Some(NOW + DAY_MS),
        confidence_milli: 1000,
    }
}

fn facts(
    instances: Vec<InstanceSnapshot>,
    tasks: Vec<TaskRuntimeSnapshot>,
    readings: Vec<ObservedFact>,
) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-zero-band".to_owned(),
        facts: readings,
        outcomes: Vec::new(),
        tasks,
        instances,
        priority_offsets: Vec::new(),
    }
}

fn time(unix_ms: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms,
        monotonic_ms: unix_ms,
    }
}

/// The pools' values as the Runtime derives them from the readings, each instance on its own idle
/// host.
fn resources(catalog: &CompiledCatalog, facts: &EvaluationFacts) -> EvaluationResources {
    project_fact_pools(
        catalog,
        facts,
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

/// The Runtime's round at `at` with `run_slots` and `guards`; the admission takes every candidate.
fn evaluate(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    run_slots: &RunSlots,
    guards: &NoProgressGuards,
    at: u64,
) -> PolicyEvaluation {
    evaluate_with_no_progress::<PolicyEvaluationError>(
        catalog,
        facts,
        &resources(catalog, facts),
        time(at),
        11,
        &BTreeSet::new(),
        &refresh_inputs(),
        &RemedyPlan::default(),
        &ChainHandoverInputs::default(),
        run_slots,
        &OwedReruns::default(),
        &InstanceHolds::default(),
        guards,
        |_, _, _| Ok(false),
        |_| Ok(CandidateEligibility::Eligible),
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

/// One round at NOW on instance A, no run limit, no guard.
fn round(
    catalog: &CompiledCatalog,
    tasks: Vec<TaskRuntimeSnapshot>,
    readings: Vec<ObservedFact>,
) -> PolicyEvaluation {
    evaluate(
        catalog,
        &facts(vec![instance(A, &[])], tasks, readings),
        &RunSlots::default(),
        &NoProgressGuards::default(),
        NOW,
    )
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

fn reason<'a>(decision: &'a TaskDecision, code: &str) -> &'a DecisionReason {
    decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == code)
        .unwrap_or_else(|| panic!("a {code} reason on {decision:#?}"))
}

fn last(decision: &TaskDecision) -> &DecisionReason {
    decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {decision:#?}"))
}

fn last_code(decision: &TaskDecision) -> &str {
    last(decision).outcome.code().as_str()
}

/// The reason is `benefit_within_tolerance` with exactly these values.
fn assert_tolerance(reason: &DecisionReason, benefit: i64, tolerance: i64, tier: &str) {
    assert!(
        reason.outcome.is(PolicyCode::BenefitWithinTolerance),
        "{reason:#?}"
    );
    let values = reason.outcome.values();
    assert_eq!(values.len(), 3, "{}: {values:?}", reason.detail);
    assert_eq!(
        values.get("benefit_milli"),
        Some(&json!(benefit)),
        "{}",
        reason.detail
    );
    assert_eq!(
        values.get("tolerance_milli"),
        Some(&json!(tolerance)),
        "{}",
        reason.detail
    );
    assert_eq!(
        values.get("zero_tier"),
        Some(&json!(tier)),
        "{}",
        reason.detail
    );
}

/// No decision of the evaluation names the zero band.
fn no_tolerance(evaluation: &PolicyEvaluation) -> bool {
    !evaluation
        .decisions
        .iter()
        .flat_map(|decision| decision.reasons.iter())
        .any(|reason| reason.outcome.is(PolicyCode::BenefitWithinTolerance))
}

// ---- The standard-pack fixture of test 14 ----

/// A `schedule.json` rule running the package of its own id on its hourly clock, unscoped.
fn rule(id: &str, produces: Vec<Value>, consumes: Vec<Value>) -> Value {
    let mut rule = task_body(id, produces, consumes);
    let members = rule.as_object_mut().expect("a task object");
    for member in ["entrypoint", "procedure_ref", "instance_overrides"] {
        members.shift_remove(member);
    }
    members.insert("package_id".to_owned(), json!(id));
    rule
}

/// The realization rule: test 3's task as a rule carrying the model's mark.
fn realization_rule(id: &str) -> Value {
    let mut rule = rule(id, vec![produce(OUT, 1)], vec![consume(IN, 3)]);
    rule["reserves"] = json!([{"pool_id": IN, "at_least": 10}]);
    rule[SCHEDULE_RULE_REALIZATION_MEMBER] = json!(true);
    rule
}

/// `schedule.json` with these rules and the pools of `pools(998)`, without scopes.
fn schedule(rules: Vec<Value>) -> Value {
    let pools = pools(998)
        .into_iter()
        .map(|mut pool| {
            pool.as_object_mut()
                .expect("a pool object")
                .shift_remove("scope");
            pool
        })
        .collect::<Vec<_>>();
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [],
        "rules": rules,
        "pools": pools,
        "activity": [],
        "timeline": [],
        "held": []
    })
}

/// Composes `schedule` for one instance of the game, whose registry holds a package per rule and
/// `rule.maint`, a return-home maintenance package.
fn compose(schedule: Value) -> Result<StandardPackComposition, crate::StandardPackComposeFailure> {
    let packages = schedule["rules"]
        .as_array()
        .expect("the rules")
        .iter()
        .map(|rule| rule["package_id"].as_str().expect("a package").to_owned())
        .chain(["rule.maint".to_owned()])
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
            .collect::<Vec<_>>(),
        "maintenance": [{"package_id": "rule.maint", "server": SERVER, "uses": ["return_home"]}]
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
    let maintenance = serde_json::from_slice::<BundleIndex>(&bundle)
        .expect("a bundle index")
        .maintenance()
        .to_vec();
    let schedule = serde_json::to_vec(&schedule).expect("schedule bytes");
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &schedule,
            root: &root,
            maintenance: &maintenance,
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
        subsystems: None,
        catalog_version: 1,
    })
}
