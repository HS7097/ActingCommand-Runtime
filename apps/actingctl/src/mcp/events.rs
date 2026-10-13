// SPDX-License-Identifier: AGPL-3.0-only

//! ac_events (Workflow #389 MCP-1; MCP-0 command table v1.1 §4.1): `actingctl events` over
//! MCP. The parameters, the read and the rows are the CLI's (`crate::events`); this module adds
//! only what §2.2 lets MCP differ in: at most 100 rows, a wait inside the call budget, a brief
//! page cut to the output budget, and a payload page over it exported whole. An instance
//! selector is resolved with one Status per Runtime connection (§2.5).

use super::observer::{self, select_instance};
use super::runtime::Connected;
use super::tools::{self, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument};
use crate::codes::ActingctlCode;
use crate::events::{self, CursorBinding, Detail, EventsError, Page};
use actingcommand_contract::{ArtifactMaterialAccumulator, InstanceId};
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
    match request.detail {
        Detail::Brief => brief_page(&page, binding),
        Detail::Payload => payload_page(&page, binding),
    }
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

/// The page cut to the output budget: rows from the end first, then the one row's material
/// list; `truncated` and a cursor after the last row kept.
fn brief_page(page: &Page, binding: CursorBinding) -> ToolOutcome {
    let mut rows = page
        .events()
        .iter()
        .map(events::brief_row)
        .collect::<Vec<_>>();
    let mut kept = rows.len();
    loop {
        let truncated = kept < rows.len();
        let next = if truncated {
            Some(page.cursor_after(page.events()[kept - 1].sequence))
        } else {
            page.next_cursor().transpose()
        };
        let next = next
            .transpose()
            .and_then(|next| next.map(|cursor| binding.encode(&cursor)).transpose())
            .map_err(tool_error)?;
        let result = page.result(&rows[..kept], next, truncated);
        if tools::fits_success(&result) {
            return Ok(ToolSuccess::new(result));
        }
        if kept > 1 {
            kept -= 1;
            continue;
        }
        // One row alone is over the budget: shorten its material list, and say so, rather
        // than drop the row.
        let Some(materials) = rows
            .first_mut()
            .and_then(|row| row.get_mut("materials"))
            .and_then(Value::as_array_mut)
            .filter(|materials| !materials.is_empty())
        else {
            // Nothing left to shorten: the result answers output_budget_exceeded.
            return Ok(ToolSuccess::new(result));
        };
        materials.pop();
        rows[0]["materials_truncated"] = json!(true);
    }
}

/// The page whole; over the output budget, the page the CLI prints is written whole to the
/// export directory and answered as `{export, events_count, next_cursor?, last_sequence}` with
/// the warning `answer_exported`.
fn payload_page(page: &Page, binding: CursorBinding) -> ToolOutcome {
    let rows = page
        .events()
        .iter()
        .map(|event| events::row(event, Detail::Payload))
        .collect::<Vec<_>>();
    let next = page
        .next_cursor()
        .and_then(|next| next.map(|cursor| binding.encode(&cursor)).transpose())
        .map_err(tool_error)?;
    let result = page.result(&rows, next.clone(), false);
    if tools::fits_success(&result) {
        return Ok(ToolSuccess::new(result));
    }
    let export = export_json(&result)?;
    let mut answer = Map::new();
    answer.insert("export".to_owned(), export.clone());
    answer.insert("events_count".to_owned(), json!(rows.len()));
    if let Some(next) = next {
        answer.insert("next_cursor".to_owned(), json!(next));
    }
    if let Some(last) = result.get("last_sequence") {
        answer.insert("last_sequence".to_owned(), last.clone());
    }
    let path = export["path"].as_str().unwrap_or_default();
    let warning = ToolError::new(
        "runtime",
        ActingctlCode::AnswerExported,
        format!(
            "the page with its payloads is larger than the output budget and is whole in the exported file {path}; read it there, or ask for fewer rows with limit"
        ),
    )
    .with_detail("export", export.clone())
    .into_warning();
    Ok(ToolSuccess {
        result: Value::Object(answer),
        warnings: vec![warning],
    })
}

/// The page as JSON, written by the export writer as `<sha256>.json`.
fn export_json(page: &Value) -> Result<Value, ToolError> {
    let failed =
        |reason: String| ToolError::new("runtime", ActingctlCode::MaterialExportFailed, reason);
    let bytes = serde_json::to_vec(page)
        .map_err(|error| failed(format!("cannot encode the page: {error}")))?;
    let mut material = ArtifactMaterialAccumulator::default();
    material
        .update(&bytes)
        .map_err(|error| failed(format!("cannot hash the page: {error}")))?;
    observer::export_bytes(material.finish().sha256(), "json", &bytes)
}

/// A failure that is not the Runtime read's, as the tool answers it.
fn tool_error(error: EventsError) -> ToolError {
    let message = error.message();
    match error {
        EventsError::Argument { field, problem } => invalid_argument(&field, &problem.to_string()),
        EventsError::Selector { miss, .. } => ToolError::usage(miss.code(), message),
        EventsError::Query(error) => {
            ToolError::usage(error.code(), message).with_detail("field", json!(error.field()))
        }
        EventsError::CursorInvalid => ToolError::usage(ActingctlCode::CursorInvalid, message),
        EventsError::CursorEncode(_) => {
            ToolError::new("runtime", ActingctlCode::CursorEncodeFailed, message)
        }
        EventsError::Runtime(error) => super::runtime::client_error(&error),
    }
}

/// The tool's input schema: every parameter of `actingctl events`, by the same names.
pub(super) fn input_schema() -> Value {
    let id = |what: &str| json!({"type": "string", "minLength": 1, "description": what});
    let severity = json!({
        "type": "string",
        "enum": ["debug", "info", "warning", "error", "fatal"],
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
            "event_type": id("One event type, for example task.failed."),
            "exclude_event_types": {
                "type": "array",
                "maxItems": 32,
                "items": {"type": "string", "minLength": 1},
                "description": "Event types left out; never the event_type.",
            },
            "min_severity": severity.clone(),
            "max_severity": severity,
            "origin_module": id("The module that recorded the event, for example policy."),
            "task_id": id("A Runtime task_id."),
            "run_id": id("A Runtime run_id."),
            "request_id": id("A Runtime request_id."),
            "correlation_id": id("A Runtime correlation_id."),
            "detail": {
                "type": "string",
                "enum": ["brief", "payload"],
                "default": "brief",
                "description": "payload adds each event's projected payload.",
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
    let row = json!({
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
            "code": {"type": "string"},
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
    json!({
        "anyOf": [
            {
                "type": "object",
                "properties": {
                    "events": {"type": "array", "items": row},
                    "next_cursor": {"type": "string"},
                    "truncated": {"type": "boolean"},
                    "snapshot_ledger_position": {"type": "integer"},
                    "last_sequence": {"type": "integer"},
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
                    "last_sequence": {"type": "integer"},
                },
                "required": ["export", "events_count", "last_sequence"],
            },
        ],
    })
}
