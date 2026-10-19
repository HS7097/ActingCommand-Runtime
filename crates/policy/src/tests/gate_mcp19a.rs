// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-6b, MCP-19a (policy key 56; register §1.143 M19A-1): every decision carries
//! its own wake, the value the evaluator computed for it, and the evaluation's wake is the least
//! of them; the wake is never serialized, so an evaluation's bytes are unchanged.

use crate::evaluator::tests::{
    NOW, base_facts, base_resources, decision_for, false_fact, two_task_catalog,
};
use crate::{EvaluationTime, PolicyEvaluation, SchedulingDecisionState, evaluate};

const INSTANCE: &str = "fixture-instance-a";
const FIRST_TASK: &str = "fixture.observe";
const SECOND_TASK: &str = "fixture.observe-secondary";
/// The first task's next occurrence, ten minutes after the evaluation.
const FIRST_WAKE: u64 = NOW + 600_000;
/// The second task's next occurrence, twenty minutes after the evaluation.
const SECOND_WAKE: u64 = NOW + 1_200_000;

/// An hourly clock on the local clock whose first occurrence is `anchor_ms`: not due before it.
fn clock_from(anchor_ms: u64) -> serde_json::Value {
    serde_json::json!({
        "kind": "clock",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": 3_600_000,
            "anchor_ms": anchor_ms
        }
    })
}

/// Two clock tasks on one instance, neither due yet, with different next occurrences.
fn two_waiting_tasks() -> PolicyEvaluation {
    let catalog = two_task_catalog(|tasks| {
        tasks[0]["trigger"] = clock_from(FIRST_WAKE);
        tasks[1]["trigger"] = clock_from(SECOND_WAKE);
        tasks[0]["feedback_stop"] = false_fact();
        tasks[1]["feedback_stop"] = false_fact();
    });
    evaluate(
        &catalog,
        &base_facts(),
        &base_resources(),
        EvaluationTime {
            unix_ms: NOW,
            monotonic_ms: NOW,
        },
        4,
    )
    .expect("evaluation")
}

#[test]
fn mcp19a_each_decision_carries_its_own_wake() {
    let evaluation = two_waiting_tasks();
    let first = decision_for(&evaluation, FIRST_TASK, INSTANCE);
    let second = decision_for(&evaluation, SECOND_TASK, INSTANCE);
    assert_eq!(first.state, SchedulingDecisionState::Blocked, "{first:?}");
    assert_eq!(second.state, SchedulingDecisionState::Blocked, "{second:?}");
    assert_eq!(first.next_wake_unix_ms, Some(FIRST_WAKE));
    assert_eq!(second.next_wake_unix_ms, Some(SECOND_WAKE));
    assert_eq!(
        evaluation.next_wake_unix_ms,
        Some(FIRST_WAKE.min(SECOND_WAKE)),
        "the evaluation's wake is the least decision wake"
    );
}

#[test]
fn mcp19a_the_decision_wake_is_not_serialized() {
    let evaluation = two_waiting_tasks();
    assert!(
        evaluation
            .decisions
            .iter()
            .all(|decision| decision.next_wake_unix_ms.is_some()),
        "every decision has its wake in memory: {:?}",
        evaluation.decisions
    );
    let serialized = serde_json::to_value(&evaluation).expect("serialized evaluation");
    let decisions = serialized["decisions"]
        .as_array()
        .expect("serialized decisions");
    assert_eq!(decisions.len(), evaluation.decisions.len());
    for decision in decisions {
        assert!(
            decision.get("next_wake_unix_ms").is_none(),
            "a serialized decision carries no wake: {decision}"
        );
    }
}
