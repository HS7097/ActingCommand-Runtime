// SPDX-License-Identifier: AGPL-3.0-only

// #455 MAKE (0.12.104; register §1.558; the coordinator's rulings of 11-29 in its work order, on
// the read-only study of the same day; contracts/resource-targets.md, "Needs computed from the
// records"; contracts/scheduling/README.md, "Needs computed from the records"), the policy's half.
// The host gives the pool of an item with a recipe whose items in hand make part of what is not
// covered the need `held + made` (`PoolNeedKind::Making`), with the pools of its recipe's `from`
// items; here the needs are given as the host gives them:
// - without a stored goal the computed goal covers the pool's makers alone: the maker is lifted
//   and told the made part, the rule that buys the item and the one that farms it decide byte for
//   byte as with the holding's part alone (`Covered`) (t1);
// - an instance with no maker decides byte for byte as with `Covered`: no item, no need, no lapse
//   (t2);
// - a stored goal on such a pool asks `stock + need` and keeps its own coverage (t3), and today's
//   stored goal on a `Covered` pool, whose need tier is 0, is pinned (t4);
// - RR3c's and RR3d's unread rule reads the made part as any goal's gap (t5);
// - a maker of a fixed value of 0 is held without the made part and dispatched with it, its turns
//   bounded by the made part (t6);
// - the pools' hard line still holds a maker whose input reads below one run's consumption (t7).
//
// The fixture: one instance `instance-a`; `pool.top`, the item's pool, without a valuation, writing
// `target_defaults` (`S` 1, `I` 1000) and `goal_tiers` (`I_n` 1000, `C` 40), read at 4; `pool.low`,
// its `from` item's pool, valued 1000 milli a unit at the step 1 with the constant lines 900 000
// and 0; `pool.coin`, its `cost` item's pool, valued 1 milli a unit; the maker consumes 4 `low` and
// produces 1 `top` a run, the buyer consumes 300 `coin` and produces 1 `top`, the farm produces 1
// `top` and consumes nothing. Names are made up; no clock is read.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{Initiator, RESOURCE_TARGETS_FACT_KEY, RuntimeChannel};
use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::resource_targets::SpendValues;
use crate::{
    CandidateEligibility, CappedMemory, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DecisionReason, DispatchIntent, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactRefreshInputs, FactRefreshSettings, FactValue, GivenPoolReadings,
    HandedRuns, HostResourceSnapshot, InstanceHolds, InstancePoolLines, InstanceRecordNeeds,
    LeftOutFacts, NeedHolding, NeedsProjection, NoProgressGuards, ObservedFact, OwedReruns,
    PolicyCode, PolicyEvaluation, PolicyEvaluationError, PoolNeed, PoolNeedKind, RecordNeeds,
    RecordNeedsNote, RemedyPlan, RunSlots, SCHEDULING_SCHEMA_VERSION_V2, ScopeSelector,
    TargetSetter, TaskDecision, check_resource_targets, compile_catalog,
    evaluate_with_left_out_facts, parse_resource_targets, project_fact_pools,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// How long a run lasts: it ends half a minute after the round that dispatched it.
const RUN_MS: u64 = 30_000;
/// The pool of the item with a recipe, read from `TOP_KEY`.
const TOP: &str = "pool.top";
const TOP_KEY: &str = "resource.top";
/// The pool of the recipe's `from` item, read from `LOW_KEY`.
const LOW: &str = "pool.low";
const LOW_KEY: &str = "resource.low";
/// The pool of the recipe's `cost` item, read from `COIN_KEY`.
const COIN: &str = "pool.coin";
const COIN_KEY: &str = "resource.coin";
/// The rule that makes the item: it consumes the `from` item's pool and produces the item's.
const MAKER: &str = "task_make";
/// The rule that buys the item: it consumes the `cost` item's pool and produces the item's.
const BUYER: &str = "task_buy";
/// The rule that farms the item: it produces the item's pool and consumes nothing.
const FARM: &str = "task_farm";
/// The sha256 the fixture's needs name as their tables'.
const SHA: &str = "fixture-tables-make";
/// The declared duration of every task, `E` without a snapshot.
const E: u64 = 60_000;

// Test 1, without a stored goal (rulings 1 and 2). The item needs 6 of `top`, held 4, of which the
// items in hand make 2 now (`Making { made: 2 }`). The computed goal `goal_records` of 6 covers the
// maker alone: it carries `goal_records@pool.top:applied current=4 at_least=6 gap=2` (need tier 2,
// `Γ` 2000, the term 2000) and `goal_records_need` with `made=2`, is dispatched with `B` 8000
// (10 000 − 4 000 + 2 000) and told the gap 2. With the holding's part alone (`Covered` 4) it is
// dispatched with `B` 6000 and no goal. The buyer and the farm decide byte for byte the same in both:
// no goal covers them.
#[test]
fn make_t1_the_computed_goal_covers_the_maker_alone_and_tells_it_the_made_part() {
    let compiled = catalog(top_pool(), vec![maker(10_000), buyer(3_000), farm(2_000)]);
    let mut making = World::new(compiled.clone(), 8).with_needs(needs(
        &[(TOP, 6, PoolNeedKind::Making { made: 2 })],
        Vec::new(),
    ));
    let lifted = making.evaluate();
    assert_eq!(dispatched(&lifted), [(MAKER, A)], "{lifted:#?}");
    let made_by = decision(&lifted, MAKER);
    fixed_first(made_by, 8_000, 10_000);
    let item = &reason(made_by, PolicyCode::ResourceTargets).detail;
    assert!(
        item.starts_with("goal_records@pool.top:applied current=4 at_least=6 gap=2 "),
        "{item}"
    );
    assert!(item.contains(" need_gap=2 "), "{item}");
    assert_eq!(
        reason(made_by, PolicyCode::GoalRecordsNeed).detail,
        format!("needed=6 stock=0 held=4 to_characters=4 gap=2 tables={SHA} made=2")
    );
    told_gap(&lifted, MAKER, 2);

    let mut covered =
        World::new(compiled, 8).with_needs(needs(&[(TOP, 4, PoolNeedKind::Covered)], Vec::new()));
    let plain = covered.evaluate();
    assert_eq!(dispatched(&plain), [(MAKER, A)], "{plain:#?}");
    let unlifted = decision(&plain, MAKER);
    fixed_first(unlifted, 6_000, 10_000);
    assert!(!has(unlifted, PolicyCode::ResourceTargets), "{unlifted:#?}");
    for task_id in [BUYER, FARM] {
        let pushed = decision(&lifted, task_id);
        assert_eq!(pushed, decision(&plain, task_id), "{task_id}");
        assert!(!has(pushed, PolicyCode::ResourceTargets), "{pushed:#?}");
        assert!(!has(pushed, PolicyCode::GoalRecordsNeed), "{pushed:#?}");
    }
}

// Test 2, no maker on the instance (ruling 1). A catalogue of the buyer and the farm alone, neither
// of which consumes the `from` item's pool: with `Making { made: 2 }` of 6 every decision is byte
// for byte the one with `Covered` 4 but for the set's own identity, which the dispatched decision's
// record names in its policy's (`goal-records:<identity>`), and the same task is dispatched; no
// candidate carries a `resource_targets` item, a `goal_records_need` or
// `resource_target_tasks_unevaluable`.
#[test]
fn make_t2_an_instance_without_a_maker_decides_as_with_the_holdings_part_alone() {
    let compiled = catalog(top_pool(), vec![buyer(3_000), farm(2_000)]);
    let made_set = needs(&[(TOP, 6, PoolNeedKind::Making { made: 2 })], Vec::new());
    let covered_set = needs(&[(TOP, 4, PoolNeedKind::Covered)], Vec::new());
    let identity_of = |given: &RecordNeeds| {
        given
            .get(A)
            .map(|set| set.identity().to_owned())
            .unwrap_or_else(|| panic!("the needs of {A}"))
    };
    let (made_identity, covered_identity) = (identity_of(&made_set), identity_of(&covered_set));
    assert_ne!(made_identity, covered_identity);
    let mut making = World::new(compiled.clone(), 8).with_needs(made_set);
    let mut covered = World::new(compiled, 8).with_needs(covered_set);
    let with_made = making.evaluate();
    let without = covered.evaluate();
    assert_eq!(
        said(&with_made, &made_identity),
        said(&without, &covered_identity)
    );
    assert_eq!(dispatched(&with_made), dispatched(&without));
    assert_eq!(dispatched(&with_made), [(BUYER, A)], "{with_made:#?}");
    for entry in &with_made.decisions {
        for code in [
            PolicyCode::ResourceTargets,
            PolicyCode::GoalRecordsNeed,
            PolicyCode::ResourceTargetTasksUnevaluable,
        ] {
            assert!(!has(entry, code), "{}: {entry:#?}", code.as_str());
        }
    }
}

// Test 3, a stored goal of 50 on the item's pool (ruling 1). It asks `stock + need` = 56, the gap
// is 52, its need tier the made part 2 and its stock tier 50 (`Γ` 2000 + 40), and it keeps its own
// coverage: the maker (`B` 8040) and the buyer (`B` 4740) both carry it. The maker is told 2, the
// need tier while it is open; `goal_records_need` says the stock and `made=2`.
#[test]
fn make_t3_a_stored_goal_on_the_pool_asks_its_stock_and_the_need_and_keeps_its_coverage() {
    let compiled = catalog(top_pool(), vec![maker(10_000), buyer(3_000), farm(2_000)]);
    let stored = goal_record(&compiled, vec![goal_target("goal-top", TOP, 50)]);
    let mut world = World::new(compiled, 8).with_goal(&stored).with_needs(needs(
        &[(TOP, 6, PoolNeedKind::Making { made: 2 })],
        Vec::new(),
    ));
    let evaluation = world.evaluate();
    assert_eq!(dispatched(&evaluation), [(MAKER, A)], "{evaluation:#?}");
    for (task_id, benefit, value) in [(MAKER, 8_040, 10_000), (BUYER, 4_740, 3_000)] {
        let entry = decision(&evaluation, task_id);
        fixed_first(entry, benefit, value);
        let item = &reason(entry, PolicyCode::ResourceTargets).detail;
        assert!(
            item.starts_with("goal-top@pool.top:applied current=4 at_least=56 gap=52 "),
            "{task_id}: {item}"
        );
        assert!(item.contains(" need_gap=2 "), "{task_id}: {item}");
        assert!(item.contains(" stock_gap=50 "), "{task_id}: {item}");
        assert!(!item.contains("goal_records@"), "{task_id}: {item}");
        assert_eq!(
            reason(entry, PolicyCode::GoalRecordsNeed).detail,
            format!("needed=6 stock=50 held=4 to_characters=4 gap=52 tables={SHA} made=2"),
            "{task_id}"
        );
    }
    told_gap(&evaluation, MAKER, 2);
}

// Test 4, today's stored goal on the pool of an item with a recipe, pinned (the study's t4). With
// the holding's part alone (`Covered` 4) the stored goal of 50 asks 54, the gap is 50, its need
// tier 0 and its stock tier 50 (`Γ` 40): only the stock lifts, the maker's `B` 6040, and it is told
// 50; no computed goal is given, and `goal_records_need` names no made part.
#[test]
fn make_t4_today_s_stored_goal_on_a_covered_pool_lifts_by_its_stock_alone() {
    let compiled = catalog(top_pool(), vec![maker(10_000), buyer(3_000), farm(2_000)]);
    let stored = goal_record(&compiled, vec![goal_target("goal-top", TOP, 50)]);
    let mut world = World::new(compiled, 8)
        .with_goal(&stored)
        .with_needs(needs(&[(TOP, 4, PoolNeedKind::Covered)], Vec::new()));
    let evaluation = world.evaluate();
    assert_eq!(dispatched(&evaluation), [(MAKER, A)], "{evaluation:#?}");
    for (task_id, benefit, value) in [(MAKER, 6_040, 10_000), (BUYER, 2_740, 3_000)] {
        let entry = decision(&evaluation, task_id);
        fixed_first(entry, benefit, value);
        let item = &reason(entry, PolicyCode::ResourceTargets).detail;
        assert!(
            item.starts_with("goal-top@pool.top:applied current=4 at_least=54 gap=50 "),
            "{task_id}: {item}"
        );
        assert!(item.contains(" need_gap=0 "), "{task_id}: {item}");
        assert!(item.contains(" stock_gap=50 "), "{task_id}: {item}");
        assert!(!item.contains("goal_records@"), "{task_id}: {item}");
        assert_eq!(
            reason(entry, PolicyCode::GoalRecordsNeed).detail,
            format!("needed=4 stock=50 held=4 to_characters=4 gap=50 tables={SHA}"),
            "{task_id}"
        );
    }
    told_gap(&evaluation, MAKER, 50);
}

// Test 5, RR3c's and RR3d's unread rule (ruling 1). A maker not judged, alone, one making a run, no
// `run_cap` (no consumed pool takes part): the first round sends it told the made part 2; its run
// ends and `top` is not read again: one turn of one unit is short of 2, so the pool is read and the
// next round sends it again, told 2. After the second run the two together reach the gap: the pool
// is unread, its holding `Unread` to the needs, and the computed goal pending (no gap told). The
// needs the host then gives, `top`'s whole need 10 `Unsplit` with its note, give it no goal at all.
#[test]
fn make_t5_the_made_part_is_read_by_the_unread_rule_as_any_goal_s_gap() {
    let compiled = catalog(
        top_pool(),
        vec![rule(MAKER, consumes(LOW, 4), produces_top())],
    );
    let mut world = World::new(compiled, 8).with_needs(needs(
        &[(TOP, 6, PoolNeedKind::Making { made: 2 })],
        Vec::new(),
    ));
    for round in ["first", "second"] {
        assert_eq!(
            world.holding(TOP),
            NeedHolding::Known {
                held: 4,
                fresh_until_unix_ms: None
            },
            "{round}"
        );
        let sent = world.evaluate();
        assert_eq!(dispatched(&sent), [(MAKER, A)], "{round}: {sent:#?}");
        told(&sent, MAKER, Some(2), None);
        world.run(&sent, MAKER);
    }
    assert_eq!(world.holding(TOP), NeedHolding::Unread);
    let unread = world.evaluate();
    assert_eq!(
        reason(decision(&unread, MAKER), PolicyCode::ResourceTargets).detail,
        "goal_records@pool.top:pending reason=expired gap_weight=pending"
    );
    told(&unread, MAKER, None, None);

    world.needs = Some(needs(
        &[(TOP, 10, PoolNeedKind::Unsplit)],
        vec![RecordNeedsNote::HoldingUnread {
            item: "item_top".to_owned(),
            pool_id: TOP.to_owned(),
        }],
    ));
    let unsplit = world.evaluate();
    let entry = decision(&unsplit, MAKER);
    assert!(!has(entry, PolicyCode::ResourceTargets), "{entry:#?}");
    assert!(!has(entry, PolicyCode::GoalRecordsNeed), "{entry:#?}");
    let holding = reason(entry, PolicyCode::GoalRecordsHoldingUnknown);
    assert_eq!(holding.outcome.value("pool_id"), Some(&json!(TOP)));
    assert_eq!(
        holding.outcome.value("unknown_reason"),
        Some(&json!("fact_stale"))
    );
}

// Test 6, a maker of the fixed value 0 (#503 FIXV's `fixed_value_any`, on this tree) producing a
// priced `top` (base 5000, gap weight 3000 at the step 1), consuming 4 `low` (read 40, `coin` 700:
// the items in hand pay for 2 makings). With the holding's part alone it is
// `policy_benefit_nonpositive`, `B` −4000; with the made part 2 (`Γ` 6000, the term 6000) it is
// dispatched with `B` 2000, told the gap 2 and the `run_cap` 2, the goal's ⌈2/1⌉ below the spend
// term's ⌊40/4⌋ = 10.
#[test]
fn make_t6_a_maker_worth_nothing_by_itself_is_sent_by_the_made_part_alone() {
    let compiled = catalog(priced_top_pool(), vec![maker(0)]);
    let mut covered = World::new(compiled.clone(), 40)
        .with_needs(needs(&[(TOP, 4, PoolNeedKind::Covered)], Vec::new()));
    covered.read(COIN_KEY, 700, NOW - MINUTE_MS);
    let plain = covered.evaluate();
    assert!(dispatched(&plain).is_empty(), "{plain:#?}");
    let waits = decision(&plain, MAKER);
    fixed_first(waits, -4_000, 0);
    assert!(
        has(waits, PolicyCode::PolicyBenefitNonpositive),
        "{waits:#?}"
    );

    let mut making = World::new(compiled, 40).with_needs(needs(
        &[(TOP, 6, PoolNeedKind::Making { made: 2 })],
        Vec::new(),
    ));
    making.read(COIN_KEY, 700, NOW - MINUTE_MS);
    let lifted = making.evaluate();
    assert_eq!(dispatched(&lifted), [(MAKER, A)], "{lifted:#?}");
    fixed_first(decision(&lifted, MAKER), 2_000, 0);
    told(&lifted, MAKER, Some(2), Some(2));
}

// Test 7, the hard line (the study's item 1). A judged maker whose valued `low` reads 3, below one
// run's 4: with the set the host computes from that reading (`Covered` 4: the items in hand pay for
// no making, the host's m2) it is held `pool_line_held` `would_cross_hard_line`, as without any
// needs; and a made part the host would not give on that reading does not release it either.
#[test]
fn make_t7_a_maker_whose_input_reads_below_one_run_is_held_by_the_hard_line() {
    let compiled = catalog(top_pool(), vec![maker(10_000)]);
    let given = [
        ("without needs", None),
        (
            "covered",
            Some(needs(&[(TOP, 4, PoolNeedKind::Covered)], Vec::new())),
        ),
        (
            "made",
            Some(needs(
                &[(TOP, 6, PoolNeedKind::Making { made: 2 })],
                Vec::new(),
            )),
        ),
    ];
    for (case, set) in given {
        let mut world = World::new(compiled.clone(), 3);
        world.needs = set;
        let evaluation = world.evaluate();
        assert!(
            dispatched(&evaluation).is_empty(),
            "{case}: {evaluation:#?}"
        );
        let held = last(decision(&evaluation, MAKER));
        assert!(
            held.outcome.is(PolicyCode::PoolLineHeld),
            "{case}: {held:?}"
        );
        for (key, value) in [
            ("pool_line_hold", json!("would_cross_hard_line")),
            ("pool_id", json!(LOW)),
        ] {
            assert_eq!(
                held.outcome.value(key),
                Some(&value),
                "{case} {key}: {held:?}"
            );
        }
    }
}

// ---- The world a test drives ----

/// The catalog, the Runtime's memory of the capped episodes, the readings, the goal record of
/// `instance-a`, the needs its records compute, the runs the host notes and the evaluation's time.
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    readings: Vec<ObservedFact>,
    goal: Option<ObservedFact>,
    needs: Option<RecordNeeds>,
    handed: HandedRuns,
    now: u64,
}

impl World {
    /// The world of `instance-a` at NOW: `top` read at 4, `low` at `low` and `coin` at 1000, a
    /// minute before NOW; no goal, no records, no run.
    fn new(compiled: CompiledCatalog, low: i64) -> Self {
        Self {
            catalog: compiled,
            memory: CappedMemory::default(),
            readings: vec![
                reading(TOP_KEY, 4, NOW - MINUTE_MS),
                reading(LOW_KEY, low, NOW - MINUTE_MS),
                reading(COIN_KEY, 1_000, NOW - MINUTE_MS),
            ],
            goal: None,
            needs: None,
            handed: HandedRuns::default(),
            now: NOW,
        }
    }

    /// The same world with `instance-a`'s goal record `goal`.
    fn with_goal(mut self, goal: &ObservedFact) -> Self {
        self.goal = Some(goal.clone());
        self
    }

    /// The same world with the needs `given`.
    fn with_needs(mut self, given: RecordNeeds) -> Self {
        self.needs = Some(given);
        self
    }

    /// `key` reads `value`, observed at `at`, in place of its reading.
    fn read(&mut self, key: &str, value: i64, at: u64) {
        self.readings.retain(|held| held.fact_key != key);
        self.readings.push(reading(key, value, at));
    }

    fn facts(&self) -> EvaluationFacts {
        let mut observed = self.readings.clone();
        observed.extend(self.goal.clone());
        facts_of(observed)
    }

    /// One round now by the Runtime's entry, with its memory, the needs and the noted runs beside
    /// the left-out facts, every candidate admitted.
    fn evaluate(&mut self) -> PolicyEvaluation {
        let facts = self.facts();
        let resource_snapshot = resources(&self.catalog, &facts);
        let mut left_out = LeftOutFacts::default();
        if let Some(given) = &self.needs {
            left_out.give_record_needs(given.clone());
        }
        left_out.give_handed_runs(self.handed.clone());
        evaluate_with_left_out_facts::<PolicyEvaluationError>(
            &self.catalog,
            &facts,
            &resource_snapshot,
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
            &InstancePoolLines::default(),
            &left_out,
            |_, _, _| Ok(false),
            |_: &DispatchIntent| Ok(CandidateEligibility::Eligible),
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
    }

    /// The round's dispatch of `task_id` ran: it ends half a minute after the round, the host notes
    /// it from its recorded reasons, and the next round is a minute after this one.
    fn run(&mut self, evaluation: &PolicyEvaluation, task_id: &str) {
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
        self.handed.note_dispatch(
            &intent.task_id,
            &intent.instance_id,
            Some(self.now + RUN_MS),
            chain.reasons.iter().map(|given| &given.outcome),
        );
        self.now += MINUTE_MS;
    }

    /// The holding of `pool` the needs computed now would read, with the noted runs.
    fn holding(&self, pool: &str) -> NeedHolding {
        NeedsProjection::new(
            &self.catalog,
            &self.facts(),
            &GivenPoolReadings::default(),
            time(self.now),
            &self.handed,
        )
        .unwrap_or_else(|error| panic!("the needs' projection: {error}"))
        .holding(A, pool)
    }
}

/// The needs of `instance-a` as the host gives them: each of `pools` with its need and kind, and
/// `notes`, of the tables [`SHA`], from records set an hour before NOW, of no expiry; a pool of the
/// kind `Making` with the pool of its recipe's `from` item, [`LOW`].
fn needs(pools: &[(&str, u64, PoolNeedKind)], notes: Vec<RecordNeedsNote>) -> RecordNeeds {
    let making_from = pools
        .iter()
        .filter(|(_, _, kind)| matches!(kind, PoolNeedKind::Making { .. }))
        .map(|(pool, _, _)| ((*pool).to_owned(), vec![LOW.to_owned()]))
        .collect::<BTreeMap<_, _>>();
    let mut computed = RecordNeeds::default();
    computed.give(
        A,
        InstanceRecordNeeds::new(
            Some(SHA.to_owned()),
            NOW - HOUR_MS,
            pools
                .iter()
                .map(|(pool, need, kind)| {
                    (
                        (*pool).to_owned(),
                        PoolNeed {
                            need: *need,
                            kind: *kind,
                        },
                    )
                })
                .collect(),
            notes,
            None,
        )
        .with_making_from(making_from),
    );
    computed
}

/// The facts of `instance-a`, at the fixture's ledger position, holding `observed`.
fn facts_of(observed: Vec<ObservedFact>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-455-make".to_owned(),
        facts: observed,
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

/// The integer `value` of `key` of `instance-a`, observed at `at`, of no expiry, at full
/// confidence.
fn reading(key: &str, value: i64, at: u64) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: key.to_owned(),
        value: FactValue::Integer(value),
        observed_at_unix_ms: at,
        expires_at_unix_ms: None,
        confidence_milli: 1_000,
    }
}

/// A v3 target `id` on `pool`, at least `at_least`, mode `adjust`, without numbers of its own.
fn goal_target(id: &str, pool: &str, at_least: u64) -> Value {
    json!({
        "id": id,
        "resource": pool,
        "condition": {"kind": "at_least", "amount": at_least},
        "apply": {"mode": "adjust", "weight": "score_stage"}
    })
}

/// `instance-a`'s v3 goal record of `targets`, checked against `compiled` and stored as the formal
/// entry stores it for a Lab connection, an hour before NOW.
fn goal_record(compiled: &CompiledCatalog, targets: Vec<Value>) -> ObservedFact {
    let document = json!({
        "schema_version": "actingcommand.resource-targets.v3",
        "instance": A,
        "targets": targets
    });
    let parsed = parse_resource_targets(&serde_json::to_vec(&document).expect("document bytes"))
        .unwrap_or_else(|error| panic!("the goal parses: {error:?}"));
    let set_at = NOW - HOUR_MS;
    let accepted = check_resource_targets(&parsed, compiled, &facts_of(Vec::new()), time(set_at))
        .unwrap_or_else(|error| panic!("the goal is accepted: {error:?}"));
    let composed = accepted
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

/// The round's dispatches, by task and instance.
fn dispatched(evaluation: &PolicyEvaluation) -> Vec<(&str, &str)> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| (intent.task_id.as_str(), intent.instance_id.as_str()))
        .collect()
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|entry| entry.task_id == task_id && entry.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id}: {evaluation:#?}"))
}

fn has(entry: &TaskDecision, code: PolicyCode) -> bool {
    entry.reasons.iter().any(|given| given.outcome.is(code))
}

/// Every decision of the round as its `Debug` text writes it, the identity of the needs it was
/// given, `identity`, written `<the set's identity>`: the one part two rounds given two sets differ
/// in when every decision is otherwise the same.
fn said(evaluation: &PolicyEvaluation, identity: &str) -> String {
    format!("{:?}", evaluation.decisions).replace(identity, "<the set's identity>")
}

fn reason(entry: &TaskDecision, code: PolicyCode) -> &DecisionReason {
    entry
        .reasons
        .iter()
        .find(|given| given.outcome.is(code))
        .unwrap_or_else(|| panic!("a {} reason on {entry:#?}", code.as_str()))
}

/// The reason that settled a decision: its last one.
fn last(entry: &TaskDecision) -> &DecisionReason {
    entry
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {entry:#?}"))
}

/// The decision opens with exactly one `benefit_fixed_value`, of `B`, `V` and `E`.
fn fixed_first(entry: &TaskDecision, benefit: i64, value: i64) {
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
    assert_eq!(first.outcome.value("expected_duration_ms"), Some(&json!(E)));
}

/// The dispatch parameters SS-6 builds of `gap` at production entry 0 (none: no `gaps_remaining`)
/// and `run_cap`.
fn parameters(gap: Option<u64>, run_cap: Option<u64>) -> Outcome {
    SpendValues {
        spend_allowances: Vec::new(),
        gaps_remaining: gap.map(|open| (0, open)).into_iter().collect(),
        run_cap,
    }
    .add_to(Outcome::new(PolicyCode::PolicyDispatchParameters))
}

/// The `policy_dispatch_parameters` of `task_id`'s decision, or an outcome without a member when it
/// carries none.
fn given_parameters(evaluation: &PolicyEvaluation, task_id: &str) -> Outcome {
    decision(evaluation, task_id)
        .reasons
        .iter()
        .find(|given| given.outcome.is(PolicyCode::PolicyDispatchParameters))
        .map_or_else(
            || Outcome::new(PolicyCode::PolicyDispatchParameters),
            |given| given.outcome.clone(),
        )
}

/// The dispatch of `task_id` is handed `gap` at its production entry 0 (none: no `gaps_remaining`)
/// and `run_cap` (none: no `run_cap`), as SS-6 builds them.
fn told(evaluation: &PolicyEvaluation, task_id: &str, gap: Option<u64>, run_cap: Option<u64>) {
    let given = given_parameters(evaluation, task_id);
    let built = parameters(gap, run_cap);
    for key in ["gaps_remaining", "run_cap"] {
        assert_eq!(given.value(key), built.value(key), "{key}: {given:?}");
    }
}

/// The dispatch of `task_id` is handed `gap` at its production entry 0, whatever its `run_cap`.
fn told_gap(evaluation: &PolicyEvaluation, task_id: &str, gap: u64) {
    let given = given_parameters(evaluation, task_id);
    let built = parameters(Some(gap), None);
    assert_eq!(
        given.value("gaps_remaining"),
        built.value("gaps_remaining"),
        "{given:?}"
    );
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

/// The `consumes` of `amount` of `pool` a run.
fn consumes(pool: &str, amount: u64) -> Value {
    json!([effect(pool, "consume", amount)])
}

/// The `produces` of 1 `top` a run.
fn produces_top() -> Value {
    json!([effect(TOP, "produce", 1)])
}

/// A task of the game of priority 100, due every hour, with `consumes` and `produces`, not judged.
fn rule(id: &str, consumed: Value, produced: Value) -> Value {
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
        "consumes": consumed,
        "produces": produced,
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

/// `task` judged by net benefit, of the fixed value `value`.
fn judged(mut task: Value, value: i64) -> Value {
    task["benefit_judged"] = json!(true);
    task["benefit_fixed_value_milli"] = json!(value);
    task
}

/// The maker of `V` `value`: 4 `low` for 1 `top` a run.
fn maker(value: i64) -> Value {
    judged(rule(MAKER, consumes(LOW, 4), produces_top()), value)
}

/// The buyer of `V` `value`: 300 `coin` for 1 `top` a run.
fn buyer(value: i64) -> Value {
    judged(rule(BUYER, consumes(COIN, 300), produces_top()), value)
}

/// The farm of `V` `value`: 1 `top` a run, consuming nothing.
fn farm(value: i64) -> Value {
    judged(rule(FARM, json!([]), produces_top()), value)
}

/// A pool of `instance-a` that does not regenerate, of capacity 1 000 000, read from
/// `resource.<the id without "pool.">`.
fn declared_pool(id: &str) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": 1_000_000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    })
}

/// `top` without a valuation: `target_defaults` `S` 1, `I` 1000; `goal_tiers` `I_n` 1000, `C` 40.
fn top_pool() -> Value {
    let mut pool = declared_pool(TOP);
    pool["target_defaults"] = json!({"scale": 1, "importance_milli": 1000});
    pool["goal_tiers"] = json!({"need_importance_milli": 1000, "stock_cap_milli": 40});
    pool
}

/// `top` priced: the step 1, the base 5000, the gap weight 3000.
fn priced_top_pool() -> Value {
    let mut pool = declared_pool(TOP);
    pool["valuation"] = json!({
        "name": "Fixture",
        "unit": "unit",
        "scale": 1,
        "base_weight_milli": 5000,
        "gap": {"rule": "shortfall_linear", "weight_milli": 3000}
    });
    pool
}

/// The catalog of `tasks` over the pool `top` and `low` (valued 1000 at the step 1, its constant
/// stop line 900 000 and hard line 0) and `coin` (valued 1 at the step 1).
fn catalog(top: Value, tasks: Vec<Value>) -> CompiledCatalog {
    let mut low = declared_pool(LOW);
    low["valuation"] =
        json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000});
    low["stop_line"] = json!(900_000);
    low["hard_line"] = json!(0);
    let mut coin = declared_pool(COIN);
    coin["valuation"] =
        json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1});
    let header = json!({
        "catalog_id": "fixture.455-make",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-make"]
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
        pools: source("pools", &document("pools", json!([top, low, coin]))),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the catalog compiles: {:?}", failure.diagnostics()))
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-455-make/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}
