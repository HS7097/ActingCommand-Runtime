// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #383 AO-2 (`contracts/ledger-observer.md` §2.1, §7, §8): the capture queue.
//!
//! One queue in capture order with two segments, memory and the spilled dump segment:
//! committed < batch in flight < dump segment < memory segment. A capture takes its number and
//! enqueues under one lock and returns at once; it never blocks, never fails and never drops.
//! The memory segment's records and bytes are watched by three tiers with hysteresis (the shared
//! watermark module of `actingcommand-artifact-store`): tier 1 reports
//! `ledger_capture_buffer_busy` and lets the flush thread take larger batches, tier 2 makes the
//! spill thread append the oldest memory records to the dump file down to the tier-2 release
//! line (`ledger_capture_spilled`), tier 3 reports `ledger_capture_buffer_overloaded` and spills
//! everything but the batch in flight. A failing spill, the memory segment's hard limit or the
//! dump file's limit stop the writer by §9.2. The flush thread (the writer thread) reads the
//! dump segment first, then memory.
//!
//! The spill thread also prepares records in the background: it assembles and sanitizes a
//! gate's capture and measures every record's size, so the capture itself does neither.

use super::write_failure::{self, DumpFile, HeldRecord, WriteFailureState, WriteStop};
use super::{
    GlobalLedgerError, GlobalLedgerResult, LedgerAppendObservation, LedgerAppendOutcome,
    LedgerTransactionWork, LedgerWriterWorkObservation, WriterCommand,
};
use crate::PersistedEvent;
use crate::codes::{LedgerCode, LedgerLocation};
use crate::fact::DumpedDraft;
use actingcommand_artifact_store::{WatermarkLines, WatermarkRatios, WatermarkTier};
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{
    AuditInput, CaptureItem, CaptureTicket, CaptureTier, CommittedRecord, EventActor, EventDraft,
    EventId, EventLinksDraft, EventOrigin, EventSeverity, EventSource, IdentifierIssuer,
    IssuedEventId, LedgerBacklogTier, LedgerCapture, LedgerFlushBarrier, LedgerPayloadDraft,
    LedgerSegmentEdge, OriginModule, SanitizedEventDraft, SecretFingerprinter, SourceRevision,
};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// `ledger_observer.tag.workbench_patterns`' default (§15).
pub const DEFAULT_TAG_WORKBENCH_PATTERNS: [&str; 10] = [
    "task.*.last_duration_ms",
    "task.*.last_outcome",
    "task.*.failure_streak",
    "task.*.completed_at_unix_ms",
    "host.scheduling_pause",
    "host.install_transition",
    "host.install_transition.pauses",
    "host.perf_baseline.*",
    "host.perf_calibration.*",
    "host.config_overrides",
];

/// The preparation work the spill thread takes at once.
const PREPARE_CHUNK: usize = 64;

/// What `ledger_observer.*` sets for the capture queue, the flush batches and the index (§7,
/// §8, §11, §15).
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerCapturePolicy {
    /// `buffer_bytes`: the memory segment's byte limit (hard limit = 100 %).
    pub buffer_bytes: u64,
    /// `buffer_records`: the memory segment's record limit.
    pub buffer_records: u64,
    /// `buffer.tier1_ratio`, `buffer.tier2_ratio`, `buffer.tier3_ratio` and
    /// `buffer.hysteresis_ratio`.
    pub ratios: WatermarkRatios,
    /// `spill_batch_bytes`: bytes per spill write.
    pub spill_batch_bytes: u64,
    /// `flush_batch_records`: records per flush batch below tier 1.
    pub flush_batch_records: u64,
    /// `flush_batch_records_busy`: records per flush batch at tier 1 and above.
    pub flush_batch_records_busy: u64,
    /// `request_index_records`: the size of the index's `request` ring.
    pub request_index_records: u64,
    /// `tag.workbench_patterns`: the tagged workbench keys.
    pub workbench_patterns: Vec<String>,
}

impl Default for LedgerCapturePolicy {
    fn default() -> Self {
        Self {
            buffer_bytes: 64 * 1024 * 1024,
            buffer_records: 20_480,
            ratios: WatermarkRatios {
                tier1: 0.60,
                tier2: 0.75,
                tier3: 0.90,
                hysteresis: 0.10,
            },
            spill_batch_bytes: 4 * 1024 * 1024,
            flush_batch_records: 256,
            flush_batch_records_busy: 2_048,
            request_index_records: 4_096,
            workbench_patterns: DEFAULT_TAG_WORKBENCH_PATTERNS
                .iter()
                .map(|pattern| (*pattern).to_owned())
                .collect(),
        }
    }
}

impl LedgerCapturePolicy {
    /// Every count at least 1; the tiers rise strictly within (0, 1] with the hysteresis within
    /// (0, 1); for both budgets the lines are distinct and each release line lies below its
    /// activation line; tier 3 − tier 2 of `buffer_bytes` holds at least `spill_batch_bytes`
    /// (§7 "configuration check"); the patterns are non-empty texts without spaces. Otherwise
    /// `invalid_ledger_config`.
    pub fn validate(&self) -> GlobalLedgerResult<()> {
        let refused = || {
            GlobalLedgerError::fatal(
                LedgerCode::InvalidLedgerConfig.code(),
                LedgerLocation::ConfigureLedgerCapture.location(),
            )
        };
        if [
            self.buffer_bytes,
            self.buffer_records,
            self.spill_batch_bytes,
            self.flush_batch_records,
            self.flush_batch_records_busy,
            self.request_index_records,
        ]
        .contains(&0)
            || self.workbench_patterns.iter().any(|pattern| {
                pattern.is_empty()
                    || pattern
                        .chars()
                        .any(|character| character.is_whitespace() || character.is_control())
            })
        {
            return Err(refused());
        }
        self.ratios
            .validate(self.buffer_bytes, self.spill_batch_bytes)
            .map_err(|_| refused())?;
        self.ratios
            .validate(self.buffer_records, 1)
            .map_err(|_| refused())
    }

    /// The index table's part of the policy.
    pub(super) fn index_policy(&self) -> super::index::IndexPolicy {
        super::index::IndexPolicy::new(self.workbench_patterns.clone(), self.request_index_records)
    }
}

/// The capture queue as status sees it (§14.1 T3); read from memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerCaptureStatus {
    pub tier: CaptureTier,
    pub memory_records: u64,
    pub memory_bytes: u64,
    pub dump_records: u64,
    pub dump_bytes: u64,
    /// Capture numbers handed out since the writer opened.
    pub captured: u64,
}

/// The reply a waiting caller of `append`, `append_deferred` or `append_transaction` gets.
pub(super) enum Reply {
    Append {
        response: SyncSender<LedgerAppendOutcome>,
        observation: Option<Box<LedgerAppendObservation>>,
    },
    Joint(SyncSender<GlobalLedgerResult<PersistedEvent>>),
}

impl Reply {
    /// The flush thread took the record.
    pub(super) fn received(&mut self, previous: LedgerWriterWorkObservation) {
        if let Self::Append {
            observation: Some(observation),
            ..
        } = self
        {
            observation.writer_received();
            observation.previous_writer_work = previous;
        }
    }

    /// The batch holding the record goes to the store's persist call at `at`: the start of the
    /// reply's `ledger_durable` span, which ends with persistence.
    pub(super) fn durable_started(&mut self, at: Instant) {
        if let Self::Append {
            observation: Some(observation),
            ..
        } = self
        {
            observation.durable_started_at = Some(at);
        }
    }

    /// The record's write returned (committed or not).
    pub(super) fn persisted(&mut self, succeeded: bool) {
        if let Self::Append {
            observation: Some(observation),
            ..
        } = self
        {
            observation.persisted(succeeded);
        }
    }

    /// Sends the result; false when the caller stopped waiting.
    pub(super) fn send(self, result: GlobalLedgerResult<PersistedEvent>) -> bool {
        match self {
            Self::Append {
                response,
                mut observation,
            } => {
                if let Some(observation) = &mut observation {
                    observation.reply_ready(result.is_ok());
                }
                response
                    .send((result, observation.map(|observation| *observation)))
                    .is_ok()
            }
            Self::Joint(response) => response.send(result).is_ok(),
        }
    }
}

/// A gate's capture with the fingerprinter its sanitizing uses.
struct ItemBody {
    item: CaptureItem,
    fingerprinter: Arc<dyn SecretFingerprinter + Send + Sync>,
}

/// A finding of the ledger itself, with the event id issued for it.
struct FindingBody {
    outcome: Outcome,
    edge: Option<LedgerSegmentEdge>,
    event_id: IssuedEventId,
}

/// How a memory record stands.
enum Body {
    /// A gate's capture, not yet assembled and sanitized.
    Item(Box<ItemBody>),
    /// A finding of the queue or the flush thread, not yet drafted.
    Finding(Box<FindingBody>),
    /// A sanitized draft: a direct append, a prepared capture, a drafted finding.
    Draft(Arc<SanitizedEventDraft>),
    /// A capture that failed assembly or sanitizing; its gap is written in its place.
    Rejected { event_id: EventId, code: Code },
    /// Being prepared on another thread.
    Taken,
}

struct Entry {
    capture_seq: u64,
    captured_at_unix_ms: u64,
    source: Option<SourceRevision>,
    body: Body,
    work: Option<Box<dyn LedgerTransactionWork>>,
    reply: Option<Reply>,
    /// The record's measured size; 0 until measured.
    bytes: u64,
    measured: bool,
}

/// One record of the dump segment: where its line is in the dump file.
struct Spilled {
    capture_seq: u64,
    offset: u64,
    len: u64,
    reply: Option<Reply>,
}

/// A record taken by the flush thread and not finished yet. Its draft is kept for the dump; a
/// record of the dump segment is in the file already, a joint record is never dumped.
struct InFlight {
    capture_seq: u64,
    captured_at_unix_ms: u64,
    source: Option<SourceRevision>,
    record: InFlightRecord,
}

enum InFlightRecord {
    Draft(Arc<SanitizedEventDraft>),
    Rejected(EventId, Code),
    InFile,
    Joint,
}

/// A reported stretch of a condition (§14.1): when it started and what it counted.
#[derive(Clone, Copy)]
struct Stretch {
    since: Instant,
    count: u64,
    bytes: u64,
}

/// What a captured record is when the flush thread takes it.
pub(super) enum CapturedRecord {
    Draft(Arc<SanitizedEventDraft>),
    Dumped(Box<DumpedDraft>),
    /// Failed assembly or sanitizing (with the refusal's code); the flush thread writes its
    /// gap.
    Rejected(EventId, Code),
}

/// One record of a flush batch.
pub(super) struct Captured {
    pub(super) capture_seq: u64,
    pub(super) captured_at_unix_ms: u64,
    pub(super) source: Option<SourceRevision>,
    pub(super) record: CapturedRecord,
    pub(super) work: Option<Box<dyn LedgerTransactionWork>>,
    pub(super) reply: Option<Reply>,
}

/// A record prepared off the queue lock: its new body and its measured size.
struct Prepared {
    capture_seq: u64,
    body: Body,
    bytes: Option<u64>,
}

/// The spill thread's next piece of work.
enum SpillJob {
    Stop,
    Prepare(Vec<(u64, u64, Body)>),
    Spill(Vec<Entry>),
    Seal(WriteStop),
}

struct QueueState {
    next_seq: u64,
    memory: VecDeque<Entry>,
    memory_bytes: u64,
    /// Memory records out for preparation (`Body::Taken`).
    taken: usize,
    unmeasured: usize,
    dump: VecDeque<Spilled>,
    dump_bytes: u64,
    dump_file: Option<DumpFile>,
    dump_deleting: bool,
    spilling: bool,
    spill_stretch: Option<Stretch>,
    in_flight: Vec<InFlight>,
    /// The last capture number handed to the flush thread.
    taken_through: u64,
    tier: WatermarkTier,
    busy_since: Option<Instant>,
    overloaded_since: Option<Instant>,
    /// A wake token is on its way to the writer, or the writer is draining.
    wake_pending: bool,
    /// A finding pushed under the lock still needs its wake token.
    wake_owed: bool,
    hard_limit: bool,
    sealed: bool,
    /// The writer drains for its close: the close record is the epoch's last record, so the
    /// segment edges the drain itself crosses are not reported.
    closing: bool,
    closed: bool,
    stop: bool,
}

impl QueueState {
    /// Every record up to this capture number is finished (committed, refused or rejected).
    fn done_through(&self) -> u64 {
        self.in_flight
            .iter()
            .map(|record| record.capture_seq)
            .min()
            .map_or(self.taken_through, |oldest| oldest.saturating_sub(1))
    }
}

pub(crate) struct CaptureQueue {
    state: Mutex<QueueState>,
    changed: Condvar,
    tier: AtomicU8,
    policy: LedgerCapturePolicy,
    byte_lines: WatermarkLines,
    record_lines: WatermarkLines,
    failure: Option<Arc<WriteFailureState>>,
    wake: Mutex<Option<SyncSender<WriterCommand>>>,
    identifiers: IdentifierIssuer,
}

fn capture_tier(tier: WatermarkTier) -> CaptureTier {
    match tier {
        WatermarkTier::Below => CaptureTier::BelowTier1,
        WatermarkTier::Tier1 => CaptureTier::Tier1,
        WatermarkTier::Tier2 => CaptureTier::Tier2,
        WatermarkTier::Tier3 => CaptureTier::Tier3,
    }
}

fn tier_number(tier: WatermarkTier) -> u8 {
    match tier {
        WatermarkTier::Below => 0,
        WatermarkTier::Tier1 => 1,
        WatermarkTier::Tier2 => 2,
        WatermarkTier::Tier3 => 3,
    }
}

fn tier_of_number(number: u8) -> WatermarkTier {
    match number {
        0 => WatermarkTier::Below,
        1 => WatermarkTier::Tier1,
        2 => WatermarkTier::Tier2,
        _ => WatermarkTier::Tier3,
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
        .max(1)
}

/// A writer for counting bytes: the size of a record's JSON without keeping it.
struct ByteCount(u64);

impl std::io::Write for ByteCount {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len() as u64);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The size of a record as the ledger stores it (its JSON), measured off the capture path.
fn measure(draft: &SanitizedEventDraft) -> u64 {
    let mut count = ByteCount(0);
    match serde_json::to_writer(&mut count, draft) {
        Ok(()) => count.0,
        // A record that does not serialize is refused when it is written; it has no size.
        Err(_) => 0,
    }
}

/// A record the ledger drafts itself (origin system, `global-ledger`) at `at_unix_ms`.
fn ledger_record(
    event_id: IssuedEventId,
    at_unix_ms: u64,
    payload: LedgerPayloadDraft,
) -> Result<SanitizedEventDraft, Code> {
    EventDraft::new(
        event_id,
        at_unix_ms,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::System,
            OriginModule::GlobalLedger,
            EventActor::System,
        ),
        EventLinksDraft::default(),
        payload.into(),
    )
    .sanitize(
        &super::Sha256SecretFingerprinter::new(b"actingcommand-ledger-recovery-v2")
            .map_err(|_| LedgerCode::RecoveryEventFailed.code())?,
    )
    .map_err(|error| error.code())
}

/// The gap that stands in for a capture refused on the flush or spill thread (§10): one
/// record, at its capture time, naming its event id and, for a revisioned source, its revision;
/// `cause` is why it was refused.
pub(super) fn rejected_gap(
    identifiers: &IdentifierIssuer,
    captured_at_unix_ms: u64,
    event_id: EventId,
    source: Option<SourceRevision>,
    cause: Outcome,
) -> Result<SanitizedEventDraft, Code> {
    let mut outcome = Outcome::new(LedgerCode::LedgerRecordRejected).caused_by(cause);
    if let Some(text) = super::index::id_text(&event_id) {
        outcome = outcome.event_id(text);
    }
    if let Some(source) = source {
        outcome = outcome
            .ledger_source(source.source)
            .source_revision(source.revision);
        if let Some(lineage) = super::index::id_text(&source.lineage) {
            outcome = outcome.source_lineage(lineage);
        }
    }
    let gap_id = identifiers
        .mint_event_id()
        .map_err(|_| LedgerCode::RecoveryEventFailed.code())?;
    ledger_record(
        gap_id,
        captured_at_unix_ms,
        LedgerPayloadDraft::rejected_gap(
            captured_at_unix_ms,
            event_id,
            source,
            outcome,
            AuditInput::new(),
        ),
    )
}

/// Assembles and sanitizes a capture, or drafts a finding, at its capture time.
fn prepare_body(body: Body, captured_at_unix_ms: u64) -> Body {
    match body {
        Body::Item(item) => {
            let ItemBody {
                item,
                fingerprinter,
            } = *item;
            let event_id = *item.event_id.transport();
            match EventDraft::new(
                item.event_id,
                captured_at_unix_ms,
                item.severity,
                item.origin,
                item.links,
                item.payload,
            )
            .sanitize(fingerprinter.as_ref())
            {
                Ok(draft) => Body::Draft(Arc::new(draft)),
                Err(error) => Body::Rejected {
                    event_id,
                    code: error.code(),
                },
            }
        }
        Body::Finding(finding) => {
            let FindingBody {
                outcome,
                edge,
                event_id,
            } = *finding;
            let id = *event_id.transport();
            match ledger_record(
                event_id,
                captured_at_unix_ms,
                LedgerPayloadDraft::health(outcome, edge, AuditInput::new()),
            ) {
                Ok(draft) => Body::Draft(Arc::new(draft)),
                // A finding has a fixed, neutral shape; one that does not draft is refused as
                // any record is, with its gap.
                Err(code) => Body::Rejected { event_id: id, code },
            }
        }
        other => other,
    }
}

/// A prepared body with its size (a refused capture holds nothing).
fn prepared(capture_seq: u64, body: Body) -> Prepared {
    let bytes = match &body {
        Body::Draft(draft) => Some(measure(draft)),
        Body::Rejected { .. } => Some(0),
        Body::Item(_) | Body::Finding(_) | Body::Taken => None,
    };
    Prepared {
        capture_seq,
        body,
        bytes,
    }
}

/// The dump form of a prepared record: its draft, or the gap of a refused capture.
fn dumped(
    identifiers: &IdentifierIssuer,
    capture_seq: u64,
    captured_at_unix_ms: u64,
    source: Option<SourceRevision>,
    body: Body,
) -> Option<HeldRecord> {
    let draft = match body {
        Body::Draft(draft) => Some(draft),
        Body::Rejected { event_id, code } => rejected_gap(
            identifiers,
            captured_at_unix_ms,
            event_id,
            source,
            Outcome::new(code),
        )
        .ok()
        .map(Arc::new),
        Body::Item(_) | Body::Finding(_) | Body::Taken => None,
    }?;
    Some(HeldRecord::captured(
        capture_seq,
        DumpedDraft::from_sanitized(&draft).with_source(source),
    ))
}

impl CaptureQueue {
    pub(super) fn new(
        policy: LedgerCapturePolicy,
        failure: Option<Arc<WriteFailureState>>,
        wake: SyncSender<WriterCommand>,
    ) -> GlobalLedgerResult<Self> {
        policy.validate()?;
        let identifiers = IdentifierIssuer::new().map_err(|_| {
            GlobalLedgerError::fatal(
                LedgerCode::RecoveryEventFailed.code(),
                LedgerLocation::LedgerCaptureRecord.location(),
            )
        })?;
        Ok(Self {
            state: Mutex::new(QueueState {
                next_seq: 1,
                memory: VecDeque::new(),
                memory_bytes: 0,
                taken: 0,
                unmeasured: 0,
                dump: VecDeque::new(),
                dump_bytes: 0,
                dump_file: None,
                dump_deleting: false,
                spilling: false,
                spill_stretch: None,
                in_flight: Vec::new(),
                taken_through: 0,
                tier: WatermarkTier::Below,
                busy_since: None,
                overloaded_since: None,
                wake_pending: false,
                wake_owed: false,
                hard_limit: false,
                sealed: false,
                closing: false,
                closed: false,
                stop: false,
            }),
            changed: Condvar::new(),
            tier: AtomicU8::new(0),
            byte_lines: policy.ratios.lines(policy.buffer_bytes),
            record_lines: policy.ratios.lines(policy.buffer_records),
            policy,
            failure,
            wake: Mutex::new(Some(wake)),
            identifiers,
        })
    }

    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait<'a>(&self, state: MutexGuard<'a, QueueState>) -> MutexGuard<'a, QueueState> {
        self.changed
            .wait(state)
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn identifiers(&self) -> &IdentifierIssuer {
        &self.identifiers
    }

    /// Whether the queue spills: a writer with a failure plan has a dump file to spill into.
    fn spills(&self) -> bool {
        self.failure.is_some()
    }

    /// The writer's queue for wake tokens and barrier lookups, while the writer runs.
    pub(super) fn sender(&self) -> Option<SyncSender<WriterCommand>> {
        self.wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Drops the queue's sender, so the writer's queue ends once its owner's sender is gone.
    pub(super) fn disconnect(&self) {
        self.wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }

    fn send_wake(&self) {
        if let Some(sender) = self.sender() {
            // A full writer queue is busy, and the writer drains the capture queue before each
            // of its commands.
            let _ = sender.try_send(WriterCommand::Flush);
        }
    }

    /// Releases the lock, wakes the threads that wait on the queue and sends the wake token a
    /// finding pushed under the lock still owes.
    fn release(&self, mut state: MutexGuard<'_, QueueState>) {
        let owed = std::mem::take(&mut state.wake_owed);
        drop(state);
        self.changed.notify_all();
        if owed {
            self.send_wake();
        }
    }

    /// Enqueues one record under the queue lock: its number, its place, the watermark check
    /// (two comparisons per budget) and the wakes; no I/O, no sanitizing, no serializing.
    fn push(
        &self,
        body: Body,
        captured_at_unix_ms: Option<u64>,
        source: Option<SourceRevision>,
        work: Option<Box<dyn LedgerTransactionWork>>,
        reply: Option<Reply>,
    ) -> u64 {
        let mut state = self.lock();
        let capture_seq = state.next_seq;
        state.next_seq = state.next_seq.saturating_add(1);
        let reply = match reply {
            // A caller waiting for a writer that stopped is answered at once.
            Some(mut reply) if state.sealed || state.closed => {
                let refused = if state.sealed {
                    write_failure::sealed_error()
                } else {
                    GlobalLedgerError::fatal(
                        LedgerCode::WriterUnavailable.code(),
                        LedgerLocation::LedgerCaptureRecord.location(),
                    )
                };
                drop(state);
                reply.persisted(false);
                let _ = reply.send(Err(refused));
                return capture_seq;
            }
            other => other,
        };
        let captured_at_unix_ms = captured_at_unix_ms.unwrap_or_else(now_unix_ms);
        state.memory.push_back(Entry {
            capture_seq,
            captured_at_unix_ms,
            source,
            body,
            work,
            reply,
            bytes: 0,
            measured: false,
        });
        state.unmeasured += 1;
        self.update_tier(&mut state);
        if self.spills()
            && (state.memory.len() as u64 >= self.policy.buffer_records
                || state.memory_bytes >= self.policy.buffer_bytes)
        {
            // §7: at the hard limit the capture still returns at once; the spill thread stops
            // the writer by §9.2.
            state.hard_limit = true;
        }
        if !state.wake_pending {
            state.wake_pending = true;
            state.wake_owed = true;
        }
        self.release(state);
        capture_seq
    }

    /// A gate's capture (§2.1).
    pub(super) fn capture_item(
        &self,
        item: CaptureItem,
        fingerprinter: Arc<dyn SecretFingerprinter + Send + Sync>,
        work: Option<Box<dyn LedgerTransactionWork>>,
    ) -> CaptureTicket {
        let event_id = *item.event_id.transport();
        let source = item.source;
        let capture_seq = self.push(
            Body::Item(Box::new(ItemBody {
                item,
                fingerprinter,
            })),
            None,
            source,
            work,
            None,
        );
        CaptureTicket {
            event_id,
            capture_seq,
        }
    }

    /// A sanitized draft with its caller's reply (`append`, `append_deferred`,
    /// `append_transaction`); its capture time is its own time.
    pub(super) fn capture_draft(
        &self,
        draft: SanitizedEventDraft,
        work: Option<Box<dyn LedgerTransactionWork>>,
        reply: Reply,
    ) -> u64 {
        let captured_at = draft.timestamp_unix_ms();
        self.push(
            Body::Draft(Arc::new(draft)),
            Some(captured_at),
            None,
            work,
            Some(reply),
        )
    }

    /// A finding of the flush thread (§8.3), written in capture order.
    pub(super) fn capture_finding(&self, outcome: Outcome) {
        let mut state = self.lock();
        self.push_finding(&mut state, outcome, None);
        self.release(state);
    }

    /// Pushes a finding while the lock is held. A finding whose event id cannot be issued is
    /// not written (the issuer's numbers are spent).
    fn push_finding(
        &self,
        state: &mut QueueState,
        outcome: Outcome,
        edge: Option<LedgerSegmentEdge>,
    ) {
        if state.closing && edge.is_some() {
            return;
        }
        let Ok(event_id) = self.identifiers.mint_event_id() else {
            return;
        };
        let capture_seq = state.next_seq;
        state.next_seq = state.next_seq.saturating_add(1);
        state.memory.push_back(Entry {
            capture_seq,
            captured_at_unix_ms: now_unix_ms(),
            source: None,
            body: Body::Finding(Box::new(FindingBody {
                outcome,
                edge,
                event_id,
            })),
            work: None,
            reply: None,
            bytes: 0,
            measured: false,
        });
        state.unmeasured += 1;
        if !state.wake_pending {
            state.wake_pending = true;
            state.wake_owed = true;
        }
    }

    /// A segment edge's finding: the records and bytes it counts and, on the end, the stretch's
    /// duration (§14.1).
    fn segment_finding(
        code: LedgerCode,
        edge: LedgerSegmentEdge,
        count: u64,
        bytes: u64,
        since: Option<Instant>,
    ) -> Outcome {
        let mut outcome = Outcome::new(code)
            .segment_edge(edge)
            .count(count)
            .buffer_bytes(bytes);
        if let Some(since) = since {
            outcome = outcome.elapsed_ms(since.elapsed());
        }
        outcome
    }

    /// Recomputes the tier from the memory segment's records and bytes (the higher of the two)
    /// and reports the edges of the busy and overloaded segments.
    fn update_tier(&self, state: &mut QueueState) {
        let previous = state.tier;
        let by_records = self.record_lines.tier(state.memory.len() as u64, previous);
        let by_bytes = self.byte_lines.tier(state.memory_bytes, previous);
        let tier = by_records.max(by_bytes);
        if tier == previous {
            return;
        }
        state.tier = tier;
        self.tier.store(tier_number(tier), Ordering::Release);
        if !self.spills() {
            return;
        }
        let records = state.memory.len() as u64;
        let bytes = state.memory_bytes;
        if previous < WatermarkTier::Tier1 && tier >= WatermarkTier::Tier1 {
            state.busy_since = Some(Instant::now());
            let outcome = Self::segment_finding(
                LedgerCode::LedgerCaptureBufferBusy,
                LedgerSegmentEdge::Started,
                records,
                bytes,
                None,
            );
            self.push_finding(state, outcome, Some(LedgerSegmentEdge::Started));
        }
        if previous < WatermarkTier::Tier3 && tier == WatermarkTier::Tier3 {
            state.overloaded_since = Some(Instant::now());
            let outcome = Self::segment_finding(
                LedgerCode::LedgerCaptureBufferOverloaded,
                LedgerSegmentEdge::Started,
                records,
                bytes,
                None,
            );
            self.push_finding(state, outcome, Some(LedgerSegmentEdge::Started));
        }
        if previous == WatermarkTier::Tier3 && tier < WatermarkTier::Tier3 {
            let since = state.overloaded_since.take();
            let outcome = Self::segment_finding(
                LedgerCode::LedgerCaptureBufferOverloaded,
                LedgerSegmentEdge::Ended,
                records,
                bytes,
                since,
            );
            self.push_finding(state, outcome, Some(LedgerSegmentEdge::Ended));
        }
        if previous >= WatermarkTier::Tier1 && tier < WatermarkTier::Tier1 {
            let since = state.busy_since.take();
            let outcome = Self::segment_finding(
                LedgerCode::LedgerCaptureBufferBusy,
                LedgerSegmentEdge::Ended,
                records,
                bytes,
                since,
            );
            self.push_finding(state, outcome, Some(LedgerSegmentEdge::Ended));
        }
    }

    /// The tier read (§2 call 4): lock-free, no I/O.
    pub(super) fn tier(&self) -> CaptureTier {
        capture_tier(tier_of_number(self.tier.load(Ordering::Acquire)))
    }

    pub(super) fn status(&self) -> LedgerCaptureStatus {
        let state = self.lock();
        LedgerCaptureStatus {
            tier: capture_tier(state.tier),
            memory_records: state.memory.len() as u64,
            memory_bytes: state.memory_bytes,
            dump_records: state.dump.len() as u64,
            dump_bytes: state.dump_bytes,
            captured: state.next_seq.saturating_sub(1),
        }
    }

    /// The flush thread starts a drain: everything captured so far is its target.
    pub(super) fn begin_drain(&self) -> u64 {
        let mut state = self.lock();
        state.wake_pending = false;
        state.next_seq.saturating_sub(1)
    }

    /// Whether records wait in the queue.
    pub(super) fn pending(&self) -> bool {
        let state = self.lock();
        !state.memory.is_empty() || !state.dump.is_empty() || state.spilling
    }

    fn removed_from_memory(state: &mut QueueState, entry: &Entry) {
        state.memory_bytes = state.memory_bytes.saturating_sub(entry.bytes);
        if !entry.measured {
            state.unmeasured = state.unmeasured.saturating_sub(1);
        }
    }

    /// Puts prepared bodies back by capture number and accounts their sizes.
    fn put_prepared(&self, state: &mut QueueState, prepared: Vec<Prepared>) {
        for record in prepared {
            let Ok(position) = state
                .memory
                .binary_search_by_key(&record.capture_seq, |entry| entry.capture_seq)
            else {
                continue;
            };
            let entry = &mut state.memory[position];
            let was_taken = matches!(entry.body, Body::Taken);
            if was_taken {
                entry.body = record.body;
            }
            let newly_measured = match record.bytes {
                Some(bytes) if !entry.measured => {
                    entry.bytes = bytes;
                    entry.measured = true;
                    Some(bytes)
                }
                _ => None,
            };
            if let Some(bytes) = newly_measured {
                state.memory_bytes = state.memory_bytes.saturating_add(bytes);
                state.unmeasured = state.unmeasured.saturating_sub(1);
            }
            if was_taken {
                state.taken = state.taken.saturating_sub(1);
            }
        }
        self.update_tier(state);
    }

    /// The flush thread's next batch through `through`, in capture order: the dump segment
    /// first, then memory (§8.1). It waits for a spill in progress and for records being
    /// prepared at the front; it prepares the front records not prepared yet itself. A joint
    /// record goes alone. `None` once nothing through `through` is left or the writer is
    /// sealed.
    pub(super) fn take_batch(&self, through: u64) -> GlobalLedgerResult<Option<Vec<Captured>>> {
        let mut state = self.lock();
        loop {
            if state.sealed {
                self.release(state);
                return Ok(None);
            }
            let limit = if state.tier >= WatermarkTier::Tier1 {
                self.policy.flush_batch_records_busy
            } else {
                self.policy.flush_batch_records
            };
            let limit = usize::try_from(limit).unwrap_or(usize::MAX).max(1);
            if state
                .dump
                .front()
                .is_some_and(|spilled| spilled.capture_seq <= through)
            {
                let mut refs = Vec::new();
                while refs.len() < limit {
                    match state.dump.front() {
                        Some(spilled) if spilled.capture_seq <= through => {}
                        _ => break,
                    }
                    if let Some(spilled) = state.dump.pop_front() {
                        state.dump_bytes = state.dump_bytes.saturating_sub(spilled.len);
                        state.in_flight.push(InFlight {
                            capture_seq: spilled.capture_seq,
                            captured_at_unix_ms: 0,
                            source: None,
                            record: InFlightRecord::InFile,
                        });
                        state.taken_through = state.taken_through.max(spilled.capture_seq);
                        refs.push(spilled);
                    }
                }
                let path = state
                    .dump_file
                    .as_ref()
                    .map(|file| file.path().to_path_buf());
                self.release(state);
                return read_spilled(path, refs).map(Some);
            }
            if state.spilling || state.dump_deleting {
                state = self.wait(state);
                continue;
            }
            let front = state
                .memory
                .front()
                .map(|front| (front.capture_seq, matches!(front.body, Body::Taken)));
            match front {
                None => {
                    self.release(state);
                    return Ok(None);
                }
                Some((capture_seq, _)) if capture_seq > through => {
                    self.release(state);
                    return Ok(None);
                }
                Some((_, true)) => {
                    state = self.wait(state);
                    continue;
                }
                Some((_, false)) => {}
            }
            // The front records that are not prepared yet are prepared here, off the lock.
            let mut unprepared = Vec::new();
            for entry in state.memory.iter_mut().take(limit) {
                if entry.capture_seq > through || matches!(entry.body, Body::Taken) {
                    break;
                }
                if matches!(entry.body, Body::Item(_) | Body::Finding(_)) {
                    let body = std::mem::replace(&mut entry.body, Body::Taken);
                    unprepared.push((entry.capture_seq, entry.captured_at_unix_ms, body));
                }
            }
            if !unprepared.is_empty() {
                state.taken += unprepared.len();
                drop(state);
                let done = unprepared
                    .into_iter()
                    .map(|(capture_seq, captured_at, body)| {
                        prepared(capture_seq, prepare_body(body, captured_at))
                    })
                    .collect::<Vec<_>>();
                state = self.lock();
                self.put_prepared(&mut state, done);
                self.changed.notify_all();
                continue;
            }
            let mut batch = Vec::new();
            while batch.len() < limit {
                match state.memory.front() {
                    Some(front)
                        if front.capture_seq <= through
                            && matches!(front.body, Body::Draft(_) | Body::Rejected { .. })
                            // A joint record commits alone.
                            && (front.work.is_none() || batch.is_empty()) => {}
                    _ => break,
                }
                let Some(entry) = state.memory.pop_front() else {
                    break;
                };
                Self::removed_from_memory(&mut state, &entry);
                let joint = entry.work.is_some();
                let (record, in_flight) = match entry.body {
                    Body::Draft(draft) => {
                        let kept = if joint {
                            InFlightRecord::Joint
                        } else {
                            InFlightRecord::Draft(Arc::clone(&draft))
                        };
                        (CapturedRecord::Draft(draft), kept)
                    }
                    Body::Rejected { event_id, code } => (
                        CapturedRecord::Rejected(event_id, code),
                        InFlightRecord::Rejected(event_id, code),
                    ),
                    // Checked above: only prepared records are taken.
                    Body::Item(_) | Body::Finding(_) | Body::Taken => continue,
                };
                state.in_flight.push(InFlight {
                    capture_seq: entry.capture_seq,
                    captured_at_unix_ms: entry.captured_at_unix_ms,
                    source: entry.source,
                    record: in_flight,
                });
                state.taken_through = state.taken_through.max(entry.capture_seq);
                batch.push(Captured {
                    capture_seq: entry.capture_seq,
                    captured_at_unix_ms: entry.captured_at_unix_ms,
                    source: entry.source,
                    record,
                    work: entry.work,
                    reply: entry.reply,
                });
                if joint {
                    break;
                }
            }
            self.update_tier(&mut state);
            self.release(state);
            return Ok((!batch.is_empty()).then_some(batch));
        }
    }

    /// The flush thread finished `done` (committed, refused or rejected): they leave the
    /// records in flight; waiters of the barrier wake. Once the dump segment is empty and
    /// nothing spills, the dump file is deleted and the spill segment ends (§7).
    pub(super) fn finish(&self, done: &[u64]) {
        let mut state = self.lock();
        state
            .in_flight
            .retain(|record| !done.contains(&record.capture_seq));
        let delete = state.dump.is_empty()
            && !state.spilling
            && !state.sealed
            && state.dump_file.is_some()
            && !state
                .in_flight
                .iter()
                .any(|record| matches!(record.record, InFlightRecord::InFile));
        if !delete {
            self.release(state);
            return;
        }
        let file = state.dump_file.take();
        state.dump_deleting = true;
        drop(state);
        let removed = file.as_ref().map(DumpFile::remove);
        let mut state = self.lock();
        state.dump_deleting = false;
        match removed {
            Some(Err(_)) => {
                // The file stays and the next spill appends to it; its committed records are
                // skipped by event id at an import.
                state.dump_file = file;
            }
            Some(Ok(())) | None => {
                if let Some(stretch) = state.spill_stretch.take() {
                    let outcome = Self::segment_finding(
                        LedgerCode::LedgerCaptureSpilled,
                        LedgerSegmentEdge::Ended,
                        stretch.count,
                        stretch.bytes,
                        Some(stretch.since),
                    );
                    self.push_finding(&mut state, outcome, Some(LedgerSegmentEdge::Ended));
                }
            }
        }
        self.release(state);
    }

    /// The barrier's wait (§8.4): until every record through `capture_seq` is finished; an
    /// error once the writer is sealed or ended first.
    pub(super) fn wait_done(&self, capture_seq: u64) -> Result<(), Outcome> {
        let mut state = self.lock();
        loop {
            if state.sealed {
                // The writer stopped by §9.2; the barrier made no attempt of its own.
                return Err(Outcome::new(LedgerCode::LedgerWriteFailed).attempts(0_u32));
            }
            if state.taken_through >= capture_seq && state.done_through() >= capture_seq {
                return Ok(());
            }
            if state.closed {
                return Err(Outcome::new(LedgerCode::WriterUnavailable));
            }
            state = self.wait(state);
        }
    }

    /// The writer drains for its close (§8.2): the segment edges of that drain are not reported
    /// after the close record.
    pub(super) fn begin_close(&self) {
        let mut state = self.lock();
        state.closing = true;
        self.release(state);
    }

    /// Test hook: waits up to `wait` for the close to begin ([`Self::begin_close`]); whether it
    /// has.
    #[cfg(test)]
    pub(super) fn wait_close_begun(&self, wait: Duration) -> bool {
        let state = self.lock();
        let (state, _) = self
            .changed
            .wait_timeout_while(state, wait, |state| !state.closing)
            .unwrap_or_else(PoisonError::into_inner);
        state.closing
    }

    /// The writer ended: waiters of the barrier wake.
    pub(super) fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
    }

    /// Stops the spill thread.
    fn stop_spill(&self) {
        self.lock().stop = true;
        self.changed.notify_all();
    }

    /// The memory segment's records and bytes a spill brings it down to: the tier-2 release
    /// line, or nothing at tier 3.
    fn spill_target(&self, state: &QueueState) -> Option<(u64, u64)> {
        match state.tier {
            WatermarkTier::Tier3 => Some((0, 0)),
            WatermarkTier::Tier2 => Some((
                self.record_lines.tier2_release,
                self.byte_lines.tier2_release,
            )),
            WatermarkTier::Below | WatermarkTier::Tier1 => None,
        }
    }

    /// The spill thread's next job.
    fn next_job(&self) -> SpillJob {
        let mut state = self.lock();
        loop {
            if state.stop {
                return SpillJob::Stop;
            }
            if state.sealed {
                state = self.wait(state);
                continue;
            }
            if state.hard_limit {
                state.hard_limit = false;
                return SpillJob::Seal(WriteStop::at_limit(self.current_attempt()));
            }
            if self.spills()
                && !state.spilling
                && !state.dump_deleting
                && let Some((records_target, bytes_target)) = self.spill_target(&state)
            {
                let mut entries = Vec::new();
                let mut batch_bytes = 0_u64;
                while (state.memory.len() as u64 > records_target
                    || state.memory_bytes > bytes_target)
                    && (entries.is_empty() || batch_bytes < self.policy.spill_batch_bytes)
                {
                    match state.memory.front() {
                        // A joint record is never dumped, so nothing behind it is spilled
                        // before it; a record being prepared is waited for.
                        Some(front)
                            if front.work.is_none() && !matches!(front.body, Body::Taken) => {}
                        _ => break,
                    }
                    let Some(entry) = state.memory.pop_front() else {
                        break;
                    };
                    Self::removed_from_memory(&mut state, &entry);
                    batch_bytes = batch_bytes.saturating_add(entry.bytes);
                    entries.push(entry);
                }
                if !entries.is_empty() {
                    state.spilling = true;
                    self.update_tier(&mut state);
                    self.release(state);
                    return SpillJob::Spill(entries);
                }
            }
            if state.unmeasured > 0 {
                let mut work = Vec::new();
                for entry in state.memory.iter_mut() {
                    if work.len() >= PREPARE_CHUNK {
                        break;
                    }
                    if entry.measured || matches!(entry.body, Body::Taken) {
                        continue;
                    }
                    let body = match &entry.body {
                        // A draft is measured through a shared reference; the flush thread may
                        // take it meanwhile.
                        Body::Draft(draft) => Body::Draft(Arc::clone(draft)),
                        _ => std::mem::replace(&mut entry.body, Body::Taken),
                    };
                    work.push((entry.capture_seq, entry.captured_at_unix_ms, body));
                }
                if !work.is_empty() {
                    state.taken += work
                        .iter()
                        .filter(|(_, _, body)| !matches!(body, Body::Draft(_)))
                        .count();
                    return SpillJob::Prepare(work);
                }
            }
            state = self.wait(state);
        }
    }

    /// The attempt in progress, for a stop that is not a write's own (§9.2 step 2).
    fn current_attempt(&self) -> u32 {
        self.failure
            .as_ref()
            .and_then(|failure| failure.attempt_in_progress())
            .unwrap_or(0)
    }

    /// The spill thread's preparation: assembles, sanitizes and measures off the lock.
    fn prepare(&self, work: Vec<(u64, u64, Body)>) {
        let done = work
            .into_iter()
            .map(|(capture_seq, captured_at, body)| {
                prepared(capture_seq, prepare_body(body, captured_at))
            })
            .collect::<Vec<_>>();
        let mut state = self.lock();
        self.put_prepared(&mut state, done);
        self.release(state);
    }

    /// Writes one spill to the dump file (§7): every record prepared and drafted, appended in
    /// capture order and synced. A failure, or a dump that would pass `dump_max_bytes`, stops
    /// the writer by §9.2 with these records.
    fn spill(&self, entries: Vec<Entry>) {
        let Some(failure) = self.failure.clone() else {
            return;
        };
        let mut lines = Vec::with_capacity(entries.len());
        let mut replies = Vec::with_capacity(entries.len());
        for entry in entries {
            let body = prepare_body(entry.body, entry.captured_at_unix_ms);
            if let Some(line) = dumped(
                &self.identifiers,
                entry.capture_seq,
                entry.captured_at_unix_ms,
                entry.source,
                body,
            ) {
                lines.push(line);
                replies.push((entry.capture_seq, entry.reply));
            } else if let Some(reply) = entry.reply {
                refuse_replies(vec![reply]);
            }
        }
        #[cfg(any(test, feature = "test-hooks"))]
        crate::write_fault::before_spill(failure.plan().dump_path());
        let file = self.lock().dump_file.take();
        let mut file = match file {
            Some(file) => Ok(file),
            None => failure.create_dump(),
        };
        let written = match &mut file {
            Ok(file) => file.append(&lines, failure.plan().policy().dump_max_bytes()),
            Err(failed) => Err(*failed),
        };
        let mut state = self.lock();
        state.spilling = false;
        if let Ok(file) = file {
            state.dump_file = Some(file);
        }
        match written {
            Ok(placed) => {
                let mut count = 0_u64;
                let mut bytes = 0_u64;
                for ((capture_seq, reply), (offset, len)) in replies.into_iter().zip(placed) {
                    state.dump.push_back(Spilled {
                        capture_seq,
                        offset,
                        len,
                        reply,
                    });
                    state.dump_bytes = state.dump_bytes.saturating_add(len);
                    count += 1;
                    bytes = bytes.saturating_add(len);
                }
                let started = state.spill_stretch.is_none();
                let stretch = state.spill_stretch.get_or_insert(Stretch {
                    since: Instant::now(),
                    count: 0,
                    bytes: 0,
                });
                stretch.count = stretch.count.saturating_add(count);
                stretch.bytes = stretch.bytes.saturating_add(bytes);
                if started {
                    let outcome = Self::segment_finding(
                        LedgerCode::LedgerCaptureSpilled,
                        LedgerSegmentEdge::Started,
                        count,
                        bytes,
                        None,
                    );
                    self.push_finding(&mut state, outcome, Some(LedgerSegmentEdge::Started));
                }
                // The flush thread may be waiting for this spill to land.
                state.wake_owed = true;
                self.release(state);
                #[cfg(any(test, feature = "test-hooks"))]
                crate::write_fault::after_spill(failure.plan().dump_path());
            }
            Err(failed) => {
                self.release(state);
                let stop = WriteStop::spill_failed(self.current_attempt(), failed);
                let pending = replies.into_iter().filter_map(|(_, reply)| reply).collect();
                match failure.seal() {
                    Some(mut held) => {
                        held.extend(lines);
                        self.seal_and_dump(held, pending, &stop, true);
                    }
                    None => refuse_replies(pending),
                }
            }
        }
    }

    /// §9.2 for a writer with a capture queue. The queue is sealed: no batch is taken any more
    /// and captures are still accepted. The dump file gets the records in flight, the memory
    /// segment and the ledger's own `runtime.failed`, after the spilled segment, and its
    /// trailer; every waiting caller is answered with `ledger_write_failed`; the plan's sink
    /// stops the Runtime. `held` are the records the writer and the spill thread held, `pending`
    /// replies already out of the queue. Only the thread that sealed the write-failure state
    /// calls it.
    pub(super) fn seal_and_dump(
        &self,
        held: Vec<HeldRecord>,
        pending: Vec<Reply>,
        stop: &WriteStop,
        from_spill: bool,
    ) {
        let Some(failure) = self.failure.clone() else {
            refuse_replies(pending);
            return;
        };
        let mut state = self.lock();
        state.sealed = true;
        self.changed.notify_all();
        while (state.spilling && !from_spill) || state.taken > 0 || state.dump_deleting {
            state = self.wait(state);
        }
        let memory = state.memory.drain(..).collect::<Vec<_>>();
        state.memory_bytes = 0;
        state.unmeasured = 0;
        let spilled = state.dump.drain(..).collect::<Vec<_>>();
        state.dump_bytes = 0;
        let in_flight = std::mem::take(&mut state.in_flight);
        let file = state.dump_file.take();
        let mut records = held;
        // The writer's own records in flight take capture numbers now, before the fatal record.
        for record in &mut records {
            if record.capture_seq().is_none() {
                record.number(state.next_seq);
                state.next_seq = state.next_seq.saturating_add(1);
            }
        }
        let fatal_seq = state.next_seq;
        state.next_seq = state.next_seq.saturating_add(1);
        drop(state);
        self.changed.notify_all();
        for record in in_flight {
            let body = match record.record {
                InFlightRecord::Draft(draft) => Body::Draft(draft),
                InFlightRecord::Rejected(event_id, code) => Body::Rejected { event_id, code },
                InFlightRecord::InFile | InFlightRecord::Joint => continue,
            };
            if let Some(line) = dumped(
                &self.identifiers,
                record.capture_seq,
                record.captured_at_unix_ms,
                record.source,
                body,
            ) {
                records.push(line);
            }
        }
        let mut replies = pending;
        for entry in memory {
            if let Some(reply) = entry.reply {
                replies.push(reply);
            }
            // A joint record is never dumped: its state work rolls back with it.
            if entry.work.is_some() {
                continue;
            }
            let body = prepare_body(entry.body, entry.captured_at_unix_ms);
            if let Some(line) = dumped(
                &self.identifiers,
                entry.capture_seq,
                entry.captured_at_unix_ms,
                entry.source,
                body,
            ) {
                records.push(line);
            }
        }
        replies.extend(spilled.into_iter().filter_map(|spilled| spilled.reply));
        failure.write_dump_and_stop(records, file, fatal_seq, stop);
        refuse_replies(replies);
        self.send_wake();
    }
}

/// Reads the dump segment's records back from the dump file (outside the lock).
fn read_spilled(path: Option<PathBuf>, refs: Vec<Spilled>) -> GlobalLedgerResult<Vec<Captured>> {
    let unreadable = || {
        GlobalLedgerError::fatal(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::LedgerReadSpilled.location(),
        )
    };
    let path = path.ok_or_else(unreadable)?;
    let mut batch = Vec::with_capacity(refs.len());
    for spilled in refs {
        let (capture_seq, draft) =
            write_failure::read_dump_line(&path, spilled.offset, spilled.len)?;
        if capture_seq != spilled.capture_seq {
            return Err(unreadable());
        }
        batch.push(Captured {
            capture_seq,
            captured_at_unix_ms: draft.timestamp_unix_ms(),
            source: draft.source().copied(),
            record: CapturedRecord::Dumped(Box::new(draft)),
            work: None,
            reply: spilled.reply,
        });
    }
    Ok(batch)
}

/// Answers every reply with the error a sealed writer gives.
pub(super) fn refuse_replies(replies: Vec<Reply>) {
    for mut reply in replies {
        reply.persisted(false);
        let _ = reply.send(Err(write_failure::sealed_error()));
    }
}

/// The spill thread (§7): prepares and measures records in the background, spills at tier 2
/// and above, and runs §9.2 when the memory segment reaches its hard limit.
pub(super) struct SpillThread {
    queue: Arc<CaptureQueue>,
    thread: Option<JoinHandle<()>>,
}

impl SpillThread {
    pub(super) fn start(queue: Arc<CaptureQueue>) -> GlobalLedgerResult<Self> {
        let spill_queue = Arc::clone(&queue);
        let thread = thread::Builder::new()
            .name("actingcommand-ledger-spill".to_string())
            .spawn(move || spill_loop(&spill_queue))
            .map_err(|error| {
                GlobalLedgerError::io(
                    LedgerCode::WriterSpawnFailed.code(),
                    LedgerLocation::LedgerSpillRecords.location(),
                    &error,
                )
            })?;
        Ok(Self {
            queue,
            thread: Some(thread),
        })
    }

    pub(super) fn finish(mut self) -> GlobalLedgerResult<()> {
        self.join()
    }

    fn join(&mut self) -> GlobalLedgerResult<()> {
        self.queue.stop_spill();
        match self.thread.take() {
            Some(thread) => thread.join().map_err(|_| {
                GlobalLedgerError::fatal(
                    LedgerCode::WriterPanicked.code(),
                    LedgerLocation::LedgerSpillRecords.location(),
                )
            }),
            None => Ok(()),
        }
    }
}

impl Drop for SpillThread {
    fn drop(&mut self) {
        let _ = self.join();
    }
}

fn spill_loop(queue: &CaptureQueue) {
    loop {
        match queue.next_job() {
            SpillJob::Stop => return,
            SpillJob::Prepare(work) => queue.prepare(work),
            SpillJob::Spill(entries) => queue.spill(entries),
            SpillJob::Seal(stop) => {
                if let Some(failure) = queue.failure.clone()
                    && let Some(held) = failure.seal()
                {
                    queue.seal_and_dump(held, Vec::new(), &stop, true);
                }
            }
        }
    }
}

/// The capture interface, the flush barrier and the tier read of one writer (§2.1). Its
/// captures are sanitized with its fingerprinter on the ledger's threads.
#[derive(Clone)]
pub struct LedgerCaptureHandle {
    queue: Arc<CaptureQueue>,
    fingerprinter: Arc<dyn SecretFingerprinter + Send + Sync>,
    reply_timeout: Duration,
}

impl std::fmt::Debug for LedgerCaptureHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LedgerCaptureHandle")
            .field("tier", &self.queue.tier())
            .finish()
    }
}

impl LedgerCaptureHandle {
    pub(super) fn new(
        queue: Arc<CaptureQueue>,
        fingerprinter: Arc<dyn SecretFingerprinter + Send + Sync>,
        reply_timeout: Duration,
    ) -> Self {
        Self {
            queue,
            fingerprinter,
            reply_timeout,
        }
    }

    /// Test hook: waits up to `wait` for the writer's close to begin (§8.2), from which the
    /// drain's segment edges are no longer reported; whether it has.
    #[cfg(test)]
    pub(crate) fn wait_close_begun(&self, wait: Duration) -> bool {
        self.queue.wait_close_begun(wait)
    }
}

impl LedgerCapture for LedgerCaptureHandle {
    type JointWork = Box<dyn LedgerTransactionWork>;

    fn capture(&self, item: CaptureItem) -> CaptureTicket {
        self.queue
            .capture_item(item, Arc::clone(&self.fingerprinter), None)
    }

    fn capture_joint(&self, item: CaptureItem, work: Self::JointWork) -> CaptureTicket {
        self.queue
            .capture_item(item, Arc::clone(&self.fingerprinter), Some(work))
    }
}

impl LedgerFlushBarrier for LedgerCaptureHandle {
    /// Waits until the record and every record captured before it are finished, then asks the
    /// writer for the record's ledger sequence; writes nothing (§8.4).
    fn wait_committed(&self, ticket: &CaptureTicket) -> Result<CommittedRecord, Outcome> {
        self.queue.wait_done(ticket.capture_seq)?;
        let sender = self
            .queue
            .sender()
            .ok_or_else(|| Outcome::new(LedgerCode::WriterUnavailable))?;
        let (response, receiver) = mpsc::sync_channel(1);
        sender
            .try_send(WriterCommand::CommittedSequence {
                event_id: ticket.event_id,
                response,
            })
            .map_err(|_| Outcome::new(LedgerCode::WriterUnavailable))?;
        match receiver.recv_timeout(self.reply_timeout) {
            Ok(Ok(Some(sequence))) => Ok(CommittedRecord {
                event_id: ticket.event_id,
                sequence,
            }),
            Ok(Ok(None)) => {
                let mut outcome = Outcome::new(LedgerCode::LedgerRecordRejected);
                if let Some(text) = super::index::id_text(&ticket.event_id) {
                    outcome = outcome.event_id(text);
                }
                Err(outcome)
            }
            Ok(Err(error)) => Err(Outcome::relayed(error.code())),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Err(Outcome::new(LedgerCode::WriterResponseTimeout))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(Outcome::new(LedgerCode::WriterUnavailable))
            }
        }
    }
}

impl LedgerBacklogTier for LedgerCaptureHandle {
    fn tier(&self) -> CaptureTier {
        self.queue.tier()
    }
}
