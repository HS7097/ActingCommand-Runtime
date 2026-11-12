// SPDX-License-Identifier: AGPL-3.0-only

//! IDLE-3 (#447, register §1.332; production v0.12.54: the idle action closed the application
//! between two chained scheduled tasks): the idle planner on a manual clock (plain Unix
//! milliseconds the test advances), with a neutral instance, the scheduler's plans as fixed inputs
//! and the policy's own dispatches as the policy host reads them for each plan ([`RunsRead`]:
//! whether a dispatch of the instance was open as the plan's evaluation read its inputs, the
//! ledger position it read them at, and at each check the latest intent of the instance's
//! dispatches). In the order of the defect: the first task's dispatch is open; an evaluation reads
//! its inputs during the run (the second task's trigger unknown, the next work beyond the gap) and
//! its own check finds the lease still held; the task ends and its lease is released; a check
//! comes before the next evaluation; the dispatch's execution and completion are recorded; the next
//! evaluation reads its inputs; the second task is dispatched.
//!
//! - The check after the release decides nothing on the plan that did not count the first run's
//!   end, although that plan was evaluated no earlier than the last check that found the instance
//!   in use; the next plan shows the second task due and decides nothing either; the second task
//!   is dispatched to an instance that was never closed (`close_app`, and `auto`'s rest and
//!   `close_app`).
//! - When the next plan shows no work closer than the gap, the action runs once on it, with its
//!   wake, as before.
//! - An action or a rest decided on a plan that counts every run does not stand on its key when
//!   the latest plan read there shows work due, work closer than the step's gap, or a run it does
//!   not count; the host leaves the instance alone and the planner is active again.

use crate::idle_action::{
    IdleEnd, IdlePlan, IdlePlanner, IdleStep, IdleView, RunsRead, WakeWork, action_min_gap_ms,
    stands_on_key,
};
use actingcommand_runtime_config::{AutoGaps, IdleAction, IdleConfig};

const MINUTE: u64 = 60_000;
/// The manual clock's first reading.
const T0: u64 = 1_800_000_000_000;
const NODE: &str = "node-a";
/// `idle.return_home_min_gap_ms`'s default.
const HOME: u64 = 10 * MINUTE;
/// `idle.min_gap_ms`'s default, the `close_app` tier's threshold.
const CLOSE: u64 = 30 * MINUTE;
/// `idle.stop_emulator_min_gap_ms`'s default.
const STOP: u64 = 120 * MINUTE;
/// The two wake leads' defaults.
const LEAD: u64 = 5 * MINUTE;
const CLOSE_APP_WAKE: &[WakeWork] = &[WakeWork::StartupPackage];

// ---- The ledger positions of the defect's order.

/// The first task's dispatch intent.
const FIRST_INTENT: u64 = 100;
/// Where the evaluation during the first task's run read its inputs.
const READ_DURING: u64 = 140;
/// Where the evaluation after the first task's completion read its inputs.
const READ_AFTER: u64 = 180;
/// The second task's dispatch intent, after that read.
const SECOND_INTENT: u64 = 181;

/// What the policy host keeps for the plan that dispatched the first task: read before its intent.
const READ_BEFORE_RUN: RunsRead = RunsRead {
    open: false,
    through: FIRST_INTENT - 1,
};
/// What it keeps for the plan of the evaluation during the run: the first dispatch open.
const READ_DURING_RUN: RunsRead = RunsRead {
    open: true,
    through: READ_DURING,
};
/// What it keeps for the plan of the evaluation after the first dispatch's completion.
const READ_AFTER_RUN: RunsRead = RunsRead {
    open: false,
    through: READ_AFTER,
};

// ---- The times of the defect's order (one second, as in production).

/// A sweep check while the first task's run holds the lease.
const LEASED_CHECK: u64 = T0;
/// The evaluation during the run, and its own check right after it, the lease still held.
const EVALUATED_DURING: u64 = T0 + 400;
/// The check before the next evaluation, after `task.completed` and `lease.released`.
const CHECK_BEFORE: u64 = T0 + 900;
/// A check after `policy.execution_recorded` and `policy.dispatch_completed`.
const SETTLED: u64 = T0 + 950;
/// The next evaluation, and its own check before its dispatch is admitted.
const EVALUATED_AFTER: u64 = T0 + 1_700;
/// The second task's dispatch.
const DISPATCHED: u64 = T0 + 1_800;

/// `close_app` with the defaults.
fn production() -> IdleConfig {
    IdleConfig {
        action: IdleAction::CloseApp,
        min_gap_ms: CLOSE,
        close_app_wake_lead_ms: LEAD,
        stop_emulator_wake_lead_ms: LEAD,
    }
}

/// `auto` with the defaults.
fn auto() -> IdleConfig {
    IdleConfig {
        action: IdleAction::Auto(AutoGaps {
            return_home_min_gap_ms: HOME,
            stop_emulator_min_gap_ms: STOP,
        }),
        ..production()
    }
}

/// An instance in no one's use, its emulator running, that every action can bring back.
fn free() -> IdleView {
    IdleView {
        emulator_on: true,
        app_wakeable: true,
        emulator_wakeable: true,
        ..IdleView::default()
    }
}

/// `view` with what the policy host says of the latest plan at a check: `read`, what that plan's
/// evaluation read, and `latest_intent`, the latest intent of the instance's dispatches then.
fn counted(view: IdleView, read: RunsRead, latest_intent: u64) -> IdleView {
    IdleView {
        run_uncounted: !read.counts_every_run(Some(latest_intent)),
        ..view
    }
}

/// A plan evaluated at `at` with no due task and the instance's next work at `next_due`.
const fn plan_at(at: u64, next_due: u64) -> Option<IdlePlan> {
    Some(IdlePlan {
        evaluated_at_unix_ms: at,
        due_now: false,
        next_wake_unix_ms: Some(next_due),
        reading_due_unix_ms: None,
    })
}

/// A plan evaluated at `at` with a task of the instance due.
const fn due_at(at: u64) -> Option<IdlePlan> {
    Some(IdlePlan {
        evaluated_at_unix_ms: at,
        due_now: true,
        next_wake_unix_ms: None,
        reading_due_unix_ms: None,
    })
}

/// One check of `NODE` at `now`: the step asked for, if any, with its time.
fn check(
    planner: &mut IdlePlanner,
    config: &IdleConfig,
    view: IdleView,
    plan: Option<IdlePlan>,
    now: u64,
) -> Option<(u64, IdleStep)> {
    planner
        .check(NODE, config, view, plan, now)
        .map(|step| (now, step))
}

/// The defect's order up to the next evaluation, on `planner` under `config`, the evaluation
/// during the first task's run planning the next work `next_work` after its time: every step
/// asked for, with its time.
fn first_run(
    planner: &mut IdlePlanner,
    config: &IdleConfig,
    next_work: u64,
) -> Vec<(u64, IdleStep)> {
    let leased = IdleView {
        leased: true,
        ..free()
    };
    let during = plan_at(EVALUATED_DURING, EVALUATED_DURING + next_work);
    let mut steps = Vec::new();
    for (now, view, plan) in [
        // The first task's run holds the lease; the latest plan is the one that dispatched it.
        (
            LEASED_CHECK,
            counted(leased, READ_BEFORE_RUN, FIRST_INTENT),
            due_at(LEASED_CHECK - MINUTE),
        ),
        // The evaluation during the run, its own check finding the lease still held: the last
        // check that finds the instance in use, at the plan's own time.
        (
            EVALUATED_DURING,
            counted(leased, READ_DURING_RUN, FIRST_INTENT),
            during,
        ),
        // `task.completed`, `lease.released`, and a check before the next evaluation. The plan
        // was evaluated no earlier than the last check that found the instance in use, so the
        // sampled rule passes it; it read the first dispatch open.
        (
            CHECK_BEFORE,
            counted(free(), READ_DURING_RUN, FIRST_INTENT),
            during,
        ),
        // `policy.execution_recorded`, `policy.dispatch_completed`: the dispatch is closed, and
        // the latest plan is still the one that read it open.
        (
            SETTLED,
            counted(free(), READ_DURING_RUN, FIRST_INTENT),
            during,
        ),
    ] {
        steps.extend(check(planner, config, view, plan, now));
    }
    steps
}

/// The rest of the defect's order on `planner` under `config`: the next evaluation reads its
/// inputs with the first dispatch completed and shows the second task due; the second task is
/// dispatched (its claim waits in the queue, then its run holds the lease). Every step asked for,
/// with its time.
fn second_task(planner: &mut IdlePlanner, config: &IdleConfig) -> Vec<(u64, IdleStep)> {
    let after = counted(free(), READ_AFTER_RUN, FIRST_INTENT);
    assert!(
        !after.run_uncounted,
        "the next plan counts the end of the first run"
    );
    let due = due_at(EVALUATED_AFTER);
    let mut steps = Vec::new();
    steps.extend(check(planner, config, after, due, EVALUATED_AFTER));
    for (now, view) in [
        (
            DISPATCHED,
            IdleView {
                queued: true,
                ..free()
            },
        ),
        (
            DISPATCHED + 1_000,
            IdleView {
                leased: true,
                ..free()
            },
        ),
    ] {
        let view = counted(view, READ_AFTER_RUN, SECOND_INTENT);
        assert!(
            view.run_uncounted,
            "the second dispatch was opened after the plan's read"
        );
        steps.extend(check(planner, config, view, due, now));
    }
    steps
}

#[test]
fn idle_action_waits_for_plan_after_run_end() {
    let config = production();
    let mut planner = IdlePlanner::default();
    // The evaluation during the run plans the next work an hour away, beyond the gap.
    let mut steps = first_run(&mut planner, &config, 60 * MINUTE);
    assert!(
        steps.is_empty(),
        "no step on the plan that did not count the first run's end: {steps:?}"
    );
    steps.extend(second_task(&mut planner, &config));
    assert!(
        steps.is_empty(),
        "no application stop was requested: {steps:?}"
    );
    assert!(
        planner.closed().is_empty(),
        "the second task was dispatched with the application open"
    );
}

#[test]
fn idle_action_waits_for_plan_after_run_end_under_auto() {
    // The evaluation during the run plans the next work 20 minutes away (a rest's gap) or an hour
    // away (`close_app`'s).
    for next_work in [20 * MINUTE, 60 * MINUTE] {
        let config = auto();
        let mut planner = IdlePlanner::default();
        let mut steps = first_run(&mut planner, &config, next_work);
        steps.extend(second_task(&mut planner, &config));
        assert!(
            steps.is_empty(),
            "next work {next_work}: no rest and no application stop: {steps:?}"
        );
        assert!(!planner.rested(NODE), "next work {next_work}: no rest");
        assert!(
            planner.closed().is_empty(),
            "next work {next_work}: the second task was dispatched with the application open"
        );
    }
}

#[test]
fn idle_action_runs_after_fresh_plan() {
    let config = production();
    let mut planner = IdlePlanner::default();
    let mut steps = first_run(&mut planner, &config, 60 * MINUTE);
    // The next evaluation reads its inputs with the first dispatch completed and shows no work
    // closer than the gap: the next task an hour away.
    let next_due = EVALUATED_AFTER + 60 * MINUTE;
    let fresh = plan_at(EVALUATED_AFTER, next_due);
    let after = counted(free(), READ_AFTER_RUN, FIRST_INTENT);
    steps.extend(check(&mut planner, &config, after, fresh, EVALUATED_AFTER));
    // On its key the latest plan is still that one, and the action stands.
    assert!(
        stands_on_key(
            fresh,
            after.run_uncounted,
            action_min_gap_ms(&config, IdleAction::CloseApp),
            EVALUATED_AFTER + 100,
        ),
        "the action stands on its key"
    );
    // Its claim holds the instance until the host reports it done.
    let held = IdleView {
        leased: true,
        ..after
    };
    steps.extend(check(
        &mut planner,
        &config,
        held,
        fresh,
        EVALUATED_AFTER + 200,
    ));
    planner.finished(NODE, IdleEnd::Done);
    // Later checks on the same plan: no second action, and the wake at next due minus the lead.
    let wake = next_due - LEAD;
    for now in [EVALUATED_AFTER + MINUTE, wake - 1, wake] {
        steps.extend(check(&mut planner, &config, after, fresh, now));
    }
    assert_eq!(
        steps,
        [
            (
                EVALUATED_AFTER,
                IdleStep::Act {
                    action: IdleAction::CloseApp,
                    next_due_unix_ms: Some(next_due),
                    wake_at_unix_ms: Some(wake),
                },
            ),
            (
                wake,
                IdleStep::Wake {
                    action: IdleAction::CloseApp,
                    work: CLOSE_APP_WAKE,
                },
            ),
        ],
        "one action on the fresh plan, then its wake"
    );
}

#[test]
fn idle_action_abandoned_on_key_for_newer_plan() {
    // The gap each step is taken from: `close_app`'s `idle.min_gap_ms`, also under `auto`, whose
    // `stop_emulator` is taken from `idle.stop_emulator_min_gap_ms`; a rest's is its `within_ms`.
    let config = production();
    let gap = action_min_gap_ms(&config, IdleAction::CloseApp);
    assert_eq!(gap, CLOSE);
    assert_eq!(action_min_gap_ms(&auto(), IdleAction::CloseApp), CLOSE);
    assert_eq!(action_min_gap_ms(&auto(), IdleAction::StopEmulator), STOP);

    // `close_app` decided on the plan after the first run's end, which counts it.
    let mut planner = IdlePlanner::default();
    let steps = first_run(&mut planner, &config, 60 * MINUTE);
    assert!(steps.is_empty(), "{steps:?}");
    let next_due = EVALUATED_AFTER + 60 * MINUTE;
    let decided_on = plan_at(EVALUATED_AFTER, next_due);
    let after = counted(free(), READ_AFTER_RUN, FIRST_INTENT);
    assert_eq!(
        planner.check(NODE, &config, after, decided_on, EVALUATED_AFTER),
        Some(IdleStep::Act {
            action: IdleAction::CloseApp,
            next_due_unix_ms: Some(next_due),
            wake_at_unix_ms: Some(next_due - LEAD),
        })
    );
    // Before its claim is granted a newer evaluation reads its inputs; the key is granted after.
    let newer_at = EVALUATED_AFTER + 300;
    let on_key = newer_at + 100;
    let newer_due = due_at(newer_at);
    assert!(
        stands_on_key(decided_on, false, gap, on_key),
        "the plan it was decided on still allows it"
    );
    for (latest, run_uncounted, why) in [
        (newer_due, false, "a task due"),
        (
            plan_at(newer_at, newer_at + CLOSE - MINUTE),
            false,
            "work closer than the gap",
        ),
        (
            Some(IdlePlan {
                evaluated_at_unix_ms: newer_at,
                due_now: false,
                next_wake_unix_ms: Some(next_due),
                reading_due_unix_ms: Some(newer_at),
            }),
            false,
            "a reading due",
        ),
        (
            decided_on,
            !READ_AFTER_RUN.counts_every_run(Some(SECOND_INTENT)),
            "a dispatch opened after the plan's read",
        ),
    ] {
        assert!(
            !stands_on_key(latest, run_uncounted, gap, on_key),
            "{why}: the action does not stand on its key"
        );
    }
    // The host leaves the instance alone, as for a pause: nothing is sent, the planner is told.
    planner.abandoned(NODE);
    assert!(planner.closed().is_empty(), "nothing is closed");
    // The planner is active again: nothing on the plan with the task due, and once a later plan
    // shows the gap again, the action.
    let newer = counted(
        free(),
        RunsRead {
            open: false,
            through: READ_AFTER + 10,
        },
        FIRST_INTENT,
    );
    assert_eq!(
        planner.check(NODE, &config, newer, newer_due, on_key + 100),
        None
    );
    let later = on_key + MINUTE;
    assert_eq!(
        planner.check(
            NODE,
            &config,
            newer,
            plan_at(later, later + 60 * MINUTE),
            later
        ),
        Some(IdleStep::Act {
            action: IdleAction::CloseApp,
            next_due_unix_ms: Some(later + 60 * MINUTE),
            wake_at_unix_ms: Some(later + 60 * MINUTE - LEAD),
        })
    );

    // A rest under `auto`, decided on a plan whose next work is 20 minutes away.
    let mut planner = IdlePlanner::default();
    let rest_due = EVALUATED_AFTER + 20 * MINUTE;
    let rest_plan = plan_at(EVALUATED_AFTER, rest_due);
    assert_eq!(
        planner.check(NODE, &auto(), after, rest_plan, EVALUATED_AFTER),
        Some(IdleStep::Rest {
            next_due_unix_ms: Some(rest_due),
            within_ms: HOME,
        })
    );
    assert!(
        stands_on_key(rest_plan, false, HOME, on_key),
        "the plan it was decided on still allows the rest"
    );
    assert!(
        !stands_on_key(newer_due, false, HOME, on_key),
        "a task due: the rest does not stand on its key"
    );
    planner.abandoned(NODE);
    assert!(!planner.rested(NODE), "the rest was not taken");
    let again = on_key + MINUTE;
    assert_eq!(
        planner.check(
            NODE,
            &auto(),
            newer,
            plan_at(again, again + 20 * MINUTE),
            again
        ),
        Some(IdleStep::Rest {
            next_due_unix_ms: Some(again + 20 * MINUTE),
            within_ms: HOME,
        }),
        "active again, the next plan with the gap asks for the rest again"
    );
}
