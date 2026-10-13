// SPDX-License-Identifier: AGPL-3.0-only

//! MCP `ac_observe` (Workflow #389 MCP-5a; MCP-0 command table v1.1 §4.5, D9), the MCP side of
//! `actingctl observe --export`: the same instance selector, the same ObserveReadonly on the
//! shared (Cli, Cli) connection and the same answer (`crate::observe_export`). The frame is
//! exported as `ac_material` mode export writes a material, to the MCP export directory; that
//! directory is the one presentation difference from the CLI (§2.2). Before U-3 the tool is in
//! the observer tier (D2).

use super::observer::{self, Material, select_instance};
use super::tools::{Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess};
use crate::codes::ActingctlCode;
use crate::observe_export::ObservedFrame;
use serde_json::{Map, Value, json};

pub(super) fn observe(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance"])?;
    let selector = arguments.required_string("instance")?;
    let connected = context.runtime.connect()?;
    // One Status resolves the selector (command table §2.5).
    let status = connected
        .client
        .status()
        .map_err(|error| context.runtime.failure(&connected, &error))?;
    let alias = select_instance(status.instances(), selector)?
        .instance_alias()
        .to_owned();
    let flow = connected
        .client
        .observe_readonly(&alias)
        .map_err(|error| context.runtime.failure(&connected, &error))?;
    let frame = ObservedFrame::of(&flow).ok_or_else(|| {
        ToolError::new(
            "runtime",
            ActingctlCode::MaterialRefUnknown,
            "none of this observation's events carries the frame it reports",
        )
    })?;
    let material = Material {
        position: frame.position(),
        artifact: frame.observation.artifact().clone(),
        snapshot: frame.snapshot,
    };
    let export = observer::material_export(context, &connected, &material)?;
    Ok(ToolSuccess {
        result: frame.answer(export.result),
        warnings: export.warnings,
    })
}

pub(super) fn result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "width": {"type": "integer"},
            "height": {"type": "integer"},
            "verdict": {"type": "string"},
            "capture_backend": {"type": "string"},
            "frame": {
                "type": "object",
                "properties": {
                    "ref": {"type": "string"},
                    "media_type": {"type": "string"},
                    "size": {"type": "integer"},
                    "sha256": {"type": "string"},
                },
                "required": ["ref", "media_type", "size", "sha256"],
            },
            "export": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "sha256": {"type": "string"},
                    "size": {"type": "integer"},
                },
                "required": ["path", "sha256", "size"],
            },
        },
        "required": ["width", "height", "verdict", "capture_backend", "frame", "export"],
    })
}
