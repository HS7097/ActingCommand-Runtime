// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, GlobalOptions};
use actingcommand_contract::{EventActor, EventSource, RuntimeErrorCode};
use actingcommand_device::{
    AdbConfig, CaptureBackend, CaptureBackendName, DeviceError, DeviceResult, DeviceTarget, Frame,
    ScreencapBackend, resolve_adb_path,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

const MAX_SEALED_FRAME_BYTES: u64 = 64 * 1024 * 1024;

pub(super) fn run(subcommand: &str, global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if !flags.positionals.is_empty() {
        return Err(CliError::usage(
            "runtime commands do not accept positional arguments",
        ));
    }
    let allowed = [
        "--state-root",
        "--instance",
        "--adb",
        "--serial",
        "--no-connect",
        "--sealed-frame",
        "--sealed-test",
    ];
    if flags
        .flags
        .keys()
        .any(|name| !allowed.contains(&name.as_str()))
    {
        return Err(CliError::usage("runtime command contains an unknown flag"));
    }
    let state_root = PathBuf::from(flags.required("--state-root")?);
    let instance = flags
        .optional("--instance")
        .or_else(|| global.instance.clone())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| CliError::usage("missing --instance <value>"))?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        state_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .map_err(map_runtime_error)?;
    let output = match subcommand {
        "reset" => {
            reject_capture_flags(&flags)?;
            client.safe_reset(&instance).map_err(map_runtime_error)?
        }
        "observe" => {
            let mut capture = open_capture(&flags)?;
            client
                .observe_readonly(&instance, capture.as_mut())
                .map_err(map_runtime_error)?
        }
        _ => {
            return Err(CliError::usage(format!(
                "unknown runtime command: {subcommand}"
            )));
        }
    };
    serde_json::to_value(output)
        .map_err(|error| CliError::usage(format!("runtime output serialization failed: {error}")))
}

fn map_runtime_error(error: RuntimeClientError) -> CliError {
    let unavailable = error.projection().is_none_or(|projection| {
        matches!(
            projection.code,
            RuntimeErrorCode::RuntimeUnavailable
                | RuntimeErrorCode::RuntimeFatal
                | RuntimeErrorCode::OwnerConflict
                | RuntimeErrorCode::ProtocolInvalid
                | RuntimeErrorCode::LedgerFailure
        )
    });
    if unavailable {
        CliError::runtime_not_running(error.to_string())
    } else {
        CliError::device(error.to_string())
    }
}

fn reject_capture_flags(flags: &FlagArgs) -> CliOutcome<()> {
    if [
        "--adb",
        "--serial",
        "--no-connect",
        "--sealed-frame",
        "--sealed-test",
    ]
    .iter()
    .any(|name| flags.flags.contains_key(*name))
    {
        return Err(CliError::usage(
            "runtime reset does not accept capture flags",
        ));
    }
    Ok(())
}

fn open_capture(flags: &FlagArgs) -> CliOutcome<Box<dyn CaptureBackend>> {
    let sealed_frame = flags.optional_path("--sealed-frame");
    let sealed_test = flags.bool("--sealed-test");
    let serial = flags.optional("--serial");
    let adb = flags.optional("--adb");
    match (sealed_frame, sealed_test, serial) {
        (Some(path), true, None) if adb.is_none() && !flags.bool("--no-connect") => {
            Ok(Box::new(SealedFrameCapture {
                path,
                consumed: false,
            }))
        }
        (None, false, Some(serial)) => {
            let resolved = resolve_adb_path(adb.as_deref())
                .map_err(|error| CliError::device(error.to_string()))?;
            if let Some(warning) = resolved.warning {
                eprintln!("WARNING actinglab runtime: {warning}");
            }
            Ok(Box::new(ScreencapBackend::new(
                AdbConfig {
                    adb_path: resolved.path,
                    command_timeout: Duration::from_secs(12),
                },
                DeviceTarget {
                    serial: Some(serial),
                    host: "127.0.0.1".to_string(),
                    port: 16384,
                    connect: !flags.bool("--no-connect"),
                },
            )))
        }
        _ => Err(CliError::usage(
            "runtime observe requires --serial [--adb] [--no-connect] or --sealed-test --sealed-frame <png>",
        )),
    }
}

struct SealedFrameCapture {
    path: PathBuf,
    consumed: bool,
}

impl CaptureBackend for SealedFrameCapture {
    fn capture(&mut self) -> DeviceResult<Frame> {
        if self.consumed {
            return Err(DeviceError::fatal("sealed frame was already consumed"));
        }
        self.consumed = true;
        let metadata = fs::metadata(&self.path).map_err(|error| {
            DeviceError::fatal(format!("sealed frame metadata failed: {error}"))
        })?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_SEALED_FRAME_BYTES {
            return Err(DeviceError::fatal("sealed frame size is invalid"));
        }
        let png = fs::read(&self.path)
            .map_err(|error| DeviceError::fatal(format!("sealed frame read failed: {error}")))?;
        Frame::from_png(png, CaptureBackendName::AdbScreencap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_rejects_capture_flags() {
        let flags = FlagArgs::parse(&[
            "--state-root".to_string(),
            "state".to_string(),
            "--instance".to_string(),
            "ak.cn".to_string(),
            "--serial".to_string(),
            "127.0.0.1:16416".to_string(),
        ])
        .expect("flags");
        assert!(reject_capture_flags(&flags).is_err());
    }

    #[test]
    fn sealed_capture_requires_explicit_marker() {
        let flags = FlagArgs::parse(&["--sealed-frame".to_string(), "frame.png".to_string()])
            .expect("flags");
        assert!(open_capture(&flags).is_err());
    }
}
