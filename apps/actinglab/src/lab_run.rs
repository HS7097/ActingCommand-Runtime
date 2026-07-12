// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, read_user_config, resolve_instance_id,
    runtime_slice_cli, runtime_state_root,
};
use actingcommand_contract::{ContainedTaskRequest, EventActor, EventSource};
use actingcommand_lab::LabValidateRequest;
use actingcommand_pack_containment::{ContainmentError, Sha256Hash};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde::Serialize;
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use zip::{ZipWriter, write::FileOptions};

pub(super) fn run_lab_run(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    flags.expect_positionals("lab run", 0)?;
    reject_client_execution_overrides(global, &flags)?;
    let package = flags
        .optional_path("--zip")
        .or_else(|| flags.optional_path("--package"))
        .ok_or_else(|| CliError::usage("lab run requires --zip <input.zip>"))?;
    let package = fs::canonicalize(&package).map_err(|error| {
        CliError::package_invalid(format!(
            "failed to canonicalize contained task package {}: {error}",
            package.display()
        ))
    })?;
    let expected_sha256 = required_expected_sha256(&flags)?;
    let output_path = flags.required_path("--out")?;
    let config = read_user_config()?;
    let instance = resolve_instance_id(global, &config)?;
    let request = ContainedTaskRequest::new(package.display().to_string(), expected_sha256)
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        runtime_state_root()?,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .map_err(runtime_slice_cli::map_runtime_error)?;
    let output = client
        .run_contained_task(&instance, request)
        .map_err(runtime_slice_cli::map_runtime_error)?;
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

fn required_expected_sha256(flags: &FlagArgs) -> CliOutcome<String> {
    let value = flags
        .optional("--expected-sha256")
        .filter(|value| value != "true")
        .ok_or_else(|| {
            CliError::usage(
                "lab run requires --expected-sha256 <sha256> from an external trust source",
            )
        })?;
    Sha256Hash::parse_hex(&value).map_err(containment_error)?;
    Ok(value)
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

pub(super) fn run_lab_validate(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let request = LabValidateRequest {
        zip_path: flags.required_path("--zip")?,
        expected_input_sha256: parse_optional_sha256(&flags, "--expected-sha256")?,
    };
    let mut lab = super::env_detection::build_readonly_lab()?;
    serialize_response(lab.lab_validate(request)?)
}

fn parse_optional_sha256(flags: &FlagArgs, name: &str) -> CliOutcome<Option<Sha256Hash>> {
    match flags.optional(name) {
        None => Ok(None),
        Some(value) if value == "true" => Err(CliError::usage(format!(
            "{name} requires an explicit SHA-256 value"
        ))),
        Some(value) => Sha256Hash::parse_hex(&value)
            .map(Some)
            .map_err(containment_error),
    }
}

fn containment_error(error: ContainmentError) -> CliError {
    CliError::package_invalid(error.to_string())
}

fn serialize_response<T: Serialize>(response: T) -> CliOutcome<Value> {
    serde_json::to_value(response)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_run_requires_external_expected_hash() {
        let flags = FlagArgs::parse(&[
            "--zip".to_string(),
            "input.zip".to_string(),
            "--out".to_string(),
            "output.zip".to_string(),
        ])
        .expect("flags");

        let error = required_expected_sha256(&flags).expect_err("missing external hash");

        assert_eq!(error.code, "validation_failed");
        assert!(error.message.contains("external trust source"));
    }

    #[test]
    fn optional_expected_hash_without_a_value_is_rejected() {
        let flags = FlagArgs::parse(&["--expected-sha256".to_string()]).expect("flags");

        let error = parse_optional_sha256(&flags, "--expected-sha256")
            .expect_err("empty expected hash must fail");

        assert_eq!(error.code, "validation_failed");
        assert!(error.message.contains("explicit SHA-256 value"));
    }

    #[test]
    fn production_run_rejects_client_device_policy() {
        let flags = FlagArgs::parse(&["--capture-interval-ms".to_string(), "10".to_string()])
            .expect("flags");

        let error = reject_client_execution_overrides(&GlobalOptions::default(), &flags)
            .expect_err("client execution policy must be retired");

        assert_eq!(error.code, "actinglab_run_authority_retired");
    }
}
