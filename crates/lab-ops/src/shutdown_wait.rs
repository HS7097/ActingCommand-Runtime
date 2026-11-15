// SPDX-License-Identifier: AGPL-3.0-only

//! `request-shutdown --wait`: a read-only observation that the accepted owner epoch closed its
//! owner journal record and that its process exited. It writes nothing and takes no file lock.
//!
//! Workflow #378 A5 (model §7.3): a wait that fails is an outcome, `shutdown_wait_timeout` or
//! `shutdown_wait_journal_unreadable`, with what it observed as values and the OS and decoder
//! failures as `foreign_*` links; it carries no prose.
//!
//! Workflow #434 LAB-1b: moved from actingctl, where `request-shutdown`, `install-transition`,
//! `start`, `restart` and `install` wait with it; `shutdown --wait` ([`crate::shutdown`]) is
//! served here. A failed wait is its outcome itself, which actingctl wraps as before.

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
pub trait Poller {
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

pub fn wait_for_shutdown(
    state_root: &Path,
    target: &RuntimeShutdownTarget,
    wait: Duration,
) -> Result<Value, Outcome> {
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
pub fn wait_with(
    state_root: &Path,
    target: &RuntimeShutdownTarget,
    wait: Duration,
    poller: &mut impl Poller,
) -> Result<Value, Outcome> {
    let journal = state_root.join(OWNER_JOURNAL_FILE);
    let epoch = serde_json::to_value(target.owner_epoch)
        .map_err(|error| unreadable_journal(&journal).caused_by(decode(&error)))?;
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
            return Err(timeout(target, wait, &epoch, observed));
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
) -> Result<(), Outcome> {
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
            return Err(unreadable_journal(path).caused_by(Outcome::from_io_error(&error, op)));
        }
        Ok(_) if bytes.len() as u64 > OWNER_JOURNAL_LIMIT => {
            return Err(unreadable_journal(path).limit_bytes(OWNER_JOURNAL_LIMIT));
        }
        Ok(_) => {}
    }
    // Only complete records count; a torn tail of an append in progress is read again next tick.
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |end| end + 1);
    let text = std::str::from_utf8(&bytes[..complete])
        .map_err(|error| unreadable_journal(path).caused_by(decode(&error)))?;
    let mut last = None;
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(line).map_err(|error| {
            unreadable_journal(path)
                .line(index + 1)
                .caused_by(decode(&error))
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
        return Err(unreadable_journal(path).line(line));
    };
    if !active && closed_at_unix_ms.is_none() {
        return Err(unreadable_journal(path).line(line));
    }
    (observed.active, observed.closed_at_unix_ms) = (Some(active), closed_at_unix_ms);
    Ok(())
}

/// `shutdown_wait_journal_unreadable` for the owner journal at `path`.
fn unreadable_journal(path: &Path) -> Outcome {
    Outcome::new(ActingctlCode::ShutdownWaitJournalUnreadable).path(path)
}

/// The decoder's refusal of the journal's text (`foreign_data_error`, `raw_source` json).
fn decode(error: &impl std::fmt::Display) -> Outcome {
    crate::foreign_data_error(error)
}

// Workflow #382 A-1 G11 (test plan v2.2 §3.7; Workflow 191-batch4-models.md s1), with an injected
// poller and clock: every 250 ms the wait reads the owner journal; it is done when the target
// owner epoch's last record is inactive and the target process is not alive, giving `{state,
// closed_at_unix_ms, elapsed_ms}`; past its time it is `shutdown_wait_timeout` with the last state
// it saw; another epoch's close never counts. Workflow #434 LAB-7: moved here from actingctl's
// `gate_382_a1`, which left with actingctl; the Lab's `shutdown --wait` runs this wait.
#[cfg(test)]
mod g11_tests {
    use super::{Poller, wait_with};
    use actingcommand_contract::outcome::Outcome;
    use actingcommand_contract::{
        IdentifierIssuer, OWNER_JOURNAL_SCHEMA, OwnerEpoch, RuntimeShutdownTarget,
    };
    use actingcommand_ledger::owner_journal::RuntimeOwnerRecord;
    use serde_json::{Value, json};
    use std::collections::VecDeque;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::Duration;
    use tempfile::TempDir;

    /// The s1 model's poll interval.
    const POLL: Duration = Duration::from_millis(250);

    /// One owner guard record of the journal (`contracts/owner-journal.md`): active, or closed at
    /// `closed_at_unix_ms`.
    fn record(
        revision: u64,
        owner_epoch: OwnerEpoch,
        pid: u32,
        closed_at_unix_ms: Option<u64>,
    ) -> RuntimeOwnerRecord {
        RuntimeOwnerRecord {
            schema_version: OWNER_JOURNAL_SCHEMA.to_owned(),
            revision,
            owner_epoch,
            pid,
            started_at_unix_ms: 1_800_000_000_000,
            active: closed_at_unix_ms.is_none(),
            active_instances: Vec::new(),
            closed_at_unix_ms,
            resource_disposition: None,
            closed_by: None,
        }
    }

    /// Appends `records` to the owner journal as the owner guard does, one JSON line each.
    fn append(journal: &Path, records: &[RuntimeOwnerRecord]) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(journal)
            .expect("open the owner journal");
        for record in records {
            let line = serde_json::to_string(record).expect("record JSON");
            writeln!(file, "{line}").expect("append the record");
        }
    }

    /// The injected poller and clock: virtual time moves only by the waits the loop asks for; at
    /// each wait the journal gets the records the Runtime appended meanwhile; the liveness answers
    /// come in order and the last one repeats.
    struct ScriptedPoller {
        journal: PathBuf,
        elapsed: Duration,
        waits: Vec<Duration>,
        appended: VecDeque<Vec<RuntimeOwnerRecord>>,
        alive: VecDeque<bool>,
        probed: Vec<u32>,
    }

    impl ScriptedPoller {
        fn new(journal: &Path, appended: Vec<Vec<RuntimeOwnerRecord>>, alive: &[bool]) -> Self {
            Self {
                journal: journal.to_path_buf(),
                elapsed: Duration::ZERO,
                waits: Vec::new(),
                appended: appended.into(),
                alive: alive.iter().copied().collect(),
                probed: Vec::new(),
            }
        }
    }

    impl Poller for ScriptedPoller {
        fn elapsed(&self) -> Duration {
            self.elapsed
        }

        fn wait(&mut self, duration: Duration) {
            self.waits.push(duration);
            self.elapsed += duration;
            if let Some(records) = self.appended.pop_front() {
                append(&self.journal, &records);
            }
        }

        fn pid_alive(&mut self, pid: u32) -> Result<bool, Outcome> {
            self.probed.push(pid);
            let alive = if self.alive.len() > 1 {
                self.alive.pop_front()
            } else {
                self.alive.front().copied()
            };
            Ok(alive.expect("a scripted liveness answer"))
        }
    }

    /// The accepted shutdown's target and the epoch of another owner of the same root.
    fn owners() -> (RuntimeShutdownTarget, OwnerEpoch) {
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let target = RuntimeShutdownTarget {
            owner_epoch: *issuer.mint_owner_epoch().expect("owner epoch").transport(),
            pid: 4_242,
            started_at_unix_ms: 1_800_000_000_000,
        };
        let other = *issuer.mint_owner_epoch().expect("owner epoch").transport();
        (target, other)
    }

    /// The target epoch's record closes at the second wait and its process is still alive at the
    /// first probe after it; the wait completes at the next poll with the record's close time. An
    /// earlier owner's close, already in the journal, never completes it.
    #[test]
    fn gate_the_wait_completes_when_the_target_epoch_closed_and_its_process_exited() {
        let root = TempDir::new().expect("tempdir");
        let journal = root.path().join("owner.lock");
        let (target, earlier) = owners();
        append(
            &journal,
            &[
                record(1, earlier, 1_111, None),
                record(2, earlier, 1_111, Some(1_800_000_000_500)),
                record(3, target.owner_epoch, target.pid, None),
            ],
        );
        let mut poller = ScriptedPoller::new(
            &journal,
            vec![
                Vec::new(),
                vec![record(
                    4,
                    target.owner_epoch,
                    target.pid,
                    Some(1_800_000_009_000),
                )],
            ],
            &[true, false],
        );

        let done = wait_with(root.path(), &target, Duration::from_secs(60), &mut poller)
            .expect("the wait completes");

        assert_eq!(
            done,
            json!({
                "state": "completed",
                "closed_at_unix_ms": 1_800_000_009_000_u64,
                "elapsed_ms": 750_u64,
            })
        );
        // The process is probed only once the target's record closed, until it is gone.
        assert_eq!(poller.probed, [target.pid, target.pid]);
        assert_eq!(poller.waits, [POLL; 3]);
    }

    /// The target owner stops without closing its record, and a later owner whose process got
    /// the same pid starts and closes meanwhile. That close is another epoch's: the wait runs to
    /// its end and times out with what it last saw of the target (record open, process gone).
    #[test]
    fn gate_another_epochs_close_does_not_end_the_wait_which_times_out_with_the_last_state() {
        let root = TempDir::new().expect("tempdir");
        let journal = root.path().join("owner.lock");
        let (target, later) = owners();
        append(&journal, &[record(1, target.owner_epoch, target.pid, None)]);
        let mut poller = ScriptedPoller::new(
            &journal,
            vec![
                Vec::new(),
                vec![
                    record(2, later, target.pid, None),
                    record(3, later, target.pid, Some(1_800_000_009_000)),
                ],
            ],
            &[false],
        );
        let wait = Duration::from_secs(1);

        let outcome =
            wait_with(root.path(), &target, wait, &mut poller).expect_err("the wait times out");

        assert_eq!(outcome.code().as_str(), "shutdown_wait_timeout");
        assert_eq!(
            outcome.value("owner_closed"),
            Some(&json!(false)),
            "{outcome}"
        );
        assert_eq!(
            outcome.value("process_alive"),
            Some(&json!(false)),
            "{outcome}"
        );
        assert_eq!(outcome.value("pid"), Some(&json!(target.pid)), "{outcome}");
        let epoch: Value = serde_json::to_value(target.owner_epoch).expect("epoch JSON");
        assert_eq!(outcome.value("owner_epoch"), Some(&epoch), "{outcome}");
        assert_eq!(
            outcome.value("timeout_ms"),
            Some(&json!(1_000_u64)),
            "{outcome}"
        );
        // It polled every 250 ms to its end and probed the process once, for the timeout's state.
        assert_eq!(poller.waits, [POLL; 4]);
        assert_eq!(poller.elapsed, wait);
        assert_eq!(poller.probed, [target.pid]);
    }

    /// The target epoch's record closes, but its process outlives the wait: not done; the timeout
    /// carries the last state (record closed, process alive).
    #[test]
    fn gate_a_closed_epoch_whose_process_outlives_the_wait_times_out_with_the_last_state() {
        let root = TempDir::new().expect("tempdir");
        let journal = root.path().join("owner.lock");
        let (target, _) = owners();
        append(&journal, &[record(1, target.owner_epoch, target.pid, None)]);
        let mut poller = ScriptedPoller::new(
            &journal,
            vec![vec![record(
                2,
                target.owner_epoch,
                target.pid,
                Some(1_800_000_009_000),
            )]],
            &[true],
        );

        let outcome = wait_with(
            root.path(),
            &target,
            Duration::from_millis(500),
            &mut poller,
        )
        .expect_err("the wait times out");

        assert_eq!(outcome.code().as_str(), "shutdown_wait_timeout");
        assert_eq!(
            outcome.value("owner_closed"),
            Some(&json!(true)),
            "{outcome}"
        );
        assert_eq!(
            outcome.value("process_alive"),
            Some(&json!(true)),
            "{outcome}"
        );
        assert_eq!(poller.waits, [POLL; 2]);
    }
}
