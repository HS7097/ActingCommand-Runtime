// SPDX-License-Identifier: AGPL-3.0-only

//! #484 IDLE-W2 (0.12.75; register §1.488; `contracts/runtime-config.md`, "What wakes an idle
//! instance"): an idle wake only for a task that can be dispatched now. Production 0.12.74 on
//! 11-19 from 08:00 to 08:12: an instance whose application the idle check had closed (its next
//! work ten hours away) was started cold again half a second later, found nothing to do and was
//! closed again, seven times in twelve minutes. A candidate past its trigger is deferred by the
//! first reason of the order of the deferrals that applies; a pause (position 1) and a held key
//! (position 2) come before the budget, the backoff and the interval (position 8) and defer it with
//! no wake of its own, which the idle plan counted as due.
//!
//! Every policy scenario runs one task whose trigger is always true (an interval clock of 1 ms, its
//! `daily_limit` 2) on the fixture instance of `gate_372k2` (activity window 06:00-23:30 UTC, a
//! manual Runtime clock, the no-progress guard off), reads its idle plan as the idle check reads it
//! (`RuntimeHost::idle_plan_and_runs_counted_for_test`) and feeds it to the pure idle planner under
//! the production keys (`close_app`, its wake lead 300 000 ms, `idle.min_gap_ms` 1 800 000 ms). A
//! client's lease stands for every claim on the key (the close's own, a Lab run's, a start's
//! preparation): the evaluation reads them all as one occupancy, position 2.
//!
//! - The reproduction (red on the code of 0.12.74): the task's budget spent, the idle check closes
//!   the application with its wake at the budget's reset less the lead; while a client holds the
//!   instance's key the evaluation defers the task `dispatch_instance_held` with no wake, and the
//!   idle plan counts it not due, its wake at the reset: no wake follows the close.
//! - The plan: behind a held key and behind a pause, a spent budget, a backoff and the activity
//!   interval each leave the task not due with the release the free evaluation names as its wake;
//!   a task really due is due behind either hold, and the plan names it.
//! - The restart: a pause restored by the root's seeding start and then by a loading start, and a
//!   claim on the key after the resume: with only the spent budget's candidate, the first checks of
//!   an application closed as idle (restored as a valid `host.idle_state` restores it) queue no wake,
//!   and its planned wake is kept.
//! - The replay of the study's first timeline (08:00 to 08:12) on the manual clock: one close, no
//!   wake under any of the claims or after them, and the wake at its planned time, which it names.
//! - The wake's cause, in the planner: a task due, the action changed, a wake the start owed
//!   (restored woken, or with no usable record) and the planned time, the first that holds.
//! - The host (the scripted emulator of `gate_idle1_host`, X's idle action `stop_emulator`): after
//!   X's action, a check while a run holds X's admission guard reads a plan with nothing due and
//!   queues no wake; a plan with a task due queues one, and its record names `task_due` and the
//!   task.
//!
//! Fix round 1 (review-1845):
//! - A cooldown (a hold with a time of its own) in front of the spent budget, on a closed
//!   instance whose key is held: no wake at the cooldown's end; the wake stays the reset less the
//!   lead, and a later position that names no time leaves no wake from the candidate.
//! - A look ahead that fails (a test switch, `LOOK_AHEAD_FAILS_FOR_TEST`) leaves the candidate due
//!   and its failure in the daemon's log as a skipped background step.
//! - The host with its own plan and its own planner (no plan handed in; every check is the one
//!   the host's own evaluated cycle runs, `RuntimeHost::evaluate_policy_cycle`): the fixture
//!   instance with the idle action `close_app` (emulator-controlled, its startup package, a
//!   standard pack root that declares its application). The close carried out, an evaluation
//!   inside its own claim (a test hook holds the claim once the planner knows the close's end), a
//!   client's tap the closed application refuses `application_not_foreground` and the evaluations
//!   under and after that client's lease queue no wake; the task due again after its reset
//!   queues one whose record names `task_due` and the task. And a restart: the close and a pause
//!   restored by a seeding and a loading start, the first evaluations under the pause and under a
//!   claim after the resume queue no wake, and the planned wake is kept. What the fixtures cannot
//!   give: an evaluation inside a start's own preparation, which runs inside `RuntimeHost::start`
//!   before it returns (and is skipped for an instance whose pause the start restored); a
//!   client's lease after the resume stands for it.
//!
//! Fix round 2 (CI of fix round 1): the restart's first evaluation under the restored pause queued
//! a wake, `planned_time_reached`. The start had withheld the instance's availability and skipped
//! its preparation, so the evaluation blocked the task at placement (`instance_unavailable`) before
//! any hold, the look ahead never read it and the plan took its trigger's next time as its wake. The
//! restart test is the proof of the fix (`PolicyHost::evaluate`: such a candidate of an instance
//! the idle action closed, under a hold, counts as held back with no time); its record lists and
//! those of the close's test print the outcome code and operation of each record they found.
//!
//! Fix round 3 (review-1845-b): the restart test asserts that the first evaluation under the
//! restored pause blocks the task at placement with `instance_unavailable`, so its plan is the one
//! fix round 2's path reads.

use super::gate_369_s6c::{connectivity, head};
use super::gate_372k2::{SlotTask, Slots, at, slot_sources};
use super::gate_idle1p::kept_rows;
use super::instance_workers::{all_events, phases};
use super::*;
use crate::application_update::IdleUpdateHold;
use crate::codes::{HostCode, IdleCode, IdleLocation, IdleWakeCause};
use crate::idle_action::{IdleEnd, IdlePlan, IdlePlanner, IdleStep, IdleView, KeptIdle, WakeWork};
use crate::policy_host::LOOK_AHEAD_FAILS_FOR_TEST;
use crate::schedule_explanation::{
    IdleGates, IdleHeldAhead, IdleHeldBack, ScheduleExplanationTable,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, FactScalar, InstanceStuckRecovery,
    RuntimeConfigManifest, RuntimeLifecyclePhase,
};
use actingcommand_host_metrics::NetworkConnectivity;
use actingcommand_policy::{
    DecisionReason, EligibilityState, PolicyCode, PolicyEvaluation, SchedulingDecisionState,
    TaskDecision,
};
use actingcommand_runtime_config::{
    DefaultsFile, IdleAction, IdleConfig, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};
use std::sync::Mutex;

const TASK: &str = "fixture.idle_w2";
const ALIAS: &str = POLICY_INSTANCE_ALIAS;
const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
/// `idle.close_app_wake_lead_ms` in production.
const LEAD_MS: u64 = 300_000;
/// The production keys of 11-19: `close_app`, its wake lead, `idle.min_gap_ms` 1 800 000.
const PRODUCTION: IdleConfig = IdleConfig {
    action: IdleAction::CloseApp,
    min_gap_ms: 1_800_000,
    close_app_wake_lead_ms: LEAD_MS,
    stop_emulator_wake_lead_ms: LEAD_MS,
};
const WAKE: IdleStep = IdleStep::Wake {
    action: IdleAction::CloseApp,
    work: &[WakeWork::StartupPackage],
};

/// The one task `TASK` on the fixture instance: its trigger always true, `daily_limit` 2, under
/// the activity interval `interval_ms`.
fn sources(interval_ms: u64) -> CatalogSources {
    let mut sources = slot_sources(
        1,
        &[SlotTask {
            id: TASK,
            priority: 100,
            trigger: json!({
                "kind": "clock",
                "schedule": {
                    "kind": "interval",
                    "clock_source": {"kind": "local"},
                    "every_ms": 1,
                    "anchor_ms": 0
                }
            }),
            daily_limit: 2,
        }],
        None,
    );
    let mut activity: serde_json::Value =
        serde_json::from_slice(&sources.activity.bytes).expect("the fixture's activity");
    activity["profiles"][0]["minimum_interval_ms"] = json!(interval_ms);
    activity["profiles"][0]["maximum_interval_ms"] = json!(interval_ms);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("the activity bytes");
    sources
}

/// The instance free, its emulator running and its startup package configured.
fn free() -> IdleView {
    IdleView {
        emulator_on: true,
        app_wakeable: true,
        emulator_wakeable: true,
        ..IdleView::default()
    }
}

/// A claim holds the instance's key.
fn leased() -> IdleView {
    IdleView {
        leased: true,
        ..free()
    }
}

/// The cycle's decision of `TASK` on the fixture instance.
fn decision(cycle: &PolicyCycle) -> &TaskDecision {
    let evaluation = cycle.evaluation.as_ref().expect("an evaluated cycle");
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == TASK && decision.instance_id.as_deref() == Some(ALIAS))
        .unwrap_or_else(|| panic!("the task's decision: {evaluation:#?}"))
}

/// `TASK` is eligible and deferred with `code` as its last reason: its wake.
fn deferred_by(cycle: &PolicyCycle, code: HostCode) -> Option<u64> {
    let decision = decision(cycle);
    assert_eq!(
        decision.state,
        SchedulingDecisionState::Deferred,
        "{decision:#?}"
    );
    assert_eq!(
        decision.eligibility,
        EligibilityState::True,
        "{decision:#?}"
    );
    assert!(
        decision
            .reasons
            .last()
            .is_some_and(|reason| reason.outcome.is(code)),
        "deferred by {}: {decision:#?}",
        code.as_str()
    );
    decision.next_wake_unix_ms
}

/// The instance's idle plan, as the idle check reads it.
fn plan(slots: &Slots) -> IdlePlan {
    slots
        .host
        .idle_plan_and_runs_counted_for_test(ALIAS)
        .expect("the idle plan")
        .0
        .expect("an evaluated plan")
}

/// The task the instance's idle plan counts due, as the idle check reads it.
fn due_task(slots: &Slots) -> Option<String> {
    slots
        .host
        .idle_due_task_for_test(ALIAS)
        .expect("the idle plan's due task")
}

/// Evaluates at `unix_ms`, admits the task's dispatch the cycle selected and records its run as a
/// recoverable failure, which schedules a retry after the task's backoff (30 s); its lease ends.
fn fail_once(slots: &mut Slots, unix_ms: u64) {
    let cycle = slots.evaluate(unix_ms);
    let evaluation = cycle.evaluation.as_ref().expect("an evaluated cycle");
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == TASK)
        .unwrap_or_else(|| panic!("the task is selected at {unix_ms}: {evaluation:#?}"))
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the dispatch's reason chain")
        .clone();
    record_policy_approval(&slots.host, &intent);
    let PolicyDispatchAdmission::Granted { context } = slots
        .host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&slots.host, &intent))
        .unwrap_or_else(|error| panic!("admit the task at {unix_ms}: {error}"))
    else {
        panic!("the task is granted at {unix_ms}");
    };
    slots
        .host
        .record_policy_dispatch_outcome(
            &intent.decision_id,
            &PolicyExecutionInput::Failed {
                outcome: Outcome::new(RuntimeErrorCode::CaptureFailed),
                failure_identity: None,
                class: PolicyFailureClass::Recoverable,
            },
        )
        .expect("record the failed run");
    slots
        .host
        .release_policy_dispatch_lease_for_test(&context)
        .expect("end the dispatch's lease");
}

#[test]
fn idle_w2_a_held_key_does_not_make_a_task_whose_budget_is_spent_due() {
    let mut slots = Slots::start_without_no_progress_guard(&sources(1), at(1, 6, 0));
    // The day's two runs spend the task's budget.
    slots.dispatch(at(1, 6, 10), TASK);
    slots.dispatch(at(1, 6, 20), TASK);

    // Free, the budget defers the task to its reset and nothing is due: the idle check closes the
    // application, its wake planned at the reset less the lead.
    let closed_at = at(1, 6, 30);
    let free_cycle = slots.evaluate(closed_at);
    let reset = deferred_by(&free_cycle, HostCode::PolicyBudgetExhausted)
        .expect("the budget names its reset");
    let free_plan = plan(&slots);
    assert!(!free_plan.due_now, "nothing is due: {free_plan:?}");
    assert_eq!(free_plan.next_wake_unix_ms, Some(reset), "{free_plan:?}");
    let mut planner = IdlePlanner::default();
    assert_eq!(
        planner.check(ALIAS, &PRODUCTION, free(), Some(free_plan), closed_at),
        Some(IdleStep::Act {
            action: IdleAction::CloseApp,
            next_due_unix_ms: Some(reset),
            wake_at_unix_ms: Some(reset - LEAD_MS),
        }),
        "the idle check closes the application"
    );
    planner.finished(ALIAS, IdleEnd::Done);

    // Half a second later a client holds the key (in production the close's own claim, a Lab
    // run): the hold defers the task with no wake, and the plan does not count it due.
    let held_at = closed_at + 500;
    let holder = slots.hold();
    let held_cycle = slots.evaluate(held_at);
    assert_eq!(
        deferred_by(&held_cycle, HostCode::DispatchInstanceHeld),
        None,
        "the hold names no wake"
    );
    let held_plan = plan(&slots);
    assert!(
        !held_plan.due_now,
        "a task whose budget is spent is not due behind the hold: {held_plan:?}"
    );
    assert_eq!(
        held_plan.next_wake_unix_ms,
        Some(reset),
        "it wakes at the budget's reset: {held_plan:?}"
    );
    assert_eq!(
        planner.check(ALIAS, &PRODUCTION, leased(), Some(held_plan), held_at),
        None,
        "no wake follows the close"
    );
    assert_eq!(
        planner.planned_wake(ALIAS),
        Some(reset - LEAD_MS),
        "the planned wake is kept"
    );
    Slots::release(holder);
    slots.close();
}

/// The later gate a case leaves the task held back by, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Later {
    /// The day's two runs spent the task's budget (`policy_budget_exhausted`).
    Budget,
    /// The task's run failed and waits for its retry (`policy_retry_backoff_active`).
    Backoff,
    /// The task ran and the activity interval (one hour) has not passed
    /// (`policy_activity_interval_active`).
    Interval,
    /// Nothing holds the task back: it is due.
    Nothing,
}

/// A host on which the task stands as `later` leaves it, the time it stands so at, and the wake
/// the free evaluation there names (none for `Nothing`, which is not evaluated free: it would be
/// dispatched).
fn standing(later: Later) -> (Slots, u64, Option<u64>) {
    match later {
        Later::Budget => {
            let mut slots = Slots::start_without_no_progress_guard(&sources(1), at(1, 6, 0));
            slots.dispatch(at(1, 6, 10), TASK);
            slots.dispatch(at(1, 6, 20), TASK);
            let free_at = at(1, 6, 30);
            let wake = deferred_by(&slots.evaluate(free_at), HostCode::PolicyBudgetExhausted);
            (slots, free_at, wake)
        }
        Later::Backoff => {
            let mut slots = Slots::start_without_no_progress_guard(&sources(1), at(1, 6, 0));
            fail_once(&mut slots, at(1, 6, 10));
            let free_at = at(1, 6, 10) + 5_000;
            let wake = deferred_by(&slots.evaluate(free_at), HostCode::PolicyRetryBackoffActive);
            (slots, free_at, wake)
        }
        Later::Interval => {
            let mut slots = Slots::start_without_no_progress_guard(&sources(HOUR_MS), at(1, 6, 0));
            slots.dispatch(at(1, 6, 10), TASK);
            let free_at = at(1, 6, 11);
            let wake = deferred_by(
                &slots.evaluate(free_at),
                HostCode::PolicyActivityIntervalActive,
            );
            (slots, free_at, wake)
        }
        Later::Nothing => (
            Slots::start_without_no_progress_guard(&sources(1), at(1, 6, 0)),
            at(1, 6, 10),
            None,
        ),
    }
}

#[test]
fn idle_w2_behind_a_hold_a_candidate_counts_as_the_later_gates_answer_it() {
    for later in [
        Later::Budget,
        Later::Backoff,
        Later::Interval,
        Later::Nothing,
    ] {
        for paused in [false, true] {
            let what = format!("{later:?}, paused {paused}");
            let (mut slots, free_at, wake) = standing(later);
            let held_at = free_at + 1_000;
            let holder = (!paused).then(|| slots.hold());
            if paused {
                slots.pause();
            }
            let hold = if paused {
                HostCode::DispatchPausedInstance
            } else {
                HostCode::DispatchInstanceHeld
            };
            assert_eq!(
                deferred_by(&slots.evaluate(held_at), hold),
                None,
                "{what}: the hold names no wake"
            );
            let read = plan(&slots);
            if later == Later::Nothing {
                assert!(
                    read.due_now,
                    "{what}: a task no later gate holds back is due: {read:?}"
                );
                assert_eq!(
                    due_task(&slots),
                    Some(TASK.to_owned()),
                    "{what}: the plan names it"
                );
            } else {
                assert!(wake.is_some(), "{what}: the gate names its release");
                assert!(!read.due_now, "{what}: not due behind the hold: {read:?}");
                assert_eq!(
                    read.next_wake_unix_ms, wake,
                    "{what}: its wake is the gate's release"
                );
                assert_eq!(due_task(&slots), None, "{what}: no task is due");
            }
            if let Some(holder) = holder {
                Slots::release(holder);
            }
            slots.close();
        }
    }
}

#[test]
fn idle_w2_after_a_restart_a_restored_pause_and_a_held_key_keep_the_application_closed() {
    let mut slots = Slots::start_without_no_progress_guard(&sources(1), at(1, 6, 0));
    slots.dispatch(at(1, 6, 10), TASK);
    slots.dispatch(at(1, 6, 20), TASK);
    let reset = deferred_by(
        &slots.evaluate(at(1, 6, 30)),
        HostCode::PolicyBudgetExhausted,
    )
    .expect("the budget names its reset");
    // The application closed as idle before the stop, its wake at the reset less the lead: what a
    // start restores from a valid `host.idle_state`.
    let kept = KeptIdle {
        action: IdleAction::CloseApp,
        wake_at_unix_ms: Some(reset - LEAD_MS),
        woken: false,
    };
    slots.pause();
    // The root's second start seeds (it replays the history), its third loads the decision memory
    // from its value records, as every restart of an installation does; each restores the pause.
    let slots = slots.restart(at(1, 7, 0));
    let mut slots = slots.restart(at(1, 7, 1));
    let started = at(1, 7, 1);
    let mut planner = IdlePlanner::default();
    planner.restore(ALIAS, kept);
    let paused = IdleView {
        paused: true,
        ..free()
    };
    assert_eq!(
        planner.check(ALIAS, &PRODUCTION, paused, None, started),
        None,
        "the start's first check, before any evaluation"
    );

    // The first evaluation under the restored pause.
    let first = started + 1_000;
    assert_eq!(
        deferred_by(&slots.evaluate(first), HostCode::DispatchPausedInstance),
        None,
        "the restored pause holds the task with no wake"
    );
    let under_pause = plan(&slots);
    assert!(!under_pause.due_now, "{under_pause:?}");
    assert_eq!(
        under_pause.next_wake_unix_ms,
        Some(reset),
        "{under_pause:?}"
    );
    assert_eq!(
        planner.check(ALIAS, &PRODUCTION, paused, Some(under_pause), first),
        None,
        "no wake under the restored pause"
    );
    assert_eq!(planner.planned_wake(ALIAS), Some(reset - LEAD_MS));

    // The resume, then a claim on the key, as a start's preparation holds it.
    slots.resume();
    let holder = slots.hold();
    let held = started + 2_000;
    assert_eq!(
        deferred_by(&slots.evaluate(held), HostCode::DispatchInstanceHeld),
        None,
        "the claim holds the task with no wake"
    );
    let held_plan = plan(&slots);
    assert!(!held_plan.due_now, "{held_plan:?}");
    assert_eq!(held_plan.next_wake_unix_ms, Some(reset), "{held_plan:?}");
    assert_eq!(
        planner.check(ALIAS, &PRODUCTION, leased(), Some(held_plan), held),
        None,
        "no wake under the claim"
    );
    assert_eq!(
        planner.planned_wake(ALIAS),
        Some(reset - LEAD_MS),
        "the planned wake is kept"
    );
    Slots::release(holder);
    slots.close();
}

#[test]
fn idle_w2_replay_of_the_first_timeline_closes_once_and_wakes_at_its_planned_time() {
    let mut slots = Slots::start_without_no_progress_guard(&sources(1), at(1, 6, 0));
    // 06:29:59 and 06:33:45, the task's two dispatches of the day: its budget is spent.
    slots.dispatch(at(1, 6, 29) + 59_000, TASK);
    slots.dispatch(at(1, 6, 33) + 45_000, TASK);
    let mut planner = IdlePlanner::default();

    // 08:00:02.4: the idle check closes the application.
    let closed_at = at(1, 8, 0) + 2_400;
    let reset = deferred_by(&slots.evaluate(closed_at), HostCode::PolicyBudgetExhausted)
        .expect("the budget names its reset");
    let Some(IdleStep::Act {
        action: IdleAction::CloseApp,
        wake_at_unix_ms: Some(wake),
        ..
    }) = planner.check(ALIAS, &PRODUCTION, free(), Some(plan(&slots)), closed_at)
    else {
        panic!("the idle check closes the application at 08:00:02.4");
    };
    assert_eq!(wake, reset - LEAD_MS, "its wake: the reset less the lead");
    planner.finished(ALIAS, IdleEnd::Done);

    // The twelve minutes after it: each moment and whether a claim holds the key then.
    let moments = [
        // The close's own claim, released at 08:00:02.9.
        (at(1, 8, 0) + 2_600, true),
        (at(1, 8, 0) + 3_200, false),
        // A person's return home through the MCP server, three seconds.
        (at(1, 8, 1) + 33_000, true),
        (at(1, 8, 1) + 38_000, false),
        // Lab runs on the closed application, each failing.
        (at(1, 8, 3) + 24_300, true),
        (at(1, 8, 4) + 51_600, false),
        (at(1, 8, 5) + 19_700, true),
        (at(1, 8, 6) + 43_500, false),
        // A Lab run queued for 60 s, then refused.
        (at(1, 8, 7) + 12_000, true),
        (at(1, 8, 8) + 24_400, false),
        // A Lab run whose entry recovery fails on the closed application.
        (at(1, 8, 9) + 14_080, true),
        (at(1, 8, 10) + 20_600, false),
        (at(1, 8, 11) + 56_600, false),
    ];
    for (moment, held) in moments {
        let holder = held.then(|| slots.hold());
        slots.evaluate(moment);
        let view = if held { leased() } else { free() };
        assert_eq!(
            planner.check(ALIAS, &PRODUCTION, view, Some(plan(&slots)), moment),
            None,
            "no wake and no second close at {moment} (held {held})"
        );
        assert_eq!(
            planner.planned_wake(ALIAS),
            Some(wake),
            "the planned wake is kept at {moment}"
        );
        if let Some(holder) = holder {
            Slots::release(holder);
        }
    }

    // Its planned time.
    slots.evaluate(wake);
    assert_eq!(
        planner.check(ALIAS, &PRODUCTION, free(), Some(plan(&slots)), wake),
        Some(WAKE),
        "woken at its planned time"
    );
    assert_eq!(
        planner.woken_by(ALIAS),
        Some(IdleWakeCause::PlannedTimeReached)
    );
    slots.close();
}

#[test]
fn idle_w2_the_wake_says_why_it_came_the_first_that_holds() {
    let now = at(1, 8, 0);
    let next_due = at(1, 12, 0);
    let closed = |wake_at_unix_ms: Option<u64>, woken: bool| {
        let mut planner = IdlePlanner::default();
        planner.restore(
            ALIAS,
            KeptIdle {
                action: IdleAction::CloseApp,
                wake_at_unix_ms,
                woken,
            },
        );
        planner
    };
    let far = IdlePlan {
        evaluated_at_unix_ms: now,
        due_now: false,
        next_wake_unix_ms: Some(next_due),
        reading_due_unix_ms: None,
    };
    let due = IdlePlan {
        due_now: true,
        next_wake_unix_ms: None,
        ..far
    };
    let switched = IdleConfig {
        action: IdleAction::StopEmulator,
        ..PRODUCTION
    };
    // A task due, even beside a switched action and a wake the start owes.
    let mut planner = IdlePlanner::default();
    planner.restore_owed(ALIAS, IdleAction::CloseApp, now);
    assert_eq!(
        planner.check(ALIAS, &switched, free(), Some(due), now),
        Some(WAKE)
    );
    assert_eq!(planner.woken_by(ALIAS), Some(IdleWakeCause::TaskDue));
    // The action in effect changed, beside a wake the start owes.
    let mut planner = IdlePlanner::default();
    planner.restore_owed(ALIAS, IdleAction::CloseApp, now);
    assert_eq!(
        planner.check(ALIAS, &switched, free(), Some(far), now),
        Some(WAKE)
    );
    assert_eq!(planner.woken_by(ALIAS), Some(IdleWakeCause::ActionChanged));
    // A start with no usable record owes the wake. #486 ASLEEP-3 (0.12.79, register §1.512, ruling
    // 3, and its fix round 1, ruling 1; adapted): not at the start's time: it waits for the start's
    // first plan, a reading alone wakes it no more than a sleeping instance, and it comes as a
    // sleeping instance's, here at the plan's own time, which has come.
    let reading_alone = IdlePlan {
        due_now: false,
        next_wake_unix_ms: None,
        reading_due_unix_ms: Some(now),
        ..far
    };
    let time_come = IdlePlan {
        next_wake_unix_ms: Some(now + LEAD_MS),
        ..far
    };
    let mut planner = IdlePlanner::default();
    planner.restore_owed(ALIAS, IdleAction::CloseApp, now);
    assert_eq!(
        planner.check(ALIAS, &PRODUCTION, free(), None, now),
        None,
        "no wake before the start's first plan"
    );
    assert_eq!(
        planner.check(ALIAS, &PRODUCTION, free(), Some(reading_alone), now),
        None,
        "no wake for a reading alone"
    );
    assert_eq!(
        planner.check(ALIAS, &PRODUCTION, free(), Some(time_come), now),
        Some(WAKE)
    );
    assert_eq!(planner.woken_by(ALIAS), Some(IdleWakeCause::OwedAtStart));
    // An instance restored woken owes it too.
    let mut planner = closed(Some(now - MINUTE_MS), true);
    assert_eq!(
        planner.check(ALIAS, &PRODUCTION, free(), None, now),
        None,
        "no wake before the start's first plan"
    );
    assert_eq!(
        planner.check(ALIAS, &PRODUCTION, free(), Some(reading_alone), now),
        None,
        "no wake for a reading alone"
    );
    assert_eq!(
        planner.check(ALIAS, &PRODUCTION, free(), Some(time_come), now),
        Some(WAKE)
    );
    assert_eq!(planner.woken_by(ALIAS), Some(IdleWakeCause::OwedAtStart));
    // A kept wake whose time comes.
    let mut planner = closed(Some(next_due - LEAD_MS), false);
    assert_eq!(
        planner.check(ALIAS, &PRODUCTION, free(), Some(far), now),
        None,
        "not before its time"
    );
    assert_eq!(planner.woken_by(ALIAS), None, "not woken");
    let at_wake = next_due - LEAD_MS;
    assert_eq!(
        planner.check(ALIAS, &PRODUCTION, leased(), Some(far), at_wake),
        Some(WAKE)
    );
    assert_eq!(
        planner.woken_by(ALIAS),
        Some(IdleWakeCause::PlannedTimeReached)
    );
}

// ---- The host: X, `stop_emulator`, its configured emulator start and a startup package. ----

const X_ALIAS: &str = "idle_w2.instance.x";
/// The host fixture's manual clock.
const HOST_NOW_UNIX_MS: u64 = 1_000;
/// X's next due task in its plan: four hours away, past `idle.min_gap_ms`'s default.
const HOST_NEXT_DUE_UNIX_MS: u64 = HOST_NOW_UNIX_MS + 4 * HOUR_MS;
/// Every bounded wait of this file (test plan v2.2 §2.1 guard 3: a gate file waits only with the
/// constant of this name); it only bounds a broken run.
const GATE_WAIT: Duration = Duration::from_secs(60);

fn instance_text(instance: InstanceId) -> String {
    match serde_json::to_value(instance).expect("an instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    }
}

/// The configuration actingd assembles for X alone over the defaults file: X's entry names
/// `start_emulator` and the idle action `stop_emulator`.
fn x_manifest(x: InstanceId) -> RuntimeConfigManifest {
    let id = instance_text(x);
    let file = json!({"instances": [{
        "instance_id": id,
        "start_emulator": true,
        "idle": {"action": "stop_emulator"}
    }]});
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        std::slice::from_ref(&id),
        &defaults,
        &file,
        &|_| None,
    )
    .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

struct HostFixture {
    host: RuntimeHost,
    x: InstanceId,
    clock: Arc<ManualRuntimeClock>,
    _root: TempDir,
}

/// X, emulator-controlled from the start (so it registers with its emulator running), its stuck
/// recovery off so no ladder acts on it.
fn host_fixture() -> HostFixture {
    let root = TempDir::new().expect("tempdir");
    let x_state = Arc::new(FakeState::default());
    x_state.emulator_control.store(true, Ordering::Release);
    x_state
        .physical_task_geometry
        .store(true, Ordering::Release);
    x_state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    x_state.observed_open.store(true, Ordering::Release);
    let package_path = root.path().join("startup-package.zip");
    fs::write(&package_path, neutral_contained_task_package(true))
        .expect("write the startup package");
    let startup = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .and_then(|request| request.with_response_deadline_ms(60_000))
        .expect("the startup package request");
    let x = instance_id();
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let clock = Arc::new(ManualRuntimeClock::new(HOST_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_stuck_recovery(BTreeMap::from([(
                X_ALIAS.to_owned(),
                InstanceStuckRecovery {
                    enabled: false,
                    cooldown_secs: 600,
                },
            )]))
            .with_emulator_autostart(vec![X_ALIAS.to_owned()])
            .with_network_connectivity_for_test(read)
            .with_startup_packages(BTreeMap::from([(X_ALIAS.to_owned(), startup)]))
            .with_config_manifest(x_manifest(x)),
        Arc::new(FakeProvider::from_entries([(
            X_ALIAS.to_owned(),
            x,
            x_state,
        )])),
    )
    .expect("the runtime host");
    HostFixture {
        host,
        x,
        clock,
        _root: root,
    }
}

/// X's plan evaluated at `at`: a task due (`due_now`), or none and its next one at
/// `HOST_NEXT_DUE_UNIX_MS`.
fn x_plan(at: u64, due_now: bool) -> BTreeMap<String, IdlePlan> {
    BTreeMap::from([(
        X_ALIAS.to_owned(),
        IdlePlan {
            evaluated_at_unix_ms: at,
            due_now,
            next_wake_unix_ms: (!due_now).then_some(HOST_NEXT_DUE_UNIX_MS),
            reading_due_unix_ms: None,
        },
    )])
}

/// X's `instance_idle_action` records after `after`, in order: each one's `operation`,
/// `idle_wake_cause` and `task_id`.
fn idle_records(
    host: &RuntimeHost,
    x: InstanceId,
    after: u64,
) -> Vec<(String, Option<String>, Option<String>)> {
    let events = all_events(host);
    phases(&events)
        .into_iter()
        .filter(|(event, _)| event.sequence() > after && event.links().instance_id() == Some(&x))
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::DeviceStateObserved { outcome }
                if outcome.code().as_str() == IdleCode::Taken.as_str() =>
            {
                let text = |key: &str| {
                    outcome
                        .value(key)
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                };
                Some((
                    text("operation").unwrap_or_default(),
                    text("idle_wake_cause"),
                    text("task_id"),
                ))
            }
            _ => None,
        })
        .collect()
}

/// Fix round 2: what an assertion on a list of [`idle_records`] prints of the records it found,
/// each one's outcome code and operation, no more.
fn codes_and_operations(records: &[(String, Option<String>, Option<String>)]) -> Vec<String> {
    records
        .iter()
        .map(|(operation, _, _)| format!("{} {operation}", IdleCode::Taken.as_str()))
        .collect()
}

#[test]
fn idle_w2_host_a_held_instance_queues_no_wake_and_a_due_task_queues_one_that_names_it() {
    let fixture = host_fixture();
    let before = head(&fixture.host);

    // 1. X's action: no task due, its next one four hours away.
    fixture
        .host
        .check_idle_instances_for_test(&x_plan(HOST_NOW_UNIX_MS, false))
        .expect("the idle check");
    assert_eq!(
        fixture.host.await_idle_action_ends_for_test(1, GATE_WAIT),
        Some(1),
        "X's idle action ended"
    );

    // 2. A run holds X's admission guard, as a Lab run on the closed instance does, and the plan
    // evaluated under it is what the look ahead gives behind a hold: nothing due.
    let admission = fixture
        .host
        .instance_admission_for_test(X_ALIAS)
        .expect("X's admission guard");
    let held = HOST_NOW_UNIX_MS + MINUTE_MS;
    fixture.clock.set_unix_ms(held);
    {
        let _run = admission.lock().expect("hold X's admission guard");
        fixture
            .host
            .check_idle_instances_with_due_tasks_for_test(&x_plan(held, false), &BTreeMap::new())
            .expect("the idle check during the run");
    }
    let after_run = held + MINUTE_MS;
    fixture.clock.set_unix_ms(after_run);
    fixture
        .host
        .check_idle_instances_for_test(&x_plan(after_run, false))
        .expect("the idle check after the run");
    assert_eq!(
        idle_records(&fixture.host, fixture.x, before),
        [(IdleLocation::Action.as_str().to_owned(), None, None)],
        "X's one action, and no wake under or after the run"
    );

    // 3. A task due: the wake comes at once, and its record says why and for which task.
    let due = after_run + MINUTE_MS;
    fixture.clock.set_unix_ms(due);
    fixture
        .host
        .check_idle_instances_with_due_tasks_for_test(
            &x_plan(due, true),
            &BTreeMap::from([(X_ALIAS.to_owned(), TASK.to_owned())]),
        )
        .expect("the idle check at the due task");
    assert_eq!(
        idle_records(&fixture.host, fixture.x, before),
        [
            (IdleLocation::Action.as_str().to_owned(), None, None),
            (
                IdleLocation::Wake.as_str().to_owned(),
                Some(IdleWakeCause::TaskDue.as_str().to_owned()),
                Some(TASK.to_owned()),
            ),
        ],
        "one action, then one wake that names its cause and its task"
    );
    let HostFixture { host, _root, .. } = fixture;
    host.close().expect("close the host");
}

// ---- Fix round 1 (review-1845): a cooldown in front of a later gate. ----

/// `TASK`'s decision on the fixture instance: eligible, deferred by `code` with its wake `wake`.
fn deferred(code: HostCode, wake: u64) -> TaskDecision {
    TaskDecision {
        task_id: TASK.to_owned(),
        instance_id: Some(ALIAS.to_owned()),
        eligibility: EligibilityState::True,
        state: SchedulingDecisionState::Deferred,
        rank: None,
        detection_suggestions: Vec::new(),
        reasons: vec![DecisionReason {
            outcome: Outcome::new(code),
            detail: code.as_str().to_owned(),
        }],
        next_wake_unix_ms: Some(wake),
    }
}

/// The idle check's plan of the fixture instance from an evaluation at `at` whose one decision is
/// `decision`, with what the look ahead behind a hold answered for it (`held_ahead`; none: no hold
/// hid a later position), as the policy host records it.
fn gated_plan(decision: TaskDecision, at: u64, held_ahead: Option<Option<u64>>) -> IdlePlan {
    let held_ahead = held_ahead
        .map(|release| {
            IdleHeldAhead::from([(
                ALIAS.to_owned(),
                BTreeMap::from([(TASK.to_owned(), release)]),
            )])
        })
        .unwrap_or_default();
    let mut table = ScheduleExplanationTable::default();
    table.record_gated(
        &PolicyEvaluation {
            decisions: vec![decision],
            next_wake_unix_ms: None,
            preload_hint: None,
            dispatch_intents: Vec::new(),
            reason_chains: Vec::new(),
        },
        "sha256:idle-w2-fix1",
        at,
        &|_: &str, _: &str| -> Option<(u64, String)> { None },
        IdleGates {
            held_back: &IdleHeldBack::new(),
            held_ahead: &held_ahead,
            update_hold: IdleUpdateHold::default(),
        },
    );
    table.idle_plan(ALIAS).expect("an evaluated plan")
}

#[test]
fn idle_w2_fix1_a_cooldown_in_front_of_a_spent_budget_wakes_at_the_reset_not_at_its_end() {
    let closed_at = at(1, 6, 30);
    let reset = at(2, 0, 0);
    // Free, the budget defers the task to its reset: the idle check closes the application, its
    // wake at the reset less the lead.
    let mut planner = IdlePlanner::default();
    let free_plan = gated_plan(
        deferred(HostCode::PolicyBudgetExhausted, reset),
        closed_at,
        None,
    );
    assert_eq!(
        planner.check(ALIAS, &PRODUCTION, free(), Some(free_plan), closed_at),
        Some(IdleStep::Act {
            action: IdleAction::CloseApp,
            next_due_unix_ms: Some(reset),
            wake_at_unix_ms: Some(reset - LEAD_MS),
        }),
        "the idle check closes the application"
    );
    planner.finished(ALIAS, IdleEnd::Done);

    // A minute later the key is held and a cooldown of six seconds defers the task to its end (a
    // takeover cooldown after an abnormal stop), while the spent budget would hold it back behind
    // the cooldown; or a later position that names no time (a suspended package) would.
    let held_at = closed_at + MINUTE_MS;
    let cooldown_end = held_at + 6_000;
    for (what, behind, wake) in [
        ("the spent budget", Some(reset), Some(reset)),
        ("a position that names no time", None, None),
    ] {
        let read = gated_plan(
            deferred(HostCode::DispatchInstanceCooldown, cooldown_end),
            held_at,
            Some(behind),
        );
        assert!(
            !read.due_now,
            "{what}: not due behind the cooldown: {read:?}"
        );
        assert_eq!(
            read.next_wake_unix_ms, wake,
            "{what}: the later of the cooldown's end and what holds it back behind it"
        );
        assert_eq!(
            planner.check(ALIAS, &PRODUCTION, leased(), Some(read), held_at),
            None,
            "{what}: no wake under the cooldown"
        );
        // The cooldown's end: the free evaluation defers the task to its reset.
        let after = gated_plan(
            deferred(HostCode::PolicyBudgetExhausted, reset),
            cooldown_end,
            None,
        );
        assert_eq!(
            planner.check(ALIAS, &PRODUCTION, free(), Some(after), cooldown_end),
            None,
            "{what}: no wake at the cooldown's end"
        );
        assert_eq!(
            planner.planned_wake(ALIAS),
            Some(reset - LEAD_MS),
            "{what}: the wake stays the reset less the lead"
        );
    }
}

// ---- Fix round 1: the host's own plan and planner. ----

/// The game and the server of `gate_372k2`'s fixture instance (`policy_facts`).
const GAME: &str = "fixture-game-a";
const SERVER: &str = "fixture-server-a";
/// The application the fake device assigns the fixture instance (its default), which its game's
/// standard pack root declares for its server.
const APPLICATION: &str = "neutral.application";
/// What the device shows in front once the application is closed: the home screen's package.
const LAUNCHER: &str = "neutral.launcher";

/// The daemon's log of the steps the host skipped, as actingd's reporter writes them.
#[derive(Default)]
struct DaemonLog {
    skipped: Mutex<Vec<Outcome>>,
}

impl DaemonLog {
    /// The code of each skipped step so far, in order.
    fn skipped_codes(&self) -> Vec<String> {
        self.skipped
            .lock()
            .expect("the daemon's log")
            .iter()
            .map(|outcome| outcome.code().as_str().to_owned())
            .collect()
    }
}

impl RuntimeDaemonReporter for DaemonLog {
    fn fatal_latched(&self, _outcome: &Outcome) {}

    fn step_skipped(&self, outcome: &Outcome) {
        self.skipped
            .lock()
            .expect("the daemon's log")
            .push(outcome.clone());
    }

    fn warning_recorded(&self, _outcome: &Outcome) {}
}

/// How the fixture instance is started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fixture {
    /// As `gate_372k2`'s: no idle action.
    Plain,
    /// With the idle action `close_app`: emulator-controlled from the start (so it registers with
    /// its emulator running), its startup package configured, its stuck recovery off, and a
    /// standard pack root whose game declares its application for its server.
    CloseApp,
}

/// The configuration of the fixture instance `instance` with the idle action `close_app`: the
/// `idle` section's keys as the assembly renders them from the shipped defaults file (the start's
/// check of the instance's own idle keys reads every member the instance does not name from them,
/// `gate_ord1c`'s fix round 2), its own `idle.action`, and its no-progress guard off.
fn close_app_manifest(instance: InstanceId) -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let section = actingcommand_runtime_config::idle::SECTION;
    let mut parameters =
        render_parameters(&mechanism_rows(), &[], &defaults, &json!({}), &|_| None)
            .expect("every key resolves")
            .into_iter()
            .filter(|row| row.key.split('.').next() == Some(section))
            .collect::<Vec<_>>();
    parameters.push(ConfigParameter {
        key: actingcommand_runtime_config::instance_key(
            &crate::failure_identity::identifier_text(&instance),
            actingcommand_runtime_config::idle::ACTION,
        ),
        value: FactScalar::String(IdleAction::CloseApp.as_str().to_owned()),
        source: ConfigParameterSource::Explicit,
        settable: false,
    });
    parameters.extend(no_progress_off_manifest(&instance).parameters);
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// A standard pack root under `root` whose game of the fixture instance declares its application
/// for its server (`applications.json`), as `close_app` reads it.
fn declared_application(root: &TempDir) -> PathBuf {
    let packs = root.path().join("packages");
    let game = packs.join(GAME);
    fs::create_dir_all(&game).expect("the game's root");
    let declared = json!({"servers": {SERVER: {"application_id": APPLICATION}}});
    fs::write(
        game.join("applications.json"),
        serde_json::to_vec_pretty(&declared).expect("the declared application"),
    )
    .expect("write the declared application");
    packs
}

/// A start of the fixture instance's host on `root`, as [`OwnHost`] describes it.
fn open_own_host(
    fixture: Fixture,
    root: &TempDir,
    registered: InstanceId,
    clock: &Arc<ManualRuntimeClock>,
    state: &Arc<FakeState>,
    log: &Arc<DaemonLog>,
) -> RuntimeHost {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    let host_config = config(root)
        .with_policy_inputs(PolicyInputSnapshot::new(facts, policy_resources()))
        .with_runtime_clock(clock.clone())
        .with_daemon_reporter(log.clone());
    let host_config = match fixture {
        Fixture::Plain => host_config.with_config_manifest(no_progress_off_manifest(&registered)),
        Fixture::CloseApp => {
            // `gate_ord1c`'s fix round 3: the start withdraws a device self-checked instance's
            // availability at the clock, and its passing self-check publishes it again at the
            // clock; the preparation's first frame takes the clock's millisecond, as a real open
            // takes time, so the second is newer than the first.
            let moved = Arc::clone(clock);
            *state.capture_hook.lock().expect("capture hook") =
                Some(Box::new(move || moved.advance(1)));
            let package = root.path().join("idle-w2-startup-package.zip");
            fs::write(&package, neutral_contained_task_package(true))
                .expect("write the startup package");
            let startup = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
                .and_then(|request| request.with_response_deadline_ms(60_000))
                .expect("the startup package request");
            host_config
                .with_config_manifest(close_app_manifest(registered))
                .with_startup_packages(BTreeMap::from([(ALIAS.to_owned(), startup)]))
                .with_standard_packs_root(Some(declared_application(root)))
                .with_stuck_recovery(BTreeMap::from([(
                    ALIAS.to_owned(),
                    InstanceStuckRecovery {
                        enabled: false,
                        cooldown_secs: 600,
                    },
                )]))
        }
    };
    RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::one(ALIAS, registered, Arc::clone(state))),
    )
    .expect("start the fixture's host")
}

/// `gate_372k2`'s fixture instance (one task, `TASK`, whose trigger is always true, `daily_limit`
/// 2, an activity interval of 1 ms) on a host of its own, with a daemon log: every idle check is
/// the one the host runs itself on its own latest plan, with its own planner.
struct OwnHost {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    state: Arc<FakeState>,
    registered: InstanceId,
    fixture: Fixture,
    log: Arc<DaemonLog>,
    approved: BTreeSet<String>,
    root: TempDir,
}

/// The fixture instance's scripted device for one start: with `close_app`, emulator-controlled
/// from the start (so it registers with its emulator running) and its opens passing.
fn fixture_state(fixture: Fixture) -> Arc<FakeState> {
    let state = Arc::new(FakeState::default());
    if fixture == Fixture::CloseApp {
        for flag in [
            &state.emulator_control,
            &state.observed_open,
            &state.physical_task_geometry,
            &state.transition_capture_after_input,
        ] {
            flag.store(true, Ordering::Release);
        }
    }
    state
}

impl OwnHost {
    fn start(fixture: Fixture, unix_ms: u64) -> Self {
        let root = TempDir::new().expect("tempdir");
        let registered = instance_id();
        let state = fixture_state(fixture);
        let clock = Arc::new(ManualRuntimeClock::new(unix_ms, 0));
        let log = Arc::new(DaemonLog::default());
        let host = open_own_host(fixture, &root, registered, &clock, &state, &log);
        host.activate_policy_catalog(&sources(1))
            .expect("activate the catalog");
        Self {
            host,
            clock,
            state,
            registered,
            fixture,
            log,
            approved: BTreeSet::new(),
            root,
        }
    }

    /// A clean stop and a new start on the same state root, its Runtime clock and its device's
    /// script a new process's, at `unix_ms`.
    fn restart(self, unix_ms: u64) -> Self {
        let Self {
            host,
            registered,
            fixture,
            log,
            approved,
            root,
            ..
        } = self;
        host.close().expect("close the fixture's host");
        let clock = Arc::new(ManualRuntimeClock::new(unix_ms, 0));
        let state = fixture_state(fixture);
        let host = open_own_host(fixture, &root, registered, &clock, &state, &log);
        Self {
            host,
            clock,
            state,
            registered,
            fixture,
            log,
            approved,
            root,
        }
    }

    fn close(self) {
        self.host.close().expect("close the fixture's host");
    }

    /// Moves the Runtime clock and samples the host's capacity at the new time, as its capacity
    /// monitor does while time passes.
    fn set_time(&self, unix_ms: u64) {
        self.clock.set_unix_ms(unix_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
    }

    /// One cycle at `unix_ms` on the test inputs, as `Slots::evaluate` runs it: no idle check.
    fn evaluate(&self, unix_ms: u64) -> PolicyCycle {
        self.set_time(unix_ms);
        let mut facts = policy_facts();
        facts.outcomes.clear();
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms,
                    monotonic_ms: unix_ms,
                },
                7,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("evaluate at {unix_ms}: {error}"))
    }

    /// One cycle at `unix_ms` as actingd's driver asks for it: the evaluation, then the host's
    /// own idle check on the plan it leaves.
    fn cycle(&self, unix_ms: u64) -> PolicyCycle {
        self.set_time(unix_ms);
        let cycle = self
            .host
            .evaluate_policy_cycle(PolicyTrigger::Recovery)
            .unwrap_or_else(|error| panic!("the cycle at {unix_ms}: {error}"));
        assert!(
            cycle.evaluation.is_some(),
            "the cycle at {unix_ms} is evaluated: {:?}",
            cycle.directive
        );
        cycle
    }

    /// Evaluates at `unix_ms`, admits the task's dispatch the cycle selected, completes it and
    /// ends its lease, as a run of the task does (`Slots::dispatch`).
    fn dispatch(&mut self, unix_ms: u64) {
        let cycle = self.evaluate(unix_ms);
        let evaluation = cycle.evaluation.as_ref().expect("an evaluated cycle");
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == TASK)
            .unwrap_or_else(|| panic!("the task is selected at {unix_ms}: {evaluation:#?}"))
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("the dispatch's reason chain")
            .clone();
        if self.approved.insert(intent.catalog_hash.clone()) {
            record_policy_approval(&self.host, &intent);
        }
        let PolicyDispatchAdmission::Granted { context } = self
            .host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&self.host, &intent))
            .unwrap_or_else(|error| panic!("admit the task at {unix_ms}: {error}"))
        else {
            panic!("the task is granted at {unix_ms}");
        };
        self.host
            .complete_policy_dispatch(&intent.decision_id)
            .expect("complete the dispatch");
        self.host
            .release_policy_dispatch_lease_for_test(&context)
            .expect("end the dispatch's lease");
    }

    fn scheduling(&self, operation: RuntimeOperation) {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(operation);
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
    }

    /// A person's instance pause, as `actingctl pause` sends it.
    fn pause(&self) {
        self.scheduling(RuntimeOperation::PauseScheduling {
            scope: actingcommand_contract::SchedulingPauseScope::Instance {
                instance_alias: ALIAS.to_owned(),
            },
            pause_reason: actingcommand_contract::SchedulingPauseReason::new("idle_w2_pause")
                .expect("pause reason"),
            drain_timeout_ms: 1_000,
            pause_mode: actingcommand_contract::SchedulingPauseMode::Stand,
        });
    }

    fn resume(&self) {
        self.scheduling(RuntimeOperation::ResumeScheduling {
            scope: actingcommand_contract::SchedulingPauseScope::Instance {
                instance_alias: ALIAS.to_owned(),
            },
            expected: None,
        });
    }

    /// A client's lease that holds the instance's key until it is released (`Slots::release`).
    fn hold(&self) -> (TestClient, LeaseToken) {
        let mut holder = TestClient::connect(&self.host);
        let (_, token) = holder.acquire(ALIAS);
        (holder, token)
    }

    /// The instance's latest plan, as the host's idle check reads it.
    fn plan(&self) -> IdlePlan {
        self.host
            .idle_plan_and_runs_counted_for_test(ALIAS)
            .expect("the idle plan")
            .0
            .expect("an evaluated plan")
    }

    /// The task the instance's latest plan counts due.
    fn due_task(&self) -> Option<String> {
        self.host
            .idle_due_task_for_test(ALIAS)
            .expect("the idle plan's due task")
    }

    /// The instance's `instance_idle_action` records after `after` ([`idle_records`]).
    fn records(&self, after: u64) -> Vec<(String, Option<String>, Option<String>)> {
        idle_records(&self.host, self.registered, after)
    }
}

/// The fixture instance's row of `host.idle_state`: closed by `close_app`, its wake at `wake`.
fn closed_row(wake: u64) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([
        (
            "instance_alias".to_owned(),
            FactScalar::String(ALIAS.to_owned()),
        ),
        (
            "action".to_owned(),
            FactScalar::String(IdleAction::CloseApp.as_str().to_owned()),
        ),
        ("wake_at_unix_ms".to_owned(), FactScalar::TimestampMs(wake)),
    ])
}

#[test]
fn idle_w2_fix1_a_look_ahead_that_fails_is_said_and_leaves_the_candidate_due() {
    let mut own = OwnHost::start(Fixture::Plain, at(1, 6, 0));
    own.dispatch(at(1, 6, 10));
    own.dispatch(at(1, 6, 20));
    // A client holds the key and the look ahead behind that hold fails.
    let holder = own.hold();
    LOOK_AHEAD_FAILS_FOR_TEST.with(|fails| fails.set(true));
    let held = own.evaluate(at(1, 6, 30));
    LOOK_AHEAD_FAILS_FOR_TEST.with(|fails| fails.set(false));
    assert_eq!(
        deferred_by(&held, HostCode::DispatchInstanceHeld),
        None,
        "the decision is the hold's, as before"
    );
    let read = own.plan();
    assert!(
        read.due_now,
        "a candidate whose look ahead failed counts due, as before: {read:?}"
    );
    assert_eq!(own.due_task(), Some(TASK.to_owned()), "the plan names it");
    let skipped = own.log.skipped_codes();
    assert!(
        !skipped.is_empty()
            && skipped
                .iter()
                .all(|code| code == HostCode::PolicyEvaluationRejected.as_str()),
        "the failure is in the daemon's log as a skipped background step: {skipped:?}"
    );
    Slots::release(holder);
    own.close();
}

#[test]
fn idle_w2_fix1_host_own_plan_close_its_claim_a_refused_client_run_and_a_due_task() {
    let mut own = OwnHost::start(Fixture::CloseApp, at(1, 6, 0));
    let before = head(&own.host);
    // The day's two runs spend the task's budget.
    own.dispatch(at(1, 6, 10));
    own.dispatch(at(1, 6, 20));

    // The close's own claim: its worker waits once the planner knows the close's end, before the
    // claim releases the key.
    let (entered_tx, entered) = mpsc::channel::<()>();
    let (go, go_rx) = mpsc::channel::<()>();
    own.host
        .run_when_idle_action_finished_for_test(Box::new(move || {
            entered_tx
                .send(())
                .expect("the test waits for the close's end");
            go_rx
                .recv_timeout(GATE_WAIT)
                .expect("the test lets the close's claim release the key");
        }))
        .expect("the hook");

    // 06:30, free: the budget defers the task to its reset, and the host's own idle check closes
    // the application on its own plan.
    let closed_at = at(1, 6, 30);
    let reset = deferred_by(&own.cycle(closed_at), HostCode::PolicyBudgetExhausted)
        .expect("the budget names its reset");
    entered
        .recv_timeout(GATE_WAIT)
        .expect("the close ended and its claim still holds the key");
    assert_eq!(
        own.state
            .application_actions
            .lock()
            .expect("application actions")
            .last(),
        Some(&ApplicationLifecycleAction::Stop),
        "the close stopped the application"
    );

    // Half a second later, inside the close's own claim.
    let in_claim = closed_at + 500;
    assert_eq!(
        deferred_by(&own.cycle(in_claim), HostCode::DispatchInstanceHeld),
        None,
        "the close's claim holds the task with no wake"
    );
    let held_plan = own.plan();
    assert!(
        !held_plan.due_now,
        "not due behind the claim: {held_plan:?}"
    );
    assert_eq!(
        held_plan.next_wake_unix_ms,
        Some(reset),
        "its wake: the reset"
    );
    go.send(()).expect("the close's claim waits");
    assert_eq!(
        own.host.await_idle_action_ends_for_test(1, GATE_WAIT),
        Some(1),
        "the close ended"
    );
    let closed: (String, Option<String>, Option<String>) =
        (IdleLocation::Action.as_str().to_owned(), None, None);
    let after_claim = own.records(before);
    assert_eq!(
        after_claim,
        std::slice::from_ref(&closed),
        "the close, and no wake inside its own claim: {:?}",
        codes_and_operations(&after_claim)
    );
    assert_eq!(
        kept_rows(&own.host),
        Some(vec![closed_row(reset - LEAD_MS)]),
        "kept closed with its wake at the reset less the lead"
    );

    // A client's run on the closed application: its tap meets the home screen in front and is
    // refused; the evaluations under its lease and after it queue no wake.
    *own.state
        .foreground_script
        .lock()
        .expect("foreground script") =
        Some(Arc::new(|_: usize, _: usize| Some(LAUNCHER.to_owned())));
    let run_at = closed_at + MINUTE_MS;
    own.set_time(run_at);
    let (mut client, token) = own.hold();
    let tap = client.request(RuntimeOperation::Input {
        frame: None,
        token: token.clone(),
        action: InputAction::Tap { x: 1, y: 1 },
    });
    let refused = client.send(&tap);
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    assert_eq!(
        refused.outcome().code().as_str(),
        HostCode::ApplicationNotForeground.as_str(),
        "{refused:?}"
    );
    assert_eq!(
        deferred_by(&own.cycle(run_at + 1_000), HostCode::DispatchInstanceHeld),
        None,
        "the client's lease holds the task with no wake"
    );
    assert!(!own.plan().due_now, "not due behind the client's lease");
    Slots::release((client, token));
    assert_eq!(
        deferred_by(&own.cycle(run_at + 2_000), HostCode::PolicyBudgetExhausted),
        Some(reset),
        "free, the budget defers the task to its reset"
    );
    let after_run = own.records(before);
    assert_eq!(
        after_run,
        std::slice::from_ref(&closed),
        "no wake under the client's run or after it: {:?}",
        codes_and_operations(&after_run)
    );
    *own.state
        .foreground_script
        .lock()
        .expect("foreground script") = None;

    // The next day, past the reset: the task is due again, and the host's own idle check wakes
    // the application at once, its record naming why and for which task.
    let due_at = at(2, 6, 10);
    assert!(reset <= due_at, "the reset {reset} comes before {due_at}");
    own.cycle(due_at);
    assert_eq!(
        own.due_task(),
        Some(TASK.to_owned()),
        "the plan names the task"
    );
    let after_due = own.records(before);
    assert_eq!(
        after_due,
        [
            closed,
            (
                IdleLocation::Wake.as_str().to_owned(),
                Some(IdleWakeCause::TaskDue.as_str().to_owned()),
                Some(TASK.to_owned()),
            ),
        ],
        "one close, then one wake that names its cause and its task: {:?}",
        codes_and_operations(&after_due)
    );
    own.close();
}

#[test]
fn idle_w2_fix1_host_own_plan_a_start_restores_the_close_and_the_pause_and_wakes_nothing() {
    let mut own = OwnHost::start(Fixture::CloseApp, at(1, 6, 0));
    own.dispatch(at(1, 6, 10));
    own.dispatch(at(1, 6, 20));
    // 06:30: the host's own idle check closes the application, its wake at the reset less the
    // lead; then a person pauses the instance.
    let reset = deferred_by(&own.cycle(at(1, 6, 30)), HostCode::PolicyBudgetExhausted)
        .expect("the budget names its reset");
    assert_eq!(
        own.host.await_idle_action_ends_for_test(1, GATE_WAIT),
        Some(1),
        "the close ended"
    );
    let kept = Some(vec![closed_row(reset - LEAD_MS)]);
    assert_eq!(kept_rows(&own.host), kept, "kept closed with its wake");
    own.pause();

    // The root's second start seeds (it replays the history), its third loads the decision
    // memory from its value records, as every restart of an installation does; each restores the
    // close from `host.idle_state` and the pause.
    let own = own.restart(at(1, 7, 0));
    let own = own.restart(at(1, 7, 1));
    let started = at(1, 7, 1);
    let after_start = head(&own.host);
    assert_eq!(
        kept_rows(&own.host),
        kept,
        "the start restored the application closed with its wake"
    );

    // The first evaluation after the start, under the restored pause. The start withholds the
    // instance's availability and skips its preparation under the released pause, until the
    // resume's reconnect proves it again, so the evaluation blocks the task at placement
    // (`instance_unavailable`, before the order of the deferrals), its wake its trigger's next
    // time, a millisecond away. Fix round 2: the idle plan counts such a candidate of a closed
    // instance under a hold as held back with no time, so its trigger's time is no wake (on the
    // code of fix round 1 the wake came here, `planned_time_reached`). Fix round 3: the decision
    // is the placement's block for the unavailable instance, so the plan below is the new path's.
    let under_pause_cycle = own.cycle(started + 1_000);
    let blocked = decision(&under_pause_cycle);
    assert!(
        blocked.state == SchedulingDecisionState::Blocked
            && blocked.eligibility == EligibilityState::True
            && blocked
                .reasons
                .iter()
                .any(|reason| reason.outcome.is(PolicyCode::InstanceUnavailable)),
        "the placement blocks the task, its instance unavailable: {blocked:#?}"
    );
    let under_pause = own.plan();
    assert!(
        !under_pause.due_now,
        "nothing due under the pause: {under_pause:?}"
    );
    let under_pause_records = own.records(after_start);
    assert!(
        under_pause_records.is_empty(),
        "no wake under the restored pause: {:?}",
        codes_and_operations(&under_pause_records)
    );

    // The resume, then a claim on the key, as a start's preparation holds it.
    own.resume();
    let holder = own.hold();
    assert_eq!(
        deferred_by(&own.cycle(started + 2_000), HostCode::DispatchInstanceHeld),
        None,
        "the claim holds the task with no wake"
    );
    let held_plan = own.plan();
    assert!(
        !held_plan.due_now,
        "not due behind the claim: {held_plan:?}"
    );
    assert_eq!(held_plan.next_wake_unix_ms, Some(reset), "{held_plan:?}");
    Slots::release(holder);
    assert_eq!(
        deferred_by(&own.cycle(started + 3_000), HostCode::PolicyBudgetExhausted),
        Some(reset),
        "free, the budget defers the task to its reset"
    );
    let after_start_records = own.records(after_start);
    assert!(
        after_start_records.is_empty(),
        "no wake after the start: {:?}",
        codes_and_operations(&after_start_records)
    );
    assert_eq!(kept_rows(&own.host), kept, "the planned wake is kept");
    own.close();
}
