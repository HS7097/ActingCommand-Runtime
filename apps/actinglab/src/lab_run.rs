// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, device_config_for_instance, effective_adb_path,
    effective_run_root, read_user_config,
};
use actingcommand_device::CaptureBackendChoice;
use actingcommand_lab::{
    FrameStoreControl, LabRunDeviceCandidate, LabRunDeviceConfig, LabRunProcessContext,
    LabRunRequest, LabValidateRequest, MemorySampleSource,
};
use actingcommand_pack_containment::{ContainmentError, Sha256Hash};
use serde::Serialize;
use serde_json::Value;
use std::env;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const GIT_COMMIT_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) fn run_lab_run(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let (request, config) = lab_run_request(global, &flags)?;
    let mut lab = super::env_detection::build_control_lab(config, None)?;
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
            device_candidates: device_candidates(global, &config),
            capture_interval_override: parse_optional_u64(flags, "--capture-interval-ms")?,
            capture_backend_override: global
                .capture_backend
                .or(parse_optional_capture_backend(flags, "--capture-backend")?),
            frame_store_override: parse_frame_store_control_from_flags(flags)?,
            expected_input_sha256: parse_optional_sha256(flags, "--expected-sha256")?,
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

fn device_candidates(
    global: &GlobalOptions,
    config: &actingcommand_lab::UserConfig,
) -> Vec<LabRunDeviceCandidate> {
    let ids = match &global.instance {
        Some(instance) => vec![instance.clone()],
        None => config.instances.keys().cloned().collect(),
    };
    ids.into_iter()
        .map(
            |id| match resolved_lab_run_device_config(global, config, &id) {
                Ok(device) => LabRunDeviceCandidate::resolved(id, device),
                Err(error) => LabRunDeviceCandidate::failed(id, error),
            },
        )
        .collect()
}

fn resolved_lab_run_device_config(
    global: &GlobalOptions,
    config: &actingcommand_lab::UserConfig,
    id: &str,
) -> CliOutcome<LabRunDeviceConfig> {
    let device = device_config_for_instance(global, config, Some(id))?;
    Ok(LabRunDeviceConfig {
        instance: device.target.resolved_serial(),
        adb_path: effective_adb_path(config)?.path,
        capture_config: device.capture_backend_config(),
        touch_config: device.touch_backend_config(),
    })
}

fn process_context() -> CliOutcome<LabRunProcessContext> {
    let lease_root = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("ActingCommand")
        .join("actinglab")
        .join("locks");
    Ok(LabRunProcessContext {
        current_dir: env::current_dir().ok(),
        lease_root,
        os: env::consts::OS.to_string(),
        runtime_commit: git_commit(),
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
    flags
        .optional(name)
        .filter(|value| value != "true")
        .map(|value| Sha256Hash::parse_hex(&value).map_err(containment_error))
        .transpose()
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
    fn device_candidates_defer_unselected_configuration_errors() {
        let mut config = actingcommand_lab::UserConfig {
            adb_path: Some("adb".to_string()),
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
                capture_backend: Some("adb".to_string()),
                ..Default::default()
            },
        );

        let candidates = device_candidates(&GlobalOptions::default(), &config);

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].id(), "a-invalid");
        assert_eq!(candidates[1].id(), "b-valid");
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

        let device =
            resolved_lab_run_device_config(&global, &config, "selected").expect("device config");

        assert_eq!(device.adb_path, global_adb.display().to_string());
        assert_eq!(
            device.capture_config.adb_config.adb_path,
            instance_adb.display().to_string()
        );
    }
}
