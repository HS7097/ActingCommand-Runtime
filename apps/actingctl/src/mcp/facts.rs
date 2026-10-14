// SPDX-License-Identifier: AGPL-3.0-only

//! `ac_facts` (Workflow #389 MCP-4; MCP-0 command table v1.1 §4.4): `actingctl facts --program`,
//! `facts --instance` and `status --config` over MCP, `scope` choosing the form (#389 v2.2.3
//! UIM:549). Its arguments are the CLI's flags (R2) and its result is the CLI's stdout JSON
//! (`crate::facts`). The one presentation difference (§2.2): an answer larger than the output
//! budget is written whole to the export directory and answered as `{scope, read_at_unix_ms,
//! records_total, records_kept, export}` with the warning `answer_exported`.

use super::tools::{
    self, Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument,
};
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::facts::{self, FactsRequest, Failure, Scope};
use actingcommand_contract::outcome::Outcome;
use serde_json::{Map, Value, json};

const ARGUMENTS: [&str; 3] = ["scope", "instance", "key_prefix"];
/// The members an exported answer keeps beside `export`.
const EXPORTED_HEADER: [&str; 4] = ["scope", "read_at_unix_ms", "records_total", "records_kept"];

pub(super) const DESCRIPTION: &str = "Reads facts, as actingctl facts and status --config print them; nothing is recorded in the ledger. scope program: the Runtime's own fact store as snapshot (its records in snapshot.records). scope config: only its config.* records as records (config.subsystems, config.selection when recorded, every config.parameters.<section> in key order); config_facts_missing when the Runtime recorded none. scope instance: one instance's facts; instance (alias, instance_id or ADB port, resolved with one status read) is required, and refused with the other scopes. Its snapshot is {instance_alias, server_id, game_id, records}: the instance's own records and those of the server and game its configuration binds it to (server_id and game_id are absent when the configuration binds none). Each record carries scope, key, content, observed_at_unix_ms and expires_at_unix_ms. key_prefix (scope program or instance) keeps only records whose key starts with it. Every answer is {scope, read_at_unix_ms, snapshot or records, records_total (records read), records_kept (records answered)}; read_at_unix_ms is the Runtime's clock at the read, to compare each record's observed_at_unix_ms and expires_at_unix_ms with. An answer larger than the output budget is written whole to %TEMP%\\actingcommand-mcp\\materials\\<sha256>.json and answered as {scope, read_at_unix_ms, records_total, records_kept, export {path, sha256, size}} with the warning answer_exported; narrow it with key_prefix.";

pub(super) fn run(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &ARGUMENTS)?;
    let scope = Scope::parse(arguments.required_string("scope")?)
        .ok_or_else(|| invalid_argument("scope", ArgumentProblem::NotRecognised))?;
    let instance = arguments.string("instance")?;
    let key_prefix = arguments.string("key_prefix")?.map(str::to_owned);
    let request = match (scope, instance) {
        (Scope::Instance, Some(selector)) => FactsRequest::Instance {
            selector: selector.to_owned(),
            key_prefix,
        },
        (Scope::Instance, None) => {
            return Err(invalid_argument("instance", ArgumentProblem::Required));
        }
        (_, Some(_)) => {
            return Err(invalid_argument("instance", ArgumentProblem::Conflicting));
        }
        (Scope::Program, None) => FactsRequest::Program { key_prefix },
        (Scope::Config, None) if key_prefix.is_some() => {
            return Err(invalid_argument("key_prefix", ArgumentProblem::Conflicting));
        }
        (Scope::Config, None) => FactsRequest::Config,
    };
    let connected = context.runtime.connect()?;
    let answer = facts::read(&connected.client, &request).map_err(|failure| match failure {
        Failure::Selector { miss, selector } => {
            ToolError::outcome("usage", miss.outcome(&selector))
        }
        Failure::ConfigFactsMissing => ToolError::new("runtime", ActingctlCode::ConfigFactsMissing),
        Failure::Runtime(error) => context.runtime.failure(&connected, &error),
        Failure::Output(error) => super::packs::encode_failed(error),
    })?;
    if tools::fits_success(&answer) {
        return Ok(ToolSuccess::new(answer));
    }
    let export = super::events::export_json(&answer)?;
    let mut exported = EXPORTED_HEADER
        .iter()
        .filter_map(|member| {
            answer
                .get(*member)
                .map(|value| ((*member).to_owned(), value.clone()))
        })
        .collect::<Map<_, _>>();
    let mut warning = Outcome::new(ActingctlCode::AnswerExported);
    if let Some(path) = export["path"].as_str() {
        warning = warning.path(path);
    }
    exported.insert("export".to_owned(), export);
    Ok(ToolSuccess {
        result: Value::Object(exported),
        warnings: vec![tools::warning(&warning)],
    })
}

pub(super) fn input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "scope": {
                "type": "string",
                "enum": ["program", "config", "instance"],
                "description": "program: the Runtime's fact store (facts --program); config: its config.* records (status --config); instance: one instance's facts (facts --instance).",
            },
            "instance": {
                "type": "string",
                "minLength": 1,
                "description": "Instance alias, instance_id or ADB port; required with scope instance, refused with the others.",
            },
            "key_prefix": {
                "type": "string",
                "minLength": 1,
                "description": "Only records whose key starts with it; scope program or instance.",
            },
        },
        "required": ["scope"],
        "additionalProperties": false,
    })
}

pub(super) fn result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "scope": {"type": "string", "enum": ["program", "config", "instance"]},
            "read_at_unix_ms": {"type": "integer"},
            "snapshot": {"type": "object"},
            "records": {"type": "array", "items": {"type": "object"}},
            "records_total": {"type": "integer"},
            "records_kept": {"type": "integer"},
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
        "required": ["scope", "read_at_unix_ms", "records_total", "records_kept"],
    })
}
