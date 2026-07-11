// SPDX-License-Identifier: AGPL-3.0-only

//! Thin production CLI for correlation-scoped Runtime flows.

#![forbid(unsafe_code)]

use actingcommand_contract::{EventActor, EventSource};
use actingcommand_device::{
    AdbConfig, CaptureBackend, CaptureBackendName, DeviceError, DeviceResult, DeviceTarget, Frame,
    ScreencapBackend, resolve_adb_path,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeFlowOutput};
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const MAX_SEALED_FRAME_BYTES: u64 = 64 * 1024 * 1024;

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(output) => match write_output(&output) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("FATAL actingctl: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("FATAL actingctl: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Vec<OsString>) -> Result<RuntimeFlowOutput, ActingctlError> {
    let invocation = Invocation::parse(arguments)?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &invocation.state_root,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(ActingctlError::runtime)?;
    match invocation.command {
        Command::Reset => client
            .safe_reset(&invocation.instance)
            .map_err(ActingctlError::runtime),
        Command::Observe(mode) => {
            let mut capture = mode.open()?;
            client
                .observe_readonly(&invocation.instance, capture.as_mut())
                .map_err(ActingctlError::runtime)
        }
    }
}

fn write_output(output: &RuntimeFlowOutput) -> Result<(), ActingctlError> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    serde_json::to_writer(&mut writer, output).map_err(|_| ActingctlError::Output)?;
    writer.write_all(b"\n").map_err(|_| ActingctlError::Output)
}

struct Invocation {
    state_root: PathBuf,
    instance: String,
    command: Command,
}

enum Command {
    Observe(ObserveMode),
    Reset,
}

enum ObserveMode {
    Adb {
        adb: Option<String>,
        serial: String,
        connect: bool,
    },
    SealedFrame(PathBuf),
}

impl ObserveMode {
    fn open(self) -> Result<Box<dyn CaptureBackend>, ActingctlError> {
        match self {
            Self::Adb {
                adb,
                serial,
                connect,
            } => {
                let resolved = resolve_adb_path(adb.as_deref()).map_err(ActingctlError::device)?;
                if let Some(warning) = resolved.warning {
                    eprintln!("WARNING actingctl: {warning}");
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
                        connect,
                    },
                )))
            }
            Self::SealedFrame(path) => Ok(Box::new(SealedFrameCapture {
                path,
                consumed: false,
            })),
        }
    }
}

impl Invocation {
    fn parse(arguments: Vec<OsString>) -> Result<Self, ActingctlError> {
        let Some(command) = arguments.first().and_then(|value| value.to_str()) else {
            return Err(ActingctlError::Usage);
        };
        let mut state_root = None;
        let mut instance = None;
        let mut adb = None;
        let mut serial = None;
        let mut sealed_frame = None;
        let mut no_connect = false;
        let mut sealed_test = false;
        let mut index = 1;
        while index < arguments.len() {
            let flag = arguments[index].to_str().ok_or(ActingctlError::Usage)?;
            match flag {
                "--state-root" => {
                    state_root = Some(PathBuf::from(require_value(&arguments, &mut index)?));
                }
                "--instance" => {
                    instance = Some(require_text(&arguments, &mut index)?);
                }
                "--adb" => adb = Some(require_text(&arguments, &mut index)?),
                "--serial" => serial = Some(require_text(&arguments, &mut index)?),
                "--sealed-frame" => {
                    sealed_frame = Some(PathBuf::from(require_value(&arguments, &mut index)?));
                }
                "--no-connect" => no_connect = true,
                "--sealed-test" => sealed_test = true,
                _ => return Err(ActingctlError::Usage),
            }
            index += 1;
        }
        let state_root = state_root.ok_or(ActingctlError::Usage)?;
        let instance = instance
            .filter(|value: &String| !value.trim().is_empty())
            .ok_or(ActingctlError::Usage)?;
        let command = match command {
            "reset"
                if adb.is_none() && serial.is_none() && sealed_frame.is_none() && !sealed_test =>
            {
                Command::Reset
            }
            "observe" => {
                let mode = match (sealed_frame, sealed_test, serial) {
                    (Some(path), true, None) if adb.is_none() && !no_connect => {
                        ObserveMode::SealedFrame(path)
                    }
                    (None, false, Some(serial)) => ObserveMode::Adb {
                        adb,
                        serial,
                        connect: !no_connect,
                    },
                    _ => return Err(ActingctlError::Usage),
                };
                Command::Observe(mode)
            }
            _ => return Err(ActingctlError::Usage),
        };
        Ok(Self {
            state_root,
            instance,
            command,
        })
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

fn require_value(arguments: &[OsString], index: &mut usize) -> Result<OsString, ActingctlError> {
    *index += 1;
    arguments.get(*index).cloned().ok_or(ActingctlError::Usage)
}

fn require_text(arguments: &[OsString], index: &mut usize) -> Result<String, ActingctlError> {
    require_value(arguments, index)?
        .into_string()
        .map_err(|_| ActingctlError::Usage)
}

#[derive(Debug)]
enum ActingctlError {
    Usage,
    Runtime(actingcommand_runtime_client::RuntimeClientError),
    Device(DeviceError),
    Output,
}

impl ActingctlError {
    fn runtime(error: actingcommand_runtime_client::RuntimeClientError) -> Self {
        Self::Runtime(error)
    }

    fn device(error: DeviceError) -> Self {
        Self::Device(error)
    }
}

impl fmt::Display for ActingctlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage => formatter.write_str(
                "usage: actingctl <observe|reset> --state-root <path> --instance <id> [observe: --serial <serial> [--adb <path>] [--no-connect] | --sealed-test --sealed-frame <png>]",
            ),
            Self::Runtime(error) => error.fmt(formatter),
            Self::Device(error) => error.fmt(formatter),
            Self::Output => formatter.write_str("failed to write JSON output"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_observation_requires_explicit_test_marker() {
        let args = |marked| {
            let mut args = vec![
                "observe",
                "--state-root",
                "state",
                "--instance",
                "ak.cn",
                "--sealed-frame",
                "frame.png",
            ];
            if marked {
                args.push("--sealed-test");
            }
            args.into_iter().map(OsString::from).collect()
        };
        assert!(Invocation::parse(args(false)).is_err());
        assert!(Invocation::parse(args(true)).is_ok());
    }

    #[test]
    fn reset_rejects_capture_configuration() {
        let args = [
            "reset",
            "--state-root",
            "state",
            "--instance",
            "ak.cn",
            "--serial",
            "127.0.0.1:16416",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert!(Invocation::parse(args).is_err());
    }
}
