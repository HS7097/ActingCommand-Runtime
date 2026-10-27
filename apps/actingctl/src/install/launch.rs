// SPDX-License-Identifier: AGPL-3.0-only

//! The installer's start (Workflow #421 INST-2; `contracts/install-cli.md`, "Formal stop, start
//! and the hold"; #421 deploy design §4, §7): before every attempt the `memory` guard (read again
//! every 500 ms until it passes within `install.start.memory_wait_ms`), then no
//! live owner and no actingd under the root; `<root>\watchdog\run.lock` held, as `actingctl
//! start` holds it; the watchdog's manual start with the log `<root>\watchdog\actingd-<ms>.log`;
//! readiness every 500 ms until `install.start.ready_timeout_ms`; a progress sample (the log's
//! size, the starting Runtime's reads) every `install.start.sample_interval_ms`, journalled as it
//! is taken, and the stall rule's detection (`start_stalled`: no log growth for
//! `install.start.stall_after_ms`, or reads past `install.start.stall_read_bytes`), which
//! reports and never kills. A start that exits with an error terminal outcome starts the next
//! attempt up to `install.start.attempts`, a fatal one does not; one still not ready at its
//! deadline is `start_timeout` and is left running (the stall rule: ending a process is the
//! person's decision).

use super::guards::{self, GuardResult};
use super::host::Host;
use super::plan::Loaded;
use super::state::Paths;
use crate::codes::{ActingctlCode, InstallGuard, InstallLocation, InstallRefusal};
use crate::watchdog::decide::{CLOCK_SLACK_MS, LiveOwner, OwnerLock};
use crate::watchdog::observe;
use crate::watchdog::start::{StartedState, Target};
use actingcommand_contract::outcome::{Category, IoOp, Outcome};
use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The readiness probe's period (fixed by the contract).
const READY_POLL_MS: u64 = 500;
/// How much of a start log's end the terminal outcome is read from.
const LOG_TAIL_BYTES: u64 = 64 * 1024;

/// How the start ended.
#[derive(Debug)]
pub(crate) struct Launched {
    /// The answering owner when the start reached readiness.
    pub(crate) ready: Option<LiveOwner>,
    /// The start path's own code when it did not (`exited_during_startup`, `start_timeout`).
    pub(crate) failure: Option<Outcome>,
    /// Whether the last attempt's process was left running (a start that timed out).
    pub(crate) left_running: bool,
    /// The last attempt's log and when it was spawned.
    pub(crate) log: PathBuf,
    pub(crate) spawn_at_unix_ms: u64,
    pub(crate) ready_ms: Option<u64>,
    /// The attempts and the progress samples, as the start step's evidence records them.
    pub(crate) attempts: Vec<Value>,
    pub(crate) samples: Vec<Value>,
    pub(crate) stalled: bool,
}

impl Launched {
    /// The start step's evidence.
    pub(crate) fn evidence(&self, commit: &str) -> Value {
        json!({
            "commit": commit,
            "at_unix_ms": self.spawn_at_unix_ms,
            "log": super::files::plain(&self.log),
            "ready": self.ready.as_ref().map(|owner| json!({
                "pid": owner.pid,
                "owner_epoch": owner.owner_epoch,
                "started_at_unix_ms": owner.started_at_unix_ms,
            })),
            "ready_ms": self.ready_ms,
            "failure": self.failure.as_ref().map(crate::cli_line::warning),
            "left_running": self.left_running,
            "attempts": self.attempts,
            "samples": self.samples,
            "stalled": self.stalled,
        })
    }
}

/// The terminal outcome of a start log's end, as the watchdog reads it.
fn terminal_of(log: &Path) -> Option<Outcome> {
    let mut file = fs::File::open(log).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(LOG_TAIL_BYTES)))
        .ok()?;
    let mut bytes = Vec::new();
    file.take(LOG_TAIL_BYTES).read_to_end(&mut bytes).ok()?;
    observe::terminal_outcome(&String::from_utf8_lossy(&bytes), None)
}

/// The refusal of a start whose preconditions fail: a held owner lock, or an actingd running
/// under the root.
pub(crate) fn refuse_running(
    host: &dyn Host,
    paths: &Paths,
    state_root: &Path,
    operation: InstallLocation,
) -> Result<(), Outcome> {
    let unreadable = |cause: Outcome| {
        super::refusal(operation, InstallRefusal::ProcessesUnreadable).caused_by(cause)
    };
    let (lock, _) = observe::owner_journal(state_root).map_err(unreadable)?;
    if lock == OwnerLock::Locked {
        return Err(super::refusal(operation, InstallRefusal::OwnerLockHeld).path(state_root));
    }
    let images = host.processes_under(&paths.root).map_err(unreadable)?;
    if let Some(image) = images.iter().find(|image| {
        image
            .path
            .to_ascii_lowercase()
            .ends_with(&format!("\\{}", super::programs::DAEMON))
    }) {
        return Err(super::refusal(operation, InstallRefusal::RuntimeRunning)
            .path(&image.path)
            .pid(image.pid));
    }
    Ok(())
}

/// Starts the root's Runtime as the module documentation says. `progress` is called with the
/// start step's evidence so far after every attempt and every sample, so the journal holds it.
/// A refusal before a process was spawned (`guard_failed`, `owner_lock_held`,
/// `runtime_running`, a run lock it could not take) is the error; a start that spawned answers
/// [`Launched`] whether or not it reached readiness.
pub(crate) fn start(
    host: &dyn Host,
    paths: &Paths,
    loaded: &Loaded,
    operation: InstallLocation,
    commit: &str,
    acked: &[String],
    progress: &mut dyn FnMut(&Value) -> Result<(), Outcome>,
) -> Result<Launched, Outcome> {
    let settings = &loaded.settings;
    let runtime = host.runtime();
    let root_plain = observe::plain_path(&paths.root);
    let config_plain = observe::plain_path(&paths.config());
    let target = Target {
        root_plain: &root_plain,
        config_plain: &config_plain,
        state_root: &loaded.state_root,
        ready_timeout: Duration::from_millis(settings.start_ready_timeout_ms),
    };
    let directory = paths.root.join("watchdog");
    let mut launched = Launched {
        ready: None,
        failure: None,
        left_running: false,
        log: PathBuf::new(),
        spawn_at_unix_ms: 0,
        ready_ms: None,
        attempts: Vec::new(),
        samples: Vec::new(),
        stalled: false,
    };
    let memory_acked = acked.iter().any(|ack| ack == InstallGuard::Memory.as_str());
    for attempt in 1..=settings.start_attempts {
        // The memory guard before every attempt, read again until it passes within
        // `install.start.memory_wait_ms` (review S2 M2: the pages of a Runtime that just stopped
        // are released over time), never refused at its first sample.
        let waited_from = host.now_unix_ms();
        let mut memory = guards::memory(host, settings);
        while !memory_acked
            && memory.result != GuardResult::Pass
            && host.now_unix_ms().saturating_sub(waited_from) < settings.start_memory_wait_ms
        {
            host.pause_ms(READY_POLL_MS);
            memory = guards::memory(host, settings);
        }
        if memory.result != GuardResult::Pass {
            memory.values["waited_ms"] = json!(host.now_unix_ms().saturating_sub(waited_from));
        }
        let passes = memory.result == GuardResult::Pass || memory_acked;
        if !passes {
            if attempt == 1 {
                return Err(super::refusal(operation, InstallRefusal::GuardFailed)
                    .install_guards([InstallGuard::Memory]));
            }
            launched
                .attempts
                .push(json!({"attempt": attempt, "memory": memory}));
            launched.failure = Some(
                super::refusal(operation, InstallRefusal::GuardFailed)
                    .install_guards([InstallGuard::Memory]),
            );
            progress(&launched.evidence(commit))?;
            return Ok(launched);
        }
        refuse_running(host, paths, &loaded.state_root, operation)?;
        super::files::create_dir_all(&directory)?;
        let run_lock = crate::watchdog::hold_run_lock(
            &directory,
            Duration::from_millis(settings.lock_wait_ms),
        )
        .map_err(|cause| {
            super::refusal(operation, InstallRefusal::WriterLockFailed).caused_by(cause)
        })?;
        let spawn_at_unix_ms = host.now_unix_ms();
        let log = directory.join(format!("actingd-{spawn_at_unix_ms}.log"));
        launched.log.clone_from(&log);
        // The start's own time is its first spawn: from then on its build is the one the root
        // starts (the open-time attribution of the operation journals).
        if launched.spawn_at_unix_ms == 0 {
            launched.spawn_at_unix_ms = spawn_at_unix_ms;
        }
        let mut started = match runtime.spawn(&target, &log) {
            Ok(started) => started,
            Err(failure) => {
                drop(run_lock);
                launched.attempts.push(json!({
                    "attempt": attempt,
                    "memory": memory,
                    "spawn": crate::cli_line::warning(&failure),
                }));
                launched.failure = Some(failure);
                progress(&launched.evidence(commit))?;
                return Ok(launched);
            }
        };
        let deadline = spawn_at_unix_ms.saturating_add(settings.start_ready_timeout_ms);
        let mut last_sample = spawn_at_unix_ms;
        let mut last_size = 0_u64;
        let mut grew_at = spawn_at_unix_ms;
        let ended = loop {
            if let Some(owner) = runtime.answering_owner(&loaded.state_root)
                && owner.started_at_unix_ms >= spawn_at_unix_ms.saturating_sub(CLOCK_SLACK_MS)
            {
                break Ok(owner);
            }
            let exited = || Outcome::new(ActingctlCode::ExitedDuringStartup).log(&log);
            let state = match started.state() {
                Ok(state) => state,
                Err(failure) => break Err(exited().caused_by(failure)),
            };
            match state {
                StartedState::Running => {}
                StartedState::Exited(Some(code)) => break Err(exited().exit_code(code)),
                StartedState::Exited(None) => break Err(exited()),
                StartedState::Unreadable(error) => {
                    break Err(exited().caused_by(Outcome::from_io_error(&error, IoOp::Wait)));
                }
                StartedState::Gone(pid) => break Err(exited().pid(pid)),
            }
            let now = host.now_unix_ms();
            if now.saturating_sub(last_sample) >= settings.start_sample_interval_ms {
                last_sample = now;
                let size = fs::metadata(&log).map_or(0, |metadata| metadata.len());
                if size > last_size {
                    last_size = size;
                    grew_at = now;
                }
                let read_bytes = host.runtime_read_bytes(&paths.root);
                let stalled = now.saturating_sub(grew_at) >= settings.start_stall_after_ms
                    || read_bytes.is_some_and(|read| read > settings.start_stall_read_bytes);
                launched.stalled |= stalled;
                launched.samples.push(json!({
                    "at_unix_ms": now,
                    "log_bytes": size,
                    "read_bytes": read_bytes,
                    "stalled": stalled,
                }));
                progress(&launched.evidence(commit))?;
            }
            if now >= deadline {
                break Err(Outcome::new(ActingctlCode::StartTimeout)
                    .log(&log)
                    .timeout_ms(Duration::from_millis(settings.start_ready_timeout_ms)));
            }
            host.pause_ms(READY_POLL_MS);
        };
        drop(run_lock);
        match ended {
            Ok(owner) => {
                launched.ready_ms = Some(host.now_unix_ms().saturating_sub(spawn_at_unix_ms));
                launched
                    .attempts
                    .push(json!({"attempt": attempt, "memory": memory, "ready": true}));
                launched.ready = Some(owner);
                launched.failure = None;
                progress(&launched.evidence(commit))?;
                return Ok(launched);
            }
            Err(failure) => {
                let timed_out = failure.is(ActingctlCode::StartTimeout);
                let terminal = if timed_out { None } else { terminal_of(&log) };
                let fatal = terminal
                    .as_ref()
                    .is_some_and(|terminal| terminal.category() == Category::Fatal);
                launched.attempts.push(json!({
                    "attempt": attempt,
                    "memory": memory,
                    "ended": crate::cli_line::warning(&failure),
                    "terminal": terminal.as_ref().map(|terminal| terminal.code().as_str().to_owned()),
                }));
                launched.left_running = timed_out;
                launched.failure = Some(failure);
                progress(&launched.evidence(commit))?;
                // A start left running at its deadline is never started again beside it; a fatal
                // terminal outcome is not tried again.
                if timed_out || fatal {
                    return Ok(launched);
                }
            }
        }
    }
    Ok(launched)
}
