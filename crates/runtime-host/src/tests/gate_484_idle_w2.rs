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
//! Every scenario runs one task whose trigger is always true (an interval clock of 1 ms, its
//! `daily_limit` 2) on the fixture instance of `gate_372k2` (activity window 06:00-23:30 UTC, a
//! manual Runtime clock, the no-progress guard off), reads its idle plan as the idle check reads it
//! (`RuntimeHost::idle_plan_and_runs_counted_for_test`) and feeds it to the pure idle planner under
//! the production keys (`close_app`, its wake lead 300 000 ms, `idle.min_gap_ms` 1 800 000 ms).
//!
//! - The reproduction: the task's budget spent, the idle check closes the application with its
//!   wake at the budget's reset less the lead; while a client holds the instance's key the
//!   evaluation defers the task `dispatch_instance_held` with no wake, and the idle plan still
//!   counts it not due, its wake at the reset: no wake follows the close.

use super::gate_372k2::{SlotTask, Slots, at, slot_sources};
use super::*;
use crate::codes::HostCode;
use crate::idle_action::{IdleEnd, IdlePlan, IdlePlanner, IdleStep, IdleView};
use actingcommand_policy::{EligibilityState, SchedulingDecisionState, TaskDecision};
use actingcommand_runtime_config::{IdleAction, IdleConfig};
use serde_json::json;

const TASK: &str = "fixture.idle_w2";
const ALIAS: &str = POLICY_INSTANCE_ALIAS;
/// `idle.close_app_wake_lead_ms` in production.
const LEAD_MS: u64 = 300_000;
/// The production keys of 11-19: `close_app`, its wake lead, `idle.min_gap_ms` 1 800 000.
const PRODUCTION: IdleConfig = IdleConfig {
    action: IdleAction::CloseApp,
    min_gap_ms: 1_800_000,
    close_app_wake_lead_ms: LEAD_MS,
    stop_emulator_wake_lead_ms: LEAD_MS,
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
