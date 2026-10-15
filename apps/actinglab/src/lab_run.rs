// SPDX-License-Identifier: AGPL-3.0-only

use super::contained_resources::{PackageInput, finish_package_use};
use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, read_user_config, resolve_instance_id,
    runtime_slice_cli, runtime_state_root,
};
use actingcommand_contract::ContainedTaskRequest;
use actingcommand_lab::LabValidateRequest;
use actingcommand_runtime_client::RuntimeClient;
use serde::Serialize;
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use zip::{ZipWriter, write::FileOptions};

pub(super) fn run_lab_run(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    if global.dry_run {
        return Err(CliError::safety_blocked(
            "explicit_offline_entry_required",
            "lab run is the Runtime-owned production execution path; use package dry-run for zero-device simulation",
            &["package_dry_run"],
        ));
    }
    let flags = FlagArgs::parse(args)?;
    flags.expect_positionals("lab run", 0)?;
    reject_client_execution_overrides(global, &flags)?;
    // Workflow #336 PKG-S4 (option 丙): the package by its path only (`--package` or `--zip`).
    let package_reader = PackageInput::open(&flags)?;
    let output_path = flags.required_path("--out")?;
    let config = read_user_config()?;
    let instance = resolve_instance_id(global, &config)?;
    let request = ContainedTaskRequest::new(package_reader.path().display().to_string())
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
    let client = RuntimeClient::connect(crate::lab_client_config(runtime_state_root()?))
        .map_err(runtime_slice_cli::map_runtime_error)?;
    let output = client
        .run_contained_task(&instance, request)
        .map_err(runtime_slice_cli::map_runtime_error);
    let output = finish_package_use(output, package_reader.close())?;
    let projection = serialize_response(&output)?;
    write_projection_package(&output_path, &projection)?;
    Ok(json!({
        "authority": "runtime",
        "projection_source": "runtime_global_ledger",
        "out": output_path,
        "runtime_flow": projection
    }))
}

fn reject_client_execution_overrides(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<()> {
    let local_override = global.run_root.is_some()
        || global.capture_backend.is_some()
        || global.touch_backend.is_some()
        || [
            "--run-root",
            "--capture-interval-ms",
            "--capture-backend",
            "--touch-backend",
            "--similarity-threshold",
            "--tier1-ratio",
            "--tier2-ratio",
            "--tier3-ratio",
            "--hysteresis-ratio",
            "--max-mem-bytes",
            "--os-reserve-bytes",
            "--flush-workspace-reserve-bytes",
        ]
        .iter()
        .any(|name| flags.optional(name).is_some());
    if local_override {
        return Err(CliError::not_implemented(
            "actinglab_run_authority_retired",
            "Lab execution overrides are retired; contained task execution policy is owned by Runtime and its admitted package",
        ));
    }
    Ok(())
}

fn write_projection_package(path: &Path, projection: &Value) -> CliOutcome<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            CliError::package_invalid(format!(
                "failed to create Runtime projection directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let file = File::create(path).map_err(|error| {
        CliError::package_invalid(format!(
            "failed to create Runtime projection package {}: {error}",
            path.display()
        ))
    })?;
    let mut zip = ZipWriter::new(file);
    zip.start_file(
        "runtime-flow.json",
        FileOptions::default().compression_method(zip::CompressionMethod::Deflated),
    )
    .map_err(|error| CliError::package_invalid(format!("failed to start projection: {error}")))?;
    serde_json::to_writer_pretty(&mut zip, projection).map_err(|error| {
        CliError::package_invalid(format!("failed to serialize Runtime projection: {error}"))
    })?;
    zip.write_all(b"\n").map_err(|error| {
        CliError::package_invalid(format!("failed to finish Runtime projection: {error}"))
    })?;
    zip.finish().map_err(|error| {
        CliError::package_invalid(format!("failed to close Runtime projection: {error}"))
    })?;
    Ok(())
}

/// Workflow #336 PKG-S4 (option 丙): the Lab package by its path; no digest is taken or
/// computed.
pub(super) fn run_lab_validate(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let request = LabValidateRequest {
        zip_path: flags.required_path("--zip")?,
    };
    let mut lab = super::env_detection::build_readonly_lab()?;
    serialize_response(lab.lab_validate(request)?)
}

fn serialize_response<T: Serialize>(response: T) -> CliOutcome<Value> {
    serde_json::to_value(response)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_run_rejects_client_device_policy() {
        let flags = FlagArgs::parse(&["--capture-interval-ms".to_string(), "10".to_string()])
            .expect("flags");

        let error = reject_client_execution_overrides(&GlobalOptions::default(), &flags)
            .expect_err("client execution policy must be retired");

        assert_eq!(error.code, "actinglab_run_authority_retired");
    }
}
