// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #383 AO-1 (`contracts/ledger-observer.md` §9, §10, §14 T8): a failed ledger write is
//! read back and tried again at once; when the attempts are used up, or the store reports an
//! integrity or format failure, every record not yet committed goes into the dump file and the
//! Runtime stops fatally; the next start imports the dump before any new record.
//!
//! The dump file (register L12, `dump_version` 1) is JSON Lines: a header, one line per record
//! (`{"record": <object>, "sha256": <hex of the record member's bytes>}`) and, for a sealed dump,
//! a trailer. Its records are sanitized drafts only; it is private to the state root.

use super::storage::{DurableStorage, EventStore};
use super::{
    GlobalLedgerError, GlobalLedgerResult, LedgerCapturePolicy, Sha256SecretFingerprinter,
};
use crate::PersistedEvent;
use crate::codes::{LedgerCode, LedgerLocation};
use crate::fact::DumpedDraft;
use actingcommand_contract::outcome::{CauseRelation, CodeStr, IoKind, IoOp, Outcome};
use actingcommand_contract::{
    AuditInput, DiagnosticCode, DiagnosticDetailDraft, EffectDisposition, EventActor, EventDraft,
    EventId, EventLinksDraft, EventOrigin, EventPayloadDraft, EventSeverity, EventSource,
    IdentifierIssuer, LedgerGapKind, LedgerPayloadDraft, OriginModule, OwnerEpoch,
    RuntimePayloadDraft, SanitizedEventDraft,
};
use actingcommand_runtime_database::CommitSync;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The dump format this build writes and reads (register L12).
pub const LEDGER_DUMP_VERSION: u64 = 1;
/// The dump file in the state root, beside `runtime-state.sqlite` (§9.3).
pub const LEDGER_DUMP_FILE_NAME: &str = "ledger-dump.jsonl";

/// What `ledger_observer.*` sets for the write path (§9.1, §15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerWritePolicy {
    write_attempts: u32,
    write_retry_backoff_ms: Vec<u64>,
    flush_attempt_stall_ms: u64,
    health_probe_interval_ms: u64,
    dump_max_bytes: u64,
}

impl LedgerWritePolicy {
    /// `write_retry_backoff_ms` holds the wait before each attempt, one entry per attempt (the
    /// first attempt included); every other value is at least 1.
    pub fn new(
        write_attempts: u32,
        write_retry_backoff_ms: Vec<u64>,
        flush_attempt_stall_ms: u64,
        health_probe_interval_ms: u64,
        dump_max_bytes: u64,
    ) -> GlobalLedgerResult<Self> {
        if write_attempts == 0
            || usize::try_from(write_attempts).ok() != Some(write_retry_backoff_ms.len())
            || flush_attempt_stall_ms == 0
            || health_probe_interval_ms == 0
            || dump_max_bytes == 0
        {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::InvalidLedgerConfig.code(),
                LedgerLocation::ConfigureLedgerWritePolicy.location(),
            ));
        }
        Ok(Self {
            write_attempts,
            write_retry_backoff_ms,
            flush_attempt_stall_ms,
            health_probe_interval_ms,
            dump_max_bytes,
        })
    }

    /// Attempts of one write, the first included (`write_attempts`).
    pub const fn write_attempts(&self) -> u32 {
        self.write_attempts
    }

    /// The wait before each attempt (`write_retry_backoff_ms`).
    pub fn write_retry_backoff_ms(&self) -> &[u64] {
        &self.write_retry_backoff_ms
    }

    /// How long one attempt may run before the probe stops it (`flush_attempt_stall_ms`).
    pub const fn flush_attempt_stall_ms(&self) -> u64 {
        self.flush_attempt_stall_ms
    }

    /// The probe's period (`health_probe_interval_ms`).
    pub const fn health_probe_interval_ms(&self) -> u64 {
        self.health_probe_interval_ms
    }

    /// The dump file's limit (`dump_max_bytes`).
    pub const fn dump_max_bytes(&self) -> u64 {
        self.dump_max_bytes
    }
}

/// Where the ledger reports what its write path decided (§9.1 reports, §9.2 step 4). The
/// running Runtime's sink writes the daemon's lines; its `stop` does not return.
pub trait LedgerFailureSink: Send + Sync {
    /// `ledger_write_retried`: a write failed, was read back and tried again, and committed.
    fn write_retried(&self, outcome: &Outcome);
    /// The writer wrote its dump, or could not, and the Runtime must stop now: `outcome` is
    /// `ledger_write_failed` (with the dump's `path`) or `ledger_dump_failed`.
    fn stop(&self, outcome: &Outcome);
}

/// Milliseconds of a monotonic clock. The stall watch compares two of its readings (§9.1, T8).
pub type LedgerMonotonicClock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The process's monotonic clock in milliseconds since the plan was made.
fn process_clock() -> LedgerMonotonicClock {
    let origin = Instant::now();
    Arc::new(move || u64::try_from(origin.elapsed().as_millis()).unwrap_or(u64::MAX))
}

/// The dump file's first line (§9.3).
#[derive(Debug, Clone, Serialize)]
struct DumpHeader {
    dump_version: u64,
    state_root: String,
    owner_epoch: OwnerEpoch,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_commit: Option<String>,
    started_at_unix_ms: u64,
}

/// How a running Runtime's writer handles a failed write (§9): its policy, its two dump places
/// and where it reports.
#[derive(Clone)]
pub struct LedgerFailurePlan {
    policy: LedgerWritePolicy,
    dump_path: PathBuf,
    fallback_dump_path: Option<PathBuf>,
    header: DumpHeader,
    sink: Option<Arc<dyn LedgerFailureSink>>,
    clock: LedgerMonotonicClock,
    /// Workflow #383 AO-2: the capture queue, flush batches and index of this writer (§7, §8,
    /// §11, §15).
    capture: LedgerCapturePolicy,
}

impl fmt::Debug for LedgerFailurePlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LedgerFailurePlan")
            .field("policy", &self.policy)
            .field("dump_path", &"<redacted-path>")
            .field("fallback", &self.fallback_dump_path.is_some())
            .field("sink", &self.sink.is_some())
            .field("capture", &self.capture)
            .finish()
    }
}

impl LedgerFailurePlan {
    /// The plan of the writer of `state_root` (its canonical path), whose owner epoch started at
    /// `started_at_unix_ms`: its dump file in the state root and, when `fallback_dir` is known,
    /// in the system drive's `%LOCALAPPDATA%\ActingCommand\ledger-dumps` (§9.3).
    pub fn new(
        policy: LedgerWritePolicy,
        state_root: &Path,
        fallback_dir: Option<&Path>,
        owner_epoch: OwnerEpoch,
        started_at_unix_ms: u64,
    ) -> Self {
        Self {
            policy,
            dump_path: state_root.join(LEDGER_DUMP_FILE_NAME),
            fallback_dump_path: fallback_dir
                .map(|directory| fallback_dump_path(directory, state_root)),
            header: DumpHeader {
                dump_version: LEDGER_DUMP_VERSION,
                state_root: state_root.to_string_lossy().into_owned(),
                owner_epoch,
                runtime_commit: None,
                started_at_unix_ms,
            },
            sink: None,
            clock: process_clock(),
            capture: LedgerCapturePolicy::default(),
        }
    }

    /// Workflow #383 AO-2: the capture queue's budget, tiers and batches and the index's keys
    /// (`ledger_observer.*`; the defaults unless set). The writer checks it when it opens.
    pub fn with_capture_policy(mut self, policy: LedgerCapturePolicy) -> Self {
        self.capture = policy;
        self
    }

    /// The capture queue's policy (Workflow #383 AO-2).
    pub fn capture_policy(&self) -> &LedgerCapturePolicy {
        &self.capture
    }

    /// The commit the Runtime was built from, for the dump's header.
    pub fn with_runtime_commit(mut self, commit: impl Into<String>) -> Self {
        self.header.runtime_commit = Some(commit.into());
        self
    }

    /// Where the writer reports retries and its stop.
    pub fn with_sink(mut self, sink: Arc<dyn LedgerFailureSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    /// The clock of the stall watch (a test drives its own).
    pub fn with_clock(mut self, clock: LedgerMonotonicClock) -> Self {
        self.clock = clock;
        self
    }

    pub fn policy(&self) -> &LedgerWritePolicy {
        &self.policy
    }

    /// The dump file in the state root.
    pub fn dump_path(&self) -> &Path {
        &self.dump_path
    }

    /// The dump file in the fallback folder, when the folder is known.
    pub fn fallback_dump_path(&self) -> Option<&Path> {
        self.fallback_dump_path.as_deref()
    }
}

/// `ledger-dump-<h>.jsonl` in `fallback_dir`, `<h>` the first 16 lowercase hex digits of the
/// SHA-256 of the state root's path in UTF-8 (§9.3).
pub fn fallback_dump_path(fallback_dir: &Path, state_root: &Path) -> PathBuf {
    let digest = format!(
        "{:x}",
        Sha256::digest(state_root.to_string_lossy().as_bytes())
    );
    fallback_dir.join(format!("ledger-dump-{}.jsonl", &digest[..16]))
}

/// Why the write path stopped (§9.1 reports, §9.2 step 2).
#[derive(Debug, Clone)]
pub(crate) struct WriteStop {
    attempts: u32,
    io_kind: Option<IoKind>,
    timeout_ms: Option<u64>,
    cause: Option<Outcome>,
}

impl WriteStop {
    /// `attempts` attempts were made; `last` is the failure that ended them.
    fn after(attempts: u32, last: &GlobalLedgerError) -> Self {
        Self {
            attempts,
            io_kind: last.write_failure_kind(),
            timeout_ms: None,
            cause: Some(Outcome::relayed(last.code())),
        }
    }

    /// Workflow #383 AO-2 (§7): the memory segment reached its hard limit while attempt
    /// `attempts` (0: none) was in progress.
    pub(crate) fn at_limit(attempts: u32) -> Self {
        Self {
            attempts,
            io_kind: None,
            timeout_ms: None,
            cause: None,
        }
    }

    /// Workflow #383 AO-2 (§7): a spill could not be written (`failed`), or the dump would pass
    /// `dump_max_bytes`, while attempt `attempts` (0: none) was in progress.
    pub(crate) fn spill_failed(attempts: u32, failed: DumpFailure) -> Self {
        Self {
            attempts,
            io_kind: match failed {
                DumpFailure::Io(_, kind) => Some(kind),
                DumpFailure::Full => None,
            },
            timeout_ms: None,
            cause: match failed {
                DumpFailure::Io(..) => Some(Outcome::new(LedgerCode::LedgerIo)),
                DumpFailure::Full => None,
            },
        }
    }

    /// `ledger_write_failed` with this stop's values, and the dump's `path` once it is written.
    fn write_failed(&self, path: Option<&Path>) -> Outcome {
        let mut outcome = Outcome::new(LedgerCode::LedgerWriteFailed).attempts(self.attempts);
        if let Some(kind) = self.io_kind {
            outcome = outcome.io_kind(kind);
        }
        if let Some(timeout_ms) = self.timeout_ms {
            outcome = outcome.timeout_ms(timeout_ms);
        }
        if let Some(path) = path {
            outcome = outcome.path(path);
        }
        match &self.cause {
            Some(cause) => outcome.caused_by(cause.clone()),
            None => outcome,
        }
    }
}

/// The writer's watch over its own write path, shared by the writer thread, the store and the
/// probe (§9.1, T8).
#[derive(Default)]
struct Watch {
    /// The writer dumped (or is dumping) and writes nothing more.
    sealed: bool,
    /// The attempt in progress: its number and when it started (the plan's clock).
    attempt: Option<(u32, u64)>,
    /// The records the writer itself writes, kept until they commit (§9.1 "the batch in flight
    /// is still in memory"); a joint record is never kept, its transaction rolls back whole. The
    /// capture queue keeps the batches it hands to the writer itself.
    in_flight: Vec<HeldRecord>,
    /// Why the write path gave up, for the writer to dump and stop.
    gave_up: Option<WriteStop>,
}

/// One running writer's write-failure state.
pub(crate) struct WriteFailureState {
    plan: LedgerFailurePlan,
    watch: Mutex<Watch>,
}

/// The error every write and every queued command meets once the writer is sealed.
pub(crate) fn sealed_error() -> GlobalLedgerError {
    GlobalLedgerError::fatal(
        LedgerCode::LedgerWriteFailed.code(),
        LedgerLocation::WriteLedgerDump.location(),
    )
}

impl WriteFailureState {
    pub(crate) fn new(plan: LedgerFailurePlan) -> Arc<Self> {
        Arc::new(Self {
            plan,
            watch: Mutex::new(Watch::default()),
        })
    }

    pub(crate) fn plan(&self) -> &LedgerFailurePlan {
        &self.plan
    }

    fn watch(&self) -> MutexGuard<'_, Watch> {
        self.watch.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn is_sealed(&self) -> bool {
        self.watch().sealed
    }

    /// Before a write's first attempt: `records` are kept until the write ends. A sealed writer
    /// writes nothing more.
    pub(crate) fn hold(&self, records: Vec<HeldRecord>) -> GlobalLedgerResult<()> {
        let mut watch = self.watch();
        if watch.sealed {
            return Err(sealed_error());
        }
        watch.in_flight = records;
        Ok(())
    }

    /// Workflow #383 AO-2: the number of the write attempt in progress, if one is.
    pub(crate) fn attempt_in_progress(&self) -> Option<u32> {
        self.watch().attempt.map(|(attempt, _)| attempt)
    }

    /// The records committed, or were refused without a failed write: nothing of them is
    /// dumped.
    pub(crate) fn release(&self) {
        self.watch().in_flight.clear();
    }

    /// Waits the configured time before attempt `attempt` (1-based).
    pub(crate) fn wait_before(&self, attempt: u32) {
        let wait = usize::try_from(attempt.saturating_sub(1))
            .ok()
            .and_then(|index| self.plan.policy.write_retry_backoff_ms.get(index).copied())
            .unwrap_or(0);
        if wait > 0 {
            thread::sleep(Duration::from_millis(wait));
        }
    }

    /// Attempt `attempt` starts now; a sealed writer starts none.
    pub(crate) fn begin_attempt(&self, attempt: u32) -> GlobalLedgerResult<()> {
        let now = (self.plan.clock)();
        let mut watch = self.watch();
        if watch.sealed {
            return Err(sealed_error());
        }
        watch.attempt = Some((attempt, now));
        Ok(())
    }

    /// The attempt ended; true when the probe sealed the writer while it ran.
    pub(crate) fn end_attempt(&self) -> bool {
        let mut watch = self.watch();
        watch.attempt = None;
        watch.sealed
    }

    /// The write path gives up after `attempts` attempts, the last failing with `last`; the
    /// writer dumps and stops. Returns the error the write ends with.
    pub(crate) fn give_up(&self, attempts: u32, last: GlobalLedgerError) -> GlobalLedgerError {
        let stop = WriteStop::after(attempts, &last);
        let error = if last.is_write_failure() {
            GlobalLedgerError::fatal(
                LedgerCode::LedgerWriteFailed.code(),
                LedgerLocation::LedgerWriteRetry.location(),
            )
            .with_detail(format!(
                "attempts={attempts}; last={}@{}",
                last.code(),
                last.operation()
            ))
        } else {
            last
        };
        let mut watch = self.watch();
        if watch.gave_up.is_none() {
            watch.gave_up = Some(stop);
        }
        error
    }

    /// Why the write path gave up, once: the writer then dumps and stops.
    pub(crate) fn take_give_up(&self) -> Option<WriteStop> {
        self.watch().gave_up.take()
    }

    /// The writer seals itself (§9.2 step 1); `Some` with the records in flight when this call
    /// sealed, `None` when the writer was sealed already.
    pub(crate) fn seal(&self) -> Option<Vec<HeldRecord>> {
        let mut watch = self.watch();
        if watch.sealed {
            return None;
        }
        watch.sealed = true;
        Some(std::mem::take(&mut watch.in_flight))
    }

    /// T8: seals the writer when the attempt in progress started more than
    /// `flush_attempt_stall_ms` ago, with the record in flight and the stop to report.
    pub(crate) fn seal_if_stalled(&self) -> Option<(Vec<HeldRecord>, WriteStop)> {
        let now = (self.plan.clock)();
        let mut watch = self.watch();
        let (attempt, started) = watch.attempt?;
        if watch.sealed || now.saturating_sub(started) <= self.plan.policy.flush_attempt_stall_ms {
            return None;
        }
        watch.sealed = true;
        Some((
            std::mem::take(&mut watch.in_flight),
            WriteStop {
                attempts: attempt,
                io_kind: None,
                timeout_ms: Some(self.plan.policy.flush_attempt_stall_ms),
                cause: None,
            },
        ))
    }

    /// `ledger_write_retried` after a write that failed `failures` times committed.
    pub(crate) fn report_retried(&self, failures: u32, io_kind: Option<IoKind>, elapsed: Duration) {
        let Some(sink) = &self.plan.sink else {
            return;
        };
        let mut outcome = Outcome::new(LedgerCode::LedgerWriteRetried)
            .attempts(failures)
            .elapsed_ms(elapsed);
        if let Some(kind) = io_kind {
            outcome = outcome.io_kind(kind);
        }
        sink.write_retried(&outcome);
    }

    /// §9.2 steps 2-4 (Workflow #383 AO-2: with the capture queue's spilled segment): the dump
    /// gets `records` in capture order, then the ledger's own `runtime.failed` numbered
    /// `fatal_seq`, then its trailer. `file` is the dump file the spill thread opened, if any;
    /// otherwise the dump goes to the state root, else to the fallback folder. Then the sink
    /// stops the Runtime.
    pub(crate) fn write_dump_and_stop(
        &self,
        mut records: Vec<HeldRecord>,
        file: Option<DumpFile>,
        fatal_seq: u64,
        stop: &WriteStop,
    ) {
        records.sort_by_key(|record| record.capture_seq.unwrap_or(fatal_seq));
        let fatal = fatal_record(stop.write_failed(None));
        if let Ok(draft) = &fatal {
            records.push(HeldRecord::captured(
                fatal_seq,
                DumpedDraft::from_sanitized(draft),
            ));
        }
        let written = match file {
            Some(file) => Ok(file),
            None => self.create_dump(),
        }
        .and_then(|mut file| {
            if file.append_fitting(&records, self.plan.policy.dump_max_bytes)? {
                file.seal(
                    CodeStr::from(LedgerCode::LedgerWriteFailed.code()),
                    self.plan.policy.dump_max_bytes,
                )?;
            }
            Ok(file.path)
        });
        let mut outcome = match written {
            Ok(path) => stop.write_failed(Some(&path)),
            Err(failure) => {
                let mut outcome = Outcome::new(LedgerCode::LedgerDumpFailed).paths(
                    [
                        Some(&self.plan.dump_path),
                        self.plan.fallback_dump_path.as_ref(),
                    ]
                    .into_iter()
                    .flatten(),
                );
                if let DumpFailure::Io(operation, kind) = failure {
                    outcome = outcome.io_kind(kind).io_op(operation);
                }
                outcome.caused_by(stop.write_failed(None))
            }
        };
        // The ledger's own record could not be drafted (it has a fixed, neutral shape): the
        // stop says so beside its cause.
        if let Err(error) = fatal {
            outcome = outcome.link(CauseRelation::Secondary, Outcome::relayed(error.code()));
        }
        if let Some(sink) = &self.plan.sink {
            sink.stop(&outcome);
        }
    }

    /// Workflow #383 AO-2: creates the dump file at the first place that takes it (never over an
    /// existing file), with its header, synced.
    pub(crate) fn create_dump(&self) -> Result<DumpFile, DumpFailure> {
        let header = json_line(&self.plan.header)
            .map_err(|_| DumpFailure::Io(IoOp::Write, IoKind::Other))?;
        let mut last = DumpFailure::Io(IoOp::Write, IoKind::Other);
        for path in [
            Some(&self.plan.dump_path),
            self.plan.fallback_dump_path.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            match write_dump_file(path, &header) {
                Ok(()) => {
                    return Ok(DumpFile {
                        path: path.clone(),
                        bytes: header.len() as u64,
                        count: 0,
                        first_capture_seq: None,
                        last_capture_seq: None,
                        lines: Sha256::new(),
                    });
                }
                Err((operation, error)) => {
                    last = DumpFailure::Io(operation, IoKind::from_error_kind(error.kind()));
                }
            }
        }
        Err(last)
    }
}

/// Workflow #383 AO-2: one record kept for the dump: its capture number (absent for a record the
/// writer writes itself until the dump numbers it) and its draft.
pub(crate) struct HeldRecord {
    capture_seq: Option<u64>,
    draft: DumpedDraft,
}

impl HeldRecord {
    /// A record the writer writes itself.
    pub(crate) fn own(draft: DumpedDraft) -> Self {
        Self {
            capture_seq: None,
            draft,
        }
    }

    /// A captured record.
    pub(crate) fn captured(capture_seq: u64, draft: DumpedDraft) -> Self {
        Self {
            capture_seq: Some(capture_seq),
            draft,
        }
    }

    pub(crate) const fn capture_seq(&self) -> Option<u64> {
        self.capture_seq
    }

    /// Numbers a record the writer wrote itself, for the dump.
    pub(crate) fn number(&mut self, capture_seq: u64) {
        self.capture_seq = Some(capture_seq);
    }
}

/// Workflow #383 AO-2: why a dump file could not take a write: an I/O step failed, or the file
/// would pass `dump_max_bytes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DumpFailure {
    Io(IoOp, IoKind),
    Full,
}

/// Workflow #383 AO-2 (§7, §9.3): the dump file the spill thread and §9.2 append to: its place,
/// its size and what its trailer will say.
pub(crate) struct DumpFile {
    path: PathBuf,
    bytes: u64,
    count: u64,
    first_capture_seq: Option<u64>,
    last_capture_seq: Option<u64>,
    lines: Sha256,
}

impl DumpFile {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one line per record, in order, and syncs; each line's offset and length. Nothing
    /// is written when the lines would pass `max_bytes`.
    pub(crate) fn append(
        &mut self,
        records: &[HeldRecord],
        max_bytes: u64,
    ) -> Result<Vec<(u64, u64)>, DumpFailure> {
        let lines = record_lines(records)?;
        let total = lines.iter().map(|(_, line)| line.len() as u64).sum::<u64>();
        if self.bytes.saturating_add(total) > max_bytes {
            return Err(DumpFailure::Full);
        }
        self.write(&lines)
    }

    /// Appends the lines of `records` that fit within `max_bytes`, in order, and syncs; whether
    /// all of them fit (§9.2: a dump that would pass the limit keeps what fits and no trailer).
    fn append_fitting(
        &mut self,
        records: &[HeldRecord],
        max_bytes: u64,
    ) -> Result<bool, DumpFailure> {
        let lines = record_lines(records)?;
        let mut fitting = Vec::with_capacity(lines.len());
        let mut size = self.bytes;
        let mut all = true;
        for line in lines {
            size = size.saturating_add(line.1.len() as u64);
            if size > max_bytes {
                all = false;
                break;
            }
            fitting.push(line);
        }
        self.write(&fitting)?;
        Ok(all)
    }

    fn write(&mut self, lines: &[(u64, Vec<u8>)]) -> Result<Vec<(u64, u64)>, DumpFailure> {
        #[cfg(any(test, feature = "test-hooks"))]
        if crate::write_fault::dump_refused(&self.path) {
            return Err(DumpFailure::Io(IoOp::Write, IoKind::StorageFull));
        }
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|error| DumpFailure::Io(IoOp::Open, IoKind::from_error_kind(error.kind())))?;
        let mut placed = Vec::with_capacity(lines.len());
        let mut offset = self.bytes;
        let mut bytes = Vec::new();
        for (_, line) in lines {
            placed.push((offset, line.len() as u64));
            offset = offset.saturating_add(line.len() as u64);
            bytes.extend_from_slice(line);
        }
        file.write_all(&bytes)
            .map_err(|error| DumpFailure::Io(IoOp::Write, IoKind::from_error_kind(error.kind())))?;
        file.sync_all()
            .map_err(|error| DumpFailure::Io(IoOp::Sync, IoKind::from_error_kind(error.kind())))?;
        for (capture_seq, line) in lines {
            self.lines.update(line);
            self.count += 1;
            self.first_capture_seq = Some(
                self.first_capture_seq
                    .map_or(*capture_seq, |first| first.min(*capture_seq)),
            );
            self.last_capture_seq = Some(
                self.last_capture_seq
                    .map_or(*capture_seq, |last| last.max(*capture_seq)),
            );
        }
        self.bytes = offset;
        Ok(placed)
    }

    /// Writes the trailer (§9.3) when it fits within `max_bytes`, and syncs.
    fn seal(&mut self, reason_code: CodeStr, max_bytes: u64) -> Result<(), DumpFailure> {
        let trailer = json_line(&TrailerOut {
            trailer: &Trailer {
                count: self.count,
                first_capture_seq: self.first_capture_seq.unwrap_or(0),
                last_capture_seq: self.last_capture_seq.unwrap_or(0),
                lines_sha256: format!("{:x}", self.lines.clone().finalize()),
                reason_code,
            },
        })
        .map_err(|_| DumpFailure::Io(IoOp::Write, IoKind::Other))?;
        if self.bytes.saturating_add(trailer.len() as u64) > max_bytes {
            return Ok(());
        }
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|error| DumpFailure::Io(IoOp::Open, IoKind::from_error_kind(error.kind())))?;
        file.write_all(&trailer)
            .map_err(|error| DumpFailure::Io(IoOp::Write, IoKind::from_error_kind(error.kind())))?;
        file.sync_all()
            .map_err(|error| DumpFailure::Io(IoOp::Sync, IoKind::from_error_kind(error.kind())))?;
        self.bytes = self.bytes.saturating_add(trailer.len() as u64);
        Ok(())
    }

    /// Deletes the file once the flush thread committed every record of it (§7).
    pub(crate) fn remove(&self) -> Result<(), DumpFailure> {
        fs::remove_file(&self.path)
            .map_err(|error| DumpFailure::Io(IoOp::Remove, IoKind::from_error_kind(error.kind())))
    }
}

/// The record lines of `records`, each with its capture number.
fn record_lines(records: &[HeldRecord]) -> Result<Vec<(u64, Vec<u8>)>, DumpFailure> {
    records
        .iter()
        .map(|record| {
            let capture_seq = record.capture_seq.unwrap_or(0);
            record_line(capture_seq, &record.draft)
                .map(|line| (capture_seq, line))
                .map_err(|_| DumpFailure::Io(IoOp::Write, IoKind::Other))
        })
        .collect()
}

/// Workflow #383 AO-2: reads one record line of the dump file back (the flush thread reading the
/// spilled segment): its capture number and its draft, its hash checked.
pub(crate) fn read_dump_line(
    path: &Path,
    offset: u64,
    len: u64,
) -> GlobalLedgerResult<(u64, DumpedDraft)> {
    let unreadable = || {
        GlobalLedgerError::fatal(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::LedgerReadSpilled.location(),
        )
    };
    let mut file = fs::File::open(path).map_err(|error| {
        GlobalLedgerError::io(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::LedgerReadSpilled.location(),
            &error,
        )
    })?;
    file.seek(SeekFrom::Start(offset)).map_err(|error| {
        GlobalLedgerError::io(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::LedgerReadSpilled.location(),
            &error,
        )
    })?;
    let mut line = vec![0; usize::try_from(len).map_err(|_| unreadable())?];
    file.read_exact(&mut line).map_err(|error| {
        GlobalLedgerError::io(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::LedgerReadSpilled.location(),
            &error,
        )
    })?;
    match read_line(&line) {
        Some(Line::Record(record)) => Ok((record.capture_seq, record.draft)),
        _ => Err(unreadable()),
    }
}

/// Creates the dump file at `path` (never over an existing one), writes it whole and syncs it.
fn write_dump_file(path: &Path, bytes: &[u8]) -> Result<(), (IoOp, std::io::Error)> {
    #[cfg(any(test, feature = "test-hooks"))]
    if crate::write_fault::dump_refused(path) {
        return Err((
            IoOp::Write,
            std::io::Error::from(std::io::ErrorKind::StorageFull),
        ));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| (IoOp::CreateDir, error))?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| (IoOp::Create, error))?;
    file.write_all(bytes)
        .map_err(|error| (IoOp::Write, error))?;
    file.sync_all().map_err(|error| (IoOp::Sync, error))
}

/// The record's category (§7): a decision commits with FULL sync, an observation with NORMAL.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum DumpCategory {
    Decision,
    Observation,
}

impl DumpCategory {
    fn of(draft: &DumpedDraft) -> Self {
        match crate::durability::commit_sync(draft.payload()) {
            CommitSync::Full => Self::Decision,
            CommitSync::Normal => Self::Observation,
        }
    }
}

/// The `record` member of a record line (§9.3).
#[derive(Serialize)]
struct RecordOut<'a> {
    capture_seq: u64,
    event_id: &'a EventId,
    category: DumpCategory,
    captured_at_unix_ms: u64,
    draft: &'a DumpedDraft,
}

/// A record line as written: the record member's exact bytes and their SHA-256.
#[derive(Serialize)]
struct LineOut<'a> {
    record: &'a RawValue,
    sha256: &'a str,
}

/// The trailer of a sealed dump (§9.3).
#[derive(Serialize, Deserialize)]
struct Trailer {
    count: u64,
    first_capture_seq: u64,
    last_capture_seq: u64,
    lines_sha256: String,
    reason_code: CodeStr,
}

#[derive(Serialize)]
struct TrailerOut<'a> {
    trailer: &'a Trailer,
}

/// One line of `value` with its newline.
fn json_line<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    Ok(line)
}

/// The record line of `draft`, the `capture_seq`-th record of the dump.
fn record_line(capture_seq: u64, draft: &DumpedDraft) -> Result<Vec<u8>, serde_json::Error> {
    let record = serde_json::to_string(&RecordOut {
        capture_seq,
        event_id: draft.event_id(),
        category: DumpCategory::of(draft),
        captured_at_unix_ms: draft.timestamp_unix_ms(),
        draft,
    })?;
    let sha256 = format!("{:x}", Sha256::digest(record.as_bytes()));
    let record = RawValue::from_string(record)?;
    json_line(&LineOut {
        record: &record,
        sha256: &sha256,
    })
}

/// The ledger's own `runtime.failed` that ends a sealed dump (§9.2 step 2).
fn fatal_record(outcome: Outcome) -> GlobalLedgerResult<SanitizedEventDraft> {
    ledger_draft(
        RuntimePayloadDraft::failed(
            DiagnosticCode::RuntimeDiagnostic,
            EffectDisposition::Indeterminate,
            outcome,
            DiagnosticDetailDraft::new(
                "ledger_write",
                LedgerLocation::WriteLedgerDump.location(),
                "ledger",
                LedgerLocation::LedgerWriteRetry.as_str(),
            ),
            AuditInput::new(),
        )
        .into(),
        LedgerLocation::WriteLedgerDump,
    )
}

/// A record the ledger writes itself (origin system, `global-ledger`), dated now.
fn ledger_draft(
    payload: EventPayloadDraft,
    operation: LedgerLocation,
) -> GlobalLedgerResult<SanitizedEventDraft> {
    let refused =
        || GlobalLedgerError::fatal(LedgerCode::RecoveryEventFailed.code(), operation.location());
    let identifiers = IdentifierIssuer::new().map_err(|_| refused())?;
    EventDraft::new(
        identifiers.mint_event_id().map_err(|_| refused())?,
        unix_ms_now()?,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::System,
            OriginModule::GlobalLedger,
            EventActor::System,
        ),
        EventLinksDraft::default(),
        payload,
    )
    .sanitize(
        &Sha256SecretFingerprinter::new(b"actingcommand-ledger-recovery-v2")
            .map_err(|_| refused())?,
    )
    .map_err(|_| refused())
}

fn unix_ms_now() -> GlobalLedgerResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .filter(|now| *now > 0)
        .ok_or_else(|| {
            GlobalLedgerError::fatal(
                LedgerCode::ClockBeforeEpoch.code(),
                LedgerLocation::ImportLedgerDump.location(),
            )
        })
}

/// A record line as read: the record member's exact bytes and the hash written beside them.
#[derive(Deserialize)]
struct LineIn<'a> {
    #[serde(borrow)]
    record: &'a RawValue,
    sha256: String,
}

/// The members of a record the import reads.
#[derive(Deserialize)]
struct RecordIn {
    capture_seq: u64,
    draft: DumpedDraft,
}

/// How a dump ends (§9.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DumpEnd {
    /// Its trailer, which matches every record line.
    Sealed,
    /// No trailer: the dump was cut short (a truncated last line, or a run that ended while
    /// writing it).
    Incomplete,
    /// A line in the middle does not verify, or the trailer does not match the lines.
    Invalid,
}

/// A dump as the import reads it: the records that verified, in file order.
struct ReadDump {
    started_at_unix_ms: Option<u64>,
    records: Vec<(u64, DumpedDraft)>,
    lines: Sha256,
    end: DumpEnd,
}

/// One line of a dump after the header.
enum Line {
    Record(Box<RecordIn>),
    Trailer(Trailer),
}

/// A line that parses and, for a record line, whose record member matches its hash; `None`
/// otherwise. A reader tolerates unknown members.
fn read_line(line: &[u8]) -> Option<Line> {
    let text = std::str::from_utf8(line).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    if let Some(found) = value.get("trailer") {
        return serde_json::from_value::<Trailer>(found.clone())
            .ok()
            .map(Line::Trailer);
    }
    let line = serde_json::from_str::<LineIn<'_>>(text).ok()?;
    if format!("{:x}", Sha256::digest(line.record.get().as_bytes())) != line.sha256 {
        return None;
    }
    serde_json::from_str::<RecordIn>(line.record.get())
        .ok()
        .map(|record| Line::Record(Box::new(record)))
}

/// Reads a dump. A `dump_version` newer than this build refuses the start and leaves the file
/// untouched (`ledger_dump_unsupported`); every other damage is the dump's end.
fn read_dump(bytes: &[u8]) -> GlobalLedgerResult<ReadDump> {
    let mut read = ReadDump {
        started_at_unix_ms: None,
        records: Vec::new(),
        lines: Sha256::new(),
        end: DumpEnd::Incomplete,
    };
    let mut lines = bytes.split_inclusive(|byte| *byte == b'\n');
    let Some(header) = lines.next().filter(|line| line.ends_with(b"\n")) else {
        return Ok(read);
    };
    let Ok(header) = serde_json::from_slice::<serde_json::Value>(header) else {
        read.end = DumpEnd::Invalid;
        return Ok(read);
    };
    let Some(version) = header
        .get("dump_version")
        .and_then(serde_json::Value::as_u64)
    else {
        read.end = DumpEnd::Invalid;
        return Ok(read);
    };
    if version > LEDGER_DUMP_VERSION {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::LedgerDumpUnsupported.code(),
            LedgerLocation::ImportLedgerDump.location(),
        )
        .with_detail(format!("dump_version={version}")));
    }
    read.started_at_unix_ms = header
        .get("started_at_unix_ms")
        .and_then(serde_json::Value::as_u64);
    let mut trailer = None;
    for line in lines {
        if !line.ends_with(b"\n") {
            // A truncated last line: the dump was cut short.
            read.end = DumpEnd::Incomplete;
            return Ok(read);
        }
        if trailer.is_some() {
            read.end = DumpEnd::Invalid;
            return Ok(read);
        }
        match read_line(line) {
            Some(Line::Trailer(found)) => trailer = Some(found),
            Some(Line::Record(record)) => {
                read.lines.update(line);
                read.records.push((record.capture_seq, record.draft));
            }
            None => {
                read.end = DumpEnd::Invalid;
                return Ok(read);
            }
        }
    }
    read.end = match trailer {
        None => DumpEnd::Incomplete,
        Some(trailer)
            if trailer.count == read.records.len() as u64
                && trailer.lines_sha256 == format!("{:x}", read.lines.clone().finalize()) =>
        {
            DumpEnd::Sealed
        }
        Some(_) => DumpEnd::Invalid,
    };
    Ok(read)
}

/// §9.4: imports every dump an earlier run left, the older first, after the ledger is opened and
/// verified and before any new record. Each record whose event id the ledger holds already is
/// skipped; the rest are appended in capture order with the write rules of §9.1; a damaged dump
/// gets its gap (§10); `ledger.dump_imported` records the import; once it is committed the dump
/// is deleted, or, when a line did not verify, renamed and kept.
pub(super) fn import_dumps<B: DurableStorage>(
    store: &mut EventStore<B>,
    state: &WriteFailureState,
) -> GlobalLedgerResult<bool> {
    let places = [
        Some(state.plan.dump_path.clone()),
        state.plan.fallback_dump_path.clone(),
    ];
    let mut dumps = Vec::new();
    // Every dump is read before anything is written: a newer one refuses the start first.
    for path in places.into_iter().flatten() {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(GlobalLedgerError::io(
                    LedgerCode::LedgerIo.code(),
                    LedgerLocation::ImportLedgerDump.location(),
                    &error,
                ));
            }
        };
        let modified_unix_ms = fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok());
        let read = read_dump(&bytes)?;
        dumps.push((path, read, modified_unix_ms));
    }
    dumps.sort_by_key(|(_, read, _)| read.started_at_unix_ms);
    let mut complete = false;
    for (path, read, modified_unix_ms) in dumps {
        complete |= import_dump(store, &path, read, modified_unix_ms)?;
    }
    Ok(complete)
}

/// Imports one dump (§9.4 steps 3-6). Workflow #383 AO-2: each imported record keeps its
/// capture; whether the dump ended its epoch complete (its trailer, its last record
/// `runtime.failed`) is returned (§10: no tail gap for such an epoch).
fn import_dump<B: DurableStorage>(
    store: &mut EventStore<B>,
    path: &Path,
    mut read: ReadDump,
    modified_unix_ms: Option<u64>,
) -> GlobalLedgerResult<bool> {
    read.records.sort_by_key(|(capture_seq, _)| *capture_seq);
    let ends_failed = read.records.last().is_some_and(|(_, draft)| {
        draft.payload().event_type() == actingcommand_contract::EventType::RuntimeFailed
    });
    let mut imported = 0_u64;
    let mut duplicates = 0_u64;
    let mut last_verified_unix_ms = read.started_at_unix_ms;
    let mut end = read.end;
    for (capture_seq, draft) in read.records {
        let captured_at = draft.timestamp_unix_ms();
        if store.indexes.contains_event_id(draft.event_id()) {
            duplicates += 1;
            last_verified_unix_ms = Some(captured_at);
            continue;
        }
        let source = draft.source().copied();
        // A record that verified but is not a valid record ends the import as a damaged line.
        let Ok(event) = draft.into_event(store.next_sequence).and_then(|event| {
            event.with_capture(actingcommand_contract::RecordCapture {
                capture_seq,
                captured_at_unix_ms: captured_at,
                source,
            })
        }) else {
            end = DumpEnd::Invalid;
            break;
        };
        store.persist_imported(event)?;
        imported += 1;
        last_verified_unix_ms = Some(captured_at);
    }
    let now = unix_ms_now()?;
    let gap_end = modified_unix_ms.unwrap_or(now);
    let gap_start = last_verified_unix_ms.unwrap_or(gap_end).min(gap_end).max(1);
    let gap_end = gap_end.max(gap_start);
    let kept = (end == DumpEnd::Invalid).then(|| kept_path(path, now));
    let gap = match (&kept, end) {
        (Some(kept), _) => Some((
            LedgerGapKind::DumpInvalid,
            Outcome::new(LedgerCode::LedgerDumpInvalid)
                .gap_start_unix_ms(gap_start)
                .gap_end_unix_ms(gap_end)
                .kept_path(kept),
        )),
        (None, DumpEnd::Incomplete) => Some((
            LedgerGapKind::DumpIncomplete,
            Outcome::new(LedgerCode::LedgerDumpIncomplete)
                .gap_start_unix_ms(gap_start)
                .gap_end_unix_ms(gap_end),
        )),
        (None, _) => None,
    };
    if let Some((gap_kind, outcome)) = gap {
        let draft = ledger_draft(
            LedgerPayloadDraft::dump_gap(gap_kind, gap_start, gap_end, outcome, AuditInput::new())
                .into(),
            LedgerLocation::ImportLedgerDump,
        )?;
        let event =
            PersistedEvent::from_sanitized(store.next_sequence, draft).map_err(|error| {
                GlobalLedgerError::fatal(error.code(), LedgerLocation::ImportLedgerDump.location())
            })?;
        store.persist_imported(event)?;
    }
    let draft = ledger_draft(
        LedgerPayloadDraft::dump_imported(
            Outcome::new(LedgerCode::LedgerDumpImported)
                .count(imported)
                .duplicate_count(duplicates)
                .dump_sealed(end == DumpEnd::Sealed)
                .dump_sha256(format!("sha256:{:x}", read.lines.finalize())),
            AuditInput::new(),
        )
        .into(),
        LedgerLocation::ImportLedgerDump,
    )?;
    let event = PersistedEvent::from_sanitized(store.next_sequence, draft).map_err(|error| {
        GlobalLedgerError::fatal(error.code(), LedgerLocation::ImportLedgerDump.location())
    })?;
    store.persist_imported(event)?;
    // The import is committed: the dump goes, or a damaged one is kept for a person.
    let removed = match &kept {
        Some(kept) => fs::rename(path, kept),
        None => fs::remove_file(path),
    };
    removed.map_err(|error| {
        GlobalLedgerError::io(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::ImportLedgerDump.location(),
            &error,
        )
    })?;
    Ok(end == DumpEnd::Sealed && ends_failed)
}

/// Workflow #383 AO-2 (`contracts/ledger-observer.md` §10, §12 step 5): the start's tail gap.
/// The previous epoch ended without its close (the writer's owner record is still active) and
/// without a complete dump: its tail is unconfirmed from the last committed record to this
/// start, the count unknown.
pub(super) fn append_tail_gap<B: DurableStorage>(
    store: &mut EventStore<B>,
) -> GlobalLedgerResult<()> {
    let now = unix_ms_now()?;
    let gap_start = store
        .events
        .last()
        .map_or(now, PersistedEvent::timestamp_unix_ms)
        .clamp(1, now);
    let draft = ledger_draft(
        LedgerPayloadDraft::dump_gap(
            LedgerGapKind::TailUnconfirmed,
            gap_start,
            now,
            Outcome::new(LedgerCode::LedgerTailUnconfirmed)
                .gap_start_unix_ms(gap_start)
                .gap_end_unix_ms(now),
            AuditInput::new(),
        )
        .into(),
        LedgerLocation::LedgerTailGap,
    )?;
    let event = PersistedEvent::from_sanitized(store.next_sequence, draft).map_err(|error| {
        GlobalLedgerError::fatal(error.code(), LedgerLocation::LedgerTailGap.location())
    })?;
    store.persist_imported(event).map(|_| ())
}

/// `<dump file stem>.invalid-<unix_ms>.jsonl` beside the dump (§9.4).
fn kept_path(path: &Path, now_unix_ms: u64) -> PathBuf {
    let stem = path
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
    path.with_file_name(format!("{stem}.invalid-{now_unix_ms}.jsonl"))
}
