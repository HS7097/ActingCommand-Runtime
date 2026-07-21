// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, GlobalOptions, attach_package_event};
use actingcommand_lab::Sha256Hash;
use actingcommand_lab::{PackageValidateRequest, PackageValidationResponse};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

#[path = "package_offline.rs"]
mod offline;

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
