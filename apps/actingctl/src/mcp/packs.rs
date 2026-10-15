// SPDX-License-Identifier: AGPL-3.0-only

//! `ac_packs` (Workflow #389 MCP-11, command table v1.1 §4.12): `actingctl packs` over MCP.
//! The answer comes from the same reader (`crate::packs`), so its `result` is the CLI's
//! document without the top-level `warnings`, which travel in the envelope (§2.3). The server's
//! own install location stands for the CLI's `--root`, and an answer over the output budget is
//! exported whole (§2.2, both presentation differences).
//!
//! Unlike the other tools, whose installation inputs are pinned when the server starts (#352),
//! `ac_packs` finds the selected generation again at every call, as a fresh `actingctl packs`
//! does (train 4 review ruling): after a deploy changes `install\active.json`, an open session
//! answers the new generation (an installed server too: `selected_config` reads its root's
//! selection again), and a selection that cannot be read answers the CLI's codes.

use super::observer;
use super::tools::{
    self, Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument,
};
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::packs::{self as reader, Answer, PacksError, Query, Role};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{Outcome, RawSource};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub(super) const DESCRIPTION: &str = "The packages the selected configuration generation binds, as actingctl packs answers them: read from that actingd.config.json on disk (source.kind selected_config: the generation install\\active.json selects, which a running actingd may not have loaded yet); no Runtime request and no ledger record. instances: one row per configured instance {alias, instance_id, packs}, with role startup (startup_package; package_id from the package folder's control.json) and resource_package (path only). installation: role procedure (policy.procedure_manifest: package_id from procedure_ref package.<id>, procedure_ref, operation_id, scheduled when it has scheduled_execution, path its package_path); prerequisite and return-home packages are the installed standard packs' own, no configuration binding. Rows carry no package digest. Paths are absolute: a relative one resolves against the configuration's folder. readable (present with path) says whether the path can be opened; each unreadable one adds the warning package_binding_unreadable. instance (alias, instance_id or ADB port, as the configuration names them) narrows instances only; role narrows both lists. An answer larger than the output budget is written whole, its warnings included, to %TEMP%\\actingcommand-mcp\\materials\\<sha256>.json and answered as {source, export {path, sha256, size}} with the warning answer_exported, whose count is the number of warnings the file holds; narrow it with role or instance. A configuration that cannot be read answers config_read_failed, one that does not decode config_decode_failed.";

pub(super) fn run(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance", "role"])?;
    let instance = arguments.string("instance")?;
    let role = arguments
        .string("role")?
        .map(|text| {
            Role::parse(text)
                .ok_or_else(|| invalid_argument("role", ArgumentProblem::NotRecognised))
        })
        .transpose()?;
    let (installation, config) =
        reader::selected_config(context.runtime.requested_root()).map_err(refused)?;
    let answer =
        reader::read(installation.as_ref(), &config, &Query { instance, role }).map_err(refused)?;
    within_budget(&answer)
}

/// The answer as it is when it fits the output budget; otherwise the whole document (the CLI's
/// stdout without its `outcome`, warnings included) exported, answered as `{source, export}`
/// with `answer_exported` naming the file (`path`) and counting the warnings it holds (`count`).
fn within_budget(answer: &Answer) -> ToolOutcome {
    let result = encode(answer.packs())?;
    let warnings = answer
        .warnings()
        .iter()
        .map(tools::warning)
        .collect::<Vec<_>>();
    // The envelope is `{ok, result, warnings}`; this wrapper is a little larger, so an answer
    // it lets through always fits.
    if tools::fits_success(&json!({"result": result, "warnings": warnings})) {
        return Ok(ToolSuccess { result, warnings });
    }
    let document = answer.document(tools::warning).map_err(encode_failed)?;
    let bytes = serde_json::to_vec(&Value::Object(document)).map_err(encode_failed)?;
    let export = observer::export_bytes(
        &format!("sha256:{:x}", Sha256::digest(&bytes)),
        "json",
        &bytes,
    )?;
    let mut note = Outcome::new(ActingctlCode::AnswerExported).count(warnings.len());
    if let Some(path) = export["path"].as_str() {
        note = note.path(path);
    }
    Ok(ToolSuccess {
        result: json!({"source": encode(answer.packs().source())?, "export": export}),
        warnings: vec![tools::warning(&note)],
    })
}

fn encode<T: Serialize>(value: &T) -> Result<Value, ToolError> {
    serde_json::to_value(value).map_err(encode_failed)
}

/// An answer that does not encode: `actingctl_output_failed`, caused by the encoder's failure.
pub(super) fn encode_failed(error: serde_json::Error) -> ToolError {
    ToolError::new("runtime", ActingctlCode::ActingctlOutputFailed).caused_by(
        Outcome::new(ContractCode::ForeignDataError)
            .raw_source(RawSource::Json)
            .raw_text(error.to_string()),
    )
}

/// A refusal of the reader with the CLI's outcome: its code, its values (the configuration's
/// path, the selector) and the failure beneath it.
fn refused(error: PacksError) -> ToolError {
    ToolError::outcome("usage", error.outcome())
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
        let whole = serde_json::to_vec(&Value::Object(
            answer
                .document(tools::warning)
                .expect("encode the document"),
        ))
        .expect("encode the document");

        let success = within_budget(&answer)
            .unwrap_or_else(|error| panic!("ac_packs refused: {}", error.to_value()));

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
        // Workflow #378 A5: each warning is `{outcome}`.
        let codes = success
            .warnings
            .iter()
            .map(|warning| warning["outcome"]["code"].clone())
            .collect::<Vec<_>>();
        assert_eq!(codes, vec![json!(ActingctlCode::AnswerExported.as_str())]);
    }
}
