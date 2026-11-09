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
