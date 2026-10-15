// SPDX-License-Identifier: AGPL-3.0-only

//! ac_capability_gap (Workflow #410 item 6; board #410, Alice 2026-10-15): `actingctl
//! capability-gap` over MCP, the same check and the same answer (`crate::capability_gap`), the
//! line recorded with `via` `mcp`. An observer tool: it is there in every session, whatever
//! tiers the server was started with, because a gap shows wherever an agent works; it touches
//! neither the Runtime nor a device, and writes only the user's own local log.

use super::tools::{Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument};
use crate::capability_gap::{
    self, CAPABILITY, MAX_CAPABILITY_CHARS, MAX_WHY_CHARS, WHY, text_problem,
};
use crate::codes::ArgumentProblem;
use serde_json::{Map, Value, json};

pub(super) const DESCRIPTION: &str = "Records a capability you needed and no ActingCommand tool offered, as actingctl capability-gap does: capability, a short name of what was missing (at most 120 characters), and why, one or two sentences on what you were doing and why no tool did it (at most 500 characters); both single lines. The record is appended as one JSON line {schema_version, recorded_at_unix_ms, via: mcp, capability, why} to this user's local log %LOCALAPPDATA%\\ActingCommand\\actingctl\\capability-gaps.jsonl and is never sent anywhere; it is not a Runtime record and needs no running actingd. Answers {path, gap}: the log and the line written. Call it once per gap when you hit one, then carry on with what the tools do offer; a person reads the log to decide what to build next. A log that cannot be written answers capability_gap_write_failed (class runtime).";

pub(super) fn run(_context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &[CAPABILITY, WHY])?;
    let capability = arguments.required_string(CAPABILITY)?;
    let why = arguments.required_string(WHY)?;
    for (name, text, max) in [
        (CAPABILITY, capability, MAX_CAPABILITY_CHARS),
        (WHY, why, MAX_WHY_CHARS),
    ] {
        match text_problem(text, max) {
            None => {}
            Some(ArgumentProblem::OutOfRange) => {
                return Err(super::tools::out_of_range(name, 1, max as u64));
            }
            Some(problem) => return Err(invalid_argument(name, problem)),
        }
    }
    capability_gap::log_path()
        .and_then(|log| capability_gap::record(&log, capability, why, "mcp"))
        .map(ToolSuccess::new)
        .map_err(|outcome| ToolError::outcome("runtime", outcome))
}

pub(super) fn input() -> Value {
    json!({
        "type": "object",
        "properties": {
            CAPABILITY: {
                "type": "string",
                "minLength": 1,
                "maxLength": MAX_CAPABILITY_CHARS,
                "description": "A short name of the missing capability, one line, at most 120 characters.",
            },
            WHY: {
                "type": "string",
                "minLength": 1,
                "maxLength": MAX_WHY_CHARS,
                "description": "What you were doing and why no tool did it, one line, at most 500 characters.",
            },
        },
        "required": [CAPABILITY, WHY],
        "additionalProperties": false,
    })
}

pub(super) fn result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "gap": {
                "type": "object",
                "properties": {
                    "schema_version": {"type": "string"},
                    "recorded_at_unix_ms": {"type": "integer"},
                    "via": {"type": "string"},
                    CAPABILITY: {"type": "string"},
                    WHY: {"type": "string"},
                },
            },
        },
        "required": ["path", "gap"],
    })
}
