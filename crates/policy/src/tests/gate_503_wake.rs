// SPDX-License-Identifier: AGPL-3.0-only

// #503 WAKE (0.12.101; register §1.549; contracts/scheduling/README.md, "An asleep instance",
// "Below a pool's trigger line"), the policy half: on an asleep instance a judged consumer that
// would pass while a regenerating pool it consumes reads below its trigger line, with no capped
// episode under way, waits for the line (or for the end of its chance less its run), and the round
// goes on to the instance's next candidate; on every other instance nothing changes.
//
// - t1: asleep, the consumer is deferred `instance_asleep_below_trigger_line` until the pool's
//   projection reaches the line, `t0 + p·⌈(T + u − v) ÷ a⌉`, also with a dispatch its reading does
//   not show (`u` 2); no intent; an unrelated task is the task the instance is woken for.
// - t2: the same instance awake: the consumer is dispatched, decision for decision as with no
//   instance named asleep.
// - t3: asleep, read at the trigger line: the episode begins and the consumer heads it as the task
//   the instance is woken for.
// - t4: the chance ends first: a timeline interval, an activity window and an interval clock that
//   end before the line give the release, their end less the run; at the release the consumer
//   passes as today.
// - t5: not held: a line the projection never reaches, a pool that does not regenerate, an episode
//   under way below the line and a hard line; a pool without written lines waits for its capacity.
// - t6: two such consumers on one asleep instance both wait, neither behind the other.
// - t7: the awake case of #455 RR3b A1's numbers (a goal-lifted consumer, B 2) and of a consumer
//   positive on its own value (B 9000): dispatched with A1's values, byte for byte as with no
//   instance named asleep; asleep, the goal-lifted one waits for its hourly clock's next time.
// - t8: an asleep instance woken for another task: at the next evaluation, awake, the consumer
//   below the line is dispatched as on an awake instance.
// - t9 (fix round 1, its regression): a chance whose end, a timeline interval's or an interval
//   clock's, is also the pool's next projection step at the evaluation is released at that end less
//   the run, and at the release the consumer passes; the first build took the end for the pool's
//   step and waited for the line.
// - t10 (fix round 1): a trigger that reads the regenerating pool names only its next step, which
//   ends nothing: the consumer waits for the line.
// - t11 (fix round 1): the same through `all` and `any`, one side naming the pool's step and the
//   other the interval's end at the same time: the end is an end; steps alone are none.
// - t12 (fix round 2, review F3, kept as built): t10's trigger with a reading that expires before
//   the line: the reading's freshness end is an end of the chance, so the release is that end
//   plus one less the run, and at the release the consumer passes.
// - t13 (fix round 2, review F7): a pair whose last settled run was shorter than declared, its
//   snapshot without `E`: judged with the shorter run, released at the window's end less the
//   declared one, the longer, which its dispatch would carry.
//
// The fixture: one instance, a pool regenerating one unit in six minutes, capacity 240, read 100 a
// minute before NOW, written lines 50 (hard), 120 (stop) and 150 (trigger) unless a test says
// otherwise; judged tasks with fixed values consuming 1 of it. Every expected value comes from the
// work order's rulings 1 to 6; no clock is read.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{Initiator, RESOURCE_TARGETS_FACT_KEY, RuntimeChannel};
use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CandidateEligibility, CappedMemory, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DecisionReason, DispatchIntent, EligibilityState, EvaluationFacts,
    EvaluationResources, EvaluationTime, FactRefreshInputs, FactRefreshSettings, FactValue,
    HostResourceSnapshot, InstanceHolds, InstancePoolLines, LeftOutFacts, NoProgressGuards,
    ObservedFact, OwedReruns, PolicyCode, PolicyEvaluation, PolicyEvaluationError, RemedyPlan,
    RunSlots, SCHEDULING_SCHEMA_VERSION_V2, SchedulingDecisionState, ScopeSelector, TargetSetter,
    TaskDecision, TaskRuntimeSnapshot, TaskTerminalState, check_resource_targets, compile_catalog,
    evaluate_with_left_out_facts, parse_resource_targets, project_fact_pools,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// The regenerating pool, read from `REGEN_KEY`: one unit every `STEP_MS`, capacity `CAP`.
const REGEN: &str = "pool.regen";
const REGEN_KEY: &str = "resource.regen";
const STEP_MS: u64 = 6 * MINUTE_MS;
const CAP: u64 = 240;
/// The written lines: hard, stop and trigger.
const LINES: (u64, u64, u64) = (50, 120, 150);
/// The reading and when it was observed.
const READ: i64 = 100;
const READ_AT: u64 = NOW - MINUTE_MS;
/// A pool without a valuation that the quiet trigger reads (read 0, never regenerating).
const OUT: &str = "pool.out";
const OUT_KEY: &str = "resource.out";
/// A pool without a valuation that writes `target_defaults` and `goal_tiers` (#455 RR3b's).
const TIERED: &str = "pool.tiered";
const TIERED_KEY: &str = "resource.tiered";
/// The timeline event of t4: active from 16:00 to 19:00 UTC each day.
const EVENT: &str = "event.window";
/// The declared duration of every task, `E` without a snapshot.
const E: u64 = 60_000;
/// The consumer below the line, of `V` 10 000 (`B` 9000 for its one unit).
const CONSUMER: &str = "task_w";
/// A second consumer, of `V` 8000 (t6).
const SECOND: &str = "task_v";
/// An unjudged task on an hourly clock: the task the instance is woken for.
const OTHER: &str = "task_other";
/// An unjudged task consuming 2 of the pool, dispatched half a minute before NOW (t1, t5).
const SPENT: &str = "task_spent";
/// The time the pool read 100 at `READ_AT` reaches 150 with nothing unshown: 50 steps after it.
const REACHES: u64 = READ_AT + 50 * STEP_MS;

// ---- t1

#[test]
fn wake_t1_an_asleep_consumer_below_the_trigger_line_waits_for_the_line() {
    let catalog = catalog(
        vec![consumer(CONSUMER, 10_000, quiet()), other(), spent()],
        regen_pools(Some(LINES)),
        no_events(),
        0,
    );
    // No run unshown: the release is the line's time.
    let mut world = World::new(catalog.clone(), &[(REGEN_KEY, READ)]);
    let asleep = world.asleep(NOW);
    assert!(asleep.dispatch_intents.is_empty(), "{asleep:#?}");
    assert!(asleep.reason_chains.is_empty(), "{asleep:#?}");
    let waits = decision(&asleep, CONSUMER);
    fixed_first(waits, 9_000, 10_000, E);
    assert_eq!(waits.state, SchedulingDecisionState::Deferred, "{waits:#?}");
    assert_eq!(waits.eligibility, EligibilityState::True, "{waits:#?}");
    assert_below_line(waits, REGEN, (READ, LINES.2), "catalog", REACHES);
    assert_eq!(REACHES, NOW + 17_940_000, "four hours and 59 minutes ahead");
    // The round goes on: the unrelated task is the one the instance is woken for.
    let woken_for = decision(&asleep, OTHER);
    assert!(
        last_is(woken_for, PolicyCode::InstanceAsleepWakeDue),
        "{woken_for:#?}"
    );
    assert!(
        !has(waits, PolicyCode::InstanceAlreadySelected)
            && !has(waits, PolicyCode::InstanceAsleepWakeDue),
        "{waits:#?}"
    );

    // A dispatch of 2 the reading does not show yet (`u` 2): the reading 98, the line two steps
    // later.
    let mut unshown =
        World::new(catalog, &[(REGEN_KEY, READ)]).with_dispatched(SPENT, NOW - 30_000);
    let asleep = unshown.asleep(NOW);
    assert!(asleep.dispatch_intents.is_empty(), "{asleep:#?}");
    assert_below_line(
        decision(&asleep, CONSUMER),
        REGEN,
        (READ - 2, LINES.2),
        "catalog",
        READ_AT + (LINES.2 + 2 - 100) * STEP_MS,
    );
}

// ---- t2

#[test]
fn wake_t2_awake_the_consumer_is_dispatched_as_with_no_instance_asleep() {
    let catalog = catalog(
        vec![consumer(CONSUMER, 10_000, quiet())],
        regen_pools(Some(LINES)),
        no_events(),
        0,
    );
    let mut plain = World::new(catalog.clone(), &[(REGEN_KEY, READ)]);
    let awake = plain.awake(NOW);
    assert_eq!(dispatched(&awake), [CONSUMER], "{awake:#?}");
    fixed_first(decision(&awake, CONSUMER), 9_000, 10_000, E);
    let mut named = World::new(catalog, &[(REGEN_KEY, READ)]);
    let elsewhere = named.evaluate(NOW, &unknown_asleep());
    assert_eq!(bytes(&elsewhere), bytes(&awake), "byte for byte");
    assert_eq!(elsewhere, awake, "decision for decision, every wake too");
    assert!(no_decision_waits_for_the_line(&awake), "{awake:#?}");
}

// ---- t3

#[test]
fn wake_t3_at_the_trigger_line_the_episode_begins_and_the_consumer_is_the_task_woken_for() {
    let catalog = catalog(
        vec![consumer(CONSUMER, 10_000, quiet())],
        regen_pools(Some(LINES)),
        no_events(),
        0,
    );
    let mut world = World::new(catalog, &[(REGEN_KEY, 150)]);
    let asleep = world.asleep(NOW);
    assert!(asleep.dispatch_intents.is_empty(), "{asleep:#?}");
    let woken_for = decision(&asleep, CONSUMER);
    assert!(
        last_is(woken_for, PolicyCode::InstanceAsleepWakeDue),
        "{woken_for:#?}"
    );
    assert!(no_decision_waits_for_the_line(&asleep), "{asleep:#?}");
}

// ---- t4

#[test]
fn wake_t4_a_chance_that_ends_before_the_line_releases_the_consumer_at_its_end_less_its_run() {
    // (trigger, activity window end minute, release, case).
    let cases = [
        // The timeline interval ends at 19:00 UTC, three hours less a minute after NOW.
        (
            timeline(),
            0,
            NOW + 3 * HOUR_MS - MINUTE_MS - E,
            "a timeline interval",
        ),
        // The activity window closes at 17:00 UTC, 59 minutes after NOW.
        (
            quiet(),
            17 * 60,
            NOW + 59 * MINUTE_MS - E,
            "an activity window",
        ),
        // The hourly clock's next occurrence, 59 minutes after NOW: an extra early release.
        (hourly(), 0, NOW + 59 * MINUTE_MS - E, "an interval clock"),
    ];
    for (trigger, window_end, release, case) in cases {
        assert!(release < REACHES, "{case}: before the line");
        let catalog = catalog(
            vec![consumer(CONSUMER, 10_000, trigger)],
            regen_pools(Some(LINES)),
            json!([daily_event()]),
            window_end,
        );
        let mut world = World::new(catalog, &[(REGEN_KEY, READ)]);
        let asleep = world.asleep(NOW);
        let waits = decision(&asleep, CONSUMER);
        assert_eq!(waits.next_wake_unix_ms, Some(release), "{case}: {waits:#?}");
        let said = waits.reasons.last().expect("a reason");
        assert!(
            said.outcome.is(PolicyCode::InstanceAsleepBelowTriggerLine),
            "{case}: {waits:#?}"
        );
        assert_eq!(
            said.outcome.value("next_eligible_unix_ms"),
            Some(&json!(release)),
            "{case}"
        );
        // At the release it passes as today: the task the instance is woken for.
        let at_release = world.asleep(release);
        let woken_for = decision(&at_release, CONSUMER);
        assert!(
            last_is(woken_for, PolicyCode::InstanceAsleepWakeDue),
            "{case}: {woken_for:#?}"
        );
    }
}

// ---- t5

#[test]
fn wake_t5_no_line_to_wait_for_leaves_the_consumer_as_today() {
    // The trigger line at the capacity with 2 unshown: `T + u` above the capacity, never reached.
    let unreachable = catalog(
        vec![consumer(CONSUMER, 10_000, quiet()), spent()],
        regen_pools(Some((LINES.0, LINES.1, CAP))),
        no_events(),
        0,
    );
    let mut world =
        World::new(unreachable, &[(REGEN_KEY, READ)]).with_dispatched(SPENT, NOW - 30_000);
    let asleep = world.asleep(NOW);
    assert!(
        last_is(
            decision(&asleep, CONSUMER),
            PolicyCode::InstanceAsleepWakeDue
        ),
        "the line unreachable: {asleep:#?}"
    );

    // A pool that does not regenerate, with the same lines.
    let mut still = regen_pools(Some(LINES));
    still[0]["projection"] = json!({"kind": "none"});
    let still = catalog(
        vec![consumer(CONSUMER, 10_000, quiet())],
        still,
        no_events(),
        0,
    );
    let mut world = World::new(still, &[(REGEN_KEY, READ)]);
    assert!(
        last_is(
            decision(&world.asleep(NOW), CONSUMER),
            PolicyCode::InstanceAsleepWakeDue
        ),
        "a pool that does not regenerate"
    );

    // An episode under way below the line: begun at 150, read 140 at the next evaluation.
    let lined = || {
        catalog(
            vec![consumer(CONSUMER, 10_000, quiet())],
            regen_pools(Some(LINES)),
            no_events(),
            0,
        )
    };
    let mut world = World::new(lined(), &[(REGEN_KEY, 150)]);
    world.asleep(NOW);
    world.readings = vec![(REGEN_KEY, 140)];
    let under_way = world.asleep(NOW + MINUTE_MS);
    assert!(
        last_is(
            decision(&under_way, CONSUMER),
            PolicyCode::InstanceAsleepWakeDue
        ),
        "an episode under way: {under_way:#?}"
    );
    assert!(no_decision_waits_for_the_line(&under_way));

    // A run that would cross the hard line: held by it, as on the instance awake.
    let mut asleep_world = World::new(lined(), &[(REGEN_KEY, 50)]);
    let mut awake_world = World::new(lined(), &[(REGEN_KEY, 50)]);
    let asleep = asleep_world.asleep(NOW);
    let held = decision(&asleep, CONSUMER);
    assert!(last_is(held, PolicyCode::PoolLineHeld), "{held:#?}");
    assert_eq!(
        held.reasons
            .last()
            .and_then(|reason| reason.outcome.value("pool_line_hold")),
        Some(&json!("would_cross_hard_line"))
    );
    assert_eq!(
        held,
        decision(&awake_world.awake(NOW), CONSUMER),
        "the decision awake, code and wake"
    );

    // A pool without written lines: its trigger line is its capacity, which the rule waits for.
    let unwritten = catalog(
        vec![consumer(CONSUMER, 10_000, quiet())],
        regen_pools(None),
        no_events(),
        0,
    );
    let mut world = World::new(unwritten, &[(REGEN_KEY, 200)]);
    let asleep = world.asleep(NOW);
    assert_below_line(
        decision(&asleep, CONSUMER),
        REGEN,
        (200, CAP),
        "default",
        READ_AT + (CAP - 200) * STEP_MS,
    );
}

// ---- t6

#[test]
fn wake_t6_two_consumers_below_the_line_both_wait_and_neither_waits_for_the_other() {
    let catalog = catalog(
        vec![
            consumer(CONSUMER, 10_000, quiet()),
            consumer(SECOND, 8_000, quiet()),
        ],
        regen_pools(Some(LINES)),
        no_events(),
        0,
    );
    let mut world = World::new(catalog, &[(REGEN_KEY, READ)]);
    let asleep = world.asleep(NOW);
    assert!(asleep.dispatch_intents.is_empty(), "{asleep:#?}");
    for task in [CONSUMER, SECOND] {
        let waits = decision(&asleep, task);
        assert_below_line(waits, REGEN, (READ, LINES.2), "catalog", REACHES);
        assert!(
            !has(waits, PolicyCode::InstanceAlreadySelected),
            "{task}: {waits:#?}"
        );
    }
    assert!(
        !asleep.decisions.iter().any(|entry| entry
            .reasons
            .iter()
            .any(|reason| reason.outcome.is(PolicyCode::InstanceAsleepWakeDue))),
        "nothing wakes the instance: {asleep:#?}"
    );
}

// ---- t7

#[test]
fn wake_t7_the_awake_a1_cases_are_dispatched_as_before_and_asleep_the_goal_lifted_one_waits() {
    // #455 RR3b A1, test (a)'s numbers: `V` 998, 1 of the pool without lines (read 100, capacity
    // 240), 1 of the pool with the member under a stored goal of 200, held 50: `B` 2 below the
    // line. And a consumer positive on its own value, `V` 10 000.
    let lifted = || {
        let compiled = catalog(
            vec![judged(
                "task_r",
                998,
                json!([effect(REGEN, "consume", 1)]),
                json!([effect(TIERED, "produce", 1)]),
                hourly(),
            )],
            a1_pools(),
            no_events(),
            0,
        );
        let stored = goal_record(&compiled, "goal-t", TIERED, 200);
        World::new(compiled, &[(REGEN_KEY, READ), (TIERED_KEY, 50)]).with_goal(&stored)
    };
    let own = || {
        World::new(
            catalog(
                vec![judged(
                    "task_r",
                    10_000,
                    json!([effect(REGEN, "consume", 1)]),
                    json!([]),
                    hourly(),
                )],
                a1_pools(),
                no_events(),
                0,
            ),
            &[(REGEN_KEY, READ)],
        )
    };
    for (case, world, benefit, value) in [
        ("goal-lifted", lifted as fn() -> World, 2, 998),
        ("positive on its own", own as fn() -> World, 9_000, 10_000),
    ] {
        let awake = world().awake(NOW);
        assert_eq!(dispatched(&awake), ["task_r"], "{case}: {awake:#?}");
        fixed_first(decision(&awake, "task_r"), benefit, value, E);
        let elsewhere = world().evaluate(NOW, &unknown_asleep());
        assert_eq!(bytes(&elsewhere), bytes(&awake), "{case}: byte for byte");
        assert_eq!(elsewhere, awake, "{case}: decision for decision");
        assert!(no_decision_waits_for_the_line(&awake), "{case}");
    }

    // Asleep, the goal-lifted consumer waits: below the capacity (no written lines), until its
    // hourly clock's next time less its run, long before the pool fills.
    let asleep = lifted().asleep(NOW);
    let waits = decision(&asleep, "task_r");
    fixed_first(waits, 2, 998, E);
    assert_below_line(
        waits,
        REGEN,
        (READ, CAP),
        "default",
        NOW + 59 * MINUTE_MS - E,
    );
}

// ---- t8

#[test]
fn wake_t8_once_the_instance_is_awake_for_another_task_the_consumer_is_dispatched() {
    let catalog = catalog(
        vec![consumer(CONSUMER, 10_000, quiet()), other()],
        regen_pools(Some(LINES)),
        no_events(),
        0,
    );
    let mut world = World::new(catalog.clone(), &[(REGEN_KEY, READ)]);
    let asleep = world.asleep(NOW);
    assert!(
        last_is(
            decision(&asleep, CONSUMER),
            PolicyCode::InstanceAsleepBelowTriggerLine
        ),
        "{asleep:#?}"
    );
    assert!(
        last_is(decision(&asleep, OTHER), PolicyCode::InstanceAsleepWakeDue),
        "{asleep:#?}"
    );
    // Woken for the other task, which then ran: awake, the consumer is dispatched.
    let after = NOW + 2 * MINUTE_MS;
    let mut woken =
        World::new(catalog.clone(), &[(REGEN_KEY, READ)]).with_dispatched(OTHER, NOW + MINUTE_MS);
    let awake = woken.awake(after);
    assert_eq!(dispatched(&awake), [CONSUMER], "{awake:#?}");
    let mut named =
        World::new(catalog, &[(REGEN_KEY, READ)]).with_dispatched(OTHER, NOW + MINUTE_MS);
    let elsewhere = named.evaluate(after, &unknown_asleep());
    assert_eq!(
        decision(&awake, CONSUMER),
        decision(&elsewhere, CONSUMER),
        "as on an awake instance"
    );
}

// ---- t9, t10, t11 (fix round 1: the end of a chance is known by where it comes from)

/// 18:55 UTC, five minutes before the timeline interval ends at 19:00, 30 steps after the reading:
/// the pool's next projection step is that end.
const AT_EVENT_END: u64 = READ_AT + 175 * MINUTE_MS;
const EVENT_END: u64 = READ_AT + 3 * HOUR_MS;

#[test]
fn wake_t9_a_chance_that_ends_on_a_pool_step_is_released_at_its_end_less_its_run() {
    // Fix round 1's regression. (trigger, evaluation, end of the chance, case): each end is a step
    // of the pool's projection and its next step at the evaluation.
    let cases = [
        (timeline(), AT_EVENT_END, EVENT_END, "a timeline interval"),
        // 16:55: the hourly clock's next occurrence at 17:00, ten steps after the reading.
        (
            hourly(),
            READ_AT + 55 * MINUTE_MS,
            READ_AT + HOUR_MS,
            "an interval clock",
        ),
    ];
    for (trigger, at, end, case) in cases {
        assert!((end - READ_AT).is_multiple_of(STEP_MS), "{case}: a step");
        assert!(end - STEP_MS < at && at < end, "{case}: the next step");
        let release = end - E;
        assert!(at < release && release < REACHES, "{case}: before the line");
        let catalog = catalog(
            vec![consumer(CONSUMER, 10_000, trigger)],
            regen_pools(Some(LINES)),
            json!([daily_event()]),
            0,
        );
        let mut world = World::new(catalog, &[(REGEN_KEY, READ)]);
        let asleep = world.asleep(at);
        assert!(asleep.dispatch_intents.is_empty(), "{case}: {asleep:#?}");
        assert_below_line(
            decision(&asleep, CONSUMER),
            REGEN,
            (projected_at(at), LINES.2),
            "catalog",
            release,
        );
        // At the release, the pool's next step still that end, it passes as today.
        let at_release = world.asleep(release);
        let woken_for = decision(&at_release, CONSUMER);
        assert!(
            last_is(woken_for, PolicyCode::InstanceAsleepWakeDue),
            "{case}: {woken_for:#?}"
        );
    }
}

#[test]
fn wake_t10_a_trigger_that_names_only_the_pools_step_waits_for_the_line() {
    // At least 60 of the regenerating pool, true from the reading on: its wake is the pool's next
    // step alone (the reading has no expiry), so the consumer waits for the line, also where that
    // step falls on a whole hour.
    for at in [NOW, READ_AT + 55 * MINUTE_MS] {
        let catalog = catalog(
            vec![consumer(CONSUMER, 10_000, regen_at_least(60))],
            regen_pools(Some(LINES)),
            no_events(),
            0,
        );
        let mut world = World::new(catalog, &[(REGEN_KEY, READ)]);
        let asleep = world.asleep(at);
        assert!(asleep.dispatch_intents.is_empty(), "{at}: {asleep:#?}");
        assert_below_line(
            decision(&asleep, CONSUMER),
            REGEN,
            (projected_at(at), LINES.2),
            "catalog",
            REACHES,
        );
    }
}

#[test]
fn wake_t11_through_all_and_any_a_pool_step_is_no_end_and_a_time_beside_it_is() {
    // At 18:55 one side names the pool's next step and the other the interval's end, the same
    // instant 19:00: (trigger, release, case).
    let cases = [
        (
            json!({"kind": "all", "predicates": [regen_at_least(60), timeline()]}),
            EVENT_END - E,
            "all: a step and an end",
        ),
        // The pool's side false and read first, the interval's true.
        (
            json!({"kind": "any", "predicates": [regen_at_least(200), timeline()]}),
            EVENT_END - E,
            "any: a step and an end",
        ),
        (
            json!({"kind": "all", "predicates": [regen_at_least(60), quiet()]}),
            REACHES,
            "all: a step alone",
        ),
        (
            json!({"kind": "any", "predicates": [regen_at_least(200), regen_at_least(60)]}),
            REACHES,
            "any: steps alone",
        ),
    ];
    for (trigger, release, case) in cases {
        let catalog = catalog(
            vec![consumer(CONSUMER, 10_000, trigger)],
            regen_pools(Some(LINES)),
            json!([daily_event()]),
            0,
        );
        let mut world = World::new(catalog, &[(REGEN_KEY, READ)]);
        let asleep = world.asleep(AT_EVENT_END);
        assert!(asleep.dispatch_intents.is_empty(), "{case}: {asleep:#?}");
        assert_below_line(
            decision(&asleep, CONSUMER),
            REGEN,
            (projected_at(AT_EVENT_END), LINES.2),
            "catalog",
            release,
        );
        if release < REACHES {
            let at_release = world.asleep(release);
            let woken_for = decision(&at_release, CONSUMER);
            assert!(
                last_is(woken_for, PolicyCode::InstanceAsleepWakeDue),
                "{case}: {woken_for:#?}"
            );
        }
    }
}

/// The regenerating pool's projection at `at` with nothing unshown: its reading and one unit a
/// whole step since.
fn projected_at(at: u64) -> i64 {
    READ + i64::try_from((at - READ_AT) / STEP_MS).expect("a count of steps")
}

// ---- t12, t13 (fix round 2: the independent review's F3 and F7)

#[test]
fn wake_t12_a_trigger_reading_the_consumed_pool_ends_the_chance_at_that_readings_expiry() {
    // Review F3, kept as built: t10's trigger (at least 60 of the regenerating pool), its reading
    // now valid until twenty minutes after NOW. The reading's freshness end is an end of what lets
    // the consumer run, so the release is that end plus one less the run: the instance is woken
    // before the knowledge lapses, never after. At the release, the reading still fresh, the
    // consumer is the task the instance is woken for.
    let expiry = NOW + 20 * MINUTE_MS;
    let release = expiry + 1 - E;
    assert!(NOW < release && release < REACHES, "before the line");
    let catalog = catalog(
        vec![consumer(CONSUMER, 10_000, regen_at_least(60))],
        regen_pools(Some(LINES)),
        no_events(),
        0,
    );
    let mut world = World::new(catalog, &[(REGEN_KEY, READ)]).with_expiry(REGEN_KEY, expiry);
    let asleep = world.asleep(NOW);
    assert!(asleep.dispatch_intents.is_empty(), "{asleep:#?}");
    assert_below_line(
        decision(&asleep, CONSUMER),
        REGEN,
        (READ, LINES.2),
        "catalog",
        release,
    );
    let at_release = world.asleep(release);
    let woken_for = decision(&at_release, CONSUMER);
    assert!(
        last_is(woken_for, PolicyCode::InstanceAsleepWakeDue),
        "{woken_for:#?}"
    );
}

#[test]
fn wake_t13_the_release_leaves_room_for_the_longer_of_the_judged_and_the_dispatched_run() {
    // Review F7: the pair's last settled run took half its declared minute and its snapshot carries
    // no `E`, so the benefit gate judges it with 30 seconds while its dispatch would carry the
    // declared minute, which the host's day-boundary fit checks. t4's activity window closing at
    // 17:00 releases it at that end less the longer of the two, the declared minute.
    let catalog = catalog(
        vec![consumer(CONSUMER, 10_000, quiet())],
        regen_pools(Some(LINES)),
        no_events(),
        17 * 60,
    );
    let mut world =
        World::new(catalog, &[(REGEN_KEY, READ)]).with_dispatched(CONSUMER, NOW - HOUR_MS);
    world.tasks[0].last_duration_ms = Some(E / 2);
    let asleep = world.asleep(NOW);
    let waits = decision(&asleep, CONSUMER);
    fixed_first(waits, 9_000, 10_000, E / 2);
    assert_below_line(
        waits,
        REGEN,
        (READ, LINES.2),
        "catalog",
        NOW + 59 * MINUTE_MS - E,
    );
}

// ---- The world a test drives

/// The catalog, the Runtime's memory of the capped episodes, the pools' readings a minute before
/// NOW (with no expiry but the one `expiry` names), the pairs' snapshots and the instance's goal
/// record.
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    readings: Vec<(&'static str, i64)>,
    expiry: Option<(&'static str, u64)>,
    tasks: Vec<TaskRuntimeSnapshot>,
    goal: Option<ObservedFact>,
}

impl World {
    fn new(catalog: CompiledCatalog, readings: &[(&'static str, i64)]) -> Self {
        Self {
            catalog,
            memory: CappedMemory::default(),
            readings: readings.to_vec(),
            expiry: None,
            tasks: Vec::new(),
            goal: None,
        }
    }

    fn with_goal(mut self, goal: &ObservedFact) -> Self {
        self.goal = Some(goal.clone());
        self
    }

    /// The same world with the reading of `key` expiring at `at` (t12).
    fn with_expiry(mut self, key: &'static str, at: u64) -> Self {
        self.expiry = Some((key, at));
        self
    }

    /// The same world with a run of `task_id` on A dispatched at `at`, settled.
    fn with_dispatched(mut self, task_id: &str, at: u64) -> Self {
        self.tasks.push(TaskRuntimeSnapshot {
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
        });
        self
    }

    fn facts(&self) -> EvaluationFacts {
        let mut observed = vec![fact(A, OUT_KEY, 0, READ_AT, None)];
        observed.extend(self.readings.iter().map(|(key, reading)| {
            let expires = self
                .expiry
                .and_then(|(expiring, at)| (expiring == *key).then_some(at));
            fact(A, key, *reading, READ_AT, expires)
        }));
        observed.extend(self.goal.clone());
        EvaluationFacts {
            facts: observed,
            tasks: self.tasks.clone(),
            ..bare_facts()
        }
    }

    /// One round at `at` by the Runtime's entry, with its memory and the instance holds `holds`,
    /// every candidate admitted.
    fn evaluate(&mut self, at: u64, holds: &InstanceHolds) -> PolicyEvaluation {
        let facts = self.facts();
        let resource_snapshot = resources(&self.catalog, &facts);
        evaluate_with_left_out_facts::<PolicyEvaluationError>(
            &self.catalog,
            &facts,
            &resource_snapshot,
            time(at),
            11,
            &BTreeSet::new(),
            &refresh_inputs(),
            &RemedyPlan::default(),
            &ChainHandoverInputs::default(),
            &RunSlots::default(),
            &OwedReruns::default(),
            holds,
            &NoProgressGuards::default(),
            &mut self.memory,
            &InstancePoolLines::default(),
            &LeftOutFacts::default(),
            |_, _, _| Ok(false),
            |_: &DispatchIntent| Ok(CandidateEligibility::Eligible),
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
    }

    /// One round at `at` with A asleep.
    fn asleep(&mut self, at: u64) -> PolicyEvaluation {
        let mut holds = InstanceHolds::default();
        holds.mark_asleep(A);
        self.evaluate(at, &holds)
    }

    /// One round at `at` with no instance asleep.
    fn awake(&mut self, at: u64) -> PolicyEvaluation {
        self.evaluate(at, &InstanceHolds::default())
    }
}

/// The holds with an instance the round does not know named asleep.
fn unknown_asleep() -> InstanceHolds {
    let mut holds = InstanceHolds::default();
    holds.mark_asleep("instance-unknown");
    holds
}

/// The instance with no fact.
fn bare_facts() -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-503-wake".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

/// The v3 goal record of one target `id` on `pool`, at least `at_least`, checked against
/// `compiled` and stored as the formal entry stores it for a Lab connection, an hour before NOW
/// (#455 RR3b's).
fn goal_record(compiled: &CompiledCatalog, id: &str, pool: &str, at_least: u64) -> ObservedFact {
    let document = json!({
        "schema_version": "actingcommand.resource-targets.v3",
        "instance": A,
        "targets": [{
            "id": id,
            "resource": pool,
            "condition": {"kind": "at_least", "amount": at_least},
            "apply": {"mode": "adjust", "weight": "score_stage"}
        }]
    });
    let parsed = parse_resource_targets(&serde_json::to_vec(&document).expect("document bytes"))
        .unwrap_or_else(|error| panic!("the goal parses: {error:?}"));
    let set_at = NOW - HOUR_MS;
    let accepted = check_resource_targets(&parsed, compiled, &bare_facts(), time(set_at))
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

// ---- Assertions

fn bytes(evaluation: &PolicyEvaluation) -> Vec<u8> {
    serde_json::to_vec(evaluation).expect("the evaluation serializes")
}

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

fn last_is(entry: &TaskDecision, code: PolicyCode) -> bool {
    entry
        .reasons
        .last()
        .is_some_and(|given| given.outcome.is(code))
}

fn no_decision_waits_for_the_line(evaluation: &PolicyEvaluation) -> bool {
    !evaluation
        .decisions
        .iter()
        .any(|entry| has(entry, PolicyCode::InstanceAsleepBelowTriggerLine))
}

/// The decision opens with exactly one `benefit_fixed_value`, of `B`, `V` and `E`.
fn fixed_first(entry: &TaskDecision, benefit: i64, value: i64, expected_ms: u64) {
    let first: &DecisionReason = entry
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

/// The decision is deferred, eligible, its last reason `instance_asleep_below_trigger_line` with
/// its values, its wake `release`; no dispatch reason.
fn assert_below_line(
    entry: &TaskDecision,
    pool: &str,
    (reading, trigger): (i64, u64),
    source: &str,
    release: u64,
) {
    assert_eq!(entry.state, SchedulingDecisionState::Deferred, "{entry:#?}");
    assert_eq!(entry.eligibility, EligibilityState::True, "{entry:#?}");
    assert_eq!(entry.next_wake_unix_ms, Some(release), "{entry:#?}");
    let said = &entry.reasons.last().expect("a reason").outcome;
    assert!(
        said.is(PolicyCode::InstanceAsleepBelowTriggerLine),
        "{entry:#?}"
    );
    for (key, expected) in [
        ("instance_id", json!(A)),
        ("pool_id", json!(pool)),
        ("pool_reading", json!(reading)),
        ("trigger_line", json!(trigger)),
        ("next_eligible_unix_ms", json!(release)),
        ("lines_source", json!(source)),
    ] {
        assert_eq!(said.value(key), Some(&expected), "{key}: {entry:#?}");
    }
    assert!(
        !has(entry, PolicyCode::PlacementSelected) && !has(entry, PolicyCode::DecisionRecord),
        "{entry:#?}"
    );
}

// ---- The catalog

fn effect(pool: &str, direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// Due every hour (one minute past the hour at NOW), its next occurrence 59 minutes ahead.
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

/// True and naming no time: the pool that never regenerates holds at least 0.
fn quiet() -> Value {
    json!({
        "kind": "resource_projection",
        "pool_id": OUT,
        "comparison": "greater_than_or_equal",
        "value": 0
    })
}

/// True while the regenerating pool's projection holds at least `value`, naming only the pool's
/// next projection step (its reading has no expiry).
fn regen_at_least(value: u64) -> Value {
    json!({
        "kind": "resource_projection",
        "pool_id": REGEN,
        "comparison": "greater_than_or_equal",
        "value": value
    })
}

/// True while [`EVENT`] is active, naming the end of its interval.
fn timeline() -> Value {
    json!({"kind": "timeline_active", "event_id": EVENT})
}

/// [`EVENT`]: active three hours from 16:00 UTC every day.
fn daily_event() -> Value {
    json!({
        "id": EVENT,
        "scope": {"kind": "instance", "instance_id": A},
        "event_kind": "activity",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": DAY_MS,
            "anchor_ms": 16 * HOUR_MS
        },
        "duration_ms": 3 * HOUR_MS,
        "invalidates_fact_prefixes": [],
        "validity": {"from_unix_ms": 0, "until_unix_ms": null}
    })
}

fn no_events() -> Value {
    json!([])
}

/// A task of the game on `trigger`, of priority 100, unmarked.
fn task(id: &str, trigger: Value) -> Value {
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
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// A judged task on `trigger` of the fixed value `value`, with `consumes` and `produces`.
fn judged(id: &str, value: i64, consumes: Value, produces: Value, trigger: Value) -> Value {
    let mut spec = task(id, trigger);
    spec["benefit_judged"] = json!(true);
    spec["benefit_fixed_value_milli"] = json!(value);
    spec["consumes"] = consumes;
    spec["produces"] = produces;
    spec
}

/// A judged consumer of 1 of the regenerating pool a run, of `V` `value`.
fn consumer(id: &str, value: i64, trigger: Value) -> Value {
    judged(
        id,
        value,
        json!([effect(REGEN, "consume", 1)]),
        json!([]),
        trigger,
    )
}

/// The unjudged task on an hourly clock the instance is woken for.
fn other() -> Value {
    task(OTHER, hourly())
}

/// An unjudged task consuming 2 of the regenerating pool, never a candidate: its trigger asks the
/// pool read 0 for at least 1.
fn spent() -> Value {
    let never = json!({
        "kind": "resource_projection",
        "pool_id": OUT,
        "comparison": "greater_than_or_equal",
        "value": 1
    });
    let mut spec = task(SPENT, never);
    spec["consumes"] = json!([effect(REGEN, "consume", 2)]);
    spec
}

/// A pool of A that does not regenerate, read from `key`, of constant `capacity`, without a
/// valuation.
fn declared_pool(id: &str, key: &str, capacity: u64) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": capacity,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": key},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    })
}

/// The regenerating pool, valued 1000 milli a unit at the step 1, with `lines` (hard, stop,
/// trigger) written as constants when given, first; then the pool the quiet trigger reads.
fn regen_pools(lines: Option<(u64, u64, u64)>) -> Value {
    let mut regen = declared_pool(REGEN, REGEN_KEY, CAP);
    regen["projection"] = json!({"amount": 1, "per_ms": STEP_MS});
    regen["valuation"] =
        json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000});
    if let Some((hard, stop, trigger)) = lines {
        regen["hard_line"] = json!(hard);
        regen["stop_line"] = json!(stop);
        regen["trigger_line"] = json!(trigger);
    }
    json!([regen, declared_pool(OUT, OUT_KEY, 1_000_000)])
}

/// t7: the regenerating pool without lines, the pool with the member (#455 RR3b's `S` 10, `I`
/// 1000, `I_n` 100 000 and `C` 40) and the pool the quiet trigger reads.
fn a1_pools() -> Value {
    let mut pools = regen_pools(None);
    let mut tiered = declared_pool(TIERED, TIERED_KEY, 1_000_000);
    tiered["target_defaults"] = json!({"scale": 10, "importance_milli": 1000});
    tiered["goal_tiers"] = json!({"need_importance_milli": 100_000, "stock_cap_milli": 40});
    pools.as_array_mut().expect("the pool list").push(tiered);
    pools
}

/// The catalog of `tasks` over `pool_list` and `events`, its activity window open from midnight to
/// `window_end_minute` UTC every day (0: all day).
fn catalog(
    tasks: Vec<Value>,
    pool_list: Value,
    events: Value,
    window_end_minute: u16,
) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.503-wake",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-503-wake"]
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
            "end_minute_of_day": window_end_minute
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
    let source = |name: &str, document: &Value| {
        CatalogDocumentSource::new(
            format!("memory://fixture-503-wake/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", pool_list)),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", events)),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}
