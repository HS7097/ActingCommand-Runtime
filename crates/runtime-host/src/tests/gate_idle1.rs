// SPDX-License-Identifier: AGPL-3.0-only

//! IDLE-1 (Alice's ruling 41 of 10-24 and her ruling of 10-25: 「没有任务的时候直接关闭模拟器……任务结束或者
//! 接下来一段时间比如30分钟内没有任务的时候关闭应用程序」; Workflow #386 comment 6393646823): the idle planner
//! on a manual clock (plain Unix milliseconds the test advances), with neutral instances and the
//! scheduler's plan as fixed inputs.
//!
//! - A gap to the next due task of at least `idle.min_gap_ms` runs the action once; a shorter
//!   one runs nothing.
//! - A paused, leased, queued or manually held instance is left alone, and so is one whose plan
//!   was made before it was last seen in use. One whose action in effect cannot bring it back
//!   (no startup package, or for `stop_emulator` no configured emulator start) never acts and is
//!   reported once, naming what it lacks (Fail Loud).
//! - `close_app` brings the instance back at next due minus its lead through the startup package;
//!   `stop_emulator` at next due minus its lead through the configured emulator start and then
//!   the startup package.
//! - A due task earlier than planned wakes the instance at once.
//! - A failed stop fails only its own instance: it is left as it is, and another instance's
//!   action and wake go on.
//! - `none` never acts and never wakes.
//!
//! IDLE-2 (the owed wake; v0.12.13 in production on 10-26: `close_app` ran 57 times and the wakes
//! of 17:54 and 18:54 never queued), with the production keys (`close_app`, its wake lead 300000,
//! `idle.min_gap_ms` 1800000):
//! - Momentary holds of a closed instance (the emulator watch's status read under its admission
//!   guard, the close's own claim, a queued claim) neither close it again nor drop its wake, which
//!   comes once at next due minus the lead, also after a hold inside the last minimum gap and
//!   while the instance is held.
//! - A task due on the closed app that waits for a reading wakes it at once (IDLE-1P: a reading
//!   alone does not, `gate_idle1p`), and a task due within the minimum gap keeps it from closing
//!   again.
//! - A close whose stop was sent and failed owes the wake; one not performed is left as it is,
//!   with no wake.

use crate::idle_action::{
    IdleEnd, IdlePlan, IdlePlanner, IdleRequirement, IdleStep, IdleView, WakeWork,
};
use actingcommand_runtime_config::{IdleAction, IdleConfig};

const MINUTE: u64 = 60_000;
/// The manual clock's first reading.
const T0: u64 = 1_800_000_000_000;
const NODE_A: &str = "node-a";
const NODE_B: &str = "node-b";
/// What a wake after `close_app` queues.
const CLOSE_APP_WAKE: &[WakeWork] = &[WakeWork::StartupPackage];
/// What a wake after `stop_emulator` queues, in order.
const STOP_EMULATOR_WAKE: &[WakeWork] = &[WakeWork::EmulatorStart, WakeWork::StartupPackage];

/// The work order's defaults with `action`.
fn config(action: IdleAction) -> IdleConfig {
    IdleConfig {
        action,
        min_gap_ms: 30 * MINUTE,
        close_app_wake_lead_ms: MINUTE,
        stop_emulator_wake_lead_ms: 5 * MINUTE,
    }
}

/// An instance in no one's use, its emulator running, that both actions can bring back.
fn free() -> IdleView {
    IdleView {
        emulator_on: true,
        app_wakeable: true,
        emulator_wakeable: true,
        ..IdleView::default()
    }
}

/// The instance while the action's own claim holds it.
fn held() -> IdleView {
    IdleView {
        leased: true,
        ..free()
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

/// A plan evaluated at `at` with a task of the instance due.
fn due(at: u64) -> Option<IdlePlan> {
    Some(IdlePlan {
        evaluated_at_unix_ms: at,
        due_now: true,
        next_wake_unix_ms: None,
        reading_due_unix_ms: None,
    })
}

/// The production keys of 10-26 on the two instances: `close_app`, its wake lead 300000 (a
/// run-time instruction) and `idle.min_gap_ms` 1800000 (the default).
fn production() -> IdleConfig {
    IdleConfig {
        action: IdleAction::CloseApp,
        min_gap_ms: 30 * MINUTE,
        close_app_wake_lead_ms: LEAD,
        stop_emulator_wake_lead_ms: 5 * MINUTE,
    }
}

/// `production()`'s wake lead of `close_app`.
const LEAD: u64 = 5 * MINUTE;

/// The wake after `close_app`.
const CLOSE_APP_WAKE_STEP: IdleStep = IdleStep::Wake {
    action: IdleAction::CloseApp,
    work: CLOSE_APP_WAKE,
};

/// What a closed instance meets for a moment between its close and its wake: the emulator
/// watch's status read under its admission guard (`manual`), the close's own claim before its
/// release (`leased`) and a queued claim (`queued`).
fn holds() -> [IdleView; 3] {
    [
        IdleView {
            manual: true,
            ..free()
        },
        IdleView {
            leased: true,
            ..free()
        },
        IdleView {
            queued: true,
            ..free()
        },
    ]
}

/// `close_app` asked for on `NODE_A` at `T0` with the production keys, for a next due task at
/// `next_due`.
fn close(planner: &mut IdlePlanner, next_due: u64) {
    assert_eq!(
        planner.check(NODE_A, &production(), free(), plan(T0, Some(next_due)), T0),
        Some(IdleStep::Act {
            action: IdleAction::CloseApp,
            next_due_unix_ms: Some(next_due),
            wake_at_unix_ms: Some(next_due - LEAD),
        }),
        "the close"
    );
}

/// One check of `NODE_A` at `now` with the production keys, as the host takes it (an action asked
/// for is carried out and reported done), and its step, if any, with its time.
fn checked(
    planner: &mut IdlePlanner,
    view: IdleView,
    plan: Option<IdlePlan>,
    now: u64,
) -> Option<(u64, IdleStep)> {
    let step = planner.check(NODE_A, &production(), view, plan, now);
    if matches!(step, Some(IdleStep::Act { .. })) {
        planner.finished(NODE_A, IdleEnd::Done);
    }
    step.map(|step| (now, step))
}

/// Runs the action of `action` on `alias` at `T0` for a next due task at `next_due` and reports
/// it finished as `succeeded`.
fn act(planner: &mut IdlePlanner, alias: &str, action: IdleAction, next_due: u64) {
    let step = planner.check(alias, &config(action), free(), plan(T0, Some(next_due)), T0);
    assert_eq!(
        step,
        Some(IdleStep::Act {
            action,
            next_due_unix_ms: Some(next_due),
            wake_at_unix_ms: Some(next_due - config(action).wake_lead_ms(action)),
        })
    );
    assert_eq!(
        planner.check(alias, &config(action), held(), plan(T0, Some(next_due)), T0),
        None,
        "the action runs once while its claim holds the instance"
    );
}

#[test]
fn idle1_a_the_action_runs_once_at_the_minimum_gap_and_never_below_it() {
    for action in [IdleAction::CloseApp, IdleAction::StopEmulator] {
        let mut planner = IdlePlanner::default();
        let next_due = T0 + 30 * MINUTE;
        act(&mut planner, NODE_A, action, next_due);
        planner.finished(NODE_A, IdleEnd::Done);
        for now in [T0 + 1, T0 + 10 * MINUTE] {
            assert_eq!(
                planner.check(
                    NODE_A,
                    &config(action),
                    free(),
                    plan(now, Some(next_due)),
                    now
                ),
                None,
                "{action:?}: an idle instance is not acted on again"
            );
        }
        // A gap one millisecond short of the minimum runs nothing, now or later.
        let short = T0 + 30 * MINUTE - 1;
        for now in [T0, T0 + MINUTE, T0 + 20 * MINUTE] {
            assert_eq!(
                planner.check(NODE_B, &config(action), free(), plan(now, Some(short)), now),
                None,
                "{action:?}: the gap is below idle.min_gap_ms"
            );
        }
        // No wake at all in the plan: nothing is due, the action runs, and no wake is planned.
        let mut planner = IdlePlanner::default();
        assert_eq!(
            planner.check(NODE_A, &config(action), free(), plan(T0, None), T0),
            Some(IdleStep::Act {
                action,
                next_due_unix_ms: None,
                wake_at_unix_ms: None,
            })
        );
    }
}

#[test]
fn idle1_b_a_paused_leased_queued_or_manually_held_instance_is_left_alone() {
    let action = IdleAction::StopEmulator;
    let far = Some(T0 + 4 * 60 * MINUTE);
    let views = [
        IdleView {
            paused: true,
            ..free()
        },
        IdleView {
            leased: true,
            ..free()
        },
        IdleView {
            queued: true,
            ..free()
        },
        IdleView {
            manual: true,
            ..free()
        },
        // Not in the state the action leaves.
        IdleView {
            emulator_on: false,
            ..free()
        },
    ];
    for view in views {
        let mut planner = IdlePlanner::default();
        for now in [T0, T0 + MINUTE] {
            assert_eq!(
                planner.check(NODE_A, &config(action), view, plan(now, far), now),
                None,
                "{view:?}"
            );
        }
    }
    // Not able to come back (Fail Loud, the coordinator's follow-up on #1333): the action never
    // runs, and the first check reports it once, naming what the instance lacks.
    let no_startup_package = IdleView {
        app_wakeable: false,
        emulator_wakeable: false,
        ..free()
    };
    let no_emulator_start = IdleView {
        emulator_wakeable: false,
        ..free()
    };
    for (action, view, missing) in [
        (
            IdleAction::StopEmulator,
            no_emulator_start,
            IdleRequirement::EmulatorStart,
        ),
        (
            IdleAction::StopEmulator,
            no_startup_package,
            IdleRequirement::StartupPackage,
        ),
        (
            IdleAction::CloseApp,
            no_startup_package,
            IdleRequirement::StartupPackage,
        ),
    ] {
        let mut planner = IdlePlanner::default();
        assert_eq!(
            planner.check(NODE_A, &config(action), view, plan(T0, far), T0),
            Some(IdleStep::Unsupported { action, missing }),
            "{action:?}, {view:?}"
        );
        for now in [T0, T0 + MINUTE] {
            assert_eq!(
                planner.check(NODE_A, &config(action), view, plan(now, far), now),
                None,
                "{action:?}, {view:?}: reported once, never acted on"
            );
        }
    }
    // Work held the instance at T0 + 1 min: the plan made before it is not enough, one made after
    // it is.
    let mut planner = IdlePlanner::default();
    let busy = IdleView {
        leased: true,
        ..free()
    };
    assert_eq!(
        planner.check(NODE_A, &config(action), busy, plan(T0, far), T0 + MINUTE),
        None
    );
    assert_eq!(
        planner.check(
            NODE_A,
            &config(action),
            free(),
            plan(T0, far),
            T0 + 2 * MINUTE
        ),
        None,
        "the plan predates the work"
    );
    assert!(matches!(
        planner.check(
            NODE_A,
            &config(action),
            free(),
            plan(T0 + 2 * MINUTE, far),
            T0 + 2 * MINUTE
        ),
        Some(IdleStep::Act { .. })
    ));
    // A plan with a task due runs nothing.
    let mut planner = IdlePlanner::default();
    assert_eq!(
        planner.check(NODE_A, &config(action), free(), due(T0), T0),
        None
    );
}

#[test]
fn idle1_c_each_action_brings_the_instance_back_at_next_due_minus_its_lead() {
    let next_due = T0 + 2 * 60 * MINUTE;
    for (action, lead, work) in [
        (IdleAction::CloseApp, MINUTE, CLOSE_APP_WAKE),
        (IdleAction::StopEmulator, 5 * MINUTE, STOP_EMULATOR_WAKE),
    ] {
        let mut planner = IdlePlanner::default();
        act(&mut planner, NODE_A, action, next_due);
        assert_eq!(
            planner.emulator_held_stopped(NODE_A),
            action == IdleAction::StopEmulator,
            "{action:?}: the emulator watch holds off only a stopped emulator"
        );
        planner.finished(NODE_A, IdleEnd::Done);
        assert_eq!(planner.planned_wake(NODE_A), Some(next_due - lead));
        // Later plans keep the same next due task.
        let before = next_due - lead - 1;
        assert_eq!(
            planner.check(
                NODE_A,
                &config(action),
                free(),
                plan(before, Some(next_due)),
                before
            ),
            None,
            "{action:?}: one millisecond before the wake"
        );
        let at = next_due - lead;
        assert_eq!(
            planner.check(
                NODE_A,
                &config(action),
                free(),
                plan(at, Some(next_due)),
                at
            ),
            Some(IdleStep::Wake { action, work }),
            "{action:?}"
        );
        assert!(!planner.emulator_held_stopped(NODE_A));
        // The wake's claims hold the instance next; nothing is acted on again at once.
        assert_eq!(
            planner.check(
                NODE_A,
                &config(action),
                held(),
                plan(at, Some(next_due)),
                at
            ),
            None
        );
    }
}

#[test]
fn idle1_d_a_due_task_earlier_than_planned_wakes_the_instance_at_once() {
    let next_due = T0 + 2 * 60 * MINUTE;
    // close_app: a plan at T0 + 10 min shows a task due.
    let mut planner = IdlePlanner::default();
    act(&mut planner, NODE_A, IdleAction::CloseApp, next_due);
    planner.finished(NODE_A, IdleEnd::Done);
    let now = T0 + 10 * MINUTE;
    assert_eq!(
        planner.check(NODE_A, &config(IdleAction::CloseApp), free(), due(now), now),
        Some(IdleStep::Wake {
            action: IdleAction::CloseApp,
            work: CLOSE_APP_WAKE,
        })
    );
    // stop_emulator: a later plan moves the next due task to T0 + 40 min, so the wake comes at
    // T0 + 35 min instead of the planned next due minus 5 min.
    let mut planner = IdlePlanner::default();
    let action = IdleAction::StopEmulator;
    act(&mut planner, NODE_A, action, next_due);
    planner.finished(NODE_A, IdleEnd::Done);
    let earlier = Some(T0 + 40 * MINUTE);
    let now = T0 + 20 * MINUTE;
    assert_eq!(
        planner.check(NODE_A, &config(action), free(), plan(now, earlier), now),
        None
    );
    assert_eq!(planner.planned_wake(NODE_A), Some(T0 + 35 * MINUTE));
    let now = T0 + 35 * MINUTE;
    assert_eq!(
        planner.check(NODE_A, &config(action), free(), plan(now, earlier), now),
        Some(IdleStep::Wake {
            action,
            work: STOP_EMULATOR_WAKE,
        })
    );
    // Switching the action off while the instance is idle brings it back at once.
    let mut planner = IdlePlanner::default();
    act(&mut planner, NODE_A, action, next_due);
    planner.finished(NODE_A, IdleEnd::Done);
    let now = T0 + MINUTE;
    assert_eq!(
        planner.check(
            NODE_A,
            &config(IdleAction::None),
            free(),
            plan(now, Some(next_due)),
            now
        ),
        Some(IdleStep::Wake {
            action,
            work: STOP_EMULATOR_WAKE,
        })
    );
}

#[test]
fn idle1_e_a_failed_stop_fails_only_that_instance() {
    let action = IdleAction::StopEmulator;
    let next_due = T0 + 2 * 60 * MINUTE;
    let mut planner = IdlePlanner::default();
    act(&mut planner, NODE_A, action, next_due);
    act(&mut planner, NODE_B, action, next_due);
    planner.finished(NODE_A, IdleEnd::NotPerformed);
    planner.finished(NODE_B, IdleEnd::Done);
    // A is left as it is: no wake is queued for it, and the emulator watch is not held off.
    assert!(!planner.emulator_held_stopped(NODE_A));
    assert!(planner.emulator_held_stopped(NODE_B));
    let at = next_due - 5 * MINUTE;
    assert_eq!(
        planner.check(
            NODE_A,
            &config(action),
            free(),
            plan(at, Some(next_due)),
            at
        ),
        None,
        "a failed action is not tried again, and no wake follows it"
    );
    // B's wake goes on.
    assert_eq!(
        planner.check(
            NODE_B,
            &config(action),
            free(),
            plan(at, Some(next_due)),
            at
        ),
        Some(IdleStep::Wake {
            action,
            work: STOP_EMULATOR_WAKE,
        })
    );
    // A's next due task goes through the normal path: once work held A, its next idle gap acts
    // again.
    let busy = IdleView {
        leased: true,
        ..free()
    };
    assert_eq!(
        planner.check(NODE_A, &config(action), busy, due(next_due), next_due),
        None
    );
    let later = next_due + MINUTE;
    let following = later + 3 * 60 * MINUTE;
    assert_eq!(
        planner.check(
            NODE_A,
            &config(action),
            free(),
            plan(later, Some(following)),
            later
        ),
        Some(IdleStep::Act {
            action,
            next_due_unix_ms: Some(following),
            wake_at_unix_ms: Some(following - 5 * MINUTE),
        })
    );
}

#[test]
fn idle1_f_none_never_acts_or_wakes() {
    let mut planner = IdlePlanner::default();
    let off = config(IdleAction::None);
    for (now, next_wake) in [
        (T0, None),
        (T0, Some(T0 + 24 * 60 * MINUTE)),
        (T0 + 60 * MINUTE, Some(T0 + 61 * MINUTE)),
        (T0 + 2 * 60 * MINUTE, None),
    ] {
        assert_eq!(
            planner.check(NODE_A, &off, free(), plan(now, next_wake), now),
            None
        );
    }
    assert!(!planner.emulator_held_stopped(NODE_A));
    assert_eq!(planner.planned_wake(NODE_A), None);
}

#[test]
fn idle2_a_momentary_holds_neither_close_the_instance_again_nor_drop_its_wake() {
    let next_due = T0 + 4 * 60 * MINUTE;
    let wake = next_due - LEAD;
    let mut planner = IdlePlanner::default();
    close(&mut planner, next_due);
    planner.finished(NODE_A, IdleEnd::Done);
    let mut steps = Vec::new();
    let mut wakes_in_effect = Vec::new();
    // Every 5 min until next due minus the minimum gap: a hold, then a free check with a plan made
    // after it.
    let rounds = (1..)
        .map(|round| T0 + round * 5 * MINUTE)
        .take_while(|now| *now <= next_due - 30 * MINUTE);
    for (now, hold) in rounds.zip(holds().into_iter().cycle()) {
        steps.extend(checked(&mut planner, hold, plan(now, Some(next_due)), now));
        let after = now + 1;
        steps.extend(checked(
            &mut planner,
            free(),
            plan(after, Some(next_due)),
            after,
        ));
        wakes_in_effect.push(planner.planned_wake(NODE_A));
    }
    // The wake, and its claim holding the instance next.
    steps.extend(checked(
        &mut planner,
        free(),
        plan(wake, Some(next_due)),
        wake,
    ));
    steps.extend(checked(
        &mut planner,
        held(),
        plan(wake, Some(next_due)),
        wake + 1,
    ));
    assert_eq!(
        steps,
        vec![(wake, CLOSE_APP_WAKE_STEP)],
        "not one second close, and exactly one wake at next due minus the lead"
    );
    assert_eq!(wakes_in_effect.len(), 42);
    assert!(
        wakes_in_effect.iter().all(|at| *at == Some(wake)),
        "the wake stays next due minus the lead: {wakes_in_effect:?}"
    );
}

#[test]
fn idle2_b_a_hold_inside_the_last_minimum_gap_does_not_drop_the_wake() {
    let next_due = T0 + 31 * MINUTE;
    let wake = next_due - LEAD;
    let mut planner = IdlePlanner::default();
    close(&mut planner, next_due);
    planner.finished(NODE_A, IdleEnd::Done);
    // The emulator watch's status read at next due minus 25 min, then free checks every minute
    // with fresh plans up to the next due time.
    let read = next_due - 25 * MINUTE;
    let mut steps = Vec::new();
    steps.extend(checked(
        &mut planner,
        IdleView {
            manual: true,
            ..free()
        },
        plan(read, Some(next_due)),
        read,
    ));
    for now in (1..25).map(|minutes| read + minutes * MINUTE) {
        steps.extend(checked(
            &mut planner,
            free(),
            plan(now, Some(next_due)),
            now,
        ));
    }
    assert_eq!(
        steps,
        vec![(wake, CLOSE_APP_WAKE_STEP)],
        "no close, no step before next due minus the lead, and the wake then"
    );
}

#[test]
fn idle2_c_a_wake_due_while_the_instance_is_held_is_queued() {
    let next_due = T0 + 4 * 60 * MINUTE;
    let wake = next_due - LEAD;
    for hold in [
        IdleView {
            leased: true,
            ..free()
        },
        IdleView {
            manual: true,
            ..free()
        },
    ] {
        let mut planner = IdlePlanner::default();
        close(&mut planner, next_due);
        planner.finished(NODE_A, IdleEnd::Done);
        assert_eq!(
            planner.check(
                NODE_A,
                &production(),
                hold,
                plan(wake, Some(next_due)),
                wake
            ),
            Some(CLOSE_APP_WAKE_STEP),
            "{hold:?}"
        );
        assert_eq!(
            planner.check(
                NODE_A,
                &production(),
                held(),
                plan(wake, Some(next_due)),
                wake + 1
            ),
            None,
            "{hold:?}: the wake's claim holds the instance next"
        );
    }
}

#[test]
fn idle2_d_a_reading_due_on_the_closed_app_wakes_it_first_and_does_not_cycle() {
    let next_due = T0 + 4 * 60 * MINUTE;
    let mut planner = IdlePlanner::default();
    close(&mut planner, next_due);
    planner.finished(NODE_A, IdleEnd::Done);
    let mut steps = Vec::new();
    // T0 + 10 min: the evaluation selected a fact-refresh reading of the instance for a task that
    // is due and waits for it (IDLE-1P: the due task wakes the instance; the reading alone would
    // not).
    let reading = T0 + 10 * MINUTE;
    steps.extend(checked(&mut planner, free(), due(reading), reading));
    // The wake's claim, then the reading's queued claim, hold the instance.
    for (now, hold) in [
        (reading + 1, held()),
        (
            reading + MINUTE,
            IdleView {
                queued: true,
                ..free()
            },
        ),
        (reading + 2 * MINUTE, held()),
    ] {
        steps.extend(checked(&mut planner, hold, plan(now, Some(next_due)), now));
    }
    // Free, with the next task due within the minimum gap: no close.
    let settled = reading + 3 * MINUTE;
    steps.extend(checked(
        &mut planner,
        free(),
        plan(settled, Some(settled + 20 * MINUTE)),
        settled,
    ));
    // A later plan shows the minimum gap: one close, with a new wake.
    let later = reading + 30 * MINUTE;
    let following = later + 30 * MINUTE;
    for now in [later, later + 1] {
        steps.extend(checked(
            &mut planner,
            free(),
            plan(now, Some(following)),
            now,
        ));
    }
    assert_eq!(
        steps,
        vec![
            (reading, CLOSE_APP_WAKE_STEP),
            (
                later,
                IdleStep::Act {
                    action: IdleAction::CloseApp,
                    next_due_unix_ms: Some(following),
                    wake_at_unix_ms: Some(following - LEAD),
                }
            ),
        ]
    );
}

#[test]
fn idle2_e_a_close_whose_stop_was_sent_and_failed_owes_the_wake() {
    let next_due = T0 + 4 * 60 * MINUTE;
    let wake = next_due - LEAD;
    let mut planner = IdlePlanner::default();
    close(&mut planner, next_due);
    planner.finished(NODE_A, IdleEnd::Indeterminate);
    assert_eq!(planner.planned_wake(NODE_A), Some(wake));
    // Every 10 min until the wake: the holds and a free check, in turn, each with a fresh plan.
    let mut steps = Vec::new();
    let views = holds().into_iter().chain([free()]).cycle();
    let checks = (1..)
        .map(|round| T0 + round * 10 * MINUTE)
        .take_while(|now| *now < wake);
    for (now, view) in checks.zip(views) {
        steps.extend(checked(&mut planner, view, plan(now, Some(next_due)), now));
    }
    steps.extend(checked(
        &mut planner,
        free(),
        plan(wake, Some(next_due)),
        wake,
    ));
    assert_eq!(
        steps,
        vec![(wake, CLOSE_APP_WAKE_STEP)],
        "no close, and the wake at next due minus the lead"
    );
    // A close not performed stays failed, with no wake.
    let mut planner = IdlePlanner::default();
    close(&mut planner, next_due);
    planner.finished(NODE_A, IdleEnd::NotPerformed);
    assert_eq!(planner.planned_wake(NODE_A), None);
    for now in [T0 + 10 * MINUTE, wake] {
        assert_eq!(
            planner.check(
                NODE_A,
                &production(),
                free(),
                plan(now, Some(next_due)),
                now
            ),
            None,
            "a close not performed is not tried again, and no wake follows it"
        );
    }
}
