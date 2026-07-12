// SPDX-License-Identifier: AGPL-3.0-only

//! Thin production CLI for correlation-scoped Runtime flows.

#![forbid(unsafe_code)]

use actingcommand_contract::{
    CaptureSequenceSpec, ContainedTaskRequest, EventActor, EventSource, RuntimeMonitorPolicy,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::Value;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

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

fn run(arguments: Vec<OsString>) -> Result<Value, ActingctlError> {
    let Invocation {
        state_root,
        instance,
        command,
    } = Invocation::parse(arguments)?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &state_root,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(ActingctlError::runtime)?;
    let instance = || instance.as_deref().ok_or(ActingctlError::Usage);
    let output = match command {
        Command::Reset => serde_json::to_value(
            client
                .safe_reset(instance()?)
                .map_err(ActingctlError::runtime)?,
        ),
        Command::Observe => serde_json::to_value(
            client
                .observe_readonly(instance()?)
                .map_err(ActingctlError::runtime)?,
        ),
        Command::Status => serde_json::to_value(client.status().map_err(ActingctlError::runtime)?),
        Command::MonitorStatus => {
            serde_json::to_value(client.monitor_status().map_err(ActingctlError::runtime)?)
        }
        Command::MonitorSet { policy } => serde_json::to_value(
            client
                .configure_monitor(instance()?, policy)
                .map_err(ActingctlError::runtime)?,
        ),
        Command::MonitorClear => serde_json::to_value(
            client
                .clear_monitor(instance()?)
                .map_err(ActingctlError::runtime)?,
        ),
        Command::Stream { spec } => serde_json::to_value(
            client
                .capture_sequence(instance()?, spec)
                .map_err(ActingctlError::runtime)?,
        ),
        Command::TaskRun {
            package,
            expected_sha256,
        } => {
            let package = std::fs::canonicalize(package).map_err(|_| ActingctlError::Package)?;
            let request = ContainedTaskRequest::new(package.display().to_string(), expected_sha256)
                .map_err(|_| ActingctlError::Usage)?;
            serde_json::to_value(
                client
                    .run_contained_task(instance()?, request)
                    .map_err(ActingctlError::runtime)?,
            )
        }
    }
    .map_err(|_| ActingctlError::Output)?;
    Ok(output)
}

fn write_output(output: &Value) -> Result<(), ActingctlError> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    serde_json::to_writer(&mut writer, output).map_err(|_| ActingctlError::Output)?;
    writer.write_all(b"\n").map_err(|_| ActingctlError::Output)
}

struct Invocation {
    state_root: PathBuf,
    instance: Option<String>,
    command: Command,
}

enum Command {
    Observe,
    Reset,
    Status,
    MonitorStatus,
    MonitorSet {
        policy: RuntimeMonitorPolicy,
    },
    MonitorClear,
    Stream {
        spec: CaptureSequenceSpec,
    },
    TaskRun {
        package: PathBuf,
        expected_sha256: String,
    },
}

impl Invocation {
    fn parse(arguments: Vec<OsString>) -> Result<Self, ActingctlError> {
        let Some(command) = arguments.first().and_then(|value| value.to_str()) else {
            return Err(ActingctlError::Usage);
        };
        let mut state_root = None;
        let mut instance = None;
        let mut interval_ms = None;
        let mut expected_page = None;
        let mut frame_count = None;
        let mut package = None;
        let mut expected_sha256 = None;
        let mut recovery_enabled = false;
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
                "--interval-ms" => {
                    interval_ms = Some(require_u64(&arguments, &mut index)?);
                }
                "--expect" => {
                    expected_page = Some(require_text(&arguments, &mut index)?);
                }
                "--max-frames" => {
                    frame_count = Some(require_u16(&arguments, &mut index)?);
                }
                "--package" => {
                    package = Some(PathBuf::from(require_value(&arguments, &mut index)?));
                }
                "--expected-sha256" => {
                    expected_sha256 = Some(require_text(&arguments, &mut index)?);
                }
                "--recover" => recovery_enabled = true,
                _ => return Err(ActingctlError::Usage),
            }
            index += 1;
        }
        let state_root = state_root.ok_or(ActingctlError::Usage)?;
        let instance = instance.filter(|value: &String| !value.trim().is_empty());
        let command = match command {
            "reset" => Command::Reset,
            "observe" => Command::Observe,
            "status" => Command::Status,
            "monitor-status" => Command::MonitorStatus,
            "monitor-set" => Command::MonitorSet {
                policy: RuntimeMonitorPolicy::new(
                    interval_ms.unwrap_or(30_000),
                    expected_page.unwrap_or_else(|| "home".to_string()),
                    recovery_enabled,
                )
                .map_err(|_| ActingctlError::Usage)?,
            },
            "monitor-clear" => Command::MonitorClear,
            "stream" => Command::Stream {
                spec: CaptureSequenceSpec::new(
                    frame_count.unwrap_or(1),
                    interval_ms.unwrap_or(250),
                )
                .map_err(|_| ActingctlError::Usage)?,
            },
            "task-run" => Command::TaskRun {
                package: package.ok_or(ActingctlError::Usage)?,
                expected_sha256: expected_sha256.ok_or(ActingctlError::Usage)?,
            },
            _ => return Err(ActingctlError::Usage),
        };
        if command.requires_instance() != instance.is_some() {
            return Err(ActingctlError::Usage);
        }
        Ok(Self {
            state_root,
            instance,
            command,
        })
    }
}

impl Command {
    const fn requires_instance(&self) -> bool {
        !matches!(self, Self::Status | Self::MonitorStatus)
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

fn require_u64(arguments: &[OsString], index: &mut usize) -> Result<u64, ActingctlError> {
    require_text(arguments, index)?
        .parse()
        .map_err(|_| ActingctlError::Usage)
}

fn require_u16(arguments: &[OsString], index: &mut usize) -> Result<u16, ActingctlError> {
    require_text(arguments, index)?
        .parse()
        .map_err(|_| ActingctlError::Usage)
}

#[derive(Debug)]
enum ActingctlError {
    Usage,
    Runtime(actingcommand_runtime_client::RuntimeClientError),
    Package,
    Output,
}

impl ActingctlError {
    fn runtime(error: actingcommand_runtime_client::RuntimeClientError) -> Self {
        Self::Runtime(error)
    }
}

impl fmt::Display for ActingctlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage => formatter
                .write_str("usage: actingctl <observe|reset|status|monitor-status|monitor-set|monitor-clear|stream|task-run> --state-root <path> [--instance <id>] [--package <zip> --expected-sha256 <hash>]"),
            Self::Runtime(error) => error.fmt(formatter),
            Self::Package => formatter.write_str("failed to resolve contained task package"),
            Self::Output => formatter.write_str("failed to write JSON output"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_uses_only_runtime_identity() {
        let args = ["observe", "--state-root", "state", "--instance", "ak.cn"]
            .into_iter()
            .map(OsString::from)
            .collect();
        assert!(Invocation::parse(args).is_ok());
    }

    #[test]
    fn client_commands_reject_capture_configuration() {
        let args = [
            "observe",
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

    #[test]
    fn status_does_not_require_an_instance() {
        let args = ["status", "--state-root", "state"]
            .into_iter()
            .map(OsString::from)
            .collect();
        assert!(Invocation::parse(args).is_ok());
    }

    #[test]
    fn monitor_and_stream_commands_build_closed_runtime_contracts() {
        let monitor = [
            "monitor-set",
            "--state-root",
            "state",
            "--instance",
            "ak.cn",
            "--interval-ms",
            "1000",
            "--expect",
            "home",
            "--recover",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert!(Invocation::parse(monitor).is_ok());

        let stream = [
            "stream",
            "--state-root",
            "state",
            "--instance",
            "ak.cn",
            "--max-frames",
            "60",
            "--interval-ms",
            "1000",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert!(Invocation::parse(stream).is_ok());
    }

    #[test]
    fn contained_task_command_requires_external_hash_and_runtime_instance_only() {
        let args = [
            "task-run",
            "--state-root",
            "state",
            "--instance",
            "neutral.instance",
            "--package",
            "neutral-task.zip",
            "--expected-sha256",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert!(Invocation::parse(args).is_ok());
    }
}
