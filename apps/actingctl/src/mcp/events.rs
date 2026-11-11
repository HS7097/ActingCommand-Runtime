// SPDX-License-Identifier: AGPL-3.0-only

//! ac_events (Workflow #389 MCP-1; MCP-0 command table v1.1 §4.1): `actingctl events` over
//! MCP. The parameters, the read and the rows are the CLI's (`crate::events`); this module adds
//! only what §2.2 lets MCP differ in: at most 100 rows, a wait inside the call budget, a brief
//! page cut to the output budget, and a payload page over it exported whole (Workflow #434
//! LAB-5D: both by `events`' `Page::within_budget`, with this server's budget and its cursor
//! binding). An instance selector is resolved with one Status per Runtime connection (§2.5).

use super::observer::select_instance;
use super::runtime::Connected;
use super::tools::{self, ToolContext, ToolError, ToolOutcome};
use crate::events::{self, CursorBinding, EventsError};
use actingcommand_contract::{InstanceId, RuntimeEventQueryCursor};
use serde_json::{Map, Value, json};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Time kept back from the call budget for the read that ends a wait.
const WAIT_READ_RESERVE: Duration = Duration::from_secs(5);

pub(super) fn events(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let max_limit = u16::try_from(tools::MAX_LIST).unwrap_or(u16::MAX);
    let request = events::parse(arguments, max_limit).map_err(tool_error)?;
    let connected = context.runtime.connect()?;
    let instance_id = request
        .instance
        .as_deref()
        .map(|selector| resolve(context, &connected, selector))
        .transpose()?;
    let last_read = context
        .deadline
        .checked_sub(WAIT_READ_RESERVE)
        .unwrap_or(context.deadline);
    let binding = CursorBinding {
        session: context.session,
        generation: connected.generation,
    };
    let page = events::read(
        &connected.client,
        &request,
        instance_id,
        binding,
        (Instant::now() + request.wait).min(last_read),
        &|| context.cancelled.load(Ordering::SeqCst),
    )
    .map_err(|error| match error {
        EventsError::Runtime(error) => context.runtime.failure(&connected, &error),
        other => tool_error(other),
    })?;
    // Workflow #434 LAB-5D: the page's cut and its export are `events`'; this server passes its
    // budget and binds each cursor to its own connection.
    let bind = |cursor: &RuntimeEventQueryCursor| binding.encode(cursor);
    page.within_budget(
        request.detail,
        &tools::envelope,
        &bind,
        Some(&tools::answer_budget()),
    )
    .map(|answer| tools::fitted(answer, Vec::new()))
    .map_err(tool_error)
}

/// The instance_id of `selector`: from what this connection already resolved, or from one
/// Status, remembered for the connection's lifetime.
fn resolve(
    context: &ToolContext<'_>,
    connected: &Connected,
    selector: &str,
) -> Result<InstanceId, ToolError> {
    if let Some(instance_id) = context.runtime.resolved_instance(connected, selector) {
        return Ok(instance_id);
    }
    let status = connected
        .client
        .status()
        .map_err(|error| context.runtime.failure(connected, &error))?;
    let instance_id = select_instance(status.instances(), selector)?.instance_id();
    context
        .runtime
        .remember_instance(connected, selector, instance_id);
    Ok(instance_id)
}

/// A failure that is not the Runtime read's, as the tool answers it: an argument is
/// `arguments_invalid` with the CLI's values; every other failure keeps the CLI's outcome.
/// Workflow #411 B27: an event type argument that names no event type also names the value it
/// was given (`argument_value`).
fn tool_error(error: EventsError) -> ToolError {
    match error {
        EventsError::Argument {
            field,
            problem,
            value,
        } => match (problem.range(), value) {
            (Some((minimum, maximum)), _) => tools::out_of_range(&field, minimum, maximum),
            (None, Some(value)) => tools::invalid_argument(&field, problem.token())
                .with(|outcome| outcome.argument_value(value)),
            (None, None) => tools::invalid_argument(&field, problem.token()),
        },
        EventsError::Runtime(error) => super::runtime::client_error(&error),
        // Workflow #434 LAB-5D: a payload page the export could not take is the writer's
        // `material_export_failed` (or the encoder's), class runtime, as before. Workflow #434
        // CLI-EV: pages of a read of the newest records that do not fit together, class runtime.
        EventsError::CursorEncode(_) | EventsError::Export(_) | EventsError::Pagination => {
            ToolError::outcome("runtime", error.outcome())
        }
        EventsError::Selector { .. } | EventsError::Query(_) | EventsError::CursorInvalid => {
            ToolError::outcome("usage", error.outcome())
        }
    }
}

/// The tool's input schema: every parameter of `actingctl events`, by the same names.
pub(super) fn input_schema() -> Value {
    let id = |what: &str| json!({"type": "string", "minLength": 1, "description": what});
    // Ledger revision 3 (#378 A3): `success` sits between `debug` and `info`.
    let severity = json!({
        "type": "string",
        "enum": ["debug", "success", "info", "warning", "error", "fatal"],
    });
    json!({
        "type": "object",
        "properties": {
            "instance": {
                "type": "string",
                "minLength": 1,
                "description": "Instance alias, instance_id or ADB port.",
            },
            "view": {
                "type": "string",
                "enum": ["events", "observation", "changes", "errors", "health", "lab"],
                "description": "Ledger view; without it every event.",
            },
            "since_unix_ms": {
                "type": "integer",
                "minimum": 0,
                "description": "Only events at or after this Unix time in milliseconds.",
            },
            "until_unix_ms": {
                "type": "integer",
                "minimum": 0,
                "description": "Only events at or before this Unix time in milliseconds.",
            },
            "after_sequence": {
                "type": "integer",
                "minimum": 0,
                "description": "Only events after this ledger sequence (a page's last_sequence); valid across restarts. Not with cursor.",
            },
            "through_sequence": {
                "type": "integer",
                "minimum": 0,
                "description": "Only events up to this ledger sequence.",
            },
            "event_type": id("One event type, for example task.failed. Not with event_types."),
            // Workflow #389 MCP-2 (MCP-0 command table v1.1 §4.2).
            "event_types": {
                "type": "array",
                "maxItems": 32,
                "items": {"type": "string", "minLength": 1},
                "description": "Any of these event types, each once; not with event_type and never an excluded type.",
            },
            "exclude_event_types": {
                "type": "array",
                "maxItems": 32,
                "items": {"type": "string", "minLength": 1},
                "description": "Event types left out; never the event_type.",
            },
            "fact_key": id("Only the fact records that name this instance or runtime fact key: fact.published, fact.invalidated, runtime.fact_recorded and runtime.fact_invalidated."),
            "min_severity": severity.clone(),
            "max_severity": severity,
            "origin_module": id("The module that recorded the event, for example policy."),
            "task_id": id("A Runtime task_id."),
            "run_id": id("A Runtime run_id."),
            "request_id": id("A Runtime request_id."),
            "correlation_id": id("A Runtime correlation_id."),
            "detail": {
                "type": "string",
                "enum": ["brief", "summary", "payload"],
                "default": "brief",
                "description": "summary adds the outcome a record carries (none on a state change); payload adds each event's projected payload.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": tools::MAX_LIST,
                "default": tools::DEFAULT_LIST,
            },
            "cursor": {
                "type": "string",
                "minLength": 1,
                "description": "next_cursor of the previous page, sent with the same filters and without after_sequence.",
            },
            "wait_s": {
                "type": "integer",
                "minimum": 0,
                "maximum": events::MAX_WAIT_S,
                "default": 0,
                "description": "Seconds to wait for rows when the page is empty.",
            },
        },
        "additionalProperties": false,
    })
}

/// A page inline, or a payload page exported whole.
pub(super) fn result_schema() -> Value {
    let mut row = json!({
        "type": "object",
        "properties": {
            "seq": {"type": "integer"},
            "ts": {"type": "integer"},
            "type": {"type": "string"},
            "severity": {"type": "string"},
            "instance": {"type": "string"},
            "request_id": {"type": "string"},
            "run_id": {"type": "string"},
            "task_id": {"type": "string"},
            "correlation_id": {"type": "string"},
            "materials": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "ref": {"type": "string"},
                        "media_type": {"type": "string"},
                        "size": {"type": "integer"},
                    },
                    "required": ["ref", "media_type", "size"],
                },
            },
            "materials_truncated": {"type": "boolean"},
            "payload": {"type": "object"},
        },
        "required": ["seq", "ts", "type", "severity"],
    });
    // The ledger's diagnostic code, a member of the row like its type (Workflow #378 A5: a row
    // member, inserted outside `json!`).
    row["properties"]["code"] = json!({"type": "string"});
    // detail summary: the record's outcome envelope (#389 MCP-1 follow-up).
    row["properties"]["outcome"] = json!({"type": "object"});
    json!({
        "anyOf": [
            {
                "type": "object",
                "properties": {
                    "events": {"type": "array", "items": row},
                    "next_cursor": {"type": "string"},
                    "truncated": {"type": "boolean"},
                    "snapshot_ledger_position": {"type": "integer"},
                    // Workflow #434 CLI-EV: a page of the newest records.
                    "first_sequence": {"type": "integer"},
                    "last_sequence": {"type": "integer"},
                    "searched_from_sequence": {"type": "integer"},
                    "waited_ms": {"type": "integer"},
                },
                "required": ["events", "snapshot_ledger_position"],
            },
            {
                "type": "object",
                "properties": {
                    "export": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string"},
                            "sha256": {"type": "string"},
                            "size": {"type": "integer"},
                        },
                        "required": ["path", "sha256", "size"],
                    },
                    "events_count": {"type": "integer"},
                    "next_cursor": {"type": "string"},
                    "first_sequence": {"type": "integer"},
                    "last_sequence": {"type": "integer"},
                    "searched_from_sequence": {"type": "integer"},
                },
                "required": ["export", "events_count", "last_sequence"],
            },
        ],
    })
}
