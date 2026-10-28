// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #427-B (#427 plan r2 point 5; approved whole by Alice 10-28 12:4x;
//! `contracts/scheduling/README.md`, "Shared run limits"): `ac_schedule` answers a candidate
//! the shared run limits hold back with its numbers. The Runtime's `ScheduleExplanation` carries
//! each row's `why` as the full outcome of its last reason, and the tool passes it through: a
//! `policy_concurrent_runs_deferred` or `heavy_scene_budget_deferred` row keeps `used`,
//! `limit_count`, `admitted_count` and `round_selected_count`, in the not-running row and in the
//! candidate rows, through the output budget and the tool result.
//!
//! Workflow #427-B follow-up (review L1): `ac_diagnose`'s `not_run` lists, beside the deferred
//! rows, a row whose state is `blocked` and whose `why` is a shared run limit (the evaluator
//! blocks a heavy candidate at placement with `heavy_scene_budget_exhausted`), with its numbers;
//! a row blocked by its own trigger stays out.

use super::super::protocol::Era;
use super::super::schedule::within_budget;
use super::super::tools::{ToolSuccess, call_result};
use super::gate_mcp19b::{bound, listed, render};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    RuntimeScheduleExplanation, ScheduleCandidateEligibility, ScheduleCandidateState,
    ScheduleExplanationCandidate, ScheduleExplanationInstance, ScheduleExplanationNotRunning,
};
use actingcommand_policy::PolicyCode;
use serde_json::{Value, json};

const EVALUATED_AT: u64 = 1_700_042_460_000;

/// A shared run limit's reason with its numbers: `used` of `limit`, `admitted` and `chosen`.
fn slot_outcome(code: PolicyCode, admitted: u16, chosen: u16, limit: u16) -> Outcome {
    Outcome::new(code)
        .used(admitted + chosen)
        .limit_count(limit)
        .admitted_count(admitted)
        .round_selected_count(chosen)
}

/// A due row deferred with `why`.
fn deferred(task_id: &str, why: Outcome) -> ScheduleExplanationCandidate {
    ScheduleExplanationCandidate {
        task_id: task_id.to_owned(),
        state: ScheduleCandidateState::Deferred,
        eligibility: ScheduleCandidateEligibility::True,
        reasons: vec![
            Outcome::relayed("task_eligible").code().clone(),
            why.code().clone(),
        ],
        why,
        reasons_omitted: None,
        next_wake_unix_ms: None,
        since_unix_ms: EVALUATED_AT,
        expected_runtime_ms: None,
        expected_runtime_source: None,
    }
}

fn values(used: i64, limit: i64, admitted: i64, chosen: i64) -> Value {
    json!({
        "used": used,
        "limit_count": limit,
        "admitted_count": admitted,
        "round_selected_count": chosen,
    })
}

#[test]
fn gate_427b_ac_schedule_answers_the_shared_run_limits_with_their_numbers() {
    let concurrent = deferred(
        "fixture.routine",
        slot_outcome(PolicyCode::PolicyConcurrentRunsDeferred, 2, 1, 3),
    );
    let heavy = deferred(
        "fixture.heavy",
        slot_outcome(PolicyCode::HeavySceneBudgetDeferred, 1, 1, 2),
    );
    let explanation = RuntimeScheduleExplanation {
        evaluated_at_unix_ms: Some(EVALUATED_AT),
        catalog_hash: Some(format!("sha256:{}", "0".repeat(64))),
        instances: vec![ScheduleExplanationInstance {
            instance_alias: "neutral.fourth".to_owned(),
            next_wake_unix_ms: None,
            selected: None,
            not_running: ScheduleExplanationNotRunning {
                task_id: Some(concurrent.task_id.clone()),
                why: concurrent.why.clone(),
                next_wake_unix_ms: None,
                since_unix_ms: EVALUATED_AT,
            },
            candidates_total: 2,
            candidates: vec![concurrent, heavy],
        }],
    };
    assert_eq!(explanation.validate(), Ok(()), "setup: a valid answer");

    let ToolSuccess { result, .. } = within_budget(json!(explanation));
    let entry = &result["instances"][0];
    assert_eq!(
        entry["not_running"]["why"]["code"],
        json!("policy_concurrent_runs_deferred")
    );
    assert_eq!(entry["not_running"]["why"]["values"], values(3, 3, 2, 1));
    assert_eq!(entry["candidates"][0]["why"]["values"], values(3, 3, 2, 1));
    assert_eq!(
        entry["candidates"][1]["why"]["code"],
        json!("heavy_scene_budget_deferred")
    );
    assert_eq!(entry["candidates"][1]["why"]["values"], values(2, 2, 1, 1));

    let answered = call_result(Ok(ToolSuccess::new(result)), Era::Modern);
    assert_eq!(
        answered["structuredContent"]["result"]["instances"][0]["candidates"][1]["why"]["values"],
        values(2, 2, 1, 1),
        "{answered}"
    );
}

#[test]
fn gate_427b_ac_diagnose_lists_a_row_the_shared_run_limits_blocked_with_its_numbers() {
    // Both heavy-scene slots of the host are taken by dispatches admitted or running: the heavy
    // candidate is blocked at placement, 2 of 2, none chosen in the round. The other row waits
    // for its own trigger.
    let exhausted = ScheduleExplanationCandidate {
        state: ScheduleCandidateState::Blocked,
        ..deferred(
            "fixture.heavy",
            slot_outcome(PolicyCode::HeavySceneBudgetExhausted, 2, 0, 2),
        )
    };
    let waiting = ScheduleExplanationCandidate {
        task_id: "fixture.later".to_owned(),
        state: ScheduleCandidateState::Blocked,
        eligibility: ScheduleCandidateEligibility::False,
        why: Outcome::relayed("trigger_false"),
        reasons: vec![Outcome::relayed("trigger_false").code().clone()],
        reasons_omitted: None,
        next_wake_unix_ms: Some(EVALUATED_AT + 3_600_000),
        since_unix_ms: EVALUATED_AT,
        expected_runtime_ms: None,
        expected_runtime_source: None,
    };
    let explanation = RuntimeScheduleExplanation {
        evaluated_at_unix_ms: Some(EVALUATED_AT),
        catalog_hash: Some(format!("sha256:{}", "0".repeat(64))),
        instances: vec![ScheduleExplanationInstance {
            instance_alias: "neutral.third".to_owned(),
            next_wake_unix_ms: Some(EVALUATED_AT + 3_600_000),
            selected: None,
            not_running: ScheduleExplanationNotRunning {
                task_id: Some(exhausted.task_id.clone()),
                why: exhausted.why.clone(),
                next_wake_unix_ms: None,
                since_unix_ms: EVALUATED_AT,
            },
            candidates_total: 2,
            candidates: vec![exhausted.clone(), waiting],
        }],
    };
    assert_eq!(explanation.validate(), Ok(()), "setup: a valid answer");

    let diagnosed = render(&explanation, bound(10));
    assert_eq!(
        diagnosed.result["not_run"],
        json!([listed("neutral.third", &exhausted)]),
        "{:#}",
        diagnosed.result
    );
    let row = &diagnosed.result["not_run"][0];
    assert_eq!(row["state"], json!("blocked"));
    assert_eq!(row["why"]["code"], json!("heavy_scene_budget_exhausted"));
    assert_eq!(row["why"]["values"], values(2, 2, 2, 0));
    assert_eq!(diagnosed.result.get("not_run_total"), None);
    assert!(diagnosed.warnings.is_empty());
}
