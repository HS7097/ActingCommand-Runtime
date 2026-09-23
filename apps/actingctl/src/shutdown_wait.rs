// SPDX-License-Identifier: AGPL-3.0-only

//! `request-shutdown --wait`: a read-only observation that the accepted owner epoch closed its
//! owner journal record and that its process exited. It writes nothing and takes no file lock.

use crate::ActingctlError;
use actingcommand_contract::{OWNER_JOURNAL_LIMIT, RuntimeShutdownTarget};
use serde_json::{Value, json};
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// The journal the Runtime owner guard appends to (runtime-host `OWNER_FILE_NAME`).
const OWNER_JOURNAL_FILE: &str = "owner.lock";

#[derive(Default)]
struct Observation {
    journal_locked: bool,
    active: Option<bool>,
    revision: Option<u64>,
    closed_at_unix_ms: Option<u64>,
    pid_alive: Option<bool>,
    pid_probe_error: Option<String>,
}

pub(crate) fn wait_for_shutdown(
    state_root: &Path,
    target: &RuntimeShutdownTarget,
    wait: Duration,
) -> Result<Value, ActingctlError> {
    let started = Instant::now();
    let deadline = started + wait;
    let journal = state_root.join(OWNER_JOURNAL_FILE);
    let mut observed = Observation::default();
    loop {
        observe_journal(&journal, target, &mut observed)?;
        let timed_out = Instant::now() >= deadline;
        // The process is probed once the epoch closed, and once more for the timeout detail.
        if observed.active == Some(false) || timed_out {
            match pid_alive(target.pid) {
                Ok(alive) => (observed.pid_alive, observed.pid_probe_error) = (Some(alive), None),
                Err(error) => (observed.pid_alive, observed.pid_probe_error) = (None, Some(error)),
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
                "elapsed_ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            }));
        }
        if timed_out {
            let detail = json!({
                "owner_epoch": target.owner_epoch,
                "pid": target.pid,
                "journal_locked": observed.journal_locked,
                "active": observed.active,
                "revision": observed.revision,
                "pid_alive": observed.pid_alive,
                "pid_probe_error": observed.pid_probe_error,
            });
            return Err(failure("shutdown_wait_timeout", detail));
        }
        std::thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

/// Reads the journal the way the Ledger's read-only writer-metadata reader reads `writer.lock`:
/// a plain shared-read open, a non-blocking read, and a lock held by the live owner (os error 33
/// on Windows, WouldBlock elsewhere) is an observation retried on the next tick, not a failure.
fn observe_journal(
    path: &Path,
    target: &RuntimeShutdownTarget,
    observed: &mut Observation,
) -> Result<(), ActingctlError> {
    let unreadable = |error: &dyn std::fmt::Display| {
        let detail = json!({ "path": path.display().to_string(), "error": error.to_string() });
        failure("shutdown_wait_journal_unreadable", detail)
    };
    let mut bytes = Vec::new();
    match File::open(path)
        .and_then(|file| file.take(OWNER_JOURNAL_LIMIT + 1).read_to_end(&mut bytes))
    {
        Err(error) if error.kind() == ErrorKind::WouldBlock || error.raw_os_error() == Some(33) => {
            observed.journal_locked = true;
            return Ok(());
        }
        Err(error) => return Err(unreadable(&error)),
        Ok(_) if bytes.len() as u64 > OWNER_JOURNAL_LIMIT => {
            return Err(unreadable(&"owner journal exceeds its size limit"));
        }
        Ok(_) => observed.journal_locked = false,
    }
    // Only complete records count; a torn tail of an append in progress is read again next tick.
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |end| end + 1);
    let text = std::str::from_utf8(&bytes[..complete]).map_err(|error| unreadable(&error))?;
    let epoch = serde_json::to_value(target.owner_epoch).map_err(|error| unreadable(&error))?;
    let mut last = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let record: Value = serde_json::from_str(line).map_err(|error| unreadable(&error))?;
        if record["owner_epoch"] == epoch && record["pid"] == target.pid {
            last = Some(record);
        }
    }
    let Some(record) = last else {
        return Ok(());
    };
    let active = record["active"].as_bool();
    let closed_at_unix_ms = record["closed_at_unix_ms"].as_u64();
    let (Some(active), Some(revision)) = (active, record["revision"].as_u64()) else {
        return Err(unreadable(&"owner record lacks active or revision"));
    };
    if !active && closed_at_unix_ms.is_none() {
        return Err(unreadable(&"closed owner record lacks closed_at_unix_ms"));
    }
    (observed.active, observed.revision) = (Some(active), Some(revision));
    observed.closed_at_unix_ms = closed_at_unix_ms;
    Ok(())
}

/// No liveness helper exists in actingctl's dependencies and unsafe code is forbidden here, so
/// this asks the platform process lister: `tasklist` on Windows (exit 0 either way; a match is a
/// CSV row whose second field is the pid), POSIX `ps` elsewhere (no match: non-zero, no output).
fn pid_alive(pid: u32) -> Result<bool, String> {
    let pid_text = pid.to_string();
    #[cfg(windows)]
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output();
    #[cfg(not(windows))]
    let output = Command::new("ps")
        .args(["-p", &pid_text, "-o", "pid="])
        .output();
    let output = output.map_err(|error| format!("process lister did not run: {error}"))?;
    let listed = String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        line.split(',')
            .take(2)
            .any(|field| field.trim().trim_matches('"') == pid_text)
    });
    if listed {
        return Ok(true);
    }
    if output.status.success() || output.stderr.is_empty() {
        return Ok(false);
    }
    Err(format!(
        "process lister failed: status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn failure(code: &'static str, detail: Value) -> ActingctlError {
    ActingctlError::ShutdownWait {
        code,
        detail: detail.to_string(),
    }
}
