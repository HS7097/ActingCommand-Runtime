// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, device_config_for_instance, effective_adb_path,
    effective_run_root, read_user_config,
};
use actingcommand_device::CaptureBackendChoice;
use actingcommand_lab::{
    ExternalExpectedSha256, FrameStoreControl, LabRunDeviceResolver, LabRunProcessContext,
    LabRunRequest, LabRunSelectedDevice, LabValidateRequest, MemorySampleSource,
};
use actingcommand_pack_containment::{ContainmentError, Sha256Hash};
use serde::Serialize;
use serde_json::Value;
use std::env;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const GIT_COMMIT_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) fn run_lab_run(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let (request, config) = lab_run_request(global, &flags)?;
    let mut lab = super::env_detection::build_control_lab(config)?;
    serialize_response(lab.lab_run(request)?)
}

fn lab_run_request(
    global: &GlobalOptions,
    flags: &FlagArgs,
) -> CliOutcome<(LabRunRequest, actingcommand_lab::UserConfig)> {
    let zip_path = flags
        .optional_path("--zip")
        .or_else(|| flags.optional_path("--package"))
        .ok_or_else(|| CliError::usage("lab run requires --zip <input.zip>"))?;
    let out_path = flags.required_path("--out")?;
    let config = read_user_config()?;
    let run_root = flags
        .optional_path("--run-root")
        .or_else(|| effective_run_root(global, &config))
        .unwrap_or_else(|| PathBuf::from("target").join("actinglab-runs"));
    Ok((
        LabRunRequest {
            zip_path,
            out_path,
            run_root,
            game: global.game.clone(),
            server: global.server.clone(),
            instance: global.instance.clone(),
            device_resolver: Box::new(AppLabRunDeviceResolver::new(global, &config)),
            capture_interval_override: parse_optional_u64(flags, "--capture-interval-ms")?,
            capture_backend_override: global
                .capture_backend
                .or(parse_optional_capture_backend(flags, "--capture-backend")?),
            frame_store_override: parse_frame_store_control_from_flags(flags)?,
            expected_input_sha256: parse_required_external_sha256(flags, "--expected-sha256")?,
            process: process_context()?,
        },
        config,
    ))
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

struct AppLabRunDeviceResolver {
    global: GlobalOptions,
    config: actingcommand_lab::UserConfig,
}

struct AppRuntimeCommitSource;

impl actingcommand_lab::RuntimeCommitSource for AppRuntimeCommitSource {
    fn sample(&self) -> Option<String> {
        git_commit()
    }
}

impl AppLabRunDeviceResolver {
    fn new(global: &GlobalOptions, config: &actingcommand_lab::UserConfig) -> Self {
        Self {
            global: global.clone(),
            config: config.clone(),
        }
    }
}

impl LabRunDeviceResolver for AppLabRunDeviceResolver {
    fn resolve_selected(&mut self, instance_id: &str) -> CliOutcome<LabRunSelectedDevice> {
        let device = device_config_for_instance(&self.global, &self.config, Some(instance_id))?;
        let adb_provenance = effective_adb_path(&self.config)?.path;
        Ok(LabRunSelectedDevice::new(
            instance_id,
            device.target.resolved_serial(),
            adb_provenance,
            device.capture_backend_config(),
            device.touch_backend_config(),
        ))
    }
}

fn process_context() -> CliOutcome<LabRunProcessContext> {
    Ok(LabRunProcessContext {
        current_dir: env::current_dir().ok(),
        os: env::consts::OS.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        runtime_commit_source: Arc::new(AppRuntimeCommitSource),
        memory_source: MemorySampleSource::live(super::frame_store::sample_system_memory),
    })
}

fn parse_optional_u64(flags: &FlagArgs, name: &str) -> CliOutcome<Option<u64>> {
    parse_optional(flags, name)
}

fn parse_optional_f32(flags: &FlagArgs, name: &str) -> CliOutcome<Option<f32>> {
    parse_optional(flags, name)
}

fn parse_optional_f64(flags: &FlagArgs, name: &str) -> CliOutcome<Option<f64>> {
    parse_optional(flags, name)
}

fn parse_optional<T>(flags: &FlagArgs, name: &str) -> CliOutcome<Option<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    flags
        .optional(name)
        .filter(|value| value != "true")
        .map(|value| {
            value.parse::<T>().map_err(|err| {
                CliError::usage(format!("failed to parse {name} value '{value}': {err}"))
            })
        })
        .transpose()
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

fn parse_required_external_sha256(
    flags: &FlagArgs,
    name: &str,
) -> CliOutcome<ExternalExpectedSha256> {
    let value = flags
        .optional(name)
        .filter(|value| value != "true")
        .ok_or_else(|| {
            CliError::usage(format!(
                "lab run requires {name} <sha256> from an external trust source"
            ))
        })?;
    ExternalExpectedSha256::parse_hex(&value).map_err(containment_error)
}

fn parse_frame_store_control_from_flags(flags: &FlagArgs) -> CliOutcome<FrameStoreControl> {
    let control = FrameStoreControl {
        similarity_threshold: parse_optional_f32(flags, "--similarity-threshold")?,
        tier1_ratio: parse_optional_f64(flags, "--tier1-ratio")?,
        tier2_ratio: parse_optional_f64(flags, "--tier2-ratio")?,
        tier3_ratio: parse_optional_f64(flags, "--tier3-ratio")?,
        hysteresis_ratio: parse_optional_f64(flags, "--hysteresis-ratio")?,
        max_mem_bytes: parse_optional_u64(flags, "--max-mem-bytes")?,
        os_reserve_bytes: parse_optional_u64(flags, "--os-reserve-bytes")?,
        flush_workspace_reserve_bytes: parse_optional_u64(
            flags,
            "--flush-workspace-reserve-bytes",
        )?,
    };
    control.validate().map_err(CliError::usage)?;
    Ok(control)
}

fn parse_optional_capture_backend(
    flags: &FlagArgs,
    name: &str,
) -> CliOutcome<Option<CaptureBackendChoice>> {
    flags
        .optional(name)
        .filter(|value| value != "true")
        .map(|value| {
            CaptureBackendChoice::parse(&value).map_err(|err| CliError::usage(err.to_string()))
        })
        .transpose()
}

fn containment_error(error: ContainmentError) -> CliError {
    CliError::package_invalid(error.to_string())
}

fn serialize_response<T: Serialize>(response: T) -> CliOutcome<Value> {
    serde_json::to_value(response)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}

fn git_commit() -> Option<String> {
    let mut child = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "echo")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().ok()? {
            break status;
        }
        if started.elapsed() >= GIT_COMMIT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(25));
    };
    if !status.success() {
        return None;
    }
    let mut stdout = Vec::new();
    child.stdout.take()?.read_to_end(&mut stdout).ok()?;
    Some(String::from_utf8_lossy(&stdout).trim().to_string())
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

        let error = parse_required_external_sha256(&flags, "--expected-sha256")
            .expect_err("missing external hash");

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
    fn frame_store_flags_build_typed_override() {
        let flags = FlagArgs::parse(&[
            "--similarity-threshold".to_string(),
            "0.8".to_string(),
            "--max-mem-bytes".to_string(),
            "4096".to_string(),
        ])
        .expect("flags");

        let control = parse_frame_store_control_from_flags(&flags).expect("frame store flags");

        assert_eq!(control.similarity_threshold, Some(0.8));
        assert_eq!(control.max_mem_bytes, Some(4096));
    }

    #[test]
    fn device_resolver_never_opens_unselected_candidate() {
        let temp = tempfile::TempDir::new().expect("temp");
        let adb = temp.path().join("adb");
        std::fs::write(&adb, b"fixture").expect("adb");
        let mut config = actingcommand_lab::UserConfig {
            adb_path: Some(adb.display().to_string()),
            ..Default::default()
        };
        config.instances.insert(
            "a-invalid".to_string(),
            actingcommand_lab::InstanceConfig {
                game: Some("azurlane".to_string()),
                server: Some("jp".to_string()),
                capture_backend: Some("invalid-backend".to_string()),
                ..Default::default()
            },
        );
        config.instances.insert(
            "b-valid".to_string(),
            actingcommand_lab::InstanceConfig {
                serial: Some("fixture:5555".to_string()),
                game: Some("arknights".to_string()),
                server: Some("cn".to_string()),
                adb_path: Some(adb.display().to_string()),
                capture_backend: Some("adb".to_string()),
                ..Default::default()
            },
        );

        let mut resolver = AppLabRunDeviceResolver::new(&GlobalOptions::default(), &config);
        let selected = resolver
            .resolve_selected("b-valid")
            .expect("selected configuration");

        assert_eq!(selected.id(), "b-valid");
        assert_eq!(selected.serial(), "fixture:5555");
        assert_eq!(
            selected.capture_config().adb_config.adb_path,
            adb.display().to_string()
        );
    }

    #[test]
    fn lab_run_keeps_the_existing_reported_adb_path() {
        let temp = tempfile::TempDir::new().expect("temp");
        let global_adb = temp.path().join("global-adb");
        let instance_adb = temp.path().join("instance-adb");
        std::fs::write(&global_adb, b"fixture").expect("global adb");
        std::fs::write(&instance_adb, b"fixture").expect("instance adb");
        let global = GlobalOptions::default();
        let mut config = actingcommand_lab::UserConfig {
            adb_path: Some(global_adb.display().to_string()),
            ..Default::default()
        };
        config.instances.insert(
            "selected".to_string(),
            actingcommand_lab::InstanceConfig {
                serial: Some("fixture:5555".to_string()),
                adb_path: Some(instance_adb.display().to_string()),
                capture_backend: Some("adb".to_string()),
                ..Default::default()
            },
        );

        let mut resolver = AppLabRunDeviceResolver::new(&global, &config);
        let selected = resolver
            .resolve_selected("selected")
            .expect("selected configuration");

        assert_eq!(selected.adb_provenance(), global_adb.display().to_string());
        assert_eq!(
            selected.capture_config().adb_config.adb_path,
            instance_adb.display().to_string()
        );
    }

    #[test]
    fn selected_capture_validation_precedes_touch_and_adb_failures() {
        let temp = tempfile::TempDir::new().expect("temp");
        let mut config = actingcommand_lab::UserConfig {
            adb_path: Some(temp.path().join("missing-adb").display().to_string()),
            ..Default::default()
        };
        config.instances.insert(
            "selected".to_string(),
            actingcommand_lab::InstanceConfig {
                capture_backend: Some("invalid-capture".to_string()),
                touch_backend: Some("invalid-touch".to_string()),
                ..Default::default()
            },
        );
        let mut resolver = AppLabRunDeviceResolver::new(&GlobalOptions::default(), &config);

        let error = resolver
            .resolve_selected("selected")
            .expect_err("invalid selected capture");

        assert!(error.message.contains("capture_backend"));
        assert!(!error.message.contains("touch_backend"));
    }

    #[test]
    fn selected_touch_validation_precedes_adb_failure() {
        let temp = tempfile::TempDir::new().expect("temp");
        let mut config = actingcommand_lab::UserConfig {
            adb_path: Some(temp.path().join("missing-adb").display().to_string()),
            ..Default::default()
        };
        config.instances.insert(
            "selected".to_string(),
            actingcommand_lab::InstanceConfig {
                capture_backend: Some("adb".to_string()),
                touch_backend: Some("invalid-touch".to_string()),
                ..Default::default()
            },
        );
        let mut resolver = AppLabRunDeviceResolver::new(&GlobalOptions::default(), &config);

        let error = resolver
            .resolve_selected("selected")
            .expect_err("invalid selected touch");

        assert!(error.message.contains("touch_backend"));
        assert!(!error.message.contains("adb path"));
    }

    #[test]
    fn selected_global_adb_provenance_is_validated_before_resolution_returns() {
        let temp = tempfile::TempDir::new().expect("temp");
        let instance_adb = temp.path().join("instance-adb");
        std::fs::write(&instance_adb, b"fixture").expect("instance adb");
        let mut config = actingcommand_lab::UserConfig {
            adb_path: Some(temp.path().join("missing-global-adb").display().to_string()),
            ..Default::default()
        };
        config.instances.insert(
            "selected".to_string(),
            actingcommand_lab::InstanceConfig {
                adb_path: Some(instance_adb.display().to_string()),
                capture_backend: Some("adb".to_string()),
                touch_backend: Some("maatouch".to_string()),
                ..Default::default()
            },
        );
        let mut resolver = AppLabRunDeviceResolver::new(&GlobalOptions::default(), &config);

        let error = resolver
            .resolve_selected("selected")
            .expect_err("invalid global ADB provenance");

        assert!(error.message.to_ascii_lowercase().contains("adb"));
    }
}
