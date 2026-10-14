// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #383 AO-2 (`contracts/ledger-observer.md` §8): the flush thread's commit of the
//! records it took from the capture queue.
//!
//! The records are assembled into ledger records in capture order, each with its capture
//! (register L4), checked as an append is checked and staged in memory; a batch commits in one
//! SQLite transaction with its index rows (§11.4), FULL synced when it holds a decision record or
//! a tagged one, NORMAL otherwise. A tagged record ends its batch (rule 13). A joint record
//! commits alone with its state work. A capture that fails assembly, sanitizing or the checks is
//! not written: a single-record gap stands in its place (§10); a direct append's refusal goes
//! back to its caller as before. Per (source, lineage) the revisions are checked to be
//! contiguous (§8.3).

use super::capture::{Captured, CapturedRecord, Reply, rejected_gap};
use super::index::{IndexPolicy, IndexRows, is_tagged};
use super::storage::{
    BatchWrite, DurableStorage, EventStore, Revisions, increment_sequence, persist_batch_retried,
};
use super::{GlobalLedgerError, GlobalLedgerResult, LedgerTransactionWork};
use crate::PersistedEvent;
use crate::codes::{LedgerCode, LedgerLocation};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    EventId, EventType, IdentifierIssuer, RecordCapture, SanitizedEventDraft, SourceRevision,
};
use actingcommand_runtime_database::CommitSync;

/// What one flush of captured records did: the records committed (for the live subscribers),
/// the replies to send, the capture numbers finished, the revision findings (§8.3) and, when
/// the write gave up or met an integrity failure, the error that ends the writer.
#[derive(Default)]
pub(super) struct FlushReport {
    pub(super) committed: Vec<PersistedEvent>,
    pub(super) replies: Vec<(Reply, GlobalLedgerResult<PersistedEvent>)>,
    pub(super) done: Vec<u64>,
    pub(super) findings: Vec<Outcome>,
    pub(super) terminal: Option<GlobalLedgerError>,
}

impl FlushReport {
    fn answer(&mut self, reply: Option<Reply>, result: GlobalLedgerResult<PersistedEvent>) {
        if let Some(mut reply) = reply {
            reply.persisted(result.is_ok());
            self.replies.push((reply, result));
        }
    }

    fn stop(&mut self, error: GlobalLedgerError) {
        if self.terminal.is_none() {
            self.terminal = Some(error);
        }
    }
}

/// The sync level of a batch: FULL when it holds a decision record or a tagged one (§8.1).
pub(super) fn batch_sync(events: &[PersistedEvent], policy: &IndexPolicy) -> CommitSync {
    if events.iter().any(|event| {
        crate::durability::commit_sync(event.payload()) == CommitSync::Full
            || is_tagged(event, policy)
    }) {
        CommitSync::Full
    } else {
        CommitSync::Normal
    }
}

/// What a captured record was, for the gap that stands in its place when it is refused.
#[derive(Clone, Copy)]
struct Origin {
    capture_seq: u64,
    captured_at_unix_ms: u64,
    event_id: EventId,
    source: Option<SourceRevision>,
}

/// Why a capture was refused: the refusal's code, as the gap's cause.
fn cause(code: &'static str) -> Outcome {
    Outcome::relayed(code)
}

/// One staged record: where it came from and whether a caller waits for it.
struct Staged {
    origin: Origin,
    reply: Option<Reply>,
}

/// The records staged in memory since the last commit.
#[derive(Default)]
struct Pending {
    /// `events.len()`, the next sequence and the revisions before the first staged record.
    start: Option<(usize, u64, Revisions)>,
    staged: Vec<Staged>,
}

/// The joint record types (`append_transaction`, G15).
fn is_joint_type(event_type: EventType) -> bool {
    matches!(
        event_type,
        EventType::CatalogActivated
            | EventType::CatalogRolledBack
            | EventType::StateMigrated
            | EventType::ApprovalDecision
            | EventType::ReleaseStaged
            | EventType::ReleaseActivated
            | EventType::ReleaseRolledBack
            | EventType::PolicyPlanningSignalObserved
            | EventType::FactInvalidated
    )
}

fn flush_error(code: actingcommand_contract::outcome::Code) -> GlobalLedgerError {
    GlobalLedgerError::request(code, LedgerLocation::LedgerFlushCaptured.location())
}

impl<B: DurableStorage> EventStore<B> {
    /// Commits one batch the flush thread took, in capture order.
    pub(super) fn commit_captured(
        &mut self,
        batch: Vec<Captured>,
        identifiers: &IdentifierIssuer,
    ) -> FlushReport {
        let mut report = FlushReport::default();
        let mut pending = Pending::default();
        for captured in batch {
            if let Some(error) = report.terminal.clone() {
                // The writer stops: the record stays in flight for the dump.
                report.answer(captured.reply, Err(error));
                continue;
            }
            if captured.work.is_some() {
                self.commit_pending(&mut pending, &mut report, identifiers);
                match report.terminal.clone() {
                    None => self.commit_joint(captured, &mut report),
                    Some(error) => report.answer(captured.reply, Err(error)),
                }
                continue;
            }
            if self.stage_captured(captured, &mut pending, &mut report, identifiers) {
                // A tagged record commits on arrival: the batch ends at it (rule 13).
                self.commit_pending(&mut pending, &mut report, identifiers);
            }
        }
        self.commit_pending(&mut pending, &mut report, identifiers);
        report
    }

    /// The record of a captured draft at the next sequence, checked as an append is.
    fn build_draft(&self, draft: &SanitizedEventDraft) -> GlobalLedgerResult<PersistedEvent> {
        self.validate_append(draft)?;
        PersistedEvent::from_sanitized_ref(self.next_sequence, draft, *draft.event_id()).map_err(
            |error| {
                GlobalLedgerError::request(
                    error.code(),
                    LedgerLocation::ValidateSanitizedEvent.location(),
                )
            },
        )
    }

    /// Builds, checks and stages one captured record; whether it is tagged. A refused capture
    /// is staged as its gap; a refused direct append is answered; a refused record is never
    /// dumped.
    fn stage_captured(
        &mut self,
        captured: Captured,
        pending: &mut Pending,
        report: &mut FlushReport,
        identifiers: &IdentifierIssuer,
    ) -> bool {
        let Captured {
            capture_seq,
            captured_at_unix_ms,
            source,
            record,
            reply,
            ..
        } = captured;
        let (event_id, built) = match record {
            CapturedRecord::Draft(draft) => (*draft.event_id(), self.build_draft(&draft)),
            CapturedRecord::Dumped(draft) => (
                *draft.event_id(),
                (*draft)
                    .into_event(self.next_sequence)
                    .map_err(|error| flush_error(error.code())),
            ),
            CapturedRecord::Rejected(event_id, code) => {
                let origin = Origin {
                    capture_seq,
                    captured_at_unix_ms,
                    event_id,
                    source,
                };
                return self.stage_gap(origin, Outcome::new(code), pending, report, identifiers);
            }
        };
        let origin = Origin {
            capture_seq,
            captured_at_unix_ms,
            event_id,
            source,
        };
        let staged = built
            .and_then(|event| {
                let captured_at = event.timestamp_unix_ms();
                event
                    .with_capture(RecordCapture {
                        capture_seq,
                        captured_at_unix_ms: captured_at,
                        source,
                    })
                    .map_err(|error| flush_error(error.code()))
            })
            .and_then(|event| self.stage(event, pending));
        match staged {
            Ok(tagged) => {
                self.check_revision(source, report);
                pending.staged.push(Staged { origin, reply });
                tagged
            }
            Err(error) => {
                report.done.push(capture_seq);
                if error.terminal() {
                    // A refusal that ends the writer, as a direct append's did: what is staged
                    // commits first.
                    self.commit_pending(pending, report, identifiers);
                    report.answer(reply, Err(error.clone()));
                    report.stop(error);
                    return false;
                }
                match reply {
                    // A direct append's refusal goes back to its caller; nothing is written.
                    Some(reply) => {
                        report.answer(Some(reply), Err(error));
                        false
                    }
                    None => {
                        report.done.pop();
                        self.stage_gap(origin, cause(error.code()), pending, report, identifiers)
                    }
                }
            }
        }
    }

    /// Stages the single-record gap of a refused capture (§10) at its capture time and number.
    fn stage_gap(
        &mut self,
        origin: Origin,
        refusal: Outcome,
        pending: &mut Pending,
        report: &mut FlushReport,
        identifiers: &IdentifierIssuer,
    ) -> bool {
        let staged = rejected_gap(
            identifiers,
            origin.captured_at_unix_ms,
            origin.event_id,
            origin.source,
            refusal,
        )
        .map_err(|code| {
            GlobalLedgerError::fatal(code, LedgerLocation::LedgerFlushCaptured.location())
        })
        .and_then(|draft| {
            PersistedEvent::from_sanitized(self.next_sequence, draft).map_err(|error| {
                GlobalLedgerError::fatal(
                    error.code(),
                    LedgerLocation::LedgerFlushCaptured.location(),
                )
            })
        })
        .and_then(|event| {
            let captured_at = event.timestamp_unix_ms();
            event
                .with_capture(RecordCapture {
                    capture_seq: origin.capture_seq,
                    captured_at_unix_ms: captured_at,
                    source: None,
                })
                .map_err(|error| {
                    GlobalLedgerError::fatal(
                        error.code(),
                        LedgerLocation::LedgerFlushCaptured.location(),
                    )
                })
        })
        .and_then(|event| self.stage(event, pending));
        match staged {
            Ok(tagged) => {
                // The gap covers the refused revision (§8.3).
                self.cover_revision(origin.source);
                pending.staged.push(Staged {
                    origin,
                    reply: None,
                });
                tagged
            }
            Err(error) => {
                // Not even the gap can be written: the writer stops, and the dump keeps the
                // record in flight.
                self.commit_pending(pending, report, identifiers);
                report.stop(error);
                false
            }
        }
    }

    /// Checks one record as the store checks every append (event id, retention) and stages it
    /// in memory; whether it is tagged.
    fn stage(&mut self, event: PersistedEvent, pending: &mut Pending) -> GlobalLedgerResult<bool> {
        if self.indexes.contains_event_id(event.event_id()) {
            return Err(GlobalLedgerError::request(
                LedgerCode::DuplicateEventId.code(),
                LedgerLocation::AppendEvent.location(),
            ));
        }
        self.validate_retention_admission(&event, false)?;
        let following = increment_sequence(self.next_sequence)?;
        if pending.start.is_none() {
            pending.start = Some((
                self.events.len(),
                self.next_sequence,
                self.revisions.clone(),
            ));
        }
        let tagged = is_tagged(&event, &self.index_policy);
        self.retention.apply(&event);
        self.indexes.insert(&event, self.events.len());
        self.events.push(event);
        self.next_sequence = following;
        Ok(tagged)
    }

    /// §8.3: a record of a revisioned source follows the last revision of its lineage; a new
    /// lineage starts at 1. A hole not covered by a refused record's gap is
    /// `ledger_store_revision_skipped`.
    fn check_revision(&mut self, source: Option<SourceRevision>, report: &mut FlushReport) {
        let Some(source) = source else {
            return;
        };
        let last = self
            .revisions
            .get(&(source.source, source.lineage))
            .copied()
            .unwrap_or(0);
        let expected = last.saturating_add(1);
        if source.revision > expected {
            let mut outcome = Outcome::new(LedgerCode::LedgerStoreRevisionSkipped)
                .ledger_source(source.source)
                .expected_revision(expected)
                .source_revision(source.revision);
            if let Some(lineage) = super::index::id_text(&source.lineage) {
                outcome = outcome.source_lineage(lineage);
            }
            report.findings.push(outcome);
        }
        self.cover_revision(Some(source));
    }

    fn cover_revision(&mut self, source: Option<SourceRevision>) {
        if let Some(source) = source {
            let last = self
                .revisions
                .entry((source.source, source.lineage))
                .or_insert(0);
            *last = (*last).max(source.revision);
        }
    }

    /// Commits what is staged as one batch with its index rows (§8.1, §11.4). A refusal that
    /// does not end the writer rolls the memory back; its direct appends are answered and its
    /// captures are written as gaps.
    fn commit_pending(
        &mut self,
        pending: &mut Pending,
        report: &mut FlushReport,
        identifiers: &IdentifierIssuer,
    ) {
        let Some((start, start_sequence, revisions)) = pending.start.take() else {
            return;
        };
        let staged = std::mem::take(&mut pending.staged);
        let events = &self.events[start..];
        let rows = IndexRows::of(events, &self.index_policy);
        let sync = batch_sync(events, &self.index_policy);
        let written = persist_batch_retried(
            &mut self.backend,
            self.write_failure.as_ref(),
            &BatchWrite {
                events,
                rows: &rows,
                work: None,
                sync,
            },
            Vec::new(),
        );
        match written {
            Ok(write_sync_ns) => {
                let committed = self.events[start..].to_vec();
                let last = committed.len().saturating_sub(1);
                for (at, event) in committed.iter().enumerate() {
                    // The batch's sync time is its last record's; a batch read back as
                    // committed has none, as a single record's had.
                    let elapsed = write_sync_ns.map(|ns| if at == last { ns } else { 0 });
                    if let Err(error) = self
                        .commit_statistics
                        .record_success(event.sequence(), elapsed)
                    {
                        report.stop(error);
                    }
                }
                for (staged, event) in staged.into_iter().zip(committed.iter()) {
                    report.done.push(staged.origin.capture_seq);
                    report.answer(staged.reply, Ok(event.clone()));
                }
                report.committed.extend(committed);
            }
            Err(error) => {
                self.roll_back(start, start_sequence, revisions);
                let terminal = error.terminal();
                let mut refused = Vec::new();
                for staged in staged {
                    match staged.reply {
                        Some(reply) => {
                            if !terminal {
                                report.done.push(staged.origin.capture_seq);
                            }
                            report.answer(Some(reply), Err(error.clone()));
                        }
                        None if !terminal => refused.push(staged.origin),
                        None => {}
                    }
                }
                if terminal {
                    report.stop(error);
                    return;
                }
                // A refusal of the batch that does not end the writer: each capture is written
                // as its gap.
                for origin in refused {
                    self.stage_gap(origin, cause(error.code()), pending, report, identifiers);
                }
                if pending.start.is_some() {
                    self.commit_pending(pending, report, identifiers);
                }
            }
        }
    }

    /// Rolls the memory back to the last commit after a refused batch: the records, the
    /// indexes, the retention state and the revisions.
    fn roll_back(&mut self, start: usize, start_sequence: u64, revisions: Revisions) {
        self.events.truncate(start);
        self.next_sequence = start_sequence;
        self.indexes = super::projection::EventIndexes::from_events(&self.events);
        if let Ok(retention) = super::retention::RetentionIndex::from_events(&self.events) {
            self.retention = retention;
        }
        self.revisions = revisions;
    }

    /// Commits one joint record alone with its state work (G15).
    fn commit_joint(&mut self, captured: Captured, report: &mut FlushReport) {
        let Captured {
            capture_seq,
            source,
            record,
            work,
            reply,
            ..
        } = captured;
        let Some(work) = work else {
            return;
        };
        let result = self.write_joint(capture_seq, source, record, work.as_ref());
        match &result {
            Ok(event) => {
                report.done.push(capture_seq);
                report.committed.push(event.clone());
            }
            Err(error) if error.terminal() => report.stop(error.clone()),
            Err(_) => report.done.push(capture_seq),
        }
        report.answer(reply, result);
    }

    fn write_joint(
        &mut self,
        capture_seq: u64,
        source: Option<SourceRevision>,
        record: CapturedRecord,
        work: &dyn LedgerTransactionWork,
    ) -> GlobalLedgerResult<PersistedEvent> {
        let unsupported = || {
            GlobalLedgerError::request(
                LedgerCode::JointEventTypeUnsupported.code(),
                LedgerLocation::AppendTransaction.location(),
            )
        };
        let CapturedRecord::Draft(draft) = record else {
            return Err(unsupported());
        };
        if !is_joint_type(draft.event_type()) {
            return Err(unsupported());
        }
        let event =
            PersistedEvent::from_sanitized_ref(self.next_sequence, &draft, *draft.event_id())
                .and_then(|event| {
                    let captured_at = event.timestamp_unix_ms();
                    event.with_capture(RecordCapture {
                        capture_seq,
                        captured_at_unix_ms: captured_at,
                        source,
                    })
                })
                .map_err(|error| {
                    GlobalLedgerError::request(
                        error.code(),
                        LedgerLocation::ValidateSanitizedEvent.location(),
                    )
                })?;
        let following = increment_sequence(self.next_sequence)?;
        if self.indexes.contains_event_id(event.event_id()) {
            return Err(GlobalLedgerError::request(
                LedgerCode::DuplicateEventId.code(),
                LedgerLocation::AppendTransaction.location(),
            ));
        }
        self.validate_retention_admission(&event, false)?;
        let write_sync_ns = self.persist_retried(&event, Some(work))?;
        self.publish_committed(event, following, write_sync_ns)
    }

    /// The ledger sequence of a committed record, for the flush barrier (§8.4).
    pub(super) fn committed_sequence(&self, event_id: &EventId) -> Option<u64> {
        self.indexes
            .position_of(event_id)
            .and_then(|position| self.events.get(position))
            .map(PersistedEvent::sequence)
    }
}
