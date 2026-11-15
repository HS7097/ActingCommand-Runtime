// SPDX-License-Identifier: AGPL-3.0-only

//! `run show` (Workflow #434 LAB-1b; register §1.287), moved from MCP's `ac_get_run` (#338 R1):
//! one contained run, manual or scheduled, by the request that submitted it or by its run, as
//! runtime-client's `actingcommand.run-status.v1` projection in full, read from its ledger events
//! (`QueryEvents`) and passed on unchanged. Waiting while the run may still change, and the jobs
//! of the MCP process, stay with MCP.

use crate::run_evidence::{parse_request_id, parse_run_id};
use actingcommand_runtime_client::{
    ContainedRunState, ContainedRunStatus, RunKey, RunStatusMode, RuntimeClient, RuntimeClientError,
};
use serde_json::{Value, json};

/// The run the request `request_id` submitted; `None` for text that is no request_id.
pub fn by_request_id(request_id: &str) -> Option<RunKey> {
    parse_request_id(request_id).map(RunKey::RequestId)
}

/// The run `run_id`; `None` for text that is no run_id.
pub fn by_run_id(run_id: &str) -> Option<RunKey> {
    parse_run_id(run_id).map(RunKey::RunId)
}

/// `run show`: the run `key` names, in full.
pub fn read(client: &RuntimeClient, key: RunKey) -> Result<ContainedRunStatus, RuntimeClientError> {
    client.contained_run_status(key, RunStatusMode::Full)
}

/// Whether a later read may show the run otherwise: not found yet, admitted or running.
pub fn may_change(status: &ContainedRunStatus) -> bool {
    matches!(
        status.state,
        ContainedRunState::NotFound | ContainedRunState::Admitted | ContainedRunState::Running
    )
}

/// The answer: the projection itself.
pub fn answer(status: &ContainedRunStatus) -> Value {
    json!(status)
}

// Workflow #469 F1 (register §1.387), the run show's half since Workflow #434 LAB-7 (pass 4, the
// second review's (f)): the test of `ac_get_run` that held it left with actingctl. The Lab's
// `run show` answers the run status as it is: the answer is the status's JSON whole, its
// terminal's `failure_code` and the `failure_values` codes mode keeps of the terminal outcome
// (the retired test's failed outcome: a vocabulary word, an id and a duration) among it.
#[cfg(test)]
mod f1_tests {
    use super::answer;
    use actingcommand_contract::outcome::Outcome;
    use actingcommand_contract::{
        IdentifierIssuer, RuntimeErrorCode, RuntimeReceiptState, TaskOutcome,
    };
    use actingcommand_runtime_client::{
        ContainedRunEvidence, ContainedRunState, ContainedRunStatus, ContainedRunTerminal,
        RunDispatch, RunOrigin,
    };
    use serde_json::json;

    const FAILURE: RuntimeErrorCode = RuntimeErrorCode::ContainedTaskDeadlineExceeded;

    #[test]
    fn f1_run_show_answers_the_run_status_as_it_is() {
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let failure = Outcome::new(FAILURE)
            .receipt_state(RuntimeReceiptState::Failed)
            .lease_id("lease_00000000000000000000000000000001")
            .retry_after_ms(1_500_u64);
        let status = ContainedRunStatus {
            schema_version: "actingcommand.run-status.v1",
            request_id: Some(*issuer.mint_request_id().expect("request id").transport()),
            correlation_id: Some(
                *issuer
                    .mint_correlation_id()
                    .expect("correlation id")
                    .transport(),
            ),
            run_id: Some(*issuer.mint_run_id().expect("run id").transport()),
            task_id: Some(*issuer.mint_task_id().expect("task id").transport()),
            instance_id: Some(*issuer.mint_instance_id().expect("instance id").transport()),
            dispatch: RunDispatch::Manual,
            origin: RunOrigin::Lab,
            package_label: Some("neutral.package".to_owned()),
            recovery_package_labels: Vec::new(),
            state: ContainedRunState::Failed,
            terminal: Some(ContainedRunTerminal {
                outcome: TaskOutcome::Failure,
                failure_code: Some(failure.code().clone()),
                failure_values: failure.codes_only().values().clone(),
                final_page: Some("neutral/page_failed".to_owned()),
                executed_steps: Some(1),
                cancellation_reason: None,
                sequence: 2,
                event_id: *issuer.mint_event_id().expect("event id").transport(),
            }),
            lease: None,
            progress: None,
            evidence: ContainedRunEvidence {
                snapshot_ledger_position: 2,
                admitted_sequence: Some(1),
                restarted_after_admission: false,
            },
        };
        let answered = answer(&status);
        assert_eq!(
            answered,
            serde_json::to_value(&status).expect("the run status encodes"),
            "run show answers the status whole"
        );
        assert_eq!(
            answered["terminal"]["failure_code"],
            json!(FAILURE.as_str()),
            "{answered}"
        );
        // The word alone, without the id or the duration.
        assert_eq!(
            answered["terminal"]["failure_values"],
            json!({"receipt_state": "failed"}),
            "{answered}"
        );
    }
}
