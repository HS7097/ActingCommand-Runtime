// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #427-B (#427 plan r2 point 5; approved whole by Alice 10-28 12:4x;
//! `contracts/scheduling/README.md`, "Shared run limits"): `ac_schedule` answers a candidate
//! the shared run limits hold back with its numbers. The Runtime's `ScheduleExplanation` carries
//! each row's `why` as the full outcome of its last reason, and the tool passes it through: a
//! `policy_concurrent_runs_deferred` or `heavy_scene_budget_deferred` row keeps `used`,
//! `limit_count`, `admitted_count` and `round_selected_count`, in the not-running row and in the
//! candidate rows, through the output budget and the tool result.

use super::super::protocol::Era;
use super::super::schedule::within_budget;
use super::super::tools::{ToolSuccess, call_result};
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
