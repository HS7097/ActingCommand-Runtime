// SPDX-License-Identifier: AGPL-3.0-only

use crate::{CliError, CliOutcome, ErrorKind, FlagArgs, GlobalOptions};
use actingcommand_device::{CaptureBackendName, Frame};
use actingcommand_lab::{
    ExternalExpectedSha256, OfflineSimulationError, OfflineSimulationResult,
    prepare_lab_package_bytes, simulate_contained_task,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Cursor, ErrorKind as IoErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use zip::{ZipWriter, write::FileOptions};

const RESULT_SCHEMA: &str = "actingcommand.offline-simulation.v1";
const FIXTURE_HASH_DOMAIN: &[u8] = b"ActingCommand recorded fixture sequence v1\0";
const RESULT_TEMP_ATTEMPTS: usize = 32;
static RESULT_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) fn capability() -> Value {
    json!({
        "command": "package dry-run",
        "needs": ["offline"],
        "status": "available",
        "executed": false
    })
}

pub(super) fn run_dry_run(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let args = PackageDryRunArgs::parse(global, flags)?;
    let expected = ExternalExpectedSha256::parse_hex(&args.expected_sha256)
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
    reject_output_collision(&args.out, &args.zip, &args.fixtures)?;

    let package_bytes = fs::read(&args.zip).map_err(|error| {
        offline_error(
            "offline_package_read_failed",
            format!("failed to read package {}: {error}", args.zip.display()),
        )
    })?;
    let (prepared, loaded) =
        prepare_lab_package_bytes("contained-package.zip", &package_bytes, expected)?;
    let package_sha256 = prepared.package_sha256().to_string();
    let fixture = load_fixture_sequence(&args.fixtures)?;
    let simulation =
        simulate_contained_task(&prepared, fixture.frames).map_err(map_simulation_error)?;
    let record = OfflineResultRecord {
        schema_version: RESULT_SCHEMA,
        mode: "offline_simulation",
        executed: false,
        runtime_head: env!("ACTINGCOMMAND_RUNTIME_HEAD"),
        package_sha256: package_sha256.clone(),
        semantic_fingerprint: simulation.semantic_fingerprint.clone(),
        decision_fingerprint: simulation.decision_fingerprint.clone(),
        fixture_sequence_sha256: fixture.sequence_sha256.clone(),
        fixtures: fixture.bindings,
        loaded,
        simulation,
        production_global_ledger_written: false,
    };
    let bundle = result_bundle(&record)?;
    let bundle_sha256 = sha256_hex(&bundle);
    write_result_bundle(&args.out, &bundle, &bundle_sha256)?;

    Ok(json!({
        "status": "offline_simulation",
        "executed": false,
        "runtime_head": env!("ACTINGCOMMAND_RUNTIME_HEAD"),
        "package_sha256": package_sha256,
        "semantic_fingerprint": record.semantic_fingerprint.as_str(),
        "decision_fingerprint": record.decision_fingerprint.as_str(),
        "fixture_sequence_sha256": fixture.sequence_sha256,
        "result_zip": args.out.display().to_string(),
        "result_zip_sha256": bundle_sha256,
        "decision": record.simulation.decision,
        "recognition": record.simulation.recognition,
        "production_global_ledger_written": false
    }))
}

struct PackageDryRunArgs {
    zip: PathBuf,
    expected_sha256: String,
    fixtures: Vec<PathBuf>,
    out: PathBuf,
}

impl PackageDryRunArgs {
    fn parse(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Self> {
        flags.expect_positionals("package dry-run", 0)?;
        let forbidden_global = global
            .present_flags
            .keys()
            .filter(|name| name.as_str() != "--json")
            .cloned()
            .collect::<Vec<_>>();
        if !forbidden_global.is_empty() {
            return Err(offline_error(
                "offline_device_scope_forbidden",
                format!(
                    "package dry-run accepts no global Runtime, device, selector, backend, resource, or execution flags; received {}",
                    forbidden_global.join(", ")
                ),
            ));
        }

        let unexpected = flags
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
        if !unexpected.is_empty() {
            let device_attempt = unexpected.iter().any(|name| {
                matches!(
                    name.as_str(),
                    "--send-input" | "--device" | "--adb" | "--lease" | "--ledger" | "--scheduler"
                )
            });
            return if device_attempt {
                Err(offline_error(
                    "offline_device_scope_forbidden",
                    format!(
                        "package dry-run cannot accept device, Runtime, scheduler, lease, or ledger capabilities: {}",
                        unexpected.join(", ")
                    ),
                ))
            } else {
                Err(CliError::usage(format!(
                    "package dry-run received unsupported flags: {}",
                    unexpected.join(", ")
                )))
            };
        }

        let zip = singleton_path(flags, "--zip")?;
        let out = singleton_path(flags, "--out")?;
        let expected_sha256 = singleton_value(flags, "--expected-sha256")?;
        let fixture_values = flags.values("--fixture");
        if fixture_values.is_empty()
            || fixture_values
                .iter()
                .any(|value| value == "true" || value.trim().is_empty())
        {
            return Err(offline_error(
                "offline_fixture_missing",
                "package dry-run requires one or more non-empty --fixture <recorded.png> values",
            ));
        }
        let fixtures = fixture_values.into_iter().map(PathBuf::from).collect();
        Ok(Self {
            zip,
            expected_sha256,
            fixtures,
            out,
        })
    }
}

fn singleton_path(flags: &FlagArgs, name: &str) -> CliOutcome<PathBuf> {
    singleton_value(flags, name).map(PathBuf::from)
}

fn singleton_value(flags: &FlagArgs, name: &str) -> CliOutcome<String> {
    let values = flags.values(name);
    let [value] = values.as_slice() else {
        return Err(offline_error(
            "offline_argument_invalid",
            format!("package dry-run requires exactly one non-empty {name} <value>"),
        ));
    };
    if value == "true" || value.trim().is_empty() {
        return Err(offline_error(
            "offline_argument_invalid",
            format!("package dry-run requires exactly one non-empty {name} <value>"),
        ));
    }
    Ok(value.clone())
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
    match fs::symlink_metadata(&out) {
        Ok(_) => {
            return Err(offline_error(
                "offline_output_already_exists",
                format!(
                    "result --out already exists and will not be overwritten: {}",
                    out.display()
                ),
            ));
        }
        Err(error) if error.kind() == IoErrorKind::NotFound => {}
        Err(error) => {
            return Err(offline_error(
                "offline_path_resolution_failed",
                format!("failed to inspect result --out {}: {error}", out.display()),
            ));
        }
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
    semantic_fingerprint: String,
    decision_fingerprint: String,
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
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| {
        offline_error(
            "offline_result_write_failed",
            format!("failed to create {}: {error}", parent.display()),
        )
    })?;
    let (temporary_path, mut temporary_file) = create_result_temp(parent)?;
    if let Err(error) = temporary_file
        .write_all(bytes)
        .and_then(|()| temporary_file.flush())
        .and_then(|()| temporary_file.sync_all())
    {
        drop(temporary_file);
        return Err(remove_failed_temp(
            &temporary_path,
            "offline_result_write_failed",
            format!(
                "failed to write temporary result {}: {error}",
                temporary_path.display()
            ),
        ));
    }
    drop(temporary_file);
    let persisted = fs::read(&temporary_path).map_err(|error| {
        remove_failed_temp(
            &temporary_path,
            "offline_result_verify_failed",
            format!(
                "failed to verify temporary result {}: {error}",
                temporary_path.display()
            ),
        )
    })?;
    if sha256_hex(&persisted) != expected_sha256 {
        return Err(remove_failed_temp(
            &temporary_path,
            "offline_result_verify_failed",
            format!(
                "temporary result hash mismatch for {}",
                temporary_path.display()
            ),
        ));
    }
    if let Err(error) = fs::hard_link(&temporary_path, path) {
        let (code, message) = if error.kind() == IoErrorKind::AlreadyExists {
            (
                "offline_output_already_exists",
                format!(
                    "result --out appeared during publication and will not be overwritten: {}",
                    path.display()
                ),
            )
        } else {
            (
                "offline_result_write_failed",
                format!(
                    "failed to publish result {} from {}: {error}",
                    path.display(),
                    temporary_path.display()
                ),
            )
        };
        return Err(remove_failed_temp(&temporary_path, code, message));
    }
    fs::remove_file(&temporary_path).map_err(|error| {
        offline_error(
            "offline_result_write_failed",
            format!(
                "result {} was published but temporary link {} could not be removed: {error}",
                path.display(),
                temporary_path.display()
            ),
        )
    })?;
    let published = fs::read(path).map_err(|error| {
        offline_error(
            "offline_result_verify_failed",
            format!(
                "failed to verify published result {}: {error}",
                path.display()
            ),
        )
    })?;
    if sha256_hex(&published) != expected_sha256 {
        return Err(offline_error(
            "offline_result_verify_failed",
            format!("published result hash mismatch for {}", path.display()),
        ));
    }
    Ok(())
}

fn create_result_temp(parent: &Path) -> CliOutcome<(PathBuf, fs::File)> {
    for _ in 0..RESULT_TEMP_ATTEMPTS {
        let sequence = RESULT_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".actingcommand-offline-result.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == IoErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(offline_error(
                    "offline_result_write_failed",
                    format!(
                        "failed to create temporary result {}: {error}",
                        candidate.display()
                    ),
                ));
            }
        }
    }
    Err(offline_error(
        "offline_result_write_failed",
        format!(
            "failed to allocate a unique temporary result in {} after {RESULT_TEMP_ATTEMPTS} attempts",
            parent.display()
        ),
    ))
}

fn remove_failed_temp(path: &Path, code: &str, message: String) -> CliError {
    match fs::remove_file(path) {
        Ok(()) => offline_error(code, message),
        Err(error) if error.kind() == IoErrorKind::NotFound => offline_error(code, message),
        Err(error) => offline_error(
            code,
            format!(
                "{message}; additionally failed to remove temporary result {}: {error}",
                path.display()
            ),
        ),
    }
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
