// SPDX-License-Identifier: AGPL-3.0-only

//! ac_runs (Workflow #389 MCP-3; MCP-0 command table v1.1 §4.3): `actingctl runs` over MCP.
//! Its arguments are the CLI's flags (R2) and its result is the CLI's stdout JSON
//! (`crate::run_list`), cut from its last row to the output budget: `truncated`, with
//! next_cursor after the last row kept (§2.2).

use super::runtime::{Connected, client_error};
use super::tools::{
    self, Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument,
};
use crate::run_list::{self, Failure, RunsRequest, StateFilter};
use serde_json::{Map, Value, json};
use std::time::Duration;

/// Time kept back from the call budget to render and answer.
const READ_RESERVE: Duration = Duration::from_secs(1);
const ARGUMENTS: [&str; 8] = [
    "instance",
    "since_unix_ms",
    "until_unix_ms",
    "task_prefix",
    "dispatch",
    "state",
    "limit",
    "cursor",
];

pub(super) fn runs(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &ARGUMENTS)?;
    let request = RunsRequest {
        instance: arguments.string("instance")?.map(str::to_owned),
        since_unix_ms: arguments.integer("since_unix_ms", 0, u64::MAX)?,
        until_unix_ms: arguments.integer("until_unix_ms", 0, u64::MAX)?,
        task_prefix: arguments.string("task_prefix")?.map(str::to_owned),
        dispatch: arguments
            .string("dispatch")?
            .map(|text| {
                run_list::parse_dispatch(text)
                    .ok_or_else(|| invalid_argument("dispatch", "must be manual or scheduled"))
            })
            .transpose()?,
        state: arguments
            .string("state")?
            .map(|text| {
                StateFilter::parse(text).ok_or_else(|| {
                    invalid_argument(
                        "state",
                        "must be open, succeeded, failed, cancelled or interrupted_unterminated",
                    )
                })
            })
            .transpose()?,
        limit: arguments
            .integer("limit", 1, run_list::MAX_LIMIT)?
            .unwrap_or(run_list::DEFAULT_LIMIT),
        cursor: arguments.string("cursor")?.map(str::to_owned),
    };
    let now_unix_ms =
        run_list::now_unix_ms().map_err(|failure| tool_error(context, None, failure))?;
    let connected = context.runtime.connect()?;
    let deadline = context
        .deadline
        .checked_sub(READ_RESERVE)
        .unwrap_or(context.deadline);
    let answer = run_list::read(&connected.client, &request, now_unix_ms, deadline)
        .map_err(|failure| tool_error(context, Some(&connected), failure))?;
    let warnings = answer.warnings();
    let mut kept = answer.row_count();
    loop {
        let result = answer.render(kept);
        // A result that does not fit even without rows answers output_budget_exceeded.
        if kept == 0 || tools::fits_success(&result) {
            return Ok(ToolSuccess { result, warnings });
        }
        kept -= 1;
    }
}

/// The CLI's failure as a tool error: an argument is `arguments_invalid` here.
fn tool_error(
    context: &ToolContext<'_>,
    connected: Option<&Connected>,
    failure: Failure,
) -> ToolError {
    match &failure {
        Failure::Argument { field, detail } => invalid_argument(field, detail),
        Failure::SinceOutOfRange { oldest_unix_ms } => {
            ToolError::usage(failure.code(), failure.message())
                .with_detail("oldest_unix_ms", json!(oldest_unix_ms))
        }
        Failure::Usage | Failure::CursorInvalid | Failure::Selector { .. } => {
            ToolError::usage(failure.code(), failure.message())
        }
        Failure::Clock | Failure::Output => {
            ToolError::new("runtime", failure.code(), failure.message())
        }
        Failure::Runtime(error) => match connected {
            Some(connected) => context.runtime.failure(connected, error),
            None => client_error(error),
        },
    }
}

fn instance_argument() -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "description": "Instance alias, instance_id or ADB port; without it every instance.",
    })
}

pub(super) fn input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "instance": instance_argument(),
            "since_unix_ms": {
                "type": "integer",
                "minimum": 0,
                "description": "Window start: runs first recorded at or after it; default 24 h ago, at most 7 days ago.",
            },
            "until_unix_ms": {
                "type": "integer",
                "minimum": 0,
                "description": "Window end, not before since_unix_ms; without it the window is open.",
            },
            "task_prefix": {
                "type": "string",
                "minLength": 1,
                "description": "Only scheduled runs whose catalog_task_id (policy task id) starts with it; a manual run has none and never matches.",
            },
            "dispatch": {"type": "string", "enum": ["manual", "scheduled"]},
            "state": {
                "type": "string",
                "enum": ["open", "succeeded", "failed", "cancelled", "interrupted_unterminated"],
                "description": "open is admitted or running.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": run_list::MAX_LIMIT,
                "default": run_list::DEFAULT_LIMIT,
            },
            "cursor": {
                "type": "string",
                "minLength": 1,
                "description": "next_cursor of the previous page, sent with the same filters.",
            },
        },
        "additionalProperties": false,
    })
}

pub(super) fn result() -> Value {
    let mut result = json!({
        "type": "object",
        "properties": {
            "runs": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "request_id": {"type": "string"},
                        "run_id": {"type": "string"},
                        "task_id": {"type": "string"},
                        "catalog_task_id": {"type": "string"},
                        "alias": {"type": "string"},
                        "instance_id": {"type": "string"},
                        "dispatch": {"type": "string", "enum": ["manual", "scheduled", "unknown"]},
                        "origin": {
                            "type": "string",
                            "enum": ["cli", "ui", "lab", "scheduler", "unknown"],
                        },
                        "state": {
                            "type": "string",
                            "enum": [
                                "admitted",
                                "running",
                                "succeeded",
                                "failed",
                                "cancelled",
                                "interrupted_unterminated",
                            ],
                        },
                        "admitted_at_unix_ms": {"type": "integer"},
                        "ended_at_unix_ms": {"type": "integer"},
                        "duration_ms": {"type": "integer"},
                        "task_result": {"type": "string", "enum": ["success", "failure", "cancelled"]},
                        "final_page": {"type": "string"},
                        "executed_steps": {"type": "integer"},
                    },
                    "required": [
                        "request_id",
                        "instance_id",
                        "dispatch",
                        "origin",
                        "state",
                        "admitted_at_unix_ms",
                    ],
                },
            },
            "window": {
                "type": "object",
                "properties": {
                    "since_unix_ms": {"type": "integer"},
                    "until_unix_ms": {"type": "integer"},
                },
                "required": ["since_unix_ms"],
            },
            "next_cursor": {"type": "string"},
            "truncated": {"type": "boolean"},
            "incomplete": {"type": "boolean"},
            "snapshot_ledger_position": {"type": "integer"},
        },
        "required": ["runs", "window", "snapshot_ledger_position"],
    });
    // The terminal's failure code, a member of the row like final_page.
    result["properties"]["runs"]["items"]["properties"]["failure_code"] = json!({"type": "string"});
    result
}
