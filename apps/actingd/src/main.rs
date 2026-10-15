// SPDX-License-Identifier: AGPL-3.0-only

//! Thin process adapter for the resident ActingCommand Runtime.

#![forbid(unsafe_code)]

mod check_config;
mod codes;
mod config;
mod ledger_maintenance;
mod monitor_retry;
mod owner_unlock;
mod suspended;

// Test-only: the shared sealed C4 fixture support, reused for its 16x9 fake-device fixtures.
#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/c4_runtime.rs"]
mod c4_support;
// Test-only: Workflow #381 A, the last daemon lines of a stopped held start (test plan A-2 G3b).
#[cfg(test)]
mod gate_381a;
// Test-only: Workflow #397 MON-1, a receipt-header timeout of the policy monitor's own request.
#[cfg(test)]
mod gate_397;
// Test-only: Workflow #378 A2a, registered refusal codes and the env-override warning.
#[cfg(test)]
mod gate_a2a;
// Test-only: Workflow #383 AO-7, the policy monitor wakes the driver from host notifications.
#[cfg(test)]
mod gate_383;
// Test-only: Workflow #378 A6a, daemon lines, the fallback log and the subcommand reports.
#[cfg(test)]
mod gate_a6a;
// Test-only: Workflow #382 A-1 (test plan v2.2 §3.7), actingd's subcommand arguments.
#[cfg(test)]
mod gate_382_a1;

use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{
    Category, CauseRelation, Code, Detail, IoOp, Location, Outcome,
};
use actingcommand_contract::{
    PolicyExecutionEventData, RunId, RuntimeErrorCode, RuntimeErrorProjection, RuntimeReceipt,
    SchedulingOutcomeProjection,
};
use actingcommand_policy::MAX_TASKS;
use actingcommand_runtime_client::ClientCode;
use actingcommand_runtime_host::{
    LedgerFailureSink, PolicyAdmissionContext, PolicyCadence, PolicyCycle, PolicyDispatchAdmission,
    PolicyRecomputeKind, PolicyRunContext, PolicyTrigger, PolicyWakes, RuntimeHost,
    RuntimeHostError, RuntimeLifecycleFailure, RuntimeLifecycleFailureStage,
};
use codes::{ActingdCode, ActingdLocation};
use config::{PolicyBootstrap, RuntimeAssembly, ScheduledExecutionMode};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::error::Error;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLICY_EVENT_WAIT_MS: u64 = 250;
const MAX_POLICY_CYCLE_DURATION: Duration = Duration::from_secs(10 * 60);

fn main() -> ExitCode {
    let arguments: Vec<std::ffi::OsString> = env::args_os().skip(1).collect();
    let installed = |purpose| {
        actingcommand_contract::process_installation_for(purpose)
            .map(|_| ())
            .map_err(|error| ActingdError::config(error.code()))
    };
    let candidate = actingcommand_contract::InstallConfigPurpose::CandidateCheck;
    let running = actingcommand_contract::InstallConfigPurpose::Running;
    let subcommand = arguments
        .first()
        .and_then(|argument| argument.to_str())
        .map(str::to_owned);
    let category = match subcommand.as_deref() {
        Some("check-config") => subcommand_report(
            check_config::SCHEMA_VERSION,
            ActingdCode::CheckConfigReportEncodeFailed,
            installed(candidate).and_then(|()| check_config::run(arguments)),
        ),
        Some("ledger-maintenance") => {
            let purpose = if arguments
                .get(1)
                .is_some_and(|argument| argument == "verify")
            {
                candidate
            } else {
                running
            };
            subcommand_report(
                ledger_maintenance::SCHEMA_VERSION,
                ActingdCode::MaintenanceReceiptEncodeFailed,
                installed(purpose).and_then(|()| ledger_maintenance::run(arguments)),
            )
        }
        Some("unlock-owner") => subcommand_report(
            owner_unlock::SCHEMA_VERSION,
            ActingdCode::UnlockOwnerReportEncodeFailed,
            installed(running).and_then(|()| owner_unlock::run(arguments)),
        ),
        Some("suspended") => subcommand_report(
            suspended::SCHEMA_VERSION,
            ActingdCode::SuspendedReportEncodeFailed,
            installed(running).and_then(|()| suspended::run(arguments)),
        ),
        _ => run_daemon(arguments),
    };
    ExitCode::from(category.exit_code())
}

/// Workflow #383 AO-1 (`contracts/ledger-observer.md` §9.1, §9.2 step 4): the daemon's side of
/// the ledger's write path. A write that was read back, retried and committed is a warning
/// daemon line; the stop after the ledger wrote its dump (or could not) is `runtime_stopped`
/// and the stopping condition as the last daemon line (Workflow #378 A6a, model section 7.5),
/// and the process exits by that condition's category: 2, as `ledger_write_failed` and
/// `ledger_dump_failed` are fatal (model section 7.6). The watchdog does not restart a fatal
/// stop; a person repairs the disk or file, and the next start imports the dump.
struct DaemonLedgerFailure;

impl LedgerFailureSink for DaemonLedgerFailure {
    fn write_retried(&self, outcome: &Outcome) {
        write_stderr(&daemon_lines(outcome, None));
    }

    fn stop(&self, outcome: &Outcome) {
        let stopped = Outcome::new(ActingdCode::RuntimeStopped);
        write_stderr(&format!(
            "{}{}",
            daemon_lines(&stopped, None),
            daemon_lines(outcome, None)
        ));
        std::process::exit(i32::from(outcome.category().exit_code()));
    }
}

/// The developer-line level of a category (model section 7.4): `INFO` for a success or an info.
const fn level(category: Category) -> &'static str {
    match category {
        Category::Success | Category::Info => "INFO",
        Category::Warning => "WARN",
        Category::Error => "ERROR",
        Category::Fatal => "FATAL",
    }
}

/// The developer line `<LEVEL> actingd: <code> <text>` of an outcome (model section 7.4): the
/// text is `developer`, else the outcome's values; it is kept on one line.
fn developer_line(outcome: &Outcome, developer: Option<&str>) -> String {
    let line = match developer {
        Some(text) => format!(
            "{} actingd: {} {text}",
            level(outcome.category()),
            outcome.code()
        ),
        None => format!("{} actingd: {outcome}", level(outcome.category())),
    };
    format!("{}\n", line.replace(['\r', '\n'], " "))
}

/// actingd's daemon lines of one outcome (model section 7.4): its developer line, then
/// `OUTCOME actingd: <envelope>` with the full envelope. Written together in one write.
fn daemon_lines(outcome: &Outcome, developer: Option<&str>) -> String {
    let developer = developer_line(outcome, developer);
    match serde_json::to_string(&outcome.envelope(Detail::Full)) {
        Ok(envelope) => format!("{developer}OUTCOME actingd: {envelope}\n"),
        Err(_) => developer,
    }
}

/// Writes `text` to stderr in one write: stderr is the last place actingd reports to, so a
/// failed write there has nowhere else to go.
fn write_stderr(text: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(text.as_bytes());
    let _ = stderr.flush();
}

/// A subcommand's result when it succeeded (Workflow #378 A6a, model section 7.4): its success
/// outcome and the data members of its report.
#[derive(Debug)]
struct Reported {
    outcome: Outcome,
    data: serde_json::Map<String, serde_json::Value>,
}

impl Reported {
    fn new(outcome: Outcome) -> Self {
        Self {
            outcome,
            data: serde_json::Map::new(),
        }
    }

    fn with(mut self, member: &str, value: serde_json::Value) -> Self {
        self.data.insert(member.to_owned(), value);
        self
    }
}

/// The detail of a subcommand report (model section 7.1, class b): full, or only the registered
/// names with `ACTINGCOMMAND_DETAIL=codes`.
fn report_detail() -> Detail {
    if env::var_os("ACTINGCOMMAND_DETAIL").is_some_and(|value| value == "codes") {
        Detail::Codes
    } else {
        Detail::Full
    }
}

/// Prints a subcommand's one stdout report line (Workflow #378 A6a, model section 7.4), then the
/// developer line on stderr of an outcome that is a warning, an error or fatal. Returns the
/// category the process exits by (model section 7.6).
fn subcommand_report(
    schema_version: &'static str,
    encode_failed: ActingdCode,
    result: Result<Reported, ActingdError>,
) -> Category {
    let (outcome, developer, line) =
        report_line(schema_version, encode_failed, result, report_detail());
    let mut stdout = std::io::stdout().lock();
    let printed = writeln!(stdout, "{line}").and_then(|()| stdout.flush());
    drop(stdout);
    if !matches!(outcome.category(), Category::Success | Category::Info) {
        write_stderr(&developer_line(&outcome, developer.as_deref()));
    }
    match printed {
        Ok(()) => outcome.category(),
        // The report is the interface; one that was not printed is the subcommand's failure.
        Err(_) => {
            let failed = Outcome::new(encode_failed);
            write_stderr(&developer_line(&failed, None));
            failed.category()
        }
    }
}

/// A subcommand's report line `{schema_version, outcome, …data}` for every exit, usage and
/// refusals included, with its outcome and developer text. A report that cannot be encoded is
/// reported as `encode_failed` instead, with the outcome as its link.
fn report_line(
    schema_version: &'static str,
    encode_failed: ActingdCode,
    result: Result<Reported, ActingdError>,
    detail: Detail,
) -> (Outcome, Option<String>, String) {
    let (outcome, data, developer) = match result {
        Ok(Reported { outcome, data }) => (outcome, data, None),
        Err(error) => (
            error.outcome(),
            error.report_data(),
            Some(error.to_string()),
        ),
    };
    let encode = |outcome: &Outcome, data: serde_json::Map<String, serde_json::Value>| {
        let mut report = serde_json::Map::new();
        report.insert(
            "schema_version".to_owned(),
            serde_json::Value::String(schema_version.to_owned()),
        );
        report.insert(
            "outcome".to_owned(),
            serde_json::to_value(outcome.envelope(detail)).ok()?,
        );
        report.extend(data);
        serde_json::to_string(&serde_json::Value::Object(report)).ok()
    };
    match encode(&outcome, data) {
        Some(line) => (outcome, developer, line),
        None => {
            let failed = Outcome::new(encode_failed).joined(CauseRelation::CausedBy, outcome);
            let line = encode(&failed, serde_json::Map::new()).unwrap_or_default();
            (failed, None, line)
        }
    }
}

/// The daemon (Workflow #378 A6a, model sections 7.4-7.6, 8.2): a refusal before the Runtime
/// host starts is written by the start-refusal writer; once it started, each lifecycle outcome
/// is a daemon line, and the last one is the terminal outcome: `runtime_stopped` on a requested
/// stop, else `runtime_stopped` followed by the stopping condition. The process exits by the
/// terminal outcome's category.
fn run_daemon(arguments: Vec<std::ffi::OsString>) -> Category {
    let installation = match actingcommand_contract::process_installation_for(
        actingcommand_contract::InstallConfigPurpose::Running,
    ) {
        Ok(installation) => installation,
        Err(error) => return start_refused(&ActingdError::config(error.code()), None),
    };
    let install_root = installation.map(actingcommand_contract::InstalledProcess::root);
    let (host, policy) = match start_daemon(arguments) {
        Ok(started) => started,
        // An accepted shutdown of a held start is a requested stop, which the watchdog never
        // restarts (`contracts/runtime-watchdog.md`, rows 5 and 6): its terminal outcome is
        // `runtime_stopped`, with the shutdown as its link.
        Err(error)
            if error
                .code
                .is(actingcommand_runtime_host::HostCode::InstallStartupShutDown) =>
        {
            let stopped = Outcome::new(ActingdCode::RuntimeStopped)
                .joined(CauseRelation::CausedBy, error.outcome());
            write_stderr(&daemon_lines(&stopped, Some(&error.to_string())));
            return stopped.category();
        }
        Err(error) => return start_refused(&error, install_root),
    };
    let mut lines = daemon_lines(host.ledger_opened(), None);
    // Slice #315-B2c-2: the same decision the Ledger records as a lifecycle observation.
    if let Some(released) = host.owner_released_by_exit() {
        lines.push_str(&daemon_lines(
            &released.outcome(),
            Some(&released.to_string()),
        ));
    }
    // Workflow #361 B1: each scheduling pause the start restored or dropped.
    for pause in host.scheduling_pause_restore() {
        lines.push_str(&daemon_lines(pause, None));
    }
    write_stderr(&lines);
    let stopped = Outcome::new(ActingdCode::RuntimeStopped);
    match serve(host, policy) {
        Ok(()) => {
            write_stderr(&daemon_lines(&stopped, None));
            stopped.category()
        }
        Err(error) => {
            let condition = error.outcome();
            write_stderr(&format!(
                "{}{}",
                daemon_lines(&stopped, None),
                daemon_lines(&condition, Some(&error.to_string()))
            ));
            condition.category()
        }
    }
}

/// Model section 8.2 (F8, H1, N4): a start refusal before the ledger opens is the refusal's
/// daemon lines on stderr, with `install_root` once actingd resolved its installation. When
/// stderr is a console or absent, nobody keeps it, so the same lines are appended to the
/// fallback log `%LOCALAPPDATA%\ActingCommand\logs\actingd-<ms>.log` too, which every root's
/// watchdog scans. A failed append is one more developer line on stderr; the exit still
/// follows the refusal's category. No ledger is opened for a refusal.
fn start_refused(error: &ActingdError, install_root: Option<&Path>) -> Category {
    let mut stderr = std::io::stderr();
    let now_unix_ms = system_unix_ms().unwrap_or_default();
    write_start_refusal(
        error,
        install_root,
        &mut stderr,
        stderr_unkept(),
        fallback_log_directory().as_deref(),
        now_unix_ms,
    )
}

/// [`start_refused`] with its sinks: `stderr`, whether nobody keeps it (`unkept`), the fallback
/// log directory (`None` when `LOCALAPPDATA` is not set) and the time that names the log.
fn write_start_refusal(
    error: &ActingdError,
    install_root: Option<&Path>,
    stderr: &mut dyn Write,
    unkept: bool,
    fallback_directory: Option<&Path>,
    now_unix_ms: u64,
) -> Category {
    let outcome = error.outcome();
    let outcome = match install_root {
        Some(root) => outcome.install_root(root),
        None => outcome,
    };
    let lines = daemon_lines(&outcome, Some(&error.to_string()));
    let _ = stderr.write_all(lines.as_bytes());
    if unkept && let Err(failed) = append_fallback_log(fallback_directory, now_unix_ms, &lines) {
        let _ = stderr.write_all(developer_line(&failed, None).as_bytes());
    }
    let _ = stderr.flush();
    outcome.category()
}

/// Whether nobody keeps actingd's stderr (model section 8.2, H1): there is no stderr handle, or
/// it is a console. A file or a pipe is kept by its starter. std's console check stands in for
/// `GetFileType` returning `FILE_TYPE_CHAR`, as actingd forbids unsafe code.
fn stderr_unkept() -> bool {
    let stderr = std::io::stderr();
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        if stderr.as_raw_handle().is_null() {
            return true;
        }
    }
    std::io::IsTerminal::is_terminal(&stderr)
}

/// `%LOCALAPPDATA%\ActingCommand\logs`, the console's log directory, which every root's watchdog
/// scans; `None` when `LOCALAPPDATA` is not set.
fn fallback_log_directory() -> Option<PathBuf> {
    env::var_os("LOCALAPPDATA")
        .filter(|directory| !directory.is_empty())
        .map(|directory| PathBuf::from(directory).join("ActingCommand").join("logs"))
}

/// Appends `lines` to `<directory>\actingd-<now_unix_ms>.log`, creating the directory; the
/// failure is `actingd_fallback_log_failed` with the OS error as its link.
fn append_fallback_log(
    directory: Option<&Path>,
    now_unix_ms: u64,
    lines: &str,
) -> Result<PathBuf, Outcome> {
    let failed = || Outcome::new(ActingdCode::FallbackLogFailed);
    let Some(directory) = directory else {
        return Err(failed().variable("LOCALAPPDATA"));
    };
    std::fs::create_dir_all(directory).map_err(|error| {
        failed()
            .path(directory)
            .caused_by(Outcome::from_io_error(&error, IoOp::CreateDir))
    })?;
    let path = directory.join(format!("actingd-{now_unix_ms}.log"));
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(lines.as_bytes()))
        .map_err(|error| {
            failed()
                .path(&path)
                .caused_by(Outcome::from_io_error(&error, IoOp::Write))
        })?;
    Ok(path)
}

/// Everything before the Runtime host runs, and its start: the arguments, the configuration,
/// the install root's adb, the resource packages and the recognition bindings, as
/// `check-config` checks them. A refusal here is a start refusal (model section 8.2).
fn start_daemon(
    arguments: Vec<std::ffi::OsString>,
) -> Result<(RuntimeHost, Option<PolicyBootstrap>), ActingdError> {
    let (arguments, install_held) = parse_install_held_arguments(arguments)?;
    let config_path = parse_arguments(arguments)?;
    let file = config::load(&config_path).map_err(ActingdError::config)?;
    // Workflow #398 RD-1b: a device refusal names its instance or file in its outcome.
    let device_refusal = file.device_refusal();
    // Workflow #369 S6a: the instance a `start_emulator` refusal names.
    let start_emulator = file.start_emulator_refusal();
    let RuntimeAssembly {
        host,
        provider,
        policy,
        resource_packages,
        recognition_packages,
        adb_requirement,
        ..
    } = file.assemble_or_refusal().map_err(|(code, refusal)| {
        match device_refusal.filter(|refused| refused.code() == code) {
            Some(refused) => {
                ActingdError::refused(code, refused.outcome()).with_detail(refused.to_string())
            }
            None => start_emulator_refused(code, start_emulator)
                .unwrap_or_else(|| ActingdError::refused(code, refusal)),
        }
    })?;
    // Workflow #337: the install root's adb check of `check-config`, before any side effect.
    config::ac_adb::require(&adb_requirement).map_err(|rejection| {
        ActingdError::refused(rejection.code(), rejection.outcome())
            .with_detail(rejection.to_string())
    })?;
    // The same resource package admission as `check-config`, before any side effect.
    let resource_packages =
        config::validate_resource_packages(&resource_packages).map_err(|rejection| {
            ActingdError::refused(rejection.code, rejection.outcome())
                .with_detail(rejection.to_string())
        })?;
    let host = host.with_resource_packages(resource_packages);
    // Workflow #383 AO-1: the ledger's retried writes and its fatal stop after a dump; the
    // commit actingd was built from names the dump's header.
    let host = host.with_ledger_failure_sink(Arc::new(DaemonLedgerFailure));
    let host = match option_env!("ACTINGCOMMAND_RUNTIME_HEAD") {
        Some(commit) => host.with_runtime_commit(commit),
        None => host,
    };
    // Workflow #384 D2-S2 (D2 model §3.2): a recognition package binding whose package the start
    // cannot read, or whose marks the configuration claims, is left out and noted in the
    // `recognition_packages` subsystem row; the start goes on. No bound package is prepared.
    let (recognition_packages, recognition_subsystem) =
        config::recognition_package::start_bindings(&recognition_packages, &host, policy.as_ref())
            .map_err(ActingdError::config)?;
    let host = host.with_recognition_packages(recognition_packages);
    let host = match recognition_subsystem {
        Some(subsystem) => host.with_config_subsystem(subsystem),
        None => host,
    };
    let host = match install_held {
        Some(held) => host.with_install_held(held),
        None => host,
    };
    let host =
        RuntimeHost::start_with_provider(host, |startup| provider.assemble_provider(startup))
            .map_err(ActingdError::runtime)?;
    Ok((host, policy))
}

/// Workflow #369 S6a: a `start_emulator_requires_discovery_binding` refusal names the instance
/// (`alias`, Workflow #378 A6a); `None` for any other code.
fn start_emulator_refused(code: Code, alias: Option<String>) -> Option<ActingdError> {
    if code != ActingdCode::StartEmulatorRequiresDiscoveryBinding.code() {
        return None;
    }
    let alias = alias?;
    let text = format!("instance {alias:?} names start_emulator but is not bound by discovery");
    Some(ActingdError::refused(code, Outcome::new(code).alias(alias)).with_detail(text))
}

/// The started Runtime until it stops: the policy's initial cycle, `runtime_ready`, then the
/// monitor. `Ok` once a requested stop closed the host, else the condition that stopped it.
fn serve(host: RuntimeHost, policy: Option<PolicyBootstrap>) -> Result<(), ActingdError> {
    let initial_policy_cycle = (|| {
        let Some(_work) = host.wait_policy_work().map_err(ActingdError::runtime)? else {
            return Ok(None);
        };
        policy
            .as_ref()
            .map(|policy| initialize_policy(&host, policy))
            .transpose()
            .map(Some)
    })();
    let initial_policy_cycle = match initial_policy_cycle {
        Ok(Some(cycle)) => cycle,
        Ok(None) => return host.close().map_err(ActingdError::runtime),
        Err(mut error) => {
            error.stage = Some(ActingdLocation::PolicyInitialization.location());
            let recorded = error.record_lifecycle_failure(
                &host,
                RuntimeLifecycleFailureStage::PolicyInitialization,
            );
            let closed = host.close();
            return combine_monitor_results(
                Err(error.with_recording_result(recorded)),
                Ok(()),
                Ok(()),
                closed.map_err(ActingdError::runtime),
            );
        }
    };
    let info = host.runtime_info();
    write_stderr(&daemon_lines(
        &Outcome::new(ActingdCode::RuntimeReady).pid(info.pid()),
        Some(&format!(
            "pid={} host={} port={}",
            info.pid(),
            info.host(),
            info.port()
        )),
    ));
    match (policy, initial_policy_cycle) {
        (Some(policy), Some(cycle)) => monitor_policy(host, policy, cycle),
        (None, None) => monitor(host),
        _ => {
            let mut error = ActingdError::process(ActingdCode::PolicyBootstrapStateInvalid);
            error.stage = Some(ActingdLocation::PolicyBootstrap.location());
            let recorded = error
                .record_lifecycle_failure(&host, RuntimeLifecycleFailureStage::PolicyBootstrap);
            let closed = host.close();
            combine_monitor_results(
                Err(error.with_recording_result(recorded)),
                Ok(()),
                Ok(()),
                closed.map_err(ActingdError::runtime),
            )
        }
    }
}

fn initialize_policy(
    host: &RuntimeHost,
    policy: &PolicyBootstrap,
) -> Result<PolicyCycleExecution, ActingdError> {
    // Workflow #361 C2: plan and check everything first; the catalog pointer moves only after
    // every check passed, so a refused configuration leaves no catalog event behind. The plan
    // reads the host's complete ledger-verified approval projection (Workflow #191 D2), builds
    // every configured approval, checks it against its latest decision (a persisted rejection
    // or revocation cannot be silently replaced by startup config) and computes the approvals
    // the planned catalog supersedes (Workflow #330 H2, #361 A).
    let plan = host
        .plan_policy_catalog_transition(
            &policy.catalog,
            policy.catalog_transition.as_ref(),
            &policy.catalog_approval_ids,
        )
        .map_err(|refused| {
            // Review P6: a refused plan (not a ledger or state failure) prints its remedy on
            // the FATAL line, for example the explicit `catalog_transition` that a
            // `catalog_activation_not_newer` needs.
            let remedy = if refused.is_fatal() {
                None
            } else {
                refused.native_detail().map(str::to_owned)
            };
            let error = ActingdError::runtime(refused);
            match remedy {
                Some(remedy) => error.with_detail(remedy),
                None => error,
            }
        })?;
    let generation = plan.generation();
    // The first catalog write: the planned transition, under the plan's compare-and-swap.
    let activated = host
        .apply_policy_catalog_transition(&plan)
        .map_err(ActingdError::runtime)?;
    if activated != *generation {
        return Err(ActingdError::process(ActingdCode::PolicyCatalogPlanChanged));
    }
    // Workflow #389 U-2 (model section 3.2): the person's configured approvals are transcribed
    // in process, after the catalog transition, with the provenance (Governance, User, Ui) every
    // approval record keeps; actingd no longer connects to itself. Superseded approvals can no
    // longer authorize a dispatch: they are revoked first, freeing their places in the bounded
    // active projection; then the undecided configured approvals and the re-approvals of a
    // restored generation.
    let approvals = plan.approvals();
    for decision in approvals
        .revoke()
        .iter()
        .chain(approvals.record())
        .chain(approvals.reapprove())
    {
        host.transcribe_configured_approval(decision)
            .map_err(ActingdError::runtime)?;
    }
    execute_policy_cycle(host, policy, PolicyTrigger::Recovery)
}

fn execute_policy_cycle(
    host: &RuntimeHost,
    policy: &PolicyBootstrap,
    trigger: PolicyTrigger,
) -> Result<PolicyCycleExecution, ActingdError> {
    let started = Instant::now();
    let cycle = host
        .evaluate_policy_cycle(trigger)
        .map_err(ActingdError::runtime)?;
    let mut recompute_wakes = Vec::new();
    if cycle.pending_dispatch_intents.len() > MAX_TASKS {
        return Err(ActingdError::process(
            ActingdCode::PolicyPendingDispatchBudgetExceeded,
        ));
    }
    let Some(evaluation) = cycle.evaluation.as_ref() else {
        if cycle.pending_dispatch_intents.is_empty() {
            return Ok(PolicyCycleExecution {
                cycle,
                recompute_wakes,
                yielded_intents: 0,
                failed_runs: false,
                retry_after_ms: None,
            });
        }
        return Err(ActingdError::process(
            ActingdCode::PolicyPendingDispatchWithoutEvaluation,
        ));
    };
    let mut yielded_intents = 0;
    let mut failed_runs = FailedRunInstances::default();
    let mut retry_after_ms: Option<u64> = None;
    for (index, intent) in cycle.pending_dispatch_intents.iter().enumerate() {
        if policy_cycle_budget_exhausted(index, started.elapsed()) {
            // Intents not yet attempted wrote no ledger fact; the next evaluation re-derives them.
            yielded_intents = cycle.pending_dispatch_intents.len() - index;
            writeln!(
                std::io::stdout().lock(),
                "actingd policy_cycle_yielded attempted={index} remaining={yielded_intents} elapsed_ms={}",
                started.elapsed().as_millis()
            )
            .map_err(|_| ActingdError::process(ActingdCode::PolicyCycleYieldReportFailed))?;
            break;
        }
        if failed_runs.skips(&intent.instance_id) {
            continue;
        }
        let reason_chain = evaluation
            .reason_chains
            .iter()
            .find(|reason_chain| reason_chain.id == intent.reason_chain_id)
            .ok_or_else(|| ActingdError::process(SharedCode::PolicyReasonChainMissing))?;
        let scheduled_task = policy.scheduled_tasks.get(&intent.procedure_ref);
        if let Some(task) = scheduled_task {
            validate_scheduled_execution_mode(
                task.mode,
                policy.registry_modes.get(&intent.instance_id).copied(),
            )?;
        }
        let admission_context = PolicyAdmissionContext {
            fact_ledger_position: intent.input_ledger_position,
            fact_snapshot_id: intent.fact_snapshot_id.clone(),
            approval_fact_ids: intent
                .approval_refs
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>(),
            fencing_owner_epoch: host.runtime_info().owner_epoch(),
            now_unix_ms: intent.prerequisites.evaluated_at_unix_ms,
        };
        let admission = match scheduled_task {
            Some(task) => host.admit_scheduled_policy_dispatch(
                intent,
                reason_chain,
                &admission_context,
                &task.request,
            ),
            None => host.admit_policy_dispatch(intent, reason_chain, &admission_context),
        };
        let admission = match admission {
            Ok(admission) => admission,
            // Host returns a nonfatal refusal only after its admission fact and
            // any original error details have been committed. Workflow #369 W-1 (review
            // L1): a refusal that says when it may run again (a contended admission guard,
            // `dispatch_instance_contended`) wakes the driver then.
            Err(error) if !error.is_fatal() => {
                if let Some(after_ms) = error.projection().retry_after_ms {
                    retry_after_ms =
                        Some(retry_after_ms.map_or(after_ms, |next| next.min(after_ms)));
                }
                continue;
            }
            Err(error) => return Err(ActingdError::runtime(error)),
        };
        let PolicyDispatchAdmission::Granted { context } = admission else {
            continue;
        };
        if let Some(task) = scheduled_task {
            validate_scheduled_execution_mode(
                task.mode,
                policy.registry_modes.get(context.instance_alias()).copied(),
            )?;
            let receipt = match host.run_scheduled_contained_task(&context, &task.request) {
                Ok(receipt) => receipt,
                // Runtime owns the original failure and same-run settlement. A later
                // candidate must pass its recovered failure disposition at admission.
                Err(error) if !error.is_fatal() => {
                    failed_runs.record(context.instance_alias());
                    continue;
                }
                Err(error) => return Err(ActingdError::runtime(error)),
            };
            let (execution, projection) =
                match host.complete_scheduled_policy_run(&context, &receipt) {
                    Ok(completed) => completed,
                    Err(error) if !error.is_fatal() => continue,
                    Err(error) => return Err(ActingdError::runtime(error)),
                };
            if let Some(projection) = projection {
                if recompute_wakes.len() >= MAX_TASKS {
                    return Err(ActingdError::process(
                        ActingdCode::PolicyRecomputeWakeBudgetExceeded,
                    ));
                }
                recompute_wakes.push(PolicyRecomputeWake::new(
                    &context, &receipt, &execution, projection,
                )?);
            }
        }
    }
    Ok(PolicyCycleExecution {
        cycle,
        recompute_wakes,
        yielded_intents,
        failed_runs: failed_runs.any(),
        retry_after_ms,
    })
}

/// Workflow #369 W-1 (review M-6): the instances whose scheduled run failed in this cycle. The
/// failed run's lease end hands its instance on (to its ladder, from #369 S2+S3b), so the
/// cycle's remaining intents for that instance are skipped and write nothing; `failed_runs`
/// makes the driver evaluate again at once, and that evaluation defers them while the
/// instance is held. Other instances' intents still run.
#[derive(Default)]
struct FailedRunInstances(BTreeSet<String>);

impl FailedRunInstances {
    fn record(&mut self, instance_alias: &str) {
        self.0.insert(instance_alias.to_owned());
    }

    fn skips(&self, instance_alias: &str) -> bool {
        self.0.contains(instance_alias)
    }

    fn any(&self) -> bool {
        !self.0.is_empty()
    }
}

/// Workflow #191 A1: whether the cycle stops before its pending intent at `index`. Every cycle
/// attempts its first intent, so each cycle makes progress; past the budget a later intent ends
/// the cycle and the driver re-evaluates at once.
fn policy_cycle_budget_exhausted(index: usize, elapsed: Duration) -> bool {
    index > 0 && elapsed >= MAX_POLICY_CYCLE_DURATION
}

struct PolicyCycleExecution {
    cycle: PolicyCycle,
    recompute_wakes: Vec<PolicyRecomputeWake>,
    /// Pending intents not yet attempted when the cycle reached its budget.
    yielded_intents: usize,
    /// Workflow #336 R22: a scheduled run of this cycle failed. The driver evaluates again at
    /// once, so a failure scheduled for an immediate rerun waits only for its retry backoff,
    /// not for the wake the evaluation before the run computed.
    failed_runs: bool,
    /// Workflow #369 W-1 (review L1): the earliest retry an admission refusal of this cycle
    /// named; the driver wakes no later than that.
    retry_after_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PolicyRecomputeWake {
    projection: SchedulingOutcomeProjection,
}

impl PolicyRecomputeWake {
    fn new(
        context: &PolicyRunContext,
        receipt: &RuntimeReceipt,
        execution: &PolicyExecutionEventData,
        projection: SchedulingOutcomeProjection,
    ) -> Result<Self, ActingdError> {
        let terminal = receipt
            .terminal()
            .ok_or_else(|| ActingdError::process(ActingdCode::PolicyRecomputeTerminalMissing))?;
        let identity = projection.scheduling_result().identity();
        if projection.ledger_position() < terminal.sequence
            || identity.terminal_event_id() != terminal.event_id
            || identity.terminal_sequence() != terminal.sequence
            || identity.instance_id() != context.lease_token().instance_id()
            || identity.task_id() != context.task_id()
            || identity.run_id() != context.run_id()
            || identity.request_id() != receipt.request_id()
            || identity.correlation_id() != context.correlation_id()
            || identity.lease_id() != context.lease_token().lease_id()
            || identity.decision_id() != context.decision_id()
            || identity.catalog_task_id() != execution.task_id
            || identity.instance_alias() != context.instance_alias()
            || execution.decision_id != context.decision_id()
            || execution.instance_id != context.instance_alias()
        {
            return Err(ActingdError::process(
                ActingdCode::PolicyRecomputeWakeIdentityConflict,
            ));
        }
        Ok(Self { projection })
    }

    fn key(&self) -> PolicyRecomputeKey {
        let identity = self.projection.scheduling_result().identity();
        PolicyRecomputeKey {
            run_id: identity.run_id(),
            terminal_event_id: identity.terminal_event_id(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PolicyRecomputeKey {
    run_id: RunId,
    terminal_event_id: actingcommand_contract::EventId,
}

fn validate_scheduled_execution_mode(
    expected: ScheduledExecutionMode,
    actual: Option<ScheduledExecutionMode>,
) -> Result<(), ActingdError> {
    let actual = actual
        .ok_or_else(|| ActingdError::process(ActingdCode::ScheduledExecutionInstanceUnknown))?;
    if actual != expected {
        return Err(ActingdError::process(
            ActingdCode::ScheduledExecutionBackendModeMismatch,
        ));
    }
    Ok(())
}

fn monitor(host: RuntimeHost) -> Result<(), ActingdError> {
    loop {
        thread::sleep(HEALTH_POLL_INTERVAL);
        match host.fatal_error() {
            Ok(Some(error)) | Err(error) => {
                let error = ActingdError::runtime(error);
                let recorded = error
                    .record_lifecycle_failure(&host, RuntimeLifecycleFailureStage::PolicyMonitor);
                let closed = host.close().map_err(ActingdError::runtime);
                return combine_monitor_results(
                    Err(error.with_recording_result(recorded)),
                    Ok(()),
                    Ok(()),
                    closed,
                );
            }
            Ok(None) => {
                if host
                    .is_shutdown_requested()
                    .map_err(ActingdError::runtime)?
                {
                    return host.close().map_err(ActingdError::runtime);
                }
            }
        }
    }
}

fn monitor_policy(
    host: RuntimeHost,
    policy: PolicyBootstrap,
    initial_cycle: PolicyCycleExecution,
) -> Result<(), ActingdError> {
    let host = Arc::new(host);
    let policy = Arc::new(policy);
    let control = Arc::new(PolicyDriverControl::default());
    let setup = (|| {
        let Some(_work) = host.wait_policy_work().map_err(ActingdError::runtime)? else {
            return Ok(None);
        };
        // Workflow #383 AO-7: the monitor wakes the driver from the host's business
        // notifications, opened where its ledger subscription used to start (the ledger head):
        // the initial cycle has evaluated everything before.
        let wakes = host.open_policy_wakes().map_err(ActingdError::runtime)?;
        let driver = thread::Builder::new()
            .name("actingd-policy-driver".to_string())
            .spawn({
                let host = Arc::clone(&host);
                let policy = Arc::clone(&policy);
                let control = Arc::clone(&control);
                move || drive_policy(host, policy, control, initial_cycle)
            })
            .map_err(|_| ActingdError::process(ActingdCode::PolicyDriverSpawnFailed))?;
        Ok(Some((wakes, driver)))
    })();
    let (wakes, driver) = match setup {
        Ok(Some(setup)) => setup,
        Ok(None) => return close_policy_host(host),
        Err(error) => {
            let mut error: ActingdError = error;
            error.stage = Some(ActingdLocation::PolicyMonitorSetup.location());
            let recorded =
                error.record_lifecycle_failure(&host, RuntimeLifecycleFailureStage::PolicyMonitor);
            let closed = close_policy_host(host);
            return combine_monitor_results(
                Err(error.with_recording_result(recorded)),
                Ok(()),
                Ok(()),
                closed,
            );
        }
    };

    let monitor_result = run_policy_monitor(&host, &control, &wakes, &driver);

    let recorded = match &monitor_result {
        Err(error) => {
            error.record_lifecycle_failure(&host, RuntimeLifecycleFailureStage::PolicyMonitor)
        }
        Ok(()) => Ok(()),
    };
    let shutdown_result = control.shutdown();
    let driver_result = match driver.join() {
        Ok(result) => result,
        Err(_) => Err(ActingdError::process(ActingdCode::PolicyDriverPanicked)),
    };
    drop(wakes);
    drop(policy);
    drop(control);
    finish_policy_closeout(
        host,
        monitor_result,
        shutdown_result,
        driver_result,
        recorded,
    )
}

/// Workflow #383 AO-7 (model-ledger-observer §3.7): the policy monitor's loop. It watches the
/// host's health and hands the host's business notifications (`wakes`) to the driver. It has
/// no Runtime client and sends no request: it does not subscribe to the ledger, so a slow or
/// paused ledger never holds a wake back. It ends with `Ok` once the host has accepted a
/// shutdown, and with the error otherwise.
fn run_policy_monitor(
    host: &RuntimeHost,
    control: &PolicyDriverControl,
    wakes: &PolicyWakes,
    driver: &thread::JoinHandle<Result<(), ActingdError>>,
) -> Result<(), ActingdError> {
    let mut waiting_for_install = false;
    loop {
        match host.fatal_error() {
            Ok(Some(error)) => return Err(ActingdError::runtime(error)),
            Ok(None) => {}
            Err(error) => return Err(ActingdError::runtime(error)),
        }
        match host.is_shutdown_requested() {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => return Err(ActingdError::runtime(error)),
        }
        if driver.is_finished() {
            return Err(ActingdError::process(ActingdCode::PolicyDriverStopped));
        }
        // Workflow #376 M1: during an install drain the monitor holds the driver's wakes back
        // (they stay pending in the host); an abort or a drain timeout resumes exactly here, and
        // only the accepted `commit_shutdown` (the shutdown check above) ends the monitor.
        match host.is_lifecycle_draining() {
            Ok(true) => {
                if !waiting_for_install {
                    waiting_for_install = true;
                    report_policy_monitor("waiting install_transition");
                }
                thread::sleep(Duration::from_millis(POLICY_EVENT_WAIT_MS));
                continue;
            }
            Ok(false) => {
                if waiting_for_install {
                    waiting_for_install = false;
                    report_policy_monitor("resumed");
                }
            }
            Err(error) => return Err(ActingdError::runtime(error)),
        }
        forward_policy_wakes(control, wakes, Duration::from_millis(POLICY_EVENT_WAIT_MS))?;
    }
}

/// Workflow #383 AO-7: hands the wakes the host raised since the last pass to the driver as one
/// trigger, coalesced as the triggers of a batch of ledger events were. It waits at most `wait`
/// for a wake, so the monitor's checks keep their pace while nothing happens.
fn forward_policy_wakes(
    control: &PolicyDriverControl,
    wakes: &PolicyWakes,
    wait: Duration,
) -> Result<(), ActingdError> {
    match wakes.take(wait).into_iter().reduce(coalesce_policy_trigger) {
        Some(trigger) => control.notify(trigger),
        None => Ok(()),
    }
}

/// Workflow #376 M2: what the policy monitor did after its subscription failed and the host
/// reported no fatal error (a fatal host error ended the monitor with that error first).
/// Workflow #383 AO-7: the monitor no longer subscribes, so nothing decides with this table any
/// more; it stays, with its test (#376 T2), until the coordinator retires both (ORD 376-B2).
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PolicySubscriptionFailureAction {
    /// Report the failure: `runtime.failed` and a FATAL exit, as before.
    Fail,
    /// The host accepted a shutdown: the monitor's normal end.
    End,
    /// An install drain holds admission: wait without subscribing (M1 on the next pass).
    Wait,
    /// An admission refusal with neither a drain nor a shutdown behind it (a drain aborted
    /// between the refusal and the check): retry once after the poll interval.
    RetryOnce,
}

/// Admission refusals in a row that neither a drain nor a shutdown explains (Q4: one retry).
/// Unused since AO-7, as [`PolicySubscriptionFailureAction`].
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Default)]
struct PolicySubscriptionRefusals {
    retried: bool,
}

#[cfg_attr(not(test), allow(dead_code))]
impl PolicySubscriptionRefusals {
    fn succeeded(&mut self) {
        self.retried = false;
    }

    fn failed(
        &mut self,
        shutdown_requested: bool,
        admission_refusal: bool,
        draining: bool,
    ) -> PolicySubscriptionFailureAction {
        let action = if shutdown_requested {
            PolicySubscriptionFailureAction::End
        } else if !admission_refusal {
            PolicySubscriptionFailureAction::Fail
        } else if draining {
            PolicySubscriptionFailureAction::Wait
        } else if self.retried {
            PolicySubscriptionFailureAction::Fail
        } else {
            PolicySubscriptionFailureAction::RetryOnce
        };
        if action == PolicySubscriptionFailureAction::RetryOnce {
            self.retried = true;
        }
        action
    }
}

/// The host's typed refusal of a request it did not admit (`begin_request` gave no work): a
/// non-fatal `RuntimeUnavailable` rejection. An install drain refuses a subscription this way.
/// Unused since AO-7, as [`PolicySubscriptionFailureAction`].
#[cfg_attr(not(test), allow(dead_code))]
fn is_admission_refusal(code: Code, projection: Option<&RuntimeErrorProjection>) -> bool {
    code == ClientCode::RuntimeRequestRejected.code()
        && projection.is_some_and(|projection| {
            projection.code == RuntimeErrorCode::RuntimeUnavailable && !projection.fatal
        })
}

/// Workflow #376 M6: one informational stdout line per policy monitor change; the ledger's
/// `host.install_transition` facts stay authoritative. Like the other informational daemon
/// lines, a failed stdout write does not stop the daemon.
fn report_policy_monitor(state: &str) {
    let _ = writeln!(std::io::stdout().lock(), "actingd policy_monitor {state}");
}

fn finish_policy_closeout(
    host: Arc<RuntimeHost>,
    monitor_result: Result<(), ActingdError>,
    shutdown_result: Result<(), ActingdError>,
    driver_result: Result<(), ActingdError>,
    mut recorded: Result<(), ActingdError>,
) -> Result<(), ActingdError> {
    for (stage, result) in [
        (RuntimeLifecycleFailureStage::PolicyDriver, &driver_result),
        (
            RuntimeLifecycleFailureStage::PolicyControl,
            &shutdown_result,
        ),
    ] {
        if recorded.is_ok()
            && let Err(error) = result
        {
            recorded = error.record_lifecycle_failure(&host, stage);
        }
    }
    let close_result = close_policy_host(host);
    match combine_monitor_results(monitor_result, shutdown_result, driver_result, close_result) {
        Err(error) => Err(error.with_recording_result(recorded)),
        Ok(()) => recorded,
    }
}

fn close_policy_host(host: Arc<RuntimeHost>) -> Result<(), ActingdError> {
    match Arc::try_unwrap(host) {
        Ok(host) => host.close().map_err(ActingdError::runtime),
        Err(host) => {
            let error = ActingdError::process(ActingdCode::PolicyDriverReferenceLeaked);
            let recorded = error
                .record_lifecycle_failure(&host, RuntimeLifecycleFailureStage::RetainedReference);
            drop(host);
            Err(error.with_recording_result(recorded))
        }
    }
}

fn combine_monitor_results(
    monitor: Result<(), ActingdError>,
    shutdown: Result<(), ActingdError>,
    driver: Result<(), ActingdError>,
    close: Result<(), ActingdError>,
) -> Result<(), ActingdError> {
    let mut failure: Option<ActingdError> = None;
    let mut secondary_index = 0;
    for (stage, result) in [
        (ActingdLocation::PolicyCloseoutDriver, driver),
        (ActingdLocation::PolicyCloseoutMonitor, monitor),
        (ActingdLocation::PolicyCloseoutShutdown, shutdown),
        (ActingdLocation::PolicyCloseoutHostClose, close),
    ] {
        if let Err(mut error) = result {
            error.stage.get_or_insert(stage.location());
            match &mut failure {
                None => failure = Some(error),
                Some(primary) => {
                    // Four fixed inputs yield at most three complete secondary errors.
                    primary.secondary[secondary_index] = Some(Box::new(error));
                    secondary_index += 1;
                }
            }
        }
    }
    failure.map_or(Ok(()), Err)
}

// Workflow #383 AO-7: the event-to-trigger mapping moved to the host with the wake source
// (`actingcommand_runtime_host::policy_trigger_for_event`); the monitor coalesces the
// host's wakes with the priority below, as it coalesced a batch of ledger events.
fn coalesce_policy_trigger(left: PolicyTrigger, right: PolicyTrigger) -> PolicyTrigger {
    if trigger_priority(right) > trigger_priority(left) {
        right
    } else {
        left
    }
}

const fn trigger_priority(trigger: PolicyTrigger) -> u8 {
    match trigger {
        PolicyTrigger::CatalogChanged => 5,
        PolicyTrigger::Reconciliation => 4,
        PolicyTrigger::ClockObserved { .. } => 3,
        PolicyTrigger::ResourcesChanged => 2,
        PolicyTrigger::FactsChanged => 1,
        PolicyTrigger::Recovery => 0,
    }
}

#[derive(Default)]
struct PolicyDriverControl {
    state: Mutex<PolicyDriverSignal>,
    changed: Condvar,
}

#[derive(Default)]
struct PolicyDriverSignal {
    pending_trigger: Option<PolicyTrigger>,
    pending_recomputes: BTreeMap<PolicyRecomputeKey, PolicyRecomputeWake>,
    active_recomputes: BTreeMap<PolicyRecomputeKey, PolicyRecomputeWake>,
    completed_recomputes: BTreeMap<PolicyRecomputeKey, PolicyRecomputeWake>,
    shutdown: bool,
    #[cfg(test)]
    wait_predicate_checks: usize,
}

enum PolicyDriverWake {
    Trigger(PolicyTrigger, Vec<PolicyRecomputeWake>),
    Shutdown,
    TimedOut,
}

impl PolicyDriverControl {
    fn notify(&self, trigger: PolicyTrigger) -> Result<(), ActingdError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActingdError::process(ActingdCode::PolicyDriverSignalPoisoned))?;
        if !state.shutdown {
            state.pending_trigger = Some(
                state
                    .pending_trigger
                    .map_or(trigger, |pending| coalesce_policy_trigger(pending, trigger)),
            );
            self.changed.notify_one();
        }
        Ok(())
    }

    fn notify_recompute(&self, wake: PolicyRecomputeWake) -> Result<(), ActingdError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActingdError::process(ActingdCode::PolicyDriverSignalPoisoned))?;
        if state.shutdown {
            return Ok(());
        }
        let key = wake.key();
        if let Some(existing) = state.completed_recomputes.get(&key) {
            return if existing == &wake {
                Ok(())
            } else {
                Err(ActingdError::process(
                    ActingdCode::PolicyRecomputeWakeIdentityConflict,
                ))
            };
        }
        if let Some(existing) = state.active_recomputes.get(&key) {
            return if existing == &wake {
                Ok(())
            } else {
                Err(ActingdError::process(
                    ActingdCode::PolicyRecomputeWakeIdentityConflict,
                ))
            };
        }
        if let Some(existing) = state.pending_recomputes.get(&key)
            && existing != &wake
        {
            return Err(ActingdError::process(
                ActingdCode::PolicyRecomputeWakeIdentityConflict,
            ));
        }
        if !state.pending_recomputes.contains_key(&key)
            && state.pending_recomputes.len() + state.active_recomputes.len() >= MAX_TASKS
        {
            return Err(ActingdError::process(
                ActingdCode::PolicyRecomputeWakeBudgetExceeded,
            ));
        }
        state.pending_recomputes.insert(key, wake);
        state.pending_trigger = Some(
            state
                .pending_trigger
                .map_or(PolicyTrigger::FactsChanged, |pending| {
                    coalesce_policy_trigger(pending, PolicyTrigger::FactsChanged)
                }),
        );
        self.changed.notify_one();
        Ok(())
    }

    fn complete_recomputes(&self, recomputes: &[PolicyRecomputeWake]) -> Result<(), ActingdError> {
        let completed = recompute_wakes_by_key(recomputes.iter().cloned())?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActingdError::process(ActingdCode::PolicyDriverSignalPoisoned))?;
        for (key, wake) in &completed {
            if state
                .completed_recomputes
                .get(key)
                .is_some_and(|existing| existing != wake)
                || state
                    .active_recomputes
                    .get(key)
                    .is_some_and(|existing| existing != wake)
                || state
                    .pending_recomputes
                    .get(key)
                    .is_some_and(|existing| existing != wake)
            {
                return Err(ActingdError::process(
                    ActingdCode::PolicyRecomputeWakeIdentityConflict,
                ));
            }
        }
        for (key, wake) in completed {
            state.active_recomputes.remove(&key);
            state.pending_recomputes.remove(&key);
            state.completed_recomputes.insert(key, wake);
        }
        while state.completed_recomputes.len() > MAX_TASKS {
            state.completed_recomputes.pop_first();
        }
        Ok(())
    }

    fn shutdown(&self) -> Result<(), ActingdError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActingdError::process(ActingdCode::PolicyDriverSignalPoisoned))?;
        state.shutdown = true;
        self.changed.notify_all();
        Ok(())
    }

    fn wait(&self, timeout: Duration) -> Result<PolicyDriverWake, ActingdError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActingdError::process(ActingdCode::PolicyDriverSignalPoisoned))?;
        if state.shutdown {
            return Ok(PolicyDriverWake::Shutdown);
        }
        if let Some((trigger, recomputes)) = take_policy_driver_work(&mut state)? {
            return Ok(PolicyDriverWake::Trigger(trigger, recomputes));
        }
        if timeout.is_zero() {
            return Ok(PolicyDriverWake::TimedOut);
        }
        let (mut state, wait) = self
            .changed
            .wait_timeout_while(state, timeout, |state| {
                #[cfg(test)]
                {
                    state.wait_predicate_checks += 1;
                    self.changed.notify_all();
                }
                !state.shutdown && state.pending_trigger.is_none()
            })
            .map_err(|_| ActingdError::process(ActingdCode::PolicyDriverSignalPoisoned))?;
        if state.shutdown {
            Ok(PolicyDriverWake::Shutdown)
        } else if let Some((trigger, recomputes)) = take_policy_driver_work(&mut state)? {
            Ok(PolicyDriverWake::Trigger(trigger, recomputes))
        } else if wait.timed_out() {
            Ok(PolicyDriverWake::TimedOut)
        } else {
            Err(ActingdError::process(ActingdCode::PolicyDriverWakeMissing))
        }
    }
}

fn take_policy_driver_work(
    state: &mut PolicyDriverSignal,
) -> Result<Option<(PolicyTrigger, Vec<PolicyRecomputeWake>)>, ActingdError> {
    let Some(trigger) = state.pending_trigger.take() else {
        if state.pending_recomputes.is_empty() {
            return Ok(None);
        }
        return Err(ActingdError::process(
            ActingdCode::PolicyRecomputeTriggerMissing,
        ));
    };
    let pending = state
        .pending_recomputes
        .values()
        .cloned()
        .collect::<Vec<_>>();
    merge_recompute_wakes(&mut state.active_recomputes, pending)?;
    let recomputes = std::mem::take(&mut state.pending_recomputes);
    Ok(Some((trigger, recomputes.into_values().collect())))
}

fn recompute_wakes_by_key(
    recomputes: impl IntoIterator<Item = PolicyRecomputeWake>,
) -> Result<BTreeMap<PolicyRecomputeKey, PolicyRecomputeWake>, ActingdError> {
    let mut keyed = BTreeMap::new();
    for wake in recomputes {
        let key = wake.key();
        if keyed
            .get(&key)
            .is_some_and(|existing: &PolicyRecomputeWake| existing != &wake)
        {
            return Err(ActingdError::process(
                ActingdCode::PolicyRecomputeWakeIdentityConflict,
            ));
        }
        keyed.insert(key, wake);
        if keyed.len() > MAX_TASKS {
            return Err(ActingdError::process(
                ActingdCode::PolicyRecomputeWakeBudgetExceeded,
            ));
        }
    }
    Ok(keyed)
}

fn merge_recompute_wakes(
    target: &mut BTreeMap<PolicyRecomputeKey, PolicyRecomputeWake>,
    recomputes: impl IntoIterator<Item = PolicyRecomputeWake>,
) -> Result<(), ActingdError> {
    let additions = recompute_wakes_by_key(recomputes)?;
    for (key, wake) in &additions {
        if target.get(key).is_some_and(|existing| existing != wake) {
            return Err(ActingdError::process(
                ActingdCode::PolicyRecomputeWakeIdentityConflict,
            ));
        }
    }
    let new_keys = additions
        .keys()
        .filter(|key| !target.contains_key(key))
        .count();
    if target.len() + new_keys > MAX_TASKS {
        return Err(ActingdError::process(
            ActingdCode::PolicyRecomputeWakeBudgetExceeded,
        ));
    }
    target.extend(additions);
    Ok(())
}

struct DeferredPolicyTrigger {
    trigger: PolicyTrigger,
    recomputes: BTreeMap<PolicyRecomputeKey, PolicyRecomputeWake>,
    eligible_at_unix_ms: u64,
}

struct PolicyDriverSchedule {
    cadence: PolicyCadence,
    next_wake_unix_ms: Option<u64>,
    next_reconciliation_unix_ms: u64,
    next_clock_observation_unix_ms: u64,
    last_clock_observation_unix_ms: u64,
    deferred: Option<DeferredPolicyTrigger>,
}

impl PolicyDriverSchedule {
    fn new(
        cadence: PolicyCadence,
        initial_cycle: &PolicyCycle,
        now_unix_ms: u64,
    ) -> Result<Self, ActingdError> {
        let mut schedule = Self {
            next_wake_unix_ms: None,
            next_reconciliation_unix_ms: checked_deadline(
                now_unix_ms,
                cadence.reconciliation_interval_ms,
            )?,
            next_clock_observation_unix_ms: checked_deadline(
                now_unix_ms,
                clock_observation_interval_ms(&cadence),
            )?,
            last_clock_observation_unix_ms: now_unix_ms,
            deferred: None,
            cadence,
        };
        schedule.apply_cycle(PolicyTrigger::Recovery, initial_cycle, now_unix_ms)?;
        Ok(schedule)
    }

    fn apply_cycle(
        &mut self,
        trigger: PolicyTrigger,
        cycle: &PolicyCycle,
        now_unix_ms: u64,
    ) -> Result<(), ActingdError> {
        if let Some(evaluation) = cycle.evaluation.as_ref() {
            self.next_wake_unix_ms = evaluation.next_wake_unix_ms;
        }
        if cycle.directive.kind == PolicyRecomputeKind::Deferred {
            self.merge_deferred_work(trigger, Vec::new(), cycle.directive.eligible_at_unix_ms)?;
        } else {
            if self
                .deferred
                .as_ref()
                .is_none_or(|deferred| deferred.recomputes.is_empty())
            {
                self.deferred = None;
            }
            if cycle.directive.kind == PolicyRecomputeKind::Full {
                self.next_reconciliation_unix_ms =
                    checked_deadline(now_unix_ms, self.cadence.reconciliation_interval_ms)?;
            }
        }
        Ok(())
    }

    fn coalesce_deferred_work(
        &mut self,
        trigger: PolicyTrigger,
        recomputes: Vec<PolicyRecomputeWake>,
    ) -> Result<bool, ActingdError> {
        let Some(deferred) = self.deferred.as_mut() else {
            return Ok(false);
        };
        if trigger == PolicyTrigger::CatalogChanged {
            return Ok(false);
        }
        merge_recompute_wakes(&mut deferred.recomputes, recomputes)?;
        deferred.trigger = coalesce_policy_trigger(deferred.trigger, trigger);
        Ok(true)
    }

    fn defer_projection(
        &mut self,
        trigger: PolicyTrigger,
        recomputes: Vec<PolicyRecomputeWake>,
        now_unix_ms: u64,
    ) -> Result<(), ActingdError> {
        let eligible_at_unix_ms = checked_deadline(now_unix_ms, self.cadence.debounce_ms.max(1))?;
        self.merge_deferred_work(trigger, recomputes, eligible_at_unix_ms)
    }

    fn merge_deferred_work(
        &mut self,
        trigger: PolicyTrigger,
        recomputes: Vec<PolicyRecomputeWake>,
        eligible_at_unix_ms: u64,
    ) -> Result<(), ActingdError> {
        let pending = recompute_wakes_by_key(recomputes)?;
        match self.deferred.as_mut() {
            Some(existing) => {
                merge_recompute_wakes(&mut existing.recomputes, pending.into_values())?;
                existing.trigger = coalesce_policy_trigger(existing.trigger, trigger);
                existing.eligible_at_unix_ms =
                    existing.eligible_at_unix_ms.max(eligible_at_unix_ms);
            }
            None => {
                self.deferred = Some(DeferredPolicyTrigger {
                    trigger,
                    recomputes: pending,
                    eligible_at_unix_ms,
                });
            }
        }
        Ok(())
    }

    fn take_due_work(
        &mut self,
        now_unix_ms: u64,
    ) -> Result<Option<(PolicyTrigger, Vec<PolicyRecomputeWake>)>, ActingdError> {
        let previous = self.last_clock_observation_unix_ms;
        let delta = now_unix_ms.saturating_sub(previous);
        if now_unix_ms < previous || delta > self.cadence.clock_jump_threshold_ms {
            self.record_clock_observation(now_unix_ms)?;
            return Ok(Some((
                PolicyTrigger::ClockObserved {
                    previous_unix_ms: previous,
                },
                Vec::new(),
            )));
        }
        if now_unix_ms >= self.next_clock_observation_unix_ms {
            self.record_clock_observation(now_unix_ms)?;
        }
        if now_unix_ms >= self.next_reconciliation_unix_ms {
            return Ok(Some((PolicyTrigger::Reconciliation, Vec::new())));
        }
        if self
            .deferred
            .as_ref()
            .is_some_and(|deferred| now_unix_ms >= deferred.eligible_at_unix_ms)
        {
            return Ok(self.deferred.take().map(|deferred| {
                (
                    deferred.trigger,
                    deferred.recomputes.into_values().collect(),
                )
            }));
        }
        if self
            .next_wake_unix_ms
            .is_some_and(|next_wake| now_unix_ms >= next_wake)
        {
            self.next_wake_unix_ms = None;
            let previous_unix_ms = self.last_clock_observation_unix_ms;
            self.record_clock_observation(now_unix_ms)?;
            return Ok(Some((
                PolicyTrigger::ClockObserved { previous_unix_ms },
                Vec::new(),
            )));
        }
        Ok(None)
    }

    /// Workflow #369 W-1 (review L1): wakes the driver at `wake_unix_ms` at the latest.
    fn wake_no_later_than(&mut self, wake_unix_ms: u64) {
        self.next_wake_unix_ms = Some(
            self.next_wake_unix_ms
                .map_or(wake_unix_ms, |next| next.min(wake_unix_ms)),
        );
    }

    fn wait_duration(&self, now_unix_ms: u64) -> Duration {
        let next = [
            self.next_wake_unix_ms,
            Some(self.next_reconciliation_unix_ms),
            Some(self.next_clock_observation_unix_ms),
            self.deferred
                .as_ref()
                .map(|deferred| deferred.eligible_at_unix_ms),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(now_unix_ms);
        Duration::from_millis(next.saturating_sub(now_unix_ms))
    }

    fn record_clock_observation(&mut self, now_unix_ms: u64) -> Result<(), ActingdError> {
        self.last_clock_observation_unix_ms = now_unix_ms;
        self.next_clock_observation_unix_ms =
            checked_deadline(now_unix_ms, clock_observation_interval_ms(&self.cadence))?;
        Ok(())
    }
}

fn drive_policy(
    host: Arc<RuntimeHost>,
    policy: Arc<PolicyBootstrap>,
    control: Arc<PolicyDriverControl>,
    initial_cycle: PolicyCycleExecution,
) -> Result<(), ActingdError> {
    let mut schedule = PolicyDriverSchedule::new(
        policy.cadence.clone(),
        &initial_cycle.cycle,
        system_unix_ms()?,
    )?;
    for wake in initial_cycle.recompute_wakes {
        control.notify_recompute(wake)?;
    }
    if initial_cycle.yielded_intents > 0 || initial_cycle.failed_runs {
        control.notify(PolicyTrigger::FactsChanged)?;
    }
    loop {
        match control.wait(Duration::ZERO)? {
            PolicyDriverWake::Shutdown => return Ok(()),
            PolicyDriverWake::Trigger(trigger, recomputes) => {
                handle_policy_driver_trigger(
                    &host,
                    &policy,
                    &control,
                    &mut schedule,
                    trigger,
                    recomputes,
                )?;
                continue;
            }
            PolicyDriverWake::TimedOut => {}
        }

        let now = system_unix_ms()?;
        if let Some((trigger, recomputes)) = schedule.take_due_work(now)? {
            execute_ready_policy_trigger(
                &host,
                &policy,
                &control,
                &mut schedule,
                trigger,
                recomputes,
                now,
            )?;
            continue;
        }
        match control.wait(schedule.wait_duration(now))? {
            PolicyDriverWake::Shutdown => return Ok(()),
            PolicyDriverWake::Trigger(trigger, recomputes) => {
                handle_policy_driver_trigger(
                    &host,
                    &policy,
                    &control,
                    &mut schedule,
                    trigger,
                    recomputes,
                )?;
            }
            PolicyDriverWake::TimedOut => {}
        }
    }
}

fn handle_policy_driver_trigger(
    host: &RuntimeHost,
    policy: &PolicyBootstrap,
    control: &PolicyDriverControl,
    schedule: &mut PolicyDriverSchedule,
    mut trigger: PolicyTrigger,
    mut recomputes: Vec<PolicyRecomputeWake>,
) -> Result<(), ActingdError> {
    if schedule.coalesce_deferred_work(trigger, recomputes.clone())? {
        return Ok(());
    }
    let now = system_unix_ms()?;
    if let Some((due, due_recomputes)) = schedule.take_due_work(now)? {
        trigger = coalesce_policy_trigger(due, trigger);
        let mut keyed = recompute_wakes_by_key(recomputes)?;
        merge_recompute_wakes(&mut keyed, due_recomputes)?;
        recomputes = keyed.into_values().collect();
    }
    execute_ready_policy_trigger(host, policy, control, schedule, trigger, recomputes, now)
}

fn execute_ready_policy_trigger(
    host: &RuntimeHost,
    policy: &PolicyBootstrap,
    control: &PolicyDriverControl,
    schedule: &mut PolicyDriverSchedule,
    trigger: PolicyTrigger,
    recomputes: Vec<PolicyRecomputeWake>,
    now_unix_ms: u64,
) -> Result<(), ActingdError> {
    let Some(_work) = host.wait_policy_work().map_err(ActingdError::runtime)? else {
        return Ok(());
    };
    apply_policy_cycle_result(
        control,
        schedule,
        trigger,
        recomputes,
        now_unix_ms,
        execute_policy_cycle(host, policy, trigger),
    )
}

fn apply_policy_cycle_result(
    control: &PolicyDriverControl,
    schedule: &mut PolicyDriverSchedule,
    trigger: PolicyTrigger,
    recomputes: Vec<PolicyRecomputeWake>,
    now_unix_ms: u64,
    result: Result<PolicyCycleExecution, ActingdError>,
) -> Result<(), ActingdError> {
    match result {
        Ok(execution) => {
            let policy_consumed = execution.cycle.directive.kind != PolicyRecomputeKind::Deferred;
            let apply = (|| {
                schedule.apply_cycle(trigger, &execution.cycle, now_unix_ms)?;
                if let Some(retry_after_ms) = execution.retry_after_ms {
                    schedule.wake_no_later_than(checked_deadline(now_unix_ms, retry_after_ms)?);
                }
                if !policy_consumed {
                    schedule.merge_deferred_work(
                        trigger,
                        recomputes.clone(),
                        execution.cycle.directive.eligible_at_unix_ms,
                    )?;
                }
                for wake in execution.recompute_wakes {
                    control.notify_recompute(wake)?;
                }
                if execution.yielded_intents > 0 || execution.failed_runs {
                    control.notify(PolicyTrigger::FactsChanged)?;
                }
                Ok(())
            })();
            match apply {
                Ok(()) if policy_consumed => control.complete_recomputes(&recomputes),
                Ok(()) => Ok(()),
                Err(error) => Err(error),
            }
        }
        Err(error)
            if !recomputes.is_empty() && error.code.is(SharedCode::OutcomeProjectionNotReady) =>
        {
            schedule.defer_projection(trigger, recomputes, now_unix_ms)
        }
        Err(error) => Err(error),
    }
}

fn clock_observation_interval_ms(cadence: &PolicyCadence) -> u64 {
    (cadence.clock_jump_threshold_ms / 2).max(1)
}

fn checked_deadline(now_unix_ms: u64, delay_ms: u64) -> Result<u64, ActingdError> {
    now_unix_ms
        .checked_add(delay_ms)
        .ok_or_else(|| ActingdError::process(ActingdCode::PolicyDriverTimeOverflow))
}

fn system_unix_ms() -> Result<u64, ActingdError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ActingdError::process(SharedCode::SystemClockInvalid))?;
    u64::try_from(elapsed.as_millis())
        .map_err(|_| ActingdError::process(SharedCode::SystemClockInvalid))
}

fn parse_install_held_arguments(
    mut arguments: Vec<std::ffi::OsString>,
) -> Result<
    (
        Vec<std::ffi::OsString>,
        Option<actingcommand_contract::InstallHeldStartup>,
    ),
    ActingdError,
> {
    if arguments.len() == 4 && arguments[2] == "--install-held" {
        let value = arguments[3]
            .to_str()
            .filter(|value| value.len() <= 4096)
            .ok_or_else(|| ActingdError::config(ActingdCode::InstallHeldInputInvalid))?;
        let held: actingcommand_contract::InstallHeldStartup = serde_json::from_str(value)
            .map_err(|_| ActingdError::config(ActingdCode::InstallHeldInputInvalid))?;
        held.validate()
            .map_err(|error| ActingdError::config(error.code()))?;
        arguments.truncate(2);
        Ok((arguments, Some(held)))
    } else {
        Ok((arguments, None))
    }
}

fn parse_arguments(arguments: Vec<std::ffi::OsString>) -> Result<PathBuf, ActingdError> {
    let [flag, path] = arguments.as_slice() else {
        return Err(ActingdError::config(ActingdCode::UsageInvalid));
    };
    if flag != "--config" || path.is_empty() {
        return Err(ActingdError::config(ActingdCode::UsageInvalid));
    }
    Ok(PathBuf::from(path))
}

/// The code an actingd error carries.
#[derive(Debug, Clone, Copy)]
enum ErrorCode {
    /// A registered code: actingd's own, a shared contract code or a runtime client code.
    Registered(Code),
    /// The code of a runtime host refusal actingd reports without the host's error value; A2b
    /// registers the host's codes.
    Host { host_code: &'static str },
}

impl ErrorCode {
    /// The code's spelling.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Registered(code) => code.as_str(),
            Self::Host { host_code } => host_code,
        }
    }

    /// Whether this is the given registered code.
    fn is(self, code: impl Into<Code>) -> bool {
        self.as_str() == code.into().as_str()
    }
}

#[derive(Debug)]
struct ActingdError {
    code: ErrorCode,
    stage: Option<Location>,
    secondary: [Option<Box<ActingdError>>; 3],
    recording_failure: Option<Box<ActingdError>>,
    runtime: Option<Box<RuntimeHostError>>,
    maintenance: Option<Box<actingcommand_runtime_host::LedgerMaintenanceFailure>>,
    /// What the error carries beside its code for its reader, in one box so that the error stays
    /// small.
    extras: Option<Box<ErrorExtras>>,
    recorded: std::sync::atomic::AtomicBool,
}

/// What an [`ActingdError`] carries beside its code.
#[derive(Debug, Default)]
struct ErrorExtras {
    /// Workflow #378 A6a: the refusal as an outcome, when the refusing check names what it
    /// refused (its values and links).
    refusal: Option<Outcome>,
    /// Developer text about what the error refers to, such as the refused resource package
    /// path; only the developer line shows it.
    detail: Option<String>,
    /// Workflow #378 A6a: the data members a subcommand's failure report carries beside its
    /// outcome (`journal_appended`, `activated`).
    report: serde_json::Map<String, serde_json::Value>,
}

impl ActingdError {
    fn extras(&mut self) -> &mut ErrorExtras {
        self.extras.get_or_insert_with(Default::default)
    }

    fn with_detail(mut self, detail: String) -> Self {
        self.extras().detail = Some(detail);
        self
    }

    /// This error with a data member of the subcommand's failure report.
    fn with_report(mut self, member: &str, value: serde_json::Value) -> Self {
        self.extras().report.insert(member.to_owned(), value);
        self
    }

    /// The data members of the subcommand's failure report.
    fn report_data(&self) -> serde_json::Map<String, serde_json::Value> {
        self.extras
            .as_ref()
            .map(|extras| extras.report.clone())
            .unwrap_or_default()
    }

    fn with_recording_result(mut self, recorded: Result<(), Self>) -> Self {
        if let Err(error) = recorded {
            self.recording_failure = Some(Box::new(match self.recording_failure.take() {
                Some(prior) => prior.with_recording_result(Err(error)),
                None => error,
            }));
        }
        self
    }

    fn maintenance(error: actingcommand_runtime_host::LedgerMaintenanceFailure) -> Self {
        let mut result = Self::process(ActingdCode::LedgerMaintenanceFailed);
        result.maintenance = Some(Box::new(error));
        result
    }

    fn record_lifecycle_failure(
        &self,
        host: &RuntimeHost,
        stage: RuntimeLifecycleFailureStage,
    ) -> Result<(), ActingdError> {
        if self.recorded.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }
        let failure = if let Some(error) = &self.runtime {
            RuntimeLifecycleFailure::Host(error)
        } else {
            RuntimeLifecycleFailure::Process {
                code: self.code.as_str(),
            }
        };
        host.record_lifecycle_failure(stage, failure)
            .map_err(ActingdError::runtime)?;
        self.recorded
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    /// An error with this code and nothing wrapped.
    const fn coded(code: ErrorCode) -> Self {
        Self {
            code,
            stage: None,
            secondary: [None, None, None],
            recording_failure: None,
            runtime: None,
            maintenance: None,
            extras: None,
            recorded: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn config(code: impl Into<Code>) -> Self {
        Self::coded(ErrorCode::Registered(code.into()))
    }

    /// Workflow #378 A6a: a refusal of `code` whose outcome names what was refused.
    fn refused(code: impl Into<Code>, refusal: Outcome) -> Self {
        let mut result = Self::config(code);
        result.extras().refusal = Some(refusal);
        result
    }

    fn process(code: impl Into<Code>) -> Self {
        Self::coded(ErrorCode::Registered(code.into()))
    }

    fn runtime(error: RuntimeHostError) -> Self {
        let mut result = Self::coded(ErrorCode::Host {
            host_code: error.code(),
        });
        result.runtime = Some(Box::new(error));
        result
    }

    /// Workflow #378 A6a (model sections 7.4, 7.5): the error as an outcome: the terminal
    /// outcome of a daemon it stops, a start refusal, or a subcommand's failure. It is the
    /// wrapped failure's own outcome (the Runtime host's, the client's, ledger maintenance's, or
    /// the refusal's with its values), else the code, with the stage actingd names; each
    /// secondary failure is a `secondary` link and a failure to record it a `recording` link,
    /// and a fatal one tops the outcome when the error's own is not, so a fatal link always
    /// sits under a fatal top.
    fn outcome(&self) -> Outcome {
        let own = if let Some(error) = &self.runtime {
            error.daemon_outcome()
        } else if let Some(error) = &self.maintenance {
            error.outcome().clone()
        } else if let Some(refusal) = self
            .extras
            .as_ref()
            .and_then(|extras| extras.refusal.as_ref())
        {
            refusal.clone()
        } else {
            match self.code {
                ErrorCode::Registered(code) => Outcome::new(code),
                ErrorCode::Host { host_code } => Outcome::relayed(host_code),
            }
        };
        // The stage actingd names (a check-config stage, a policy or close-out stage) is where
        // the error arose for actingd's reader, so it is the outcome's `stage`.
        let own = match self.stage {
            Some(stage) => own.stage(stage),
            None => own,
        };
        let own = self.secondary.iter().flatten().fold(own, |top, secondary| {
            top.joined(CauseRelation::Secondary, secondary.outcome())
        });
        match &self.recording_failure {
            Some(failure) => own.joined(CauseRelation::Recording, failure.outcome()),
            None => own,
        }
    }
}

/// Developer text only (model section 7.4): the developer line shows it after the code.
impl fmt::Display for ActingdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(stage) = self.stage {
            write!(formatter, "{stage}: ")?;
        }
        if let Some(error) = &self.maintenance {
            error.fmt(formatter)?;
        } else if let Some(error) = &self.runtime {
            formatter.write_str(&error.complete_message())?;
        } else if let Some(refusal) = self
            .extras
            .as_ref()
            .and_then(|extras| extras.refusal.as_ref())
        {
            write!(formatter, "{refusal}")?;
        } else {
            formatter.write_str(self.code.as_str())?;
        }
        if let Some(detail) = self
            .extras
            .as_ref()
            .and_then(|extras| extras.detail.as_ref())
        {
            write!(formatter, ": {detail}")?;
        }
        for secondary in self.secondary.iter().flatten() {
            write!(formatter, "; secondary {secondary}")?;
        }
        if let Some(error) = &self.recording_failure {
            write!(formatter, "; lifecycle recording failure: {error}")?;
        }
        Ok(())
    }
}

impl Error for ActingdError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ScheduledProcedureTask;
    use actingcommand_contract::{
        ApplicationLifecycleAction, ContainedTaskRequest, EventActor, EventFamily, EventPayload,
        EventQuery, EventSource, EventType, IdentifierIssuer, PolicyPayload, ProjectionPayload,
        ProjectionProfile, RuntimeEventQueryPageRequest,
    };
    use actingcommand_device::{CaptureBackend, DeviceError, DeviceResult, Frame, InputBackend};
    use actingcommand_policy::{
        CatalogDocumentSource, CatalogSources, EvaluationFacts, EvaluationResources,
        HostResourceSnapshot, InstanceSnapshot, PoolValueSnapshot,
    };
    use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
    use actingcommand_runtime_host::{
        ExecutionBackendProvider, PolicyInputSnapshot, ProcedureBinding, ProcedureManifest,
        ResolvedExecutionInstance, RuntimeHostConfig,
    };
    use actingcommand_runtime_host::{PolicyRecomputeDirective, PolicyRecomputeReason};
    // Workflow #383 AO-7: the wake mapping moved to the host; its tests here are unchanged.
    use actingcommand_runtime_host::{policy_trigger_for_event, policy_trigger_for_family};
    use serde_json::{Value, json};
    use std::collections::{BTreeMap, VecDeque};
    use std::ffi::OsString;
    use std::io::{Cursor, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tempfile::TempDir;
    use zip::ZipWriter;
    use zip::write::FileOptions;

    #[test]
    fn policy_closeout_records_unrecorded_results_before_close() {
        use actingcommand_contract::{EventPayload, EventType, RuntimePayload};
        use actingcommand_ledger::{GlobalLedger, GlobalLedgerEvidenceConfig};
        let root = TempDir::new().expect("tempdir");
        let ids = IdentifierIssuer::new().expect("issuer");
        let host = Arc::new(
            RuntimeHost::start(
                RuntimeHostConfig::new(root.path(), b"policy-closeout-test-salt"),
                Arc::new(RecordingDeviceProvider {
                    instance_id: *ids.mint_instance_id().expect("instance").transport(),
                    input_count: Arc::new(AtomicUsize::new(0)),
                    capture_count: Arc::new(AtomicUsize::new(0)),
                }),
            )
            .expect("host"),
        );
        let retained = Arc::clone(&host);
        let monitor = ActingdError::process(ActingdCode::PolicyDriverStopped);
        monitor
            .record_lifecycle_failure(&host, RuntimeLifecycleFailureStage::PolicyMonitor)
            .expect("monitor fact");
        monitor
            .record_lifecycle_failure(&host, RuntimeLifecycleFailureStage::PolicyMonitor)
            .expect("same occurrence");
        let result = finish_policy_closeout(
            host,
            Err(monitor),
            // A shutdown failure under a code no product path emits: carried as an
            // unregistered (host) spelling so the recorded code stays the test's own.
            Err(ActingdError::coded(ErrorCode::Host {
                host_code: "policy_control_shutdown_failed",
            })),
            Err(ActingdError::process(ActingdCode::PolicyDriverPanicked)),
            Ok(()),
        );
        assert_eq!(
            result.expect_err("driver primary").code.as_str(),
            "policy_driver_panicked"
        );
        let ledger =
            GlobalLedger::open_evidence(GlobalLedgerEvidenceConfig::new(root.path()), |_| None)
                .expect("read-only before final close");
        let events = ledger.query(&EventQuery::default());
        // Ledger revision 3: each failure's code is its record's outcome; a spelling the
        // catalog does not list is relayed as `outcome_relay_unregistered` with `raw_code`.
        let failures = events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Runtime(RuntimePayload::Failed(payload)) => {
                    assert!(payload.lifecycle_failure().is_some(), "typed lifecycle");
                    let outcome = payload.outcome();
                    Some(
                        if *outcome.code()
                            == actingcommand_contract::codes::ContractCode::OutcomeRelayUnregistered
                                .code()
                        {
                            outcome
                                .value("raw_code")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                        } else {
                            outcome.code().as_str()
                        },
                    )
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(failures.len(), 4);
        for code in [
            "policy_driver_stopped",
            "policy_control_shutdown_failed",
            "policy_driver_panicked",
            "policy_driver_reference_leaked",
        ] {
            assert_eq!(
                failures.iter().filter(|failure| **failure == code).count(),
                1
            );
        }
        assert!(
            events
                .iter()
                .all(|event| event.event_type() != EventType::RuntimeLifecycleObserved)
        );
        drop(ledger);
        Arc::try_unwrap(retained)
            .ok()
            .expect("sole retained host")
            .close()
            .expect("final close");
        let ledger =
            GlobalLedger::open_evidence(GlobalLedgerEvidenceConfig::new(root.path()), |_| None)
                .expect("read-only after close");
        assert_eq!(
            ledger
                .events()
                .iter()
                .filter(|event| event.event_type() == EventType::RuntimeFailed)
                .count(),
            4
        );
    }

    /// Workflow #376 T2: the policy monitor's decision after a failed subscription.
    #[test]
    fn policy_monitor_waits_through_an_install_drain_and_otherwise_fails_loud() {
        use super::PolicySubscriptionFailureAction::{End, Fail, RetryOnce, Wait};
        let refusal = RuntimeErrorProjection::new(RuntimeErrorCode::RuntimeUnavailable, false);
        assert!(is_admission_refusal(
            ClientCode::RuntimeRequestRejected.code(),
            Some(&refusal)
        ));
        for (code, projection) in [
            (
                ClientCode::RuntimeRequestRejected.code(),
                Some(RuntimeErrorProjection::new(
                    RuntimeErrorCode::RuntimeUnavailable,
                    true,
                )),
            ),
            (
                ClientCode::RuntimeRequestRejected.code(),
                Some(RuntimeErrorProjection::new(
                    RuntimeErrorCode::InvalidRequest,
                    false,
                )),
            ),
            (ClientCode::RuntimeReceiptHeaderFailed.code(), None),
        ] {
            assert!(!is_admission_refusal(code, projection.as_ref()), "{code}");
        }

        // (shutdown_requested, admission_refusal, draining)
        let mut refusals = PolicySubscriptionRefusals::default();
        assert_eq!(refusals.failed(false, true, true), Wait);
        assert_eq!(refusals.failed(false, true, true), Wait);
        assert_eq!(refusals.failed(true, true, true), End);
        assert_eq!(refusals.failed(true, false, false), End);
        assert_eq!(refusals.failed(false, false, true), Fail);
        assert_eq!(refusals.failed(false, false, false), Fail);
        // One unexplained refusal is retried once; the second in a row fails as before.
        assert_eq!(refusals.failed(false, true, false), RetryOnce);
        assert_eq!(refusals.failed(false, true, false), Fail);
        // A successful subscription in between resets the count.
        let mut refusals = PolicySubscriptionRefusals::default();
        assert_eq!(refusals.failed(false, true, false), RetryOnce);
        refusals.succeeded();
        assert_eq!(refusals.failed(false, true, false), RetryOnce);
        // A drain or a shutdown after a retry is still explained.
        assert_eq!(refusals.failed(false, true, true), Wait);
        assert_eq!(refusals.failed(true, true, false), End);
    }

    #[test]
    fn process_adapter_requires_exact_config_argument() {
        assert!(parse_arguments(Vec::new()).is_err());
        assert!(parse_arguments(vec![OsString::from("--config")]).is_err());
        assert!(
            parse_arguments(vec![
                OsString::from("--config"),
                OsString::from("actingd.json")
            ])
            .is_ok()
        );
    }

    #[test]
    fn resident_driver_coalesces_relevant_event_bursts_without_terminal_feedback() {
        let control = PolicyDriverControl::default();
        control
            .notify(PolicyTrigger::FactsChanged)
            .expect("fact notification");
        control
            .notify(PolicyTrigger::ResourcesChanged)
            .expect("resource notification");
        control
            .notify(PolicyTrigger::CatalogChanged)
            .expect("catalog notification");
        assert!(matches!(
            control.wait(Duration::ZERO).expect("coalesced wake"),
            PolicyDriverWake::Trigger(PolicyTrigger::CatalogChanged, recomputes)
                if recomputes.is_empty()
        ));
        assert_eq!(
            policy_trigger_for_family(EventFamily::Policy),
            None,
            "terminal/settlement policy events belong to #95 and must not wake #94"
        );
        assert_eq!(
            policy_trigger_for_family(EventFamily::Fact),
            Some(PolicyTrigger::FactsChanged)
        );
        assert_eq!(
            policy_trigger_for_family(EventFamily::Performance),
            Some(PolicyTrigger::ResourcesChanged)
        );
    }

    #[test]
    fn resident_driver_shutdown_wakes_and_reclaims_the_waiter() {
        let control = Arc::new(PolicyDriverControl::default());
        let waiter = thread::spawn({
            let control = Arc::clone(&control);
            move || control.wait(Duration::from_secs(5))
        });
        thread::sleep(Duration::from_millis(20));
        let started = Instant::now();
        control.shutdown().expect("request driver shutdown");
        assert!(matches!(
            waiter.join().expect("join waiter").expect("wait result"),
            PolicyDriverWake::Shutdown
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn resident_driver_ignores_spurious_notification_until_a_real_wake() {
        let control = Arc::new(PolicyDriverControl::default());
        let finished = Arc::new(AtomicBool::new(false));
        let waiter = thread::spawn({
            let control = Arc::clone(&control);
            let finished = Arc::clone(&finished);
            move || {
                let result = control.wait(Duration::from_secs(30));
                finished.store(true, Ordering::SeqCst);
                control.changed.notify_all();
                result
            }
        });

        let state = control.state.lock().expect("driver signal");
        let (state, wait) = control
            .changed
            .wait_timeout_while(state, Duration::from_secs(5), |state| {
                state.wait_predicate_checks == 0 && !finished.load(Ordering::SeqCst)
            })
            .expect("observe waiter entering its predicate wait");
        assert!(!wait.timed_out(), "waiter did not enter its predicate wait");
        assert!(!finished.load(Ordering::SeqCst));
        let predicate_checks_before_notification = state.wait_predicate_checks;
        assert!(predicate_checks_before_notification > 0);

        control.changed.notify_all();
        let (state, wait) = control
            .changed
            .wait_timeout_while(state, Duration::from_secs(5), |state| {
                state.wait_predicate_checks <= predicate_checks_before_notification
                    && !finished.load(Ordering::SeqCst)
            })
            .expect("observe predicate recheck after spurious notification");
        assert!(
            !wait.timed_out(),
            "waiter did not recheck its predicate after notification"
        );
        let predicate_checks = state.wait_predicate_checks;
        let finished_early = finished.load(Ordering::SeqCst);
        drop(state);

        if finished_early {
            let result = waiter.join().expect("join early waiter");
            let code = match result {
                Ok(_) => "unexpected_policy_driver_wake",
                Err(error) => error.code.as_str(),
            };
            panic!("spurious notification completed the wait early: {}", code);
        }
        assert!(predicate_checks > predicate_checks_before_notification);

        control.shutdown().expect("request driver shutdown");
        assert!(matches!(
            waiter.join().expect("join waiter").expect("wait result"),
            PolicyDriverWake::Shutdown
        ));
    }

    #[test]
    fn formal_provenance_provider_uses_the_same_scheduled_owner_chain() {
        let root = TempDir::new().expect("tempdir");
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let instance_id = *ids.mint_instance_id().expect("instance id").transport();
        let package = resident_test_package();
        let package_path = root.path().join("task.zip");
        std::fs::write(&package_path, &package).expect("write package");
        let input_count = Arc::new(AtomicUsize::new(0));
        let capture_count = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(RecordingDeviceProvider {
            instance_id,
            input_count: Arc::clone(&input_count),
            capture_count: Arc::clone(&capture_count),
        });
        let catalog = resident_test_catalog();
        let manifest = ProcedureManifest::new(vec![
            ProcedureBinding::new(
                "procedure.observe",
                "operation.observe",
                vec!["after_observation".to_string()],
            )
            .expect("procedure binding")
            .with_package_path(package_path.to_string_lossy()),
            ProcedureBinding::new(
                "procedure.followup",
                "operation.observe",
                vec!["after_observation".to_string()],
            )
            .expect("followup procedure binding"),
        ])
        .expect("procedure manifest");
        let now = system_unix_ms().expect("system clock");
        let host = RuntimeHost::start(
            RuntimeHostConfig::new(root.path(), b"0123456789abcdef")
                .with_policy_inputs(PolicyInputSnapshot::new(
                    resident_test_facts(),
                    resident_test_resources(now),
                ))
                .with_procedure_manifest(manifest),
            provider,
        )
        .expect("start formal-provider runtime");
        let policy = PolicyBootstrap {
            catalog_approval_ids: vec!["approval:fixture-a".to_string()],
            catalog,
            catalog_transition: None,
            scheduled_tasks: BTreeMap::from([(
                "procedure.observe".to_string(),
                ScheduledProcedureTask {
                    request: ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
                        .expect("contained task request")
                        .with_response_deadline_ms(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS)
                        .expect("bounded scheduled request budget"),
                    mode: ScheduledExecutionMode::DeviceRegistry,
                },
            )]),
            registry_modes: BTreeMap::from([(
                "fixture-instance-a".to_string(),
                ScheduledExecutionMode::DeviceRegistry,
            )]),
            cadence: PolicyCadence::default(),
        };
        let execution = initialize_policy(&host, &policy).expect("formal scheduled Recovery cycle");
        assert_eq!(execution.cycle.pending_dispatch_intents.len(), 1);
        let [wake] = execution.recompute_wakes.as_slice() else {
            let forensic_context = match RuntimeClient::connect(
                RuntimeClientConfig::new(
                    root.path(),
                    actingcommand_runtime_client::RuntimeChannel::Cli,
                    EventActor::Agent,
                    EventSource::Adapter,
                )
                .with_io_timeout(Duration::from_secs(1)),
            ) {
                Ok(client) => {
                    let snapshot = match RuntimeEventQueryPageRequest::new(1, None) {
                        Ok(request) => client
                            .query_event_page(
                                EventQuery::default(),
                                ProjectionProfile::Forensic,
                                request,
                            )
                            .map_err(|error| format!("snapshot_read_error={error:#?}")),
                        Err(error) => Err(format!("snapshot_request_error={error:#?}")),
                    };
                    match snapshot {
                        Ok(snapshot) if snapshot.snapshot_ledger_position() > 0 => {
                            let position = snapshot.snapshot_ledger_position();
                            let from_sequence = position.saturating_sub(63).max(1);
                            let tail = match RuntimeEventQueryPageRequest::new(64, None)
                                .and_then(|request| request.at_snapshot(position))
                            {
                                Ok(request) => client
                                    .query_event_page(
                                        EventQuery {
                                            from_sequence: Some(from_sequence),
                                            to_sequence: Some(position),
                                            ..EventQuery::default()
                                        },
                                        ProjectionProfile::Forensic,
                                        request,
                                    )
                                    .map_err(|error| format!("tail_read_error={error:#?}")),
                                Err(error) => Err(format!("tail_request_error={error:#?}")),
                            };
                            format!(
                                "snapshot_request_limit=1; snapshot={snapshot:#?}; \
                                 L={position}; tail_request_range={from_sequence}..={position}; \
                                 tail_request_limit=64; tail={tail:#?}"
                            )
                        }
                        Ok(snapshot) => format!(
                            "snapshot_request_limit=1; snapshot={snapshot:#?}; \
                             L=0; tail_not_read=empty_snapshot"
                        ),
                        Err(error) => format!(
                            "snapshot_request_limit=1; {error}; \
                             L=unavailable; tail_not_read=snapshot_unavailable"
                        ),
                    }
                }
                Err(error) => format!(
                    "forensic_client_connect_error={error:#?}; \
                     L=unavailable; snapshot_and_tail_not_read=client_unavailable"
                ),
            };
            panic!(
                "one exact mapped-outcome wake\nactual_count={}\ncycle={:#?}\n\
                 recompute_wakes={:#?}\nforensic_context={forensic_context}\n\
                 coverage=one bounded tail, not the complete ledger; \
                 events outside returned pages remain unobserved; no cursor follow or retry",
                execution.recompute_wakes.len(),
                execution.cycle,
                execution.recompute_wakes,
            )
        };
        assert_eq!(
            wake.projection
                .scheduling_result()
                .identity()
                .catalog_task_id(),
            "fixture.observe"
        );
        assert_eq!(
            wake.projection
                .scheduling_result()
                .disposition()
                .outcome_key(),
            "resident-completed"
        );
        let control = PolicyDriverControl::default();
        control
            .notify_recompute(wake.clone())
            .expect("first mapped-outcome wake");
        control
            .notify_recompute(wake.clone())
            .expect("idempotent mapped-outcome wake");
        let PolicyDriverWake::Trigger(trigger, recomputes) =
            control.wait(Duration::ZERO).expect("mapped-outcome work")
        else {
            panic!("mapped outcome must wake the resident driver")
        };
        assert_eq!(trigger, PolicyTrigger::FactsChanged);
        assert_eq!(recomputes, vec![wake.clone()]);
        let signal = control.state.lock().expect("driver signal");
        assert!(
            signal.completed_recomputes.is_empty(),
            "taking exact work must not mark it completed before policy consumption"
        );
        assert_eq!(
            signal.active_recomputes.get(&wake.key()),
            Some(wake),
            "the exact obligation must remain active until policy consumption succeeds"
        );
        drop(signal);
        let client = RuntimeClient::connect(
            RuntimeClientConfig::new(
                root.path(),
                actingcommand_runtime_client::RuntimeChannel::Cli,
                EventActor::Agent,
                EventSource::Adapter,
            )
            .with_io_timeout(Duration::from_secs(1)),
        )
        .expect("connect formal-provider runtime");
        let defer_now = 10_000;
        let mut projection_schedule = PolicyDriverSchedule::new(
            policy.cadence.clone(),
            &policy_cycle(PolicyRecomputeKind::Full, defer_now),
            defer_now,
        )
        .expect("projection schedule");
        apply_policy_cycle_result(
            &control,
            &mut projection_schedule,
            trigger,
            recomputes,
            defer_now,
            Err(ActingdError::process(SharedCode::OutcomeProjectionNotReady)),
        )
        .expect("first bounded not-ready defer");
        assert!(
            projection_schedule
                .take_due_work(defer_now)
                .expect("pre-deadline projection work")
                .is_none()
        );
        let (first_deferred_trigger, first_deferred_recomputes) = projection_schedule
            .take_due_work(defer_now + policy.cadence.debounce_ms)
            .expect("first projection deadline")
            .expect("first retained exact projection work");
        assert_eq!(first_deferred_trigger, PolicyTrigger::FactsChanged);
        assert_eq!(first_deferred_recomputes, vec![wake.clone()]);
        assert_eq!(
            client
                .query_events(
                    EventQuery {
                        event_type: Some(EventType::PolicyDispatchIntent),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Forensic,
                )
                .expect("query intents before projection readiness")
                .len(),
            1,
            "not-ready projection attempts must not dispatch a successor"
        );
        apply_policy_cycle_result(
            &control,
            &mut projection_schedule,
            first_deferred_trigger,
            first_deferred_recomputes,
            defer_now + policy.cadence.debounce_ms,
            Err(ActingdError::process(SharedCode::OutcomeProjectionNotReady)),
        )
        .expect("second bounded not-ready defer");
        let (second_deferred_trigger, second_deferred_recomputes) = projection_schedule
            .take_due_work(defer_now + (policy.cadence.debounce_ms * 2))
            .expect("second projection deadline")
            .expect("second retained exact projection work");
        assert_eq!(second_deferred_trigger, PolicyTrigger::FactsChanged);
        assert_eq!(second_deferred_recomputes, vec![wake.clone()]);
        assert_eq!(
            client
                .query_events(
                    EventQuery {
                        event_type: Some(EventType::PolicyDispatchIntent),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Forensic,
                )
                .expect("query intents after second not-ready result")
                .len(),
            1,
            "a second not-ready projection must still produce zero successor dispatches"
        );
        thread::sleep(Duration::from_millis(policy.cadence.cooldown_ms + 10));
        let ready = execute_policy_cycle(&host, &policy, second_deferred_trigger);
        assert_eq!(
            ready
                .as_ref()
                .expect("third projection attempt is ready")
                .cycle
                .pending_dispatch_intents
                .iter()
                .filter(|intent| intent.task_id == "fixture.followup")
                .count(),
            1,
            "the evaluator, not the wake, must decide exactly one successor"
        );
        apply_policy_cycle_result(
            &control,
            &mut projection_schedule,
            second_deferred_trigger,
            second_deferred_recomputes,
            defer_now + (policy.cadence.debounce_ms * 2),
            ready,
        )
        .expect("third cycle consumes the ready exact projection");
        control
            .notify_recompute(wake.clone())
            .expect("replayed completed exact wake");
        assert!(matches!(
            control.wait(Duration::ZERO).expect("deduplicated replay"),
            PolicyDriverWake::TimedOut
        ));
        let conflict = PolicyRecomputeWake {
            projection: SchedulingOutcomeProjection::new(
                wake.projection.ledger_position(),
                actingcommand_contract::AuthoritativeSchedulingOutcome::new(
                    wake.projection.scheduling_result().identity().clone(),
                    actingcommand_contract::SchedulingDisposition::new(
                        "resident-conflict",
                        wake.projection
                            .scheduling_result()
                            .disposition()
                            .effect()
                            .clone(),
                    )
                    .expect("conflicting disposition"),
                    wake.projection
                        .scheduling_result()
                        .terminal_timestamp_unix_ms(),
                )
                .expect("conflicting outcome"),
            )
            .expect("conflicting projection"),
        };
        assert_eq!(
            control
                .notify_recompute(conflict)
                .expect_err("same terminal identity cannot change outcome")
                .code
                .as_str(),
            "policy_recompute_wake_identity_conflict"
        );
        assert_eq!(input_count.load(Ordering::SeqCst), 1);
        assert_eq!(capture_count.load(Ordering::SeqCst), 2);

        let events = client
            .query_events(EventQuery::default(), ProjectionProfile::Forensic)
            .expect("query formal-provider owner chain");
        // Workflow #269 B16: the real driver must pass its request budget into admission.
        assert!(events.iter().any(|event| matches!(
            &event.payload,
            ProjectionPayload::Full(payload)
                if matches!(payload.as_ref(), EventPayload::Task(actingcommand_contract::TaskPayload::Semantic(payload))
                    if payload.lease_expires_at_monotonic_ms().is_some_and(|expiry|
                        expiry > ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS)
                    && matches!(payload.fact(), actingcommand_contract::TaskSemanticFact::PackageAdmitted {
                        response_deadline_monotonic_ms: Some(deadline),
                        ..
                    } if *deadline >= ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS))
        )));
        for event_type in [
            EventType::PolicyDispatchIntent,
            EventType::PolicyDispatchAdmitted,
            EventType::LeaseGranted,
            EventType::TaskRequested,
            EventType::InputCommitted,
            EventType::TaskCompleted,
            EventType::LeaseReleased,
            EventType::PolicyExecutionRecorded,
            EventType::PolicyDispatchCompleted,
        ] {
            // PR346 CI34248791592: both selected dispatches pass real admission;
            // only the primary has a scheduled resource execution in this fixture.
            let expected = if matches!(
                event_type,
                EventType::PolicyDispatchIntent
                    | EventType::PolicyDispatchAdmitted
                    | EventType::LeaseGranted
            ) {
                2
            } else {
                1
            };
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event_type == event_type)
                    .count(),
                expected,
                "{event_type:?}"
            );
        }
        let followup_intents = events
            .iter()
            .filter(|event| {
                matches!(
                    &event.payload,
                    ProjectionPayload::Full(payload)
                        if matches!(
                            payload.as_ref(),
                            EventPayload::Policy(PolicyPayload::DispatchIntent(payload))
                                if payload.task_id() == "fixture.followup"
                        )
                )
            })
            .count();
        assert_eq!(followup_intents, 1);
        drop(client);
        host.close().expect("close formal-provider runtime");
    }

    #[test]
    fn resident_schedule_has_bounded_idle_reconciliation_and_clock_jump_wakes() {
        let now = 10_000;
        let cadence = PolicyCadence {
            debounce_ms: 10,
            cooldown_ms: 20,
            reconciliation_interval_ms: 1_000,
            clock_jump_threshold_ms: 2_000,
        };
        let cycle = policy_cycle(PolicyRecomputeKind::Full, now);
        let mut schedule =
            PolicyDriverSchedule::new(cadence, &cycle, now).expect("resident schedule");
        assert_eq!(schedule.wait_duration(now), Duration::from_millis(1_000));
        assert_eq!(
            schedule
                .take_due_work(now + 1_000)
                .expect("reconciliation deadline"),
            Some((PolicyTrigger::Reconciliation, Vec::new()))
        );

        let cadence = PolicyCadence {
            debounce_ms: 10,
            cooldown_ms: 20,
            reconciliation_interval_ms: 60_000,
            clock_jump_threshold_ms: 1_000,
        };
        let cycle = policy_cycle(PolicyRecomputeKind::Full, now);
        let mut schedule = PolicyDriverSchedule::new(cadence, &cycle, now).expect("clock schedule");
        assert_eq!(
            schedule.take_due_work(now + 1_001).expect("clock jump"),
            Some((
                PolicyTrigger::ClockObserved {
                    previous_unix_ms: now
                },
                Vec::new()
            ))
        );
    }

    struct RecordingDeviceProvider {
        instance_id: actingcommand_contract::InstanceId,
        input_count: Arc<AtomicUsize>,
        capture_count: Arc<AtomicUsize>,
    }

    impl ExecutionBackendProvider for RecordingDeviceProvider {
        fn instance_aliases(&self) -> Vec<String> {
            vec!["fixture-instance-a".to_string()]
        }

        fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
            (instance_alias == "fixture-instance-a")
                .then(|| ResolvedExecutionInstance::new(self.instance_id, "recording-device"))
        }

        fn open_input(
            &self,
            _instance_alias: &str,
        ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn InputBackend>>> {
            let open = || -> DeviceResult<Box<dyn InputBackend>> {
                Ok(Box::new(RecordingInput {
                    count: Arc::clone(&self.input_count),
                }))
            };
            let result: DeviceResult<Box<dyn InputBackend>> = open();
            result.map(|backend| {
                actingcommand_device::OpenedBackend::unobserved(
                    backend,
                    actingcommand_contract::BackendOpenEntry::Input,
                )
            })
        }

        fn open_capture(
            &self,
            _instance_alias: &str,
            _memory: Option<&actingcommand_device::FrameMemoryBudget>,
        ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn CaptureBackend>>> {
            let open = || -> DeviceResult<Box<dyn CaptureBackend>> {
                Ok(Box::new(RecordingCapture {
                    count: Arc::clone(&self.capture_count),
                    frames: VecDeque::from([
                        crate::c4_support::page_frame([255, 0, 0]).expect("home frame"),
                        crate::c4_support::page_frame([0, 0, 255]).expect("terminal frame"),
                    ]),
                }))
            };
            let result: DeviceResult<Box<dyn CaptureBackend>> = open();
            result.map(|backend| {
                actingcommand_device::OpenedBackend::unobserved(
                    backend,
                    actingcommand_contract::BackendOpenEntry::Capture,
                )
            })
        }

        // Slice #316-B3: the fake device always reports its assigned application in the
        // foreground, so the foreground gate passes exactly as before the gate existed.
        fn observe_foreground_application(
            &self,
            _instance_alias: &str,
        ) -> DeviceResult<actingcommand_runtime_host::ForegroundApplicationObservation> {
            Ok(crate::c4_support::neutral_foreground())
        }

        fn control_application(
            &self,
            _witness: &actingcommand_contract::FencedWrite,
            _instance_alias: &str,
            _action: ApplicationLifecycleAction,
        ) -> DeviceResult<()> {
            Err(DeviceError::fatal(
                "recording provider application control is forbidden",
            ))
        }
    }

    struct RecordingCapture {
        count: Arc<AtomicUsize>,
        frames: VecDeque<Frame>,
    }

    impl CaptureBackend for RecordingCapture {
        fn capture(&mut self) -> DeviceResult<Frame> {
            let frame = self
                .frames
                .pop_front()
                .ok_or_else(|| DeviceError::fatal("recording capture exhausted"))?;
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(frame)
        }
        fn observe_geometry(
            &mut self,
            _deadline: std::time::Instant,
        ) -> DeviceResult<actingcommand_contract::CaptureGeometryObservation> {
            // The same 16x9 extent of the recorded frames.
            Ok(crate::c4_support::physical_geometry_16x9(
                "recording-device",
            ))
        }
        fn close_once(
            &mut self,
            _authority: actingcommand_device::DeviceCloseAuthority,
        ) -> DeviceResult<actingcommand_device::DeviceResourceCloseOutcome> {
            Ok(actingcommand_device::DeviceResourceCloseOutcome::confirmed(
                0,
            ))
        }
    }

    struct RecordingInput {
        count: Arc<AtomicUsize>,
    }

    impl RecordingInput {
        fn record(&self) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl InputBackend for RecordingInput {
        fn tap(
            &mut self,
            _witness: &actingcommand_contract::FencedWrite,
            _x: i32,
            _y: i32,
        ) -> DeviceResult<()> {
            self.record();
            Ok(())
        }

        fn long_tap(
            &mut self,
            _witness: &actingcommand_contract::FencedWrite,
            _x: i32,
            _y: i32,
            _duration_ms: u64,
        ) -> DeviceResult<()> {
            self.record();
            Ok(())
        }

        fn swipe(
            &mut self,
            _witness: &actingcommand_contract::FencedWrite,
            _x1: i32,
            _y1: i32,
            _x2: i32,
            _y2: i32,
            _duration_ms: u64,
        ) -> DeviceResult<()> {
            self.record();
            Ok(())
        }

        fn key(
            &mut self,
            _witness: &actingcommand_contract::FencedWrite,
            _key: &str,
        ) -> DeviceResult<()> {
            self.record();
            Ok(())
        }

        fn text(
            &mut self,
            _witness: &actingcommand_contract::FencedWrite,
            _text: &str,
        ) -> DeviceResult<()> {
            self.record();
            Ok(())
        }

        fn reset(&mut self, _witness: &actingcommand_contract::FencedWrite) -> DeviceResult<()> {
            self.record();
            Ok(())
        }

        fn close_once(
            &mut self,
            _authority: actingcommand_device::DeviceCloseAuthority,
        ) -> DeviceResult<actingcommand_device::DeviceResourceCloseOutcome> {
            Ok(actingcommand_device::DeviceResourceCloseOutcome::confirmed(
                0,
            ))
        }
    }

    fn resident_test_catalog() -> CatalogSources {
        let mut sources = CatalogSources {
            tasks: CatalogDocumentSource::new(
                "memory://resident/tasks.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/tasks.json")
                    .to_vec(),
            ),
            pools: CatalogDocumentSource::new(
                "memory://resident/pools.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/pools.json")
                    .to_vec(),
            ),
            activity: CatalogDocumentSource::new(
                "memory://resident/activity.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/activity.json")
                    .to_vec(),
            ),
            timeline: CatalogDocumentSource::new(
                "memory://resident/timeline.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/timeline.json")
                    .to_vec(),
            ),
            selection: None,
        };
        let mut tasks: Value =
            serde_json::from_slice(&sources.tasks.bytes).expect("task catalog JSON");
        tasks["tasks"][0]["feedback_stop"] = json!({
            "kind": "clock",
            "schedule": {
                "kind": "at",
                "clock_source": {
                    "kind": "server",
                    "timezone_id": "etc/utc",
                    "utc_offset_minutes": 0,
                    "dst_offset_minutes": 0,
                    "maintenance_drift_ms": 0
                },
                "at_ms": 4_102_444_800_000_u64
            }
        });
        let mut followup = tasks["tasks"][0].clone();
        followup["id"] = json!("fixture.followup");
        followup["procedure_ref"] = json!("procedure.followup");
        followup["priority"] = json!(200);
        followup["loop_budget"]["daily_limit"] = json!(1);
        followup["loop_budget"]["window_iteration_limit"] = json!(1);
        followup["trigger"] = json!({
            "kind": "any",
            "predicates": [
                {
                    "kind": "outcome",
                    "task_id": "fixture.observe",
                    "outcome_key": "resident-completed",
                    "comparison": "eq",
                    "value": {"type": "boolean", "value": true}
                },
                {
                    "kind": "outcome",
                    "task_id": "fixture.observe",
                    "outcome_key": "resident-no-effect",
                    "comparison": "eq",
                    "value": {"type": "boolean", "value": true}
                }
            ]
        });
        followup["feedback_stop"] = json!({
            "kind": "clock",
            "schedule": {
                "kind": "at",
                "clock_source": {
                    "kind": "server",
                    "timezone_id": "etc/utc",
                    "utc_offset_minutes": 0,
                    "dst_offset_minutes": 0,
                    "maintenance_drift_ms": 0
                },
                "at_ms": 4_102_444_800_000_u64
            }
        });
        followup["produces"] = json!([]);
        followup["instance_overrides"] = json!([]);
        tasks["tasks"]
            .as_array_mut()
            .expect("resident task array")
            .push(followup);
        sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("task catalog bytes");
        let mut activity: Value =
            serde_json::from_slice(&sources.activity.bytes).expect("activity catalog JSON");
        activity["profiles"][0]["windows"][0]["start_minute_of_day"] = json!(0);
        activity["profiles"][0]["windows"][0]["end_minute_of_day"] = json!(0);
        // PR346 CI34247423098: the successor assertion runs after driver cooldown.
        activity["profiles"][0]["minimum_interval_ms"] = json!(1);
        activity["profiles"][0]["maximum_interval_ms"] = json!(1);
        sources.activity.bytes =
            serde_json::to_vec_pretty(&activity).expect("activity catalog bytes");
        sources
    }

    fn resident_test_facts() -> EvaluationFacts {
        EvaluationFacts {
            ledger_position: 0,
            fact_snapshot_id: "snapshot:resident".to_string(),
            facts: Vec::new(),
            outcomes: Vec::new(),
            tasks: Vec::new(),
            instances: vec![InstanceSnapshot {
                instance_id: "fixture-instance-a".to_string(),
                server_id: "fixture-server-a".to_string(),
                game_id: "fixture-game-a".to_string(),
                host_id: "fixture-host-a".to_string(),
                available: true,
                capability_operation_ids: vec!["operation.observe".to_string()],
                preferred_task_ids: Vec::new(),
            }],
            priority_offsets: Vec::new(),
        }
    }

    fn resident_test_resources(now_unix_ms: u64) -> EvaluationResources {
        EvaluationResources {
            pools: vec![PoolValueSnapshot {
                pool_id: "fixture-pool-a".to_string(),
                value: 10,
                observed_at_unix_ms: now_unix_ms,
            }],
            hosts: vec![HostResourceSnapshot {
                host_id: "fixture-host-a".to_string(),
                cpu_available_milli: 1_000,
                gpu_available_milli: 1_000,
                io_available_milli: 1_000,
                host_responsiveness_basis_points: 10_000,
                third_party_pressure_basis_points: 0,
                heavy_dispatch_limit: 1,
                active_heavy_dispatches: 0,
            }],
        }
    }

    fn resident_test_package() -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(cursor);
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let files: &[(&str, &[u8])] = &[
            // Workflow #330 CI36549559197/CI36325595894: the whole-task budget covers the real
            // ledger and artifact path under a parallel test run; this test does not verify it.
            (
                "control.json",
                br#"{
                    "schema_version":"Lab-1y.control.v1",
                    "package_id":"neutral.semantic.task",
                    "execution_mode":"navigable_route",
                    "game":"neutral",
                    "server":"test",
                    "resolution":{"width":16,"height":9},
                    "entry_task_id":"task",
                    "capture_interval_ms":50,
                    "step_timeout_ms":50,
                    "timeout_ms":10000,
                    "max_steps":2
                }"#,
            ),
            (
                "resources/manifest.json",
                br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
            ),
            (
                "resources/operations/task/task.json",
                br#"{
                    "schema_version":"0.6",
                    "task_id":"task",
                    "game":"neutral",
                    "server_scope":["test"],
                    "coordinate_space":{"width":16,"height":9},
                    "entry_page":"home",
                    "target_page":"terminal",
                    "scheduling_outcome":{
                        "designated_operation":"open_terminal",
                        "mappings":[
                            {
                                "outcome_key":"resident-completed",
                                "effect":"designated_effect_completed",
                                "terminal_pages":["terminal"]
                            },
                            {
                                "outcome_key":"resident-no-effect",
                                "effect":"no_designated_effect",
                                "terminal_pages":["terminal"]
                            }
                        ]
                    },
                    "operations":[{
                        "id":"open_terminal",
                        "from":"home",
                        "to":"terminal",
                        "click":{"kind":"point","x":1,"y":0},
                        "unguarded_trusted_coordinate":true
                    }]
                }"#,
            ),
            (
                "resources/recognition/neutral.test.pack.json",
                br#"{
                    "schema_version":"0.3",
                    "game":"neutral",
                    "server":"test",
                    "coordinate_space":{"width":16,"height":9},
                    "defaults":{"color_max_distance":0.0},
                    "targets":[
                        {"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                        {"type":"color","id":"page/terminal","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]}
                    ]
                }"#,
            ),
            (
                "resources/recognition/neutral.test.pages.json",
                br#"{
                    "schema_version":"0.3",
                    "pages":[
                        {"id":"neutral/home","required":["page/home"],"optional":[],"forbidden":[]},
                        {"id":"neutral/terminal","required":["page/terminal"],"optional":[],"forbidden":[]}
                    ]
                }"#,
            ),
        ];
        for (path, contents) in files {
            zip.start_file(*path, options).expect("zip entry");
            zip.write_all(contents).expect("zip content");
        }
        zip.finish().expect("finish zip").into_inner()
    }

    fn policy_cycle(kind: PolicyRecomputeKind, eligible_at_unix_ms: u64) -> PolicyCycle {
        PolicyCycle {
            directive: PolicyRecomputeDirective {
                kind,
                reason: PolicyRecomputeReason::StartupOrRecovery,
                eligible_at_unix_ms,
            },
            evaluation: None,
            pending_dispatch_intents: Vec::new(),
            detection_planning_signals: Vec::new(),
            clock_slot_signals: Vec::new(),
            measurement: None,
        }
    }

    #[test]
    fn lease_ends_wake_the_resident_driver_and_capture_and_fact_records_do_not() {
        // Workflow #369 W-1: a lease end returns a key; nothing else of the lease, scheduler,
        // capture or runtime-fact families wakes the driver.
        for event_type in [
            EventType::LeaseReleased,
            EventType::LeaseTransferred,
            EventType::LeaseExpired,
        ] {
            assert_eq!(
                policy_trigger_for_event(event_type),
                Some(PolicyTrigger::ResourcesChanged),
                "{event_type:?}"
            );
        }
        for event_type in [
            EventType::LeaseRequested,
            EventType::LeaseTransitionIntent,
            EventType::LeaseGranted,
            EventType::LeaseRenewed,
            EventType::SchedulerQueued,
            EventType::SchedulerAdmitted,
            EventType::SchedulerDenied,
            EventType::CaptureRequested,
            EventType::CaptureCompleted,
            EventType::CaptureFailed,
            EventType::CapturePressureChanged,
            EventType::CaptureDedupWindow,
            EventType::CapturePolicyChanged,
            EventType::CaptureSummaryCommitted,
            EventType::RuntimeFactRecorded,
        ] {
            assert_eq!(policy_trigger_for_event(event_type), None, "{event_type:?}");
        }
    }

    #[test]
    fn a_failed_run_skips_its_instance_for_the_rest_of_the_cycle() {
        // Workflow #369 W-1 (review M-6): after a failed run the cycle writes no further intent
        // for that instance, while other instances' intents still run.
        let mut failed = FailedRunInstances::default();
        assert!(!failed.any());
        assert!(!failed.skips("fixture-instance-a"));
        failed.record("fixture-instance-a");
        assert!(failed.any());
        assert!(failed.skips("fixture-instance-a"));
        assert!(!failed.skips("fixture-instance-b"));
    }
}
