// SPDX-License-Identifier: AGPL-3.0-only

// #455 RR3e (0.12.104; register §1.560; the coordinator's rulings of 11-29 in its work order
// `rd-1123/brief-455-rr3e.md`, after the read-only study `rd-1123/study-rr3c-gaps-1128.md`;
// contracts/scheduling/README.md, "Fact Refresh" and "Pool lines"; contracts/resource-targets.md,
// "A pool a run was told enough to close the gap on"), ruling 1: a judged producer the net-benefit
// gate holds as not positive while a goal that covers it waits on a pool the production told under
// the goal's gap left unread asks the fact refresh for that pool's reading, with the pool's key and
// `fact_stale`; from there it is the existing fact refresh. A producer that passes on its own value
// asks nothing (test 2), and a pool read from rows of a list is not asked of the fact refresh, whose
// reader never reads a list (test 3). Ruling 2: the task whose scheduled run published a list (bound
// to it beside the reading, as the host gives it) is treated as triggered once while a candidate
// waits on a pool read from the list that the runs left unread (`list_reading_owed`), and passes no
// gate: once in the pool's unread episode, not while its run is open nor after it (test 4); its
// cooldown holds it with its own reason (test 5); on an instance the idle action closed a reading
// alone waits (test 6); without a reader nothing is owed (test 7); a list read after the runs' end
// releases the pool and owes nothing (test 8); two dispatches that together leave the pool unread
// owe one reading, not one each (test 9); a reader the failure pacing holds waits for the pacing's
// end (test 10).
//
// The fixture is RR3c's (`gate_455_rr3c`): the instance `instance-a` of one game; the capped pool
// `pool.neutral` (valued 1000 milli a unit at the step 1, no regeneration, its capacity read from
// its fact, 1000, its stop line 40 below and its hard line 900 below it); `pool.tiered`, read from
// its own fact, and `pool.listed`, read from rows of a list, both without a valuation, writing
// `target_defaults` (`S` 10, `I` 1000) and `goal_tiers` (`I_n` 100 000, `C` 40). The records need 2
// of the producer's pool, held 0, so the first round sends a producer of `V` 900 with `B` 1900 and
// tells it 2 at `run_cap` 2, which leaves the pool unread; the capped pool is read again after the
// run's end. A recognition maintenance package of `instance-a` marks `pool.tiered`'s key, and the
// fact refresh is on. Ruling 2's tests add `task_r`, the list's reader, due by its own trigger never
// (it reads `pool.tiered`, held 0, against 1 000 000), and, in test 6, `task_w`, which reads
// `pool.listed` against 1 000 000. Names are made up; no clock is read.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::{Code, Outcome};
use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::resource_targets::SpendValues;
use crate::{
    CandidateEligibility, CappedMemory, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DecisionReason, DispatchIntent, EvaluationFacts, EvaluationResources,
    EvaluationTime, FACT_REFRESH_TASK_ID, FactRefreshInputs, FactRefreshReader,
    FactRefreshSettings, FactValue, GivenPoolReading, GivenPoolReadings, GivenReadingState,
    HandedRuns, HostResourceSnapshot, IdleClosed, InstanceHolds, InstancePoolLines,
    InstanceRecordNeeds, LeftOutFacts, NoProgressGuards, ObservedFact, OwedReruns, PolicyCode,
    PolicyEvaluation, PolicyEvaluationError, PoolNeed, PoolNeedKind, RecordNeeds, RemedyPlan,
    RunSlots, SCHEDULING_SCHEMA_VERSION_V2, SchedulingDecisionState, ScopeSelector, TaskDecision,
    TaskRuntimeSnapshot, TaskTerminalState, compile_catalog, evaluate_with_left_out_facts,
    project_fact_pools_with_readings,
};

const A: &str = "instance-a";
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
/// The pool of `instance-a` read from its own fact, `TIERED_KEY`.
const TIERED: &str = "pool.tiered";
const TIERED_KEY: &str = "resource.tiered";
/// The pool of `instance-a` read from rows of the list `LIST_KEY`.
const LISTED: &str = "pool.listed";
const LIST_KEY: &str = "inventory.list.example_kind_a";
/// `S` and `I`: the defaults of every goal on the two pools without a valuation.
const DEFAULTS: (u64, u64) = (10, 1000);
/// `I_n` and `C`: their `goal_tiers`.
const TIERS: (u64, u64) = (100_000, 40);
/// The sha256 the fixture's needs name as their tables'.
const SHA: &str = "fixture-tables-e";
/// `instance-a`'s recognition maintenance package.
const READER: &str = "package.reader";
/// The declared duration of every task, `E` without a snapshot.
const E: u64 = 60_000;

// Test 1 (the study's first R3a test). `task_t` of `V` 900 consumes 1 of the capped pool at a cost
// of 1000 and produces 1 of `pool.tiered`, on which the records need 2: sent once, `B` 1900. Its run
// ends and the capped pool is read again after that end; `pool.tiered`'s reading is the one before
// the run. The next round holds it as not positive (`B` −100) while the goal on `pool.tiered` is
// pending, and it asks for the pool: the recognition maintenance package is requested for its key
// with `fact_stale` (`fact_refresh_requested`) and dispatched, and `task_t` waits for it
// (`fact_refresh_pending`). Before this slice nothing asked: no trigger reads the pool. Once a
// reading observed after the run's end shows the need met, the producer is held as not positive
// again, and asks nothing, as the goal no longer waits on the pool.
#[test]
fn rr3e_t1_a_producer_held_not_positive_asks_for_the_pool_its_pending_goal_is_on() {
    let mut world = World::new(catalog(vec![producer(900, TIERED)])).with_needs(TIERED, 2);
    world.read(READING_KEY, 500, NOW - MINUTE_MS);
    world.read(TIERED_KEY, 0, NOW - MINUTE_MS);
    let sent = world.evaluate();
    assert_eq!(dispatched(&sent), [("task_t", A)], "{sent:#?}");
    world.run(&sent, "task_t");
    let ended = NOW + RUN_MS;
    world.read(READING_KEY, 499, ended + 1_000);

    let asked = world.evaluate();
    assert_eq!(
        dispatched(&asked),
        [(FACT_REFRESH_TASK_ID, A)],
        "{asked:#?}"
    );
    let waits = decision(&asked, "task_t");
    assert!(
        has(waits, PolicyCode::PolicyBenefitNonpositive),
        "{waits:#?}"
    );
    assert_eq!(
        reason(waits, PolicyCode::ResourceTargets).detail,
        "goal_records@pool.tiered:pending reason=expired gap_weight=pending"
    );
    let pending = reason(waits, PolicyCode::FactRefreshPending);
    for (key, value) in [
        ("fact_key", json!(TIERED_KEY)),
        ("unknown_reason", json!("fact_stale")),
        ("package_id", json!(READER)),
    ] {
        assert_eq!(
            pending.outcome.value(key),
            Some(&value),
            "{key}: {pending:?}"
        );
    }
    let requested = reason(
        decision(&asked, FACT_REFRESH_TASK_ID),
        PolicyCode::FactRefreshRequested,
    );
    assert_eq!(
        requested.outcome.value("fact_key"),
        Some(&json!(TIERED_KEY))
    );
    assert_eq!(
        requested.outcome.value("unknown_reason"),
        Some(&json!("fact_stale"))
    );

    world.read(TIERED_KEY, 2, ended + 2_000);
    world.now += MINUTE_MS;
    let met = world.evaluate();
    assert!(dispatched(&met).is_empty(), "{met:#?}");
    let held = decision(&met, "task_t");
    assert!(has(held, PolicyCode::PolicyBenefitNonpositive), "{held:#?}");
    assert!(refresh_codes(held).is_empty(), "{held:#?}");
    assert!(no_reader_decision(&met), "{met:#?}");
}

// Test 2 (the study's second R3a test). `task_t` of `V` 10 000, the same rounds: on the second its
// goal on `pool.tiered` is pending and it passes on its own value (`B` 9000), so it is dispatched
// and asks nothing: no `fact_refresh_*` reason and no decision of the recognition maintenance
// package.
#[test]
fn rr3e_t2_a_producer_that_passes_on_its_own_value_asks_for_nothing() {
    let mut world = World::new(catalog(vec![producer(10_000, TIERED)])).with_needs(TIERED, 2);
    world.read(READING_KEY, 500, NOW - MINUTE_MS);
    world.read(TIERED_KEY, 0, NOW - MINUTE_MS);
    let first = world.evaluate();
    assert_eq!(dispatched(&first), [("task_t", A)], "{first:#?}");
    world.run(&first, "task_t");
    world.read(READING_KEY, 499, NOW + RUN_MS + 1_000);

    let second = world.evaluate();
    assert_eq!(dispatched(&second), [("task_t", A)], "{second:#?}");
    let passes = decision(&second, "task_t");
    assert_eq!(
        reason(passes, PolicyCode::ResourceTargets).detail,
        "goal_records@pool.tiered:pending reason=expired gap_weight=pending"
    );
    assert!(refresh_codes(passes).is_empty(), "{passes:#?}");
    assert!(no_reader_decision(&second), "{second:#?}");
}

// Test 3 (the study's third R3a test). Test 1's rounds with the producer's pool `pool.listed`, read
// from rows of a list (held 0, the list read a minute before NOW): on the second round `task_t` is
// held as not positive while the goal on the pool is pending, and the fact refresh is not asked:
// no `fact_refresh_*` reason, no decision of the recognition maintenance package, nothing
// dispatched. The list's key is left out for its instance with the reading, as before this slice.
#[test]
fn rr3e_t3_a_pool_read_from_rows_of_a_list_is_not_asked_of_the_fact_refresh() {
    let mut world = World::new(catalog(vec![producer(900, LISTED)])).with_needs(LISTED, 2);
    world.read(READING_KEY, 500, NOW - MINUTE_MS);
    world.read_list(0, NOW - MINUTE_MS);
    let sent = world.evaluate();
    assert_eq!(dispatched(&sent), [("task_t", A)], "{sent:#?}");
    world.run(&sent, "task_t");
    world.read(READING_KEY, 499, NOW + RUN_MS + 1_000);

    let after = world.evaluate();
    assert!(dispatched(&after).is_empty(), "{after:#?}");
    let waits = decision(&after, "task_t");
    assert!(
        has(waits, PolicyCode::PolicyBenefitNonpositive),
        "{waits:#?}"
    );
    assert_eq!(
        reason(waits, PolicyCode::ResourceTargets).detail,
        "goal_records@pool.listed:pending reason=expired gap_weight=pending"
    );
    assert!(refresh_codes(waits).is_empty(), "{waits:#?}");
    assert!(no_reader_decision(&after), "{after:#?}");
}

// Test 4 (the study's first and second R3b tests). Test 3's rounds with `task_r`, the reader the
// list is bound to: the first round owes nothing (the pool is read) and sends `task_t`; on the second
// `task_t` is held as not positive with its goal on `pool.listed` pending, so `task_r` is treated as
// triggered (`list_reading_owed`, the list's key and the pool) and dispatched, `task_t` not. Once a
// dispatch of `task_r` is noted after `task_t`'s, the reading is owed no more in that episode: not
// while its run is open, nor once it ended without a reading observed after `task_t`'s end; it is
// held by its own trigger again (`trigger_false`) and nothing is dispatched.
#[test]
fn rr3e_t4_the_reader_of_a_list_is_owed_its_reading_once_while_a_candidate_waits_on_it() {
    for ended in [false, true] {
        let (mut world, owed) = owed_round(900, false);
        assert_eq!(dispatched(&owed), [("task_r", A)], "{owed:#?}");
        let reading = reason(decision(&owed, "task_r"), PolicyCode::ListReadingOwed);
        assert_eq!(reading.outcome.value("fact_key"), Some(&json!(LIST_KEY)));
        assert_eq!(reading.outcome.value("pool_id"), Some(&json!(LISTED)));
        let waits = decision(&owed, "task_t");
        assert!(
            has(waits, PolicyCode::PolicyBenefitNonpositive),
            "{waits:#?}"
        );
        let end = ended.then_some(world.now + RUN_MS);
        world.note("task_r", end);
        world.now += MINUTE_MS;
        let after = world.evaluate();
        assert!(dispatched(&after).is_empty(), "ended {ended}: {after:#?}");
        let reader = decision(&after, "task_r");
        assert!(
            !has(reader, PolicyCode::ListReadingOwed),
            "ended {ended}: {reader:#?}"
        );
        assert!(has(reader, PolicyCode::TriggerFalse), "{reader:#?}");
    }
}

// Test 5 (the study's third R3b test). `task_r` of a cooldown of an hour, dispatched ten minutes
// before NOW (its run noted before `task_t`'s, ended a minute later): on the second round it is owed
// the reading and treated as triggered, but its cooldown holds it, its last reason
// `task_cooldown_active`, waking at the cooldown's end; nothing is dispatched.
#[test]
fn rr3e_t5_the_readers_cooldown_holds_the_owed_reading_with_its_own_reason() {
    let dispatched_at = NOW - 10 * MINUTE_MS;
    let mut world = World::new(catalog(vec![producer(900, LISTED), reader(HOUR_MS)]))
        .with_needs(LISTED, 2)
        .read_by("task_r");
    world.tasks = vec![ran("task_r", dispatched_at)];
    world.note("task_r", Some(dispatched_at + MINUTE_MS));
    world.read(READING_KEY, 500, NOW - MINUTE_MS);
    world.read(TIERED_KEY, 0, NOW - MINUTE_MS);
    world.read_list(0, NOW - MINUTE_MS);
    let sent = world.evaluate();
    assert_eq!(dispatched(&sent), [("task_t", A)], "{sent:#?}");
    world.run(&sent, "task_t");
    world.read(READING_KEY, 499, NOW + RUN_MS + 1_000);

    let owed = world.evaluate();
    assert!(dispatched(&owed).is_empty(), "{owed:#?}");
    let reader = decision(&owed, "task_r");
    assert!(has(reader, PolicyCode::ListReadingOwed), "{reader:#?}");
    assert!(
        last(reader).outcome.is(PolicyCode::TaskCooldownActive),
        "{reader:#?}"
    );
    assert_eq!(reader.state, SchedulingDecisionState::Deferred);
    assert_eq!(reader.next_wake_unix_ms, Some(dispatched_at + HOUR_MS));
}

// Test 6 (the study's fourth R3b test). `task_t` of `V` 10 000, which passes on its own value, and
// `task_w`, whose trigger reads `pool.listed`: on the second round `task_w` waits on the unread pool
// (its trigger undecided, not due) and is the only candidate that does. On an awake instance
// `task_r` is owed the reading, treated as triggered and dispatched ahead of `task_t`. On an instance
// the idle action closed as the cycle starts nothing that waits on the list is due, so nothing is
// owed: `task_r` is held by its trigger and `task_t` is dispatched.
#[test]
fn rr3e_t6_on_an_instance_the_idle_action_closed_a_reading_alone_waits() {
    for closed in [false, true] {
        let mut world = World::new(catalog(vec![
            producer(10_000, LISTED),
            reader(1_000),
            watcher(LISTED),
        ]))
        .with_needs(LISTED, 2)
        .read_by("task_r");
        world.closed = closed;
        world.read(READING_KEY, 500, NOW - MINUTE_MS);
        world.read(TIERED_KEY, 0, NOW - MINUTE_MS);
        world.read_list(0, NOW - MINUTE_MS);
        let sent = world.evaluate();
        assert_eq!(dispatched(&sent), [("task_t", A)], "{sent:#?}");
        world.run(&sent, "task_t");
        world.read(READING_KEY, 499, NOW + RUN_MS + 1_000);

        let second = world.evaluate();
        assert!(
            has(decision(&second, "task_w"), PolicyCode::TriggerUnknown),
            "{second:#?}"
        );
        let owed = has(decision(&second, "task_r"), PolicyCode::ListReadingOwed);
        assert_eq!(owed, !closed, "closed {closed}: {second:#?}");
        let expected = if closed { "task_t" } else { "task_r" };
        assert_eq!(dispatched(&second), [(expected, A)], "{second:#?}");
    }
}

// Test 7 (the study's fifth R3b test). Test 4's rounds with the list bound to no reader (a list a
// direct run published, or one this start saw no scheduled run publish): the second round owes
// nothing; `task_t` is held as not positive, `task_r` by its trigger, and nothing is dispatched.
#[test]
fn rr3e_t7_a_list_without_a_reader_is_owed_nothing() {
    let (_, second) = owed_round(900, true);
    assert!(dispatched(&second).is_empty(), "{second:#?}");
    assert!(has(
        decision(&second, "task_t"),
        PolicyCode::PolicyBenefitNonpositive
    ));
    let reader = decision(&second, "task_r");
    assert!(!has(reader, PolicyCode::ListReadingOwed), "{reader:#?}");
    assert!(has(reader, PolicyCode::TriggerFalse), "{reader:#?}");
}

// Test 8 (the study's sixth R3b test). After test 4's owed round, the list is read again after
// `task_t`'s run ended, the need met (2 held): the pool is read, the goal on it satisfied, `task_t`
// held as not positive on its own value, and `task_r` owed nothing.
#[test]
fn rr3e_t8_a_list_read_after_the_runs_end_releases_the_pool_and_owes_nothing() {
    let (mut world, _) = owed_round(900, false);
    world.read_list(2, NOW + RUN_MS + 2_000);
    world.now += MINUTE_MS;
    let released = world.evaluate();
    assert!(dispatched(&released).is_empty(), "{released:#?}");
    let producing = decision(&released, "task_t");
    assert!(
        reason(producing, PolicyCode::ResourceTargets)
            .detail
            .starts_with("goal_records@pool.listed:satisfied"),
        "{producing:#?}"
    );
    assert!(!has(
        decision(&released, "task_r"),
        PolicyCode::ListReadingOwed
    ));
}

// Test 9 (the work order's first addition, under #455 RR3d's sum). The list read an hour before NOW;
// two dispatches of `task_t`, each told the gap 2 at `run_cap` 1 (1000 each of 2000), noted before
// NOW and ended; the capped pool read after both. After the first the pool is read: `task_t` is sent
// (`B` 1900) and nothing is owed. After the second the pool is unread (2000 ≥ 2000): `task_t` is held
// as not positive and `task_r` is owed one reading, dispatched. Once a dispatch of `task_r` is noted
// after both, nothing is owed, though both still leave the pool unread: one reading for the episode
// the two made together, not one for each.
#[test]
fn rr3e_t9_two_dispatches_that_together_leave_the_pool_unread_owe_one_reading() {
    let mut world = World::new(catalog(vec![producer(900, LISTED), reader(1_000)]))
        .with_needs(LISTED, 2)
        .read_by("task_r");
    world.read(READING_KEY, 500, NOW - 20 * MINUTE_MS);
    world.read(TIERED_KEY, 0, NOW - HOUR_MS);
    world.read_list(0, NOW - HOUR_MS);
    world.told("task_t", NOW - 50 * MINUTE_MS);
    let first = world.evaluate();
    assert_eq!(dispatched(&first), [("task_t", A)], "{first:#?}");
    assert!(!has(
        decision(&first, "task_r"),
        PolicyCode::ListReadingOwed
    ));

    world.told("task_t", NOW - 40 * MINUTE_MS);
    let together = world.evaluate();
    assert_eq!(dispatched(&together), [("task_r", A)], "{together:#?}");
    assert!(has(
        decision(&together, "task_r"),
        PolicyCode::ListReadingOwed
    ));

    world.note("task_r", Some(NOW - 30 * MINUTE_MS));
    let read_once = world.evaluate();
    assert!(dispatched(&read_once).is_empty(), "{read_once:#?}");
    assert!(!has(
        decision(&read_once, "task_r"),
        PolicyCode::ListReadingOwed
    ));
}

// Test 10 (the work order's second addition). Test 4's rounds, the Runtime's admission holding
// `task_r` as the failure pacing of its pair holds it (`instance_task_failure_waiting`, its wait
// ending five minutes after the second round): owed the reading and treated as triggered, it is not
// dispatched before the wait ends, deferred with the pacing's reason and waking at its end; at that
// end it is dispatched, still owed the reading.
#[test]
fn rr3e_t10_a_reader_the_failure_pacing_holds_waits_for_the_pacings_end() {
    let mut world = World::new(catalog(vec![producer(900, LISTED), reader(1_000)]))
        .with_needs(LISTED, 2)
        .read_by("task_r");
    world.read(READING_KEY, 500, NOW - MINUTE_MS);
    world.read(TIERED_KEY, 0, NOW - MINUTE_MS);
    world.read_list(0, NOW - MINUTE_MS);
    let sent = world.evaluate();
    assert_eq!(dispatched(&sent), [("task_t", A)], "{sent:#?}");
    world.run(&sent, "task_t");
    world.read(READING_KEY, 499, NOW + RUN_MS + 1_000);
    let until = world.now + 5 * MINUTE_MS;
    world.paced_until = Some(until);

    let paced = world.evaluate();
    assert!(dispatched(&paced).is_empty(), "{paced:#?}");
    let reader = decision(&paced, "task_r");
    assert!(has(reader, PolicyCode::ListReadingOwed), "{reader:#?}");
    assert_eq!(last(reader).outcome.code().as_str(), PACED, "{reader:#?}");
    assert_eq!(reader.next_wake_unix_ms, Some(until));

    world.now = until;
    let ended = world.evaluate();
    assert_eq!(dispatched(&ended), [("task_r", A)], "{ended:#?}");
    assert!(has(decision(&ended, "task_r"), PolicyCode::ListReadingOwed));
}

/// Test 4's world after its two rounds: `task_t` of `V` `value` producing `pool.listed`, `task_r`
/// the list's reader (bound to none when `unbound`); the first round sent `task_t`, its run ended and
/// the capped pool was read again after that end; the second round's evaluation.
fn owed_round(value: i64, unbound: bool) -> (World, PolicyEvaluation) {
    let mut world =
        World::new(catalog(vec![producer(value, LISTED), reader(1_000)])).with_needs(LISTED, 2);
    if !unbound {
        world = world.read_by("task_r");
    }
    world.read(READING_KEY, 500, NOW - MINUTE_MS);
    world.read(TIERED_KEY, 0, NOW - MINUTE_MS);
    world.read_list(0, NOW - MINUTE_MS);
    let sent = world.evaluate();
    assert_eq!(dispatched(&sent), [("task_t", A)], "{sent:#?}");
    assert!(!has(decision(&sent, "task_r"), PolicyCode::ListReadingOwed));
    world.run(&sent, "task_t");
    world.read(READING_KEY, 499, NOW + RUN_MS + 1_000);
    let second = world.evaluate();
    (world, second)
}

// ---- The world a test drives ----

/// The catalog, the Runtime's memory of the capped episodes, the readings of `instance-a`, the
/// reading given to the pool read from rows of a list, the needs the records compute (by pool and
/// need), the runs the host notes and the evaluation's time.
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    readings: Vec<ObservedFact>,
    listed: GivenPoolReadings,
    needs: Vec<(&'static str, u64)>,
    handed: HandedRuns,
    now: u64,
    /// Ruling 2: the task the list `pool.listed` is read from is bound to, as the host binds a
    /// list a scheduled run of that task published; none: the list has no reader.
    reader: Option<&'static str>,
    /// The task snapshots of the evaluation's facts.
    tasks: Vec<TaskRuntimeSnapshot>,
    /// Whether the idle action closed `instance-a` as the cycle starts (IDLE-1P).
    closed: bool,
    /// Until when the Runtime's admission holds `task_r` as the failure pacing of its pair does.
    paced_until: Option<u64>,
}

impl World {
    /// The world of `instance-a` at NOW: nothing read but the capped pool's capacity, no records,
    /// no run, no reader bound, awake, nothing paced.
    fn new(compiled: CompiledCatalog) -> Self {
        Self {
            catalog: compiled,
            memory: CappedMemory::default(),
            readings: Vec::new(),
            listed: GivenPoolReadings::default(),
            needs: Vec::new(),
            handed: HandedRuns::default(),
            now: NOW,
            reader: None,
            tasks: Vec::new(),
            closed: false,
            paced_until: None,
        }
    }

    /// The same world with records whose computed needs land `need` on `pool`.
    fn with_needs(mut self, pool: &'static str, need: u64) -> Self {
        self.needs.push((pool, need));
        self
    }

    /// The same world with the list `pool.listed` is read from bound to the task `task_id`.
    fn read_by(mut self, task_id: &'static str) -> Self {
        self.reader = Some(task_id);
        self
    }

    /// A dispatch of `task_id` on `instance-a` handed no parameters, noted by the host after every
    /// one noted before it, its run ended at `ended_at_unix_ms` (`None`: still running).
    fn note(&mut self, task_id: &str, ended_at_unix_ms: Option<u64>) {
        self.handed
            .note_dispatch(task_id, A, ended_at_unix_ms, std::iter::empty());
    }

    /// A dispatch of `task_id` on `instance-a` told the gap 2 at its production entry 0 and
    /// `run_cap` 1, noted after every one noted before it, its run ended at `ended_at_unix_ms`.
    fn told(&mut self, task_id: &str, ended_at_unix_ms: u64) {
        let parameters = SpendValues {
            spend_allowances: Vec::new(),
            gaps_remaining: vec![(0, 2)],
            run_cap: Some(1),
        }
        .add_to(Outcome::new(PolicyCode::PolicyDispatchParameters));
        self.handed
            .note_dispatch(task_id, A, Some(ended_at_unix_ms), [&parameters]);
    }

    /// `key` of `instance-a` reads `value`, observed at `at`, in place of its reading.
    fn read(&mut self, key: &str, value: i64, at: u64) {
        self.readings.retain(|held| held.fact_key != key);
        self.readings.push(reading(key, value, at));
    }

    /// The list `pool.listed` is read from was read at `at` and its rows sum to `value`, as the
    /// host gives the reading, valid a day.
    fn read_list(&mut self, value: u64, at: u64) {
        let mut given = GivenPoolReadings::default();
        given.give(
            LISTED,
            GivenPoolReading {
                instance_id: A.to_owned(),
                fact_key: LIST_KEY.to_owned(),
                state: GivenReadingState::Read {
                    value,
                    observed_at_unix_ms: at,
                    expires_at_unix_ms: Some(at + DAY_MS),
                },
            },
        );
        self.listed = given;
    }

    fn facts(&self) -> EvaluationFacts {
        let mut observed = vec![reading(CAPACITY_KEY, CAPACITY, NOW - HOUR_MS)];
        observed.extend(self.readings.iter().cloned());
        EvaluationFacts {
            ledger_position: 7,
            fact_snapshot_id: "snapshot:fixture-455-rr3e".to_owned(),
            facts: observed,
            outcomes: Vec::new(),
            tasks: self.tasks.clone(),
            instances: vec![instance(A, &[])],
            priority_offsets: Vec::new(),
        }
    }

    /// The needs of `instance-a`'s records, of the tables [`SHA`], set an hour before NOW, of no
    /// expiry and with no notes.
    fn record_needs(&self) -> RecordNeeds {
        let landing = self
            .needs
            .iter()
            .map(|(pool, need)| {
                (
                    (*pool).to_owned(),
                    PoolNeed {
                        need: *need,
                        kind: PoolNeedKind::Lands,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut computed = RecordNeeds::default();
        computed.give(
            A,
            InstanceRecordNeeds::new(
                Some(SHA.to_owned()),
                NOW - HOUR_MS,
                landing,
                Vec::new(),
                None,
            ),
        );
        computed
    }

    /// One round now by the Runtime's entry, with its memory, the list's reading (bound to its
    /// reader, if any), the needs and the noted runs beside the left-out facts, the fact refresh on
    /// with `instance-a`'s recognition maintenance package (and `instance-a` closed by the idle
    /// action when [`World::closed`]), every candidate admitted but `task_r` while it is paced.
    fn evaluate(&mut self) -> PolicyEvaluation {
        let facts = self.facts();
        let mut listed = self.listed.clone();
        if let Some(reader) = self.reader {
            listed.bind_readers(|_, _| Some(reader.to_owned()));
        }
        let resources = project_fact_pools_with_readings(
            &self.catalog,
            &facts,
            &EvaluationResources {
                pools: Vec::new(),
                hosts: vec![host()],
            },
            &listed,
        );
        let mut left_out = LeftOutFacts::default();
        left_out.give_pool_readings(listed);
        left_out.give_record_needs(self.record_needs());
        left_out.give_handed_runs(self.handed.clone());
        let mut inputs = refresh_inputs();
        if self.closed {
            inputs.idle_closed.insert(A.to_owned(), idle_closed());
        }
        let (now, paced_until) = (self.now, self.paced_until);
        evaluate_with_left_out_facts::<PolicyEvaluationError>(
            &self.catalog,
            &facts,
            &resources,
            time(self.now),
            11,
            &BTreeSet::new(),
            &inputs,
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
            |intent: &DispatchIntent| Ok(admitted(intent, now, paced_until)),
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
    }

    /// The round's dispatch of `task_id` on `instance-a` ran: it ends half a minute after the
    /// round, the host notes it from its recorded reasons, and the next round is a minute after
    /// this one.
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
}

/// The integer `value` of `key` in the scope of `instance-a`, observed at `at`, of no expiry, at
/// full confidence.
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

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

fn host() -> HostResourceSnapshot {
    HostResourceSnapshot {
        host_id: instance(A, &[]).host_id,
        cpu_available_milli: 1_000,
        gpu_available_milli: 1_000,
        io_available_milli: 1_000,
        host_responsiveness_basis_points: 10_000,
        third_party_pressure_basis_points: 0,
        heavy_dispatch_limit: 1,
        active_heavy_dispatches: 0,
        heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
    }
}

/// The reason and gate of an instance the idle action closed, registered by runtime-host.
const IDLE: &str = "instance_idle_action";
/// The reason the Runtime's admission holds a pair the failure pacing makes wait with, registered
/// by runtime-host.
const PACED: &str = "instance_task_failure_waiting";

/// `instance-a` closed by the idle action as the cycle starts, under the reason and gate the host
/// gives it (IDLE-1P).
fn idle_closed() -> IdleClosed {
    IdleClosed {
        gate: Code::relayed(IDLE).expect("a registered code"),
        reason: DecisionReason {
            outcome: Outcome::relayed(IDLE).alias(A).idle_action("close_app"),
            detail: "closed by the idle action as the cycle starts".to_owned(),
        },
    }
}

/// The Runtime admission's answer for `intent` at `now`: `task_r` waits as its failure pacing
/// makes it wait until `paced_until`; everything else is admitted.
fn admitted(intent: &DispatchIntent, now: u64, paced_until: Option<u64>) -> CandidateEligibility {
    match paced_until {
        Some(until) if intent.task_id == "task_r" && now < until => {
            CandidateEligibility::Deferred {
                reason: DecisionReason {
                    outcome: Outcome::relayed(PACED),
                    detail: format!("fixture admission: {PACED} until {until}"),
                },
                next_wake_unix_ms: Some(until),
            }
        }
        _ => CandidateEligibility::Eligible,
    }
}

/// `task_id`'s latest run on `instance-a`, dispatched at `at`, succeeded.
fn ran(task_id: &str, at: u64) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: task_id.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(at),
        eligible_since_unix_ms: None,
        terminal_state: Some(TaskTerminalState::Succeeded),
        completed_window: None,
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
        clock_owed: false,
    }
}

/// The fact refresh on, `instance-a`'s recognition maintenance package bound, marking
/// `pool.tiered`'s key (a mark never names a list's key).
fn refresh_inputs() -> FactRefreshInputs {
    FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: true,
            min_interval_ms: 300_000,
            daily_limit: 96,
            failure_backoff_ms: 600_000,
            max_consecutive_failures: 3,
            expected_duration_ms: crate::FACT_REFRESH_EXPECTED_DURATION_MS,
        },
        readers: vec![FactRefreshReader {
            instance_id: A.to_owned(),
            package_id: READER.to_owned(),
            marked_keys: BTreeSet::from([TIERED_KEY.to_owned()]),
            dispatched_today: 0,
            last_failure_code: None,
            problem: None,
        }],
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
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}: {evaluation:#?}"))
}

/// Whether no decision of the round is the recognition maintenance package's.
fn no_reader_decision(evaluation: &PolicyEvaluation) -> bool {
    evaluation
        .decisions
        .iter()
        .all(|entry| entry.task_id != FACT_REFRESH_TASK_ID)
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

/// The reason that settled a decision: its last one.
fn last(entry: &TaskDecision) -> &DecisionReason {
    entry
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {entry:#?}"))
}

/// The codes of the decision's reasons that are the fact refresh's (`fact_refresh_*`).
fn refresh_codes(entry: &TaskDecision) -> Vec<&str> {
    entry
        .reasons
        .iter()
        .map(|given| given.outcome.code().as_str())
        .filter(|code| code.starts_with("fact_refresh_"))
        .collect()
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

/// `task_t` of the game, judged by net benefit, of the fixed value `value`, hourly, consuming 1 of
/// the capped pool and producing 1 of `pool` a run.
fn producer(value: i64, pool: &str) -> Value {
    json!({
        "id": "task_t",
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": "procedure.task_t",
        "priority": 100,
        "trigger": hourly(),
        "feedback_stop": {
            "kind": "outcome",
            "task_id": "task_t",
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [effect(POOL, "consume", 1)],
        "produces": [effect(pool, "produce", 1)],
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
        "benefit_judged": true,
        "benefit_fixed_value_milli": value
    })
}

/// A task of the game of priority 100, not judged, consuming and producing nothing, due while `pool`
/// holds at least 1 000 000 (its capacity: never once it is read), of the cooldown `cooldown_ms`.
fn reading_task(id: &str, pool: &str, cooldown_ms: u64) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
        "priority": 100,
        "trigger": {
            "kind": "resource_projection",
            "pool_id": pool,
            "comparison": "greater_than_or_equal",
            "value": 1_000_000
        },
        "feedback_stop": {
            "kind": "outcome",
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
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
        "cooldown_ms": cooldown_ms,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// `task_r`, the list's reader: never due by its own trigger, which reads `pool.tiered` (held 0).
fn reader(cooldown_ms: u64) -> Value {
    reading_task("task_r", TIERED, cooldown_ms)
}

/// `task_w`, which waits on `pool`: its trigger reads it, never true once it is read.
fn watcher(pool: &str) -> Value {
    reading_task("task_w", pool, 1_000)
}

/// A pool of `instance-a` that does not regenerate, of capacity 1 000 000, observed by
/// `observation`.
fn declared_pool(id: &str, observation: Value) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": 1_000_000,
        "projection": {"kind": "none"},
        "observation": observation,
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    })
}

/// A pool of `instance-a` without a valuation, observed by `observation`, writing [`DEFAULTS`] and
/// [`TIERS`].
fn goal_pool(id: &str, observation: Value) -> Value {
    let mut pool = declared_pool(id, observation);
    pool["target_defaults"] = json!({"scale": DEFAULTS.0, "importance_milli": DEFAULTS.1});
    pool["goal_tiers"] = json!({"need_importance_milli": TIERS.0, "stock_cap_milli": TIERS.1});
    pool
}

/// The capacity fact plus `offset`.
fn line(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

/// The fixture's pools: the capped pool, `pool.tiered` and `pool.listed`.
fn pools() -> Value {
    let mut capped = declared_pool(POOL, json!({"kind": "fact", "fact_key": READING_KEY}));
    capped["valuation"] =
        json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000});
    capped["capacity"] = line(0);
    capped["stop_line"] = line(-40);
    capped["hard_line"] = line(-900);
    let listed = json!({
        "kind": "list_rows",
        "fact_key": LIST_KEY,
        "key_column": "id",
        "value_column": "count",
        "rows": [{"key": "item_a", "factor": 1}]
    });
    json!([
        capped,
        goal_pool(TIERED, json!({"kind": "fact", "fact_key": TIERED_KEY})),
        goal_pool(LISTED, listed)
    ])
}

fn catalog(tasks: Vec<Value>) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.455-rr3e",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-rr3e"]
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
        format!("memory://fixture-455-rr3e/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}
