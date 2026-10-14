// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl start` and `actingctl restart` (Workflow #389 MCP-10; command table v1.1 §4.11).
//! Starting the Runtime belongs to the person and the watchdog and is no Runtime request, so
//! both commands exist in the CLI only and have no MCP tool. Like `watchdog` they take `--root`
//! and leave before `Invocation::parse`.
//!
//! - `start` refuses a state root whose owner lock a live process holds, and then changes
//!   nothing. While acsetup holds `install\writer.lock` it refuses as the watchdog does (row
//!   7a, `watchdog_installer_busy`), again with nothing changed. Otherwise it holds the
//!   watchdog's `<root>\watchdog\run.lock`, so no tick starts beside it, starts through the
//!   watchdog's manual start (WMI `Win32_Process.Create`, hidden window, outside the caller's
//!   job, stdout and stderr appended to a log), waits for an answering owner and reads Status
//!   once for the pauses the new owner restored.
//! - `restart` takes the same installer check before it stops anything, waits until every
//!   instance is idle (no lease, nothing queued) or refuses with nothing stopped, requests the
//!   shutdown and waits for it as `request-shutdown --wait 60` does, then starts as `start`
//!   does. While it waits for idle it reads Status every 10 s and once more at the end of
//!   `--wait-idle`: each Status appends a ledger observation (command table §2.5), so a long
//!   wait must not read it every second.
//!
//! The root is an A/B installation (the selection of `install\active.json`, as the watchdog
//! starts it) or, with `--config`, a verification root started with that config. Processes are
//! recognised by the owner lock, runtime-info and the accepted shutdown target only, never by
//! image name.
//!
//! Workflow #378 A5: both print one result line through `crate::cli_line`: on success
//! `{…report, outcome}` with `start_runtime_started` or `start_runtime_restarted` (`pid`,
//! `owner_epoch`, `log`), on failure `{outcome}` with the failure. A refusal the watchdog
//! decides as info (the installer holds its writer lock, a tick holds the run lock) is
//! `start_runtime_refused` caused by that decision, so it exits 1.

use crate::ActingctlError;
use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::shutdown_wait;
use crate::watchdog::{self, decide, io_failure, json_failure, observe, start};
use actingcommand_contract::outcome::{Category, CauseRelation, IoOp, Outcome};
use actingcommand_contract::{
    EventActor, EventSource, InstancePauseState, OwnerEpoch, RuntimeReceipt, SchedulingPauseState,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use serde::Serialize;
use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const USAGE: &str = "usage: actingctl start --root <root> [--config <file>] [--state-root <dir>] [--ready-timeout-s <s>]\nusage: actingctl restart --root <root> [--config <file>] [--state-root <dir>] [--wait-idle <s>] [--ready-timeout-s <s>]";
/// Upper bound of `--ready-timeout-s` and `--wait-idle`, as of `request-shutdown --wait`.
const MAX_WAIT_SECONDS: u64 = 3600;
/// `restart` waits for the shutdown as `request-shutdown --wait 60` does.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(60);
/// A watchdog tick holds `run.lock` through its own start: wait for it as `watchdog uninstall`
/// does, up to the tick's readiness window and its probes.
const RUN_LOCK_WAIT: Duration = Duration::from_millis(decide::READY_TIMEOUT_MS + 20_000);
/// How often `restart --wait-idle` reads Status again (train 4 review ruling): every Status
/// appends a ledger observation (command table §2.5), so the wait reads it every 10 s, and once
/// more when `--wait-idle` ends.
const IDLE_POLL: Duration = Duration::from_secs(10);
/// The bound of a configuration file (the bound of an installed selection's config).
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// Runs `start` or `restart` when the first argument names one; `None` leaves every other
/// command to the CLI, unchanged.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    let mode = match arguments.first()?.to_str()? {
        "start" => Mode::Start,
        "restart" => Mode::Restart,
        _ => return None,
    };
    if let Err(invalid) = cli_line::detail() {
        return Some(cli_line::failure(&invalid, None));
    }
    let parsed = match Arguments::parse(mode, &arguments[1..]) {
        Ok(parsed) => parsed,
        Err(usage) => return Some(cli_line::failure(&usage, Some(USAGE))),
    };
    Some(match run(mode, &parsed) {
        Ok((report, outcome)) => match serde_json::to_value(&report) {
            Ok(value) => cli_line::success(cli_line::members(value, "result"), &outcome),
            Err(error) => {
                cli_line::developer_line(
                    &Outcome::new(ActingctlCode::ActingctlOutputFailed),
                    Some(&error.to_string()),
                );
                ExitCode::FAILURE
            }
        },
        // A Runtime refusal topped with an info or warning code still failed (A4 part B L2):
        // `crate::failed` exits non-zero whatever the top's category.
        Err(failed) => crate::failed(&failed.outcome, failed.text.as_deref()),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Start,
    Restart,
}

/// The flags of one `start` or `restart`; each at most once, `--wait-idle` for `restart` only.
#[derive(Debug, PartialEq, Eq)]
struct Arguments {
    root: PathBuf,
    config: Option<PathBuf>,
    state_root: Option<PathBuf>,
    ready_timeout: Duration,
    wait_idle: Duration,
}

impl Arguments {
    /// A flag the subcommand does not take, a flag given twice and a stray word are refused by
    /// name (Workflow #382 A1-FLAG); any other usage error unnamed.
    fn parse(mode: Mode, arguments: &[OsString]) -> Result<Self, Outcome> {
        let usage = || Outcome::new(ActingctlCode::ActingctlUsageInvalid);
        let mut root = None;
        let mut config = None;
        let mut state_root = None;
        let mut ready_timeout = None;
        let mut wait_idle = None;
        let mut index = 0;
        while index < arguments.len() {
            let flag = arguments[index].to_str().ok_or_else(usage)?;
            let given = match flag {
                "--root" => root.is_some(),
                "--config" => config.is_some(),
                "--state-root" => state_root.is_some(),
                "--ready-timeout-s" => ready_timeout.is_some(),
                "--wait-idle" if mode == Mode::Restart => wait_idle.is_some(),
                _ => {
                    return Err(cli_line::usage_invalid(
                        Some(flag),
                        ArgumentProblem::Unknown,
                    ));
                }
            };
            if given {
                return Err(cli_line::usage_invalid(
                    Some(flag),
                    ArgumentProblem::Repeated,
                ));
            }
            index += 1;
            let value = arguments.get(index).ok_or_else(usage)?;
            match flag {
                "--root" => root = Some(PathBuf::from(value)),
                "--config" => config = Some(PathBuf::from(value)),
                "--state-root" => state_root = Some(PathBuf::from(value)),
                "--ready-timeout-s" => {
                    ready_timeout = Some(seconds(value, 1..=MAX_WAIT_SECONDS).ok_or_else(usage)?);
                }
                "--wait-idle" => {
                    wait_idle = Some(seconds(value, 0..=MAX_WAIT_SECONDS).ok_or_else(usage)?);
                }
                // Every flag taken above has its arm.
                _ => return Err(usage()),
            }
            index += 1;
        }
        Ok(Self {
            root: root.ok_or_else(usage)?,
            config,
            state_root,
            ready_timeout: ready_timeout.unwrap_or(Duration::from_millis(decide::READY_TIMEOUT_MS)),
            wait_idle: wait_idle.unwrap_or(Duration::ZERO),
        })
    }
}

fn seconds(value: &OsStr, range: std::ops::RangeInclusive<u64>) -> Option<Duration> {
    let seconds = value.to_str()?.parse::<u64>().ok()?;
    range
        .contains(&seconds)
        .then_some(Duration::from_secs(seconds))
}

/// A failure of `start` or `restart`: its outcome, and developer text for the stderr line (what
/// was already done, the start log's last line).
#[derive(Debug)]
struct Failed {
    outcome: Outcome,
    text: Option<String>,
}

impl Failed {
    fn new(outcome: Outcome) -> Self {
        Self {
            outcome,
            text: None,
        }
    }

    fn runtime(error: &RuntimeClientError) -> Self {
        Self::new(error.outcome())
    }

    fn after(self, done: &str) -> Self {
        Self {
            outcome: self.outcome,
            text: Some(match self.text {
                Some(text) => format!("{text}; {done}"),
                None => done.to_owned(),
            }),
        }
    }
}

impl From<Outcome> for Failed {
    fn from(outcome: Outcome) -> Self {
        Self::new(outcome)
    }
}

impl From<ActingctlError> for Failed {
    fn from(error: ActingctlError) -> Self {
        Self::new(error.outcome())
    }
}

/// A watchdog refusal: an info decision (`watchdog_installer_busy`, `watchdog_run_in_progress`)
/// under `start_runtime_refused`, so it exits 1; any other failure as it is.
fn refused(failure: Outcome) -> Failed {
    if failure.category() < Category::Error {
        Failed::new(Outcome::new(ActingctlCode::StartRuntimeRefused).caused_by(failure))
    } else {
        Failed::new(failure)
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum Report {
    Start(StartReport),
    Restart {
        shutdown: Box<ShutdownReport>,
        start: StartReport,
    },
}

/// `start`'s output (§4.11): the answering owner, how it was started and the pauses it holds.
#[derive(Serialize)]
struct StartReport {
    pid: u32,
    owner_epoch: OwnerEpoch,
    method: &'static str,
    config: String,
    log: String,
    ready_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    global_pause: Option<SchedulingPauseState>,
    paused_instances: Vec<PausedInstance>,
}

#[derive(Serialize)]
struct PausedInstance {
    instance_alias: String,
    pause: InstancePauseState,
}

/// The accepted shutdown and its wait (the members `request-shutdown --wait` prints).
#[derive(Serialize)]
struct ShutdownReport {
    receipt: RuntimeReceipt,
    #[serde(flatten)]
    wait: Value,
}

/// The report and its success outcome: `start_runtime_started` or `start_runtime_restarted`,
/// with the answering owner's `pid`, `owner_epoch` and the start `log`.
fn run(mode: Mode, arguments: &Arguments) -> Result<(Report, Outcome), Failed> {
    let located = Located::resolve(arguments)?;
    let target = start::Target {
        root_plain: &located.root_plain,
        config_plain: &located.config_plain,
        state_root: &located.state_root,
        ready_timeout: arguments.ready_timeout,
    };
    let report = match mode {
        Mode::Start => Report::Start(start_runtime(&located, &target)?),
        Mode::Restart => restart_runtime(&located, &target, arguments.wait_idle)?,
    };
    let outcome = match &report {
        Report::Start(start) => started(ActingctlCode::StartRuntimeStarted, start),
        Report::Restart { start, .. } => started(ActingctlCode::StartRuntimeRestarted, start),
    };
    Ok((report, outcome))
}

fn started(code: ActingctlCode, report: &StartReport) -> Outcome {
    let outcome = Outcome::new(code).pid(report.pid).log(&report.log);
    match serde_json::to_value(report.owner_epoch) {
        Ok(Value::String(epoch)) => outcome.owner_epoch(epoch),
        _ => outcome,
    }
}

/// The root, the config the fixed entry gets and the state root whose owner is watched.
struct Located {
    root: PathBuf,
    root_plain: PathBuf,
    config_plain: PathBuf,
    state_root: PathBuf,
    /// An A/B root's selection, holding its slot pinned for the command as a watchdog tick does.
    _installation: Option<observe::Installation>,
}

impl Located {
    fn resolve(arguments: &Arguments) -> Result<Self, Failed> {
        let root = fs::canonicalize(&arguments.root).map_err(|error| {
            io_failure(
                ActingctlCode::InstallRootUnresolved,
                &arguments.root,
                &error,
                IoOp::Canonicalize,
            )
        })?;
        let (installation, config_plain, configured) = match &arguments.config {
            // The selected generation, exactly as the watchdog starts it.
            None => {
                let installation = observe::Installation::resolve(&root).map_err(|failure| {
                    let hint = failure.is(ActingctlCode::WatchdogLayoutUnknown);
                    let failed = Failed::new(failure);
                    if hint {
                        failed.after("pass --config <file> to start a root without one")
                    } else {
                        failed
                    }
                })?;
                let config = installation.config_plain.clone();
                let state_root = installation.state_root.clone();
                (Some(installation), config, state_root)
            }
            Some(config) => {
                let unreadable = |path: &Path, error: &std::io::Error, op: IoOp| {
                    io_failure(ActingctlCode::WatchdogConfigUnreadable, path, error, op)
                };
                let config = fs::canonicalize(config)
                    .map_err(|error| unreadable(config, &error, IoOp::Canonicalize))?;
                let mut bytes = Vec::new();
                File::open(&config)
                    .map_err(|error| unreadable(&config, &error, IoOp::Open))?
                    .take(MAX_CONFIG_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|error| unreadable(&config, &error, IoOp::Read))?;
                if bytes.len() as u64 > MAX_CONFIG_BYTES {
                    return Err(Failed::new(
                        Outcome::new(ActingctlCode::WatchdogConfigUnreadable)
                            .path(&config)
                            .limit_bytes(MAX_CONFIG_BYTES),
                    ));
                }
                let config = observe::plain_path(&config);
                let state_root = configured_state_root(&bytes, &config)?;
                (None, config, state_root)
            }
        };
        let state_root = match &arguments.state_root {
            None => configured,
            Some(requested) => same_state_root(requested, configured)?,
        };
        Ok(Self {
            root_plain: observe::plain_path(&root),
            root,
            config_plain,
            state_root,
            _installation: installation,
        })
    }
}

/// The config's absolute `state_root`, as the watchdog reads a selected config: a config that
/// is not JSON is `watchdog_config_unreadable`, one without an absolute `state_root`
/// `watchdog_state_root_invalid`, each with the config's `path`.
fn configured_state_root(bytes: &[u8], config: &Path) -> Result<PathBuf, Outcome> {
    let document: Value = serde_json::from_slice(bytes).map_err(|error| {
        Outcome::new(ActingctlCode::WatchdogConfigUnreadable)
            .path(config)
            .caused_by(json_failure(&error))
    })?;
    document
        .get("state_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| Outcome::new(ActingctlCode::WatchdogStateRootInvalid).path(config))
}

/// `--state-root` only checks the config's state root: the same spelling, or the same
/// directory once both resolve; otherwise `install_process_state_root_mismatch` with the
/// requested `path` and the config's `state_root`.
fn same_state_root(requested: &Path, configured: PathBuf) -> Result<PathBuf, Outcome> {
    if requested == configured {
        return Ok(configured);
    }
    match (fs::canonicalize(requested), fs::canonicalize(&configured)) {
        (Ok(requested), Ok(resolved)) if requested == resolved => Ok(configured),
        _ => Err(Outcome::new(ActingctlCode::InstallProcessStateRootMismatch)
            .path(requested)
            .state_root(&configured)),
    }
}

/// Step 1: a live Runtime on this state root (its owner lock held) refuses the start with
/// `start_runtime_already_running` (`path` = the state root; the answering owner's `pid` and
/// `owner_epoch`, or the client's failure as a `related` link when it does not answer).
fn refuse_live(state_root: &Path) -> Result<(), Failed> {
    let (lock, _) = observe::owner_journal(state_root)?;
    if lock != decide::OwnerLock::Locked {
        return Ok(());
    }
    let refusal = Outcome::new(ActingctlCode::StartRuntimeAlreadyRunning).path(state_root);
    Err(Failed::new(match observe::live_owner(state_root, None) {
        Ok(owner) => refusal.pid(owner.pid).owner_epoch(owner.owner_epoch),
        Err(failure) => refusal.link(CauseRelation::Related, failure),
    }))
}

/// Row 7a as the watchdog decides it (`watchdog/decide.rs`), through its own probe: while
/// acsetup holds `install\writer.lock` nothing starts, refused with `start_runtime_refused`
/// caused by the watchdog's `watchdog_installer_busy`. A free lock comes back held shared, so
/// acsetup cannot begin before the guard is dropped, as during a tick's start (review L4).
fn refuse_installer_busy(root: &Path) -> Result<Option<File>, Failed> {
    match observe::writer_lock(root)? {
        observe::WriterProbe::Free(guard) => Ok(guard),
        observe::WriterProbe::Busy => Err(refused(decide::Decision::InstallerBusy.outcome())),
    }
}

fn start_runtime(located: &Located, target: &start::Target<'_>) -> Result<StartReport, Failed> {
    // 1. A live Runtime refuses; nothing has been touched yet.
    refuse_live(&located.state_root)?;
    // Row 7a: an installer write refuses too, still before anything is touched.
    let writer = refuse_installer_busy(&located.root)?;
    start::preflight(target)?;
    // 2. The watchdog's run lock, so no tick starts beside this start.
    let directory = located.root.join("watchdog");
    fs::create_dir_all(&directory).map_err(|error| {
        io_failure(
            ActingctlCode::WatchdogDirectoryUnavailable,
            &directory,
            &error,
            IoOp::CreateDir,
        )
    })?;
    let run_lock = watchdog::hold_run_lock(&directory, RUN_LOCK_WAIT).map_err(refused)?;
    // A tick that held the lock may have started the Runtime meanwhile.
    refuse_live(&located.state_root)?;
    // 3-4. The watchdog's manual start and readiness. The log lies in the root, where the
    // watchdog looks for a FATAL (`contracts/runtime-watchdog.md`, "Starters and logs").
    let spawn_at_unix_ms = observe::unix_ms(SystemTime::now());
    let log = located
        .root_plain
        .join(format!("actingd-{spawn_at_unix_ms}.log"));
    let attempt = start::start_target(target, &log, spawn_at_unix_ms, false);
    drop(run_lock);
    drop(writer);
    let owner = attempt
        .result
        .map_err(|failure| Failed::new(failure).after(&start::last_log_line(&log)))?;
    let ready_ms = observe::unix_ms(SystemTime::now()).saturating_sub(spawn_at_unix_ms);
    // 5. One Status: the pauses the new owner restored.
    let started = format!(
        "the Runtime started: pid={} log={}",
        owner.pid,
        log.display()
    );
    let client = connect(&located.state_root).map_err(|failed| failed.after(&started))?;
    let status = client
        .status()
        .map_err(|error| Failed::runtime(&error).after(&started))?;
    let info = client.runtime_info();
    Ok(StartReport {
        pid: info.pid(),
        owner_epoch: info.owner_epoch(),
        method: attempt.method,
        config: located.config_plain.display().to_string(),
        log: log.display().to_string(),
        ready_ms,
        global_pause: status.scheduling_pause().cloned(),
        paused_instances: status
            .instances()
            .iter()
            .filter_map(|instance| {
                instance.pause().map(|pause| PausedInstance {
                    instance_alias: instance.instance_alias().to_owned(),
                    pause: pause.clone(),
                })
            })
            .collect(),
    })
}

fn restart_runtime(
    located: &Located,
    target: &start::Target<'_>,
    wait_idle: Duration,
) -> Result<Report, Failed> {
    // What the start needs is checked before anything stops: no installer write (row 7a),
    // held off until the restart ends, and the fixed entry and its paths.
    let _writer = refuse_installer_busy(&located.root)?;
    start::preflight(target)?;
    let client = connect(&located.state_root)?;
    // 1. Every instance idle: no lease, nothing queued; past `--wait-idle` nothing is stopped
    // (`start_wait_idle_expired` with `timeout_ms` and the busy `aliases`).
    let deadline = Instant::now() + wait_idle;
    loop {
        let status = client.status().map_err(|error| Failed::runtime(&error))?;
        let busy = status
            .instances()
            .iter()
            .filter(|instance| instance.lease_active() || instance.queued_request_count() != 0)
            .map(|instance| instance.instance_alias())
            .collect::<Vec<_>>();
        if busy.is_empty() {
            break;
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(Failed::new(
                Outcome::new(ActingctlCode::StartWaitIdleExpired)
                    .timeout_ms(wait_idle)
                    .aliases(busy),
            ));
        }
        thread::sleep(IDLE_POLL.min(deadline - now));
    }
    // 2-3. The shutdown, and its wait for the owner record's close and the process's exit.
    let receipt = client
        .request_shutdown()
        .map_err(|error| Failed::runtime(&error))?;
    let shutdown_target = client.runtime_info().shutdown_target();
    // Close the connection so the waiting CLI leaves nothing for the Runtime to drain.
    drop(client);
    let wait =
        shutdown_wait::wait_for_shutdown(&located.state_root, &shutdown_target, SHUTDOWN_WAIT)
            .map_err(Failed::from)?;
    // 4. The start.
    let start = start_runtime(located, target)
        .map_err(|failed| failed.after("the Runtime was shut down"))?;
    Ok(Report::Restart {
        shutdown: Box::new(ShutdownReport { receipt, wait }),
        start,
    })
}

fn connect(state_root: &Path) -> Result<RuntimeClient, Failed> {
    RuntimeClient::connect(RuntimeClientConfig::new(
        state_root,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(|error| Failed::runtime(&error))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(mode: Mode, arguments: &[&str]) -> Option<Arguments> {
        let arguments = arguments.iter().map(OsString::from).collect::<Vec<_>>();
        Arguments::parse(mode, &arguments).ok()
    }

    /// §2.1 and §4.11: each subcommand takes only its own flags, each once; the defaults are
    /// 180 s for readiness and 0 s (idle now) for `--wait-idle`.
    #[test]
    fn start_and_restart_take_only_their_flags_once() {
        let start = parse(Mode::Start, &["--root", "R"]).expect("start --root");
        assert_eq!(
            (
                start.ready_timeout,
                start.wait_idle,
                start.config,
                start.state_root
            ),
            (Duration::from_secs(180), Duration::ZERO, None, None)
        );
        let restart = parse(
            Mode::Restart,
            &[
                "--root",
                "R",
                "--config",
                "C",
                "--state-root",
                "S",
                "--wait-idle",
                "30",
                "--ready-timeout-s",
                "60",
            ],
        )
        .expect("restart with every flag");
        assert_eq!(
            restart,
            Arguments {
                root: PathBuf::from("R"),
                config: Some(PathBuf::from("C")),
                state_root: Some(PathBuf::from("S")),
                ready_timeout: Duration::from_secs(60),
                wait_idle: Duration::from_secs(30),
            }
        );
        let refused: [&[&str]; 6] = [
            &["--config", "C"],
            &["--root", "R", "--root", "R"],
            &["--root", "R", "--wait-idle", "5"],
            &["--root", "R", "--state-root"],
            &["--root", "R", "--instance", "a"],
            &["--root", "R", "--ready-timeout-s", "0"],
        ];
        for refused in refused {
            assert_eq!(parse(Mode::Start, refused), None, "start {refused:?}");
        }
        assert_eq!(
            parse(
                Mode::Restart,
                &["--root", "R", "--wait-idle", "5", "--wait-idle", "5"]
            ),
            None
        );
    }
}
