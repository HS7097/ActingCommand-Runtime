// SPDX-License-Identifier: AGPL-3.0-only

//! IDLE-WG (0.12.27; brief rd-1029/brief-wave2c.md; #421 6476382014). Production v0.12.26 on
//! 10-29 02:43: an install hold's release resumed an instance whose application the idle action
//! had closed (its wake 05:55 for its 06:00 task). While the resume's reconnect claim held the
//! instance's key, the evaluation deferred its candidates `dispatch_instance_held` with no wake (a
//! hold comes before every other gate), the idle plan counted them due and woke the closed game,
//! and the candidates' closed activity window then refused them: the startup package ran for
//! nothing and the action closed the game again. The required state, on a manual clock, from a
//! recorded evaluation with the instances a time-bound gate holds back as the policy host derives
//! them (`idle_held_back`):
//! - A: an idle instance whose only due candidates its closed activity window holds back (or a
//!   blackout that covers it) is not woken at the resume, also while a hold is what their decisions
//!   name; it keeps its wake at the time the gates let them through minus the lead (05:55 for the
//!   window opening at 06:00), and is woken then.
//! - B: an idle instance with a candidate really due (its window open, no blackout) is woken at
//!   once, under the resume's claim, a pause or free, as before.
//!
//! W1-2's woken and owed cases (`gate_idle1p`: O-2, M-1, N-1, N-2, L-1 to L-3) are unchanged and
//! not repeated here.

use crate::idle_action::{IdleEnd, IdlePlan, IdlePlanner, IdleStep, IdleView, WakeWork};
use crate::schedule_explanation::{ScheduleExplanationTable, idle_held_back};
use actingcommand_contract::outcome::BlackoutSource;
use actingcommand_policy::{
    ActivityProfile, ActivityWindow, Blackout, DetectionBudget, EligibilityState, PolicyEvaluation,
    ResamplePolicy, SchedulingDecisionState, ScopeSelector, SeedSource, TaskDecision,
};
use actingcommand_runtime_config::{IdleAction, IdleConfig};
use std::collections::BTreeMap;

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const NODE: &str = "node-a";
/// A UTC midnight: every time below is of that day.
const DAY0: u64 = 1_799_971_200_000;
/// The instance's activity window opens at 06:00 and closes at 23:00 (UTC), every day.
const OPENS: u64 = DAY0 + 6 * HOUR_MS;
/// The production keys of 10-29: `close_app`, its wake lead 300000, `idle.min_gap_ms` 1800000.
const LEAD: u64 = 5 * MINUTE_MS;
const PRODUCTION: IdleConfig = IdleConfig {
    action: IdleAction::CloseApp,
    min_gap_ms: 30 * MINUTE_MS,
    close_app_wake_lead_ms: LEAD,
    stop_emulator_wake_lead_ms: 5 * MINUTE_MS,
};
const WAKE: IdleStep = IdleStep::Wake {
    action: IdleAction::CloseApp,
    work: &[WakeWork::StartupPackage],
};

/// The instance's activity profile: one window, 06:00 to 23:00 every day.
fn profile() -> ActivityProfile {
    ActivityProfile {
        id: "activity.idle_wg".to_owned(),
        scope: ScopeSelector::Instance {
            instance_id: NODE.to_owned(),
        },
        windows: vec![ActivityWindow {
            weekdays: (1..=7).collect(),
            utc_offset_minutes: 0,
            start_minute_of_day: 6 * 60,
            end_minute_of_day: 23 * 60,
        }],
        daily_budget: Some(100),
        max_window_iterations: Some(100),
        session_max_ms: Some(4 * HOUR_MS),
        detection_budget: DetectionBudget {
            window_dispatch_limit: 1,
            window_runtime_ms: MINUTE_MS,
            expected_duration_ms: MINUTE_MS,
        },
        minimum_interval_ms: 0,
        maximum_interval_ms: 0,
        seed_source: SeedSource::Ledger,
        resample_policy: ResamplePolicy::SameRoundStable,
        importance_milli: 0,
        goals: Vec::new(),
    }
}

/// A blackout that covers the instance from `start` to `end`.
fn blackout(start: u64, end: u64) -> BTreeMap<String, Blackout> {
    BTreeMap::from([(
        NODE.to_owned(),
        Blackout {
            id: "blackout.idle_wg".to_owned(),
            source: BlackoutSource::Host,
            start_unix_ms: start,
            end_unix_ms: end,
        },
    )])
}

fn free() -> IdleView {
    IdleView {
        emulator_on: true,
        app_wakeable: true,
        emulator_wakeable: true,
        ..IdleView::default()
    }
}

/// The resume's reconnect claim holds the instance's key.
fn reconnecting() -> IdleView {
    IdleView {
        leased: true,
        ..free()
    }
}

fn decision(
    task_id: &str,
    eligibility: EligibilityState,
    state: SchedulingDecisionState,
    next_wake_unix_ms: Option<u64>,
) -> TaskDecision {
    TaskDecision {
        task_id: task_id.to_owned(),
        instance_id: Some(NODE.to_owned()),
        eligibility,
        state,
        rank: None,
        detection_suggestions: Vec::new(),
        reasons: Vec::new(),
        next_wake_unix_ms,
    }
}

/// The three candidates whose triggers are true, as a hold of the instance (the resume's claim,
/// `dispatch_instance_held`; a pause) leaves them: eligible, deferred with no wake.
fn held_candidates() -> Vec<TaskDecision> {
    ["task.first", "task.second", "task.third"]
        .into_iter()
        .map(|task_id| {
            decision(
                task_id,
                EligibilityState::True,
                SchedulingDecisionState::Deferred,
                None,
            )
        })
        .collect()
}

/// The same candidates once no hold names them: the gate that holds them back defers them to its
/// end (`policy_activity_window_closed` to the window's opening, a blackout to its end).
fn deferred_to(through: u64) -> Vec<TaskDecision> {
    held_candidates()
        .into_iter()
        .map(|decision| TaskDecision {
            next_wake_unix_ms: Some(through),
            ..decision
        })
        .collect()
}

/// The instance's next task at `next_due`: its trigger is false until then.
fn later_task(next_due: u64) -> TaskDecision {
    decision(
        "task.later",
        EligibilityState::False,
        SchedulingDecisionState::Blocked,
        Some(next_due),
    )
}

/// The idle check's plan from an evaluation of `decisions` at `at`, with the instances a
/// time-bound gate holds back at `at` (the instance's activity window and `blackouts`), as the
/// policy host records it.
fn explained(
    decisions: Vec<TaskDecision>,
    at: u64,
    blackouts: &BTreeMap<String, Blackout>,
) -> Option<IdlePlan> {
    let profile = profile();
    let held_back = idle_held_back([(NODE, Some(&profile))], blackouts, at).expect("held back");
    let mut table = ScheduleExplanationTable::default();
    table.record_held_back(
        &PolicyEvaluation {
            decisions,
            next_wake_unix_ms: None,
            preload_hint: None,
            dispatch_intents: Vec::new(),
            reason_chains: Vec::new(),
        },
        "sha256:idle-wg",
        at,
        &|_: &str, _: &str| -> Option<(u64, String)> { None },
        &held_back,
        crate::application_update::IdleUpdateHold::default(),
    );
    table.idle_plan(NODE)
}

/// `close_app` on the instance at `at` for its next task at `next_due`, done: it owes its wake at
/// `next_due` minus the lead.
fn closed(at: u64, next_due: u64) -> IdlePlanner {
    let mut planner = IdlePlanner::default();
    let plan = explained(vec![later_task(next_due)], at, &BTreeMap::new());
    assert_eq!(
        planner.check(NODE, &PRODUCTION, free(), plan, at),
        Some(IdleStep::Act {
            action: IdleAction::CloseApp,
            next_due_unix_ms: Some(next_due),
            wake_at_unix_ms: Some(next_due - LEAD),
        })
    );
    planner.finished(NODE, IdleEnd::Done);
    planner
}

#[test]
fn idle_wg_a_a_task_a_time_bound_gate_holds_back_does_not_wake_the_idle_instance_at_resume() {
    struct Case {
        what: &'static str,
        closed_at: u64,
        next_due: u64,
        resume: u64,
        blackouts: BTreeMap<String, Blackout>,
        /// When the gates let the candidates through.
        through: u64,
    }
    for case in [
        // The production case: closed at 01:00 for its 06:00 task (wake 05:55), resumed at 02:43
        // with its window closed until 06:00.
        Case {
            what: "the closed activity window",
            closed_at: DAY0 + HOUR_MS,
            next_due: OPENS,
            resume: DAY0 + 2 * HOUR_MS + 43 * MINUTE_MS,
            blackouts: BTreeMap::new(),
            through: OPENS,
        },
        // Closed at 06:30 for its 12:00 task (wake 11:55), resumed at 07:00 inside a blackout
        // that covers it from 06:45 to 08:00, its window open: the wake moves to 07:55.
        Case {
            what: "a blackout covering the instance",
            closed_at: OPENS + 30 * MINUTE_MS,
            next_due: DAY0 + 12 * HOUR_MS,
            resume: DAY0 + 7 * HOUR_MS,
            blackouts: blackout(OPENS + 45 * MINUTE_MS, DAY0 + 8 * HOUR_MS),
            through: DAY0 + 8 * HOUR_MS,
        },
        // A blackout from 02:00 to 04:00 while the window is closed until 06:00: the gates let
        // the candidates through at the opening, not at the blackout's end.
        Case {
            what: "a blackout ending before the window opens",
            closed_at: DAY0 + HOUR_MS,
            next_due: OPENS,
            resume: DAY0 + 2 * HOUR_MS + 43 * MINUTE_MS,
            blackouts: blackout(DAY0 + 2 * HOUR_MS, DAY0 + 4 * HOUR_MS),
            through: OPENS,
        },
    ] {
        let what = case.what;
        let profile = profile();
        assert_eq!(
            idle_held_back([(NODE, Some(&profile))], &case.blackouts, case.resume)
                .expect("held back"),
            BTreeMap::from([(NODE.to_owned(), Some(case.through))]),
            "{what}: the instance is held back until the gates let its candidates through"
        );
        let mut planner = closed(case.closed_at, case.next_due);
        let wake = case.through.min(case.next_due) - LEAD;
        // The resume: its reconnect claim holds the instance and its candidates wait under that
        // hold with no wake.
        let plan = explained(
            [held_candidates(), vec![later_task(case.next_due)]].concat(),
            case.resume,
            &case.blackouts,
        );
        let read = plan.expect("an evaluated plan");
        assert!(
            !read.due_now,
            "{what}: no candidate could be dispatched now"
        );
        assert_eq!(
            read.next_wake_unix_ms,
            Some(case.through.min(case.next_due)),
            "{what}: the candidates wake when the gates let them through"
        );
        assert_eq!(
            planner.check(NODE, &PRODUCTION, reconnecting(), plan, case.resume),
            None,
            "{what}: the idle instance is not woken at the resume"
        );
        assert_eq!(
            planner.planned_wake(NODE),
            Some(wake),
            "{what}: it keeps its wake at the gates' end minus the lead"
        );
        // The claim has ended: the gate itself now defers the candidates to its end.
        let after = case.resume + 2 * MINUTE_MS;
        let plan = explained(
            [deferred_to(case.through), vec![later_task(case.next_due)]].concat(),
            after,
            &case.blackouts,
        );
        assert_eq!(
            planner.check(NODE, &PRODUCTION, free(), plan, after),
            None,
            "{what}: still not woken once the claim has ended"
        );
        assert_eq!(planner.planned_wake(NODE), Some(wake), "{what}");
        // Its wake: brought back the lead before the gates let the candidates through.
        let plan = explained(
            [deferred_to(case.through), vec![later_task(case.next_due)]].concat(),
            wake,
            &case.blackouts,
        );
        assert_eq!(
            planner.check(NODE, &PRODUCTION, free(), plan, wake),
            Some(WAKE),
            "{what}: woken at its wake"
        );
    }
}

#[test]
fn idle_wg_b_a_really_due_task_still_wakes_the_idle_instance() {
    // Closed at 06:30 (its window open) for its 12:00 task: the wake 11:55.
    let next_due = DAY0 + 12 * HOUR_MS;
    let now = DAY0 + 8 * HOUR_MS;
    let profile = profile();
    assert_eq!(
        idle_held_back([(NODE, Some(&profile))], &BTreeMap::new(), now).expect("held back"),
        BTreeMap::new(),
        "the window is open and no blackout covers the instance"
    );
    let paused = IdleView {
        paused: true,
        ..free()
    };
    let selected = decision(
        "task.first",
        EligibilityState::True,
        SchedulingDecisionState::Selected,
        None,
    );
    for (what, view, due) in [
        (
            "held by the resume's claim",
            reconnecting(),
            held_candidates(),
        ),
        ("held by a pause", paused, held_candidates()),
        ("free and selected", free(), vec![selected]),
    ] {
        let mut planner = closed(OPENS + 30 * MINUTE_MS, next_due);
        let plan = explained(
            [due, vec![later_task(next_due)]].concat(),
            now,
            &BTreeMap::new(),
        );
        assert!(
            plan.is_some_and(|plan| plan.due_now),
            "{what}: the task is due: {plan:?}"
        );
        assert_eq!(
            planner.check(NODE, &PRODUCTION, view, plan, now),
            Some(WAKE),
            "{what}: the idle instance is woken at once"
        );
    }
}
