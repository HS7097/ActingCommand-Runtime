// SPDX-License-Identifier: AGPL-3.0-only

// SS-2h (register §1.289; Alice's ruling 138 of 11-07, her answers 172 A and 173 of 11-09 16:39 and
// her approval of the two water lines at 17:37, 17:42 and 17:48; design
// `rd-1031/design-c424-redline-1109.md` §1.6 and §4.1 tests 1 to 17; coordinator work order
// `rd-1109/brief-ss2h.md`; contracts/scheduling/README.md, "Pool lines", "Capped spending order",
// "Net benefit within a tier", "Within tolerance"):
// - below the cap a consumption costs its full weight `W·c` (SS-2b's linear discount retired), and
//   a judged candidate never leaves a pool below its hard line (`would_cross_hard_line`);
// - at or above the cap the pool is in a capped episode: each round lists the consumers that can
//   be dispatched now by their value (never by their cost), freezes them by `V ÷ c`, and spends
//   the pool down to its stop line one dispatch at a time, the last one allowed past the stop line
//   but never below the hard line; later rounds take repeatable tasks only;
// - the head of the order competes with the instance's other candidates by its tier and
//   `B = V − W·(c − a)`, `a = min(c, max(0, r − S))`; the pool's other consumers wait;
// - the memory of the episodes is the Runtime's, in memory only;
// - a catalog without valuations, lines and `benefit_limited` decides and hashes as before;
// - fix 1 (coordinator brief `rd-1109/brief-ss2h-fix1.md`, the review of PR #1546): a member a gate
//   of its own refuses is passed for its round and the next one tried, a round with nothing left
//   that can be dispatched ends (tests 5 and 18), and lines without a valuation warn (test 19).
//
// The catalog is neutral and V2: tasks on one instance, the capped pool `pool.neutral` read from
// `resource.neutral` with its capacity read from `resource.neutral.capacity` (180 unless a test
// says otherwise), valued 1000 milli a unit, no regeneration; its stop line the capacity less 40
// (140) and its hard line the capacity less 76 (104) unless a test says otherwise. Each consumer
// produces `pool.out` at 1000 milli a unit, so its value `V` without the pool's cost is its
// production in milli, and it is due while the pool holds its consumption. A run is simulated:
// the pool is read again half a minute after the dispatch, and the next round is a minute after it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CandidateEligibility, CappedMemory, CatalogCompileFailure, CatalogDiagnosticCode,
    CatalogDocumentSource, CatalogSources, ChainHandoverInputs, CompiledCatalog, DecisionReason,
    DiagnosticSeverity, DispatchIntent, EvaluationFacts, EvaluationResources, EvaluationTime,
    FactRefreshInputs, FactRefreshSettings, FactValue, HostResourceSnapshot, InstanceHold,
    InstanceHolds, NoProgressGuards, ObservedFact, OwedReruns, PolicyCode, PolicyEvaluation,
    PolicyEvaluationError, RemedyPlan, RunSlots, SCHEDULING_SCHEMA_VERSION,
    SCHEDULING_SCHEMA_VERSION_V2, SchedulingDecisionState, ScopeSelector, StandardPackBinding,
    StandardPackComposeInput, StandardPackComposition, StandardPackGameFiles, TaskDecision,
    TaskRuntimeSnapshot, TaskTerminalState, check_resource_targets, compile_catalog,
    compose_standard_pack_rules, evaluate_with_capped_spending, evaluate_with_no_progress,
    parse_resource_targets, project_fact_pools,
};
use actingcommand_contract::outcome::{Category, Outcome};
use actingcommand_contract::{
    BundleIndex, StandardPackProblem, StandardPackRootBytes, read_standard_pack_root,
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
/// A produced pool with a shortfall weight, for test 9's goal.
const GOAL: &str = "pool.goal";
/// Facts a trigger reads in test 5: runs left, and a precondition.
const LEFT_KEY: &str = "resource.neutral.left";
const UNLOCKED_KEY: &str = "resource.neutral.unlocked";
/// The Runtime's deferrals the fixture admission answers: an instance another holder holds (a
/// busy instance), a blackout, and a used-up daily count.
const INSTANCE_HELD: &str = "dispatch_instance_held";
const BLACKOUT: &str = "blackout_active";
const BUDGET: &str = "policy_budget_exhausted";
/// The neutral example catalog's golden identities, computed before SS-2h (as in `gate_386_cf4`
/// and `compiler::tests`).
const V1_EXAMPLE_HASH: &str =
    "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c";
const V2_EXAMPLE_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";

// Test 1, the fix below the cap (fails before: SS-2b's discount dispatched it). A regenerating pool
// of capacity 240 read at 190, without lines; the judged task consumes 20 at W = 1000 (Q = 1) and
// produces 5000 milli. SS-2b's linear discount (reserve line 0) cost 1000 × 20 × 50/240 = 4166.67
// milli, B = 833, dispatched. Now the cost is the full 20 000 milli, B = −15 000:
// `policy_benefit_nonpositive`. The same pool with both lines (constants 140 and 104) read at 160,
// above its stop line and below its cap, costs the full 20 000 as well.
#[test]
fn ss2h_t1_below_the_cap_consumption_costs_its_full_weight() {
    for (case, lines, amount) in [
        ("no lines", None, 190),
        ("both lines", Some((json!(140), json!(104))), 160),
    ] {
        let mut stamina = json!({
            "id": "pool.stamina",
            "scope": {"kind": "instance", "instance_id": A},
            "capacity": 240,
            "projection": {"amount": 1, "per_ms": 6 * MINUTE_MS},
            "observation": {"kind": "fact", "fact_key": "resource.stamina"},
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
            "valuation": valuation(1000)
        });
        if let Some((stop, hard)) = lines {
            stamina["stop_line"] = stop;
            stamina["hard_line"] = hard;
        }
        let catalog = catalog(
            vec![with(
                judged("task_spend", 0, 5),
                "consumes",
                json!([consume("pool.stamina", 20)]),
            )],
            json!([out_pool(), stamina]),
        );
        let mut world = World::new(catalog, 0);
        world
            .extra
            .insert("resource.stamina".to_owned(), i64::from(amount));
        let evaluation = world.evaluate();
        assert!(
            dispatched(&evaluation).is_empty(),
            "{case}: {evaluation:#?}"
        );
        let held = decision(&evaluation, "task_spend");
        assert_eq!(held.state, SchedulingDecisionState::Deferred, "{case}");
        assert!(
            last(held).outcome.is(PolicyCode::PolicyBenefitNonpositive),
            "{case}: {held:#?}"
        );
        assert_eq!(
            last(held).outcome.value("benefit_milli"),
            Some(&json!(-15_000)),
            "{case}"
        );
        assert!(
            last(held)
                .detail
                .contains(" items=pool.out:+5000,pool.stamina:-20000"),
            "{case}: {}",
            last(held).detail
        );
    }
}

// Test 2, one capped round (fails before: no capped state existed). The pool reads 200 against
// its cap of 180. Three repeatable consumers A, B and C, by `V ÷ c` 1000, 800 and 500, consume 30,
// 25 and 10: 200 → 170 → 145 → 135. C takes the reading past the stop line (140) to 135, not
// below the hard line (104), so it runs. They are dispatched in this order, one a round, each as
// the head (`capped_spend`, ranks 1 to 3) while the others wait (`capped_not_head`); at 135 the
// episode ends and nothing more is dispatched.
#[test]
fn ss2h_t2_a_capped_round_spends_the_pool_down_to_its_stop_line() {
    let mut world = World::new(
        catalog(
            vec![
                judged("task_a", 30, 30),
                judged("task_b", 25, 20),
                judged("task_c", 10, 5),
            ],
            pools(true),
        ),
        200,
    );
    for (rank, (task_id, reading)) in [("task_a", 200), ("task_b", 170), ("task_c", 145)]
        .into_iter()
        .enumerate()
    {
        let evaluation = world.evaluate();
        assert_eq!(dispatched(&evaluation), [task_id], "{evaluation:#?}");
        let spend = spend(&evaluation, task_id);
        assert_eq!(spend.value("capped_rank"), Some(&json!(rank + 1)));
        assert_eq!(spend.value("capped_episode"), Some(&json!(1)));
        assert_eq!(spend.value("capped_round"), Some(&json!(1)));
        assert_eq!(spend.value("pool_reading"), Some(&json!(reading)));
        assert_eq!(spend.value("pool_capacity"), Some(&json!(180)));
        assert_eq!(spend.value("stop_line"), Some(&json!(140)));
        assert_eq!(spend.value("hard_line"), Some(&json!(104)));
        for other in ["task_a", "task_b", "task_c"] {
            if other != task_id {
                assert_eq!(
                    hold(decision(&evaluation, other)).as_deref(),
                    Some("capped_not_head"),
                    "{other} while {task_id} runs: {evaluation:#?}"
                );
            }
        }
        let consumed = [("task_a", 30), ("task_b", 25), ("task_c", 10)]
            .into_iter()
            .find(|(id, _)| *id == task_id)
            .map_or(0, |(_, consumed)| consumed);
        world.run(&evaluation, consumed, TaskTerminalState::Succeeded);
    }
    assert_eq!(world.reading, 135);
    let after = world.evaluate();
    assert!(dispatched(&after).is_empty(), "{after:#?}");
    assert_eq!(world.memory.episode(A, POOL), None, "the episode ended");
}

// Test 3, a skip for the hard line. At the cap (180) the order is X (`V ÷ c` 1000, 30), D (900, 50)
// and E (800, 20). The plan takes X (180 → 150); D would leave 100, below the hard line (104), so it
// is skipped (`capped_would_cross_hard_line`, `amount_after` 100) and E is tried: 150 → 130. With
// the reading at 150, D ranks first of what is left but E is dispatched; at 130 the episode ends.
#[test]
fn ss2h_t3_a_task_that_would_cross_the_hard_line_is_skipped() {
    let mut world = World::new(
        catalog(
            vec![
                judged("task_x", 30, 30),
                judged("task_d", 50, 45),
                judged("task_e", 20, 16),
            ],
            pools(true),
        ),
        180,
    );
    let first = world.evaluate();
    assert_eq!(dispatched(&first), ["task_x"], "{first:#?}");
    world.run(&first, 30, TaskTerminalState::Succeeded);
    let second = world.evaluate();
    assert_eq!(dispatched(&second), ["task_e"], "{second:#?}");
    let skipped = decision(&second, "task_d");
    assert_eq!(skipped.state, SchedulingDecisionState::Deferred);
    assert_eq!(
        hold(skipped).as_deref(),
        Some("capped_would_cross_hard_line"),
        "{skipped:#?}"
    );
    assert_eq!(
        last(skipped).outcome.value("amount_after"),
        Some(&json!(100))
    );
    assert_eq!(
        last(skipped).outcome.value("pool_reading"),
        Some(&json!(150))
    );
    world.run(&second, 20, TaskTerminalState::Succeeded);
    assert_eq!(world.reading, 130);
    assert!(dispatched(&world.evaluate()).is_empty());
    assert_eq!(world.memory.episode(A, POOL), None);
}

// Test 4, the cost does not filter before the order. At the cap, a consumer whose value without
// the pool's cost is 10 000 milli (above its bound) but whose full-price B is 10 000 − 30 000 =
// −20 000 enters the order and is dispatched; as the head its B charges only the part below the
// stop line: a = min(30, 180 − 140) = 30, B = 10 000. Below the cap (179) the same candidate is
// `policy_benefit_nonpositive` at −20 000.
#[test]
fn ss2h_t4_the_cost_never_keeps_a_consumer_out_of_the_order() {
    let catalog = || catalog(vec![judged("task_f", 30, 10)], pools(true));
    let mut capped = World::new(catalog(), 180);
    let evaluation = capped.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_f"], "{evaluation:#?}");
    spend(&evaluation, "task_f");
    let breakdown = reason(decision(&evaluation, "task_f"), PolicyCode::RankBreakdown);
    assert_eq!(
        breakdown.outcome.value("benefit_milli"),
        Some(&json!(10_000))
    );

    let mut below = World::new(catalog(), 179);
    let evaluation = below.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, "task_f");
    assert!(
        last(held).outcome.is(PolicyCode::PolicyBenefitNonpositive),
        "{held:#?}"
    );
    assert_eq!(
        last(held).outcome.value("benefit_milli"),
        Some(&json!(-20_000))
    );
}

// Test 5, only what can be dispatched now. At the cap, of four consumers each consuming 20:
// - K (no run left, `resource.neutral.left` 0) and P (its precondition `resource.neutral.unlocked`
//   0 unmet) are no candidates: they do not enter the order and say so (`capped_not_dispatchable`,
//   `gate_code` `trigger_false`);
// - the instance is busy (the cycle starts with it held, as the Runtime says in
//   `FactRefreshInputs::instance_holds`, and the admission defers every candidate,
//   `dispatch_instance_held`): nothing runs, nothing is passed, the round stays and its candidates
//   keep the admission's reason;
// - then Bk is in a blackout of its own (`blackout_active`): it keeps that reason, yields its turn
//   and G, next in the order, is dispatched; Bk is passed for the round (fix 1);
// - once the run is left, the precondition met and the blackout over, the round has nothing left
//   and ends: the next round takes all four, K (the highest `V ÷ c`) first, while P, Bk and G wait
//   for the head (`capped_not_head`); at 140 the episode ends (below the cap K, worth it at full
//   price, is dispatched as any candidate is), and at the next episode's first round K is the head.
// Adapted by fix 1: before it, the blacked-out Bk stayed in its round and ran next, ahead of K.
#[test]
fn ss2h_t5_only_consumers_that_can_be_dispatched_now_enter_the_order() {
    let gated = |id: &str, key: &str, produced: u64| {
        with(
            judged(id, 20, produced),
            "trigger",
            json!({"kind": "all", "predicates": [enough(20), at_least(key, 1)]}),
        )
    };
    let mut world = World::new(
        catalog(
            vec![
                judged("task_g", 20, 20),
                gated("task_k", LEFT_KEY, 30),
                gated("task_p", UNLOCKED_KEY, 28),
                judged("task_bk", 20, 26),
            ],
            pools(true),
        ),
        180,
    );
    world.extra.insert(LEFT_KEY.to_owned(), 0);
    world.extra.insert(UNLOCKED_KEY.to_owned(), 0);
    let not_dispatchable = |evaluation: &PolicyEvaluation| {
        for task_id in ["task_k", "task_p"] {
            let held = decision(evaluation, task_id);
            assert_eq!(
                hold(held).as_deref(),
                Some("capped_not_dispatchable"),
                "{task_id}: {held:#?}"
            );
            assert_eq!(
                last(held).outcome.value("gate_code"),
                Some(&json!("trigger_false")),
                "{task_id}"
            );
        }
    };

    let (busy, asked) = world.evaluate_busy();
    assert!(dispatched(&busy).is_empty(), "{busy:#?}");
    assert_eq!(
        asked,
        ["task_bk", "task_g"],
        "the order, Bk (1300) before G (1000)"
    );
    for task_id in ["task_bk", "task_g"] {
        assert_eq!(
            last(decision(&busy, task_id)).outcome.code().as_str(),
            INSTANCE_HELD,
            "{task_id}"
        );
    }
    not_dispatchable(&busy);
    assert_eq!(
        world.memory.episode(A, POOL),
        Some((1, 1)),
        "the round stays"
    );

    let (blackout, asked) = world.evaluate_with(|intent| {
        if intent.task_id == "task_bk" {
            deferred(BLACKOUT)
        } else {
            CandidateEligibility::Eligible
        }
    });
    assert_eq!(asked, ["task_bk", "task_g"]);
    assert_eq!(dispatched(&blackout), ["task_g"], "{blackout:#?}");
    assert_eq!(
        spend(&blackout, "task_g").value("capped_rank"),
        Some(&json!(2))
    );
    assert_eq!(
        last(decision(&blackout, "task_bk")).outcome.code().as_str(),
        BLACKOUT
    );
    not_dispatchable(&blackout);
    world.run(&blackout, 20, TaskTerminalState::Succeeded);

    world.extra.insert(LEFT_KEY.to_owned(), 1);
    world.extra.insert(UNLOCKED_KEY.to_owned(), 1);
    let recovered = world.evaluate();
    assert_eq!(dispatched(&recovered), ["task_k"], "{recovered:#?}");
    let head = spend(&recovered, "task_k");
    assert_eq!(
        head.value("capped_round"),
        Some(&json!(2)),
        "the next round"
    );
    assert_eq!(head.value("capped_rank"), Some(&json!(1)));
    for task_id in ["task_p", "task_bk", "task_g"] {
        assert_eq!(
            hold(decision(&recovered, task_id)).as_deref(),
            Some("capped_not_head"),
            "{task_id} waits for the head: {recovered:#?}"
        );
    }
    world.run(&recovered, 20, TaskTerminalState::Succeeded);
    assert_eq!(world.reading, 140);
    let below = world.evaluate();
    assert_eq!(world.memory.episode(A, POOL), None);
    assert_eq!(dispatched(&below), ["task_k"], "{below:#?}");
    assert!(
        !decision(&below, "task_k")
            .reasons
            .iter()
            .any(|reason| reason.outcome.is(PolicyCode::CappedSpend)),
        "below the cap no capped order"
    );
    world.run(&below, 20, TaskTerminalState::Succeeded);

    world.set_reading(180);
    let next = world.evaluate();
    assert_eq!(dispatched(&next), ["task_k"], "{next:#?}");
    let spend = spend(&next, "task_k");
    assert_eq!(spend.value("capped_episode"), Some(&json!(2)));
    assert_eq!(spend.value("capped_rank"), Some(&json!(1)));
}

// Test 6, the cap triggers. Read at 170, below the cap, the consumer of test 4 is judged at full
// price and not dispatched, and no episode begins; read at 180, exactly the cap, it is dispatched as
// the head of episode 1.
#[test]
fn ss2h_t6_the_capped_state_begins_at_the_cap() {
    let catalog = || catalog(vec![judged("task_f", 30, 10)], pools(true));
    let mut below = World::new(catalog(), 170);
    let evaluation = below.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    assert!(
        last(decision(&evaluation, "task_f"))
            .outcome
            .is(PolicyCode::PolicyBenefitNonpositive)
    );
    assert_eq!(below.memory.episode(A, POOL), None);

    let mut at = World::new(catalog(), 180);
    let evaluation = at.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_f"], "{evaluation:#?}");
    assert_eq!(
        spend(&evaluation, "task_f").value("capped_episode"),
        Some(&json!(1))
    );
    assert_eq!(at.memory.episode(A, POOL), Some((1, 1)));
}

// Test 7, already above the cap at the start (an item converted into the pool): read at 500. L is
// limited (`benefit_limited`, 100 a run, `V ÷ c` 1500), R1 and R2 repeatable (60 at 1200, 30 at
// 1000). Round 1 takes all three: 500 → 400 → 340 → 310. Rounds 2 and 3 take R1 and R2 only: 310
// → 250 → 220, then 220 → 160 → 130. Three rounds, seven dispatches; at 130 the episode ends, and
// the two that would pass at full price would leave the pool below its hard line
// (`would_cross_hard_line`).
#[test]
fn ss2h_t7_from_above_the_cap_the_rounds_spend_down_to_the_stop_line() {
    let mut world = World::new(
        catalog(
            vec![
                limited(judged("task_l", 100, 150)),
                judged("task_r1", 60, 72),
                judged("task_r2", 30, 30),
            ],
            pools(true),
        ),
        500,
    );
    let consumed = |task_id: &str| match task_id {
        "task_l" => 100,
        "task_r1" => 60,
        _ => 30,
    };
    let mut sequence = Vec::new();
    let mut rounds = Vec::new();
    for _ in 0..7 {
        let evaluation = world.evaluate();
        let task_id = dispatched(&evaluation)
            .first()
            .map(|task_id| (*task_id).to_owned())
            .unwrap_or_else(|| panic!("a dispatch: {evaluation:#?}"));
        let round = spend(&evaluation, &task_id).value("capped_round").cloned();
        rounds.push(round);
        world.run(
            &evaluation,
            consumed(&task_id),
            TaskTerminalState::Succeeded,
        );
        sequence.push(task_id);
    }
    assert_eq!(
        sequence,
        [
            "task_l", "task_r1", "task_r2", "task_r1", "task_r2", "task_r1", "task_r2"
        ]
    );
    assert_eq!(
        rounds,
        [1, 1, 1, 2, 2, 3, 3].map(|round| Some(json!(round))),
        "three rounds"
    );
    assert_eq!(world.reading, 130);
    let after = world.evaluate();
    assert!(dispatched(&after).is_empty(), "{after:#?}");
    assert_eq!(world.memory.episode(A, POOL), None);
    for (task_id, after_run) in [("task_l", 30), ("task_r1", 70)] {
        let held = decision(&after, task_id);
        assert_eq!(
            hold(held).as_deref(),
            Some("would_cross_hard_line"),
            "{task_id}: {held:#?}"
        );
        assert_eq!(
            last(held).outcome.value("amount_after"),
            Some(&json!(after_run))
        );
    }
}

// Test 8, from the second round on a limited task waits (`capped_limited_spent`), and the next
// episode's first round takes it again. Read at 240 (cap 180): round 1 is L (limited, 30, 1500)
// then R (30, 1000), 240 → 210 → 180; round 2 is R alone (180 → 150) while L is held; round 3 R
// again (150 → 120). Read at 200 again later, episode 2's first round dispatches L first. The
// standard-pack composer writes `benefit_limited: true` from the rule member `limited: true`.
#[test]
fn ss2h_t8_from_the_second_round_limited_tasks_wait_for_the_next_episode() {
    let mut world = World::new(
        catalog(
            vec![limited(judged("task_l", 30, 45)), judged("task_r", 30, 30)],
            pools(true),
        ),
        240,
    );
    for task_id in ["task_l", "task_r"] {
        let evaluation = world.evaluate();
        assert_eq!(dispatched(&evaluation), [task_id], "{evaluation:#?}");
        world.run(&evaluation, 30, TaskTerminalState::Succeeded);
    }
    for round in [2, 3] {
        let evaluation = world.evaluate();
        assert_eq!(dispatched(&evaluation), ["task_r"], "{evaluation:#?}");
        assert_eq!(
            spend(&evaluation, "task_r").value("capped_round"),
            Some(&json!(round))
        );
        let held = decision(&evaluation, "task_l");
        assert_eq!(
            hold(held).as_deref(),
            Some("capped_limited_spent"),
            "round {round}: {held:#?}"
        );
        world.run(&evaluation, 30, TaskTerminalState::Succeeded);
    }
    assert_eq!(world.reading, 120);
    assert!(dispatched(&world.evaluate()).is_empty());

    world.set_reading(200);
    let next = world.evaluate();
    assert_eq!(dispatched(&next), ["task_l"], "{next:#?}");
    let spend = spend(&next, "task_l");
    assert_eq!(spend.value("capped_episode"), Some(&json!(2)));
    assert_eq!(spend.value("capped_round"), Some(&json!(1)));

    // The composer's mark.
    let mut marked = schedule_rule("rule.limited");
    marked["limited"] = json!(true);
    let composition = compose(
        vec![marked, schedule_rule("rule.plain")],
        Vec::new(),
        Vec::new(),
    )
    .unwrap_or_else(|failure| panic!("the schedule composes: {failure}"));
    let marks = composition
        .compiled()
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| (task.id.clone(), task.benefit_limited))
        .collect::<Vec<_>>();
    assert_eq!(
        marks,
        [
            ("rule.limited".to_owned(), Some(true)),
            ("rule.plain".to_owned(), None)
        ]
    );
    let mut written = schedule_rule("rule.limited");
    written["benefit_limited"] = json!(true);
    let failure = compose(vec![written], Vec::new(), Vec::new()).expect_err("the composer's");
    assert_eq!(
        failure.issues()[0].issue.pointer,
        "/rules/0/benefit_limited"
    );
}

// Test 9, the order is frozen for the round. Read at 200: A (30, `V ÷ c` 1000) before B (25, 640).
// The instance is busy at the round's start (held as the cycle starts, as in test 5; fix 1: so the
// round waits and passes no one). Then a goal on B's product raises B's value far above A's
// (`V ÷ c` above 4000): the round keeps its order and dispatches A, then B; the next round is
// sorted again and dispatches B first.
#[test]
fn ss2h_t9_the_order_is_frozen_for_the_round() {
    let catalog = catalog(
        vec![
            judged("task_a", 30, 30),
            with(
                judged("task_b", 25, 15),
                "produces",
                json!([produce(OUT, 15), produce(GOAL, 1)]),
            ),
        ],
        json!([out_pool(), goal_pool(), capped_pool(true)]),
    );
    let mut world = World::new(catalog, 200);
    let (busy, asked) = world.evaluate_busy();
    assert!(dispatched(&busy).is_empty(), "{busy:#?}");
    assert_eq!(asked, ["task_a", "task_b"], "frozen: A first");

    world.extra.insert("resource.goal".to_owned(), 0);
    let goal = stored_goal(&world.catalog);
    world.records.push(goal);
    let frozen = world.evaluate();
    assert_eq!(dispatched(&frozen), ["task_a"], "{frozen:#?}");
    world.run(&frozen, 30, TaskTerminalState::Succeeded);
    let still = world.evaluate();
    assert_eq!(dispatched(&still), ["task_b"], "{still:#?}");
    world.run(&still, 25, TaskTerminalState::Succeeded);
    assert_eq!(world.reading, 145);

    let sorted = world.evaluate();
    assert_eq!(dispatched(&sorted), ["task_b"], "re-sorted: {sorted:#?}");
    let spend = spend(&sorted, "task_b");
    assert_eq!(spend.value("capped_round"), Some(&json!(2)));
    assert_eq!(spend.value("capped_rank"), Some(&json!(1)));
}

// Test 10, the reading before each dispatch. Read at 200, the plan is A (30) to 170 then B (50) to
// 120. A ran and the pool reads 150, lower than planned: B would leave 100, below the hard line, so
// the round ends and round 2 dispatches A (150 → 120). That run fails and consumes nothing: the next
// round checks the reading by the same rule and dispatches A again (round 3).
#[test]
fn ss2h_t10_the_reading_is_checked_again_before_each_dispatch() {
    let mut world = World::new(
        catalog(
            vec![judged("task_a", 30, 30), judged("task_b", 50, 45)],
            pools(true),
        ),
        200,
    );
    let first = world.evaluate();
    assert_eq!(dispatched(&first), ["task_a"], "{first:#?}");
    world.run(&first, 50, TaskTerminalState::Succeeded);
    assert_eq!(world.reading, 150);

    let second = world.evaluate();
    assert_eq!(dispatched(&second), ["task_a"], "{second:#?}");
    assert_eq!(
        spend(&second, "task_a").value("capped_round"),
        Some(&json!(2)),
        "the round ended before B"
    );
    assert!(
        dispatched(&second)
            .iter()
            .all(|task_id| *task_id != "task_b")
    );
    world.run(&second, 0, TaskTerminalState::Failed);
    assert_eq!(world.reading, 150);

    let third = world.evaluate();
    assert_eq!(dispatched(&third), ["task_a"], "{third:#?}");
    assert_eq!(
        spend(&third, "task_a").value("capped_round"),
        Some(&json!(3))
    );
}

// Test 11, below the cap the hard line holds. Read at 120 (below the cap of 180), a consumer worth
// it at full price (B = 40 000 − 30 000 = 10 000) would leave 90 < 104: it is not dispatched
// (`would_cross_hard_line`, `amount_after` 90, `hard_line` 104). Read at 140 it would leave 110 and
// is dispatched.
#[test]
fn ss2h_t11_below_the_cap_a_run_never_crosses_the_hard_line() {
    let catalog = || catalog(vec![judged("task_w", 30, 40)], pools(true));
    let mut low = World::new(catalog(), 120);
    let evaluation = low.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, "task_w");
    assert_eq!(held.state, SchedulingDecisionState::Deferred);
    assert_eq!(hold(held).as_deref(), Some("would_cross_hard_line"));
    assert_eq!(last(held).outcome.value("amount_after"), Some(&json!(90)));
    assert_eq!(last(held).outcome.value("hard_line"), Some(&json!(104)));

    let mut enough = World::new(catalog(), 140);
    let evaluation = enough.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_w"], "{evaluation:#?}");
}

// Test 12, layout 乙: the head competes with the instance's other candidates. At 195 the order is
// H1 (30, 1000) then H2 (30, 800). A due maintenance task, of the tier before the business tier,
// runs first; H1 waits for the instance, H2 for its head (`capped_not_head`, `task_id` H1). Without
// it H1 runs as the head, B = 30 000 − 1000·(30 − min(30, 195 − 140)) = 30 000; then at 165 H2 runs,
// B = 24 000 − 1000·(30 − min(30, 165 − 140)) = 19 000.
#[test]
fn ss2h_t12_the_head_ranks_by_its_tier_and_its_benefit_above_the_stop_line() {
    let tasks = || {
        vec![
            maintenance("task_maint"),
            judged("task_h1", 30, 30),
            judged("task_h2", 30, 24),
        ]
    };
    let mut busy = World::new(catalog(tasks(), pools(true)), 195);
    let evaluation = busy.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_maint"], "{evaluation:#?}");
    assert!(
        last(decision(&evaluation, "task_h1"))
            .outcome
            .is(PolicyCode::InstanceAlreadySelected)
    );
    let waiting = decision(&evaluation, "task_h2");
    assert_eq!(hold(waiting).as_deref(), Some("capped_not_head"));
    assert_eq!(
        last(waiting).outcome.value("task_id"),
        Some(&json!("task_h1"))
    );

    let mut free = World::new(catalog(tasks(), pools(true)), 195);
    free.states.push(snapshot(
        "task_maint",
        NOW - 30_000,
        TaskTerminalState::Succeeded,
    ));
    for (task_id, benefit) in [("task_h1", 30_000), ("task_h2", 19_000)] {
        let evaluation = free.evaluate();
        assert_eq!(dispatched(&evaluation), [task_id], "{evaluation:#?}");
        let breakdown = reason(decision(&evaluation, task_id), PolicyCode::RankBreakdown);
        assert_eq!(
            breakdown.outcome.value("benefit_milli"),
            Some(&json!(benefit)),
            "{task_id}"
        );
        free.run(&evaluation, 30, TaskTerminalState::Succeeded);
    }
}

// Test 13, the lines' rules. Refused with `pool_lines_invalid` at the member: the stop line not
// above the hard line (two constants, 100 and 100; two references to the capacity, −76 and −40), a
// line above the capacity (a constant 250 of a constant 240; a reference to the capacity +10), one
// line without the other. A V1 document that names either is `unknown_field` at the member, and
// `benefit_limited` too; `benefit_limited` without `benefit_judged` is `missing_required_field`.
// Both lines compile and are carried as written; a `schedule.json` pool that writes them needs
// `pool_lines` in `requires`.
#[test]
fn ss2h_t13_the_two_lines_are_checked() {
    let stock = |stop: Option<Value>, hard: Option<Value>| {
        let mut pool = valued("pool.stock", 1000, 240);
        if let Some(stop) = stop {
            pool["stop_line"] = stop;
        }
        if let Some(hard) = hard {
            pool["hard_line"] = hard;
        }
        pool
    };
    let refused = |case: &str, pool: Value, member: &str| {
        let failure = compile_v2(
            vec![judged("task_f", 30, 10)],
            json!([out_pool(), capped_pool(true), pool]),
        )
        .expect_err(case);
        assert!(
            refused_at(
                &failure,
                CatalogDiagnosticCode::PoolLinesInvalid,
                &format!("/pools/2/{member}")
            ),
            "{case}: {:?}",
            failure.diagnostics()
        );
    };
    refused(
        "two constants, the stop line not above",
        stock(Some(json!(100)), Some(json!(100))),
        "hard_line",
    );
    refused(
        "a constant above the capacity",
        stock(Some(json!(250)), Some(json!(100))),
        "stop_line",
    );
    refused(
        "a stop line alone",
        stock(Some(json!(140)), None),
        "stop_line",
    );
    refused(
        "a hard line alone",
        stock(None, Some(json!(104))),
        "hard_line",
    );
    let mut swapped = capped_pool(false);
    swapped["stop_line"] = line(-76);
    swapped["hard_line"] = line(-40);
    let failure = compile_v2(vec![judged("task_f", 30, 10)], json!([out_pool(), swapped]))
        .expect_err("swapped references");
    assert!(
        refused_at(
            &failure,
            CatalogDiagnosticCode::PoolLinesInvalid,
            "/pools/1/hard_line"
        ),
        "{:?}",
        failure.diagnostics()
    );
    let mut above = capped_pool(false);
    above["stop_line"] = line(10);
    above["hard_line"] = line(-76);
    let failure = compile_v2(vec![judged("task_f", 30, 10)], json!([out_pool(), above]))
        .expect_err("a reference above the capacity");
    assert!(
        refused_at(
            &failure,
            CatalogDiagnosticCode::PoolLinesInvalid,
            "/pools/1/stop_line"
        ),
        "{:?}",
        failure.diagnostics()
    );

    for member in ["stop_line", "hard_line"] {
        let failure = compile_catalog(&example(SCHEDULING_SCHEMA_VERSION, |_, pools| {
            pools["pools"][0][member] = json!(10);
        }))
        .expect_err(member);
        assert!(
            refused_at(
                &failure,
                CatalogDiagnosticCode::UnknownField,
                &format!("/pools/0/{member}")
            ),
            "{member}: {:?}",
            failure.diagnostics()
        );
    }
    let failure = compile_catalog(&example(SCHEDULING_SCHEMA_VERSION, |tasks, _| {
        tasks["tasks"][0]["benefit_limited"] = json!(true);
    }))
    .expect_err("V1 benefit_limited");
    assert!(
        refused_at(
            &failure,
            CatalogDiagnosticCode::UnknownField,
            "/tasks/0/benefit_limited"
        ),
        "{:?}",
        failure.diagnostics()
    );
    let mut unjudged = task_body(
        "task_u",
        100,
        vec![consume(POOL, 10)],
        vec![produce(OUT, 1)],
    );
    unjudged["benefit_limited"] = json!(true);
    let failure = compile_v2(vec![unjudged], pools(true)).expect_err("unjudged");
    assert!(
        refused_at(
            &failure,
            CatalogDiagnosticCode::MissingRequiredField,
            "/tasks/0/benefit_judged"
        ),
        "{:?}",
        failure.diagnostics()
    );

    let compiled = catalog(
        vec![judged("task_f", 30, 10)],
        json!([
            out_pool(),
            capped_pool(true),
            stock(Some(json!(140)), Some(json!(104)))
        ]),
    );
    let written = serde_json::to_value(&compiled.catalog().pools.pools).expect("pools");
    assert_eq!(written[1]["stop_line"], line(-40));
    assert_eq!(written[1]["hard_line"], line(-76));
    assert_eq!(written[2]["stop_line"], json!(140));
    assert_eq!(written[2]["hard_line"], json!(104));
    assert!(written[0].get("stop_line").is_none(), "{}", written[0]);

    let mut lined = schedule_pool(POOL);
    lined["stop_line"] = json!(140);
    lined["hard_line"] = json!(104);
    let failure = compose(
        vec![schedule_rule("rule.plain")],
        vec![lined.clone()],
        Vec::new(),
    )
    .expect_err("requires");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleInvalid);
    assert_eq!(issue.pointer, "/pools/1/stop_line");
    compose(
        vec![schedule_rule("rule.plain")],
        vec![lined],
        vec!["pool_lines"],
    )
    .unwrap_or_else(|failure| panic!("with pool_lines: {failure}"));
}

// Test 14, a pool without lines: its stop line is its cap (180) and its hard line 0. At 180 one
// consumer runs (`capped_spend`, `stop_line` 180, `hard_line` 0), the reading falls below the cap
// and the episode ends: the next round judges it at full price and dispatches nothing.
#[test]
fn ss2h_t14_without_lines_the_stop_line_is_the_cap_and_the_hard_line_zero() {
    let mut world = World::new(catalog(vec![judged("task_f", 30, 10)], pools(false)), 180);
    let evaluation = world.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_f"], "{evaluation:#?}");
    let spend = spend(&evaluation, "task_f");
    assert_eq!(spend.value("stop_line"), Some(&json!(180)));
    assert_eq!(spend.value("hard_line"), Some(&json!(0)));
    world.run(&evaluation, 30, TaskTerminalState::Succeeded);
    let after = world.evaluate();
    assert!(dispatched(&after).is_empty(), "{after:#?}");
    assert!(
        last(decision(&after, "task_f"))
            .outcome
            .is(PolicyCode::PolicyBenefitNonpositive)
    );
    assert_eq!(world.memory.episode(A, POOL), None);
}

// Test 15, byte for byte. The neutral example catalog keeps its V1 and V2 golden identities. A V2
// catalog without valuations, lines or `benefit_limited`, its pool at its cap: the Runtime's entry
// with its memory and the plain entry decide byte for byte alike, round after round, the memory
// stays empty and nothing names a line. Lines written without a valuation change no decision.
#[test]
fn ss2h_t15_without_valuations_lines_or_limits_nothing_changes() {
    for (version, golden) in [
        (SCHEDULING_SCHEMA_VERSION, V1_EXAMPLE_HASH),
        (SCHEDULING_SCHEMA_VERSION_V2, V2_EXAMPLE_HASH),
    ] {
        let example = compile_catalog(&example(version, |_, _| {}))
            .unwrap_or_else(|failure| panic!("{version}: {:?}", failure.diagnostics()));
        assert_eq!(example.catalog_hash(), golden, "{version}");
    }
    let tasks = || {
        vec![
            judged("task_a", 30, 30),
            judged("task_b", 25, 20),
            maintenance("task_maint"),
        ]
    };
    let plain = catalog(tasks(), bare(pools(false)));
    let pools_text = serde_json::to_string(&plain.catalog().pools.pools).expect("pools");
    assert!(!pools_text.contains("_line"), "{pools_text}");
    let mut world = World::new(plain, 200);
    for _ in 0..3 {
        let with_memory = world.evaluate();
        let without = world.evaluate_plain();
        assert_eq!(
            serde_json::to_vec(&with_memory).expect("bytes"),
            serde_json::to_vec(&without).expect("bytes")
        );
        assert!(world.memory.is_empty());
        let text = serde_json::to_string(&with_memory).expect("text");
        for absent in ["capped_spend", "pool_line_held", "stop_line", "hard_line"] {
            assert!(!text.contains(absent), "{absent}: {text}");
        }
        assert_eq!(dispatched(&with_memory), ["task_maint"], "{with_memory:#?}");
        world.run(&with_memory, 0, TaskTerminalState::Succeeded);
        world.states.retain(|state| state.task_id != "task_maint");
    }

    let lined = catalog(tasks(), bare(pools(true)));
    let mut before = World::new(catalog(tasks(), bare(pools(false))), 200);
    let mut after = World::new(lined, 200);
    assert_eq!(
        serde_json::to_vec(&before.evaluate().decisions).expect("decisions"),
        serde_json::to_vec(&after.evaluate().decisions).expect("decisions"),
        "lines without a valuation change no decision"
    );
}

// Test 16, a restart. A ran at 200 and the pool reads 170. The Runtime's memory keeps the episode,
// so B runs next; after a restart (an empty memory) 170 is below the cap and nothing runs; read at
// 190, a new episode begins, counted from 1 again. A new catalog generation ends an episode too.
#[test]
fn ss2h_t16_a_restart_forgets_the_episode() {
    let tasks = || vec![judged("task_a", 30, 30), judged("task_b", 25, 20)];
    let mut world = World::new(catalog(tasks(), pools(true)), 200);
    let first = world.evaluate();
    assert_eq!(dispatched(&first), ["task_a"], "{first:#?}");
    world.run(&first, 30, TaskTerminalState::Succeeded);

    let mut kept = world.clone_world();
    assert_eq!(dispatched(&kept.evaluate()), ["task_b"]);

    let mut restarted = world.clone_world();
    restarted.memory = CappedMemory::default();
    assert!(dispatched(&restarted.evaluate()).is_empty());
    assert!(restarted.memory.is_empty());
    restarted.set_reading(190);
    let again = restarted.evaluate();
    assert_eq!(dispatched(&again).len(), 1, "{again:#?}");
    let task_id = dispatched(&again)[0].to_owned();
    assert_eq!(
        spend(&again, &task_id).value("capped_episode"),
        Some(&json!(1))
    );

    let mut generation = world.clone_world();
    let mut renamed = tasks();
    renamed.push(judged("task_c", 10, 5));
    generation.catalog = catalog(renamed, pools(true));
    assert!(dispatched(&generation.evaluate()).is_empty());
    assert_eq!(generation.memory.episode(A, POOL), None);
}

// Test 17, SS-2d's bound reads the whole consumption. A regenerating pool of 240 read at 200, below
// its cap, 20 units at W = 1000 consumed and one unit produced: ε = 1000/2000 + 20 000/2000 = 21/2,
// with or without lines on the pool. Produced at 20 005, S = 5 is within ε:
// `benefit_within_tolerance` (⌊S⌋ 5, ⌈ε⌉ 11, unmarked); at 20 011, S = 11 is above it, dispatched
// with B = 11. The design's appendix A5 case 1 still holds: two thousandths of a unit at W = 2
// against one unit at W = 6100 per 10^6: S = −21/10000 within ε = 4501/2000000.
#[test]
fn ss2h_t17_the_rounding_bound_reads_the_whole_consumption() {
    for lined in [false, true] {
        let stamina = || {
            let mut pool = json!({
                "id": "pool.stamina",
                "scope": {"kind": "instance", "instance_id": A},
                "capacity": 240,
                "projection": {"amount": 1, "per_ms": 6 * MINUTE_MS},
                "observation": {"kind": "fact", "fact_key": "resource.stamina"},
                "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
                "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
                "valuation": valuation(1000)
            });
            if lined {
                pool["stop_line"] = json!(140);
                pool["hard_line"] = json!(104);
            }
            pool
        };
        let round = |out_weight: u32| {
            let catalog = catalog(
                vec![with(
                    judged("task_t", 0, 1),
                    "consumes",
                    json!([consume("pool.stamina", 20)]),
                )],
                json!([valued(OUT, out_weight, 1_000_000), stamina()]),
            );
            let mut world = World::new(catalog, 0);
            world.extra.insert("resource.stamina".to_owned(), 200);
            world.evaluate()
        };
        let within = round(20_005);
        assert!(dispatched(&within).is_empty(), "{lined}: {within:#?}");
        let held = last(decision(&within, "task_t"));
        assert!(
            held.outcome.is(PolicyCode::BenefitWithinTolerance),
            "{lined}: {held:#?}"
        );
        assert_eq!(held.outcome.value("benefit_milli"), Some(&json!(5)));
        assert_eq!(held.outcome.value("tolerance_milli"), Some(&json!(11)));
        assert_eq!(held.outcome.value("zero_tier"), Some(&json!("unmarked")));
        assert!(
            held.detail.contains(" sum=5/1 tolerance=21/2 "),
            "{lined}: {}",
            held.detail
        );
        let above = round(20_011);
        assert_eq!(dispatched(&above), ["task_t"], "{lined}: {above:#?}");
        let breakdown = reason(decision(&above, "task_t"), PolicyCode::RankBreakdown);
        assert_eq!(breakdown.outcome.value("benefit_milli"), Some(&json!(11)));
    }

    let catalog = catalog(
        vec![with(
            with(
                judged("task_t", 0, 0),
                "produces",
                json!([{
                    "pool_id": "pool.trace",
                    "direction": "produce",
                    "expected_amount_milli": 2,
                    "observation_source": "self_reported",
                    "confidence_milli": 1000
                }]),
            ),
            "consumes",
            json!([consume("pool.dust", 1)]),
        )],
        json!([
            valued_scaled("pool.trace", 2, 1),
            valued_scaled("pool.dust", 6100, 1_000_000)
        ]),
    );
    let mut world = World::new(catalog, 0);
    let evaluation = world.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = last(decision(&evaluation, "task_t"));
    assert!(
        held.outcome.is(PolicyCode::BenefitWithinTolerance),
        "{held:#?}"
    );
    assert!(
        held.detail
            .contains(" sum=-21/10000 tolerance=4501/2000000 "),
        "{}",
        held.detail
    );
}

// Test 18 (fix 1, the review's finding 2; fails before: the round held the pool at its cap). A
// consumer that cannot be dispatched never holds its round.
// - Read at 200, the order is L (limited, 20, `V ÷ c` 1500), M1 (20, 1300) and M2 (20, 1200), all
//   planned (200 → 180 → 160 → 140). L's daily count is used up: the admission refuses it
//   (`policy_budget_exhausted`) whenever it is asked. L keeps that reason, is passed for the round
//   and M1, next in the order, is dispatched; then M2, L held `capped_not_dispatchable` with that
//   `gate_code` and not asked again. With nothing left, the round ends; still above the stop line,
//   the next round, without the limited L (`capped_limited_spent`), dispatches M1 at 160, and at
//   140 the episode ends.
// - Read at 200, X (20, 1500), Y (20, 1300) and Z (20, 1200, due only while `resource.neutral.left`
//   is at least 1) are all planned. X and Y run; Z's trigger then turns false: no planned one left
//   can be dispatched while X and Y can, so the round ends and round 2 dispatches X at 160, Z
//   saying why it is left out (`capped_not_dispatchable`, `trigger_false`).
#[test]
fn ss2h_t18_a_consumer_that_cannot_be_dispatched_never_holds_its_round() {
    let mut world = World::new(
        catalog(
            vec![
                limited(judged("task_l", 20, 30)),
                judged("task_m1", 20, 26),
                judged("task_m2", 20, 24),
            ],
            pools(true),
        ),
        200,
    );
    let admission = |intent: &DispatchIntent| {
        if intent.task_id == "task_l" {
            deferred(BUDGET)
        } else {
            CandidateEligibility::Eligible
        }
    };
    let (first, asked) = world.evaluate_with(admission);
    assert_eq!(
        asked,
        ["task_l", "task_m1"],
        "L, then the next of the order"
    );
    assert_eq!(dispatched(&first), ["task_m1"], "{first:#?}");
    assert_eq!(
        last(decision(&first, "task_l")).outcome.code().as_str(),
        BUDGET,
        "L keeps the admission's reason"
    );
    assert_eq!(
        spend(&first, "task_m1").value("capped_rank"),
        Some(&json!(2))
    );
    world.run(&first, 20, TaskTerminalState::Succeeded);

    let (second, asked) = world.evaluate_with(admission);
    assert_eq!(asked, ["task_m2"], "L is passed for the round: {second:#?}");
    assert_eq!(dispatched(&second), ["task_m2"]);
    assert_eq!(
        spend(&second, "task_m2").value("capped_round"),
        Some(&json!(1))
    );
    let passed = decision(&second, "task_l");
    assert_eq!(
        hold(passed).as_deref(),
        Some("capped_not_dispatchable"),
        "{passed:#?}"
    );
    assert_eq!(
        last(passed).outcome.value("gate_code"),
        Some(&json!(BUDGET))
    );
    world.run(&second, 20, TaskTerminalState::Succeeded);
    assert_eq!(world.reading, 160);

    let (third, _) = world.evaluate_with(admission);
    assert_eq!(
        dispatched(&third),
        ["task_m1"],
        "the round ended and the pool is not held at its cap: {third:#?}"
    );
    let next = spend(&third, "task_m1");
    assert_eq!(next.value("capped_round"), Some(&json!(2)));
    assert_eq!(next.value("capped_rank"), Some(&json!(1)));
    assert_eq!(
        hold(decision(&third, "task_l")).as_deref(),
        Some("capped_limited_spent")
    );
    world.run(&third, 20, TaskTerminalState::Succeeded);
    assert_eq!(world.reading, 140);
    let (spent, _) = world.evaluate_with(admission);
    assert_eq!(
        world.memory.episode(A, POOL),
        None,
        "spent to the stop line: {spent:#?}"
    );

    let gated = with(
        judged("task_z", 20, 24),
        "trigger",
        json!({"kind": "all", "predicates": [enough(20), at_least(LEFT_KEY, 1)]}),
    );
    let mut world = World::new(
        catalog(
            vec![judged("task_x", 20, 30), judged("task_y", 20, 26), gated],
            pools(true),
        ),
        200,
    );
    world.extra.insert(LEFT_KEY.to_owned(), 1);
    for task_id in ["task_x", "task_y"] {
        let evaluation = world.evaluate();
        assert_eq!(dispatched(&evaluation), [task_id], "{evaluation:#?}");
        world.run(&evaluation, 20, TaskTerminalState::Succeeded);
    }
    world.extra.insert(LEFT_KEY.to_owned(), 0);
    let ended = world.evaluate();
    assert_eq!(
        dispatched(&ended),
        ["task_x"],
        "the round ended: {ended:#?}"
    );
    assert_eq!(
        spend(&ended, "task_x").value("capped_round"),
        Some(&json!(2))
    );
    let left_out = decision(&ended, "task_z");
    assert_eq!(
        hold(left_out).as_deref(),
        Some("capped_not_dispatchable"),
        "{left_out:#?}"
    );
    assert_eq!(
        last(left_out).outcome.value("gate_code"),
        Some(&json!("trigger_false"))
    );
}

// Test 19 (fix 1, the review's finding 3). A pool that writes both lines but no valuation compiles
// with one warning, `pool_lines_unvalued` (a registered warning) at its `stop_line`, listed with the
// compiled catalog and counted by its dry-run report; with a valuation, or without lines, there is
// none.
#[test]
fn ss2h_t19_lines_without_a_valuation_compile_with_a_warning() {
    let unvalued = catalog(vec![judged("task_f", 30, 10)], bare(pools(true)));
    let [warning] = unvalued.warnings() else {
        panic!("one warning: {:?}", unvalued.warnings());
    };
    assert_eq!(warning.code, CatalogDiagnosticCode::PoolLinesUnvalued);
    assert_eq!(warning.severity, DiagnosticSeverity::Warning);
    assert_eq!(warning.json_path, "/pools/1/stop_line");
    assert_eq!(warning.code.code(), PolicyCode::PoolLinesUnvalued.code());
    assert_eq!(
        PolicyCode::PoolLinesUnvalued.as_str(),
        "pool_lines_unvalued"
    );
    assert_eq!(PolicyCode::PoolLinesUnvalued.category(), Category::Warning);
    let report = serde_json::from_slice::<Value>(&unvalued.dry_run_json().expect("the dry run"))
        .expect("the dry-run report");
    assert_eq!(report["status"], json!("accepted"));
    assert_eq!(report["diagnostic_statistics"]["warnings"], json!(1));
    assert_eq!(report["warnings"][0]["code"], json!("pool_lines_unvalued"));
    for (case, pools) in [("valued", pools(true)), ("no lines", bare(pools(false)))] {
        let compiled = catalog(vec![judged("task_f", 30, 10)], pools);
        assert!(
            compiled.warnings().is_empty(),
            "{case}: {:?}",
            compiled.warnings()
        );
    }
}

// ---- The world a test drives ----

/// The catalog, the Runtime's memory, the capped pool's reading and the tasks' snapshots.
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    reading: i64,
    read_at: u64,
    now: u64,
    states: Vec<TaskRuntimeSnapshot>,
    /// Other integer facts of the instance, by key, read when the round is.
    extra: BTreeMap<String, i64>,
    /// Other facts as given.
    records: Vec<ObservedFact>,
    /// The instance is held as the cycle starts (`FactRefreshInputs::instance_holds`).
    busy: bool,
}

impl World {
    /// The world at NOW, the pool read at `reading` a minute before.
    fn new(catalog: CompiledCatalog, reading: i64) -> Self {
        Self {
            catalog,
            memory: CappedMemory::default(),
            reading,
            read_at: NOW - MINUTE_MS,
            now: NOW,
            states: Vec::new(),
            extra: BTreeMap::new(),
            records: Vec::new(),
            busy: false,
        }
    }

    fn clone_world(&self) -> Self {
        Self {
            catalog: self.catalog.clone(),
            memory: self.memory.clone(),
            reading: self.reading,
            read_at: self.read_at,
            now: self.now,
            states: self.states.clone(),
            extra: self.extra.clone(),
            records: self.records.clone(),
            busy: self.busy,
        }
    }

    /// A new reading of the pool, observed now; the next round is a minute later.
    fn set_reading(&mut self, reading: i64) {
        self.reading = reading;
        self.read_at = self.now;
        self.now += MINUTE_MS;
    }

    fn facts(&self) -> EvaluationFacts {
        let mut facts = vec![
            fact(A, READING_KEY, self.reading, self.read_at, None),
            fact(A, CAPACITY_KEY, 180, NOW - HOUR_MS, None),
        ];
        facts.extend(
            self.extra
                .iter()
                .map(|(key, value)| fact(A, key, *value, NOW - HOUR_MS, None)),
        );
        facts.extend(self.records.iter().cloned());
        EvaluationFacts {
            ledger_position: 7,
            fact_snapshot_id: "snapshot:fixture-ss2h".to_owned(),
            facts,
            outcomes: Vec::new(),
            tasks: self.states.clone(),
            instances: vec![instance(A, &[])],
            priority_offsets: Vec::new(),
        }
    }

    /// One round with every candidate admitted.
    fn evaluate(&mut self) -> PolicyEvaluation {
        self.evaluate_with(|_| CandidateEligibility::Eligible).0
    }

    /// One round on a busy instance, as the Runtime gives it: the cycle starts with the instance
    /// held (`dispatch_instance_held` in `FactRefreshInputs::instance_holds`) and the admission
    /// defers every candidate with it; and the tasks it was asked about, in order.
    fn evaluate_busy(&mut self) -> (PolicyEvaluation, Vec<String>) {
        self.busy = true;
        let answer = self.evaluate_with(|_| deferred(INSTANCE_HELD));
        self.busy = false;
        answer
    }

    /// One round with `admission`'s answers, and the tasks it was asked about, in order.
    fn evaluate_with(
        &mut self,
        admission: impl Fn(&DispatchIntent) -> CandidateEligibility,
    ) -> (PolicyEvaluation, Vec<String>) {
        let facts = self.facts();
        let resources = resources(&self.catalog, &facts);
        let mut refresh = refresh_inputs();
        if self.busy {
            refresh.instance_holds.insert(
                A.to_owned(),
                InstanceHold {
                    reason: admission_reason(INSTANCE_HELD),
                    next_wake_unix_ms: None,
                },
            );
        }
        let mut asked = Vec::new();
        let evaluation = evaluate_with_capped_spending::<PolicyEvaluationError>(
            &self.catalog,
            &facts,
            &resources,
            time(self.now),
            11,
            &BTreeSet::new(),
            &refresh,
            &RemedyPlan::default(),
            &ChainHandoverInputs::default(),
            &RunSlots::default(),
            &OwedReruns::default(),
            &InstanceHolds::default(),
            &NoProgressGuards::default(),
            &mut self.memory,
            |_, _, _| Ok(false),
            |intent: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
                asked.push(intent.task_id.clone());
                Ok(admission(intent))
            },
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"));
        (evaluation, asked)
    }

    /// The same round by the entry point the Runtime called before SS-2h (no memory), every
    /// candidate admitted.
    fn evaluate_plain(&self) -> PolicyEvaluation {
        let facts = self.facts();
        evaluate_with_no_progress::<PolicyEvaluationError>(
            &self.catalog,
            &facts,
            &resources(&self.catalog, &facts),
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
            |_, _, _| Ok(false),
            |_: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
                Ok(CandidateEligibility::Eligible)
            },
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
    }

    /// Runs the round's one dispatch: it ends in `terminal` having consumed `spent`; the pool is
    /// read half a minute after the dispatch and the next round is a minute after it.
    fn run(&mut self, evaluation: &PolicyEvaluation, spent: i64, terminal: TaskTerminalState) {
        let [intent] = evaluation.dispatch_intents.as_slice() else {
            panic!("one dispatch: {evaluation:#?}");
        };
        self.states.retain(|state| state.task_id != intent.task_id);
        self.states
            .push(snapshot(&intent.task_id, self.now, terminal));
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

fn snapshot(task_id: &str, at: u64, terminal: TaskTerminalState) -> TaskRuntimeSnapshot {
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

/// The admission's deferral by the registered code `code`.
fn deferred(code: &str) -> CandidateEligibility {
    CandidateEligibility::Deferred {
        reason: admission_reason(code),
        next_wake_unix_ms: None,
    }
}

/// The admission's reason of the registered code `code`.
fn admission_reason(code: &str) -> DecisionReason {
    DecisionReason {
        outcome: Outcome::relayed(code),
        detail: format!("fixture admission: {code}"),
    }
}

/// The v2 goal on `pool.goal`, at least 100, stored as the formal entry stores it.
fn stored_goal(catalog: &CompiledCatalog) -> ObservedFact {
    let document = json!({
        "schema_version": "actingcommand.resource-targets.v2",
        "instance": A,
        "valid_until_unix_ms": NOW + DAY_MS,
        "targets": [{
            "id": "goal-b",
            "resource": GOAL,
            "condition": {"kind": "at_least", "amount": 100},
            "apply": {"mode": "adjust", "weight": "score_stage"}
        }]
    });
    let parsed = parse_resource_targets(&serde_json::to_vec(&document).expect("document bytes"))
        .unwrap_or_else(|error| panic!("the goal parses: {error:?}"));
    let checked_at = NOW - MINUTE_MS;
    let checked = check_resource_targets(
        &parsed,
        catalog,
        &EvaluationFacts {
            ledger_position: 7,
            fact_snapshot_id: "snapshot:fixture-ss2h".to_owned(),
            facts: Vec::new(),
            outcomes: Vec::new(),
            tasks: Vec::new(),
            instances: vec![instance(A, &[])],
            priority_offsets: Vec::new(),
        },
        time(checked_at),
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
        observed_at_unix_ms: checked_at,
        expires_at_unix_ms: Some(NOW + DAY_MS),
        confidence_milli: 1000,
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

/// The `capped_spend` reason of `task_id`'s dispatch, on its decision and its reason chain.
fn spend<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a Outcome {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id)
        .unwrap_or_else(|| panic!("a dispatch of {task_id}: {evaluation:#?}"));
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .unwrap_or_else(|| panic!("the reason chain of {task_id}"));
    assert!(
        chain
            .reasons
            .iter()
            .any(|reason| reason.outcome.is(PolicyCode::CappedSpend)),
        "{chain:#?}"
    );
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

fn at_least(fact_key: &str, threshold: i64) -> Value {
    json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": fact_key,
        "comparison": "greater_than_or_equal",
        "value": {"type": "integer", "value": threshold},
        "max_age_ms": DAY_MS
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
        "expected_duration_ms": 60000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// A judged consumer of the capped pool: `consumed` a run (none when 0), `produced` units of
/// `pool.out`, due while the pool holds its consumption.
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

/// `task` marked `benefit_limited`.
fn limited(mut task: Value) -> Value {
    task["benefit_limited"] = json!(true);
    task
}

/// A maintenance task (unmarked) of priority 300 on its hourly clock, consuming nothing.
fn maintenance(id: &str) -> Value {
    task_body(id, 300, Vec::new(), Vec::new())
}

fn with(mut task: Value, name: &str, value: Value) -> Value {
    task[name] = value;
    task
}

fn valuation(weight: u32) -> Value {
    json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": weight})
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
        "valuation": valuation(weight)
    })
}

/// [`valued`] with a capacity of 1 000 000, weighed `weight` milli per `scale` units.
fn valued_scaled(id: &str, weight: u32, scale: u64) -> Value {
    let mut pool = valued(id, weight, 1_000_000);
    pool["valuation"]["scale"] = json!(scale);
    pool
}

fn out_pool() -> Value {
    valued(OUT, 1000, 1_000_000)
}

/// `pool.goal`: 1000 milli a unit, and 1000 more per unit missing from a goal.
fn goal_pool() -> Value {
    let mut pool = valued(GOAL, 1000, 1_000_000);
    pool["valuation"]["gap"] = json!({"rule": "shortfall_linear", "weight_milli": 1000});
    pool
}

/// The capacity fact plus `offset`.
fn line(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

/// The capped pool: its capacity read from its fact, and with `lines` its stop line 40 below the
/// cap and its hard line 76 below it.
fn capped_pool(lines: bool) -> Value {
    let mut pool = valued(POOL, 1000, 1);
    pool["capacity"] = line(0);
    if lines {
        pool["stop_line"] = line(-40);
        pool["hard_line"] = line(-76);
    }
    pool
}

/// The produced pool and the capped pool.
fn pools(lines: bool) -> Value {
    json!([out_pool(), capped_pool(lines)])
}

/// `pools` without any valuation.
fn bare(mut pools: Value) -> Value {
    for pool in pools.as_array_mut().expect("the pools") {
        pool.as_object_mut()
            .expect("a pool object")
            .shift_remove("valuation");
    }
    pools
}

fn catalog(tasks: Vec<Value>, pools: Value) -> CompiledCatalog {
    compile_v2(tasks, pools).unwrap_or_else(|failure| {
        panic!("the fixture catalog compiles: {:?}", failure.diagnostics())
    })
}

fn compile_v2(tasks: Vec<Value>, pools: Value) -> Result<CompiledCatalog, CatalogCompileFailure> {
    let header = json!({
        "catalog_id": "fixture.ss2h",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-ss2h"]
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
        format!("memory://fixture-ss2h/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

// ---- The standard-pack fixture of tests 8 and 13 ----

/// A `schedule.json` rule running the package of its own id on its hourly clock, consuming 10 of
/// the capped pool and producing one `pool.out`.
fn schedule_rule(id: &str) -> Value {
    let mut rule = task_body(id, 100, vec![consume(POOL, 10)], vec![produce(OUT, 1)]);
    let members = rule.as_object_mut().expect("a task object");
    for member in ["scope", "entrypoint", "procedure_ref", "instance_overrides"] {
        members.shift_remove(member);
    }
    members.insert("package_id".to_owned(), json!(id));
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

/// Composes a `schedule.json` of these rules, the two pools `pool.out` and `pool.neutral` (both
/// constant, without lines) or `pools` when given, and `requires`, for one instance of the game.
fn compose(
    rules: Vec<Value>,
    pools: Vec<Value>,
    requires: Vec<&str>,
) -> Result<StandardPackComposition, crate::StandardPackComposeFailure> {
    let pools = if pools.is_empty() {
        vec![schedule_pool(OUT), schedule_pool(POOL)]
    } else {
        let mut all = vec![schedule_pool(OUT)];
        all.extend(pools);
        all
    };
    let schedule = json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": rules,
        "pools": pools,
        "activity": [],
        "timeline": [],
        "held": []
    });
    let packages = schedule["rules"]
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
            .collect::<Vec<_>>(),
        "maintenance": []
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
