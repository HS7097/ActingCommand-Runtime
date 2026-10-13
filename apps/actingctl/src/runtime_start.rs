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
//!   does.
//!
//! The root is an A/B installation (the selection of `install\active.json`, as the watchdog
//! starts it) or, with `--config`, a verification root started with that config. Processes are
//! recognised by the owner lock, runtime-info and the accepted shutdown target only, never by
//! image name.

use crate::ActingctlError;
use crate::codes::ActingctlCode;
use crate::shutdown_wait;
use crate::watchdog::{self, Failure, decide, observe, start};
use actingcommand_contract::outcome::Code;
use actingcommand_contract::{
    EventActor, EventSource, InstancePauseState, OwnerEpoch, RuntimeReceipt, SchedulingPauseState,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use serde::Serialize;
use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Write};
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
/// How often `restart --wait-idle` reads Status again.
const IDLE_POLL: Duration = Duration::from_secs(1);
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
    let result = match Arguments::parse(mode, &arguments[1..]) {
        Some(parsed) => run(mode, &parsed),
        None => Err(Failed::new(ActingctlCode::ActingctlUsageInvalid, USAGE)),
    };
    let written = result.and_then(|report| {
        let failed = |error: &dyn fmt::Display| {
            Failed::new(ActingctlCode::ActingctlOutputFailed, error.to_string())
        };
        let mut writer = std::io::stdout().lock();
        serde_json::to_writer(&mut writer, &report).map_err(|error| failed(&error))?;
        writer.write_all(b"\n").map_err(|error| failed(&error))
    });
    Some(match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(failed) => {
            eprintln!("FATAL actingctl: {failed}");
            ExitCode::FAILURE
        }
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
    fn parse(mode: Mode, arguments: &[OsString]) -> Option<Self> {
        let mut root = None;
        let mut config = None;
        let mut state_root = None;
        let mut ready_timeout = None;
        let mut wait_idle = None;
        let mut index = 0;
        while index < arguments.len() {
            let flag = arguments[index].to_str()?;
            index += 1;
            let value = arguments.get(index)?;
            let slot_taken = match flag {
                "--root" => root.replace(PathBuf::from(value)).is_some(),
                "--config" => config.replace(PathBuf::from(value)).is_some(),
                "--state-root" => state_root.replace(PathBuf::from(value)).is_some(),
                "--ready-timeout-s" => ready_timeout
                    .replace(seconds(value, 1..=MAX_WAIT_SECONDS)?)
                    .is_some(),
                "--wait-idle" if mode == Mode::Restart => wait_idle
                    .replace(seconds(value, 0..=MAX_WAIT_SECONDS)?)
                    .is_some(),
                _ => return None,
            };
            if slot_taken {
                return None;
            }
            index += 1;
        }
        Some(Self {
            root: root?,
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

/// A failure of `start` or `restart`: its registered code and the developer detail of the
/// `FATAL actingctl:` line (A5 replaces the line with the outcome envelope).
#[derive(Debug)]
struct Failed {
    code: Code,
    detail: String,
}

impl Failed {
    fn new(code: impl Into<Code>, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
        }
    }

    fn runtime(error: &RuntimeClientError) -> Self {
        Self::new(error.code(), error.to_string())
    }

    fn after(self, done: &str) -> Self {
        Self {
            code: self.code,
            detail: format!("{}; {done}", self.detail),
        }
    }
}

impl From<Failure> for Failed {
    fn from(failure: Failure) -> Self {
        Self::new(failure.code, failure.detail)
    }
}

impl From<start::StartError> for Failed {
    fn from(error: start::StartError) -> Self {
        Self::new(error.code, error.detail)
    }
}

impl From<ActingctlError> for Failed {
    fn from(error: ActingctlError) -> Self {
        Self::new(error.code(), error.to_string())
    }
}

impl fmt::Display for Failed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
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

fn run(mode: Mode, arguments: &Arguments) -> Result<Report, Failed> {
    let located = Located::resolve(arguments)?;
    let target = start::Target {
        root_plain: &located.root_plain,
        config_plain: &located.config_plain,
        state_root: &located.state_root,
        ready_timeout: arguments.ready_timeout,
    };
    match mode {
        Mode::Start => start_runtime(&located, &target).map(Report::Start),
        Mode::Restart => restart_runtime(&located, &target, arguments.wait_idle),
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
            Failed::new(
                ActingctlCode::InstallRootUnresolved,
                format!("path={}: {error}", arguments.root.display()),
            )
        })?;
        let (installation, config_plain, configured) = match &arguments.config {
            // The selected generation, exactly as the watchdog starts it.
            None => {
                let installation = observe::Installation::resolve(&root).map_err(|failure| {
                    let hint = failure.code == ActingctlCode::WatchdogLayoutUnknown.code();
                    let failed = Failed::from(failure);
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
                let unreadable = |detail: String| {
                    Failed::new(
                        ActingctlCode::WatchdogConfigUnreadable,
                        format!("path={}: {detail}", config.display()),
                    )
                };
                let config =
                    fs::canonicalize(config).map_err(|error| unreadable(error.to_string()))?;
                let mut bytes = Vec::new();
                File::open(&config)
                    .and_then(|file| file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes))
                    .map_err(|error| unreadable(error.to_string()))?;
                if bytes.len() as u64 > MAX_CONFIG_BYTES {
                    return Err(unreadable(format!("larger than {MAX_CONFIG_BYTES} bytes")));
                }
                let state_root = configured_state_root(&bytes)
                    .map_err(|failed| failed.after(&format!("path={}", config.display())))?;
                (None, observe::plain_path(&config), state_root)
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

/// The config's absolute `state_root`, as the watchdog reads a selected config.
fn configured_state_root(bytes: &[u8]) -> Result<PathBuf, Failed> {
    let document: Value = serde_json::from_slice(bytes)
        .map_err(|error| Failed::new(ActingctlCode::WatchdogConfigUnreadable, error.to_string()))?;
    document
        .get("state_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| {
            Failed::new(
                ActingctlCode::WatchdogStateRootInvalid,
                "the config's state_root must be an absolute path",
            )
        })
}

/// `--state-root` only checks the config's state root: the same spelling, or the same
/// directory once both resolve.
fn same_state_root(requested: &Path, configured: PathBuf) -> Result<PathBuf, Failed> {
    if requested == configured {
        return Ok(configured);
    }
    match (fs::canonicalize(requested), fs::canonicalize(&configured)) {
        (Ok(requested), Ok(resolved)) if requested == resolved => Ok(configured),
        _ => Err(Failed::new(
            ActingctlCode::InstallProcessStateRootMismatch,
            format!(
                "path={}: the config's state_root is {}",
                requested.display(),
                configured.display()
            ),
        )),
    }
}

/// Step 1: a live Runtime on this state root (its owner lock held) refuses the start.
fn refuse_live(state_root: &Path) -> Result<(), Failed> {
    let (lock, _) = observe::owner_journal(state_root).map_err(Failed::from)?;
    if lock != decide::OwnerLock::Locked {
        return Ok(());
    }
    let owner = match observe::live_owner(state_root, None) {
        Ok(owner) => format!("pid={} owner_epoch={}", owner.pid, owner.owner_epoch),
        Err(error) => format!("the owner does not answer: {error}"),
    };
    Err(Failed::new(
        ActingctlCode::StartRuntimeAlreadyRunning,
        format!("path={} {owner}", state_root.display()),
    ))
}

/// Row 7a as the watchdog decides it (`watchdog/decide.rs`), through its own probe: while
/// acsetup holds `install\writer.lock` nothing starts, refused with the watchdog's
/// `watchdog_installer_busy`. A free lock comes back held shared, so acsetup cannot begin
/// before the guard is dropped, as during a tick's start (review L4).
fn refuse_installer_busy(root: &Path) -> Result<Option<File>, Failed> {
    match observe::writer_lock(root).map_err(Failed::from)? {
        observe::WriterProbe::Free(guard) => Ok(guard),
        observe::WriterProbe::Busy => Err(Failed::new(
            ActingctlCode::WatchdogInstallerBusy,
            "acsetup holds install\\writer.lock; start the Runtime after it finishes",
        )),
    }
}

fn start_runtime(located: &Located, target: &start::Target<'_>) -> Result<StartReport, Failed> {
    // 1. A live Runtime refuses; nothing has been touched yet.
    refuse_live(&located.state_root)?;
    // Row 7a: an installer write refuses too, still before anything is touched.
    let writer = refuse_installer_busy(&located.root)?;
    start::preflight(target).map_err(Failed::from)?;
    // 2. The watchdog's run lock, so no tick starts beside this start.
    let directory = located.root.join("watchdog");
    fs::create_dir_all(&directory).map_err(|error| {
        Failed::new(
            ActingctlCode::WatchdogDirectoryUnavailable,
            format!("path={}: {error}", directory.display()),
        )
    })?;
    let run_lock = watchdog::hold_run_lock(&directory, RUN_LOCK_WAIT).map_err(Failed::from)?;
    // A tick that held the lock may have started the Runtime meanwhile.
    refuse_live(&located.state_root)?;
    // 3-4. The watchdog's manual start and readiness. The log lies in the root, where the
    // watchdog looks for a FATAL (`contracts/runtime-watchdog.md`, "Starters and logs").
    let spawn_at_unix_ms = observe::unix_ms(SystemTime::now());
    let log = located
        .root_plain
        .join(format!("actingd-{spawn_at_unix_ms}.log"));
    let outcome = start::start_target(target, &log, spawn_at_unix_ms, false);
    drop(run_lock);
    drop(writer);
    let owner = outcome.result.map_err(Failed::from)?;
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
        method: outcome.method,
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
    start::preflight(target).map_err(Failed::from)?;
    let client = connect(&located.state_root)?;
    // 1. Every instance idle: no lease, nothing queued; past `--wait-idle` nothing is stopped.
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
                ActingctlCode::StartWaitIdleExpired,
                format!(
                    "timeout_ms={} aliases={}",
                    wait_idle.as_millis(),
                    busy.join(",")
                ),
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
        Arguments::parse(mode, &arguments)
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
