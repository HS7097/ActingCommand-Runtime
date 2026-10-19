// SPDX-License-Identifier: AGPL-3.0-only

//! Thin production CLI for correlation-scoped Runtime flows.

#![forbid(unsafe_code)]

// Workflow #378 A5: the one result line of every exit.
mod cli_line;
// Workflow #389 MCP-14: `codes` (MCP `ac_outcome_code`), entries of the embedded outcome catalog.
mod code_lookup;
// Test-only: Workflow #381 A, the install-transition run budget (test plan A-2 G2a).
mod codes;
mod events;
// Workflow #389 MCP-4: `facts --program | --instance` and `status --config` (MCP `ac_facts`).
mod facts;
#[cfg(test)]
mod gate_381a;
// Test-only: Workflow #382 A-1 (test plan v2.2 §3.7), the CLI grammar (G10) and
// `request-shutdown --wait` (G11), in-process.
#[cfg(test)]
mod gate_382_a1;
// Test-only: Workflow #381 A, the held Runtime start and the provider without instances, mounted
// once for the whole binary (watchdog::gate_381a, mcp::tests::gate_387_d1b).
#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/held_runtime.rs"]
mod held_runtime;
#[cfg(feature = "mcp")]
mod mcp;
mod packs;
mod process_probe;
// Workflow #389 MCP-10: `start` and `restart`.
#[cfg(windows)]
mod runtime_start;
mod shutdown_wait;
#[cfg(windows)]
mod watchdog;

// Workflow #389 MCP-5a: `observe --export` (MCP `ac_observe`), with the instance selector and the
// material export writer it shares with MCP.
mod instance_selector;
mod material_export;
mod observe_export;
// End of MCP-5a.

// Workflow #389 MCP-3: `runs` (MCP `ac_runs`); it uses MCP-5a's instance selector above.
mod run_list;
// End of MCP-3.

// ---- Workflow #389 MCP-7 (begin): `diagnose` (MCP `ac_diagnose`), with the child-process
// runner it shares with MCP for `actingd suspended`. ----
mod child;
mod diagnose;
// ---- Workflow #389 MCP-7 (end) ----

// ---- Workflow #389 MCP-8 (begin): `overview` (MCP `ac_overview`), with the read both share.
mod overview;
// ---- Workflow #389 MCP-8 (end) ----

// ---- Workflow #389 MCP-6 (begin): `run evidence` (MCP `ac_run_evidence`).
mod run_evidence;
// ---- Workflow #389 MCP-6 (end) ----

// Workflow #390 DRILL-1: `ladder-drill` (MCP `ac_ladder_drill`), a drill of an instance's ladder.
mod ladder_drill;

// ---- Workflow #398 RD-1c (begin): `devices list|bind|unbind` (MCP `ac_devices`,
// `ac_device_bind`, `ac_device_unbind`).
mod devices;
// ---- Workflow #398 RD-1c (end) ----

// Workflow #410 item 6: `capability-gap` (MCP `ac_capability_gap`), the local capability-gap log.
mod capability_gap;

// Workflow #411 B15 R6: `remedy set|get|lift` (MCP `ac_remedy_set`, `ac_remedy_get`), an
// instance's remedy choices.
mod remedy;

use actingcommand_contract::codes::{ContractCode, ContractLocation, SharedCode};
use actingcommand_contract::outcome::{Category, Detail, IoOp, Outcome, RawSource};
use actingcommand_contract::{
    CaptureSequenceSpec, ConfigSettingChange, ContainedTaskRecoveryBinding, ContainedTaskRequest,
    CorrelationId, EmulatorInstanceAction, EventActor, EventSource, FactObservation, FactRecord,
    FactScope, InstallTransitionAction, InstallTransitionPhase, MAX_CONFIG_SETTING_CHANGES,
    RequestId, RuntimeMonitorPolicy, RuntimeReceipt, RuntimeReceiptState, RuntimeResult,
    SanitizationError, SchedulingPauseScope,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use codes::{ActingctlCode, ArgumentProblem};
use instance_selector::SelectorMiss;
use serde::Serialize;
use serde_json::{Map, Value};
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The developer text of `actingctl_usage_invalid`.
const USAGE: &str = "usage: actingctl <observe|reset|status [--config]|config set <key>=<json>...|config clear <key>...|facts (--program|--instance <alias|instance_id|ADB port>) [--key-prefix <prefix>]|request-shutdown|install-transition --action-json <json>|monitor-status|perf-status [--instance <alias|instance_id|ADB port>]|fact-refresh [--instance <alias|instance_id|ADB port>]|monitor-set|monitor-clear|emulator <status|start|stop|restart|discover>|stream|task-run|task-offset <task_id> <offset_milli>|pause [--reason <code>] [--drain-timeout-ms <n>]|resume|selfcheck <alias>|ladder-drill --instance <alias|instance_id|ADB port> [--device]> --state-root <path> [--instance <id>] [--program] [--wait <seconds>] [--package <locator> [--recovery-package <locator>]]\nusage: actingctl watchdog <status|run-once [--from-task]|install|uninstall> --root <install root>";
/// The developer text of an `ACTINGCOMMAND_DETAIL` the CLI does not know.
const DETAIL_USAGE: &str = "ACTINGCOMMAND_DETAIL takes codes or full";
/// The member a result that is not a JSON object goes under.
const RESULT_MEMBER: &str = "result";
/// Upper bound for `--wait <seconds>` (`request-shutdown`, `install-transition`; Workflow #389
/// MCP-21: `emulator start|stop|restart` and `task-run`).
const MAX_WAIT_SECONDS: u64 = 3600;
/// `source_detector` of the priority offsets `task-offset` publishes.
const TASK_OFFSET_DETECTOR: &str = "actingctl.task-offset";
/// `pause` defaults (Workflow #191 ps1): the reason code and the instance drain timeout.
const DEFAULT_PAUSE_REASON: &str = "operator";
const DEFAULT_PAUSE_DRAIN_TIMEOUT_MS: u64 = 60_000;

fn main() -> ExitCode {
    // Model §6 (M2): actingctl's diagnostic sink. Foreign text cut at its bound and other
    // developer text about an outcome go to stderr as `DIAG actingctl:` lines.
    if let Err(error) =
        actingcommand_contract::outcome::install_diagnostic_sink(Box::new(|outcome, text| {
            eprintln!("DIAG actingctl: {outcome} {text}");
        }))
    {
        eprintln!("DIAG actingctl: {error}");
    }
    // Workflow #389 U-2 (model section 3.5): an actingd before U-2 leaves this process's
    // connections unbound; the runtime client reports it once, as a developer line on stderr
    // (mcp-serve's stdout is the protocol, so the same line serves it).
    actingcommand_runtime_client::set_channel_unsupported_reporter(|outcome| {
        cli_line::developer_line(outcome, None);
    });
    let arguments: Vec<OsString> = env::args_os().skip(1).collect();
    if let Err(error) = actingcommand_contract::process_installation() {
        let outcome = Outcome::new(error.code());
        // Model §7.3: `mcp-serve` and `mcp-config` print no result line (stdout is the
        // protocol or the printed configuration); only the developer line.
        if matches!(
            arguments.first().and_then(|argument| argument.to_str()),
            Some("mcp-serve" | "mcp-config")
        ) {
            cli_line::developer_line(&outcome, None);
            return ExitCode::from(cli_line::exit_code(outcome.category(), true));
        }
        return failed(&outcome, None);
    }
    // Workflow #338: `mcp-serve` and `mcp-config` take their own flags, so they leave before
    // `Invocation::parse`, which requires `--state-root`.
    #[cfg(feature = "mcp")]
    {
        if let Some(exit) = mcp::dispatch(&arguments) {
            return exit;
        }
    }
    // Workflow #378 A5: every other exit prints one result line through `cli_line`, in the
    // detail ACTINGCOMMAND_DETAIL asks for; an unknown value is refused before any command runs.
    let detail = match cli_line::detail() {
        Ok(detail) => detail,
        Err(outcome) => return failed(&outcome, Some(DETAIL_USAGE)),
    };
    // Workflow #374: `watchdog` takes `--root`, not `--state-root`.
    #[cfg(windows)]
    {
        if let Some(exit) = watchdog::dispatch(&arguments) {
            return exit;
        }
    }
    // --- Workflow #389 MCP-1: `events`, the CLI of ac_events, parses its own flags. ---
    if let Some(exit) = events::dispatch(&arguments) {
        return exit;
    }
    // --- end MCP-1 ---
    // Workflow #389 MCP-5a: `observe --export` takes the instance selector of MCP and leaves
    // before `Invocation::parse`; `observe` without `--export` is unchanged.
    if let Some(exit) = observe_export::dispatch(&arguments) {
        return exit;
    }
    // Workflow #389 MCP-10: `start` and `restart` take `--root`, not `--state-root`; CLI only.
    #[cfg(windows)]
    {
        if let Some(exit) = runtime_start::dispatch(&arguments) {
            return exit;
        }
    }
    // ---- Workflow #389 MCP-11 (begin): `packs` reads the selected configuration offline and
    // takes no `--state-root` (command table D5). ----
    if let Some(exit) = packs::dispatch(&arguments) {
        return exit;
    }
    // ---- Workflow #389 MCP-11 (end) ----
    // ---- Workflow #389 MCP-14 (begin): `codes` reads the outcome catalog this build embeds
    // offline and takes no `--state-root` (command table D5); its flags are `flags_of`'s. ----
    if let Some(exit) = code_lookup::dispatch(&arguments) {
        return exit;
    }
    // ---- Workflow #389 MCP-14 (end) ----
    // Workflow #389 MCP-3: `runs` takes the instance selector of MCP and leaves before
    // `Invocation::parse`.
    if let Some(exit) = run_list::dispatch(&arguments) {
        return exit;
    }
    // ---- Workflow #389 MCP-7 (begin): `diagnose` takes the instance selector of MCP, optional,
    // and leaves before `Invocation::parse`. ----
    if let Some(exit) = diagnose::dispatch(&arguments) {
        return exit;
    }
    // ---- Workflow #389 MCP-7 (end) ----
    // ---- Workflow #389 MCP-8 (begin): `overview` takes the instance selector of MCP, `--root`
    // and `--sections`, and leaves before `Invocation::parse`. ----
    if let Some(exit) = overview::dispatch(&arguments) {
        return exit;
    }
    // ---- Workflow #389 MCP-8 (end) ----
    // ---- Workflow #389 MCP-6 (begin): `run evidence` takes `--request-id` or `--run-id` and
    // leaves before `Invocation::parse`; its flags are `flags_of`'s. ----
    if let Some(exit) = run_evidence::dispatch(&arguments) {
        return exit;
    }
    // ---- Workflow #389 MCP-6 (end) ----
    // ---- Workflow #398 RD-1c (begin): `devices` runs the install's adb, never the Runtime, and
    // leaves before `Invocation::parse`; its flags are `flags_of`'s. ----
    if let Some(exit) = devices::dispatch(&arguments) {
        return exit;
    }
    // ---- Workflow #398 RD-1c (end) ----
    // Workflow #410 item 6: `capability-gap` appends to the user's local log, never the Runtime,
    // and leaves before `Invocation::parse`; its flags are `flags_of`'s.
    if let Some(exit) = capability_gap::dispatch(&arguments) {
        return exit;
    }
    // Workflow #411 B15 R6: `remedy` takes its action as the second word and leaves before
    // `Invocation::parse`; its flags are `flags_of`'s.
    if let Some(exit) = remedy::dispatch(&arguments, detail) {
        return exit;
    }
    match run(arguments, detail) {
        Ok(Done {
            members,
            outcome,
            refused: false,
        }) => cli_line::success(members, &outcome),
        // Receipt v2 (Workflow #378 A4, part B L2): a refused or failed request exits 1, judged
        // by its receipt's state; the outcome's category only says how bad it is.
        Ok(Done {
            members,
            outcome,
            refused: true,
        }) => cli_line::receipt(members, &outcome, true),
        // Model §7.3: a result that does not encode leaves only the developer line.
        Err(error @ ActingctlError::Output(_)) => {
            cli_line::developer_line(&error.outcome(), Some(&error.to_string()));
            ExitCode::FAILURE
        }
        Err(error) => failed(&error.outcome(), Some(&error.to_string())),
    }
}

/// A failed exit: `{outcome}` and the developer line with `text`. It exits non-zero whatever
/// the top's category: a refusal the Runtime topped with an info or warning code still failed
/// (A4 part B L2), so its line goes out as a refused one.
fn failed(outcome: &Outcome, text: Option<&str>) -> ExitCode {
    cli_line::refused(outcome, text)
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

/// Workflow #381 A R1′ (review M2): one deadline for each run, not one per exchange. Query,
/// Release, BeginDrain and Abort take at most 60 s in all; CommitShutdown's exchanges take at
/// most 10 s, so that with the `--wait 60` acsetup adds they stay inside acsetup's 75 s.
const INSTALL_TRANSITION_RUN_BUDGET: Duration = Duration::from_secs(60);
const INSTALL_COMMIT_EXCHANGE_BUDGET: Duration = Duration::from_secs(10);

fn install_transition_run(action: &InstallTransitionAction) -> InstallTransitionRun {
    let budget = match action {
        InstallTransitionAction::CommitShutdown { .. } => INSTALL_COMMIT_EXCHANGE_BUDGET,
        _ => INSTALL_TRANSITION_RUN_BUDGET,
    };
    InstallTransitionRun {
        budget: Some(budget),
        exchange_timeout: budget,
        // Workflow #381 A R2: the read-only Query declares no identity, so it appends nothing.
        declares_identity: !matches!(action, InstallTransitionAction::Query { .. }),
    }
}

/// Workflow #381 A R1′: the client configuration of one run. An install-transition run gets its
/// budget as the exchange timeout and as one deadline counted from `started`.
fn client_config(state_root: &Path, command: &Command, started: Instant) -> RuntimeClientConfig {
    let (actor, source) = command.origin();
    let config = RuntimeClientConfig::new(
        state_root,
        actingcommand_runtime_client::RuntimeChannel::Cli,
        actor,
        source,
    );
    let Command::InstallTransition { action } = command else {
        return config;
    };
    let run = install_transition_run(action);
    let config = config.with_io_timeout(run.exchange_timeout);
    match run.budget {
        Some(budget) => config.with_deadline(started + budget),
        None => config,
    }
}

/// One subcommand's result line (Workflow #378 A5): the result's members, its top outcome, and
/// whether the receipt it prints was refused or failed (A4 part B L2: such a line exits
/// non-zero whatever the outcome's category).
struct Done {
    members: Map<String, Value>,
    outcome: Outcome,
    refused: bool,
}

impl Done {
    fn new(members: Map<String, Value>, outcome: Outcome) -> Self {
        Self {
            members,
            outcome,
            refused: false,
        }
    }
}

fn run(arguments: Vec<OsString>, detail: Detail) -> Result<Done, ActingctlError> {
    let started = Instant::now();
    let Invocation {
        state_root,
        instance,
        wait,
        command,
    } = Invocation::parse(arguments)?;
    let success = command.success(wait.is_some());
    let observation = match &command {
        Command::AgentPublishFacts { record_file } => Some(read_fact_observation(record_file)?),
        _ => None,
    };
    // Workflow #308 RT-S1a: the policy document is read bounded and checked as UTF-8 only;
    // the Runtime parses and checks its content.
    let resource_targets = match &command {
        Command::AgentApplyResourceTargets { policy_file } => {
            Some(read_resource_targets(policy_file)?)
        }
        _ => None,
    };
    let install_run = match &command {
        Command::InstallTransition { action } => Some(install_transition_run(action)),
        _ => None,
    };
    let config = client_config(&state_root, &command, started);
    // Model §7.1: `ACTINGCOMMAND_DETAIL=codes` reaches the receipts the line prints, too.
    let config = match detail {
        Detail::Codes => config.with_detail(Detail::Codes),
        Detail::Full => config,
    };
    let client = RuntimeClient::connect(config).map_err(ActingctlError::Runtime)?;
    let optional_instance = instance.clone();
    let instance = || instance.as_deref().ok_or(ActingctlError::Usage);
    match command {
        Command::AgentPublishFacts { .. } => {
            let observation = observation.ok_or(ActingctlError::Usage)?;
            let event_id = client
                .publish_facts(observation)
                .map_err(ActingctlError::Runtime)?;
            Ok(Done::new(
                members([member("event_id", &event_id)?]),
                Outcome::new(success),
            ))
        }
        // Workflow #387 D1b: `config set|clear` (MCP `ac_config_set`); a refused change prints
        // the Runtime's receipt beside its outcome and exits non-zero.
        Command::ConfigureSettings { changes } => match client.configure_settings(changes) {
            Ok(settings) => Ok(Done::new(
                members([member("settings", &settings)?]),
                Outcome::new(success),
            )),
            Err(error) => refused(error),
        },
        // A refused policy prints the Runtime's receipt beside its outcome and exits non-zero.
        Command::AgentApplyResourceTargets { .. } => {
            let document = resource_targets.ok_or(ActingctlError::Usage)?;
            match client.apply_resource_targets(document) {
                Ok(applied) => Ok(Done::new(
                    members([member("applied", &applied)?]),
                    Outcome::new(success),
                )),
                Err(error) => refused(error),
            }
        }
        // One manual priority offset (Workflow #308 slice 4a-2) through the ordinary fact
        // publication, with this CLI's Cli/Cli origin.
        Command::TaskOffset {
            task_id,
            offset_milli,
        } => {
            let status = client.status().map_err(ActingctlError::Runtime)?;
            let scope = match optional_instance.as_deref() {
                Some(alias) => {
                    if !status
                        .instances()
                        .iter()
                        .any(|registered| registered.instance_alias() == alias)
                    {
                        return Err(ActingctlError::InstanceUnknown(alias.to_owned()));
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
            .map_err(ActingctlError::PriorityOffsetInvalid)?;
            let (scope, key) = (record.scope.clone(), record.key.clone());
            let event_id = client
                .publish_facts(FactObservation {
                    records: vec![record],
                })
                .map_err(ActingctlError::Runtime)?;
            Ok(Done::new(
                members([
                    member("event_id", &event_id)?,
                    member("scope", &scope)?,
                    member("key", &key)?,
                    member("offset_milli", &offset_milli)?,
                ]),
                Outcome::new(success),
            ))
        }
        Command::RequestShutdown => {
            let receipt = client.request_shutdown().map_err(ActingctlError::Runtime)?;
            let mut result = members([member("receipt", &receipt)?]);
            if let Some(wait) = wait {
                // request_shutdown verified that the accepted receipt names exactly this target.
                let target = client.runtime_info().shutdown_target();
                // Close the connection so the waiting CLI leaves nothing for the Runtime to drain.
                drop(client);
                let shutdown = shutdown_wait::wait_for_shutdown(&state_root, &target, wait)?;
                result.insert("shutdown".to_owned(), shutdown);
            }
            Ok(Done::new(result, Outcome::new(success)))
        }
        Command::InstallTransition { action } => {
            if install_run.is_some_and(|install_run| install_run.declares_identity) {
                client
                    .declare_governance_identity(&actingcommand_contract::GovernanceIdentityCard {
                        client: "actingctl".to_owned(),
                        client_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
                        instance: None,
                    })
                    .map_err(ActingctlError::Runtime)?;
            }
            let transition_id = action_transition_id(&action).to_owned();
            let receipt = client
                .install_transition(action)
                .map_err(ActingctlError::Runtime)?;
            let mut result = members([member("receipt", &receipt)?]);
            if let Some(wait) = wait {
                let target = client.runtime_info().shutdown_target();
                drop(client);
                let shutdown = shutdown_wait::wait_for_shutdown(&state_root, &target, wait)?;
                result.insert("shutdown".to_owned(), shutdown);
            }
            Ok(Done::new(
                result,
                install_transition_outcome(&receipt, &transition_id),
            ))
        }
        Command::Reset => reported(
            &client
                .safe_reset(instance()?)
                .map_err(ActingctlError::Runtime)?,
            success,
        ),
        Command::Observe => reported(
            &client
                .observe_readonly(instance()?)
                .map_err(ActingctlError::Runtime)?,
            success,
        ),
        Command::Status => reported(&client.status().map_err(ActingctlError::Runtime)?, success),
        // Workflow #389 MCP-4 (command table §4.4): the `ac_facts` answer of each form. `status
        // --config` is the program-fact read reduced to the `config.*` records (Workflow #387
        // D1b); `facts --instance` resolves its selector with one status read.
        Command::StatusConfig => facts_line(&client, &facts::FactsRequest::Config, success),
        Command::ProgramFacts { key_prefix } => facts_line(
            &client,
            &facts::FactsRequest::Program { key_prefix },
            success,
        ),
        Command::InstanceFacts { key_prefix } => facts_line(
            &client,
            &facts::FactsRequest::Instance {
                selector: instance()?.to_owned(),
                key_prefix,
            },
            success,
        ),
        Command::MonitorStatus => reported(
            &client.monitor_status().map_err(ActingctlError::Runtime)?,
            success,
        ),
        // Workflow #385 P-m (PERF §4.6): the shadow measurement's status, the object
        // `ac_overview` answers as its `performance` block; `--instance` (alias, instance_id
        // or ADB port) narrows its rows to one, resolved with one status read.
        Command::PerformanceStatus => {
            let selected = match optional_instance.as_deref() {
                Some(selector) => {
                    let status = client.status().map_err(ActingctlError::Runtime)?;
                    let alias = instance_selector::select(status.instances(), selector)
                        .map_err(|miss| match miss {
                            SelectorMiss::Unknown => {
                                ActingctlError::InstanceUnknown(selector.to_owned())
                            }
                            SelectorMiss::Ambiguous => {
                                ActingctlError::InstanceAmbiguous(selector.to_owned())
                            }
                        })?
                        .instance_alias()
                        .to_owned();
                    Some(alias)
                }
                None => None,
            };
            reported(
                &overview::performance_block(&client, selected.as_deref())
                    .map_err(ActingctlError::Runtime)?,
                success,
            )
        }
        // Workflow #384 D2-S5 (D2 model §6 S5): what the scheduler's fact refresh waits for, the
        // object `ac_overview` answers as its `fact_refresh` block; `--instance` (alias,
        // instance_id or ADB port) narrows its rows to one, resolved with one status read.
        Command::FactRefreshStatus => {
            let selected = match optional_instance.as_deref() {
                Some(selector) => {
                    let status = client.status().map_err(ActingctlError::Runtime)?;
                    let alias = instance_selector::select(status.instances(), selector)
                        .map_err(|miss| match miss {
                            SelectorMiss::Unknown => {
                                ActingctlError::InstanceUnknown(selector.to_owned())
                            }
                            SelectorMiss::Ambiguous => {
                                ActingctlError::InstanceAmbiguous(selector.to_owned())
                            }
                        })?
                        .instance_alias()
                        .to_owned();
                    Some(alias)
                }
                None => None,
            };
            reported(
                &overview::fact_refresh_block(&client, selected.as_deref())
                    .map_err(ActingctlError::Runtime)?,
                success,
            )
        }
        Command::MonitorSet { policy } => reported(
            &client
                .configure_monitor(instance()?, policy)
                .map_err(ActingctlError::Runtime)?,
            success,
        ),
        Command::MonitorClear => reported(
            &client
                .clear_monitor(instance()?)
                .map_err(ActingctlError::Runtime)?,
            success,
        ),
        // Served by the existing status read, filtered to the one alias; no new operation.
        Command::EmulatorStatus => {
            let alias = instance()?;
            let status = client.status().map_err(ActingctlError::Runtime)?;
            let instance = status
                .instances()
                .iter()
                .find(|instance| instance.instance_alias() == alias)
                .ok_or_else(|| ActingctlError::InstanceUnknown(alias.to_owned()))?;
            reported(instance, success)
        }
        Command::EmulatorControl { action } => {
            let alias = instance()?.to_owned();
            let Some(wait) = wait else {
                return reported(
                    &client
                        .control_emulator_instance(&alias, action)
                        .map_err(ActingctlError::Runtime)?,
                    success,
                );
            };
            // Workflow #389 MCP-21: the interaction's correlation names the request before its
            // receipt; the client mints the request id only as it writes it.
            let interaction = client
                .begin_interaction()
                .map_err(ActingctlError::Runtime)?;
            let correlation_id = interaction.correlation_id();
            match wait_at_most(wait, move || {
                interaction.control_emulator_instance(&alias, action)
            }) {
                Waited::Done(result) => {
                    reported(&result.map_err(ActingctlError::Runtime)?, success)
                }
                Waited::Expired => Ok(wait_expired(
                    ContractLocation::ControlEmulatorInstance,
                    wait,
                    None,
                    correlation_id,
                )),
            }
        }
        Command::EmulatorDiscover => {
            let discovery = client
                .discover_instances()
                .map_err(ActingctlError::Runtime)?;
            Ok(Done::new(
                result_members(&discovery, RESULT_MEMBER)?,
                Outcome::new(success).count(discovery.instances().len()),
            ))
        }
        // Workflow #191 ps1: without --instance the pause / resume is global.
        Command::Pause {
            pause_reason,
            drain_timeout_ms,
        } => reported(
            &client
                .pause_scheduling(
                    pause_scope(optional_instance),
                    pause_reason,
                    drain_timeout_ms,
                )
                .map_err(ActingctlError::Runtime)?,
            success,
        ),
        Command::Resume => reported(
            &client
                .resume_scheduling(pause_scope(optional_instance))
                .map_err(ActingctlError::Runtime)?,
            success,
        ),
        // Workflow #317 sc3 (d): manual reconnect and self-check of one physical instance. A
        // check that failed is no success: `instance_self_check_failed`, caused by the check's
        // failure code (the result is printed beside it).
        Command::SelfCheck { instance_alias } => {
            let result = client
                .self_check_instance(&instance_alias)
                .map_err(ActingctlError::Runtime)?;
            let outcome = match &result {
                RuntimeResult::InstanceSelfChecked { selfcheck, .. }
                    if selfcheck.failure_code.is_some()
                        || !selfcheck.capture.ok
                        || !selfcheck.touch.ok =>
                {
                    let check_failed = Outcome::new(ActingctlCode::InstanceSelfCheckFailed);
                    match selfcheck.failure_code.as_deref() {
                        Some(spelling) => with_cause(check_failed, Outcome::relayed(spelling)),
                        None => check_failed,
                    }
                }
                _ => Outcome::new(success),
            };
            Ok(Done::new(result_members(&result, RESULT_MEMBER)?, outcome))
        }
        // Workflow #390 DRILL-1: the drill the Runtime admitted, as `ac_ladder_drill` answers it.
        Command::LadderDrill { mode } => {
            let drill =
                ladder_drill::drill(&client, instance()?, mode).map_err(
                    |failure| match failure {
                        ladder_drill::Failure::Selector {
                            miss: SelectorMiss::Unknown,
                            selector,
                        } => ActingctlError::InstanceUnknown(selector),
                        ladder_drill::Failure::Selector {
                            miss: SelectorMiss::Ambiguous,
                            selector,
                        } => ActingctlError::InstanceAmbiguous(selector),
                        ladder_drill::Failure::Runtime(error) => ActingctlError::Runtime(error),
                    },
                )?;
            reported(&drill, success)
        }
        Command::Stream { spec } => reported(
            &client
                .capture_sequence(instance()?, spec)
                .map_err(ActingctlError::Runtime)?,
            success,
        ),
        Command::TaskRun {
            package,
            recovery_package,
        } => {
            let package = canonical_package(package)?;
            let recovery = match recovery_package {
                Some(package) => Some(canonical_package(package)?.display().to_string()),
                None => None,
            };
            let request = task_run_request(package.display().to_string(), recovery)?;
            let output = match wait {
                None => client
                    .run_contained_task(instance()?, request)
                    .map_err(ActingctlError::Runtime)?,
                // Workflow #389 MCP-21: the prepared request names its request_id before it is
                // written; not done within `--wait`, the run goes on in the Runtime.
                Some(wait) => {
                    let prepared = client
                        .prepare_contained_task(instance()?, request)
                        .map_err(ActingctlError::Runtime)?;
                    let request_id = prepared.request_id();
                    let correlation_id = prepared.correlation_id();
                    let submitter = client.clone();
                    match wait_at_most(wait, move || submitter.submit_prepared(prepared)) {
                        Waited::Done(output) => output.map_err(ActingctlError::Runtime)?,
                        Waited::Expired => {
                            return Ok(wait_expired(
                                ContractLocation::RunContainedTask,
                                wait,
                                Some(request_id),
                                Some(correlation_id),
                            ));
                        }
                    }
                }
            };
            // A run that ended failed may still come back with its receipt and events (a
            // committed failure with its official OCR fields): the receipt's own outcome is the
            // top, and the line exits non-zero.
            let completed = output.receipt().state() == RuntimeReceiptState::Completed;
            let outcome = if completed {
                Outcome::new(success)
            } else {
                output.receipt().outcome().clone()
            };
            Ok(Done {
                members: result_members(&output, RESULT_MEMBER)?,
                outcome,
                refused: !completed,
            })
        }
    }
}

/// The line of a typed result whose subcommand succeeded with `success`.
fn reported(result: &impl Serialize, success: ActingctlCode) -> Result<Done, ActingctlError> {
    Ok(Done::new(
        result_members(result, RESULT_MEMBER)?,
        Outcome::new(success),
    ))
}

/// Workflow #389 MCP-21: how a `--wait` ended.
enum Waited<T> {
    /// The request ended within the wait, with this.
    Done(T),
    /// The wait ended first.
    Expired,
}

/// Runs `request` on a thread of its own and waits for it at most `wait` (Workflow #389 MCP-21).
/// A request that has not ended by then goes on in the Runtime: the Runtime serves it on its
/// own connection thread whether its client still waits or not, and this process stops waiting.
/// A request thread that panicked panics here too.
fn wait_at_most<T: Send + 'static>(
    wait: Duration,
    request: impl FnOnce() -> T + Send + 'static,
) -> Waited<T> {
    let (sender, receiver) = mpsc::channel();
    let worker = thread::spawn(move || {
        // The receiver is gone only when the wait already ended.
        let _ = sender.send(request());
    });
    match receiver.recv_timeout(wait) {
        Ok(done) => Waited::Done(done),
        Err(RecvTimeoutError::Timeout) => Waited::Expired,
        Err(RecvTimeoutError::Disconnected) => match worker.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            // A thread that ended sent its result first, which `recv_timeout` takes before it
            // sees the channel closed.
            Ok(()) => unreachable!("a request thread ended without its result"),
        },
    }
}

/// Workflow #389 MCP-21 (MCP-0 command table v1.1 §4.22, D11 as verified): the line of a
/// `--wait` that ended before its request did, `{request_id?, correlation_id?, state:
/// "in_progress"}` with `command_wait_expired` (`operation`, `request_id` when this command
/// knows it, `waited_ms`). The request goes on in the Runtime; the line exits 1.
fn wait_expired(
    operation: ContractLocation,
    wait: Duration,
    request_id: Option<RequestId>,
    correlation_id: Option<CorrelationId>,
) -> Done {
    let mut members = Map::new();
    let mut outcome = Outcome::new(SharedCode::CommandWaitExpired)
        .operation(operation)
        .waited_ms(wait);
    if let Some(request_id) = request_id {
        let text = run_evidence::id_text(&request_id);
        outcome = outcome.request_id(text.as_str());
        members.insert("request_id".to_owned(), Value::String(text));
    }
    if let Some(correlation_id) = correlation_id {
        members.insert(
            "correlation_id".to_owned(),
            Value::String(run_evidence::id_text(&correlation_id)),
        );
    }
    members.insert("state".to_owned(), Value::String("in_progress".to_owned()));
    Done {
        members,
        outcome,
        refused: true,
    }
}

/// Workflow #389 MCP-4: the line of `facts` and `status --config`, the `ac_facts` result of the
/// same read beside the outcome `success`.
fn facts_line(
    client: &RuntimeClient,
    request: &facts::FactsRequest,
    success: ActingctlCode,
) -> Result<Done, ActingctlError> {
    let answer = facts::read(client, request).map_err(|failure| match failure {
        facts::Failure::Selector {
            miss: SelectorMiss::Unknown,
            selector,
        } => ActingctlError::InstanceUnknown(selector),
        facts::Failure::Selector {
            miss: SelectorMiss::Ambiguous,
            selector,
        } => ActingctlError::InstanceAmbiguous(selector),
        facts::Failure::ConfigFactsMissing => ActingctlError::ConfigFactsMissing,
        facts::Failure::Runtime(error) => ActingctlError::Runtime(error),
        facts::Failure::Output(error) => ActingctlError::Output(error),
    })?;
    Ok(Done::new(
        result_members(&answer, RESULT_MEMBER)?,
        Outcome::new(success),
    ))
}

/// A refused request whose receipt the subcommand prints: `{receipt, outcome}` with the
/// receipt's outcome on top; it exits non-zero (A4 part B L2). Without a receipt it is the
/// client's failure.
fn refused(error: RuntimeClientError) -> Result<Done, ActingctlError> {
    let Some(receipt) = error.received_receipt() else {
        return Err(ActingctlError::Runtime(error));
    };
    Ok(Done {
        members: members([member("receipt", receipt)?]),
        outcome: error.outcome(),
        refused: true,
    })
}

/// One member of a result line; a value that does not encode is `actingctl_output_failed`.
fn member(name: &str, value: &impl Serialize) -> Result<(String, Value), ActingctlError> {
    Ok((
        name.to_owned(),
        serde_json::to_value(value).map_err(ActingctlError::Output)?,
    ))
}

fn members<const N: usize>(members: [(String, Value); N]) -> Map<String, Value> {
    members.into_iter().collect()
}

/// A typed result as the members of its line; a result that is not a JSON object goes under
/// `name`.
fn result_members(
    result: &impl Serialize,
    name: &str,
) -> Result<Map<String, Value>, ActingctlError> {
    serde_json::to_value(result)
        .map(|value| cli_line::members(value, name))
        .map_err(ActingctlError::Output)
}

/// `top` caused by `cause`. A fatal cause is the top itself: a fatal link needs a fatal top
/// (spec §5), and the operation ended in that condition.
fn with_cause(top: Outcome, cause: Outcome) -> Outcome {
    if cause.category() == Category::Fatal {
        cause
    } else {
        top.caused_by(cause)
    }
}

/// The transition an install-transition action names.
fn action_transition_id(action: &InstallTransitionAction) -> &str {
    match action {
        InstallTransitionAction::BeginDrain { transition_id, .. }
        | InstallTransitionAction::Query { transition_id } => transition_id,
        InstallTransitionAction::Abort { ticket }
        | InstallTransitionAction::CommitShutdown { ticket }
        | InstallTransitionAction::Release { ticket, .. } => &ticket.transition_id,
    }
}

/// Model §7.3: the outcome of an install transition's receipt. A reported phase `failed` or
/// `aborted` is `install_transition_failed` / `install_transition_aborted` (error), caused by
/// the transition's failure code when it carries one; any other phase is
/// `install_transition_reported`. A commit's receipt (the accepted shutdown) reports no phase,
/// only the transition the action named.
fn install_transition_outcome(receipt: &RuntimeReceipt, transition_id: &str) -> Outcome {
    let Some(RuntimeResult::InstallTransition { status }) = receipt.result() else {
        return Outcome::new(ActingctlCode::InstallTransitionReported).transition_id(transition_id);
    };
    let (code, ended) = match status.phase {
        InstallTransitionPhase::Failed => (ActingctlCode::InstallTransitionFailed, true),
        InstallTransitionPhase::Aborted => (ActingctlCode::InstallTransitionAborted, true),
        _ => (ActingctlCode::InstallTransitionReported, false),
    };
    let outcome = Outcome::new(code)
        .transition_id(&status.ticket.transition_id)
        .phase(status.phase);
    match status.failure_code.as_deref() {
        Some(spelling) if ended => with_cause(outcome, Outcome::relayed(spelling)),
        _ => outcome,
    }
}

/// `task-run`: a package path as the Runtime opens it.
fn canonical_package(path: PathBuf) -> Result<PathBuf, ActingctlError> {
    match std::fs::canonicalize(&path) {
        Ok(canonical) => Ok(canonical),
        Err(error) => Err(ActingctlError::Package { path, error }),
    }
}

/// Reads at most `limit` bytes of an input file; a longer file is `TooLarge`.
fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, InputProblem> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|error| InputProblem::Io(error, IoOp::Open))?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| InputProblem::Io(error, IoOp::Read))?;
    if bytes.len() > limit {
        return Err(InputProblem::TooLarge(limit));
    }
    Ok(bytes)
}

/// `agent-publish-facts --record-file`: one bounded, valid fact observation.
fn read_fact_observation(path: &Path) -> Result<FactObservation, ActingctlError> {
    let invalid = |problem: InputProblem| ActingctlError::FactRecord {
        path: path.to_path_buf(),
        problem,
    };
    let bytes =
        read_bounded(path, actingcommand_contract::MAX_FACT_OBSERVATION_BYTES).map_err(invalid)?;
    let observation: FactObservation =
        serde_json::from_slice(&bytes).map_err(|error| invalid(InputProblem::Json(error)))?;
    observation
        .validate()
        .map_err(|error| invalid(InputProblem::Invalid(error)))?;
    Ok(observation)
}

/// `agent-apply-resource-targets --policy-file`: the document, bounded, not empty, UTF-8.
fn read_resource_targets(path: &Path) -> Result<String, ActingctlError> {
    let invalid = |problem: InputProblem| ActingctlError::ResourceTargetsFile {
        path: path.to_path_buf(),
        problem,
    };
    let bytes = read_bounded(
        path,
        actingcommand_contract::MAX_RESOURCE_TARGETS_DOCUMENT_BYTES,
    )
    .map_err(invalid)?;
    if bytes.is_empty() {
        return Err(invalid(InputProblem::Empty));
    }
    String::from_utf8(bytes).map_err(|error| invalid(InputProblem::NotUtf8(error)))
}

/// Workflow #387 D1b: the changes of `config set <key>=<json>…` and `config clear <key>…`, the
/// same `ConfigureSettings` changes MCP `ac_config_set` sends: a set carries its value (JSON, not
/// `null`), a clear none.
fn config_changes(
    action: &str,
    items: &[String],
) -> Result<Vec<ConfigSettingChange>, ActingctlError> {
    if items.is_empty() || items.len() > MAX_CONFIG_SETTING_CHANGES {
        return Err(ActingctlError::Usage);
    }
    items
        .iter()
        .map(|item| match action {
            "set" => {
                let (key, json) = item.split_once('=').ok_or(ActingctlError::Usage)?;
                let value = serde_json::from_str::<Value>(json)
                    .ok()
                    .filter(|value| !value.is_null())
                    .ok_or(ActingctlError::Usage)?;
                Ok(ConfigSettingChange {
                    key: key.to_owned(),
                    value: Some(value),
                })
            }
            "clear" => Ok(ConfigSettingChange {
                key: item.clone(),
                value: None,
            }),
            _ => Err(ActingctlError::Usage),
        })
        .collect()
}

fn pause_scope(instance: Option<String>) -> SchedulingPauseScope {
    match instance {
        Some(instance_alias) => SchedulingPauseScope::Instance { instance_alias },
        None => SchedulingPauseScope::Global,
    }
}

/// Workflow #336 PKG-S4 (option 丙): the task's package, and its recovery package, by their
/// paths only.
fn task_run_request(
    package_path: String,
    recovery: Option<String>,
) -> Result<ContainedTaskRequest, ActingctlError> {
    ContainedTaskRequest::new(package_path)
        .and_then(|request| match recovery {
            Some(package_path) => {
                request.with_recovery(ContainedTaskRecoveryBinding::new(package_path)?)
            }
            None => Ok(request),
        })
        .and_then(|request| {
            request.with_response_deadline_ms(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS)
        })
        .map_err(|_| ActingctlError::Usage)
}

struct Invocation {
    state_root: PathBuf,
    instance: Option<String>,
    /// `--wait <seconds>`.
    wait: Option<Duration>,
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
    /// `config set <key>=<json>…` and `config clear <key>…` (Workflow #387 D1b).
    ConfigureSettings {
        changes: Vec<ConfigSettingChange>,
    },
    /// `facts --program [--key-prefix <prefix>]`.
    ProgramFacts {
        key_prefix: Option<String>,
    },
    /// `facts --instance <selector> [--key-prefix <prefix>]` (Workflow #389 MCP-4).
    InstanceFacts {
        key_prefix: Option<String>,
    },
    MonitorStatus,
    /// `perf-status [--instance <selector>]` (Workflow #385 P-m).
    PerformanceStatus,
    /// `fact-refresh [--instance <selector>]` (Workflow #384 D2-S5).
    FactRefreshStatus,
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
        pause_reason: actingcommand_contract::SchedulingPauseReason,
        drain_timeout_ms: u64,
    },
    /// `resume [--instance <alias>]`.
    Resume,
    /// `selfcheck <alias>`: reconnect and self-check one physical instance now.
    SelfCheck {
        instance_alias: String,
    },
    /// `ladder-drill --instance <selector> [--device]` (Workflow #390 DRILL-1): a drill of one
    /// instance's stuck-recovery ladder; a plan unless `--device` says to act on the device.
    LadderDrill {
        mode: actingcommand_contract::RecoveryDrillMode,
    },
    Stream {
        spec: CaptureSequenceSpec,
    },
    TaskRun {
        package: PathBuf,
        recovery_package: Option<PathBuf>,
    },
}

impl Invocation {
    fn parse(arguments: Vec<OsString>) -> Result<Self, ActingctlError> {
        let Some(command) = arguments.first().and_then(|value| value.to_str()) else {
            return Err(ActingctlError::Usage);
        };
        // Workflow #387 D1b: `config` takes its action as the second token and its keys (and
        // values) as the tokens before the first flag.
        let config_items = if command == "config" {
            let action = arguments
                .get(1)
                .and_then(|value| value.to_str())
                .ok_or(ActingctlError::Usage)?;
            let items = arguments[2..]
                .iter()
                .map_while(|value| value.to_str().filter(|text| !text.starts_with("--")))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            Some((action, items))
        } else {
            None
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
        let mut recovery_package = None;
        let mut recovery_enabled = false;
        let mut program = false;
        let mut config = false;
        let mut key_prefix = None;
        let mut record_file = None;
        let mut policy_file = None;
        let mut wait = None;
        let mut install_action = None;
        let mut pause_reason = None;
        let mut drain_timeout_ms = None;
        // Workflow #390 DRILL-1: `ladder-drill --device` acts on the device.
        let mut device = false;
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
        let mut index = if let Some((_, items)) = &config_items {
            2 + items.len()
        } else if emulator_action.is_some() || selfcheck_alias.is_some() {
            2
        } else if task_offset.is_some() {
            3
        } else {
            1
        };
        let takes = flags_of(command, emulator_action).ok_or(ActingctlError::Usage)?;
        // Workflow #382 A1-FLAG: every word after the positional ones is a flag of this
        // subcommand, each given once; anything else is refused by name, never ignored.
        let mut given: Vec<&str> = Vec::new();
        while index < arguments.len() {
            let flag = arguments[index].to_str().ok_or(ActingctlError::Usage)?;
            if !takes.contains(&flag) {
                return Err(ActingctlError::argument(flag, ArgumentProblem::Unknown));
            }
            if given.contains(&flag) {
                return Err(ActingctlError::argument(flag, ArgumentProblem::Repeated));
            }
            given.push(flag);
            match flag {
                "--record-file" => {
                    record_file = Some(PathBuf::from(require_value(&arguments, &mut index)?));
                }
                "--policy-file" => {
                    policy_file = Some(PathBuf::from(require_value(&arguments, &mut index)?));
                }
                "--action-json" => {
                    let json = require_text(&arguments, &mut index)?;
                    if json.len() > 4096 {
                        return Err(ActingctlError::Usage);
                    }
                    let action: actingcommand_contract::InstallTransitionAction =
                        serde_json::from_str(&json).map_err(|_| ActingctlError::Usage)?;
                    action.validate().map_err(|_| ActingctlError::Usage)?;
                    install_action = Some(action);
                }
                "--wait" => {
                    let seconds = require_u64(&arguments, &mut index)?;
                    if !(1..=MAX_WAIT_SECONDS).contains(&seconds) {
                        return Err(ActingctlError::Usage);
                    }
                    wait = Some(Duration::from_secs(seconds));
                }
                "--reason" => {
                    let reason = require_text(&arguments, &mut index)?;
                    pause_reason = Some(
                        actingcommand_contract::SchedulingPauseReason::new(reason)
                            .map_err(|_| ActingctlError::Usage)?,
                    );
                }
                "--drain-timeout-ms" => {
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
                    // An empty or blank alias names no instance: a usage error, never the global
                    // scope that a pause or resume without `--instance` means (Fail Loud).
                    let alias = require_text(&arguments, &mut index)?;
                    if alias.trim().is_empty() {
                        return Err(ActingctlError::Usage);
                    }
                    instance = Some(alias);
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
                "--recovery-package" => {
                    recovery_package = Some(PathBuf::from(require_value(&arguments, &mut index)?));
                }
                "--recover" => recovery_enabled = true,
                "--device" => device = true,
                "--program" => program = true,
                "--config" => config = true,
                "--key-prefix" => {
                    let prefix = require_text(&arguments, &mut index)?;
                    if prefix.is_empty() {
                        return Err(ActingctlError::argument(
                            flag,
                            ArgumentProblem::ValueMissing,
                        ));
                    }
                    key_prefix = Some(prefix);
                }
                // Every flag of the table has its arm (`codes` and `run evidence`, whose flags
                // `code_lookup` and `run_evidence` parse, never come here).
                _ => return Err(ActingctlError::Usage),
            }
            index += 1;
        }
        let state_root = state_root.ok_or(ActingctlError::Usage)?;
        let command = match command {
            "agent-publish-facts" => Command::AgentPublishFacts {
                record_file: record_file.ok_or(ActingctlError::Usage)?,
            },
            "agent-apply-resource-targets" => Command::AgentApplyResourceTargets {
                policy_file: policy_file.ok_or(ActingctlError::Usage)?,
            },
            "task-offset" => {
                let (task_id, offset_milli) = task_offset.ok_or(ActingctlError::Usage)?;
                Command::TaskOffset {
                    task_id,
                    offset_milli,
                }
            }
            "request-shutdown" => Command::RequestShutdown,
            "install-transition" => {
                let action = install_action.ok_or(ActingctlError::Usage)?;
                if wait.is_some()
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
            "config" => {
                let (action, items) = config_items.ok_or(ActingctlError::Usage)?;
                Command::ConfigureSettings {
                    changes: config_changes(action, &items)?,
                }
            }
            "status" => Command::Status,
            // Workflow #389 MCP-4: the program store or one instance, never both.
            "facts" => match (program, instance.is_some()) {
                (true, false) => Command::ProgramFacts { key_prefix },
                (false, true) => Command::InstanceFacts { key_prefix },
                (true, true) => {
                    return Err(ActingctlError::argument(
                        "--instance",
                        ArgumentProblem::Conflicting,
                    ));
                }
                (false, false) => return Err(ActingctlError::Usage),
            },
            "monitor-status" => Command::MonitorStatus,
            "perf-status" => Command::PerformanceStatus,
            "fact-refresh" => Command::FactRefreshStatus,
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
                pause_reason: match pause_reason {
                    Some(reason) => reason,
                    None => {
                        actingcommand_contract::SchedulingPauseReason::new(DEFAULT_PAUSE_REASON)
                            .map_err(|_| ActingctlError::Usage)?
                    }
                },
                drain_timeout_ms: drain_timeout_ms.unwrap_or(DEFAULT_PAUSE_DRAIN_TIMEOUT_MS),
            },
            "resume" => Command::Resume,
            "selfcheck" => Command::SelfCheck {
                instance_alias: selfcheck_alias.ok_or(ActingctlError::Usage)?,
            },
            "ladder-drill" => Command::LadderDrill {
                mode: ladder_drill::mode(device),
            },
            "stream" => Command::Stream {
                spec: CaptureSequenceSpec::new(
                    frame_count.unwrap_or(1),
                    interval_ms.unwrap_or(250),
                )
                .map_err(|_| ActingctlError::Usage)?,
            },
            "task-run" => Command::TaskRun {
                package: package.ok_or(ActingctlError::Usage)?,
                recovery_package,
            },
            _ => return Err(ActingctlError::Usage),
        };
        // `task-offset` takes `--instance` optionally: without it the offset is task-level;
        // `pause` / `resume` without it are global; `perf-status` (Workflow #385 P-m) and
        // `fact-refresh` (Workflow #384 D2-S5) without it read every row.
        if !matches!(
            command,
            Command::TaskOffset { .. }
                | Command::Pause { .. }
                | Command::Resume
                | Command::PerformanceStatus
                | Command::FactRefreshStatus
        ) && command.requires_instance() != instance.is_some()
        {
            return Err(ActingctlError::Usage);
        }
        Ok(Self {
            state_root,
            instance,
            wait,
            command,
        })
    }
}

impl Command {
    /// The success code of this subcommand's line (Workflow #378 A5, model §7.3). `waits` is
    /// `--wait`: `request-shutdown` then saw the shutdown complete.
    const fn success(&self, waits: bool) -> ActingctlCode {
        match self {
            Self::AgentPublishFacts { .. } => ActingctlCode::AgentFactsPublished,
            Self::AgentApplyResourceTargets { .. } => ActingctlCode::ResourceTargetsApplied,
            Self::TaskOffset { .. } => ActingctlCode::TaskOffsetPublished,
            Self::RequestShutdown if waits => ActingctlCode::RuntimeShutdownCompleted,
            Self::RequestShutdown => ActingctlCode::RuntimeShutdownRequested,
            Self::InstallTransition { .. } => ActingctlCode::InstallTransitionReported,
            Self::Observe => ActingctlCode::InstanceObserved,
            Self::Reset => ActingctlCode::InstanceResetDone,
            Self::Status => ActingctlCode::RuntimeStatusReported,
            Self::StatusConfig => ActingctlCode::RuntimeConfigReported,
            Self::ConfigureSettings { .. } => ActingctlCode::ConfigSettingsChanged,
            Self::ProgramFacts { .. } => ActingctlCode::RuntimeFactsReported,
            Self::InstanceFacts { .. } => ActingctlCode::InstanceFactsReported,
            Self::MonitorStatus => ActingctlCode::MonitorStatusReported,
            Self::PerformanceStatus => ActingctlCode::RuntimePerformanceReported,
            Self::FactRefreshStatus => ActingctlCode::RuntimeFactRefreshReported,
            Self::MonitorSet { .. } => ActingctlCode::MonitorPolicySet,
            Self::MonitorClear => ActingctlCode::MonitorPolicyCleared,
            Self::EmulatorStatus => ActingctlCode::InstanceStatusReported,
            Self::EmulatorControl { .. } => ActingctlCode::InstanceEmulatorControlled,
            Self::EmulatorDiscover => ActingctlCode::InstancesDiscovered,
            Self::Pause { .. } => ActingctlCode::RuntimeSchedulingPaused,
            Self::Resume => ActingctlCode::RuntimeSchedulingResumed,
            Self::SelfCheck { .. } => ActingctlCode::InstanceSelfCheckReported,
            Self::LadderDrill { .. } => ActingctlCode::RecoveryLadderDrillQueued,
            Self::Stream { .. } => ActingctlCode::InstanceFramesCaptured,
            Self::TaskRun { .. } => ActingctlCode::TaskRunReported,
        }
    }

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
                | Self::ConfigureSettings { .. }
                | Self::ProgramFacts { .. }
                | Self::MonitorStatus
                | Self::PerformanceStatus
                | Self::FactRefreshStatus
                | Self::EmulatorDiscover
                | Self::RequestShutdown
                | Self::InstallTransition { .. }
                | Self::AgentPublishFacts { .. }
                | Self::AgentApplyResourceTargets { .. }
                | Self::SelfCheck { .. }
        )
    }
}

/// Workflow #382 A1-FLAG: the flags a subcommand of [`Invocation::parse`] takes (`emulator` and
/// `run` by their action), as its usage lists them; `None` for a word that is no such subcommand.
/// Any other flag, a flag given twice and a word that is no flag are refused by name (Fail Loud:
/// nothing is ignored silently). The offline `codes` leaves before `Invocation::parse` and reads
/// its flags here (Workflow #389 MCP-14), as `run evidence` does (Workflow #389 MCP-6).
fn flags_of(command: &str, action: Option<&str>) -> Option<&'static [&'static str]> {
    let flags: &'static [&'static str] = match command {
        // Workflow #389 MCP-14: the list flags of `codes`; its code is a positional word.
        "codes" => &["--prefix", "--owner", "--limit", "--cursor"],
        // Workflow #389 MCP-1: `events`, one flag per parameter of `ac_events`
        // (`events::PARAMETERS`); MCP-2 adds `--event-types` and `--fact-key`.
        "events" => &[
            "--state-root",
            "--instance",
            "--view",
            "--since-unix-ms",
            "--until-unix-ms",
            "--after-sequence",
            "--through-sequence",
            "--event-type",
            "--event-types",
            "--exclude-event-types",
            "--fact-key",
            "--min-severity",
            "--max-severity",
            "--origin-module",
            "--task-id",
            "--run-id",
            "--request-id",
            "--correlation-id",
            "--detail",
            "--limit",
            "--cursor",
            "--wait-s",
        ],
        // Workflow #389 MCP-6: `run evidence`, by request_id or run_id, a page of steps.
        "run" if action == Some(run_evidence::ACTION) => &[
            "--state-root",
            "--request-id",
            "--run-id",
            "--from-step",
            "--limit",
        ],
        "agent-publish-facts" => &["--state-root", "--record-file"],
        "agent-apply-resource-targets" => &["--state-root", "--policy-file"],
        "config" | "monitor-status" | "selfcheck" => &["--state-root"],
        "emulator" if action == Some("discover") => &["--state-root"],
        // Workflow #389 MCP-21: an emulator control may bound its wait.
        "emulator" if matches!(action, Some("start" | "stop" | "restart")) => {
            &["--state-root", "--instance", "--wait"]
        }
        "task-offset" | "observe" | "reset" | "monitor-clear" | "emulator" | "resume" => {
            &["--state-root", "--instance"]
        }
        "request-shutdown" => &["--state-root", "--wait"],
        "install-transition" => &["--state-root", "--action-json", "--wait"],
        "status" => &["--state-root", "--config"],
        // Workflow #385 P-m: the shadow measurement's status, every row or one instance's.
        "perf-status" => &["--state-root", "--instance"],
        // Workflow #384 D2-S5: what the fact refresh waits for, every row or one instance's.
        "fact-refresh" => &["--state-root", "--instance"],
        "facts" => &["--state-root", "--program", "--instance", "--key-prefix"],
        // Workflow #390 DRILL-1: a ladder drill acts on the device only with `--device`.
        "ladder-drill" => &["--state-root", "--instance", "--device"],
        "monitor-set" => &[
            "--state-root",
            "--instance",
            "--interval-ms",
            "--expect",
            "--recover",
        ],
        "pause" => &[
            "--state-root",
            "--instance",
            "--reason",
            "--drain-timeout-ms",
        ],
        "stream" => &[
            "--state-root",
            "--instance",
            "--max-frames",
            "--interval-ms",
        ],
        // Workflow #336 PKG-S4 (option 丙): a package by its path only; a digest flag
        // (`--expected-sha256`, `--package-ref` and their `--recovery-*` forms) is refused as
        // an unknown flag.
        "task-run" => &[
            "--state-root",
            "--instance",
            "--package",
            "--recovery-package",
            // Workflow #389 MCP-21.
            "--wait",
        ],
        // Workflow #398 RD-1c: `devices`; the alias and the handle are positional words.
        "devices" if matches!(action, Some(devices::LIST | devices::BIND)) => {
            &["--state-root", "--root"]
        }
        "devices" if action == Some(devices::UNBIND) => &["--state-root"],
        // Workflow #410 item 6: both texts are required; no root is read.
        capability_gap::COMMAND => &["--capability", "--why"],
        // Workflow #411 B15 R6: `remedy set` writes one scope's rows, an instance's or a game's
        // (no `--server`: a server id is not game-qualified); `get` and `lift` take one instance.
        remedy::COMMAND if action == Some(remedy::SET) => {
            &["--state-root", "--instance", "--game", "--rows"]
        }
        remedy::COMMAND if matches!(action, Some(remedy::GET | remedy::LIFT)) => {
            &["--state-root", "--instance"]
        }
        _ => return None,
    };
    Some(flags)
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

/// Why a subcommand's bounded input file (`--record-file`, `--policy-file`) cannot be used.
#[derive(Debug)]
enum InputProblem {
    /// Opening (`IoOp::Open`) or reading (`IoOp::Read`) it failed.
    Io(io::Error, IoOp),
    /// It holds no byte.
    Empty,
    /// It is larger than its bound, in bytes.
    TooLarge(usize),
    /// Its text is not UTF-8.
    NotUtf8(std::string::FromUtf8Error),
    /// Its JSON does not decode.
    Json(serde_json::Error),
    /// The contract refuses its content.
    Invalid(SanitizationError),
}

impl InputProblem {
    /// `top` with what this problem adds: the I/O or decode failure as its `caused_by` link
    /// (`foreign_os_error`, `foreign_data_error`), the size, or the contract's refusal code.
    fn attach(&self, top: Outcome) -> Outcome {
        match self {
            Self::Io(error, op) => top.caused_by(Outcome::from_io_error(error, *op)),
            Self::Empty => top.actual_bytes(0_u64),
            Self::TooLarge(limit) => top.limit_bytes(*limit),
            Self::NotUtf8(error) => top.caused_by(foreign_data_error(error)),
            Self::Json(error) => top.caused_by(foreign_data_error(error)),
            Self::Invalid(error) => top.caused_by(Outcome::new(error.code())),
        }
    }
}

/// `foreign_data_error` for JSON text a decoder refused: the decoder's message verbatim as
/// `raw_text` (spec §7).
fn foreign_data_error(error: &impl fmt::Display) -> Outcome {
    Outcome::new(ContractCode::ForeignDataError)
        .raw_source(RawSource::Json)
        .raw_text(error.to_string())
}

#[derive(Debug)]
enum ActingctlError {
    Usage,
    /// Workflow #382 A1-FLAG: a command-line word the subcommand refuses, and why (a flag it
    /// does not take or a stray word: `unknown`; a flag given twice: `repeated`; the second
    /// spelling of one value: `conflicting`).
    Argument {
        argument: String,
        problem: ArgumentProblem,
    },
    Runtime(actingcommand_runtime_client::RuntimeClientError),
    /// `task-run`: the package (or recovery package) path does not canonicalize.
    Package {
        path: PathBuf,
        error: io::Error,
    },
    /// `agent-publish-facts`: the bounded fact observation file cannot be used.
    FactRecord {
        path: PathBuf,
        problem: InputProblem,
    },
    /// `agent-apply-resource-targets`: the policy file is unreadable, empty, larger than the
    /// document bound or not UTF-8.
    ResourceTargetsFile {
        path: PathBuf,
        problem: InputProblem,
    },
    /// The runtime status lists no instance with this alias.
    InstanceUnknown(String),
    /// Workflow #389 MCP-4: more than one instance of the runtime status matches this selector.
    InstanceAmbiguous(String),
    /// `task-offset` without `--instance`: the registered instances do not name exactly one
    /// configured game.
    TaskOffsetScopeAmbiguous,
    /// `task-offset`: the task identifier cannot form a valid priority offset record.
    PriorityOffsetInvalid(SanitizationError),
    /// The system clock is before the Unix epoch or out of range.
    Clock,
    /// `status --config`: the snapshot carries no `config.subsystems` or no
    /// `config.parameters.<section>`.
    ConfigFactsMissing,
    /// A result does not encode as JSON (developer line only, model §7.3).
    Output(serde_json::Error),
    /// `--wait` after an accepted shutdown: the wait's failure (`shutdown_wait_timeout`,
    /// `shutdown_wait_journal_unreadable`) with the OS failures as `foreign_*` links.
    ShutdownWait(Outcome),
}

impl ActingctlError {
    fn argument(argument: &str, problem: ArgumentProblem) -> Self {
        Self::Argument {
            argument: argument.to_owned(),
            problem,
        }
    }

    /// This failure as one outcome (model §7.3): a Runtime client failure as the client gives
    /// it (the refused receipt's outcome on top), every other kind as actingctl's code with its
    /// values and its I/O or decode cause as a link.
    fn outcome(&self) -> Outcome {
        match self {
            Self::Usage => Outcome::new(ActingctlCode::ActingctlUsageInvalid),
            Self::Argument { argument, problem } => {
                cli_line::usage_invalid(Some(argument.as_str()), *problem)
            }
            Self::Runtime(error) => error.outcome(),
            Self::Package { path, error } => {
                Outcome::new(ActingctlCode::ActingctlPackageResolutionFailed)
                    .path(path)
                    .caused_by(Outcome::from_io_error(error, IoOp::Canonicalize))
            }
            Self::FactRecord { path, problem } => {
                problem.attach(Outcome::new(ActingctlCode::ActingctlFactRecordInvalid).path(path))
            }
            Self::ResourceTargetsFile { path, problem } => {
                problem.attach(Outcome::new(ActingctlCode::ResourceTargetsFileInvalid).path(path))
            }
            Self::InstanceUnknown(alias) => SelectorMiss::Unknown.outcome(alias),
            Self::InstanceAmbiguous(selector) => SelectorMiss::Ambiguous.outcome(selector),
            Self::TaskOffsetScopeAmbiguous => Outcome::new(ActingctlCode::TaskOffsetScopeAmbiguous),
            Self::PriorityOffsetInvalid(error) => Outcome::new(SharedCode::PriorityOffsetInvalid)
                .caused_by(Outcome::new(error.code())),
            Self::Clock => Outcome::new(ActingctlCode::ClockUnavailable),
            Self::ConfigFactsMissing => Outcome::new(ActingctlCode::ConfigFactsMissing),
            Self::Output(_) => Outcome::new(ActingctlCode::ActingctlOutputFailed),
            Self::ShutdownWait(outcome) => outcome.clone(),
        }
    }
}

/// The developer text of the stderr line; the outcome before it already names the code and
/// its values.
impl fmt::Display for ActingctlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage | Self::Argument { .. } => formatter.write_str(USAGE),
            Self::Runtime(error) => error.fmt(formatter),
            Self::Package { .. } => {
                formatter.write_str("the contained task package cannot be resolved")
            }
            Self::FactRecord { .. } => formatter.write_str(
                "the fact observation file is unreadable, larger than its bound or invalid",
            ),
            Self::ResourceTargetsFile { .. } => formatter.write_str(
                "the policy file is unreadable, empty, larger than 65536 bytes or not UTF-8",
            ),
            Self::InstanceUnknown(_) => {
                formatter.write_str("the runtime status lists no instance with that alias")
            }
            Self::InstanceAmbiguous(_) => formatter
                .write_str("more than one instance of the runtime status matches that selector"),
            Self::TaskOffsetScopeAmbiguous => formatter.write_str(
                "the registered instances do not name exactly one configured game; pass --instance <alias>",
            ),
            Self::PriorityOffsetInvalid(_) => {
                formatter.write_str("the task identifier cannot form a priority offset fact")
            }
            Self::Clock => formatter
                .write_str("the system clock is before the Unix epoch or out of range"),
            Self::ConfigFactsMissing => formatter.write_str(
                "the runtime fact snapshot holds no config.subsystems or no config.parameters.<section> record",
            ),
            Self::Output(error) => write!(formatter, "the JSON result cannot be encoded: {error}"),
            Self::ShutdownWait(_) => {
                formatter.write_str("the accepted shutdown was not seen to complete")
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
    fn contained_task_command_takes_a_package_path_and_runtime_instance_only() {
        let args = [
            "task-run",
            "--state-root",
            "state",
            "--instance",
            "neutral.instance",
            "--package",
            "neutral-task.zip",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert!(Invocation::parse(args).is_ok());
    }

    #[test]
    fn task_run_request_uses_maximum_bounded_response_deadline() {
        let request =
            task_run_request("neutral-task.zip".to_string(), None).expect("task-run request");

        assert_eq!(
            request.response_deadline_ms(),
            ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS
        );
        assert_eq!(request.response_deadline_ms(), 1_800_000);
    }

    // Test class: specification criterion. Task Contract: https://github.com/HS7097/ActingCommand-Workflow/issues/241#issuecomment-5491623342
    // Workflow #336 PKG-S4: the recovery package is one flag, its path.
    #[test]
    fn task_run_recovery_binding_is_the_recovery_package_path() {
        let complete = [
            "task-run",
            "--state-root",
            "state",
            "--instance",
            "neutral.instance",
            "--package",
            "neutral-task.zip",
            "--recovery-package",
            "return-home.zip",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert!(Invocation::parse(complete).is_ok());
    }

    // Test class: specification criterion. Task Contract: https://github.com/HS7097/ActingCommand-Workflow/issues/241#issuecomment-5491623342
    #[test]
    fn task_run_request_preserves_the_typed_recovery_identity() {
        let request = task_run_request(
            "neutral-task.zip".to_string(),
            Some("return-home.zip".to_string()),
        )
        .expect("task-run request");
        let recovery = request.recovery().expect("typed recovery binding");

        assert_eq!(recovery.package_path(), "return-home.zip");
    }

    // Workflow #336 PKG-S4 (option 丙), model M-PKG v1.1 §4.5 S4-T3: `task-run` names its
    // package (and its recovery package) by its path only; a package digest flag is a usage
    // error naming that flag, never ignored.
    #[test]
    fn s4_t3_task_run_refuses_a_package_digest_flag() {
        let words = |extra: &[&str]| -> Vec<OsString> {
            [
                "task-run",
                "--state-root",
                "state",
                "--instance",
                "neutral.instance",
                "--package",
                "neutral-task.zip",
                "--recovery-package",
                "return-home.zip",
            ]
            .iter()
            .chain(extra)
            .map(|word| OsString::from(*word))
            .collect()
        };
        assert!(Invocation::parse(words(&[])).is_ok());
        let digest = "0".repeat(64);
        for flag in [
            "--package-ref",
            "--expected-sha256",
            "--recovery-package-ref",
            "--recovery-expected-sha256",
        ] {
            let error = Invocation::parse(words(&[flag, digest.as_str()]))
                .err()
                .unwrap_or_else(|| panic!("{flag} was taken"));
            assert_eq!(
                error.outcome(),
                cli_line::usage_invalid(Some(flag), ArgumentProblem::Unknown),
                "{flag}"
            );
        }
    }

    /// Workflow #378 A5 (model §7.3): every subcommand's line has its own success code
    /// (category success; `request-shutdown --wait` the completed shutdown's), and every local
    /// failure its own error code with its I/O, decode or contract cause as the first link; a
    /// Runtime client failure is the client's outcome unchanged.
    #[test]
    fn every_command_has_its_success_code_and_every_failure_its_code() {
        let commands: [(&[&str], &str); 27] = [
            (
                &["agent-publish-facts", "--record-file", "observation.json"],
                "agent_facts_published",
            ),
            (
                &[
                    "agent-apply-resource-targets",
                    "--policy-file",
                    "policy.json",
                ],
                "resource_targets_applied",
            ),
            (
                &["task-offset", "neutral.task", "250"],
                "task_offset_published",
            ),
            (&["request-shutdown"], "runtime_shutdown_requested"),
            (
                &["request-shutdown", "--wait", "60"],
                "runtime_shutdown_completed",
            ),
            (
                &[
                    "install-transition",
                    "--action-json",
                    r#"{"action":"query","transition_id":"transition-a5"}"#,
                ],
                "install_transition_reported",
            ),
            (
                &["observe", "--instance", "neutral.instance"],
                "instance_observed",
            ),
            (
                &["reset", "--instance", "neutral.instance"],
                "instance_reset_done",
            ),
            (&["status"], "runtime_status_reported"),
            (&["status", "--config"], "runtime_config_reported"),
            (
                &["config", "set", "neutral.key=1"],
                "config_settings_changed",
            ),
            (&["facts", "--program"], "runtime_facts_reported"),
            (
                &["facts", "--instance", "neutral.instance"],
                "instance_facts_reported",
            ),
            (&["monitor-status"], "monitor_status_reported"),
            (&["perf-status"], "runtime_performance_reported"),
            (&["fact-refresh"], "runtime_fact_refresh_reported"),
            (
                &["monitor-set", "--instance", "neutral.instance"],
                "monitor_policy_set",
            ),
            (
                &["monitor-clear", "--instance", "neutral.instance"],
                "monitor_policy_cleared",
            ),
            (
                &["emulator", "status", "--instance", "neutral.instance"],
                "instance_status_reported",
            ),
            (
                &["emulator", "restart", "--instance", "neutral.instance"],
                "instance_emulator_controlled",
            ),
            (&["emulator", "discover"], "instances_discovered"),
            (&["pause"], "runtime_scheduling_paused"),
            (&["resume"], "runtime_scheduling_resumed"),
            (
                &["selfcheck", "neutral.instance"],
                "instance_self_check_reported",
            ),
            // Workflow #390 DRILL-1.
            (
                &["ladder-drill", "--instance", "neutral.instance", "--device"],
                "recovery_ladder_drill_queued",
            ),
            (
                &["stream", "--instance", "neutral.instance"],
                "instance_frames_captured",
            ),
            (
                &[
                    "task-run",
                    "--instance",
                    "neutral.instance",
                    "--package",
                    "neutral-task.zip",
                ],
                "task_run_reported",
            ),
        ];
        for (words, expected) in commands {
            let arguments = words
                .iter()
                .chain(&["--state-root", "state"])
                .map(|word| OsString::from(*word))
                .collect();
            let invocation =
                Invocation::parse(arguments).unwrap_or_else(|error| panic!("{words:?}: {error}"));
            let success = invocation.command.success(invocation.wait.is_some());
            assert_eq!(
                (success.as_str(), success.category()),
                (expected, Category::Success),
                "{words:?}"
            );
        }

        let Err(client_error) = RuntimeClient::connect(RuntimeClientConfig::new(
            "",
            actingcommand_runtime_client::RuntimeChannel::Cli,
            EventActor::Cli,
            EventSource::Cli,
        )) else {
            panic!("a client without a state root is refused before any I/O");
        };
        assert_eq!(
            ActingctlError::Runtime(client_error.clone()).outcome(),
            client_error.outcome()
        );
        let refusal = FactObservation {
            records: Vec::new(),
        }
        .validate()
        .expect_err("an empty observation is refused");
        let refusal_code = refusal.code().as_str();
        let decode = || serde_json::from_str::<Value>("{").expect_err("truncated JSON");
        let failures = [
            (ActingctlError::Usage, "actingctl_usage_invalid", None),
            (
                ActingctlError::Package {
                    path: PathBuf::from("neutral-task.zip"),
                    error: io::Error::from(io::ErrorKind::NotFound),
                },
                "actingctl_package_resolution_failed",
                Some("foreign_os_error"),
            ),
            (
                ActingctlError::FactRecord {
                    path: PathBuf::from("observation.json"),
                    problem: InputProblem::Json(decode()),
                },
                "actingctl_fact_record_invalid",
                Some("foreign_data_error"),
            ),
            (
                ActingctlError::ResourceTargetsFile {
                    path: PathBuf::from("policy.json"),
                    problem: InputProblem::Empty,
                },
                "resource_targets_file_invalid",
                None,
            ),
            (
                ActingctlError::InstanceUnknown("neutral.instance".to_owned()),
                "instance_unknown",
                None,
            ),
            (
                ActingctlError::InstanceAmbiguous("neutral.instance".to_owned()),
                "instance_ambiguous",
                None,
            ),
            (
                ActingctlError::TaskOffsetScopeAmbiguous,
                "task_offset_scope_ambiguous",
                None,
            ),
            (
                ActingctlError::PriorityOffsetInvalid(refusal),
                "priority_offset_invalid",
                Some(refusal_code),
            ),
            (ActingctlError::Clock, "clock_unavailable", None),
            (
                ActingctlError::ConfigFactsMissing,
                "config_facts_missing",
                None,
            ),
            (
                ActingctlError::Output(decode()),
                "actingctl_output_failed",
                None,
            ),
            (
                ActingctlError::ShutdownWait(
                    Outcome::new(ActingctlCode::ShutdownWaitTimeout)
                        .pid(4_242_u32)
                        .timeout_ms(Duration::from_secs(60)),
                ),
                "shutdown_wait_timeout",
                None,
            ),
        ];
        for (error, expected, expected_cause) in failures {
            let outcome = error.outcome();
            let cause = outcome
                .causes()
                .first()
                .map(|link| link.outcome().code().as_str());
            assert_eq!(
                (outcome.code().as_str(), outcome.category(), cause),
                (expected, Category::Error, expected_cause),
                "{error:?}"
            );
        }
    }

    // Workflow #389 MCP-21 (MCP-0 command table v1.1 §4.22): `--wait <s>` (1-3600) is taken by
    // `emulator start|stop|restart` and by `task-run`. A wait that ended before its request is
    // `command_wait_expired` (error) with the request's operation, its request_id and the time
    // waited, beside `{request_id, correlation_id, state: in_progress}`, and the line exits
    // non-zero.
    #[test]
    fn mcp21_wait_bounds_an_emulator_control_or_a_task_run_and_its_expiry_names_the_request() {
        let words_of = |words: &[&str]| -> Vec<OsString> {
            words
                .iter()
                .chain(&["--state-root", "state"])
                .map(|word| OsString::from(*word))
                .collect()
        };
        for words in [
            &[
                "emulator",
                "start",
                "--instance",
                "neutral.instance",
                "--wait",
                "1",
            ][..],
            &[
                "emulator",
                "stop",
                "--instance",
                "neutral.instance",
                "--wait",
                "60",
            ],
            &[
                "emulator",
                "restart",
                "--instance",
                "neutral.instance",
                "--wait",
                "3600",
            ],
            &[
                "task-run",
                "--instance",
                "neutral.instance",
                "--package",
                "neutral-task.zip",
                "--wait",
                "60",
            ],
        ] {
            let invocation = Invocation::parse(words_of(words))
                .unwrap_or_else(|error| panic!("{words:?}: {error}"));
            assert!(invocation.wait.is_some(), "{words:?}");
        }
        for seconds in ["0", "3601"] {
            assert!(
                Invocation::parse(words_of(&[
                    "emulator",
                    "start",
                    "--instance",
                    "neutral.instance",
                    "--wait",
                    seconds,
                ]))
                .is_err(),
                "--wait {seconds}"
            );
        }

        let issuer = actingcommand_contract::IdentifierIssuer::new().expect("identifier issuer");
        let request_id = *issuer.mint_request_id().expect("request id").transport();
        let correlation_id = *issuer
            .mint_correlation_id()
            .expect("correlation id")
            .transport();
        let expired = wait_expired(
            ContractLocation::RunContainedTask,
            Duration::from_secs(60),
            Some(request_id),
            Some(correlation_id),
        );
        assert!(expired.refused, "an expired wait exits non-zero");
        let outcome = &expired.outcome;
        assert_eq!(
            (outcome.code().as_str(), outcome.category()),
            ("command_wait_expired", Category::Error)
        );
        assert_eq!(
            (
                outcome.values().get("operation"),
                outcome.values().get("request_id"),
                outcome.values().get("waited_ms"),
            ),
            (
                Some(&serde_json::json!(
                    ContractLocation::RunContainedTask.location().as_str()
                )),
                Some(&serde_json::json!(request_id)),
                Some(&serde_json::json!(60_000)),
            ),
            "{outcome}"
        );
        assert_eq!(
            Value::Object(expired.members.clone()),
            serde_json::json!({
                "request_id": request_id,
                "correlation_id": correlation_id,
                "state": "in_progress",
            })
        );
    }

    // Workflow #390 九处-1 (board #390 comment 6095942731, item 1; corrected ruling 4 of wave
    // 18): `actingctl status` shows, beside each instance's status members, whether its emulator
    // runs as the Runtime last read it: `on` or `off` with the reading's time, and `unknown`,
    // never `off`, when the Runtime holds no reading.
    #[test]
    fn nine1_status_shows_each_emulator_as_last_read_with_the_time() {
        use actingcommand_contract::{
            EmulatorRunningState, IdentifierIssuer, RuntimeControlPlaneStatus,
            RuntimeInstanceStatus,
        };
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let instance = |alias: &str, emulator_state: EmulatorRunningState| {
            RuntimeInstanceStatus::new(
                alias,
                *issuer.mint_instance_id().expect("instance id").transport(),
                false,
                0,
                false,
                false,
                false,
            )
            .expect("instance status")
            .with_emulator_state(emulator_state)
        };
        let status = RuntimeControlPlaneStatus::new(
            *issuer.mint_owner_epoch().expect("owner epoch").transport(),
            vec![
                instance(
                    "neutral.on",
                    EmulatorRunningState::On {
                        observed_at_unix_ms: 1_000,
                    },
                ),
                instance(
                    "neutral.off",
                    EmulatorRunningState::Off {
                        observed_at_unix_ms: 2_000,
                    },
                ),
                instance("neutral.unread", EmulatorRunningState::Unknown),
            ],
        )
        .expect("runtime status");
        let line = reported(&status, ActingctlCode::RuntimeStatusReported).expect("status line");
        let shown = line.members["instances"]
            .as_array()
            .expect("the status' instances")
            .iter()
            .map(|row| (row["instance_alias"].clone(), row["emulator_state"].clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            shown,
            [
                (
                    serde_json::json!("neutral.off"),
                    serde_json::json!({"state": "off", "observed_at_unix_ms": 2_000}),
                ),
                (
                    serde_json::json!("neutral.on"),
                    serde_json::json!({"state": "on", "observed_at_unix_ms": 1_000}),
                ),
                (
                    serde_json::json!("neutral.unread"),
                    serde_json::json!({"state": "unknown"}),
                ),
            ]
        );
    }
}
