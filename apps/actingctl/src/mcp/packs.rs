// SPDX-License-Identifier: AGPL-3.0-only

//! `ac_packs` (Workflow #389 MCP-11, command table v1.1 §4.12): `actingctl packs` over MCP.
//! The answer comes from the same reader (`crate::packs`), so its `result` is the CLI's
//! document without the top-level `warnings`, which travel in the envelope (§2.3). The server's
//! own install location stands for the CLI's `--root`, and an answer over the output budget is
//! exported whole (§2.2, both presentation differences).

use super::observer;
use super::tools::{
    self, Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument,
};
use crate::codes::ActingctlCode;
use crate::packs::{self as reader, Answer, PacksError, Query, Role};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub(super) const DESCRIPTION: &str = "The packages the selected configuration generation binds, as actingctl packs answers them: read from that actingd.config.json on disk (source.kind selected_config: the generation install\\active.json selects, which a running actingd may not have loaded yet); no Runtime request and no ledger record. instances: one row per configured instance {alias, instance_id, packs}, with role startup (startup_package; package_id from the package folder's control.json) and resource_package (path only). installation: role procedure (policy.procedure_manifest: package_id from procedure_ref package.<id>, procedure_ref, operation_id, scheduled when it has scheduled_execution, path its package_path), prerequisite (prerequisite_packages) and return_home (return_home_packages, with the path of the prerequisite package it names). Rows carry no package digest. Paths are absolute: a relative one resolves against the configuration's folder. readable (present with path) says whether the path can be opened; each unreadable one adds the warning package_binding_unreadable. instance (alias, instance_id or ADB port, as the configuration names them) narrows instances only; role narrows both lists. An answer larger than the output budget is written whole, its warnings included, to %TEMP%\\actingcommand-mcp\\materials\\<sha256>.json and answered as {source, export {path, sha256, size}} with the warning answer_exported; narrow it with role or instance. A configuration that cannot be read answers config_read_failed, one that does not decode config_decode_failed.";

pub(super) fn run(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance", "role"])?;
    let instance = arguments.string("instance")?;
    let role = arguments
        .string("role")?
        .map(|text| Role::parse(text).ok_or_else(|| invalid_argument("role", "is not a role")))
        .transpose()?;
    let location = context.runtime.locate();
    location.check()?;
    let config = location.config.as_deref().ok_or_else(|| {
        ToolError::usage(
            ActingctlCode::InstallRootUnresolved,
            "this server does not run from an install root and was started without --root",
        )
        .blocked_by("mcp-serve --root <install root>")
    })?;
    let answer = reader::read(
        location.installation.as_ref(),
        config,
        &Query { instance, role },
    )
    .map_err(refused)?;
    within_budget(&answer)
}

/// The answer as it is when it fits the output budget; otherwise the whole document (the CLI's
/// stdout, warnings included) exported, answered as `{source, export}` with `answer_exported`.
fn within_budget(answer: &Answer) -> ToolOutcome {
    let result = encode(answer.packs())?;
    let warnings = answer
        .warnings()
        .iter()
        .map(encode)
        .collect::<Result<Vec<_>, _>>()?;
    // The envelope is `{ok, result, warnings}`; this wrapper is a little larger, so an answer
    // it lets through always fits.
    if tools::fits_success(&json!({"result": result, "warnings": warnings})) {
        return Ok(ToolSuccess { result, warnings });
    }
    let bytes = serde_json::to_vec(&answer.document()).map_err(encode_failed)?;
    let export = observer::export_bytes(
        &format!("sha256:{:x}", Sha256::digest(&bytes)),
        "json",
        &bytes,
    )?;
    let note = ToolError::new(
        "runtime",
        ActingctlCode::AnswerExported,
        "the answer is larger than the output budget and is whole, its warnings included, in the exported file; narrow it with role or instance",
    )
    .with_detail("export", export.clone())
    .with_detail("warnings_exported", json!(warnings.len()))
    .into_warning();
    Ok(ToolSuccess {
        result: json!({"source": encode(answer.packs().source())?, "export": export}),
        warnings: vec![note],
    })
}

fn encode<T: Serialize>(value: &T) -> Result<Value, ToolError> {
    serde_json::to_value(value).map_err(encode_failed)
}

fn encode_failed(error: serde_json::Error) -> ToolError {
    ToolError::new(
        "runtime",
        ActingctlCode::ActingctlOutputFailed,
        format!("cannot encode the answer: {error}"),
    )
}

/// A refusal of the reader with the same code as the CLI's; the configuration's path and the
/// install selection's own code, when there are, go into the details.
fn refused(error: PacksError) -> ToolError {
    let mut tool_error = ToolError::usage(error.code(), error.message());
    if let Some(path) = error.path() {
        tool_error = tool_error.with_detail("path", json!(path));
    }
    if let Some(cause) = error.cause() {
        tool_error = tool_error.with_detail("caused_by", json!(cause.as_str()));
    }
    tool_error
}

pub(super) fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "instance": {
                "type": "string",
                "minLength": 1,
                "description": "Instance alias, instance_id or ADB port, as the configuration names them; without it every configured instance.",
            },
            "role": {
                "type": "string",
                "enum": Role::ALL,
                "description": "Only the rows of this role.",
            },
        },
        "additionalProperties": false,
    })
}

pub(super) fn result_schema() -> Value {
    let text = json!({"type": "string"});
    let pack = json!({
        "type": "object",
        "properties": {
            "role": text,
            "package_id": text,
            "path": text,
            "readable": {"type": "boolean"},
        },
        "required": ["role", "path", "readable"],
    });
    json!({
        "type": "object",
        "properties": {
            "source": {
                "type": "object",
                "properties": {"kind": text, "config": text},
                "required": ["kind", "config"],
            },
            "instances": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "alias": text,
                        "instance_id": text,
                        "packs": {"type": "array", "items": pack},
                    },
                    "required": ["alias", "instance_id", "packs"],
                },
            },
            "installation": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "role": text,
                        "package_id": text,
                        "path": text,
                        "readable": {"type": "boolean"},
                        "procedure_ref": text,
                        "operation_id": text,
                        "scheduled": {"type": "boolean"},
                    },
                    "required": ["role", "scheduled"],
                },
            },
            "export": {
                "type": "object",
                "properties": {"path": text, "sha256": text, "size": {"type": "integer"}},
                "required": ["path", "sha256", "size"],
            },
        },
        "required": ["source"],
    })
}

// Test-only: Workflow #389 MCP-11, the output budget of `ac_packs` (command table §2.2).
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use tempfile::TempDir;

    /// An installation's routine bindings: about as many as a real installation holds.
    const BINDINGS: usize = 200;

    #[test]
    fn an_answer_over_the_output_budget_is_exported_whole_with_its_source() {
        let root = TempDir::new().expect("tempdir");
        let config = root.path().join("actingd.config.json");
        let bindings = (0..BINDINGS)
            .map(|index| {
                json!({
                    "procedure_ref": format!("package.neutral.task{index:03}"),
                    "operation_id": format!("neutral.task{index:03}"),
                    "yield_points": [],
                })
            })
            .collect::<Vec<_>>();
        let document = json!({"instances": [], "policy": {"procedure_manifest": bindings}});
        fs::write(&config, document.to_string()).expect("write configuration");
        let answer = reader::read(None, &config, &Query::default())
            .unwrap_or_else(|error| panic!("packs refused the configuration: {error}"));
        let whole = serde_json::to_vec(&answer.document()).expect("encode the document");

        let success = within_budget(&answer)
            .unwrap_or_else(|error| panic!("ac_packs refused: {}", error.message()));

        let members = success
            .result
            .as_object()
            .map(|result| result.keys().map(String::as_str).collect::<BTreeSet<_>>())
            .unwrap_or_default();
        assert_eq!(
            members,
            BTreeSet::from(["export", "source"]),
            "exported answer: {}",
            success.result
        );
        assert_eq!(
            success.result["source"],
            serde_json::to_value(answer.packs().source()).expect("encode the source")
        );
        let export = &success.result["export"];
        let path = export["path"]
            .as_str()
            .unwrap_or_else(|| panic!("an export without a path: {export}"));
        assert_eq!(fs::read(path).expect("read the export"), whole);
        assert_eq!(export["size"], json!(whole.len()));
        assert_eq!(
            export["sha256"],
            json!(format!("sha256:{:x}", Sha256::digest(&whole)))
        );
        let codes = success
            .warnings
            .iter()
            .map(|warning| warning["code"].clone())
            .collect::<Vec<_>>();
        assert_eq!(codes, vec![json!(ActingctlCode::AnswerExported.as_str())]);
    }
}
