// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, GlobalOptions, attach_package_event};
use actingcommand_lab::{PackageValidateRequest, PackageValidationResponse};
use actingcommand_pack_containment::Sha256Hash;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::Path;

#[path = "package_offline.rs"]
mod offline;

pub(super) fn is_offline_command(args: &[String]) -> bool {
    matches!(args, [group, command, ..] if group == "package" && matches!(command.as_str(), "dry-run" | "preflight"))
}

pub(super) fn run_preflight(flags: &FlagArgs) -> CliOutcome<Value> {
    for name in flags.flags.keys() {
        if !matches!(
            name.as_str(),
            "--package" | "--package-ref" | "--zip" | "--expected-sha256"
        ) {
            return Err(CliError::usage(format!(
                "package preflight does not accept {name}"
            )));
        }
    }
    let resources = super::contained_resources::load(flags, "package-preflight").map_err(|error| {
        error.with_details(json!({"coverage":{"declaration_parse":"not_completed","package_load":"failed","task_prepare":"not_run","execution":"not_run"}}))
    })?;
    let bundle = std::sync::Arc::try_unwrap(resources)
        .map_err(|_| CliError::package_invalid("preflight package ownership is shared"))?;
    let prepared = actingcommand_lab::PreparedContainedTask::from_bundle(bundle).map_err(|error| {
        CliError::package_invalid(format!("{}: {}", error.code(), error.detail().unwrap_or("task preparation failed")))
            .with_details(json!({"code":error.code(),"declaration":error.declaration_issue(),"coverage":{"declaration_parse":"passed","package_load":"passed","task_prepare":"failed","execution":"not_run"}}))
    })?;
    Ok(json!({
        "schema_version":"actingcommand.package-preflight.v1", "status":"prepared", "executed":false,
        "task_id":prepared.task_label(), "package_ref":prepared.package_sha256(),
        "coverage":{"declaration_parse":"passed","package_load":"passed","task_prepare":"passed","recognition_execution":"not_run","execution":"not_run"},
        "effective_timing":prepared.effective_timing(), "production_global_ledger_written":false
    }))
}

pub(super) fn run_offline(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    offline::run_dry_run(global, flags)
}

pub(super) fn offline_capability() -> Value {
    offline::capability()
}

pub(super) fn run_validate(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let zip = flags.required_path("--zip")?;
    let expected_input_sha256 = optional_expected_sha256(flags)?;
    let validation = validate_package_with_expected(&zip, false, expected_input_sha256)?;
    let mut payload = serialize_response(&validation)?;
    payload["coverage"] =
        json!({"package_format":"passed","task_prepare":"not_run","execution":"not_run"});
    attach_package_event(
        global,
        "package.validate.ok",
        "package-validate",
        &zip,
        &validation,
        &mut payload,
    )?;
    Ok(payload)
}

pub(super) fn validate_package(
    zip_path: &Path,
    include_entries: bool,
) -> CliOutcome<PackageValidationResponse> {
    validate_package_with_expected(zip_path, include_entries, None)
}

fn validate_package_with_expected(
    zip_path: &Path,
    include_entries: bool,
    expected_input_sha256: Option<Sha256Hash>,
) -> CliOutcome<PackageValidationResponse> {
    let mut lab = super::env_detection::build_readonly_lab()?;
    lab.package_validate(PackageValidateRequest {
        zip_path: zip_path.to_path_buf(),
        include_entries,
        expected_input_sha256,
    })
}

fn optional_expected_sha256(flags: &FlagArgs) -> CliOutcome<Option<Sha256Hash>> {
    match flags.optional("--expected-sha256") {
        None => Ok(None),
        Some(value) if value == "true" => Err(CliError::usage(
            "--expected-sha256 requires an explicit SHA-256 value",
        )),
        Some(value) => Sha256Hash::parse_hex(&value)
            .map(Some)
            .map_err(|error| CliError::package_invalid(error.to_string())),
    }
}

pub(super) fn serialize_response<T: Serialize>(response: T) -> CliOutcome<Value> {
    serde_json::to_value(response)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}
