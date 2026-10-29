// SPDX-License-Identifier: AGPL-3.0-only

//! `ac_perf_preset` (Workflow #429 R-2, design `429-A-runtime.md` A.1 契约 2; register §1.220),
//! the MCP side of `actingctl perf-preset set|get` (`crate::perf_preset`). A general tool
//! (observer until U-3, always on), as `ac_config_set` is: applying a preset is one
//! `ConfigureSettings`, and every change is recorded. Both actions read the preset table and the
//! members in effect with RuntimeFactSnapshot and judge them with the shared function of
//! `actingcommand-runtime-config`; a `set` writes on its own (Cli, Cli) connection the provenance
//! way, as `ac_config_set` does, and what the Runtime refuses is passed on unchanged. The answers
//! are the CLI's, without its outcome.

use super::operator::{CLI, record_provenance};
use super::runtime::client_error;
use super::tools::{Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument};
use crate::codes::ArgumentProblem;
use crate::perf_preset::{self, Failure, GET, SET};
use serde_json::{Map, Value, json};

pub(super) const DESCRIPTION: &str = "Reads or applies a performance preset, as actingctl perf-preset get and perf-preset set do. A preset is a named set of four settings the Runtime defines (performance.presets): performance.pacing.mode, performance.pacing.max_concurrent_runs (how many scheduled tasks run at once; unwritten in a preset means no such limit of its own), performance.pacing.heavy_dispatch_limit.normal and scheduling.parallel.enabled. The shipped presets are standard (mode observe: nothing slowed down), adaptive (mode pace: slower while the computer is busy; the default of a new install) and light (pace, one task at a time, one heavy scene). action get answers {preset, members: [{key, value, source}]}: the current preset, the one whose every member equals the value in effect, else custom, and each member's value in effect (null for max_concurrent_runs when nothing sets it) and its source (default, explicit, instruction). action set with preset (a name of the table) changes all four keys in one change, all or none, as ac_config_set would, and answers {preset, settings}, each key as it is in effect afterwards; a member the preset leaves unwritten clears that key's online value, which returns it to the configuration file's (so a file that writes max_concurrent_runs keeps it, and the current preset stays custom). A configuration file whose mode is full may be left by any preset; full itself is never set by an instruction (config_value_invalid). A name the table does not have is arguments_invalid (preset, not_recognised) and nothing is written. Every change is recorded in the ledger and kept across restarts until the configuration file changes the same key.";

/// `{action, preset?}`.
pub(super) fn input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": {"enum": [GET, SET]},
            "preset": {
                "type": "string",
                "minLength": 1,
                "description": "The preset to apply (set only), a name of performance.presets.",
            },
        },
        "required": ["action"],
        "additionalProperties": false,
    })
}

/// The answer of either action.
pub(super) fn result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "preset": {"type": "string"},
            "members": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "key": {"type": "string"},
                        "value": {},
                        "source": {"type": "string"},
                    },
                    "required": ["key", "value", "source"],
                },
            },
            "settings": {"type": "array", "items": {"type": "object"}},
        },
        "required": ["preset"],
    })
}

/// `ac_perf_preset {action: get}` or `{action: set, preset}`.
pub(super) fn run(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["action", "preset"])?;
    let action = arguments.required_string("action")?;
    let preset = arguments.string("preset")?;
    match (action, preset) {
        (GET, None) => {
            let connected = context.runtime.connect()?;
            let answer = perf_preset::get(&connected.client).map_err(|failure| match failure {
                Failure::Runtime(error) => context.runtime.failure(&connected, &error),
                other => tool_error(&other),
            })?;
            Ok(ToolSuccess::new(answer))
        }
        (GET, Some(_)) => Err(invalid_argument("preset", ArgumentProblem::Conflicting)),
        (SET, None) => Err(invalid_argument("preset", ArgumentProblem::Required)),
        (SET, Some(name)) => {
            let connection = context.runtime.connect_fresh(CLI.0, CLI.1)?;
            let write = record_provenance(connection, "ac_perf_preset", None, CLI, None)?;
            let answer =
                perf_preset::set(&write.client, name).map_err(|failure| tool_error(&failure))?;
            Ok(ToolSuccess {
                result: answer,
                warnings: write.warnings,
            })
        }
        _ => Err(invalid_argument("action", ArgumentProblem::NotRecognised)),
    }
}

/// A preset failure as a tool error: a name the table does not have is `arguments_invalid`
/// (`preset`, `not_recognised`); a missing or unreadable fact `runtime`; a Runtime failure the
/// client's.
fn tool_error(failure: &Failure) -> ToolError {
    match failure {
        Failure::Runtime(error) => client_error(error),
        Failure::Unknown(_) => invalid_argument("preset", ArgumentProblem::NotRecognised),
        Failure::Missing | Failure::Unreadable => ToolError::outcome("runtime", failure.outcome()),
    }
}
