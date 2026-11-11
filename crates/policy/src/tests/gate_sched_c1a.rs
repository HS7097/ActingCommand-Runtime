// SPDX-License-Identifier: AGPL-3.0-only

// #428 SCHED-C1a (register §1.299; Alice's two kinds of scoring of 2026-11-09 15:47 and 15:50,
// board #428 6734155178; the coordinator's rulings of 11-11, not Alice's words; coordinator work
// order `rd-1111/brief-sched-c1a.md`; contracts/scheduling/README.md, "Fixed values written in the
// pack", "Net benefit within a tier", "Capped spending order"; contracts/standard-pack-schedule.md,
// "The fixed value", "Provenance"):
// - a judged task's `benefit_fixed_value_milli` `V` is its known net benefit without a
//   consumption, and `V` less the cost of what it consumes otherwise, its `produces` unread;
// - static and dynamic candidates rank on one scale: `B ÷ E` compared exactly in the business
//   tier, `V ÷ c` in a capped round;
// - a consumed pool still needs its valuation, and a regenerating one its reading;
// - every decision whose `B` came from a fixed value carries `benefit_fixed_value` first, and a
//   judged winner's `rank_breakdown` names `V`;
// - the compiler refuses the member at its pointer, `fixed_value_conflict` beside a priced product
//   or the realization mark;
// - the composer writes it from the rule member `fixed_value_milli`, keeps the origins out of the
//   catalog, and refuses it without `requires`, on a maintenance rule and as the task member;
// - a catalog without the member keeps its golden hashes.
//
// The catalog is neutral and V2, as SS-2h's: tasks on one instance, the capped pool `pool.neutral`
// read from `resource.neutral`, its capacity read from `resource.neutral.capacity` (180), valued
// 1000 milli a unit (Q = 1), no regeneration, its stop line the capacity less 40 (140) and its hard
// line the capacity less 76 (104); `pool.out` valued 1000 milli a unit. A run is simulated: the
// pool is read again half a minute after the dispatch, and the next round is a minute after it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    BENEFIT_FIXED_VALUE_SUBSYSTEM, CandidateEligibility, CappedMemory, CatalogCompileFailure,
    CatalogDiagnosticCode, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DecisionReason, DispatchIntent, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactRefreshInputs, FactRefreshSettings, HostResourceSnapshot, InstanceHolds,
    NoProgressGuards, OwedReruns, PolicyCode, PolicyEvaluation, PolicyEvaluationError, RemedyPlan,
    RunSlots, SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2, SchedulingDecisionState,
    StandardPackBinding, StandardPackComposeInput, StandardPackComposition, StandardPackGameFiles,
    TaskDecision, TaskRuntimeSnapshot, TaskTerminalState, compile_catalog,
    compose_standard_pack_rules, evaluate_with_capped_spending, project_fact_pools,
};
use actingcommand_contract::outcome::{Category, Outcome};
use actingcommand_contract::{
    BundleIndex, STANDARD_PACK_SCHEDULE_FILE, StandardPackProblem, StandardPackRootBytes,
    parse_standard_pack_schedule, read_standard_pack_root,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// The capped pool, read from `READING_KEY`.
const POOL: &str = "pool.neutral";
const READING_KEY: &str = "resource.neutral";
/// The capacity fact of the capped pool, in its scope.
const CAPACITY_KEY: &str = "resource.neutral.capacity";
/// The produced pool, 1000 milli a unit.
const OUT: &str = "pool.out";
/// A pool without a valuation.
const BARE: &str = "pool.bare";
/// A pool whose valuation is the pending marker.
const LATER: &str = "pool.later";
/// The declared duration of every task, `E` without a snapshot.
const E: u64 = 60_000;
/// The neutral example catalog's golden identities, computed before SS-2h (as in `gate_ss2h`).
const V1_EXAMPLE_HASH: &str =
    "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c";
const V2_EXAMPLE_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";

// Test 1, a fixed value without effects. A judged collect rule that consumes and produces nothing
// and writes `V` = 12 000 is dispatched with the known `B = V`: its decision opens with
// `benefit_fixed_value` (`benefit_milli` 12 000, `fixed_value_milli` 12 000, `expected_duration_ms`
// 60 000), and its `rank_breakdown` names `V` after `B` and `E`. The same rule without the member
// is `benefit_unknown` `no_effects`. A fixed value leaves the rule's `produces` unread: with a
// production of a pool without a valuation it is still `B = V`.
#[test]
fn sched_c1a_t1_a_fixed_value_without_effects_is_its_known_benefit() {
    let mut world = World::new(
        catalog(
            vec![
                judged("task_plain", 0, 0),
                fixed(judged("task_fixed", 0, 0), 12_000),
            ],
            json!([out_pool()]),
        ),
        0,
    );
    let evaluation = world.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_fixed"], "{evaluation:#?}");
    let winner = decision(&evaluation, "task_fixed");
    fixed_first(winner, 12_000, 12_000, E);
    let breakdown = reason(winner, PolicyCode::RankBreakdown);
    assert_eq!(
        breakdown.outcome.value("benefit_milli"),
        Some(&json!(12_000))
    );
    assert!(
        breakdown
            .detail
            .ends_with(" benefit_milli=12000 expected_duration_ms=60000 fixed_value_milli=12000"),
        "{}",
        breakdown.detail
    );
    let plain = decision(&evaluation, "task_plain");
    assert_eq!(plain.state, SchedulingDecisionState::Deferred);
    assert!(
        last(plain).outcome.is(PolicyCode::BenefitUnknown),
        "{plain:#?}"
    );
    assert_eq!(
        last(plain).outcome.value("benefit_gap"),
        Some(&json!("no_effects"))
    );
    assert!(no_fixed_reason(plain), "{plain:#?}");

    let mut unread = World::new(
        catalog(
            vec![fixed(
                with(
                    judged("task_fixed", 0, 0),
                    "produces",
                    json!([produce(BARE, 3)]),
                ),
                12_000,
            )],
            json!([out_pool(), bare_pool(BARE)]),
        ),
        0,
    );
    let evaluation = unread.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_fixed"], "{evaluation:#?}");
    fixed_first(decision(&evaluation, "task_fixed"), 12_000, 12_000, E);
}

// Test 2, one scale. Two collect rules of one instance order by `B ÷ E` compared exactly: A
// (`V` 10 001, `E` 60 000) before B (`V` 5 000, `E` 30 000), declared first, since
// 10 001 · 30 000 > 5 000 · 60 000 (a quotient rounded to an integer would tie them at 0). C
// (5 000 at 30 000) and D (10 000 at 60 000) tie exactly, so the tie-break decides: the one
// declared first runs, whichever it is.
#[test]
fn sched_c1a_t2_two_collect_rules_order_by_b_per_e_exactly() {
    let rule = |id: &str, value: i64, expected: u64| {
        with(
            fixed(judged(id, 0, 0), value),
            "expected_duration_ms",
            json!(expected),
        )
    };
    let mut exact = World::new(
        catalog(
            vec![
                rule("task_b", 5_000, 30_000),
                rule("task_a", 10_001, 60_000),
            ],
            json!([out_pool()]),
        ),
        0,
    );
    let evaluation = exact.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_a"], "{evaluation:#?}");
    let breakdown = reason(decision(&evaluation, "task_a"), PolicyCode::RankBreakdown);
    assert!(
        breakdown
            .detail
            .ends_with(" benefit_milli=10001 expected_duration_ms=60000 fixed_value_milli=10001"),
        "{}",
        breakdown.detail
    );
    let second = decision(&evaluation, "task_b");
    assert!(
        last(second).outcome.is(PolicyCode::InstanceAlreadySelected),
        "{second:#?}"
    );
    fixed_first(second, 5_000, 5_000, 30_000);

    for (order, winner) in [
        (["task_c", "task_d"], "task_c"),
        (["task_d", "task_c"], "task_d"),
    ] {
        let tasks = order
            .iter()
            .map(|id| match *id {
                "task_c" => rule("task_c", 5_000, 30_000),
                _ => rule("task_d", 10_000, 60_000),
            })
            .collect();
        let mut tied = World::new(catalog(tasks, json!([out_pool()])), 0);
        let evaluation = tied.evaluate();
        assert_eq!(
            dispatched(&evaluation),
            [winner],
            "{order:?}: {evaluation:#?}"
        );
    }
}

// Test 3, a consumer below the cap. Read at 150, below the cap of 180, a consumer of 30 units at
// W = 1000 is judged by `V − W·c`: with `V` 40 000, B = 10 000 and it runs (150 − 30 = 120, above
// the hard line); with `V` 20 000, B = −10 000, `policy_benefit_nonpositive`; with `V` 30 000,
// S = 0 within ε = 30 000/2000 = 15, the consumption's term alone, `benefit_within_tolerance`.
// Read at 120, the consumer worth 10 000 would leave 90 < 104: the hard line holds it
// (`would_cross_hard_line`). Each decision opens with `benefit_fixed_value` and its own `B`.
#[test]
fn sched_c1a_t3_below_the_cap_a_consumer_is_judged_by_v_less_its_cost() {
    let consumer = |value: i64| catalog(vec![fixed(judged("task_v", 30, 0), value)], pools());

    let mut positive = World::new(consumer(40_000), 150);
    let evaluation = positive.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_v"], "{evaluation:#?}");
    let runs = decision(&evaluation, "task_v");
    fixed_first(runs, 10_000, 40_000, E);
    assert_eq!(
        reason(runs, PolicyCode::RankBreakdown)
            .outcome
            .value("benefit_milli"),
        Some(&json!(10_000))
    );

    let mut negative = World::new(consumer(20_000), 150);
    let evaluation = negative.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, "task_v");
    assert_eq!(held.state, SchedulingDecisionState::Deferred);
    assert!(
        last(held).outcome.is(PolicyCode::PolicyBenefitNonpositive),
        "{held:#?}"
    );
    assert_eq!(
        last(held).outcome.value("benefit_milli"),
        Some(&json!(-10_000))
    );
    fixed_first(held, -10_000, 20_000, E);

    let mut zero = World::new(consumer(30_000), 150);
    let evaluation = zero.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let within = decision(&evaluation, "task_v");
    assert!(
        last(within).outcome.is(PolicyCode::BenefitWithinTolerance),
        "{within:#?}"
    );
    assert_eq!(
        last(within).outcome.value("tolerance_milli"),
        Some(&json!(15))
    );
    assert!(
        last(within).detail.contains(" sum=0/1 tolerance=15/1 "),
        "{}",
        last(within).detail
    );
    fixed_first(within, 0, 30_000, E);

    let mut low = World::new(consumer(40_000), 120);
    let evaluation = low.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let kept = decision(&evaluation, "task_v");
    assert_eq!(hold(kept).as_deref(), Some("would_cross_hard_line"));
    assert_eq!(last(kept).outcome.value("amount_after"), Some(&json!(90)));
    fixed_first(kept, 10_000, 40_000, E);
}

// Test 4, a capped round. Read at 200 (cap 180), a fixed consumer F (30 units, `V` 36 000, `V ÷ c`
// 1200) and a modelled one M (25 units, 20 units of `pool.out` produced, `V` 20 000, `V ÷ c` 800)
// are frozen F then M: F runs as the head (`capped_spend`, rank 1), its B = 36 000 − 1000·(30 − 30)
// = 36 000, while M waits (`capped_not_head`); at 170 M runs as rank 2 of the same round. With `V`
// 18 000 (`V ÷ c` 600) the order is M then F, and F waits for M with its own B = 18 000.
#[test]
fn sched_c1a_t4_in_a_capped_round_a_fixed_consumer_ranks_by_v_per_c() {
    let mut world = World::new(
        catalog(
            vec![
                fixed(judged("task_f", 30, 0), 36_000),
                judged("task_m", 25, 20),
            ],
            pools(),
        ),
        200,
    );
    let first = world.evaluate();
    assert_eq!(dispatched(&first), ["task_f"], "{first:#?}");
    let head = spend(&first, "task_f");
    assert_eq!(head.value("capped_rank"), Some(&json!(1)));
    assert_eq!(head.value("capped_round"), Some(&json!(1)));
    let runs = decision(&first, "task_f");
    fixed_first(runs, 36_000, 36_000, E);
    assert!(
        reason(runs, PolicyCode::RankBreakdown)
            .detail
            .ends_with(" fixed_value_milli=36000"),
        "{runs:#?}"
    );
    assert_eq!(
        hold(decision(&first, "task_m")).as_deref(),
        Some("capped_not_head")
    );
    world.run(&first, 30);
    let second = world.evaluate();
    assert_eq!(dispatched(&second), ["task_m"], "{second:#?}");
    let next = spend(&second, "task_m");
    assert_eq!(next.value("capped_rank"), Some(&json!(2)), "frozen");
    assert_eq!(next.value("capped_round"), Some(&json!(1)));
    assert!(no_fixed_reason(decision(&second, "task_m")));

    let mut cheaper = World::new(
        catalog(
            vec![
                fixed(judged("task_f", 30, 0), 18_000),
                judged("task_m", 25, 20),
            ],
            pools(),
        ),
        200,
    );
    let evaluation = cheaper.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_m"], "{evaluation:#?}");
    assert_eq!(
        spend(&evaluation, "task_m").value("capped_rank"),
        Some(&json!(1))
    );
    let waits = decision(&evaluation, "task_f");
    assert_eq!(
        hold(waits).as_deref(),
        Some("capped_not_head"),
        "{waits:#?}"
    );
    fixed_first(waits, 18_000, 18_000, E);
}

// Test 5, a consumed pool without a value. A fixed consumer of a pool without a valuation is
// `benefit_unknown` `valuation_missing`, of a pool whose valuation is the pending marker
// `valuation_pending`, and of a valued regenerating pool without a reading `reading_unknown`; none
// is dispatched and none carries `benefit_fixed_value`, its `B` being unknown.
#[test]
fn sched_c1a_t5_a_consumed_pool_without_a_valuation_stays_unknown() {
    let consumer = |id: &str, pool: &str| {
        let mut task = task_body(id, 100, vec![consume(pool, 5)], Vec::new());
        task["benefit_judged"] = json!(true);
        fixed(task, 9_000)
    };
    let mut later = valued(LATER, 1000, 1000);
    later["valuation"] = json!("pending");
    let mut regenerating = valued("pool.regen", 1000, 240);
    regenerating["projection"] = json!({"amount": 1, "per_ms": 6 * MINUTE_MS});
    let mut world = World::new(
        catalog(
            vec![
                consumer("task_missing", BARE),
                consumer("task_pending", LATER),
                consumer("task_unread", "pool.regen"),
            ],
            json!([out_pool(), bare_pool(BARE), later, regenerating]),
        ),
        0,
    );
    let evaluation = world.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    for (task_id, gap, pool) in [
        ("task_missing", "valuation_missing", BARE),
        ("task_pending", "valuation_pending", LATER),
        ("task_unread", "reading_unknown", "pool.regen"),
    ] {
        let held = decision(&evaluation, task_id);
        // Found by its code, not as the last reason: a reading no reader is bound for adds the
        // warning `fact_refresh_unbound` after it.
        let unknown = &reason(held, PolicyCode::BenefitUnknown).outcome;
        assert_eq!(
            unknown.value("benefit_gap"),
            Some(&json!(gap)),
            "{task_id}: {held:#?}"
        );
        assert_eq!(unknown.value("pool_id"), Some(&json!(pool)), "{task_id}");
        assert!(no_fixed_reason(held), "{task_id}: {held:#?}");
    }
}

// Test 6, the compiler's refusals, each at its pointer: in V1 `unknown_field`; a string, `null` or
// a fraction `type_mismatch`; 0, −1 and 2^53 `limit_exceeded`; without `benefit_judged: true`
// `missing_required_field` at `benefit_judged`; `fixed_value_conflict` beside a production of a
// valued pool or of a pool whose valuation is pending (case a) and beside `benefit_realization:
// true` (case b). A production of a pool without a valuation and the largest value compile.
#[test]
fn sched_c1a_t6_the_compiler_refuses_the_member_at_its_pointer() {
    const MEMBER: &str = "/tasks/0/benefit_fixed_value_milli";
    let failure = compile_catalog(&example(SCHEDULING_SCHEMA_VERSION, |tasks, _| {
        tasks["tasks"][0]["benefit_fixed_value_milli"] = json!(12_000);
    }))
    .expect_err("V1");
    assert!(
        refused_at(&failure, CatalogDiagnosticCode::UnknownField, MEMBER),
        "{:?}",
        failure.diagnostics()
    );

    let refused = |case: &str, task: Value, pools: Value, code, path: &str| {
        let failure = compile_v2(vec![task], pools).expect_err(case);
        assert!(
            refused_at(&failure, code, path),
            "{case}: {:?}",
            failure.diagnostics()
        );
    };
    let base = || judged("task_v", 0, 0);
    for (case, value) in [
        ("a string", json!("12000")),
        ("null", Value::Null),
        ("a fraction", json!(1.5)),
    ] {
        let mut task = base();
        task["benefit_fixed_value_milli"] = value;
        refused(
            case,
            task,
            json!([out_pool()]),
            CatalogDiagnosticCode::TypeMismatch,
            MEMBER,
        );
    }
    for value in [0, -1, 9_007_199_254_740_992] {
        refused(
            &format!("{value}"),
            fixed(base(), value),
            json!([out_pool()]),
            CatalogDiagnosticCode::LimitExceeded,
            MEMBER,
        );
    }
    refused(
        "unjudged",
        fixed(task_body("task_v", 100, Vec::new(), Vec::new()), 12_000),
        json!([out_pool()]),
        CatalogDiagnosticCode::MissingRequiredField,
        "/tasks/0/benefit_judged",
    );
    let mut later = valued(LATER, 1000, 1000);
    later["valuation"] = json!("pending");
    for (case, produced) in [("a valued product", OUT), ("a pending product", LATER)] {
        refused(
            case,
            fixed(
                with(base(), "produces", json!([produce(produced, 1)])),
                12_000,
            ),
            json!([out_pool(), later.clone()]),
            CatalogDiagnosticCode::FixedValueConflict,
            MEMBER,
        );
    }
    refused(
        "the realization mark",
        with(
            fixed(judged("task_v", 30, 0), 12_000),
            "benefit_realization",
            json!(true),
        ),
        pools(),
        CatalogDiagnosticCode::FixedValueConflict,
        MEMBER,
    );

    let compiled = catalog(
        vec![
            fixed(with(base(), "produces", json!([produce(BARE, 1)])), 12_000),
            fixed(judged("task_w", 0, 0), 9_007_199_254_740_991),
        ],
        json!([out_pool(), bare_pool(BARE)]),
    );
    let values = compiled
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| task.benefit_fixed_value_milli)
        .collect::<Vec<_>>();
    assert_eq!(values, [Some(12_000), Some(9_007_199_254_740_991)]);

    assert_eq!(
        CatalogDiagnosticCode::FixedValueConflict.code(),
        PolicyCode::FixedValueConflict.code()
    );
    assert_eq!(PolicyCode::FixedValueConflict.category(), Category::Error);
    assert_eq!(PolicyCode::BenefitFixedValue.category(), Category::Info);
}

// Test 7, the composer. A rule's `fixed_value_milli` becomes the judged task's
// `benefit_fixed_value_milli`; neither `fixed_value_milli` nor `fixed_value_origin` reaches the
// composed task, nor `valuation_origin` the composed pool, so two files that differ only in the
// origins compose the same catalog hash and rules digest (their provenance differs). Refused, as
// `schedule_invalid` at the member: a file that writes the member without naming
// `benefit_fixed_value` in `requires`, at the first such member (a rule's, then a pool's
// `valuation_origin`); the member on a declared maintenance rule (the reader) or on a rule whose
// package is maintenance-role (the composer); the task member written by a rule; an origin of
// another value, an origin without a value, a valuation origin beside the pending marker (the
// reader); and the compiler's refusals of the task member, mapped back to `fixed_value_milli`.
#[test]
fn sched_c1a_t7_the_composer_writes_the_member_and_keeps_the_origins_out() {
    let requires = [BENEFIT_FIXED_VALUE_SUBSYSTEM];
    let pools = || vec![schedule_pool(OUT), schedule_pool(POOL)];
    let rules = || {
        vec![
            schedule_rule("rule.fixed", Some(12_000)),
            schedule_rule("rule.plain", None),
        ]
    };
    let mut marked_rules = rules();
    marked_rules[0]["fixed_value_origin"] = json!("model");
    let mut marked_pools = pools();
    marked_pools[1]["valuation_origin"] = json!("model");
    let marked = compose(&schedule(marked_rules, marked_pools, &requires))
        .unwrap_or_else(|failure| panic!("the marked file composes: {failure}"));
    let unmarked = compose(&schedule(rules(), pools(), &requires))
        .unwrap_or_else(|failure| panic!("the unmarked file composes: {failure}"));
    let members = marked
        .compiled()
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| {
            (
                task.id.clone(),
                task.benefit_judged,
                task.benefit_fixed_value_milli,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        members,
        [
            ("rule.fixed".to_owned(), Some(true), Some(12_000)),
            ("rule.plain".to_owned(), Some(true), None),
        ]
    );
    let document = |bytes: &[u8]| serde_json::from_slice::<Value>(bytes).expect("a document");
    let tasks = document(&marked.sources().tasks.bytes);
    for task in tasks["tasks"].as_array().expect("the tasks") {
        for member in ["fixed_value_milli", "fixed_value_origin"] {
            assert!(task.get(member).is_none(), "{member}: {task}");
        }
    }
    let composed_pools = document(&marked.sources().pools.bytes);
    for pool in composed_pools["pools"].as_array().expect("the pools") {
        assert!(pool.get("valuation_origin").is_none(), "{pool}");
    }
    assert_eq!(
        marked.compiled().catalog_hash(),
        unmarked.compiled().catalog_hash()
    );
    assert_eq!(marked.rules_digest(), unmarked.rules_digest());
    let [(_, provenance)] = marked.benefit_provenance() else {
        panic!("one game: {:?}", marked.benefit_provenance());
    };
    assert_eq!(provenance.fixed_value.model, ["rule.fixed"]);
    assert_eq!(provenance.valuation.model, [POOL]);
    let [(_, provenance)] = unmarked.benefit_provenance() else {
        panic!("one game: {:?}", unmarked.benefit_provenance());
    };
    assert_eq!(provenance.fixed_value.pack, ["rule.fixed"]);
    assert_eq!(provenance.valuation.pack, [OUT, POOL]);

    // `requires`: the first member that needs the subsystem, a rule's before a pool's.
    let mut late = rules();
    late.reverse();
    let failure = compose(&schedule(late, pools(), &[])).expect_err("requires");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleInvalid);
    assert_eq!(issue.pointer, "/rules/1/fixed_value_milli");
    let mut origin_only = pools();
    origin_only[1]["valuation_origin"] = json!("model");
    let failure = compose(&schedule(
        vec![schedule_rule("rule.plain", None)],
        origin_only,
        &[],
    ))
    .expect_err("requires");
    assert_eq!(
        failure.issues()[0].issue.pointer,
        "/pools/1/valuation_origin"
    );

    // The refusals at the rule or the pool.
    let mut declared = schedule_rule("rule.fixed", Some(12_000));
    declared["maintenance"] = json!(true);
    let on_maintenance = schedule_rule("rule.maint", Some(12_000));
    let mut written = schedule_rule("rule.plain", None);
    written["benefit_fixed_value_milli"] = json!(12_000);
    let mut other_origin = schedule_rule("rule.fixed", Some(12_000));
    other_origin["fixed_value_origin"] = json!("pack");
    let mut bare_origin = schedule_rule("rule.plain", None);
    bare_origin["fixed_value_origin"] = json!("model");
    let mut pending = pools();
    pending[1]["valuation"] = json!("pending");
    pending[1]["valuation_origin"] = json!("model");
    let mut priced = schedule_rule("rule.fixed", Some(12_000));
    priced["produces"] = json!([produce(OUT, 1)]);
    let mut text = schedule_rule("rule.fixed", None);
    text["fixed_value_milli"] = json!("12000");
    for (case, rule, file_pools, pointer, by_reader) in [
        (
            "a declared maintenance rule",
            declared,
            pools(),
            "/rules/0/fixed_value_milli",
            true,
        ),
        (
            "a maintenance-role package",
            on_maintenance,
            pools(),
            "/rules/0/fixed_value_milli",
            false,
        ),
        (
            "the task member",
            written,
            pools(),
            "/rules/0/benefit_fixed_value_milli",
            false,
        ),
        (
            "another origin",
            other_origin,
            pools(),
            "/rules/0/fixed_value_origin",
            true,
        ),
        (
            "an origin without a value",
            bare_origin,
            pools(),
            "/rules/0/fixed_value_origin",
            true,
        ),
        (
            "a valuation origin beside the pending marker",
            schedule_rule("rule.plain", None),
            pending,
            "/pools/1/valuation_origin",
            true,
        ),
        (
            "a priced product",
            priced,
            pools(),
            "/rules/0/fixed_value_milli",
            false,
        ),
        (
            "no integer",
            text,
            pools(),
            "/rules/0/fixed_value_milli",
            false,
        ),
    ] {
        let file = schedule(vec![rule], file_pools, &requires);
        let bytes = serde_json::to_vec(&file).expect("schedule bytes");
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
        let failure = compose(&file).expect_err(case);
        assert!(
            failure.issues().iter().any(|refused| {
                refused.issue.problem == StandardPackProblem::ScheduleInvalid
                    && refused.issue.pointer == pointer
            }),
            "{case}: {failure}"
        );
    }
}

// Test 8, identity. The neutral example catalog keeps its V1 and V2 golden identities; a judged
// catalog without the member serializes no `benefit_fixed_value_milli`, its decisions carry no
// `benefit_fixed_value` and its winner's `rank_breakdown` names no fixed value.
#[test]
fn sched_c1a_t8_without_the_member_nothing_changes() {
    for (version, golden) in [
        (SCHEDULING_SCHEMA_VERSION, V1_EXAMPLE_HASH),
        (SCHEDULING_SCHEMA_VERSION_V2, V2_EXAMPLE_HASH),
    ] {
        let example = compile_catalog(&example(version, |_, _| {}))
            .unwrap_or_else(|failure| panic!("{version}: {:?}", failure.diagnostics()));
        assert_eq!(example.catalog_hash(), golden, "{version}");
    }
    let plain = catalog(vec![judged("task_w", 30, 40)], pools());
    let text = serde_json::to_string(&plain.catalog().tasks.tasks).expect("tasks");
    assert!(!text.contains("benefit_fixed_value_milli"), "{text}");
    let mut world = World::new(plain, 140);
    let evaluation = world.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_w"], "{evaluation:#?}");
    let winner = decision(&evaluation, "task_w");
    assert!(no_fixed_reason(winner), "{winner:#?}");
    assert!(
        !reason(winner, PolicyCode::RankBreakdown)
            .detail
            .contains("fixed_value_milli"),
        "{winner:#?}"
    );
}

// ---- The world a test drives ----

/// The catalog, the Runtime's memory of the capped episodes, the capped pool's reading and the
/// tasks' snapshots.
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    reading: i64,
    read_at: u64,
    now: u64,
    states: Vec<TaskRuntimeSnapshot>,
}

impl World {
    /// The world at NOW, the capped pool read at `reading` a minute before.
    fn new(catalog: CompiledCatalog, reading: i64) -> Self {
        Self {
            catalog,
            memory: CappedMemory::default(),
            reading,
            read_at: NOW - MINUTE_MS,
            now: NOW,
            states: Vec::new(),
        }
    }

    fn facts(&self) -> EvaluationFacts {
        EvaluationFacts {
            ledger_position: 7,
            fact_snapshot_id: "snapshot:fixture-sched-c1a".to_owned(),
            facts: vec![
                fact(A, READING_KEY, self.reading, self.read_at, None),
                fact(A, CAPACITY_KEY, 180, NOW - HOUR_MS, None),
            ],
            outcomes: Vec::new(),
            tasks: self.states.clone(),
            instances: vec![instance(A, &[])],
            priority_offsets: Vec::new(),
        }
    }

    /// One round by the Runtime's entry, with its memory, every candidate admitted.
    fn evaluate(&mut self) -> PolicyEvaluation {
        let facts = self.facts();
        let resources = resources(&self.catalog, &facts);
        evaluate_with_capped_spending::<PolicyEvaluationError>(
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
            |_, _, _| Ok(false),
            |_: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
                Ok(CandidateEligibility::Eligible)
            },
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
    }

    /// Runs the round's one dispatch: it succeeds having consumed `spent`; the pool is read half a
    /// minute after the dispatch and the next round is a minute after it.
    fn run(&mut self, evaluation: &PolicyEvaluation, spent: i64) {
        let [intent] = evaluation.dispatch_intents.as_slice() else {
            panic!("one dispatch: {evaluation:#?}");
        };
        self.states.retain(|state| state.task_id != intent.task_id);
        self.states.push(TaskRuntimeSnapshot {
            task_id: intent.task_id.clone(),
            instance_id: A.to_owned(),
            last_dispatched_unix_ms: Some(self.now),
            eligible_since_unix_ms: None,
            terminal_state: Some(TaskTerminalState::Succeeded),
            completed_window: None,
            last_duration_ms: None,
            failure_streak: None,
            expected_runtime_ms: None,
            expected_runtime_runs: None,
        });
        self.reading -= spent;
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

fn resources(catalog: &CompiledCatalog, facts: &EvaluationFacts) -> EvaluationResources {
    project_fact_pools(
        catalog,
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
        .find(|decision| decision.task_id == task_id && decision.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}: {evaluation:#?}"))
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

/// The decision opens with exactly one `benefit_fixed_value`, of `B`, `V` and `E`.
fn fixed_first(decision: &TaskDecision, benefit: i64, value: i64, expected: u64) {
    let first = decision
        .reasons
        .first()
        .unwrap_or_else(|| panic!("a reason on {decision:#?}"));
    assert!(
        first.outcome.is(PolicyCode::BenefitFixedValue),
        "{decision:#?}"
    );
    assert_eq!(
        first.outcome.value("benefit_milli"),
        Some(&json!(benefit)),
        "{decision:#?}"
    );
    assert_eq!(
        first.outcome.value("fixed_value_milli"),
        Some(&json!(value))
    );
    assert_eq!(
        first.outcome.value("expected_duration_ms"),
        Some(&json!(expected))
    );
    let count = decision
        .reasons
        .iter()
        .filter(|reason| reason.outcome.is(PolicyCode::BenefitFixedValue))
        .count();
    assert_eq!(count, 1, "{decision:#?}");
}

/// The decision carries no `benefit_fixed_value`.
fn no_fixed_reason(decision: &TaskDecision) -> bool {
    !decision
        .reasons
        .iter()
        .any(|reason| reason.outcome.is(PolicyCode::BenefitFixedValue))
}

/// The `pool_line_hold` of the decision's last reason, when it is `pool_line_held`.
fn hold(decision: &TaskDecision) -> Option<String> {
    let last = last(decision);
    if !last.outcome.is(PolicyCode::PoolLineHeld) {
        return None;
    }
    last.outcome
        .value("pool_line_hold")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The `capped_spend` reason of `task_id`'s dispatch.
fn spend<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a Outcome {
    &reason(decision(evaluation, task_id), PolicyCode::CappedSpend).outcome
}

fn refused_at(failure: &CatalogCompileFailure, code: CatalogDiagnosticCode, path: &str) -> bool {
    failure
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == code && diagnostic.json_path == path)
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

/// A task of the game on its hourly clock, of `priority`, unmarked.
fn task_body(id: &str, priority: i16, consumes: Vec<Value>, produces: Vec<Value>) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
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

/// A judged task: `consumed` units of the capped pool a run (none when 0), due while the pool holds
/// them, and `produced` units of `pool.out` (none when 0).
fn judged(id: &str, consumed: u64, produced: u64) -> Value {
    let consumes = if consumed == 0 {
        Vec::new()
    } else {
        vec![consume(POOL, consumed)]
    };
    let produces = if produced == 0 {
        Vec::new()
    } else {
        vec![produce(OUT, produced)]
    };
    let mut task = task_body(id, 100, consumes, produces);
    task["benefit_judged"] = json!(true);
    if consumed > 0 {
        task["trigger"] = enough(consumed);
    }
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

/// [`valued`] without a valuation.
fn bare_pool(id: &str) -> Value {
    let mut pool = valued(id, 1000, 1000);
    pool.as_object_mut()
        .expect("a pool object")
        .shift_remove("valuation");
    pool
}

fn out_pool() -> Value {
    valued(OUT, 1000, 1_000_000)
}

/// The capacity fact plus `offset`.
fn line(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

/// The produced pool and the capped pool: its capacity read from its fact, its stop line 40 below
/// the cap and its hard line 76 below it.
fn pools() -> Value {
    let mut capped = valued(POOL, 1000, 1);
    capped["capacity"] = line(0);
    capped["stop_line"] = line(-40);
    capped["hard_line"] = line(-76);
    json!([out_pool(), capped])
}

fn catalog(tasks: Vec<Value>, pools: Value) -> CompiledCatalog {
    compile_v2(tasks, pools).unwrap_or_else(|failure| {
        panic!("the fixture catalog compiles: {:?}", failure.diagnostics())
    })
}

fn compile_v2(tasks: Vec<Value>, pools: Value) -> Result<CompiledCatalog, CatalogCompileFailure> {
    let header = json!({
        "catalog_id": "fixture.sched-c1a",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-sched-c1a"]
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
        pools: source("pools", &document("pools", pools)),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
}

/// The neutral example catalog `catalog-a` declared as `version` (V2: every timeline event valid
/// from 0, as its V2 golden was computed), its tasks and pools documents changed by `edit`.
fn example(version: &str, edit: impl FnOnce(&mut Value, &mut Value)) -> CatalogSources {
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
    let [mut tasks, mut pools, activity, timeline] = documents;
    edit(&mut tasks, &mut pools);
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
        format!("memory://fixture-sched-c1a/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

// ---- The standard-pack fixture of test 7 ----

/// A `schedule.json` rule running the package of its own id on its hourly clock and consuming 10
/// of the capped pool; with `value` it writes that fixed value and produces nothing, without it
/// produces one `pool.out`.
fn schedule_rule(id: &str, value: Option<i64>) -> Value {
    let produces = match value {
        Some(_) => Vec::new(),
        None => vec![produce(OUT, 1)],
    };
    let mut rule = task_body(id, 100, vec![consume(POOL, 10)], produces);
    let members = rule.as_object_mut().expect("a task object");
    for member in ["scope", "entrypoint", "procedure_ref", "instance_overrides"] {
        members.shift_remove(member);
    }
    members.insert("package_id".to_owned(), json!(id));
    if let Some(value) = value {
        members.insert("fixed_value_milli".to_owned(), json!(value));
    }
    rule
}

/// A `schedule.json` pool: `id` of constant capacity 240, valued, without a scope.
fn schedule_pool(id: &str) -> Value {
    let mut pool = valued(id, 1000, 240);
    pool.as_object_mut()
        .expect("a pool object")
        .shift_remove("scope");
    pool
}

/// `schedule.json` with these rules and pools, naming `requires`.
fn schedule(rules: Vec<Value>, pools: Vec<Value>, requires: &[&str]) -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": rules,
        "pools": pools,
        "activity": [],
        "timeline": [],
        "held": []
    })
}

/// Composes `schedule` for one instance of the game, whose registry holds a package per rule and
/// `rule.maint`, a return-home maintenance package; no Runtime manifest checks `requires`.
fn compose(schedule: &Value) -> Result<StandardPackComposition, crate::StandardPackComposeFailure> {
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
    let schedule = serde_json::to_vec(schedule).expect("schedule bytes");
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
