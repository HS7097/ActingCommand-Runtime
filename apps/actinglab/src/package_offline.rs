// SPDX-License-Identifier: AGPL-3.0-only

use crate::{CliError, CliOutcome, ErrorKind, FlagArgs, GlobalOptions};
use actingcommand_device::{CaptureBackendName, Frame};
use actingcommand_lab::{
    ExternalExpectedSha256, OfflineSimulationError, OfflineSimulationResult, PreparedContainedTask,
    simulate_contained_task, validate_lab_package_bytes,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use zip::{ZipWriter, write::FileOptions};

const RESULT_SCHEMA: &str = "actingcommand.offline-simulation.v1";
const FIXTURE_HASH_DOMAIN: &[u8] = b"ActingCommand recorded fixture sequence v1\0";

pub(super) fn capability() -> Value {
    json!({
        "command": "package dry-run",
        "needs": ["offline"],
        "status": "available",
        "executed": false
    })
}

pub(super) fn run_dry_run(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    flags.expect_positionals("package dry-run", 0)?;
    reject_device_scope(global, flags)?;
    let zip_path = flags.required_path("--zip")?;
    let out_path = flags.required_path("--out")?;
    let expected_text = flags.required("--expected-sha256")?;
    let expected = ExternalExpectedSha256::parse_hex(&expected_text)
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
    let fixture_paths = fixture_paths(flags)?;
    reject_output_collision(&out_path, &zip_path, &fixture_paths)?;

    let package_bytes = fs::read(&zip_path).map_err(|error| {
        offline_error(
            "offline_package_read_failed",
            format!("failed to read package {}: {error}", zip_path.display()),
        )
    })?;
    let loaded = validate_lab_package_bytes("contained-package.zip", &package_bytes, expected)?;
    let prepared = PreparedContainedTask::load("offline.simulation", &package_bytes, expected)
        .map_err(|error| {
            offline_error(
                error.code(),
                error
                    .detail()
                    .map(str::to_string)
                    .unwrap_or_else(|| error.to_string()),
            )
        })?;
    let package_sha256 = prepared.package_sha256().to_string();
    let fixture = load_fixture_sequence(&fixture_paths)?;
    let simulation =
        simulate_contained_task(&prepared, fixture.frames).map_err(map_simulation_error)?;
    let record = OfflineResultRecord {
        schema_version: RESULT_SCHEMA,
        mode: "offline_simulation",
        executed: false,
        runtime_head: env!("ACTINGCOMMAND_RUNTIME_HEAD"),
        package_sha256: package_sha256.clone(),
        fixture_sequence_sha256: fixture.sequence_sha256.clone(),
        fixtures: fixture.bindings,
        loaded,
        simulation,
        production_global_ledger_written: false,
    };
    let bundle = result_bundle(&record)?;
    let bundle_sha256 = sha256_hex(&bundle);
    write_result_bundle(&out_path, &bundle, &bundle_sha256)?;

    Ok(json!({
        "status": "offline_simulation",
        "executed": false,
        "runtime_head": env!("ACTINGCOMMAND_RUNTIME_HEAD"),
        "package_sha256": package_sha256,
        "fixture_sequence_sha256": fixture.sequence_sha256,
        "result_zip": out_path.display().to_string(),
        "result_zip_sha256": bundle_sha256,
        "decision": record.simulation.decision,
        "recognition": record.simulation.recognition,
        "production_global_ledger_written": false
    }))
}

fn reject_device_scope(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<()> {
    let unexpected_flags = flags
        .flags
        .keys()
        .filter(|name| {
            !matches!(
                name.as_str(),
                "--zip" | "--out" | "--expected-sha256" | "--fixture"
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    if !unexpected_flags.is_empty() {
        return Err(CliError::usage(format!(
            "package dry-run received unsupported flags: {}",
            unexpected_flags.join(", ")
        )));
    }
    if global.instance.is_some()
        || !global.instances.is_empty()
        || global.profile.is_some()
        || global.game.is_some()
        || global.server.is_some()
        || global.runtime_endpoint.is_some()
        || global.capture_backend.is_some()
        || global.touch_backend.is_some()
        || global.run_root.is_some()
        || global.resource_root.is_some()
    {
        return Err(offline_error(
            "offline_device_scope_forbidden",
            "package dry-run accepts package and recorded fixtures only; device, Runtime, instance, profile, game, server, and backend selectors are forbidden",
        ));
    }
    Ok(())
}

fn fixture_paths(flags: &FlagArgs) -> CliOutcome<Vec<PathBuf>> {
    let paths = flags
        .values("--fixture")
        .into_iter()
        .filter(|value| value != "true")
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if paths.is_empty() || paths.len() != flags.values("--fixture").len() {
        return Err(offline_error(
            "offline_fixture_missing",
            "package dry-run requires one or more --fixture <recorded.png> values",
        ));
    }
    Ok(paths)
}

fn reject_output_collision(out: &Path, zip: &Path, fixtures: &[PathBuf]) -> CliOutcome<()> {
    let out = collision_path(out)?;
    let mut inputs = vec![collision_path(zip)?];
    for fixture in fixtures {
        inputs.push(collision_path(fixture)?);
    }
    if inputs.iter().any(|input| input == &out) {
        return Err(offline_error(
            "offline_output_conflicts_with_input",
            "result --out must not overwrite the package or a recorded fixture",
        ));
    }
    Ok(())
}

fn collision_path(path: &Path) -> CliOutcome<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .map_err(|error| {
                offline_error(
                    "offline_current_directory_failed",
                    format!("failed to resolve current directory: {error}"),
                )
            })?
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    if normalized.exists() {
        fs::canonicalize(&normalized).map_err(|error| {
            offline_error(
                "offline_path_resolution_failed",
                format!("failed to resolve {}: {error}", normalized.display()),
            )
        })
    } else {
        Ok(normalized)
    }
}

struct LoadedFixtureSequence {
    frames: Vec<Frame>,
    bindings: Vec<FixtureBinding>,
    sequence_sha256: String,
}

#[derive(Serialize)]
struct FixtureBinding {
    index: usize,
    sha256: String,
    width: u32,
    height: u32,
}

fn load_fixture_sequence(paths: &[PathBuf]) -> CliOutcome<LoadedFixtureSequence> {
    let mut frames = Vec::with_capacity(paths.len());
    let mut bindings = Vec::with_capacity(paths.len());
    let mut sequence_hasher = Sha256::new();
    sequence_hasher.update(FIXTURE_HASH_DOMAIN);
    for (index, path) in paths.iter().enumerate() {
        let png = fs::read(path).map_err(|error| {
            offline_error(
                "offline_fixture_read_failed",
                format!("failed to read fixture {}: {error}", path.display()),
            )
        })?;
        let hash = sha256_hex(&png);
        sequence_hasher.update((png.len() as u64).to_be_bytes());
        sequence_hasher.update(&png);
        let frame = Frame::from_png(png, CaptureBackendName::AdbScreencap).map_err(|error| {
            offline_error(
                "offline_fixture_invalid",
                format!("failed to decode fixture {}: {error}", path.display()),
            )
        })?;
        bindings.push(FixtureBinding {
            index,
            sha256: hash,
            width: frame.width,
            height: frame.height,
        });
        frames.push(frame);
    }
    Ok(LoadedFixtureSequence {
        frames,
        bindings,
        sequence_sha256: format!("{:x}", sequence_hasher.finalize()),
    })
}

#[derive(Serialize)]
struct OfflineResultRecord {
    schema_version: &'static str,
    mode: &'static str,
    executed: bool,
    runtime_head: &'static str,
    package_sha256: String,
    fixture_sequence_sha256: String,
    fixtures: Vec<FixtureBinding>,
    loaded: actingcommand_lab::LabContainedPackageValidationResponse,
    simulation: OfflineSimulationResult,
    production_global_ledger_written: bool,
}

fn result_bundle(record: &OfflineResultRecord) -> CliOutcome<Vec<u8>> {
    let cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(cursor);
    zip.start_file(
        "offline-simulation.json",
        FileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .map_err(|error| {
        offline_error(
            "offline_result_bundle_failed",
            format!("failed to start result bundle: {error}"),
        )
    })?;
    serde_json::to_writer_pretty(&mut zip, record).map_err(|error| {
        offline_error(
            "offline_result_bundle_failed",
            format!("failed to serialize result: {error}"),
        )
    })?;
    zip.write_all(b"\n").map_err(|error| {
        offline_error(
            "offline_result_bundle_failed",
            format!("failed to finish result record: {error}"),
        )
    })?;
    zip.finish().map(Cursor::into_inner).map_err(|error| {
        offline_error(
            "offline_result_bundle_failed",
            format!("failed to close result bundle: {error}"),
        )
    })
}

fn write_result_bundle(path: &Path, bytes: &[u8], expected_sha256: &str) -> CliOutcome<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            offline_error(
                "offline_result_write_failed",
                format!("failed to create {}: {error}", parent.display()),
            )
        })?;
    }
    fs::write(path, bytes).map_err(|error| {
        offline_error(
            "offline_result_write_failed",
            format!("failed to write {}: {error}", path.display()),
        )
    })?;
    let persisted = fs::read(path).map_err(|error| {
        offline_error(
            "offline_result_verify_failed",
            format!("failed to verify {}: {error}", path.display()),
        )
    })?;
    if sha256_hex(&persisted) != expected_sha256 {
        return Err(offline_error(
            "offline_result_verify_failed",
            format!("persisted result hash mismatch for {}", path.display()),
        ));
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn map_simulation_error(error: OfflineSimulationError) -> CliError {
    offline_error(error.code(), error.to_string())
}

fn offline_error(code: impl Into<String>, message: impl Into<String>) -> CliError {
    CliError::new(ErrorKind::UsageValidation, code, message, &[])
}
