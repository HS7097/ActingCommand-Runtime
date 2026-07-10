// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, GlobalOptions, attach_package_event};
use actingcommand_lab::{PackageValidateRequest, PackageValidationResponse};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

pub(super) fn run_validate(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let zip = flags.required_path("--zip")?;
    let validation = validate_package(&zip, false)?;
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
    let mut lab = super::env_detection::build_readonly_lab()?;
    lab.package_validate(PackageValidateRequest {
        zip_path: zip_path.to_path_buf(),
        include_entries,
    })
}

pub(super) fn serialize_response<T: Serialize>(response: T) -> CliOutcome<Value> {
    serde_json::to_value(response)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}
