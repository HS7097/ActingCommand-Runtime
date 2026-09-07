// SPDX-License-Identifier: AGPL-3.0-only

use crate::commands::capabilities::command_cap;
use crate::runtime_debug::{parse_u16_flag, parse_u64_flag, runtime_lab_client};
use crate::{CliError, CliOutcome, FlagArgs};
use actingcommand_contract::{
    DiagnosticSignatureDefinition, MAX_SIGNATURE_PAGE_ROWS, RuntimeResult,
    RuntimeSignatureMatchRequest, SignaturePageRequest, SignatureRegistrationRef,
};
use serde_json::{Map, Value, json};

const DEFINITION: &[(&str, &str)] = &[
    ("--signature-id", "signature_id"),
    ("--signature-version", "version"),
    ("--origin-module", "origin_module"),
    ("--diagnostic-code", "diagnostic_code"),
    ("--event-type", "event_type"),
    ("--minimum-severity", "minimum_severity"),
];
const LIFECYCLE: &[(&str, &str)] = &[
    ("--lifecycle-stage", "stage"),
    ("--lifecycle-operation", "operation"),
    ("--lifecycle-code", "code"),
    ("--cause-phase", "cause_phase"),
    ("--cause-source", "cause_source"),
    ("--resource-kind", "resource"),
    ("--resource-phase", "resource_phase"),
    ("--quiescence", "quiescence"),
];
const REGISTRATION: &[(&str, &str)] = &[
    ("--signature-id", "signature_id"),
    ("--signature-version", "version"),
    ("--registration-event-id", "event_id"),
    ("--registration-sequence", "sequence"),
];
const MATCH_FLAGS: &[&str] = &[
    "--input-state-root",
    "--input-through",
    "--catalog-through",
    "--limit",
    "--cursor",
];

pub(super) fn capability(operation: &str) -> Value {
    let command = format!("lab signatures {operation}");
    let mut value = command_cap(&command, ["running_runtime"], "available");
    value["options"] = json!(options(operation));
    value
}

pub(super) fn options(operation: &str) -> Vec<&'static str> {
    match operation {
        "register" => DEFINITION
            .iter()
            .chain(LIFECYCLE)
            .map(|(flag, _)| *flag)
            .collect(),
        "retire" => REGISTRATION.iter().map(|(flag, _)| *flag).collect(),
        "match" => MATCH_FLAGS.to_vec(),
        _ => Vec::new(),
    }
}

pub(super) fn run_signatures(args: &[String]) -> CliOutcome<Value> {
    let (operation, rest) = args
        .split_first()
        .ok_or_else(|| CliError::usage("lab signatures requires register, match or retire"))?;
    if !matches!(operation.as_str(), "register" | "match" | "retire") {
        return Err(CliError::usage("unknown lab signatures operation"));
    }
    let flags = FlagArgs::parse_values(rest)?;
    flags.expect_positionals("lab signatures", 0)?;
    let allowed = options(operation);
    for (flag, values) in &flags.flags {
        if !allowed.contains(&flag.as_str()) || values.len() != 1 {
            return Err(CliError::usage(format!(
                "unsupported or duplicate signature option {flag}"
            )));
        }
    }
    let receipt = match operation.as_str() {
        "register" => {
            let mut object = fields(&flags, DEFINITION)?;
            let lifecycle = fields(&flags, LIFECYCLE)?;
            if !lifecycle.is_empty() {
                object.insert("lifecycle".into(), Value::Object(lifecycle));
            }
            let definition: DiagnosticSignatureDefinition =
                serde_json::from_value(Value::Object(object)).map_err(|error| {
                    CliError::usage(format!("invalid signature definition: {error}"))
                })?;
            definition
                .validate()
                .map_err(|error| CliError::usage(error.to_string()))?;
            runtime_lab_client()?.register_diagnostic_signature(definition)
        }
        "retire" => {
            let registration: SignatureRegistrationRef = serde_json::from_value(Value::Object(
                fields(&flags, REGISTRATION)?,
            ))
            .map_err(|error| CliError::usage(format!("invalid signature registration: {error}")))?;
            registration
                .validate()
                .map_err(|error| CliError::usage(error.to_string()))?;
            runtime_lab_client()?.retire_diagnostic_signature(registration)
        }
        _ => {
            let cursor = flags
                .optional("--cursor")
                .map(|value| {
                    if value.len() > 4096 {
                        return Err(CliError::usage("signature cursor exceeds bound"));
                    }
                    serde_json::from_str(&value).map_err(|error| {
                        CliError::usage(format!("invalid signature cursor: {error}"))
                    })
                })
                .transpose()?;
            let request = RuntimeSignatureMatchRequest {
                input_state_root: flags
                    .optional("--input-state-root")
                    .ok_or_else(|| CliError::usage("missing --input-state-root"))?,
                input_through: parse_u64_flag(&flags, "--input-through", 0)?,
                catalog_through: parse_u64_flag(&flags, "--catalog-through", 0)?,
                page: SignaturePageRequest {
                    limit: parse_u16_flag(&flags, "--limit", MAX_SIGNATURE_PAGE_ROWS)?,
                    cursor,
                },
            };
            request
                .validate()
                .map_err(|error| CliError::usage(error.to_string()))?;
            runtime_lab_client()?.match_diagnostic_signatures(request)
        }
    }
    .map_err(|error| CliError::device(error.to_string()))?;
    let complete = match receipt.result() {
        Some(RuntimeResult::SignaturesMatched { page }) => page.evidence_complete(),
        _ => true,
    };
    let result = json!({ "authority": "runtime_global_ledger", "operation": operation, "evidence_complete": complete, "receipt": receipt });
    if !complete {
        return Err(CliError::safety_blocked(
            "signature_replay_incomplete",
            "signature evidence contains explicit gaps",
            &["evidence_complete"],
        )
        .with_details(result));
    }
    Ok(result)
}

fn fields(flags: &FlagArgs, mapping: &[(&str, &str)]) -> CliOutcome<Map<String, Value>> {
    let mut result = Map::new();
    for (flag, field) in mapping {
        if let Some(value) = flags.optional(flag) {
            let value = if matches!(*field, "version" | "sequence") {
                json!(parse_u64_flag(flags, flag, 0)?)
            } else {
                json!(value)
            };
            result.insert((*field).to_owned(), value);
        }
    }
    Ok(result)
}
