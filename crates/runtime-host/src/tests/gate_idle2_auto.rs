// SPDX-License-Identifier: AGPL-3.0-only

//! IDLE-2 (Alice's words of 11-11: 「根据任务间隔时间自动选择是关闭模拟器还是关闭应用还是回到主页面」; the
//! coordinator's rulings on Workflow #386, comment 6787682270): `idle.action` `auto` in the idle
//! planner, on a manual clock (plain Unix milliseconds the test advances), with neutral instances
//! and the scheduler's plan as fixed inputs, and the work order's thresholds: a rest from
//! 10 min, `close_app` from 30 min (`idle.min_gap_ms`), `stop_emulator` from 2 h.
//!
//! - Each tier is taken at its gap, directly, a gap of exactly a threshold takes the tier that
//!   starts there, and nothing is taken below the first.
//! - Without its configured emulator start an instance's tier stops at `close_app`; with no next
//!   wake it is `stop_emulator`; without a startup package it is reported once and may only rest.
//! - A rest runs once per idle stretch, whatever its end, owes no wake and is not kept; a later
//!   plan with a longer gap closes the application; work starts a new stretch.
//! - Nothing runs while a pause, a player hold, a blackout or a fence holds the instance (each
//!   reaches the planner as the routine gate's hold, `IdleView::paused`).
//! - A closed application restored under `auto` is not woken at once; a tier `auto` no longer
//!   allows is.

use crate::idle_action::{
    IdleEnd, IdlePlan, IdlePlanner, IdleRequirement, IdleStep, IdleView, KeptIdle, WakeWork,
};
use actingcommand_runtime_config::{AutoGaps, IdleAction, IdleConfig};

const MINUTE: u64 = 60_000;
/// The manual clock's first reading.
const T0: u64 = 1_800_000_000_000;
const NODE_A: &str = "node-a";
/// `idle.return_home_min_gap_ms`'s default.
const HOME: u64 = 10 * MINUTE;
/// `idle.min_gap_ms`'s default, the `close_app` tier's threshold.
const CLOSE: u64 = 30 * MINUTE;
/// `idle.stop_emulator_min_gap_ms`'s default.
const STOP: u64 = 120 * MINUTE;
/// The two wake leads' defaults.
const LEAD: u64 = 5 * MINUTE;
const CLOSE_APP_WAKE: &[WakeWork] = &[WakeWork::StartupPackage];
const STOP_EMULATOR_WAKE: &[WakeWork] = &[WakeWork::EmulatorStart, WakeWork::StartupPackage];

/// `auto` with the work order's defaults.
fn auto() -> IdleConfig {
    IdleConfig {
        action: IdleAction::Auto(AutoGaps {
            return_home_min_gap_ms: HOME,
            stop_emulator_min_gap_ms: STOP,
        }),
        min_gap_ms: CLOSE,
        close_app_wake_lead_ms: LEAD,
        stop_emulator_wake_lead_ms: LEAD,
    }
}

/// An instance in no one's use, its emulator running, that every tier can bring back.
fn free() -> IdleView {
    IdleView {
        emulator_on: true,
        app_wakeable: true,
        emulator_wakeable: true,
        ..IdleView::default()
    }
}

/// A plan evaluated at `at` with no due task and the instance's next wake `next_wake`.
fn plan(at: u64, next_wake: Option<u64>) -> Option<IdlePlan> {
    Some(IdlePlan {
        evaluated_at_unix_ms: at,
        due_now: false,
        next_wake_unix_ms: next_wake,
        reading_due_unix_ms: None,
    })
}

/// The rest a gap to `next_due` asks for.
const fn rest(next_due: Option<u64>) -> IdleStep {
    IdleStep::Rest {
        next_due_unix_ms: next_due,
        within_ms: HOME,
    }
}

/// The action `action` asked for, for a next due task at `next_due`, with its lead.
fn act(action: IdleAction, next_due: Option<u64>) -> IdleStep {
    IdleStep::Act {
        action,
        next_due_unix_ms: next_due,
        wake_at_unix_ms: next_due.map(|due| due - LEAD),
    }
}

/// One check at `T0` of a fresh planner for the gap `gap` (none: no next wake) with `view`.
fn first_step(view: IdleView, gap: Option<u64>) -> Option<IdleStep> {
    IdlePlanner::default().check(NODE_A, &auto(), view, plan(T0, gap.map(|gap| T0 + gap)), T0)
}

#[test]
fn idle2_auto_a_each_tier_is_taken_at_its_gap_and_nothing_below_the_first() {
    for (gap, expected) in [
        (Some(HOME - 1), None),
        (Some(HOME), Some(rest(Some(T0 + HOME)))),
        (Some(CLOSE - 1), Some(rest(Some(T0 + CLOSE - 1)))),
        (
            Some(CLOSE),
            Some(act(IdleAction::CloseApp, Some(T0 + CLOSE))),
        ),
        (
            Some(STOP - 1),
            Some(act(IdleAction::CloseApp, Some(T0 + STOP - 1))),
        ),
        (
            Some(STOP),
            Some(act(IdleAction::StopEmulator, Some(T0 + STOP))),
        ),
        (
            Some(24 * 60 * MINUTE),
            Some(act(IdleAction::StopEmulator, Some(T0 + 24 * 60 * MINUTE))),
        ),
    ] {
        assert_eq!(first_step(free(), gap), expected, "gap {gap:?}");
    }
    // A gap below the first threshold takes nothing, now or at later checks.
    let mut planner = IdlePlanner::default();
    let next_due = T0 + HOME - 1;
    for now in [T0, T0 + 1, T0 + MINUTE] {
        assert_eq!(
            planner.check(NODE_A, &auto(), free(), plan(now, Some(next_due)), now),
            None,
            "{now}"
        );
    }
    // A reading counts in the gap as for `idle.min_gap_ms`: one in 15 min beside a task in 3 h
    // makes the gap 15 min, a rest.
    let mut planner = IdlePlanner::default();
    let with_reading = Some(IdlePlan {
        evaluated_at_unix_ms: T0,
        due_now: false,
        next_wake_unix_ms: Some(T0 + 180 * MINUTE),
        reading_due_unix_ms: Some(T0 + 15 * MINUTE),
    });
    assert_eq!(
        planner.check(NODE_A, &auto(), free(), with_reading, T0),
        Some(rest(Some(T0 + 180 * MINUTE)))
    );
}

#[test]
fn idle2_auto_b_without_its_emulator_start_the_tier_stops_at_close_app_and_no_next_wake_stops_the_emulator()
 {
    // No next wake at all: `stop_emulator`, with no wake planned.
    assert_eq!(
        first_step(free(), None),
        Some(act(IdleAction::StopEmulator, None))
    );
    // Without the configured emulator start: `close_app`, and no warning (the tier is capped, not
    // refused).
    let unstarted = IdleView {
        emulator_wakeable: false,
        ..free()
    };
    for (gap, expected) in [
        (Some(STOP), act(IdleAction::CloseApp, Some(T0 + STOP))),
        (None, act(IdleAction::CloseApp, None)),
        (Some(HOME), rest(Some(T0 + HOME))),
    ] {
        assert_eq!(first_step(unstarted, gap), Some(expected), "gap {gap:?}");
    }
    // Without a startup package: reported once (as `close_app` and `stop_emulator` are), then
    // only a rest, whatever the gap.
    let unwakeable = IdleView {
        app_wakeable: false,
        emulator_wakeable: false,
        ..free()
    };
    let mut planner = IdlePlanner::default();
    let far = Some(T0 + STOP);
    assert_eq!(
        planner.check(NODE_A, &auto(), unwakeable, plan(T0, far), T0),
        Some(IdleStep::Unsupported {
            action: auto().action,
            missing: IdleRequirement::StartupPackage,
        })
    );
    assert_eq!(
        planner.check(NODE_A, &auto(), unwakeable, plan(T0, far), T0),
        Some(rest(far)),
        "no close and no stop on an instance that could not be brought back; a rest needs none"
    );
}

#[test]
fn idle2_auto_c_a_rest_runs_once_per_stretch_and_a_later_longer_gap_closes_the_application() {
    let mut planner = IdlePlanner::default();
    let next_due = T0 + 20 * MINUTE;
    assert_eq!(
        planner.check(NODE_A, &auto(), free(), plan(T0, Some(next_due)), T0),
        Some(rest(Some(next_due)))
    );
    // While the rest runs (its claim holds the instance, then frees it), nothing more.
    let held = IdleView {
        leased: true,
        ..free()
    };
    for view in [held, free()] {
        assert_eq!(
            planner.check(NODE_A, &auto(), view, plan(T0, Some(next_due)), T0 + 1),
            None
        );
    }
    planner.finished(NODE_A, IdleEnd::Done);
    assert!(planner.rested(NODE_A));
    // Rested: no wake, nothing kept, no second rest at fresh plans of the same task.
    assert_eq!(planner.planned_wake(NODE_A), None);
    assert!(planner.closed().is_empty(), "a rested instance is not kept");
    assert!(!planner.emulator_held_stopped(NODE_A));
    for now in [T0 + MINUTE, T0 + 5 * MINUTE] {
        assert_eq!(
            planner.check(NODE_A, &auto(), free(), plan(now, Some(next_due)), now),
            None,
            "{now}: one rest per idle stretch"
        );
    }
    // A later plan moves the next task an hour away: the application is closed, with its wake.
    let later = T0 + 6 * MINUTE;
    let following = later + 60 * MINUTE;
    assert_eq!(
        planner.check(NODE_A, &auto(), free(), plan(later, Some(following)), later),
        Some(act(IdleAction::CloseApp, Some(following)))
    );

    // Work ends the stretch: after it, a plan made after the work rests the instance again.
    let mut planner = IdlePlanner::default();
    assert_eq!(
        planner.check(NODE_A, &auto(), free(), plan(T0, Some(next_due)), T0),
        Some(rest(Some(next_due)))
    );
    planner.finished(NODE_A, IdleEnd::Done);
    let worked = T0 + 2 * MINUTE;
    assert_eq!(
        planner.check(NODE_A, &auto(), held, plan(worked, Some(next_due)), worked),
        None
    );
    assert!(!planner.rested(NODE_A), "work ended the stretch");
    let settled = worked + MINUTE;
    let again = settled + 15 * MINUTE;
    assert_eq!(
        planner.check(NODE_A, &auto(), free(), plan(T0, Some(again)), settled),
        None,
        "the plan predates the work"
    );
    assert_eq!(
        planner.check(NODE_A, &auto(), free(), plan(settled, Some(again)), settled),
        Some(rest(Some(again)))
    );
}

#[test]
fn idle2_auto_d_a_failed_rest_is_not_retried_in_its_stretch() {
    for end in [IdleEnd::NotPerformed, IdleEnd::Indeterminate] {
        let mut planner = IdlePlanner::default();
        let next_due = T0 + 20 * MINUTE;
        assert_eq!(
            planner.check(NODE_A, &auto(), free(), plan(T0, Some(next_due)), T0),
            Some(rest(Some(next_due)))
        );
        planner.finished(NODE_A, end);
        assert!(planner.rested(NODE_A), "{end:?}");
        assert_eq!(planner.planned_wake(NODE_A), None, "{end:?}: no wake owed");
        assert!(planner.closed().is_empty(), "{end:?}");
        for now in [T0 + MINUTE, T0 + 9 * MINUTE] {
            assert_eq!(
                planner.check(NODE_A, &auto(), free(), plan(now, Some(next_due)), now),
                None,
                "{end:?} at {now}: not tried again in the stretch"
            );
        }
    }
    // A rest the host did not carry out (on its key it met a pause or work) is no rest: the
    // instance is active again and may rest once it is free.
    let mut planner = IdlePlanner::default();
    let next_due = T0 + 20 * MINUTE;
    assert_eq!(
        planner.check(NODE_A, &auto(), free(), plan(T0, Some(next_due)), T0),
        Some(rest(Some(next_due)))
    );
    planner.abandoned(NODE_A);
    assert!(!planner.rested(NODE_A));
    let later = T0 + MINUTE;
    assert_eq!(
        planner.check(NODE_A, &auto(), free(), plan(later, Some(next_due)), later),
        Some(rest(Some(next_due)))
    );
}

#[test]
fn idle2_auto_e_no_rest_and_no_action_while_a_pause_a_player_hold_or_a_fence_holds_the_instance() {
    // A pause, a player hold, a blackout and a resource fence all hold the routine gate, which is
    // what the idle check reads as `paused`.
    let held = IdleView {
        paused: true,
        ..free()
    };
    let mut planner = IdlePlanner::default();
    for gap in [Some(HOME), Some(CLOSE), Some(STOP), None] {
        for now in [T0, T0 + MINUTE] {
            assert_eq!(
                planner.check(
                    NODE_A,
                    &auto(),
                    held,
                    plan(now, gap.map(|gap| now + gap)),
                    now
                ),
                None,
                "gap {gap:?} at {now}"
            );
        }
    }
    assert!(!planner.rested(NODE_A));
    // Once the hold ends, the tier the gap gives.
    let free_at = T0 + 2 * MINUTE;
    assert_eq!(
        planner.check(
            NODE_A,
            &auto(),
            free(),
            plan(free_at, Some(free_at + HOME)),
            free_at
        ),
        Some(rest(Some(free_at + HOME)))
    );
}

#[test]
fn idle2_auto_f_a_closed_application_restored_under_auto_is_not_woken_at_once() {
    let wake_at = T0 + 60 * MINUTE;
    let restored = |action| {
        let mut planner = IdlePlanner::default();
        planner.restore(
            NODE_A,
            KeptIdle {
                action,
                wake_at_unix_ms: Some(wake_at),
                woken: false,
            },
        );
        planner
    };
    let mut planner = restored(IdleAction::CloseApp);
    // The kept tier, never `auto`.
    assert_eq!(
        planner
            .closed()
            .get(NODE_A)
            .map(|kept| kept.action.as_str()),
        Some("close_app")
    );
    // The start's first checks, with no plan and with one: `auto` beside the kept `close_app` is
    // no switch.
    assert_eq!(planner.check(NODE_A, &auto(), free(), None, T0), None);
    assert_eq!(
        planner.check(
            NODE_A,
            &auto(),
            free(),
            plan(T0, Some(wake_at + LEAD)),
            T0 + 1
        ),
        None
    );
    assert_eq!(planner.planned_wake(NODE_A), Some(wake_at));
    // Its wake at its time.
    assert_eq!(
        planner.check(NODE_A, &auto(), free(), None, wake_at),
        Some(IdleStep::Wake {
            action: IdleAction::CloseApp,
            work: CLOSE_APP_WAKE,
        })
    );
    // The same for a kept `stop_emulator` on an instance whose emulator the Runtime starts.
    let mut planner = restored(IdleAction::StopEmulator);
    assert_eq!(planner.check(NODE_A, &auto(), free(), None, T0), None);
    // A tier `auto` no longer allows on the instance (its emulator start is off now) is a switch:
    // it is woken at once.
    let unstarted = IdleView {
        emulator_wakeable: false,
        ..free()
    };
    assert_eq!(
        planner.check(NODE_A, &auto(), unstarted, None, T0 + 1),
        Some(IdleStep::Wake {
            action: IdleAction::StopEmulator,
            work: STOP_EMULATOR_WAKE,
        })
    );
}
