// SPDX-License-Identifier: AGPL-3.0-only

//! SS-2d (0.12.35; register §1.235 SS2d-5; design `rd-1031/design-acm-price-addendum2.1.md` §7.3
//! "空闲计划" and §7.6 test 17): the idle plan counts no decision within its rounding error
//! (`benefit_within_tolerance` among its reasons, released or deferred) as due or as a wake of
//! its own, as SS-2b's held decisions are never due (SS2b-7). A zero tier only fills an open
//! game's idle time; it never wakes a closed game for work worth 0. A selected decision is due
//! as before.

use crate::schedule_explanation::ScheduleExplanationTable;
use actingcommand_contract::outcome::Outcome;
use actingcommand_policy::{
    DecisionReason, EligibilityState, PolicyCode, PolicyEvaluation, SchedulingDecisionState,
    TaskDecision,
};

const NODE: &str = "node-a";
const MINUTE_MS: u64 = 60_000;
/// The evaluation instant.
const AT: u64 = 1_799_971_200_000;

fn reason(code: PolicyCode) -> DecisionReason {
    let detail = format!("fixture: {}", code.code().as_str());
    DecisionReason {
        outcome: Outcome::new(code),
        detail,
    }
}

/// An eligible decision of `task_id` in `state` with `reasons` and its own wake.
fn decision(
    task_id: &str,
    state: SchedulingDecisionState,
    reasons: Vec<DecisionReason>,
    next_wake_unix_ms: Option<u64>,
) -> TaskDecision {
    TaskDecision {
        task_id: task_id.to_owned(),
        instance_id: Some(NODE.to_owned()),
        eligibility: EligibilityState::True,
        state,
        rank: None,
        detection_suggestions: Vec::new(),
        reasons,
        next_wake_unix_ms,
    }
}

/// Within tolerance and not released: deferred, its wake the next step of a regenerating pool.
fn unmarked() -> TaskDecision {
    decision(
        "task.unmarked",
        SchedulingDecisionState::Deferred,
        vec![reason(PolicyCode::BenefitWithinTolerance)],
        Some(AT + 6 * MINUTE_MS),
    )
}

/// Released, then held by the reserve gate, with the gate's wake.
fn released_held() -> TaskDecision {
    decision(
        "task.released",
        SchedulingDecisionState::Deferred,
        vec![
            reason(PolicyCode::BenefitWithinTolerance),
            DecisionReason {
                outcome: Outcome::relayed("resource_reserve_insufficient"),
                detail: "fixture: resource_reserve_insufficient".to_owned(),
            },
        ],
        Some(AT + 10 * MINUTE_MS),
    )
}

/// The instance's idle plan from an evaluation of `decisions` at AT.
fn plan(decisions: Vec<TaskDecision>) -> crate::idle_action::IdlePlan {
    let mut table = ScheduleExplanationTable::default();
    table.record(
        &PolicyEvaluation {
            decisions,
            next_wake_unix_ms: None,
            preload_hint: None,
            dispatch_intents: Vec::new(),
            reason_chains: Vec::new(),
        },
        "sha256:ss2d",
        AT,
        &|_: &str, _: &str| -> Option<(u64, String)> { None },
    );
    table.idle_plan(NODE).expect("an evaluated plan")
}

// Test 17: an instance with only decisions within tolerance, one deferred, one released and then
// held, is not due and has no wake. Beside them, SS-2b's not-positive decision keeps its own wake
// (SS2b-7), and a released decision that was selected is due as any selected decision.
#[test]
fn ss2d_t17_a_decision_within_tolerance_is_neither_due_nor_a_wake() {
    let only = plan(vec![unmarked(), released_held()]);
    assert!(!only.due_now, "{only:?}");
    assert_eq!(only.next_wake_unix_ms, None, "{only:?}");

    let beside = plan(vec![
        unmarked(),
        released_held(),
        decision(
            "task.nonpositive",
            SchedulingDecisionState::Deferred,
            vec![reason(PolicyCode::PolicyBenefitNonpositive)],
            Some(AT + 20 * MINUTE_MS),
        ),
    ]);
    assert!(!beside.due_now, "{beside:?}");
    assert_eq!(
        beside.next_wake_unix_ms,
        Some(AT + 20 * MINUTE_MS),
        "only the not-positive decision's own wake: {beside:?}"
    );

    let selected = plan(vec![
        unmarked(),
        decision(
            "task.released",
            SchedulingDecisionState::Selected,
            vec![reason(PolicyCode::BenefitWithinTolerance)],
            None,
        ),
    ]);
    assert!(selected.due_now, "{selected:?}");
}
