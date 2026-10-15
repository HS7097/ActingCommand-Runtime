// SPDX-License-Identifier: AGPL-3.0-only

//! ac_devices, ac_device_bind and ac_device_unbind (Workflow #398 RD-1c; model-398 v1.1 §3.3):
//! `actingctl devices list|bind|unbind` over MCP, the same answers (`crate::devices`). The
//! server's location stands for the CLI's `--root` and `--state-root`; a binding written here
//! names the channel `mcp`. Observer tools before U-3, as D1b's configuration tools are.

use super::tools::{Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument};
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::devices::{self, Bind};
use actingcommand_contract::DeviceBoundVia;
use actingcommand_contract::outcome::Outcome;
use serde_json::{Map, Value, json};
use std::path::PathBuf;

pub(super) const LIST_DESCRIPTION: &str = "The devices adb lists now, as actingctl devices list answers them: the install's adb (tools\\platform-tools\\adb.exe) runs adb devices -l once; no Runtime request and no ledger record. One row per device: handle (<transport>:<transport_id>, the adb server's session number; give it to ac_device_bind), transport (usb or tcp), state (device, unauthorized, offline, ...), model, bound_alias (the handset instance this state root's private binding file binds it to) and, for a loopback tcp row (an emulator), endpoint. No row names a device serial. adb that does not run answers device_list_failed; a private binding file that breaks its rules device_bindings_invalid.";
pub(super) const BIND_DESCRIPTION: &str = "Binds the device of handle (from ac_devices) to the handset instance alias, as actingctl devices bind does: the alias must be a handset (device.kind handset) of the install's selected configuration (otherwise instance_unknown or device_kind_unsupported), adb must list the handle now (otherwise device_handle_unknown), and one device is bound to one alias (device_bindings_invalid). The binding goes into the state root's private binding file with the channel mcp; it never leaves that file, and the answer {alias, handle, model, restart_required: true} names no serial. It takes effect when actingd starts again; restarting is the person's.";
pub(super) const UNBIND_DESCRIPTION: &str = "Removes the binding of the handset instance alias from the state root's private binding file, as actingctl devices unbind does: {alias, unbound, restart_required}, both false when the file had no binding for it. It takes effect when actingd starts again; restarting is the person's.";

/// The state root this server reads, or why it has none (class usage).
fn state_root(context: &ToolContext<'_>) -> Result<PathBuf, ToolError> {
    context
        .runtime
        .locate()
        .state_root
        .map_err(|outcome| ToolError::outcome("usage", outcome))
}

/// A refusal with the CLI's outcome: a request the configuration, adb's list or the binding
/// file's rules refuse is class usage; adb that does not run, a file that cannot be written and
/// a configuration that cannot be read are class runtime.
fn refused(outcome: Outcome) -> ToolError {
    let usage = [
        ActingctlCode::DeviceHandleUnknown,
        ActingctlCode::DeviceKindUnsupported,
        ActingctlCode::DeviceBindingsInvalid,
    ]
    .into_iter()
    .any(|code| outcome.is(code))
        || outcome.is(actingcommand_contract::RuntimeErrorCode::InstanceUnknown);
    ToolError::outcome(if usage { "usage" } else { "runtime" }, outcome)
}

pub(super) fn list(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    Arguments::new(arguments, &[])?;
    let state_root = state_root(context)?;
    devices::list(
        &state_root,
        context.runtime.requested_root(),
        &devices::run_adb,
    )
    .map(ToolSuccess::new)
    .map_err(refused)
}

pub(super) fn bind(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["alias", "handle"])?;
    let alias = arguments.required_string("alias")?;
    let handle = arguments.required_string("handle")?;
    let state_root = state_root(context)?;
    devices::bind(
        &state_root,
        context.runtime.requested_root(),
        &Bind {
            alias,
            handle,
            via: DeviceBoundVia::Mcp,
        },
        &devices::run_adb,
    )
    .map(ToolSuccess::new)
    .map_err(refused)
}

pub(super) fn unbind(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["alias"])?;
    let alias = arguments.required_string("alias")?;
    if alias.trim().is_empty() {
        return Err(invalid_argument("alias", ArgumentProblem::ValueMissing));
    }
    let state_root = state_root(context)?;
    devices::unbind(&state_root, alias)
        .map(ToolSuccess::new)
        .map_err(refused)
}

pub(super) fn list_input() -> Value {
    json!({"type": "object", "properties": {}, "additionalProperties": false})
}

pub(super) fn bind_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "alias": {
                "type": "string",
                "minLength": 1,
                "description": "The handset instance's alias in the selected configuration.",
            },
            "handle": {
                "type": "string",
                "minLength": 1,
                "description": "A device's handle from ac_devices, for example usb:7.",
            },
        },
        "required": ["alias", "handle"],
        "additionalProperties": false,
    })
}

pub(super) fn unbind_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "alias": {
                "type": "string",
                "minLength": 1,
                "description": "The handset instance's alias.",
            },
        },
        "required": ["alias"],
        "additionalProperties": false,
    })
}

pub(super) fn list_result() -> Value {
    let text = json!({"type": "string"});
    json!({
        "type": "object",
        "properties": {
            "devices": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "handle": text,
                        "transport": {"type": "string", "enum": ["usb", "tcp"]},
                        "state": text,
                        "model": text,
                        "bound_alias": text,
                        "endpoint": text,
                    },
                    "required": ["transport", "state"],
                },
            },
        },
        "required": ["devices"],
    })
}

pub(super) fn bind_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "alias": {"type": "string"},
            "handle": {"type": "string"},
            "model": {"type": "string"},
            "restart_required": {"type": "boolean"},
        },
        "required": ["alias", "handle", "restart_required"],
    })
}

pub(super) fn unbind_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "alias": {"type": "string"},
            "unbound": {"type": "boolean"},
            "restart_required": {"type": "boolean"},
        },
        "required": ["alias", "unbound", "restart_required"],
    })
}
