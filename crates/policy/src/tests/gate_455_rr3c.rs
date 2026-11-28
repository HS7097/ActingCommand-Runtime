// SPDX-License-Identifier: AGPL-3.0-only

// #455 RR3c (0.12.98; register §1.543; the coordinator's rulings 1 to 7 of 11-28 in its work order
// `rd-1123/brief-455-rr3c.md`; contracts/scheduling/README.md, "Pool lines";
// contracts/resource-targets.md, "Resource weights (v2)" and "Needs computed from the records";
// contracts/remedies.md, section 4): a run that was told enough turns to close a goal's gap leaves
// the pool it produced for that goal unread, as #477 LF-1's run told its turns leaves the pools it
// consumes, until a reading observed at or after its end.
// - ruling 1's arithmetic, `n·y ≥ g·1000`, one unit a run, half a unit a run, a pending amount
//   (test 1); the span, from the dispatch until a reading at or after the run's end (test 2);
// - the three readers on one fixture: the pool values (`fact_stale`, and a trigger that reads the
//   pool asks for it), a resource target's observation (pending, `Γ` 0, no goal term of `run_cap`,
//   no `gap_remaining`) and the needs' holding (`Unread`, said `goal_records_holding_unknown`
//   `fact_stale`) (test 3);
// - RR3b's numbers: a rule of negative value a need of 2 sends once, not again until the pool is
//   read (test 4); a stock gap of 150 a run of 3 turns cannot close is sent again as before
//   (test 5);
// - a run that spends under a `run_cap` and closes a gap leaves both pools unread (test 6);
// - a pool whose scope covers two instances is unread on both after a run on one (test 8).
//
// The fixture: instances `instance-a` (and `instance-b` in test 8) of one game; the capped pool
// `pool.neutral` of `instance-a` (valued 1000 milli a unit at the step 1, no regeneration, its
// capacity read from its fact, 1000, its stop line 40 below and its hard line 900 below it), as in
// RR3b's gate; the pool `pool.tiered` of `instance-a` and the game's pool `pool.shared`, without a
// valuation, writing `target_defaults` (`S` 10, `I` 1000) and `goal_tiers` (`I_n` 100 000, `C`
// 40). A run ends half a minute after its round, and the host's note of it is made from its
// recorded reasons, as `PolicyHost::handed_runs` makes it. Names are made up; no clock is read.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{Initiator, RESOURCE_TARGETS_FACT_KEY, RuntimeChannel};
use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::evaluator::produced_unread;
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
const B: &str = "instance-b";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// How long a run lasts: it ends half a minute after the round that dispatched it.
const RUN_MS: u64 = 30_000;
/// The capped pool, read from `READING_KEY`, its capacity from `CAPACITY_KEY`.
const POOL: &str = "pool.neutral";
const READING_KEY: &str = "resource.neutral";
const CAPACITY_KEY: &str = "resource.neutral.capacity";
/// The capped pool's capacity.
const CAPACITY: i64 = 1000;
/// The pool of `instance-a` a goal is on, read from `TIERED_KEY`.
const TIERED: &str = "pool.tiered";
const TIERED_KEY: &str = "resource.tiered";
/// The game's pool, read from `SHARED_KEY` in the game's scope (test 8).
const SHARED: &str = "pool.shared";
const SHARED_KEY: &str = "resource.shared";
/// `S` and `I`: the defaults of every goal on the two pools without a valuation.
const DEFAULTS: (u64, u64) = (10, 1000);
/// `I_n` and `C`: their `goal_tiers`.
const TIERS: (u64, u64) = (100_000, 40);
/// The sha256 the fixture's needs name as their tables'.
const SHA: &str = "fixture-tables-c";
/// The declared duration of every task, `E` without a snapshot.
const E: u64 = 60_000;

// Test 1, ruling 1's arithmetic. A dispatch of a task that produces the goal's pool, handed the gap
// `g` at its production entry 0 and the `run_cap` `n` (none: `n` 1), leaves the pool unread exactly
// when `n·y ≥ g·1000`, `y` the production per run in thousandths. One unit a run: a gap of 2 told 2
// unread, told 1 read; a gap of 1 without a `run_cap` unread; a gap of 150 told 3 read. An expected
// yield of half a unit a run (500): a gap of 2 told 4 unread, told 3 read. A pending amount yields
// nothing per run and is passed over, however many turns it was told.
#[test]
fn rr3c_t1_a_run_told_enough_turns_to_close_its_gap_leaves_the_pool_unread() {
    let compiled = catalog(vec![
        rule(
            "task_one",
            hourly(),
            json!([]),
            json!([produce(TIERED, 1)]),
            false,
        ),
        rule(
            "task_half",
            hourly(),
            json!([]),
            json!([produce_expected(TIERED, 500)]),
            false,
        ),
        rule(
            "task_pending",
            hourly(),
            json!([]),
            json!([produce_pending(TIERED)]),
            false,
        ),
    ]);
    let ended = NOW;
    let facts = facts_of(&[A], vec![reading(A, TIERED_KEY, 0, ended - MINUTE_MS)]);
    for (case, task, gap, run_cap, unread) in [
        ("a gap of 2, told 2", "task_one", 2, Some(2), true),
        ("a gap of 2, told 1", "task_one", 2, Some(1), false),
        ("a gap of 1, no run_cap", "task_one", 1, None, true),
        ("a gap of 150, told 3", "task_one", 150, Some(3), false),
        ("half a unit, told 4", "task_half", 2, Some(4), true),
        ("half a unit, told 3", "task_half", 2, Some(3), false),
        ("a pending amount", "task_pending", 2, Some(1_000), false),
    ] {
        let mut handed = HandedRuns::default();
        handed.note_dispatch(task, A, Some(ended), [&parameters(gap, run_cap)]);
        assert_eq!(
            unread_pools(&compiled, &facts, &handed).contains(TIERED),
            unread,
            "{case}"
        );
    }
}

// Test 2, the span (ruling 2). One dispatch at NOW handed a gap of 2 and told 2, one unit a run:
// the pool is unread while its run has not ended (a reading from before the dispatch, or one
// observed while it runs); after it ended at NOW + 30 s, unread with a reading older than that end,
// one observed while it ran included; read again with a reading observed at its end or after.
#[test]
fn rr3c_t2_the_pool_is_unread_until_a_reading_observed_at_or_after_the_runs_end() {
    let compiled = catalog(vec![rule(
        "task_one",
        hourly(),
        json!([]),
        json!([produce(TIERED, 1)]),
        false,
    )]);
    let ended = NOW + RUN_MS;
    for (case, ended_at, observed_at, unread) in [
        ("not ended, read before", None, NOW - MINUTE_MS, true),
        ("not ended, read while it runs", None, NOW + 10_000, true),
        ("ended, read before", Some(ended), NOW - MINUTE_MS, true),
        ("ended, read while it ran", Some(ended), NOW + 10_000, true),
        ("ended, read at its end", Some(ended), ended, false),
        ("ended, read after its end", Some(ended), ended + 1, false),
    ] {
        let mut handed = HandedRuns::default();
        handed.note_dispatch("task_one", A, ended_at, [&parameters(2, Some(2))]);
        let facts = facts_of(&[A], vec![reading(A, TIERED_KEY, 0, observed_at)]);
        assert_eq!(
            unread_pools(&compiled, &facts, &handed).contains(TIERED),
            unread,
            "{case}"
        );
    }
}

// Test 3, the three readers on one fixture (rulings 2 and 3). `task_t` of `V` 10 000 consumes 1 of
// the capped pool (read at 500) and produces 1 of `pool.tiered` (held 0), on which the records need
// 2; `task_w`, due while `pool.tiered` holds its capacity, reads the pool and nothing else. The first
// round sends `task_t`: `B` 11 000, `gap_remaining` 2, `run_cap` min(400, 2) = 2, and the needs'
// holding of the pool is known, 0. Its run ends; the capped pool is read again after that end (499),
// `pool.tiered` is not. The next round:
// - the pool values: `task_w`'s trigger reads the pool unknown, `fact_stale` under its own key, and
//   suggests reading it (the fact refresh is asked for it);
// - the target: pending as a stale reading is, `Γ` 0 (the term 0), so `B` 9000 and the run is
//   handed no `gap_remaining` and a `run_cap` of the spend term alone, 499 − 100 = 399;
// - the needs: the holding is `Unread`, and a set that notes it says it on every candidate as
//   `goal_records_holding_unknown` with `unknown_reason` `fact_stale`.
#[test]
fn rr3c_t3_the_pool_values_the_target_and_the_needs_read_the_pool_as_unread() {
    let mut world = World::new(catalog(vec![producer(10_000), watcher()])).with_needs(A, TIERED, 2);
    world.read(A, READING_KEY, 500, NOW - MINUTE_MS);
    world.read(A, TIERED_KEY, 0, NOW - MINUTE_MS);
    let first = world.evaluate();
    assert_eq!(dispatched(&first), [("task_t", A)], "{first:#?}");
    fixed_first(decision(&first, "task_t", A), 11_000, 10_000);
    told(&first, "task_t", A, Some(2), Some(2));
    assert!(!stale(decision(&first, "task_w", A), TIERED_KEY));
    assert_eq!(
        world.holding(A, TIERED),
        NeedHolding::Known {
            held: 0,
            fresh_until_unix_ms: None
        }
    );
    world.run(&first, "task_t", A);
    world.read(A, READING_KEY, 499, NOW + RUN_MS + 1_000);

    let second = world.evaluate();
    assert!(
        stale(decision(&second, "task_w", A), TIERED_KEY),
        "{second:#?}"
    );
    let producing = decision(&second, "task_t", A);
    assert_eq!(
        reason(producing, PolicyCode::ResourceTargets).detail,
        "goal_records@pool.tiered:pending reason=expired gap_weight=pending"
    );
    let weights = &reason(producing, PolicyCode::ResourceWeights).detail;
    assert!(
        weights.ends_with(" term=0 items=pool.tiered:r=1,per=10,base=0,gap=0,effective=0,term=0"),
        "{weights}"
    );
    fixed_first(producing, 9_000, 10_000);
    assert_eq!(dispatched(&second), [("task_t", A)], "{second:#?}");
    told(&second, "task_t", A, None, Some(399));
    assert_eq!(world.holding(A, TIERED), NeedHolding::Unread);

    world.notes = vec![RecordNeedsNote::HoldingUnread {
        item: "item_a".to_owned(),
        pool_id: TIERED.to_owned(),
    }];
    let noted = world.evaluate();
    let holding = reason(
        decision(&noted, "task_t", A),
        PolicyCode::GoalRecordsHoldingUnknown,
    );
    assert_eq!(holding.outcome.value("pool_id"), Some(&json!(TIERED)));
    assert_eq!(
        holding.outcome.value("unknown_reason"),
        Some(&json!("fact_stale"))
    );
    assert_eq!(holding.detail, "item=item_a");
}

// Test 4, RR3b's numbers. `task_t` of `V` 900 consumes 1 of the capped pool (read at 500) at a cost
// of 1000, so it is −100 of its own, and produces 1 of `pool.tiered`, held 0, on which the records
// need 2: `Γ_n = ⌊100 000·2/10⌋ = 20 000`, the term 2000, `B` 1900, sent once and told 2
// (`gap_remaining` 2, `run_cap` 2). Its run ends and the capped pool is read again after that end;
// `pool.tiered`'s reading is unchanged: the next round does not send it again (`B` −100,
// `policy_benefit_nonpositive`) and the pool is asked for (`task_w`). A reading after the end that
// shows the need met (2 held): not sent (`B` −100). One that shows 1 still lacking: sent again
// (`B` 900) and told 1.
#[test]
fn rr3c_t4_a_negative_rule_a_need_sent_is_not_sent_again_until_the_pool_is_read() {
    let mut world = World::new(catalog(vec![producer(900), watcher()])).with_needs(A, TIERED, 2);
    world.read(A, READING_KEY, 500, NOW - MINUTE_MS);
    world.read(A, TIERED_KEY, 0, NOW - MINUTE_MS);
    let sent = world.evaluate();
    assert_eq!(dispatched(&sent), [("task_t", A)], "{sent:#?}");
    fixed_first(decision(&sent, "task_t", A), 1_900, 900);
    told(&sent, "task_t", A, Some(2), Some(2));
    world.run(&sent, "task_t", A);
    let ended = NOW + RUN_MS;
    world.read(A, READING_KEY, 499, ended + 1_000);

    let unchanged = world.evaluate();
    assert!(dispatched(&unchanged).is_empty(), "{unchanged:#?}");
    let waits = decision(&unchanged, "task_t", A);
    fixed_first(waits, -100, 900);
    assert!(
        has(waits, PolicyCode::PolicyBenefitNonpositive),
        "{waits:#?}"
    );
    assert!(
        stale(decision(&unchanged, "task_w", A), TIERED_KEY),
        "{unchanged:#?}"
    );

    world.read(A, TIERED_KEY, 2, ended + 2_000);
    world.now += MINUTE_MS;
    let met = world.evaluate();
    assert!(dispatched(&met).is_empty(), "{met:#?}");
    fixed_first(decision(&met, "task_t", A), -100, 900);

    world.read(A, TIERED_KEY, 1, ended + 3_000);
    world.now += MINUTE_MS;
    let lacking = world.evaluate();
    assert_eq!(dispatched(&lacking), [("task_t", A)], "{lacking:#?}");
    fixed_first(decision(&lacking, "task_t", A), 900, 900);
    told(&lacking, "task_t", A, Some(1), Some(1));
}

// Test 5, a gap a run cannot close (ruling 4; today's behaviour, pinned). `task_t` of `V` 10 000
// produces 1 of `pool.tiered` under a stored goal of 150, the records' need of 2 met (2 held): the
// stock's part, `Γ` 40, the term 4, `B` 9004. The capped pool read at 103 pays for 3 turns, so the
// run is told 150 and 3: 3·1000 < 150·1000, and the pool stays read. Its run ends and the capped
// pool is read again after that end (103 again); `pool.tiered` is not: the next round sends the
// rule again with the same values.
#[test]
fn rr3c_t5_a_run_not_told_enough_to_close_the_gap_leaves_the_pool_read() {
    let compiled = catalog(vec![producer(10_000)]);
    let stored = goal_record(&compiled, vec![goal_target("goal-t", TIERED, 150)]);
    let mut world = World::new(compiled)
        .with_goal(&stored)
        .with_needs(A, TIERED, 2);
    world.read(A, READING_KEY, 103, NOW - MINUTE_MS);
    world.read(A, TIERED_KEY, 2, NOW - MINUTE_MS);
    let first = world.evaluate();
    assert_eq!(dispatched(&first), [("task_t", A)], "{first:#?}");
    fixed_first(decision(&first, "task_t", A), 9_004, 10_000);
    told(&first, "task_t", A, Some(150), Some(3));
    world.run(&first, "task_t", A);
    world.read(A, READING_KEY, 103, NOW + RUN_MS + 1_000);

    let again = world.evaluate();
    assert_eq!(dispatched(&again), [("task_t", A)], "{again:#?}");
    fixed_first(decision(&again, "task_t", A), 9_004, 10_000);
    told(&again, "task_t", A, Some(150), Some(3));
}

// Test 6, beside LF-1 (ruling 4). Test 3's first round, then its run ends and nothing is read
// again: the capped pool, which the run spent from under its `run_cap`, is unread by LF-1's rule
// (`task_t` held `pool_line_held` `capped_cap_unknown`, `fact_stale` under the pool's key), and
// `pool.tiered`, which it was told enough to close the gap on, by this slice's (`task_w` asks for
// it). Nothing is dispatched.
#[test]
fn rr3c_t6_a_run_that_spends_under_a_run_cap_and_closes_a_gap_leaves_both_pools_unread() {
    let mut world = World::new(catalog(vec![producer(10_000), watcher()])).with_needs(A, TIERED, 2);
    world.read(A, READING_KEY, 500, NOW - MINUTE_MS);
    world.read(A, TIERED_KEY, 0, NOW - MINUTE_MS);
    let first = world.evaluate();
    assert_eq!(dispatched(&first), [("task_t", A)], "{first:#?}");
    told(&first, "task_t", A, Some(2), Some(2));
    world.run(&first, "task_t", A);

    let after = world.evaluate();
    assert!(dispatched(&after).is_empty(), "{after:#?}");
    let held = last(decision(&after, "task_t", A));
    assert!(held.outcome.is(PolicyCode::PoolLineHeld), "{held:?}");
    for (key, value) in [
        ("pool_line_hold", json!("capped_cap_unknown")),
        ("pool_id", json!(POOL)),
        ("fact_key", json!(READING_KEY)),
        ("unknown_reason", json!("fact_stale")),
    ] {
        assert_eq!(held.outcome.value(key), Some(&value), "{key}: {held:?}");
    }
    assert!(
        stale(decision(&after, "task_w", A), TIERED_KEY),
        "{after:#?}"
    );
}

// Test 8, a pool whose scope covers two instances (ruling 6, a limit written down). `task_g` of
// `V` 10 000, of the game, produces 2 of the game's pool (held 0) a run, on which the records of
// each instance need 2: both are sent, told 2 at `run_cap` none (1 turn, 2·1000 ≥ 2·1000). Only the
// run on `instance-a` is noted, and it ends: the next round reads the pool's goal pending on both
// instances.
#[test]
fn rr3c_t8_a_pool_whose_scope_covers_two_instances_is_unread_on_both_after_a_run_on_one() {
    let compiled = catalog(vec![fixed(
        rule(
            "task_g",
            hourly(),
            json!([]),
            json!([produce(SHARED, 2)]),
            true,
        ),
        10_000,
    )]);
    let mut world = World::new(compiled)
        .on(&[A, B])
        .with_needs(A, SHARED, 2)
        .with_needs(B, SHARED, 2);
    world.read_game(SHARED_KEY, 0, NOW - MINUTE_MS);
    let first = world.evaluate();
    for alias in [A, B] {
        let item = targets_item(&first, "task_g", alias);
        assert!(
            item.starts_with("goal_records@pool.shared:applied current=0 at_least=2 gap=2 "),
            "{alias}: {item}"
        );
    }
    told(&first, "task_g", A, Some(2), None);
    world.run(&first, "task_g", A);

    let after = world.evaluate();
    for alias in [A, B] {
        assert_eq!(
            targets_item(&after, "task_g", alias),
            "goal_records@pool.shared:pending reason=expired gap_weight=pending",
            "{alias}"
        );
    }
}

// ---- The world a test drives ----

/// The catalog, the Runtime's memory of the capped episodes, the instances, the readings, the goal
/// record of `instance-a`, the needs the records compute (by instance, pool and need, with the
/// notes of `instance-a`'s set), the runs the host notes and the evaluation's time.
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    instances: Vec<&'static str>,
    readings: Vec<ObservedFact>,
    goal: Option<ObservedFact>,
    needs: Vec<(&'static str, &'static str, u64)>,
    notes: Vec<RecordNeedsNote>,
    handed: HandedRuns,
    now: u64,
}

impl World {
    /// The world of `instance-a` at NOW: nothing read but the capped pool's capacity, no goal, no
    /// records, no run.
    fn new(compiled: CompiledCatalog) -> Self {
        Self {
            catalog: compiled,
            memory: CappedMemory::default(),
            instances: vec![A],
            readings: Vec::new(),
            goal: None,
            needs: Vec::new(),
            notes: Vec::new(),
            handed: HandedRuns::default(),
            now: NOW,
        }
    }

    /// The same world over `instances`.
    fn on(mut self, instances: &[&'static str]) -> Self {
        self.instances = instances.to_vec();
        self
    }

    /// The same world with `instance-a`'s goal record `goal`.
    fn with_goal(mut self, goal: &ObservedFact) -> Self {
        self.goal = Some(goal.clone());
        self
    }

    /// The same world with records of `alias` whose computed needs land `need` on `pool`.
    fn with_needs(mut self, alias: &'static str, pool: &'static str, need: u64) -> Self {
        self.needs.push((alias, pool, need));
        self
    }

    /// `key` of the instance `alias` reads `value`, observed at `at`, in place of its reading.
    fn read(&mut self, alias: &str, key: &str, value: i64, at: u64) {
        self.replace(reading(alias, key, value, at));
    }

    /// `key` of the game reads `value`, observed at `at`, in place of its reading.
    fn read_game(&mut self, key: &str, value: i64, at: u64) {
        let mut read = reading(A, key, value, at);
        read.scope = ScopeSelector::Game {
            game_id: GAME.to_owned(),
        };
        self.replace(read);
    }

    fn replace(&mut self, read: ObservedFact) {
        self.readings
            .retain(|held| held.scope != read.scope || held.fact_key != read.fact_key);
        self.readings.push(read);
    }

    fn facts(&self) -> EvaluationFacts {
        let mut observed = vec![reading(A, CAPACITY_KEY, CAPACITY, NOW - HOUR_MS)];
        observed.extend(self.readings.iter().cloned());
        observed.extend(self.goal.clone());
        facts_of(&self.instances, observed)
    }

    /// The needs of each instance with records, of the tables [`SHA`], set an hour before NOW, of
    /// no expiry; `instance-a`'s with [`World::notes`].
    fn record_needs(&self) -> RecordNeeds {
        let mut computed = RecordNeeds::default();
        for alias in self.instances.iter().copied() {
            let landing = self
                .needs
                .iter()
                .filter(|(of, _, _)| *of == alias)
                .map(|(_, pool, need)| {
                    (
                        (*pool).to_owned(),
                        PoolNeed {
                            need: *need,
                            kind: PoolNeedKind::Lands,
                        },
                    )
                })
                .collect::<BTreeMap<_, _>>();
            if landing.is_empty() {
                continue;
            }
            let notes = if alias == A {
                self.notes.clone()
            } else {
                Vec::new()
            };
            computed.give(
                alias,
                InstanceRecordNeeds::new(Some(SHA.to_owned()), NOW - HOUR_MS, landing, notes, None),
            );
        }
        computed
    }

    /// One round now by the Runtime's entry, with its memory, the needs and the noted runs beside
    /// the left-out facts, every candidate admitted.
    fn evaluate(&mut self) -> PolicyEvaluation {
        let facts = self.facts();
        let resource_snapshot = resources(&self.catalog, &facts, &self.instances);
        let mut left_out = LeftOutFacts::default();
        left_out.give_record_needs(self.record_needs());
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

    /// The round's dispatch of `task_id` on `alias` ran: it ends half a minute after the round, the
    /// host notes it from its recorded reasons, and the next round is a minute after this one.
    fn run(&mut self, evaluation: &PolicyEvaluation, task_id: &str, alias: &str) {
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == task_id && intent.instance_id == alias)
            .unwrap_or_else(|| panic!("a dispatch of {task_id} on {alias}: {evaluation:#?}"));
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

    /// The holding of `pool` on `alias` the needs computed now would read, with the noted runs.
    fn holding(&self, alias: &str, pool: &str) -> NeedHolding {
        NeedsProjection::new(
            &self.catalog,
            &self.facts(),
            &GivenPoolReadings::default(),
            time(self.now),
            &self.handed,
        )
        .unwrap_or_else(|error| panic!("the needs' projection: {error}"))
        .holding(alias, pool)
    }
}

/// The facts of `instances`, at the fixture's ledger position, holding `observed`.
fn facts_of(instances: &[&str], observed: Vec<ObservedFact>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-455-rr3c".to_owned(),
        facts: observed,
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: instances.iter().map(|alias| instance(alias, &[])).collect(),
        priority_offsets: Vec::new(),
    }
}

/// The integer `value` of `key` in the scope of the instance `alias`, observed at `at`, of no
/// expiry, at full confidence.
fn reading(alias: &str, key: &str, value: i64, at: u64) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: alias.to_owned(),
        },
        fact_key: key.to_owned(),
        value: FactValue::Integer(value),
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
) -> BTreeSet<String> {
    let bundle = compiled.catalog();
    produced_unread(
        &bundle.pools.pools,
        facts,
        &GivenPoolReadings::default(),
        &bundle.tasks.tasks,
        handed,
    )
}

/// The `policy_dispatch_parameters` SS-6 builds of the gap `gap` at production entry 0 and
/// `run_cap`.
fn parameters(gap: u64, run_cap: Option<u64>) -> Outcome {
    SpendValues {
        spend_allowances: Vec::new(),
        gaps_remaining: vec![(0, gap)],
        run_cap,
    }
    .add_to(Outcome::new(PolicyCode::PolicyDispatchParameters))
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
    let accepted =
        check_resource_targets(&parsed, compiled, &facts_of(&[A], Vec::new()), time(set_at))
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

fn resources(
    compiled: &CompiledCatalog,
    facts: &EvaluationFacts,
    instances: &[&str],
) -> EvaluationResources {
    project_fact_pools(
        compiled,
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

// ---- Assertions ----

/// The round's dispatches, by task and instance.
fn dispatched(evaluation: &PolicyEvaluation) -> Vec<(&str, &str)> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| (intent.task_id.as_str(), intent.instance_id.as_str()))
        .collect()
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str, alias: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|entry| entry.task_id == task_id && entry.instance_id.as_deref() == Some(alias))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {alias}: {evaluation:#?}"))
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

/// The detail of the `resource_targets` reason of `task_id`'s decision on `alias`.
fn targets_item(evaluation: &PolicyEvaluation, task_id: &str, alias: &str) -> String {
    reason(
        decision(evaluation, task_id, alias),
        PolicyCode::ResourceTargets,
    )
    .detail
    .clone()
}

/// The reason that settled a decision: its last one.
fn last(entry: &TaskDecision) -> &DecisionReason {
    entry
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {entry:#?}"))
}

/// Whether the decision suggests reading `fact_key` for `fact_stale`.
fn stale(entry: &TaskDecision, fact_key: &str) -> bool {
    entry
        .detection_suggestions
        .iter()
        .any(|suggestion| suggestion.fact_key == fact_key && suggestion.reason == "fact_stale")
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

/// The dispatch of `task_id` on `alias` is handed `gap` at its production entry 0 (none: no
/// `gaps_remaining`) and `run_cap` (none: no `run_cap`), as SS-6 builds them.
fn told(
    evaluation: &PolicyEvaluation,
    task_id: &str,
    alias: &str,
    gap: Option<u64>,
    run_cap: Option<u64>,
) {
    let given = &reason(
        decision(evaluation, task_id, alias),
        PolicyCode::PolicyDispatchParameters,
    )
    .outcome;
    let built = SpendValues {
        spend_allowances: Vec::new(),
        gaps_remaining: gap.map(|open| (0, open)).into_iter().collect(),
        run_cap,
    }
    .add_to(Outcome::new(PolicyCode::PolicyDispatchParameters));
    for key in ["gaps_remaining", "run_cap"] {
        assert_eq!(given.value(key), built.value(key), "{key}: {given:?}");
    }
}

// ---- The catalog ----

fn effect(pool: &str, amount: Value) -> Value {
    json!({
        "pool_id": pool,
        "direction": "produce",
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

fn consume(pool: &str, amount: u64) -> Value {
    let mut consumed = effect(pool, json!(amount));
    consumed["direction"] = json!("consume");
    consumed
}

fn produce(pool: &str, amount: u64) -> Value {
    effect(pool, json!(amount))
}

/// A production of `pool` of an expected `expected_milli` thousandths a run, of no `amount`.
fn produce_expected(pool: &str, expected_milli: u64) -> Value {
    let mut produced = effect(pool, json!(0));
    let members = produced.as_object_mut().expect("an effect object");
    members.shift_remove("amount");
    members.insert("expected_amount_milli".to_owned(), json!(expected_milli));
    produced
}

/// A production of `pool` whose amount is pending.
fn produce_pending(pool: &str) -> Value {
    effect(pool, json!("pending"))
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

/// A task of the game of priority 100, due by `trigger`, with `consumes` and `produces`, judged by
/// net benefit when `judged`.
fn rule(id: &str, trigger: Value, consumes: Value, produces: Value, judged: bool) -> Value {
    let mut task = json!({
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
    });
    if judged {
        task["benefit_judged"] = json!(true);
    }
    task
}

/// `task` with the fixed value `value`.
fn fixed(mut task: Value, value: i64) -> Value {
    task["benefit_fixed_value_milli"] = json!(value);
    task
}

/// `task_t` of `V` `value`, hourly, consuming 1 of the capped pool and producing 1 of
/// `pool.tiered` a run.
fn producer(value: i64) -> Value {
    fixed(
        rule(
            "task_t",
            hourly(),
            json!([consume(POOL, 1)]),
            json!([produce(TIERED, 1)]),
            true,
        ),
        value,
    )
}

/// `task_w`, not judged, due while `pool.tiered` holds its capacity: a reader of the pool that does
/// nothing else, consuming and producing nothing.
fn watcher() -> Value {
    rule(
        "task_w",
        json!({
            "kind": "resource_projection",
            "pool_id": TIERED,
            "comparison": "greater_than_or_equal",
            "value": 1_000_000
        }),
        json!([]),
        json!([]),
        false,
    )
}

/// A pool of `scope` that does not regenerate, of capacity 1 000 000, read from `resource.<the id
/// without "pool.">`.
fn declared_pool(id: &str, scope: Value) -> Value {
    json!({
        "id": id,
        "scope": scope,
        "capacity": 1_000_000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    })
}

/// A pool of `scope` without a valuation, writing [`DEFAULTS`] and [`TIERS`].
fn goal_pool(id: &str, scope: Value) -> Value {
    let mut pool = declared_pool(id, scope);
    pool["target_defaults"] = json!({"scale": DEFAULTS.0, "importance_milli": DEFAULTS.1});
    pool["goal_tiers"] = json!({"need_importance_milli": TIERS.0, "stock_cap_milli": TIERS.1});
    pool
}

/// The capacity fact plus `offset`.
fn line(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

/// The fixture's pools: the capped pool, `pool.tiered` and the game's `pool.shared`.
fn pools() -> Value {
    let instance_a = json!({"kind": "instance", "instance_id": A});
    let mut capped = declared_pool(POOL, instance_a.clone());
    capped["valuation"] =
        json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000});
    capped["capacity"] = line(0);
    capped["stop_line"] = line(-40);
    capped["hard_line"] = line(-900);
    json!([
        capped,
        goal_pool(TIERED, instance_a),
        goal_pool(SHARED, json!({"kind": "game", "game_id": GAME}))
    ])
}

fn catalog(tasks: Vec<Value>) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.455-rr3c",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-rr3c"]
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
        pools: source("pools", &document("pools", pools())),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the catalog compiles: {:?}", failure.diagnostics()))
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-455-rr3c/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}
