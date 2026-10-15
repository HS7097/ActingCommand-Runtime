// SPDX-License-Identifier: AGPL-3.0-only

use crate::{CliError, CliOutcome, ErrorKind, FlagArgs, GlobalOptions};
use actingcommand_contract::{
    ContainedTaskRequest, MAX_READONLY_OBSERVATION_ARTIFACT_BYTES,
    MAX_RUNTIME_CAPTURE_SEQUENCE_FRAMES, PackageRef,
};
use actingcommand_device::{CaptureBackendName, Frame, parse_png_dimensions};
use actingcommand_lab::{
    OfflineDecision, OfflineSimulationError, OfflineSimulationResult, PreparedContainedTask,
    simulate_contained_task_at, validate_lab_package_path,
};
use actingcommand_pack_containment::DEFAULT_MAX_COMPRESSED_BYTES;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Cursor, ErrorKind as IoErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use zip::{ZipWriter, write::FileOptions};

const RESULT_SCHEMA: &str = "actingcommand.offline-simulation.v1";
const FIXTURE_HASH_DOMAIN: &[u8] = b"ActingCommand recorded fixture sequence v1\0";
const RESULT_TEMP_ATTEMPTS: usize = 32;
const MAX_OFFLINE_FIXTURE_COUNT: usize = MAX_RUNTIME_CAPTURE_SEQUENCE_FRAMES as usize;
const MAX_OFFLINE_FIXTURE_BYTES: u64 = MAX_READONLY_OBSERVATION_ARTIFACT_BYTES;
const RGBA_BYTES_PER_PIXEL: u64 = 4;
const MAX_OFFLINE_FIXTURE_DECODED_PIXELS: u64 = MAX_OFFLINE_FIXTURE_BYTES / RGBA_BYTES_PER_PIXEL;
// Four maximum-sized decoded observation buffers bound the peak payload retained by this
// single-purpose offline adapter without creating a second capture policy.
const MAX_OFFLINE_FIXTURE_RESIDENT_BYTES: u64 = MAX_OFFLINE_FIXTURE_BYTES * 4;
static RESULT_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) fn capability() -> Value {
    let mut capability =
        crate::commands::capabilities::command_cap("package dry-run", ["offline"], "available");
    capability["executed"] = json!(false);
    capability["limits"] = json!({
        "package_compressed_bytes": DEFAULT_MAX_COMPRESSED_BYTES,
        "fixture_count": MAX_OFFLINE_FIXTURE_COUNT,
        "fixture_bytes": MAX_OFFLINE_FIXTURE_BYTES,
        "fixture_decoded_pixels": MAX_OFFLINE_FIXTURE_DECODED_PIXELS,
        "fixture_aggregate_resident_bytes": MAX_OFFLINE_FIXTURE_RESIDENT_BYTES
    });
    capability
}

pub(super) fn run_dry_run(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    // A select step evaluates at the Lab process's instant when the dry run starts.
    let now_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .filter(|now| *now > 0)
        .ok_or_else(|| {
            offline_error(
                "offline_clock_invalid",
                "the Lab process clock is before the Unix epoch".to_string(),
            )
        })?;
    let args = PackageDryRunArgs::parse(global, flags)?;
    reject_output_collision(&args.out, &args.package, &args.fixtures)?;

    // Workflow #336 PKG-S4 (option 丙, forced compromise T-01): the package by its path, read as
    // the path chooses (PKG-S1): a content directory, a `.json` or `.zip` container, or an
    // assembled ZIP. No digest is taken or computed. A package file keeps the bounded
    // regular-file budget.
    let package = absolute_package_path(&args.package)?;
    if !fs::symlink_metadata(&package).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        read_regular_file_bounded(
            &package,
            DEFAULT_MAX_COMPRESSED_BYTES,
            "offline_package",
            "package",
        )?;
    }
    let deadline = std::time::Instant::now()
        .checked_add(std::time::Duration::from_millis(
            ContainedTaskRequest::DEFAULT_RESPONSE_DEADLINE_MS,
        ))
        .ok_or_else(|| {
            offline_error(
                "offline_clock_invalid",
                "the package admission deadline overflows".to_string(),
            )
        })?;
    let loaded = validate_lab_package_path("contained-package.zip", &package, deadline)?;
    // Workflow #396 LAB-3: the replay evaluates OCR targets with the Runtime's OCR provider.
    let prepared = PreparedContainedTask::load_path(
        "offline.simulation",
        &package,
        &PackageRef::default(),
        crate::offline_ocr::lab_ocr().provider(),
        deadline,
    )
    .map_err(|error| {
        offline_error(
            error.code().as_str(),
            error
                .detail()
                .map(str::to_string)
                .unwrap_or_else(|| error.to_string()),
        )
    })?;
    let package_label = prepared.package_label().to_owned();
    let fixture = load_fixture_sequence(&args.fixtures)?;
    let simulation = simulate_contained_task_at(&prepared, fixture.frames, now_unix_ms)
        .map_err(map_simulation_error)?;
    let refusal = match &simulation.decision {
        OfflineDecision::Refused { code, detail } => Some((*code, detail.clone())),
        _ => None,
    };
    let record = OfflineResultRecord {
        schema_version: RESULT_SCHEMA,
        mode: "offline_simulation",
        executed: false,
        runtime_head: env!("ACTINGCOMMAND_RUNTIME_HEAD"),
        package_label: package_label.clone(),
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

    if let Some((code, detail)) = refusal {
        return Err(offline_error(
            code.as_str(),
            detail.unwrap_or_else(|| format!("offline decision refused with {code}")),
        )
        .with_details(json!({
            "status": "refused",
            "executed": false,
            "decision_fingerprint": record.decision_fingerprint,
            "fixture_sequence_sha256": record.fixture_sequence_sha256,
            "result_zip": args.out.display().to_string(),
            "result_zip_sha256": bundle_sha256,
            "decision": record.simulation.decision,
            "post_admission_ocr": record.simulation.post_admission_ocr,
            "ocr": crate::offline_ocr::status(),
            "production_global_ledger_written": false
        })));
    }

    Ok(json!({
        "status": "offline_simulation",
        "executed": false,
        "runtime_head": env!("ACTINGCOMMAND_RUNTIME_HEAD"),
        "package_label": package_label,
        "decision_fingerprint": record.decision_fingerprint.as_str(),
        "fixture_sequence_sha256": record.fixture_sequence_sha256.as_str(),
        "result_zip": args.out.display().to_string(),
        "result_zip_sha256": bundle_sha256,
        "decision": record.simulation.decision,
        "recognition": record.simulation.recognition,
        "post_admission_ocr": record.simulation.post_admission_ocr,
        "ocr": crate::offline_ocr::status(),
        "production_global_ledger_written": false
    }))
}

struct PackageDryRunArgs {
    /// Workflow #336 PKG-S4: `--package <path>` or `--zip <path>`, exactly one.
    package: PathBuf,
    fixtures: Vec<PathBuf>,
    out: PathBuf,
}

impl PackageDryRunArgs {
    fn parse(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Self> {
        flags.expect_positionals("package dry-run", 0)?;
        if global.version {
            return Err(CliError::usage(
                "package dry-run cannot be combined with --version",
            ));
        }
        if global.run_root.is_some()
            || global.instance.is_some()
            || global.instances.is_some()
            || global.profile.is_some()
            || global.resource_root.is_some()
            || global.dry_run
            || global.verbose
            || global.quiet
            || global.game.is_some()
            || global.server.is_some()
            || global.runtime_endpoint.is_some()
            || global.capture_backend.is_some()
            || global.touch_backend.is_some()
        {
            return Err(offline_error(
                "offline_device_scope_forbidden",
                "package dry-run accepts no global Runtime, device, selector, backend, resource, or execution flags",
            ));
        }

        let unexpected = flags
            .flags
            .keys()
            .filter(|name| !matches!(name.as_str(), "--package" | "--zip" | "--out" | "--fixture"))
            .cloned()
            .collect::<Vec<_>>();
        if !unexpected.is_empty() {
            let capability_attempt = unexpected.iter().any(|name| {
                matches!(
                    name.as_str(),
                    "--send-input" | "--device" | "--adb" | "--lease" | "--ledger" | "--scheduler"
                )
            });
            return if capability_attempt {
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

        let package = match (flags.optional("--package"), flags.optional("--zip")) {
            (Some(_), None) => singleton_path(flags, "--package")?,
            (None, Some(_)) => singleton_path(flags, "--zip")?,
            _ => {
                return Err(offline_error(
                    "offline_argument_invalid",
                    "package dry-run requires exactly one of --package <path> and --zip <path>"
                        .to_string(),
                ));
            }
        };
        let out = singleton_path(flags, "--out")?;
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
        if fixture_values.len() > MAX_OFFLINE_FIXTURE_COUNT {
            return Err(offline_error(
                "offline_fixture_count_exceeded",
                format!(
                    "package dry-run received {} fixtures, limit {MAX_OFFLINE_FIXTURE_COUNT}",
                    fixture_values.len()
                ),
            ));
        }
        let fixtures = fixture_values.into_iter().map(PathBuf::from).collect();
        Ok(Self {
            package,
            fixtures,
            out,
        })
    }
}

/// The package path as containment reads it: absolute, its components kept (Workflow #336
/// PKG-S1, links are refused there).
fn absolute_package_path(path: &Path) -> CliOutcome<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|current| current.join(path))
        .map_err(|error| {
            offline_error(
                "offline_current_directory_failed",
                format!("failed to resolve current directory: {error}"),
            )
        })
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
    if paths.len() > MAX_OFFLINE_FIXTURE_COUNT {
        return Err(offline_error(
            "offline_fixture_count_exceeded",
            format!(
                "package dry-run received {} fixtures, limit {MAX_OFFLINE_FIXTURE_COUNT}",
                paths.len()
            ),
        ));
    }
    let mut frames = Vec::with_capacity(paths.len());
    let mut bindings = Vec::with_capacity(paths.len());
    let mut sequence_hasher = Sha256::new();
    let mut resident_bytes = 0_u64;
    sequence_hasher.update(FIXTURE_HASH_DOMAIN);
    for (index, path) in paths.iter().enumerate() {
        let png = read_regular_file_bounded(
            path,
            MAX_OFFLINE_FIXTURE_BYTES,
            "offline_fixture",
            "fixture",
        )?;
        let (width, height) = parse_png_dimensions(&png).map_err(|error| {
            offline_error(
                "offline_fixture_invalid",
                format!("failed to inspect fixture {}: {error}", path.display()),
            )
        })?;
        let decoded_bytes = decoded_rgba_bytes(width, height)?;
        let png_resident_bytes = u64::try_from(png.capacity()).map_err(|_| {
            offline_error(
                "offline_fixture_resident_bytes_exceeded",
                format!("fixture {} capacity cannot be represented", path.display()),
            )
        })?;
        charge_fixture_resident(resident_bytes, &[png_resident_bytes, decoded_bytes])?;
        let hash = sha256_hex(&png);
        sequence_hasher.update((png.len() as u64).to_be_bytes());
        sequence_hasher.update(&png);
        let mut frame =
            Frame::from_png(png, CaptureBackendName::AdbScreencap).map_err(|error| {
                offline_error(
                    "offline_fixture_invalid",
                    format!("failed to decode fixture {}: {error}", path.display()),
                )
            })?;
        frame.original_png = None;
        let actual_decoded_bytes = u64::try_from(frame.pixels.capacity()).map_err(|_| {
            offline_error(
                "offline_fixture_resident_bytes_exceeded",
                format!(
                    "decoded fixture {} capacity cannot be represented",
                    path.display()
                ),
            )
        })?;
        resident_bytes = charge_fixture_resident(resident_bytes, &[actual_decoded_bytes])?;
        bindings.push(FixtureBinding {
            index,
            sha256: hash,
            width,
            height,
        });
        frames.push(frame);
    }
    Ok(LoadedFixtureSequence {
        frames,
        bindings,
        sequence_sha256: format!("{:x}", sequence_hasher.finalize()),
    })
}

/// Workflow #308 SEL-L6a: one recorded frame read and decoded as a dry-run fixture is, under
/// the same per-fixture bounds, for a replay that reads its sequence one frame at a time.
pub(crate) fn read_fixture_frame(path: &Path) -> CliOutcome<Frame> {
    let png = read_regular_file_bounded(
        path,
        MAX_OFFLINE_FIXTURE_BYTES,
        "offline_fixture",
        "fixture",
    )?;
    let (width, height) = parse_png_dimensions(&png).map_err(|error| {
        offline_error(
            "offline_fixture_invalid",
            format!("failed to inspect fixture {}: {error}", path.display()),
        )
    })?;
    decoded_rgba_bytes(width, height)?;
    let mut frame = Frame::from_png(png, CaptureBackendName::AdbScreencap).map_err(|error| {
        offline_error(
            "offline_fixture_invalid",
            format!("failed to decode fixture {}: {error}", path.display()),
        )
    })?;
    frame.original_png = None;
    Ok(frame)
}

fn read_regular_file_bounded(
    path: &Path,
    maximum_bytes: u64,
    code_prefix: &str,
    label: &str,
) -> CliOutcome<Vec<u8>> {
    let path_metadata = fs::symlink_metadata(path).map_err(|error| {
        offline_error(
            format!("{code_prefix}_metadata_failed"),
            format!("failed to inspect {label} {}: {error}", path.display()),
        )
    })?;
    if !path_metadata.file_type().is_file() {
        return Err(offline_error(
            format!("{code_prefix}_not_regular_file"),
            format!("{label} {} is not a regular file", path.display()),
        ));
    }
    if path_metadata.len() > maximum_bytes {
        return Err(offline_error(
            format!("{code_prefix}_too_large"),
            format!(
                "{label} {} is {} bytes, limit {maximum_bytes}",
                path.display(),
                path_metadata.len()
            ),
        ));
    }

    let mut file = fs::File::open(path).map_err(|error| {
        offline_error(
            format!("{code_prefix}_open_failed"),
            format!("failed to open {label} {}: {error}", path.display()),
        )
    })?;
    let metadata = file.metadata().map_err(|error| {
        offline_error(
            format!("{code_prefix}_metadata_failed"),
            format!(
                "failed to inspect opened {label} {}: {error}",
                path.display()
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(offline_error(
            format!("{code_prefix}_not_regular_file"),
            format!("opened {label} {} is not a regular file", path.display()),
        ));
    }
    if metadata.len() > maximum_bytes {
        return Err(offline_error(
            format!("{code_prefix}_too_large"),
            format!(
                "opened {label} {} is {} bytes, limit {maximum_bytes}",
                path.display(),
                metadata.len()
            ),
        ));
    }
    let length = usize::try_from(metadata.len()).map_err(|_| {
        offline_error(
            format!("{code_prefix}_too_large"),
            format!(
                "opened {label} {} length cannot fit in process memory",
                path.display()
            ),
        )
    })?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|error| {
        offline_error(
            format!("{code_prefix}_allocation_failed"),
            format!(
                "failed to reserve {length} bytes for {label} {}: {error}",
                path.display()
            ),
        )
    })?;
    bytes.resize(length, 0);
    file.read_exact(&mut bytes).map_err(|error| {
        offline_error(
            format!("{code_prefix}_read_failed"),
            format!("failed to read {label} {}: {error}", path.display()),
        )
    })?;
    let mut extra = [0_u8; 1];
    match file.read(&mut extra) {
        Ok(0) => Ok(bytes),
        Ok(_) => Err(offline_error(
            format!("{code_prefix}_changed_during_read"),
            format!(
                "{label} {} grew while it was being read and was rejected",
                path.display()
            ),
        )),
        Err(error) => Err(offline_error(
            format!("{code_prefix}_read_failed"),
            format!(
                "failed to verify the end of {label} {}: {error}",
                path.display()
            ),
        )),
    }
}

fn decoded_rgba_bytes(width: u32, height: u32) -> CliOutcome<u64> {
    decoded_rgba_bytes_with_limit(width, height, MAX_OFFLINE_FIXTURE_DECODED_PIXELS)
}

fn decoded_rgba_bytes_with_limit(width: u32, height: u32, maximum_pixels: u64) -> CliOutcome<u64> {
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| {
            offline_error(
                "offline_fixture_decoded_pixels_exceeded",
                format!("fixture dimensions overflow: {width}x{height}"),
            )
        })?;
    if pixels > maximum_pixels {
        return Err(offline_error(
            "offline_fixture_decoded_pixels_exceeded",
            format!(
                "fixture dimensions {width}x{height} contain {pixels} pixels, limit {maximum_pixels}"
            ),
        ));
    }
    pixels.checked_mul(RGBA_BYTES_PER_PIXEL).ok_or_else(|| {
        offline_error(
            "offline_fixture_decoded_pixels_exceeded",
            format!("fixture RGBA size overflows: {width}x{height}"),
        )
    })
}

fn charge_fixture_resident(current: u64, additions: &[u64]) -> CliOutcome<u64> {
    charge_fixture_resident_with_limit(current, additions, MAX_OFFLINE_FIXTURE_RESIDENT_BYTES)
}

fn charge_fixture_resident_with_limit(
    current: u64,
    additions: &[u64],
    maximum: u64,
) -> CliOutcome<u64> {
    let next = additions.iter().try_fold(current, |total, value| {
        total.checked_add(*value).ok_or_else(|| {
            offline_error(
                "offline_fixture_resident_bytes_exceeded",
                "fixture resident byte accounting overflowed",
            )
        })
    })?;
    if next > maximum {
        return Err(offline_error(
            "offline_fixture_resident_bytes_exceeded",
            format!("fixture resident payload would be {next} bytes, limit {maximum}"),
        ));
    }
    Ok(next)
}

#[derive(Serialize)]
struct OfflineResultRecord {
    schema_version: &'static str,
    mode: &'static str,
    executed: bool,
    runtime_head: &'static str,
    /// Workflow #336 PKG-S4: the package's `package_id`; no digest.
    package_label: String,
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
    offline_error(error.code().as_str(), error.to_string())
}

fn offline_error(code: impl Into<String>, message: impl Into<String>) -> CliError {
    CliError::new(ErrorKind::UsageValidation, code, message, &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn bounded_regular_reader_enforces_limit_minus_one_limit_and_limit_plus_one() {
        let temp = TempDir::new().expect("temp dir");
        for (name, length) in [("below", 7_usize), ("limit", 8), ("above", 9)] {
            let path = temp.path().join(name);
            fs::write(&path, vec![b'x'; length]).expect("write fixture");
            let result = read_regular_file_bounded(&path, 8, "offline_test", "test input");
            if length <= 8 {
                assert_eq!(result.expect("bounded read").len(), length);
            } else {
                assert_eq!(
                    result.expect_err("limit+1 must fail").code,
                    "offline_test_too_large"
                );
            }
        }

        let directory = temp.path().join("directory");
        fs::create_dir(&directory).expect("create directory");
        assert_eq!(
            read_regular_file_bounded(&directory, 8, "offline_test", "test input")
                .expect_err("directory must fail")
                .code,
            "offline_test_not_regular_file"
        );
    }

    #[test]
    fn decoded_pixel_and_aggregate_resident_budgets_cover_boundaries() {
        assert_eq!(
            decoded_rgba_bytes_with_limit(3, 1, 4).expect("limit-1 pixels"),
            12
        );
        assert_eq!(
            decoded_rgba_bytes_with_limit(2, 2, 4).expect("exact pixel limit"),
            16
        );
        assert_eq!(
            decoded_rgba_bytes_with_limit(5, 1, 4)
                .expect_err("limit+1 pixels")
                .code,
            "offline_fixture_decoded_pixels_exceeded"
        );

        assert_eq!(
            charge_fixture_resident_with_limit(4, &[3], 8).expect("limit-1 resident"),
            7
        );
        assert_eq!(
            charge_fixture_resident_with_limit(4, &[4], 8).expect("exact resident limit"),
            8
        );
        assert_eq!(
            charge_fixture_resident_with_limit(4, &[5], 8)
                .expect_err("cumulative limit+1")
                .code,
            "offline_fixture_resident_bytes_exceeded"
        );
    }
}
