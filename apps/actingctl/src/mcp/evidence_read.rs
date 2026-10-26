// SPDX-License-Identifier: AGPL-3.0-only

//! `ac_evidence_confirm_read` (#422 LN-7c C; `rd-1026/ln7c/final-query.md` §4), the MCP side of
//! `actingctl evidence confirm-read` (`crate::evidence_read`). An operator tool, since it writes:
//! on its own (Agent, Adapter) connection it begins an interaction and records the confirmation,
//! one `client.action` of surface `mcp` (control `evidence_read_frame` or `evidence_read_run`,
//! the target's id as its value). That record is the call's only one: no provenance record of the
//! tool is written beside it, and (Agent, Adapter) declares no identity card. A target the Runtime
//! does not hold is its refusal, passed on unchanged (`client_action_invalid`). The answer is the
//! CLI's, without its outcome.

use super::runtime::client_error;
use super::tools::{Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument};
use crate::codes::ArgumentProblem;
use crate::evidence_read::{self, Failure, Target};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{EventActor, EventSource};
use serde_json::{Map, Value, json};

/// The `surface_id` of the confirmations MCP records.
const MCP_SURFACE: &str = "mcp";

pub(super) const DESCRIPTION: &str = "Confirms that you read one evidence frame (artifact_id) or one run (run_id), as actingctl evidence confirm-read does; give exactly one. The Runtime records the confirmation as one client.action (control evidence_read_frame or evidence_read_run, surface mcp) with the agent's origin, and the time; a run's confirmation counts for the frames captured in it before the confirmation. A confirmed evidence frame may be removed by the frame cleaner at any age, an unconfirmed one after its maximum age. Opening or reading a frame (ac_run_evidence, ac_observe, ac_material) is no confirmation, so confirm only what you actually read. An artifact_id that names no capture frame of the ledger, or a run_id that names no run with a capture frame, is refused with client_action_invalid and nothing is recorded. Answers {target: {artifact_id} or {run_id}, event_id, sequence}: the recorded client.action.";

/// `ac_evidence_confirm_read {artifact_id}` or `{run_id}`.
pub(super) fn confirm(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["artifact_id", "run_id"])?;
    let target = match (
        arguments.string("artifact_id")?,
        arguments.string("run_id")?,
    ) {
        (Some(text), None) => Target::frame(text)
            .ok_or_else(|| invalid_argument("artifact_id", ArgumentProblem::NotRecognised))?,
        (None, Some(text)) => Target::run(text)
            .ok_or_else(|| invalid_argument("run_id", ArgumentProblem::NotRecognised))?,
        (Some(_), Some(_)) => {
            return Err(invalid_argument("run_id", ArgumentProblem::Conflicting));
        }
        (None, None) => return Err(invalid_argument("artifact_id", ArgumentProblem::Required)),
    };
    let client = context
        .runtime
        .connect_fresh(EventActor::Agent, EventSource::Adapter)?
        .begin_interaction()
        .map_err(|error| client_error(&error))?;
    let answer =
        evidence_read::confirm(&client, MCP_SURFACE, target).map_err(|failure| match failure {
            Failure::Runtime(error) => client_error(&error),
            Failure::Record(_) => ToolError::usage(SharedCode::ClientActionInvalid),
        })?;
    Ok(ToolSuccess::new(answer))
}

pub(super) fn input() -> Value {
    let id = |what: &str| {
        json!({
            "type": "string",
            "minLength": 1,
            "description": what,
        })
    };
    json!({
        "type": "object",
        "properties": {
            "artifact_id": id("The artifact id of the capture frame you read (artifact_<32 hex>)."),
            "run_id": id("The run you read (run_<32 hex>): every frame captured in it before this confirmation."),
        },
        "additionalProperties": false,
    })
}

pub(super) fn result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "target": {
                "type": "object",
                "properties": {
                    "artifact_id": {"type": "string"},
                    "run_id": {"type": "string"},
                },
            },
            "event_id": {"type": "string"},
            "sequence": {"type": "integer"},
        },
        "required": ["target", "event_id", "sequence"],
    })
}
