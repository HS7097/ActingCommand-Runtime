// SPDX-License-Identifier: AGPL-3.0-only

//! `request-shutdown --wait`: a read-only observation that the accepted owner epoch closed its
//! owner journal record and that its process exited. It writes nothing and takes no file lock.
//!
//! Workflow #378 A5 (model §7.3): a wait that fails is an outcome, `shutdown_wait_timeout` or
//! `shutdown_wait_journal_unreadable`, with what it observed as values and the OS and decoder
//! failures as `foreign_*` links; it carries no prose.

use crate::ActingctlError;
use crate::codes::ActingctlCode;
use crate::process_probe::pid_alive;
use actingcommand_contract::outcome::{IoOp, Outcome};
use actingcommand_contract::{OWNER_JOURNAL_LIMIT, RuntimeShutdownTarget};
use serde_json::{Value, json};
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// The journal the Runtime owner guard appends to (runtime-host `OWNER_FILE_NAME`).
const OWNER_JOURNAL_FILE: &str = "owner.lock";

#[derive(Default)]
struct Observation {
    active: Option<bool>,
    closed_at_unix_ms: Option<u64>,
    pid_alive: Option<bool>,
    /// The failure of the last liveness probe, which left `pid_alive` unknown.
    pid_probe_failure: Option<Outcome>,
}

/// Workflow #382 A-1 (test plan v2.2 §3.7, G11): what a wait reads besides the owner journal,
/// the time since it started and the liveness of the owner's process, and how it waits between
/// two polls. The process waits with `SystemPoller`; a test injects its own clock and probe.
pub(crate) trait Poller {
    /// The time since the wait started.
    fn elapsed(&self) -> Duration;
    /// Waits `duration` before the next poll.
    fn wait(&mut self, duration: Duration);
    /// Whether the process `pid` is alive; the failure of a probe that cannot tell.
    fn pid_alive(&mut self, pid: u32) -> Result<bool, Outcome>;
}

/// The process's poller: the monotonic clock from the wait's start, a sleep and the platform
/// process lister.
struct SystemPoller {
    started: Instant,
}

impl Poller for SystemPoller {
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn wait(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn pid_alive(&mut self, pid: u32) -> Result<bool, Outcome> {
        pid_alive(pid)
    }
}

pub(crate) fn wait_for_shutdown(
    state_root: &Path,
    target: &RuntimeShutdownTarget,
    wait: Duration,
) -> Result<Value, ActingctlError> {
    wait_with(
        state_root,
        target,
        wait,
        &mut SystemPoller {
            started: Instant::now(),
        },
    )
}

/// `wait_for_shutdown` with its clock and liveness probe taken from `poller`.
pub(crate) fn wait_with(
    state_root: &Path,
    target: &RuntimeShutdownTarget,
    wait: Duration,
    poller: &mut impl Poller,
) -> Result<Value, ActingctlError> {
    let journal = state_root.join(OWNER_JOURNAL_FILE);
    let epoch = serde_json::to_value(target.owner_epoch)
        .map_err(|error| unreadable(unreadable_journal(&journal).caused_by(decode(&error))))?;
    let mut observed = Observation::default();
    loop {
        observe_journal(&journal, &epoch, target.pid, &mut observed)?;
        let timed_out = poller.elapsed() >= wait;
        // The process is probed once the epoch closed, and once more for the timeout's values.
        if observed.active == Some(false) || timed_out {
            match poller.pid_alive(target.pid) {
                Ok(alive) => {
                    (observed.pid_alive, observed.pid_probe_failure) = (Some(alive), None);
                }
                Err(failure) => {
                    (observed.pid_alive, observed.pid_probe_failure) = (None, Some(failure));
                }
            }
        }
        if let (Some(false), Some(false), Some(closed_at_unix_ms)) = (
            observed.active,
            observed.pid_alive,
            observed.closed_at_unix_ms,
        ) {
            return Ok(json!({
                "state": "completed",
                "closed_at_unix_ms": closed_at_unix_ms,
                "elapsed_ms": u64::try_from(poller.elapsed().as_millis()).unwrap_or(u64::MAX),
            }));
        }
        if timed_out {
            return Err(ActingctlError::ShutdownWait(timeout(
                target, wait, &epoch, observed,
            )));
        }
        poller.wait(POLL_INTERVAL.min(wait.saturating_sub(poller.elapsed())));
    }
}

/// `shutdown_wait_timeout`: the accepted owner (`owner_epoch`, `pid`) did not close and exit
/// within `timeout_ms`. `owner_closed` and `process_alive` are what the wait last saw (absent
/// when it saw no record of the epoch, or the probe failed); a failed probe is the cause.
fn timeout(
    target: &RuntimeShutdownTarget,
    wait: Duration,
    epoch: &Value,
    observed: Observation,
) -> Outcome {
    let mut outcome = Outcome::new(ActingctlCode::ShutdownWaitTimeout)
        .pid(target.pid)
        .timeout_ms(wait);
    if let Some(epoch) = epoch.as_str() {
        outcome = outcome.owner_epoch(epoch);
    }
    if let Some(active) = observed.active {
        outcome = outcome.owner_closed(!active);
    }
    if let Some(alive) = observed.pid_alive {
        outcome = outcome.process_alive(alive);
    }
    match observed.pid_probe_failure {
        Some(failure) => outcome.caused_by(failure),
        None => outcome,
    }
}

/// Reads the journal the way the Ledger's read-only writer-metadata reader reads `writer.lock`:
/// a plain shared-read open, a non-blocking read, and a lock held by the live owner (os error 33
/// on Windows, WouldBlock elsewhere) is an observation retried on the next tick, not a failure.
fn observe_journal(
    path: &Path,
    epoch: &Value,
    pid: u32,
    observed: &mut Observation,
) -> Result<(), ActingctlError> {
    let mut bytes = Vec::new();
    let read = File::open(path)
        .map_err(|error| (error, IoOp::Open))
        .and_then(|file| {
            file.take(OWNER_JOURNAL_LIMIT + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| (error, IoOp::Read))
        });
    match read {
        Err((error, _))
            if error.kind() == ErrorKind::WouldBlock || error.raw_os_error() == Some(33) =>
        {
            return Ok(());
        }
        Err((error, op)) => {
            return Err(unreadable(
                unreadable_journal(path).caused_by(Outcome::from_io_error(&error, op)),
            ));
        }
        Ok(_) if bytes.len() as u64 > OWNER_JOURNAL_LIMIT => {
            return Err(unreadable(
                unreadable_journal(path).limit_bytes(OWNER_JOURNAL_LIMIT),
            ));
        }
        Ok(_) => {}
    }
    // Only complete records count; a torn tail of an append in progress is read again next tick.
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |end| end + 1);
    let text = std::str::from_utf8(&bytes[..complete])
        .map_err(|error| unreadable(unreadable_journal(path).caused_by(decode(&error))))?;
    let mut last = None;
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(line).map_err(|error| {
            unreadable(
                unreadable_journal(path)
                    .line(index + 1)
                    .caused_by(decode(&error)),
            )
        })?;
        if record["owner_epoch"] == *epoch && record["pid"] == pid {
            last = Some((index + 1, record));
        }
    }
    let Some((line, record)) = last else {
        return Ok(());
    };
    let active = record["active"].as_bool();
    let closed_at_unix_ms = record["closed_at_unix_ms"].as_u64();
    // The owner guard's record has `active` and `revision`, and a closed one `closed_at_unix_ms`.
    let Some(active) = active.filter(|_| record["revision"].as_u64().is_some()) else {
        return Err(unreadable(unreadable_journal(path).line(line)));
    };
    if !active && closed_at_unix_ms.is_none() {
        return Err(unreadable(unreadable_journal(path).line(line)));
    }
    (observed.active, observed.closed_at_unix_ms) = (Some(active), closed_at_unix_ms);
    Ok(())
}

/// `shutdown_wait_journal_unreadable` for the owner journal at `path`.
fn unreadable_journal(path: &Path) -> Outcome {
    Outcome::new(ActingctlCode::ShutdownWaitJournalUnreadable).path(path)
}

fn unreadable(outcome: Outcome) -> ActingctlError {
    ActingctlError::ShutdownWait(outcome)
}

/// The decoder's refusal of the journal's text (`foreign_data_error`, `raw_source` json).
fn decode(error: &impl std::fmt::Display) -> Outcome {
    crate::foreign_data_error(error)
}
