// SPDX-License-Identifier: AGPL-3.0-only

//! Thin production CLI for correlation-scoped Runtime flows.

#![forbid(unsafe_code)]

mod shutdown_wait;

use actingcommand_contract::{
    CONFIG_PARAMETERS_FACT_KEY, CONFIG_SUBSYSTEMS_FACT_KEY, CaptureSequenceSpec,
    ContainedTaskRecoveryBinding, ContainedTaskRequest, EmulatorInstanceAction, EventActor,
    EventSource, RuntimeMonitorPolicy,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::Value;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

/// Upper bound for `request-shutdown --wait <seconds>`.
const MAX_SHUTDOWN_WAIT_SECONDS: u64 = 3600;

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(output) => match write_output(&output) {
            Ok(()) => {
                if output
                    .get("receipt")
                    .and_then(|receipt| receipt.get("error"))
                    .is_some_and(|error| !error.is_null())
                {
                    ExitCode::FAILURE
                } else {
                    ExitCode::SUCCESS
                }
            }
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
        shutdown_wait,
        command,
    } = Invocation::parse(arguments)?;
    let observation = if let Command::AgentPublishFacts { record_file } = &command {
        let mut bytes = Vec::new();
        std::fs::File::open(record_file)
            .map_err(|_| ActingctlError::FactRecord)?
            .take(actingcommand_contract::MAX_FACT_OBSERVATION_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ActingctlError::FactRecord)?;
        if bytes.len() > actingcommand_contract::MAX_FACT_OBSERVATION_BYTES {
            return Err(ActingctlError::FactRecord);
        }
        let observation: actingcommand_contract::FactObservation =
            serde_json::from_slice(&bytes).map_err(|_| ActingctlError::FactRecord)?;
        observation
            .validate()
            .map_err(|_| ActingctlError::FactRecord)?;
        Some(observation)
    } else {
        None
    };
    let (actor, source) = command.origin();
    let client = RuntimeClient::connect(RuntimeClientConfig::new(&state_root, actor, source))
        .map_err(ActingctlError::runtime)?;
    let instance = || instance.as_deref().ok_or(ActingctlError::Usage);
    let output = match command {
        Command::AgentPublishFacts { .. } => Ok(serde_json::json!({
            "event_id": client.publish_facts(observation.ok_or(ActingctlError::FactRecord)?).map_err(ActingctlError::runtime)?,
        })),
        Command::RequestShutdown => match shutdown_wait {
            None => Ok(serde_json::json!({
                "receipt": client.request_shutdown().map_err(ActingctlError::runtime)?,
            })),
            Some(wait) => {
                let receipt = client.request_shutdown().map_err(ActingctlError::runtime)?;
                // request_shutdown verified that the accepted receipt names exactly this target.
                let target = client.runtime_info().shutdown_target();
                // Close the connection so the waiting CLI leaves nothing for the Runtime to drain.
                drop(client);
                let shutdown = shutdown_wait::wait_for_shutdown(&state_root, &target, wait)?;
                Ok(serde_json::json!({ "receipt": receipt, "shutdown": shutdown }))
            }
        },
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
        // The same program-fact read as `facts --program`, reduced to the two records the
        // daemon's configuration manifest produced; no new operation.
        Command::StatusConfig => {
            let snapshot = client
                .runtime_fact_snapshot()
                .map_err(ActingctlError::runtime)?;
            let records = [CONFIG_SUBSYSTEMS_FACT_KEY, CONFIG_PARAMETERS_FACT_KEY]
                .into_iter()
                .map(|key| {
                    snapshot
                        .records
                        .iter()
                        .find(|record| record.key == key)
                        .ok_or(ActingctlError::ConfigFactsMissing)
                })
                .collect::<Result<Vec<_>, _>>()?;
            serde_json::to_value(records)
        }
        Command::ProgramFacts => serde_json::to_value(
            client
                .runtime_fact_snapshot()
                .map_err(ActingctlError::runtime)?,
        ),
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
        // Served by the existing status read, filtered to the one alias; no new operation.
        Command::EmulatorStatus => {
            let alias = instance()?;
            let status = client.status().map_err(ActingctlError::runtime)?;
            let instance = status
                .instances()
                .iter()
                .find(|instance| instance.instance_alias() == alias)
                .ok_or(ActingctlError::InstanceUnknown)?;
            serde_json::to_value(instance)
        }
        Command::EmulatorControl { action } => serde_json::to_value(
            client
                .control_emulator_instance(instance()?, action)
                .map_err(ActingctlError::runtime)?,
        ),
        Command::EmulatorDiscover => serde_json::to_value(
            client
                .discover_instances()
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
            recovery_package,
            recovery_expected_sha256,
        } => {
            let package = std::fs::canonicalize(package).map_err(|_| ActingctlError::Package)?;
            let recovery = match (recovery_package, recovery_expected_sha256) {
                (Some(package), Some(expected_sha256)) => Some((
                    std::fs::canonicalize(package)
                        .map_err(|_| ActingctlError::Package)?
                        .display()
                        .to_string(),
                    expected_sha256,
                )),
                (None, None) => None,
                _ => return Err(ActingctlError::Usage),
            };
            let request =
                task_run_request(package.display().to_string(), expected_sha256, recovery)?;
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

fn task_run_request(
    package_path: String,
    expected_sha256: String,
    recovery: Option<(String, String)>,
) -> Result<ContainedTaskRequest, ActingctlError> {
    let expected = actingcommand_contract::PackageRef::parse_argument(&expected_sha256)
        .map_err(|_| ActingctlError::Usage)?;
    ContainedTaskRequest::new(package_path, expected)
        .and_then(|request| match recovery {
            Some((package_path, expected_sha256)) => {
                request.with_recovery(ContainedTaskRecoveryBinding::new(
                    package_path,
                    actingcommand_contract::PackageRef::parse_argument(&expected_sha256)?,
                )?)
            }
            None => Ok(request),
        })
        .and_then(|request| {
            request.with_response_deadline_ms(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS)
        })
        .map_err(|_| ActingctlError::Usage)
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
    shutdown_wait: Option<Duration>,
    command: Command,
}

enum Command {
    AgentPublishFacts {
        record_file: PathBuf,
    },
    RequestShutdown,
    Observe,
    Reset,
    Status,
    /// `status --config`: the `config.*` records of the runtime fact snapshot.
    StatusConfig,
    ProgramFacts,
    MonitorStatus,
    MonitorSet {
        policy: RuntimeMonitorPolicy,
    },
    MonitorClear,
    EmulatorStatus,
    EmulatorControl {
        action: EmulatorInstanceAction,
    },
    EmulatorDiscover,
    Stream {
        spec: CaptureSequenceSpec,
    },
    TaskRun {
        package: PathBuf,
        expected_sha256: String,
        recovery_package: Option<PathBuf>,
        recovery_expected_sha256: Option<String>,
    },
}

impl Invocation {
    fn parse(arguments: Vec<OsString>) -> Result<Self, ActingctlError> {
        let Some(command) = arguments.first().and_then(|value| value.to_str()) else {
            return Err(ActingctlError::Usage);
        };
        // `emulator` takes its action as the second token; flags follow it.
        let emulator_action = if command == "emulator" {
            Some(
                arguments
                    .get(1)
                    .and_then(|value| value.to_str())
                    .ok_or(ActingctlError::Usage)?,
            )
        } else {
            None
        };
        let mut state_root = None;
        let mut instance = None;
        let mut interval_ms = None;
        let mut expected_page = None;
        let mut frame_count = None;
        let mut package = None;
        let mut expected_sha256 = None;
        let mut recovery_package = None;
        let mut recovery_expected_sha256 = None;
        let mut recovery_enabled = false;
        let mut program = false;
        let mut config = false;
        let mut record_file = None;
        let mut shutdown_wait = None;
        let mut index = if emulator_action.is_some() { 2 } else { 1 };
        while index < arguments.len() {
            let flag = arguments[index].to_str().ok_or(ActingctlError::Usage)?;
            match flag {
                "--record-file" if command == "agent-publish-facts" && record_file.is_none() => {
                    record_file = Some(PathBuf::from(require_value(&arguments, &mut index)?));
                }
                "--wait" if command == "request-shutdown" && shutdown_wait.is_none() => {
                    let seconds = require_u64(&arguments, &mut index)?;
                    if !(1..=MAX_SHUTDOWN_WAIT_SECONDS).contains(&seconds) {
                        return Err(ActingctlError::Usage);
                    }
                    shutdown_wait = Some(Duration::from_secs(seconds));
                }
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
                "--expected-sha256" | "--package-ref" => {
                    if expected_sha256.is_some() {
                        return Err(ActingctlError::Usage);
                    }
                    expected_sha256 = Some(require_text(&arguments, &mut index)?);
                }
                "--recovery-package" => {
                    recovery_package = Some(PathBuf::from(require_value(&arguments, &mut index)?));
                }
                "--recovery-expected-sha256" | "--recovery-package-ref" => {
                    if recovery_expected_sha256.is_some() {
                        return Err(ActingctlError::Usage);
                    }
                    recovery_expected_sha256 = Some(require_text(&arguments, &mut index)?);
                }
                "--recover" => recovery_enabled = true,
                "--program" => program = true,
                "--config" if command == "status" && !config => config = true,
                _ => return Err(ActingctlError::Usage),
            }
            index += 1;
        }
        let state_root = state_root.ok_or(ActingctlError::Usage)?;
        let instance = instance.filter(|value: &String| !value.trim().is_empty());
        let command = match command {
            "agent-publish-facts" => {
                if arguments
                    .iter()
                    .skip(1)
                    .filter_map(|argument| argument.to_str())
                    .any(|argument| {
                        argument.starts_with("--")
                            && !matches!(argument, "--state-root" | "--record-file")
                    })
                {
                    return Err(ActingctlError::Usage);
                }
                Command::AgentPublishFacts {
                    record_file: record_file.ok_or(ActingctlError::Usage)?,
                }
            }
            "request-shutdown" => Command::RequestShutdown,
            "reset" => Command::Reset,
            "observe" => Command::Observe,
            "status" if config => Command::StatusConfig,
            "status" => Command::Status,
            "facts" => {
                // The per-instance read is not built; only the program store is readable.
                if !program {
                    return Err(ActingctlError::Usage);
                }
                Command::ProgramFacts
            }
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
            "emulator" => match emulator_action {
                Some("status") => Command::EmulatorStatus,
                Some("start") => Command::EmulatorControl {
                    action: EmulatorInstanceAction::Start,
                },
                Some("stop") => Command::EmulatorControl {
                    action: EmulatorInstanceAction::Stop,
                },
                Some("restart") => Command::EmulatorControl {
                    action: EmulatorInstanceAction::Restart,
                },
                Some("discover") => Command::EmulatorDiscover,
                _ => return Err(ActingctlError::Usage),
            },
            "stream" => Command::Stream {
                spec: CaptureSequenceSpec::new(
                    frame_count.unwrap_or(1),
                    interval_ms.unwrap_or(250),
                )
                .map_err(|_| ActingctlError::Usage)?,
            },
            "task-run" => {
                if recovery_package.is_some() != recovery_expected_sha256.is_some() {
                    return Err(ActingctlError::Usage);
                }
                Command::TaskRun {
                    package: package.ok_or(ActingctlError::Usage)?,
                    expected_sha256: expected_sha256.ok_or(ActingctlError::Usage)?,
                    recovery_package,
                    recovery_expected_sha256,
                }
            }
            _ => return Err(ActingctlError::Usage),
        };
        if command.requires_instance() != instance.is_some() {
            return Err(ActingctlError::Usage);
        }
        Ok(Self {
            state_root,
            instance,
            shutdown_wait,
            command,
        })
    }
}

impl Command {
    const fn origin(&self) -> (EventActor, EventSource) {
        if matches!(self, Self::AgentPublishFacts { .. }) {
            (EventActor::Agent, EventSource::Adapter)
        } else {
            (EventActor::Cli, EventSource::Cli)
        }
    }

    const fn requires_instance(&self) -> bool {
        !matches!(
            self,
            Self::Status
                | Self::StatusConfig
                | Self::ProgramFacts
                | Self::MonitorStatus
                | Self::EmulatorDiscover
                | Self::RequestShutdown
                | Self::AgentPublishFacts { .. }
        )
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
    FactRecord,
    InstanceUnknown,
    /// `status --config`: the snapshot carries no `config.subsystems` / `config.parameters`.
    ConfigFactsMissing,
    Output,
    /// `request-shutdown --wait` failed after the shutdown was accepted; `detail` is JSON.
    ShutdownWait {
        code: &'static str,
        detail: String,
    },
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
                .write_str("usage: actingctl <observe|reset|status [--config]|facts|request-shutdown|monitor-status|monitor-set|monitor-clear|emulator <status|start|stop|restart|discover>|stream|task-run> --state-root <path> [--instance <id>] [--program] [--wait <seconds>] [--package <locator> (--expected-sha256 <hash>|--package-ref <json>) [--recovery-package <locator> (--recovery-expected-sha256 <hash>|--recovery-package-ref <json>)]]"),
            Self::Runtime(error) => error.fmt(formatter),
            Self::Package => formatter.write_str("failed to resolve contained task package"),
            Self::FactRecord => formatter.write_str("invalid or unreadable bounded fact observation file"),
            Self::InstanceUnknown => formatter.write_str("instance_unknown: the runtime status lists no instance with that alias"),
            Self::ConfigFactsMissing => formatter.write_str("config_facts_missing: the runtime fact snapshot holds no config.subsystems / config.parameters record"),
            Self::Output => formatter.write_str("failed to write JSON output"),
            Self::ShutdownWait { code, detail } => {
                write!(formatter, "{code} during wait_shutdown: {detail}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_uses_only_runtime_identity() {
        let args = ["observe", "--state-root", "state", "--instance", "node.a"]
            .into_iter()
            .map(OsString::from)
            .collect();
        assert!(Invocation::parse(args).is_ok());
        // LIVE-FACT-POOL-v1 specification; no device or process execution.
        let parsed = Invocation::parse(
            [
                "agent-publish-facts",
                "--state-root",
                "state",
                "--record-file",
                "observation.json",
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
        )
        .unwrap();
        assert_eq!(
            parsed.command.origin(),
            (EventActor::Agent, EventSource::Adapter)
        );
        assert_eq!(
            Command::Observe.origin(),
            (EventActor::Cli, EventSource::Cli)
        );
        for flag in ["--actor", "--instance", "--serial"] {
            assert!(
                Invocation::parse(
                    [
                        "agent-publish-facts",
                        "--state-root",
                        "state",
                        "--record-file",
                        "observation.json",
                        flag,
                        "arbitrary"
                    ]
                    .into_iter()
                    .map(OsString::from)
                    .collect()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn client_commands_reject_capture_configuration() {
        let args = [
            "observe",
            "--state-root",
            "state",
            "--instance",
            "node.a",
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
        let args = ["request-shutdown", "--state-root", "state"]
            .into_iter()
            .map(OsString::from)
            .collect();
        assert!(matches!(
            Invocation::parse(args).expect("shutdown command").command,
            Command::RequestShutdown
        ));
        let args = [
            "request-shutdown",
            "--state-root",
            "state",
            "--instance",
            "node.a",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert!(Invocation::parse(args).is_err());
    }

    #[test]
    fn monitor_and_stream_commands_build_closed_runtime_contracts() {
        let monitor = [
            "monitor-set",
            "--state-root",
            "state",
            "--instance",
            "node.a",
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
            "node.a",
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

    #[test]
    fn task_run_request_uses_maximum_bounded_response_deadline() {
        let request = task_run_request("neutral-task.zip".to_string(), "0".repeat(64), None)
            .expect("task-run request");

        assert_eq!(
            request.response_deadline_ms(),
            ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS
        );
        assert_eq!(request.response_deadline_ms(), 1_800_000);
    }

    // Test class: specification criterion. Task Contract: https://github.com/HS7097/ActingCommand-Workflow/issues/241#issuecomment-5491623342
    #[test]
    fn task_run_recovery_binding_requires_the_exact_paired_flags() {
        let complete = [
            "task-run",
            "--state-root",
            "state",
            "--instance",
            "neutral.instance",
            "--package",
            "neutral-task.zip",
            "--expected-sha256",
            "0",
            "--recovery-package",
            "return-home.zip",
            "--recovery-expected-sha256",
            "1",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert!(Invocation::parse(complete).is_ok());

        for incomplete in [
            vec!["--recovery-package", "return-home.zip"],
            vec!["--recovery-expected-sha256", "1"],
        ] {
            let mut args = vec![
                "task-run",
                "--state-root",
                "state",
                "--instance",
                "neutral.instance",
                "--package",
                "neutral-task.zip",
                "--expected-sha256",
                "0",
            ];
            args.extend(incomplete);
            assert!(Invocation::parse(args.into_iter().map(OsString::from).collect()).is_err());
        }
    }

    // Test class: specification criterion. Task Contract: https://github.com/HS7097/ActingCommand-Workflow/issues/241#issuecomment-5491623342
    #[test]
    fn task_run_request_preserves_the_typed_recovery_identity() {
        let request = task_run_request(
            "neutral-task.zip".to_string(),
            "0".repeat(64),
            Some(("return-home.zip".to_string(), "1".repeat(64))),
        )
        .expect("task-run request");
        let recovery = request.recovery().expect("typed recovery binding");

        assert_eq!(recovery.package_path(), "return-home.zip");
        assert_eq!(recovery.expected_sha256(), &"1".repeat(64).into());
    }
}
