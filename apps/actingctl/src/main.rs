// SPDX-License-Identifier: AGPL-3.0-only

//! Thin production CLI for correlation-scoped Runtime flows.

#![forbid(unsafe_code)]

// Test-only: Workflow #381 A, the install-transition run budget (test plan A-2 G2a).
#[cfg(test)]
mod gate_381a;
#[cfg(feature = "mcp")]
mod mcp;
mod process_probe;
mod shutdown_wait;
#[cfg(windows)]
mod watchdog;

use actingcommand_contract::{
    CONFIG_PARAMETERS_FACT_KEY, CONFIG_SUBSYSTEMS_FACT_KEY, CaptureSequenceSpec,
    ContainedTaskRecoveryBinding, ContainedTaskRequest, EmulatorInstanceAction, EventActor,
    EventSource, FactObservation, FactRecord, FactScope, RuntimeMonitorPolicy,
    SchedulingPauseScope,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::Value;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Upper bound for `request-shutdown --wait <seconds>`.
const MAX_SHUTDOWN_WAIT_SECONDS: u64 = 3600;
/// `source_detector` of the priority offsets `task-offset` publishes.
const TASK_OFFSET_DETECTOR: &str = "actingctl.task-offset";
/// `pause` defaults (Workflow #191 ps1): the reason code and the instance drain timeout.
const DEFAULT_PAUSE_REASON: &str = "operator";
const DEFAULT_PAUSE_DRAIN_TIMEOUT_MS: u64 = 60_000;

fn main() -> ExitCode {
    if let Err(error) = actingcommand_contract::process_installation() {
        eprintln!("FATAL actingctl: {error}");
        return ExitCode::FAILURE;
    }
    let arguments: Vec<OsString> = env::args_os().skip(1).collect();
    // Workflow #338: `mcp-serve` and `mcp-config` take their own flags, so they leave before
    // `Invocation::parse`, which requires `--state-root`.
    #[cfg(feature = "mcp")]
    {
        if let Some(exit) = mcp::dispatch(&arguments) {
            return exit;
        }
    }
    // Workflow #374: `watchdog` takes `--root`, not `--state-root`.
    #[cfg(windows)]
    {
        if let Some(exit) = watchdog::dispatch(&arguments) {
            return exit;
        }
    }
    match run(arguments) {
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

/// Workflow #381 A R1′, R2: how one `install-transition` run talks to the Runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InstallTransitionRun {
    /// The deadline of the whole run, counted from its start; no exchange waits past it.
    budget: Option<Duration>,
    /// The I/O timeout of one exchange.
    exchange_timeout: Duration,
    /// Whether the run declares its governance identity before the action.
    declares_identity: bool,
}

fn install_transition_run(
    _action: &actingcommand_contract::InstallTransitionAction,
) -> InstallTransitionRun {
    InstallTransitionRun {
        budget: None,
        exchange_timeout: Duration::from_secs(5),
        declares_identity: true,
    }
}

fn run(arguments: Vec<OsString>) -> Result<Value, ActingctlError> {
    let started = Instant::now();
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
    // Workflow #308 RT-S1a: the policy document is read bounded and checked as UTF-8 only;
    // the Runtime parses and checks its content.
    let resource_targets = if let Command::AgentApplyResourceTargets { policy_file } = &command {
        let mut bytes = Vec::new();
        std::fs::File::open(policy_file)
            .map_err(|_| ActingctlError::ResourceTargetsFile)?
            .take(actingcommand_contract::MAX_RESOURCE_TARGETS_DOCUMENT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ActingctlError::ResourceTargetsFile)?;
        if bytes.is_empty()
            || bytes.len() > actingcommand_contract::MAX_RESOURCE_TARGETS_DOCUMENT_BYTES
        {
            return Err(ActingctlError::ResourceTargetsFile);
        }
        Some(String::from_utf8(bytes).map_err(|_| ActingctlError::ResourceTargetsFile)?)
    } else {
        None
    };
    let (actor, source) = command.origin();
    let install_run = match &command {
        Command::InstallTransition { action } => Some(install_transition_run(action)),
        _ => None,
    };
    let mut client_config = RuntimeClientConfig::new(&state_root, actor, source);
    if let Some(install_run) = install_run {
        client_config = client_config.with_io_timeout(install_run.exchange_timeout);
        if let Some(budget) = install_run.budget {
            client_config = client_config.with_deadline(started + budget);
        }
    }
    let client = RuntimeClient::connect(client_config).map_err(ActingctlError::runtime)?;
    let optional_instance = instance.clone();
    let instance = || instance.as_deref().ok_or(ActingctlError::Usage);
    let output = match command {
        Command::AgentPublishFacts { .. } => Ok(serde_json::json!({
            "event_id": client.publish_facts(observation.ok_or(ActingctlError::FactRecord)?).map_err(ActingctlError::runtime)?,
        })),
        // A refused policy prints the Runtime's receipt, whose error exits non-zero.
        Command::AgentApplyResourceTargets { .. } => match client
            .apply_resource_targets(resource_targets.ok_or(ActingctlError::ResourceTargetsFile)?)
        {
            Ok(applied) => Ok(serde_json::json!({ "applied": applied })),
            Err(error) => match error.received_receipt() {
                Some(receipt) => Ok(serde_json::json!({ "receipt": receipt })),
                None => return Err(ActingctlError::runtime(error)),
            },
        },
        // One manual priority offset (Workflow #308 slice 4a-2) through the ordinary fact
        // publication, with this CLI's Cli/Cli origin.
        Command::TaskOffset {
            task_id,
            offset_milli,
        } => {
            let status = client.status().map_err(ActingctlError::runtime)?;
            let scope = match optional_instance.as_deref() {
                Some(alias) => {
                    if !status
                        .instances()
                        .iter()
                        .any(|registered| registered.instance_alias() == alias)
                    {
                        return Err(ActingctlError::InstanceUnknown);
                    }
                    FactScope::Instance {
                        instance_id: alias.to_owned(),
                    }
                }
                // A task-level offset applies to every instance of the configured game; the
                // registered instances must name exactly one.
                None => {
                    let games = status
                        .instances()
                        .iter()
                        .map(|registered| registered.game_id())
                        .collect::<Vec<_>>();
                    match games.first() {
                        Some(Some(game_id)) if games.iter().all(|game| *game == Some(*game_id)) => {
                            FactScope::Game {
                                game_id: (*game_id).to_owned(),
                            }
                        }
                        _ => return Err(ActingctlError::TaskOffsetScopeAmbiguous),
                    }
                }
            };
            let observed_at_unix_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
                .filter(|millis| *millis > 0)
                .ok_or(ActingctlError::Clock)?;
            let record = FactRecord::priority_offset(
                scope,
                &task_id,
                offset_milli,
                observed_at_unix_ms,
                TASK_OFFSET_DETECTOR,
            )
            .map_err(|_| ActingctlError::PriorityOffsetInvalid)?;
            let (scope, key) = (record.scope.clone(), record.key.clone());
            let event_id = client
                .publish_facts(FactObservation {
                    records: vec![record],
                })
                .map_err(ActingctlError::runtime)?;
            Ok(serde_json::json!({
                "event_id": event_id,
                "scope": scope,
                "key": key,
                "offset_milli": offset_milli,
            }))
        }
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
        Command::InstallTransition { action } => {
            if install_run.is_some_and(|install_run| install_run.declares_identity) {
                client.declare_governance_identity(&actingcommand_contract::GovernanceIdentityCard {
                    client: "actingctl".to_owned(), client_version: Some(env!("CARGO_PKG_VERSION").to_owned()), instance: None,
                }).map_err(ActingctlError::runtime)?;
            }
            let receipt = client.install_transition(action).map_err(ActingctlError::runtime)?;
            match shutdown_wait {
                Some(wait) => {
                    let target = client.runtime_info().shutdown_target();
                    drop(client);
                    let shutdown = shutdown_wait::wait_for_shutdown(&state_root, &target, wait)?;
                    Ok(serde_json::json!({ "receipt": receipt, "shutdown": shutdown }))
                }
                None => Ok(serde_json::json!({ "receipt": receipt })),
            }
        }
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
        // Workflow #191 ps1: without --instance the pause / resume is global.
        Command::Pause {
            reason_code,
            drain_timeout_ms,
        } => serde_json::to_value(
            client
                .pause_scheduling(
                    pause_scope(optional_instance),
                    &reason_code,
                    drain_timeout_ms,
                )
                .map_err(ActingctlError::runtime)?,
        ),
        Command::Resume => serde_json::to_value(
            client
                .resume_scheduling(pause_scope(optional_instance))
                .map_err(ActingctlError::runtime)?,
        ),
        // Workflow #317 sc3 (d): manual reconnect and self-check of one physical instance.
        Command::SelfCheck { instance_alias } => serde_json::to_value(
            client
                .self_check_instance(&instance_alias)
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

fn pause_scope(instance: Option<String>) -> SchedulingPauseScope {
    match instance {
        Some(instance_alias) => SchedulingPauseScope::Instance { instance_alias },
        None => SchedulingPauseScope::Global,
    }
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
    /// `agent-apply-resource-targets --policy-file <path>`: one resource target policy
    /// through the Runtime's formal entry (Workflow #308 RT-S1a).
    AgentApplyResourceTargets {
        policy_file: PathBuf,
    },
    /// `task-offset <task_id> <offset_milli> [--instance <alias>]`.
    TaskOffset {
        task_id: String,
        offset_milli: i64,
    },
    RequestShutdown,
    InstallTransition {
        action: actingcommand_contract::InstallTransitionAction,
    },
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
    /// `pause [--instance <alias>] [--reason <code>] [--drain-timeout-ms <n>]`.
    Pause {
        reason_code: String,
        drain_timeout_ms: u64,
    },
    /// `resume [--instance <alias>]`.
    Resume,
    /// `selfcheck <alias>`: reconnect and self-check one physical instance now.
    SelfCheck {
        instance_alias: String,
    },
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
        let mut policy_file = None;
        let mut shutdown_wait = None;
        let mut install_action = None;
        let mut pause_reason = None;
        let mut drain_timeout_ms = None;
        // `task-offset` takes the task and the offset as the second and third tokens.
        let task_offset = if command == "task-offset" {
            let task_id = arguments
                .get(1)
                .and_then(|value| value.to_str())
                .filter(|value| !value.starts_with("--"))
                .ok_or(ActingctlError::Usage)?;
            let offset_milli = arguments
                .get(2)
                .and_then(|value| value.to_str())
                .and_then(|value| value.parse::<i64>().ok())
                .ok_or(ActingctlError::Usage)?;
            Some((task_id.to_owned(), offset_milli))
        } else {
            None
        };
        // `selfcheck` takes the instance alias as the second token.
        let selfcheck_alias = if command == "selfcheck" {
            Some(
                arguments
                    .get(1)
                    .and_then(|value| value.to_str())
                    .filter(|value| !value.starts_with("--") && !value.trim().is_empty())
                    .ok_or(ActingctlError::Usage)?
                    .to_owned(),
            )
        } else {
            None
        };
        let mut index = if emulator_action.is_some() || selfcheck_alias.is_some() {
            2
        } else if task_offset.is_some() {
            3
        } else {
            1
        };
        while index < arguments.len() {
            let flag = arguments[index].to_str().ok_or(ActingctlError::Usage)?;
            match flag {
                "--record-file" if command == "agent-publish-facts" && record_file.is_none() => {
                    record_file = Some(PathBuf::from(require_value(&arguments, &mut index)?));
                }
                "--policy-file"
                    if command == "agent-apply-resource-targets" && policy_file.is_none() =>
                {
                    policy_file = Some(PathBuf::from(require_value(&arguments, &mut index)?));
                }
                "--action-json" if command == "install-transition" && install_action.is_none() => {
                    let json = require_text(&arguments, &mut index)?;
                    if json.len() > 4096 {
                        return Err(ActingctlError::Usage);
                    }
                    let action: actingcommand_contract::InstallTransitionAction =
                        serde_json::from_str(&json).map_err(|_| ActingctlError::Usage)?;
                    action.validate().map_err(|_| ActingctlError::Usage)?;
                    install_action = Some(action);
                }
                "--wait"
                    if matches!(command, "request-shutdown" | "install-transition")
                        && shutdown_wait.is_none() =>
                {
                    let seconds = require_u64(&arguments, &mut index)?;
                    if !(1..=MAX_SHUTDOWN_WAIT_SECONDS).contains(&seconds) {
                        return Err(ActingctlError::Usage);
                    }
                    shutdown_wait = Some(Duration::from_secs(seconds));
                }
                "--reason" if command == "pause" && pause_reason.is_none() => {
                    let reason = require_text(&arguments, &mut index)?;
                    actingcommand_contract::validate_scheduling_pause_reason(&reason)
                        .map_err(|_| ActingctlError::Usage)?;
                    pause_reason = Some(reason);
                }
                "--drain-timeout-ms" if command == "pause" && drain_timeout_ms.is_none() => {
                    let timeout = require_u64(&arguments, &mut index)?;
                    if !(actingcommand_contract::MIN_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS
                        ..=actingcommand_contract::MAX_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS)
                        .contains(&timeout)
                    {
                        return Err(ActingctlError::Usage);
                    }
                    drain_timeout_ms = Some(timeout);
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
            "agent-apply-resource-targets" => {
                if arguments
                    .iter()
                    .skip(1)
                    .filter_map(|argument| argument.to_str())
                    .any(|argument| {
                        argument.starts_with("--")
                            && !matches!(argument, "--state-root" | "--policy-file")
                    })
                {
                    return Err(ActingctlError::Usage);
                }
                Command::AgentApplyResourceTargets {
                    policy_file: policy_file.ok_or(ActingctlError::Usage)?,
                }
            }
            "task-offset" => {
                if arguments
                    .iter()
                    .skip(3)
                    .filter_map(|argument| argument.to_str())
                    .any(|argument| {
                        argument.starts_with("--")
                            && !matches!(argument, "--state-root" | "--instance")
                    })
                {
                    return Err(ActingctlError::Usage);
                }
                let (task_id, offset_milli) = task_offset.ok_or(ActingctlError::Usage)?;
                Command::TaskOffset {
                    task_id,
                    offset_milli,
                }
            }
            "request-shutdown" => Command::RequestShutdown,
            "install-transition" => {
                let action = install_action.ok_or(ActingctlError::Usage)?;
                if shutdown_wait.is_some()
                    && !matches!(
                        action,
                        actingcommand_contract::InstallTransitionAction::CommitShutdown { .. }
                    )
                {
                    return Err(ActingctlError::Usage);
                }
                Command::InstallTransition { action }
            }
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
            "pause" => Command::Pause {
                reason_code: pause_reason.unwrap_or_else(|| DEFAULT_PAUSE_REASON.to_owned()),
                drain_timeout_ms: drain_timeout_ms.unwrap_or(DEFAULT_PAUSE_DRAIN_TIMEOUT_MS),
            },
            "resume" => Command::Resume,
            "selfcheck" => Command::SelfCheck {
                instance_alias: selfcheck_alias.ok_or(ActingctlError::Usage)?,
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
        // `task-offset` takes `--instance` optionally: without it the offset is task-level;
        // `pause` / `resume` without it are global.
        if !matches!(
            command,
            Command::TaskOffset { .. } | Command::Pause { .. } | Command::Resume
        ) && command.requires_instance() != instance.is_some()
        {
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
        if matches!(
            self,
            Self::AgentPublishFacts { .. } | Self::AgentApplyResourceTargets { .. }
        ) {
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
                | Self::InstallTransition { .. }
                | Self::AgentPublishFacts { .. }
                | Self::AgentApplyResourceTargets { .. }
                | Self::SelfCheck { .. }
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
    /// `agent-apply-resource-targets`: the policy file is unreadable, empty, larger than the
    /// document bound or not UTF-8.
    ResourceTargetsFile,
    InstanceUnknown,
    /// `task-offset` without `--instance`: the registered instances do not name exactly one
    /// configured game.
    TaskOffsetScopeAmbiguous,
    /// `task-offset`: the task identifier cannot form a valid priority offset record.
    PriorityOffsetInvalid,
    /// The system clock is before the Unix epoch or out of range.
    Clock,
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
                .write_str("usage: actingctl <observe|reset|status [--config]|facts|request-shutdown|install-transition --action-json <json>|monitor-status|monitor-set|monitor-clear|emulator <status|start|stop|restart|discover>|stream|task-run|task-offset <task_id> <offset_milli>|pause [--reason <code>] [--drain-timeout-ms <n>]|resume|selfcheck <alias>> --state-root <path> [--instance <id>] [--program] [--wait <seconds>] [--package <locator> (--expected-sha256 <hash>|--package-ref <json>) [--recovery-package <locator> (--recovery-expected-sha256 <hash>|--recovery-package-ref <json>)]]\nusage: actingctl watchdog <status|run-once [--from-task]|install|uninstall> --root <install root>"),
            Self::Runtime(error) => error.fmt(formatter),
            Self::Package => formatter.write_str("failed to resolve contained task package"),
            Self::FactRecord => formatter.write_str("invalid or unreadable bounded fact observation file"),
            Self::ResourceTargetsFile => formatter.write_str("resource_targets_file_invalid: the policy file is unreadable, empty, larger than 65536 bytes or not UTF-8"),
            Self::InstanceUnknown => formatter.write_str("instance_unknown: the runtime status lists no instance with that alias"),
            Self::TaskOffsetScopeAmbiguous => formatter.write_str("task_offset_scope_ambiguous: the registered instances do not name exactly one configured game; pass --instance <alias>"),
            Self::PriorityOffsetInvalid => formatter.write_str("priority_offset_invalid: the task identifier cannot form a priority offset fact"),
            Self::Clock => formatter.write_str("clock_unavailable: the system clock is before the Unix epoch or out of range"),
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
