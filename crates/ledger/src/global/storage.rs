// SPDX-License-Identifier: AGPL-3.0-only

#[cfg(test)]
use super::GlobalLedgerConfig;
use super::index::{IndexPolicy, IndexRows};
use super::{
    CommitStatistics, GlobalLedgerError, GlobalLedgerResult, LedgerAppendObservation,
    LedgerProjectViewObservation, Sha256SecretFingerprinter, is_identifier,
    projection::EventIndexes,
};
use crate::PersistedEvent;
use crate::codes::{LedgerCode, LedgerLocation};
#[cfg(test)]
use crate::fact::LedgerEventRead;
use crate::fact::StoredEventRecord;
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::{
    AuditInput, EffectDisposition, EventAction, EventActor, EventDraft, EventId, EventLinks,
    EventLinksDraft, EventOrigin, EventPayload, EventPayloadDraft, EventSeverity, EventSource,
    EventType, IdentifierIssuer, IssuedActionId, LeasePayload, LedgerPayload, LedgerPayloadDraft,
    LedgerSource, OriginModule, OwnerEpoch, PolicyDispatchEventData, PolicyDispatchPayload,
    PolicyExecutionEventData, PolicyExecutionOutcome, PolicyFailureClass, PolicyFailureDisposition,
    PolicyPayload, PolicyPayloadDraft, RecoveryReason, SanitizedEventDraft,
    ScheduledPolicyRecoveryContinuation, Sensitivity, TaskOutcome, TaskPayload, TaskSemanticFact,
    record_severity, scheduled_failure_class, scheduled_failure_code_str,
};
#[cfg(test)]
use actingcommand_contract::{
    GLOBAL_EVENT_SCHEMA_VERSION, ProjectedArtifactReference, VerifiedArtifactReference,
};
use actingcommand_runtime_database::CommitSync;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const WRITER_SCHEMA_VERSION: &str = "actingcommand.ledger-writer.v2";
const REPAIR_SCHEMA_VERSION: &str = "actingcommand.ledger-repair.v1";
const REPAIR_JOURNAL_FILE: &str = "repair-journal.jsonl";
pub(super) const LINE_TYPE: &str = "event";

#[cfg(test)]
type ArtifactVerifier<'a> =
    dyn FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference> + 'a;

// Specification criterion 2: https://github.com/HS7097/ActingCommand-Workflow/issues/257#issuecomment-5552006104
#[cfg(test)]
#[test]
fn b3_commit_statistics_follow_successful_write_sync_and_preserve_failure() {
    use super::GlobalLedgerCommitObservation;
    let temp = tempfile::tempdir().expect("ledger root");
    let mut store = SegmentStore::open(GlobalLedgerConfig::new(temp.path(), "neutral-writer"))
        .expect("open store");
    let ids = IdentifierIssuer::new().expect("identifiers");
    let draft = |timestamp| {
        EventDraft::new(
            ids.mint_event_id().expect("event id"),
            timestamp,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            EventLinksDraft::default(),
            actingcommand_contract::CommandPayloadDraft::received(
                EventAction::RuntimeStart,
                AuditInput::new(),
            )
            .into(),
        )
        .sanitize(&Sha256SecretFingerprinter::new(b"neutral-statistics").expect("fingerprinter"))
        .expect("draft")
    };
    let GlobalLedgerCommitObservation::Available(initial) =
        store.commit_statistics.sample().expect("initial")
    else {
        panic!("available");
    };
    assert_eq!(
        (
            initial.successful_commits,
            initial.through_sequence,
            initial.write_sync_total_ns
        ),
        (0, 0, 0)
    );
    let first = store.append(draft(9_000_000)).expect("first durable event");
    let second = store
        .append(draft(1_000))
        .expect("draft time is not commit time");
    let GlobalLedgerCommitObservation::Available(committed) =
        store.commit_statistics.sample().expect("committed")
    else {
        panic!("available");
    };
    assert_eq!(committed.writer_id, initial.writer_id);
    assert_eq!(committed.successful_commits, 2);
    assert_eq!(committed.through_sequence, second.sequence());
    assert_eq!(second.sequence(), first.sequence() + 1);
    assert!(committed.sampled_at_monotonic_ns >= initial.sampled_at_monotonic_ns);
    assert!(committed.write_sync_max_ns <= committed.write_sync_total_ns);
    let read_only = File::open(segment_path(
        &store.backend.segments_dir,
        store.backend.active_index,
    ))
    .expect("read-only handle");
    let writable = std::mem::replace(&mut store.backend.active_file, read_only);
    let error = store.append(draft(42_000)).expect_err("write failure");
    assert!(error.is_fatal());
    assert_eq!(
        (error.code(), error.operation()),
        ("ledger_io", "append_event")
    );
    let GlobalLedgerCommitObservation::Available(failed) =
        store.commit_statistics.sample().expect("failure snapshot")
    else {
        panic!("available");
    };
    assert_eq!(failed.successful_commits, committed.successful_commits);
    assert_eq!(failed.through_sequence, committed.through_sequence);
    assert_eq!(failed.write_sync_total_ns, committed.write_sync_total_ns);
    store.backend.active_file = writable;
    {
        let held = store
            .commit_statistics
            .totals
            .lock()
            .expect("hold only statistics");
        assert_eq!(
            store.commit_statistics.sample().expect("nonblocking"),
            GlobalLedgerCommitObservation::Unavailable(
                actingcommand_contract::PerformanceLedgerUnavailable::Busy
            )
        );
        drop(held);
    }
    store
        .commit_statistics
        .totals
        .lock()
        .expect("totals")
        .as_mut()
        .expect("available")
        .successful_commits = u64::MAX;
    store
        .commit_statistics
        .record_success(second.sequence() + 1, Some(1))
        .expect("overflow recorded as unavailable");
    assert_eq!(
        store.commit_statistics.sample().expect("overflow"),
        GlobalLedgerCommitObservation::Unavailable(
            actingcommand_contract::PerformanceLedgerUnavailable::CounterOverflow
        )
    );
    store.close().expect("close");
}

pub(super) trait DurableStorage: Send + 'static {
    fn material_root(&self) -> &Path;

    fn resolve_artifact(
        &self,
        _selection: &super::LedgerArtifactSelection,
        _deadline: Instant,
    ) -> Option<GlobalLedgerResult<super::ResolvedLedgerArtifact>> {
        None
    }

    fn project_view_page(
        &self,
        _query: &actingcommand_contract::EventQuery,
        _profile: actingcommand_contract::ProjectionProfile,
        _request: &actingcommand_contract::RuntimeEventQueryPageRequest,
        _observation: &mut Option<LedgerProjectViewObservation>,
    ) -> Option<GlobalLedgerResult<actingcommand_contract::RuntimeEventQueryPage>> {
        None
    }

    fn persist(&mut self, event: &PersistedEvent) -> GlobalLedgerResult<Option<u64>>;
    /// Workflow #383 AO-2 (`contracts/ledger-observer.md` §8.1, §11.4): commits one batch, its
    /// index rows and a joint record's state work in one transaction. A store without
    /// transactions writes the records one by one and keeps no index; it has no joint records.
    fn persist_batch(&mut self, write: &BatchWrite<'_>) -> GlobalLedgerResult<Option<u64>> {
        if write.work.is_some() {
            return Err(GlobalLedgerError::request(
                LedgerCode::LedgerJointTransactionUnsupported.code(),
                LedgerLocation::AppendTransaction.location(),
            ));
        }
        let mut elapsed = None;
        for event in write.events {
            elapsed = self.persist(event)?;
        }
        Ok(elapsed)
    }
    /// Workflow #383 AO-1 (`contracts/ledger-observer.md` §9.1): after a failed write of a
    /// batch, whether the store holds it after all; a store that cannot tell refuses, and the
    /// write is not tried again.
    fn read_back_batch(&mut self, _events: &[PersistedEvent]) -> GlobalLedgerResult<ReadBack> {
        Err(GlobalLedgerError::fatal(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::ReadBackLedgerWrite.location(),
        ))
    }
    fn close(&mut self) -> GlobalLedgerResult<()>;
}

/// Workflow #383 AO-2: one batch to commit: consecutive records from the store's next sequence,
/// the index rows they change, a joint record's state work and the batch's sync level.
pub(super) struct BatchWrite<'a> {
    pub(super) events: &'a [PersistedEvent],
    pub(super) rows: &'a IndexRows,
    pub(super) work: Option<&'a dyn super::LedgerTransactionWork>,
    pub(super) sync: CommitSync,
}

/// Workflow #383 AO-1, AO-2 (`contracts/ledger-observer.md` §9.1): one batch's write. Without a
/// write-failure state it is one attempt. With one, a write failure is read back and, unless the
/// batch turns out committed, tried again with its original event ids, by the configured
/// attempts and waits. When they are used up, or the store reports an integrity or format
/// failure, the write gives up: the records stay in flight for the dump and the writer stops
/// (§9.2). `held` are the records the writer itself holds for the dump while it writes (the
/// capture queue holds its own batches); a joint record is never dumped, its state work rolls
/// back with it.
pub(super) fn persist_batch_retried<B: DurableStorage>(
    backend: &mut B,
    failure: Option<&Arc<super::write_failure::WriteFailureState>>,
    write: &BatchWrite<'_>,
    held: Vec<super::write_failure::HeldRecord>,
) -> GlobalLedgerResult<Option<u64>> {
    let Some(state) = failure else {
        return backend.persist_batch(write);
    };
    state.hold(held)?;
    let attempts = state.plan().policy().write_attempts();
    let started = Instant::now();
    let mut last_failure: Option<GlobalLedgerError> = None;
    let mut attempt = 0_u32;
    let result = loop {
        attempt += 1;
        state.wait_before(attempt);
        if last_failure.is_some() {
            match backend.read_back_batch(write.events) {
                Ok(ReadBack::Committed) => break Ok(None),
                Ok(ReadBack::Absent) => {}
                Err(error) if error.is_write_failure() && attempt < attempts => {
                    last_failure = Some(error);
                    continue;
                }
                Err(error) => break Err(state.give_up(attempt, error)),
            }
        }
        state.begin_attempt(attempt)?;
        let written = backend.persist_batch(write);
        if state.end_attempt() {
            // The probe found this attempt stalled and dumped it (T8).
            return Err(super::write_failure::sealed_error());
        }
        match written {
            Ok(write_sync_ns) => break Ok(write_sync_ns),
            Err(error) if error.is_write_failure() && attempt < attempts => {
                last_failure = Some(error);
            }
            Err(error) if error.is_write_failure() || error.terminal() => {
                break Err(state.give_up(attempt, error));
            }
            Err(error) => {
                state.release();
                return Err(error);
            }
        }
    };
    if result.is_ok() {
        state.release();
        if let Some(failure) = &last_failure {
            state.report_retried(
                attempt.saturating_sub(1),
                failure.write_failure_kind(),
                started.elapsed(),
            );
        }
    }
    result
}

/// Workflow #383 AO-1: what reading the store back after a failed write found (§9.1).
pub(super) enum ReadBack {
    /// The record is committed: the store's head moved to it.
    Committed,
    /// The store's head is where the write left it: the record is not there.
    Absent,
}

/// Shared typed semantics and committed snapshot, owned by the existing writer.
pub(super) struct EventStore<B> {
    pub(super) commit_statistics: Arc<CommitStatistics>,
    pub(super) backend: B,
    pub(super) next_sequence: u64,
    pub(super) events: Vec<PersistedEvent>,
    pub(super) indexes: EventIndexes,
    pub(super) retention: super::retention::RetentionIndex,
    pub(super) recovering_retention: bool,
    /// Workflow #383 AO-1: the running Runtime's write-failure state; `None` for an offline
    /// writer, whose failed write is not tried again (`contracts/ledger-observer.md` §6).
    pub(super) write_failure: Option<Arc<super::write_failure::WriteFailureState>>,
    /// Workflow #383 AO-2: what the index table keeps (`ledger_observer.tag.workbench_patterns`,
    /// `request_index_records`).
    pub(super) index_policy: IndexPolicy,
    /// Workflow #383 AO-2 (§8.3): the last revision committed per (source, lineage).
    pub(super) revisions: Revisions,
}

/// Workflow #383 AO-2 (§8.3): the last revision committed per (source, lineage).
pub(super) type Revisions = BTreeMap<(LedgerSource, OwnerEpoch), u64>;

#[cfg(test)]
pub(super) type SegmentStore = EventStore<SegmentStorage>;

#[cfg(test)]
pub(super) struct SegmentStorage {
    root: PathBuf,
    segments_dir: PathBuf,
    ownership: WriterOwnership,
    segment_max_bytes: u64,
    active_index: u64,
    active_bytes: u64,
    active_file: File,
}

#[cfg(test)]
impl SegmentStore {
    pub(super) fn open(config: GlobalLedgerConfig) -> GlobalLedgerResult<Self> {
        Self::open_inner(config, None)
    }

    pub(super) fn open_with_artifact_verifier<F>(
        config: GlobalLedgerConfig,
        mut verifier: F,
    ) -> GlobalLedgerResult<Self>
    where
        F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
    {
        Self::open_inner(config, Some(&mut verifier))
    }

    fn open_inner(
        config: GlobalLedgerConfig,
        mut verifier: Option<&mut ArtifactVerifier<'_>>,
    ) -> GlobalLedgerResult<Self> {
        fs::create_dir_all(&config.root).map_err(|error| {
            GlobalLedgerError::io_untyped("ledger_io", "create_ledger_root", &error)
        })?;
        let segments_dir = config.root.join("segments");
        fs::create_dir_all(&segments_dir).map_err(|error| {
            GlobalLedgerError::io_untyped("ledger_io", "create_segments", &error)
        })?;
        let (mut ownership, stale_owner) =
            WriterOwnership::acquire(&config.root, &config.owner_id)?;
        let recovery = match recover_segments(&config.root, &segments_dir, &mut verifier) {
            Ok(recovery) => recovery,
            Err(error) => {
                let _ = ownership.close();
                return Err(error);
            }
        };
        let RecoveryState {
            next_sequence,
            events,
            last_segment_index,
            pending_repairs,
        } = recovery;
        let active_index = last_segment_index.max(1);
        let active_path = segment_path(&segments_dir, active_index);
        let active_file = match OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&active_path)
        {
            Ok(file) => file,
            Err(error) => {
                let _ = ownership.close();
                return Err(GlobalLedgerError::io_untyped(
                    "ledger_io",
                    "open_active_segment",
                    &error,
                ));
            }
        };
        let active_bytes = active_file
            .metadata()
            .map_err(|error| GlobalLedgerError::io_untyped("ledger_io", "stat_segment", &error))?
            .len();
        let commit_statistics =
            match CommitStatistics::new(events.last().map_or(0, PersistedEvent::sequence)) {
                Ok(statistics) => Arc::new(statistics),
                Err(error) => {
                    ownership.close()?;
                    return Err(error);
                }
            };
        let retention = super::retention::RetentionIndex::from_events(&events)?;
        let recovering_retention = retention.has_pending();
        let mut store = Self {
            commit_statistics,
            backend: SegmentStorage {
                root: config.root.canonicalize().map_err(|error| {
                    GlobalLedgerError::io_untyped("ledger_io", "canonicalize_ledger_root", &error)
                })?,
                segments_dir,
                ownership,
                segment_max_bytes: config.segment_max_bytes,
                active_index,
                active_bytes,
                active_file,
            },
            next_sequence,
            indexes: EventIndexes::from_events(&events),
            retention,
            recovering_retention,
            events,
            write_failure: None,
            index_policy: config.capture_policy.index_policy(),
            revisions: BTreeMap::new(),
        };
        let recovery_result = (|| {
            if let Some(owner_id) = stale_owner {
                store.append_recovery(RecoveryReason::StaleOwner, Some(owner_id), None, None)?;
            }
            for repair in pending_repairs {
                let event_id = repair.event_id()?;
                let event = store.append_recovery(
                    RecoveryReason::TruncatedFinalTail,
                    None,
                    Some((repair.segment_index, repair.affected_bytes())),
                    Some(event_id),
                )?;
                verify_recovery_event(&event, &repair)?;
                repair_test_barrier("after_recovery_append")?;
                append_repair_record(&store.backend.root, &repair.completed())?;
                repair_test_barrier("after_completion")?;
            }
            Ok(())
        })();
        if let Err(error) = recovery_result {
            store.backend.ownership.close()?;
            return Err(error);
        }
        Ok(store)
    }
}

impl<B: DurableStorage> EventStore<B> {
    pub(super) fn recovered(
        mut backend: B,
        next_sequence: u64,
        events: Vec<PersistedEvent>,
    ) -> GlobalLedgerResult<Self> {
        let commit_statistics =
            match CommitStatistics::new(events.last().map_or(0, PersistedEvent::sequence)) {
                Ok(statistics) => Arc::new(statistics),
                Err(error) => {
                    backend.close()?;
                    return Err(error);
                }
            };
        let retention = super::retention::RetentionIndex::from_events(&events)?;
        let recovering_retention = retention.has_pending();
        Ok(Self {
            commit_statistics,
            backend,
            next_sequence,
            indexes: EventIndexes::from_events(&events),
            retention,
            recovering_retention,
            events,
            write_failure: None,
            index_policy: IndexPolicy::default(),
            revisions: BTreeMap::new(),
        })
    }

    /// Workflow #383 AO-1, AO-2: the running Runtime's writer: the index policy, the
    /// write-failure state and the dump import (§9.4); when the previous owner did not close
    /// (`stale_owner`) and no complete dump ended its epoch, the tail gap (§10). An offline
    /// writer (no failure plan) imports nothing and writes no gap.
    pub(super) fn open_running(
        &mut self,
        config: &super::GlobalLedgerConfig,
        stale_owner: bool,
    ) -> GlobalLedgerResult<()> {
        let complete = self.attach_write_failure(config)?;
        if stale_owner && !complete && self.write_failure.is_some() {
            super::write_failure::append_tail_gap(self)?;
        }
        Ok(())
    }

    /// Workflow #383 AO-1, AO-2: the running Runtime's write-failure state and index policy,
    /// then the import of every dump an earlier run left (§9.4), before any other new record;
    /// whether an imported dump ended its epoch complete (sealed) is returned for the tail gap
    /// (§10).
    pub(super) fn attach_write_failure(
        &mut self,
        config: &super::GlobalLedgerConfig,
    ) -> GlobalLedgerResult<bool> {
        self.index_policy = config.capture_policy.index_policy();
        self.write_failure = config.write_failure.clone();
        match self.write_failure.clone() {
            Some(state) => super::write_failure::import_dumps(self, &state),
            None => Ok(false),
        }
    }

    /// Workflow #383 AO-1, AO-2 (`contracts/ledger-observer.md` §9.1, §11.4): one record the
    /// writer writes itself (a dump import, a recovery, retention, a settlement continuation),
    /// with its index rows, by [`persist_batch_retried`]. A plain record is held for the dump
    /// while it is written; a joint record never is.
    pub(super) fn persist_retried(
        &mut self,
        event: &PersistedEvent,
        work: Option<&dyn super::LedgerTransactionWork>,
    ) -> GlobalLedgerResult<Option<u64>> {
        let events = std::slice::from_ref(event);
        let rows = IndexRows::of(events, &self.index_policy);
        let sync = match work {
            Some(_) => CommitSync::Full,
            None => super::flush::batch_sync(events, &self.index_policy),
        };
        let held = match work {
            Some(_) => Vec::new(),
            None => vec![super::write_failure::HeldRecord::own(
                crate::fact::DumpedDraft::from_event(event),
            )],
        };
        persist_batch_retried(
            &mut self.backend,
            self.write_failure.as_ref(),
            &BatchWrite {
                events,
                rows: &rows,
                work,
                sync,
            },
            held,
        )
    }

    /// Workflow #383 AO-1 (§9.4): appends one record of a dump import, or the import's own
    /// record. An imported record precedes every record of this start, so a retention recovery
    /// still pending does not hold it back.
    pub(super) fn persist_imported(
        &mut self,
        event: PersistedEvent,
    ) -> GlobalLedgerResult<PersistedEvent> {
        let following_sequence = increment_sequence(self.next_sequence)?;
        if self.indexes.contains_event_id(event.event_id()) {
            return Err(GlobalLedgerError::request(
                LedgerCode::DuplicateEventId.code(),
                LedgerLocation::ImportLedgerDump.location(),
            ));
        }
        self.retention
            .validate(&event, &self.events, &self.indexes, false, true)?;
        let write_sync_ns = self.persist_retried(&event, None)?;
        self.publish_committed(event, following_sequence, write_sync_ns)
    }

    #[cfg(test)]
    pub(super) fn append(
        &mut self,
        draft: SanitizedEventDraft,
    ) -> GlobalLedgerResult<PersistedEvent> {
        self.validate_append(&draft)?;
        self.append_with_event_id(draft, None, &mut None)
    }

    /// The checks a sanitized draft of a caller meets before it is written (`append`,
    /// `append_deferred`): no new record while a retention recovery is pending, a migration
    /// record only through the import's transaction, a scheduled settlement only through its
    /// continuation.
    pub(super) fn validate_append(&self, draft: &SanitizedEventDraft) -> GlobalLedgerResult<()> {
        if self.recovering_retention && draft.event_type() != EventType::LedgerRecovered {
            return Err(GlobalLedgerError::request(
                LedgerCode::ArtifactRetentionRecoveryPending.code(),
                LedgerLocation::AppendEvent.location(),
            ));
        }
        if matches!(draft.payload(), EventPayload::Ledger(actingcommand_contract::LedgerPayload::Recovered(payload)) if payload.migration().is_some())
        {
            return Err(GlobalLedgerError::request(
                LedgerCode::MigrationRequiresImportTransaction.code(),
                LedgerLocation::AppendEvent.location(),
            ));
        }
        if is_unlinked_scheduled_settlement_draft(draft) {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::ScheduledSettlementContinuationRequired.code(),
                LedgerLocation::AppendEvent.location(),
            ));
        }
        Ok(())
    }

    /// The only ledger-owned continuation for a persisted scheduled policy settlement.
    ///
    /// Callers provide policy-owned outcome semantics, never links, identifiers, release reasons,
    /// or an event draft. This method revalidates the persisted source chain and can append at
    /// most one execution and one completion for that decision.
    pub(super) fn reconcile_scheduled_policy_settlement(
        &mut self,
        execution: PolicyExecutionEventData,
    ) -> GlobalLedgerResult<(PersistedEvent, Vec<PersistedEvent>)> {
        #[cfg(any(test, feature = "test-hooks"))]
        if super::take_settlement_refusal_for_test(&execution.decision_id) {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledExecutionRecoveryRefusedForTest.code(),
                LedgerLocation::ReconcileScheduledPolicySettlement.location(),
            ));
        }
        let first_new_sequence = self.next_sequence;
        let execution_draft = scheduled_policy_execution_recovery_draft(&execution)?;
        self.append_recovered_policy_execution(execution_draft)?;
        let completion_draft =
            self.scheduled_policy_completion_recovery_draft(&execution.decision_id)?;
        let completion = self.append_recovered_policy_completion(completion_draft)?;
        let appended = self
            .events
            .iter()
            .filter(|event| event.sequence() >= first_new_sequence)
            .cloned()
            .collect();
        Ok((completion, appended))
    }

    fn scheduled_policy_completion_recovery_draft(
        &self,
        decision_id: &str,
    ) -> GlobalLedgerResult<SanitizedEventDraft> {
        let intents = self
            .events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Policy(PolicyPayload::DispatchIntent(payload))
                    if payload.decision_id() == decision_id =>
                {
                    Some(payload)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let [intent] = intents.as_slice() else {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledRecoveryIntentNotUnique.code(),
                LedgerLocation::BuildPolicyCompletionRecovery.location(),
            ));
        };
        let admissions = self
            .events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload))
                    if payload.decision_id() == decision_id =>
                {
                    Some(payload)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let [admission] = admissions.as_slice() else {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledRecoveryAdmissionNotUnique.code(),
                LedgerLocation::BuildPolicyCompletionRecovery.location(),
            ));
        };
        let Some(admission) = admission.admission() else {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledRecoveryPayloadConflict.code(),
                LedgerLocation::BuildPolicyCompletionRecovery.location(),
            ));
        };
        let executions = self
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload))
                        if payload.decision_id() == decision_id
                )
            })
            .collect::<Vec<_>>();
        let [execution] = executions.as_slice() else {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledRecoveryExecutionNotUnique.code(),
                LedgerLocation::BuildPolicyCompletionRecovery.location(),
            ));
        };
        scheduled_policy_recovery_draft(
            execution.timestamp_unix_ms(),
            EventSeverity::Info,
            PolicyPayloadDraft::dispatch_completed(
                dispatch_event_data(intent),
                admission.clone(),
                AuditInput::new(),
            )
            .into(),
        )
    }

    fn append_with_event_id(
        &mut self,
        draft: SanitizedEventDraft,
        event_id: Option<EventId>,
        observation: &mut Option<LedgerAppendObservation>,
    ) -> GlobalLedgerResult<PersistedEvent> {
        let event = match event_id {
            Some(event_id) => {
                PersistedEvent::from_sanitized_with_event_id(self.next_sequence, draft, event_id)
            }
            None => PersistedEvent::from_sanitized(self.next_sequence, draft),
        }
        .map_err(|error| {
            GlobalLedgerError::request(
                error.code(),
                LedgerLocation::ValidateSanitizedEvent.location(),
            )
        })?;
        self.persist_event(event, observation)
    }

    fn append_with_scheduled_recovery_continuation(
        &mut self,
        draft: SanitizedEventDraft,
        continuation: ScheduledPolicyRecoveryContinuation,
    ) -> GlobalLedgerResult<PersistedEvent> {
        let draft = continuation.apply_to(draft).map_err(|error| {
            GlobalLedgerError::request(
                error.code(),
                LedgerLocation::ValidateRecoveredScheduledEvent.location(),
            )
        })?;
        self.append_with_event_id(draft, None, &mut None)
    }

    fn append_recovered_policy_completion(
        &mut self,
        draft: SanitizedEventDraft,
    ) -> GlobalLedgerResult<PersistedEvent> {
        // Ledger revision 3 (Workflow #378 A3): a record's severity is its category (model
        // section 8.1), so the completion is checked against the rule, not a fixed value.
        if draft.event_type() != EventType::PolicyDispatchCompleted
            || draft.severity() != record_severity(EventSeverity::Info, draft.payload())
            || draft.origin().source() != EventSource::Scheduler
            || draft.origin().module() != OriginModule::Policy
            || draft.origin().actor() != EventActor::Scheduler
            || !draft.artifacts().is_empty()
            || !event_links_empty(draft.links())
        {
            return Err(GlobalLedgerError::settlement_refusal(
                SharedCode::ScheduledRecoveryDraftInvalid.code(),
                LedgerLocation::RecoverPolicyCompletion.location(),
            ));
        }
        let EventPayload::Policy(PolicyPayload::DispatchCompleted(completion)) = draft.payload()
        else {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledRecoveryDraftTypeInvalid.code(),
                LedgerLocation::RecoverPolicyCompletion.location(),
            ));
        };

        let intents = self
            .events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Policy(PolicyPayload::DispatchIntent(payload))
                    if payload.decision_id() == completion.decision_id() =>
                {
                    Some((event, payload))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let [(intent_fact, intent)] = intents.as_slice() else {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledRecoveryIntentNotUnique.code(),
                LedgerLocation::RecoverPolicyCompletion.location(),
            ));
        };
        let admissions = self
            .events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload))
                    if payload.decision_id() == completion.decision_id() =>
                {
                    Some((event, payload))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let [(admission_fact, admission)] = admissions.as_slice() else {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledRecoveryAdmissionNotUnique.code(),
                LedgerLocation::RecoverPolicyCompletion.location(),
            ));
        };
        let executions = self
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload))
                        if payload.decision_id() == completion.decision_id()
                )
            })
            .collect::<Vec<_>>();
        let [execution_fact] = executions.as_slice() else {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledRecoveryExecutionNotUnique.code(),
                LedgerLocation::RecoverPolicyCompletion.location(),
            ));
        };
        let EventPayload::Policy(PolicyPayload::ExecutionRecorded(execution)) =
            execution_fact.payload()
        else {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledRecoveryFactTypeInvalid.code(),
                LedgerLocation::RecoverPolicyCompletion.location(),
            ));
        };
        let completions = self
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Policy(PolicyPayload::DispatchCompleted(payload))
                        if payload.decision_id() == execution.decision_id()
                )
            })
            .collect::<Vec<_>>();
        let existing_completion = match completions.as_slice() {
            [] => None,
            [completion] => Some(*completion),
            _ => {
                return Err(GlobalLedgerError::settlement_refusal(
                    LedgerCode::ScheduledRecoveryCompletionNotUnique.code(),
                    LedgerLocation::RecoverPolicyCompletion.location(),
                ));
            }
        };
        if !same_dispatch_definition(intent, admission)
            || !same_dispatch_definition(intent, completion)
            || admission.admission() != completion.admission()
            || intent.admission().is_some()
            || admission.admission().is_none()
            || execution.task_id() != intent.task_id()
            || execution.instance_id() != intent.instance_id()
            || !(intent_fact.sequence() < admission_fact.sequence()
                && admission_fact.sequence() < execution_fact.sequence())
            || !same_chain_without_lease(intent_fact.links(), admission_fact.links())
        {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledRecoveryPayloadConflict.code(),
                LedgerLocation::RecoverPolicyCompletion.location(),
            ));
        }

        let lease_grants = self
            .events
            .iter()
            .filter(|event| {
                event.event_type() == EventType::LeaseGranted
                    && intent_fact.sequence() < event.sequence()
                    && event.sequence() < execution_fact.sequence()
                    && same_chain_with_required_lease(intent_fact.links(), event.links())
            })
            .collect::<Vec<_>>();
        let [lease_granted] = lease_grants.as_slice() else {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledRecoveryLeaseNotUnique.code(),
                LedgerLocation::RecoverPolicyCompletion.location(),
            ));
        };
        let recovered_links = execution_fact.links();
        if !same_scheduled_chain(recovered_links, lease_granted.links()) {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledRecoveryExecutionLinksConflict.code(),
                LedgerLocation::RecoverPolicyCompletion.location(),
            ));
        }
        let releases = self
            .events
            .iter()
            .filter(|event| {
                event.event_type() == EventType::LeaseReleased
                    && same_task_run_chain(event.links(), lease_granted.links())
            })
            .collect::<Vec<_>>();
        // Workflow #369 E3 (#670 rulings 2 and 3, final review, ruling on the open case): a run
        // whose lease a transfer handed on, that expired (or both, one end), or that was
        // released without run links has no run-linked release; that end ends its lease
        // (`unique_lease_end`), whether the run is settled from its terminal or interrupted.
        let handed_on = if releases.is_empty() {
            unique_lease_end(&self.events, lease_granted.links())
        } else {
            None
        };
        let release = match (releases.as_slice(), handed_on) {
            ([release], _) => *release,
            ([], Some((end, _))) => end,
            _ => {
                return Err(GlobalLedgerError::settlement_refusal(
                    LedgerCode::ScheduledRecoveryReleaseNotUnique.code(),
                    LedgerLocation::RecoverPolicyCompletion.location(),
                ));
            }
        };
        if release.sequence() <= admission_fact.sequence()
            || release.sequence() >= execution_fact.sequence()
        {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledRecoveryReleaseOrderInvalid.code(),
                LedgerLocation::RecoverPolicyCompletion.location(),
            ));
        }
        if let Some(existing_completion) = existing_completion {
            let EventPayload::Policy(PolicyPayload::DispatchCompleted(existing)) =
                existing_completion.payload()
            else {
                return Err(GlobalLedgerError::settlement_refusal(
                    LedgerCode::ScheduledRecoveryFactTypeInvalid.code(),
                    LedgerLocation::RecoverPolicyCompletion.location(),
                ));
            };
            if existing != completion
                || !same_scheduled_chain(existing_completion.links(), lease_granted.links())
                || existing_completion.sequence() <= execution_fact.sequence()
            {
                return Err(GlobalLedgerError::settlement_refusal(
                    LedgerCode::ScheduledRecoveryCompletionConflict.code(),
                    LedgerLocation::RecoverPolicyCompletion.location(),
                ));
            }
            return Ok(existing_completion.clone());
        }
        let action_id = issue_scheduled_recovery_action_id()?;
        let continuation = ScheduledPolicyRecoveryContinuation::materialize(
            intent_fact.links().scheduled_policy_recovery_source(),
            lease_granted.links().scheduled_policy_recovery_source(),
            action_id,
        )
        .map_err(|_| {
            GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledRecoveryLinksInvalid.code(),
                LedgerLocation::RecoverPolicyCompletion.location(),
            )
        })?;
        self.append_with_scheduled_recovery_continuation(draft, continuation)
    }

    fn append_recovered_policy_execution(
        &mut self,
        draft: SanitizedEventDraft,
    ) -> GlobalLedgerResult<PersistedEvent> {
        if draft.event_type() != EventType::PolicyExecutionRecorded
            || draft.origin().source() != EventSource::Scheduler
            || draft.origin().module() != OriginModule::Policy
            || draft.origin().actor() != EventActor::Scheduler
            || !draft.artifacts().is_empty()
            || !event_links_empty(draft.links())
        {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledExecutionRecoveryDraftInvalid.code(),
                ContractLocation::RecoverPolicyExecution.location(),
            ));
        }
        let EventPayload::Policy(PolicyPayload::ExecutionRecorded(execution)) = draft.payload()
        else {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledExecutionRecoveryDraftTypeInvalid.code(),
                ContractLocation::RecoverPolicyExecution.location(),
            ));
        };
        // Ledger revision 3 (Workflow #378 A3): the record's severity is its category (a failed
        // execution's failure outcome's, Workflow #378 A2-P; a success's event type's), never
        // the policy's class.
        if draft.severity() != record_severity(EventSeverity::Info, draft.payload()) {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledExecutionRecoverySeverityInvalid.code(),
                ContractLocation::RecoverPolicyExecution.location(),
            ));
        }

        let intents = self
            .events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Policy(PolicyPayload::DispatchIntent(payload))
                    if payload.decision_id() == execution.decision_id() =>
                {
                    Some((event, payload))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let [(intent_fact, intent)] = intents.as_slice() else {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledExecutionRecoveryIntentNotUnique.code(),
                ContractLocation::RecoverPolicyExecution.location(),
            ));
        };
        let admissions = self
            .events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload))
                    if payload.decision_id() == execution.decision_id() =>
                {
                    Some((event, payload))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let [(admission_fact, admission)] = admissions.as_slice() else {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledExecutionRecoveryAdmissionNotUnique.code(),
                ContractLocation::RecoverPolicyExecution.location(),
            ));
        };
        if !same_dispatch_definition(intent, admission)
            || intent.admission().is_some()
            || admission.admission().is_none()
            || execution.task_id() != intent.task_id()
            || execution.instance_id() != intent.instance_id()
            || !same_chain_without_lease(intent_fact.links(), admission_fact.links())
        {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledExecutionRecoveryPayloadConflict.code(),
                ContractLocation::RecoverPolicyExecution.location(),
            ));
        }
        let existing_executions = self
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload))
                        if payload.decision_id() == execution.decision_id()
                )
            })
            .collect::<Vec<_>>();
        let existing_execution = match existing_executions.as_slice() {
            [] => None,
            [existing] => Some(*existing),
            _ => {
                return Err(GlobalLedgerError::settlement_refusal(
                    LedgerCode::ScheduledExecutionRecoveryNotUnique.code(),
                    ContractLocation::RecoverPolicyExecution.location(),
                ));
            }
        };
        let completion_count = self
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Policy(PolicyPayload::DispatchCompleted(payload))
                        if payload.decision_id() == execution.decision_id()
                )
            })
            .count();
        if completion_count > 1 || completion_count == 1 && existing_execution.is_none() {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledExecutionRecoveryOutcomeConflict.code(),
                ContractLocation::RecoverPolicyExecution.location(),
            ));
        }

        let lease_grants = self
            .events
            .iter()
            .filter(|event| {
                event.event_type() == EventType::LeaseGranted
                    && same_chain_with_required_lease(intent_fact.links(), event.links())
            })
            .collect::<Vec<_>>();
        let [lease_granted] = lease_grants.as_slice() else {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledExecutionRecoveryLeaseNotUnique.code(),
                ContractLocation::RecoverPolicyExecution.location(),
            ));
        };
        let recovered_links = lease_granted.links();

        let releases = self
            .events
            .iter()
            .filter(|event| {
                event.event_type() == EventType::LeaseReleased
                    && same_task_run_chain(event.links(), recovered_links)
            })
            .collect::<Vec<_>>();
        // Workflow #369 E3 (#670 rulings 2 and 3, final review, ruling on the open case): a run
        // whose lease a transfer handed on, that expired (or both, one end), or that was
        // released without run links, before any run-linked release, has that end as its lease
        // end (`unique_lease_end`); no release is written for a lease that already ended. With
        // a terminal the run is settled from it, otherwise as interrupted from that end.
        let handed_on = if releases.is_empty() {
            unique_lease_end(&self.events, recovered_links)
        } else {
            None
        };
        let release = match (releases.as_slice(), handed_on) {
            ([release], _) => *release,
            ([], Some((end, _))) => end,
            _ => {
                return Err(GlobalLedgerError::settlement_refusal(
                    LedgerCode::ScheduledExecutionRecoveryReleaseNotUnique.code(),
                    ContractLocation::RecoverPolicyExecution.location(),
                ));
            }
        };
        if handed_on.is_none()
            && (release.severity() != record_severity(EventSeverity::Info, release.payload())
                || release.payload().action() != EventAction::LeaseRelease
                || !matches!(
                    release.payload(),
                    EventPayload::Lease(LeasePayload::Released(_))
                )
                || !matches!(
                    release.payload().effect_disposition(),
                    Some(EffectDisposition::Performed | EffectDisposition::NotPerformed)
                ))
        {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledExecutionRecoveryReleaseInvalid.code(),
                ContractLocation::RecoverPolicyExecution.location(),
            ));
        }

        let terminals = self
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type(),
                    EventType::TaskCompleted | EventType::TaskFailed | EventType::TaskCancelled
                ) && event.links().task_id() == recovered_links.task_id()
                    && event.links().run_id() == recovered_links.run_id()
            })
            .collect::<Vec<_>>();
        if terminals.len() > 1 {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledExecutionRecoveryTerminalNotUnique.code(),
                ContractLocation::RecoverPolicyExecution.location(),
            ));
        }
        // Workflow #369 E3 (coordinator ruling on #670 review H-1 and on the open case): a run
        // with no terminal whose lease ended without a recorded outcome (the start recovered its
        // release, its own release records `not_performed`, or its lease otherwise ended) is
        // settled once as interrupted, from that release or end. A run's release with no
        // terminal is taken through `same_task_run_chain`, under the grant's request or the
        // run's own task request (final review M-1). A run that has a terminal is settled from
        // it (`terminal_lease_end_valid`); an interrupted settlement of such a run is refused,
        // whatever the order of its terminal and its release or lease end (coordinator ruling
        // on #670, R4).
        let recovered_interruption = (handed_on.is_some()
            || release.payload().effect_disposition() == Some(EffectDisposition::NotPerformed))
            && is_interrupted_settlement(execution.policy_result());
        let source_fact = match terminals.as_slice() {
            _ if recovered_interruption => release,
            [] => release,
            [terminal] => *terminal,
            _ => {
                return Err(GlobalLedgerError::settlement_refusal(
                    LedgerCode::ScheduledExecutionRecoveryTerminalNotUnique.code(),
                    ContractLocation::RecoverPolicyExecution.location(),
                ));
            }
        };
        // Coordinator ruling on #670's open case: a run that has a terminal is settled from it,
        // whatever the order of its lease end. The lease end is the run's run-linked release
        // after the terminal: its own (under the terminal's request), or one on the grant's
        // chain (the release the start recovered, or the one the run's settlement wrote when its
        // own cleanup left the lease). With no run-linked release it is the end that ended the
        // lease, after the terminal, or before it only when the lease expired (the run's
        // terminal followed its lease's expiry); the order check relaxes for that shape only.
        let terminal_lease_end_valid =
            |terminal: &PersistedEvent, same_request: bool| match handed_on {
                None => {
                    same_task_run_chain(release.links(), recovered_links)
                        && (!same_request
                            || release.links().request_id() == terminal.links().request_id()
                            || same_scheduled_chain(release.links(), recovered_links))
                        && release.sequence() > terminal.sequence()
                }
                Some((end, expired)) => end.sequence() > terminal.sequence() || expired,
            };
        // Coordinator ruling on #670 (runtime budget): a run settled from its terminal that ran
        // past its admitted runtime budget carries the policy's rewrite instead of its own
        // outcome. The ledger does not record the dispatch's expected duration, so it checks the
        // rewrite's form and the bound every rewrite meets (`runtime_exceeded` with no expected
        // duration); the host checks the policy's exact rule.
        let runtime_rewrite_valid = |reported_success: bool, runtime_ms: u64| {
            matches!(
                execution.policy_result(),
                PolicyExecutionOutcome::Failed { failure }
                    if failure.is_runtime_budget_rewrite(reported_success)
                        && failure.runtime_ms == runtime_ms
            ) && admission
                .admission()
                .and_then(|record| record.budget.runtime_exceeded(0, runtime_ms))
                == Some(true)
        };
        // Coordinator ruling on #670 (R4; verification N-2): only a run with no terminal is
        // settled as interrupted.
        let recovered_topology_valid = recovered_interruption && terminals.is_empty();
        if !(intent_fact.sequence() < lease_granted.sequence()
            && lease_granted.sequence() < admission_fact.sequence()
            && admission_fact.sequence() < source_fact.sequence())
        {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledExecutionRecoveryOrderInvalid.code(),
                ContractLocation::RecoverPolicyExecution.location(),
            ));
        }
        if let Some(existing_execution) = existing_execution {
            let EventPayload::Policy(PolicyPayload::ExecutionRecorded(existing)) =
                existing_execution.payload()
            else {
                return Err(GlobalLedgerError::settlement_refusal(
                    LedgerCode::ScheduledExecutionRecoveryFactTypeInvalid.code(),
                    ContractLocation::RecoverPolicyExecution.location(),
                ));
            };
            let topology_valid = match source_fact.event_type() {
                _ if recovered_interruption => recovered_topology_valid,
                EventType::TaskCompleted | EventType::TaskFailed => {
                    same_task_run_chain(source_fact.links(), recovered_links)
                        && terminal_lease_end_valid(source_fact, false)
                }
                EventType::LeaseReleased => {
                    same_task_run_chain(source_fact.links(), recovered_links)
                        && terminals.is_empty()
                        && !self.events.iter().any(|event| {
                            event.links().task_id() == recovered_links.task_id()
                                && event.links().run_id() == recovered_links.run_id()
                                && matches!(
                                    event.event_type(),
                                    EventType::TaskEffectIntent
                                        | EventType::TaskEffectCompleted
                                        | EventType::InputIntent
                                        | EventType::InputCommitted
                                        | EventType::InputFailed
                                )
                        })
                        && matches!(
                            execution.policy_result(),
                            PolicyExecutionOutcome::Failed { failure }
                                if failure.outcome.is(SharedCode::PolicySettlementInterrupted)
                                    && failure.failure_identity.is_none()
                                    && failure.original_class == PolicyFailureClass::Severe
                                    && failure.effective_class == PolicyFailureClass::Severe
                                    && failure.disposition == PolicyFailureDisposition::PausedTask
                                    && failure.retry_attempt == 0
                                    && failure.retry_at_unix_ms.is_none()
                                    && !failure.reported_success
                                    && failure.runtime_ms == 0
                        )
                }
                _ => false,
            };
            if existing != execution
                || !same_scheduled_chain(existing_execution.links(), recovered_links)
                || !topology_valid
            {
                return Err(GlobalLedgerError::settlement_refusal(
                    LedgerCode::ScheduledExecutionRecoveryConflict.code(),
                    ContractLocation::RecoverPolicyExecution.location(),
                ));
            }
            return Ok(existing_execution.clone());
        }
        match source_fact.event_type() {
            _ if recovered_interruption => {
                if !recovered_topology_valid {
                    return Err(GlobalLedgerError::settlement_refusal(
                        LedgerCode::ScheduledExecutionRecoveryInterruptionConflict.code(),
                        ContractLocation::RecoverPolicyExecution.location(),
                    ));
                }
            }
            EventType::TaskCompleted => {
                let [terminal] = terminals.as_slice() else {
                    return Err(GlobalLedgerError::settlement_refusal(
                        LedgerCode::ScheduledExecutionRecoveryTerminalNotUnique.code(),
                        ContractLocation::RecoverPolicyExecution.location(),
                    ));
                };
                let task_requests =
                    matching_scheduled_task_requests(&self.events, source_fact, recovered_links);
                let [task_request] = task_requests.as_slice() else {
                    return Err(GlobalLedgerError::settlement_refusal(
                        LedgerCode::ScheduledExecutionRecoveryTaskRequestNotUnique.code(),
                        ContractLocation::RecoverPolicyExecution.location(),
                    ));
                };
                let runtime_ms = source_fact
                    .timestamp_unix_ms()
                    .checked_sub(task_request.timestamp_unix_ms())
                    .ok_or_else(|| {
                        GlobalLedgerError::settlement_refusal(
                            LedgerCode::ScheduledExecutionRecoveryClockRegressed.code(),
                            ContractLocation::RecoverPolicyExecution.location(),
                        )
                    })?;
                if *terminal != source_fact
                    || !same_task_run_chain(source_fact.links(), recovered_links)
                    || !terminal_lease_end_valid(source_fact, true)
                    || source_fact.severity()
                        != record_severity(EventSeverity::Info, source_fact.payload())
                    || !(admission_fact.sequence() < task_request.sequence()
                        && task_request.sequence() < source_fact.sequence())
                    || !matches!(
                        source_fact.payload(),
                        EventPayload::Task(TaskPayload::Semantic(payload))
                            if matches!(
                                payload.fact(),
                                TaskSemanticFact::TerminalCommitted {
                                    task_result: TaskOutcome::Success,
                                    outcome: None,
                                    ..
                                }
                            )
                    )
                    || !(matches!(
                        execution.policy_result(),
                        PolicyExecutionOutcome::Succeeded {
                            runtime_ms: actual
                        } if *actual == runtime_ms
                    ) || runtime_rewrite_valid(true, runtime_ms))
                {
                    return Err(GlobalLedgerError::settlement_refusal(
                        LedgerCode::ScheduledExecutionRecoverySuccessConflict.code(),
                        ContractLocation::RecoverPolicyExecution.location(),
                    ));
                }
            }
            EventType::TaskFailed => {
                let [terminal] = terminals.as_slice() else {
                    return Err(GlobalLedgerError::settlement_refusal(
                        LedgerCode::ScheduledExecutionRecoveryTerminalNotUnique.code(),
                        ContractLocation::RecoverPolicyExecution.location(),
                    ));
                };
                // Ledger revision 3 (Workflow #378 A3): a terminal's severity is its outcome's
                // category; one that breaks the rule is refused as before, and the class is read
                // from the terminal's outcome code below (model section 8.1), never from it.
                if source_fact.severity()
                    != record_severity(EventSeverity::Info, source_fact.payload())
                {
                    return Err(GlobalLedgerError::settlement_refusal(
                        LedgerCode::ScheduledExecutionRecoveryFailureSeverityAmbiguous.code(),
                        ContractLocation::RecoverPolicyExecution.location(),
                    ));
                }
                let EventPayload::Task(TaskPayload::Semantic(payload)) = source_fact.payload()
                else {
                    return Err(GlobalLedgerError::settlement_refusal(
                        LedgerCode::ScheduledExecutionRecoveryFailureInvalid.code(),
                        ContractLocation::RecoverPolicyExecution.location(),
                    ));
                };
                let TaskSemanticFact::TerminalCommitted {
                    task_result: TaskOutcome::Failure,
                    outcome: Some(failure_outcome),
                    failure_severe,
                    ..
                } = payload.fact()
                else {
                    return Err(GlobalLedgerError::settlement_refusal(
                        LedgerCode::ScheduledExecutionRecoveryFailureInvalid.code(),
                        ContractLocation::RecoverPolicyExecution.location(),
                    ));
                };
                // The code and the class come from the terminal through the contract's shared
                // functions, as the Runtime's live settlement and startup recovery read them
                // (model section 8.1). The recovered failure is the terminal's: the same code
                // (Workflow #378 A2-P), and no failure identity, which only a live settlement
                // computes.
                let failure_code = scheduled_failure_code_str(failure_outcome);
                let class = scheduled_failure_class(failure_outcome.code(), *failure_severe);
                let task_requests =
                    matching_scheduled_task_requests(&self.events, source_fact, recovered_links);
                let [task_request] = task_requests.as_slice() else {
                    return Err(GlobalLedgerError::settlement_refusal(
                        LedgerCode::ScheduledExecutionRecoveryTaskRequestNotUnique.code(),
                        ContractLocation::RecoverPolicyExecution.location(),
                    ));
                };
                let runtime_ms = source_fact
                    .timestamp_unix_ms()
                    .checked_sub(task_request.timestamp_unix_ms())
                    .ok_or_else(|| {
                        GlobalLedgerError::settlement_refusal(
                            LedgerCode::ScheduledExecutionRecoveryClockRegressed.code(),
                            ContractLocation::RecoverPolicyExecution.location(),
                        )
                    })?;
                if *terminal != source_fact
                    || !same_task_run_chain(source_fact.links(), recovered_links)
                    || !terminal_lease_end_valid(source_fact, true)
                    || !(admission_fact.sequence() < task_request.sequence()
                        && task_request.sequence() < source_fact.sequence())
                    || !(matches!(
                        execution.policy_result(),
                        PolicyExecutionOutcome::Failed { failure }
                            if scheduled_failure_code_str(&failure.outcome) == failure_code
                                && failure.failure_identity.is_none()
                                && failure.original_class == class
                                && !failure.reported_success
                                && failure.runtime_ms == runtime_ms
                    ) || runtime_rewrite_valid(false, runtime_ms))
                {
                    return Err(GlobalLedgerError::settlement_refusal(
                        LedgerCode::ScheduledExecutionRecoveryFailureConflict.code(),
                        ContractLocation::RecoverPolicyExecution.location(),
                    ));
                }
            }
            EventType::LeaseReleased => {
                if source_fact != release
                    || !same_task_run_chain(source_fact.links(), recovered_links)
                    || !terminals.is_empty()
                    || source_fact.timestamp_unix_ms() != execution.observed_at_unix_ms()
                    || self.events.iter().any(|event| {
                        event.links().task_id() == recovered_links.task_id()
                            && event.links().run_id() == recovered_links.run_id()
                            && matches!(
                                event.event_type(),
                                EventType::TaskEffectIntent
                                    | EventType::TaskEffectCompleted
                                    | EventType::InputIntent
                                    | EventType::InputCommitted
                                    | EventType::InputFailed
                            )
                    })
                    || !matches!(
                        execution.policy_result(),
                        PolicyExecutionOutcome::Failed { failure }
                            if failure.outcome.is(SharedCode::PolicySettlementInterrupted)
                                && failure.failure_identity.is_none()
                                && failure.original_class == PolicyFailureClass::Severe
                                && failure.effective_class == PolicyFailureClass::Severe
                                && failure.disposition == PolicyFailureDisposition::PausedTask
                                && failure.retry_attempt == 0
                                && failure.retry_at_unix_ms.is_none()
                                && !failure.reported_success
                                && failure.runtime_ms == 0
                    )
                {
                    return Err(GlobalLedgerError::settlement_refusal(
                        LedgerCode::ScheduledExecutionRecoveryInterruptionConflict.code(),
                        ContractLocation::RecoverPolicyExecution.location(),
                    ));
                }
            }
            _ => {
                return Err(GlobalLedgerError::settlement_refusal(
                    LedgerCode::ScheduledExecutionRecoveryFactTypeInvalid.code(),
                    ContractLocation::RecoverPolicyExecution.location(),
                ));
            }
        }
        if execution.observed_at_unix_ms() != source_fact.timestamp_unix_ms() {
            return Err(GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledExecutionRecoveryTimeConflict.code(),
                ContractLocation::RecoverPolicyExecution.location(),
            ));
        }
        let action_id = issue_scheduled_recovery_action_id()?;
        let continuation = ScheduledPolicyRecoveryContinuation::materialize(
            intent_fact.links().scheduled_policy_recovery_source(),
            lease_granted.links().scheduled_policy_recovery_source(),
            action_id,
        )
        .map_err(|_| {
            GlobalLedgerError::settlement_refusal(
                LedgerCode::ScheduledExecutionRecoveryLinksInvalid.code(),
                ContractLocation::RecoverPolicyExecution.location(),
            )
        })?;
        self.append_with_scheduled_recovery_continuation(draft, continuation)
    }

    fn persist_event(
        &mut self,
        event: PersistedEvent,
        observation: &mut Option<LedgerAppendObservation>,
    ) -> GlobalLedgerResult<PersistedEvent> {
        self.persist_retention_checked_observed(event, false, observation)
    }

    pub(super) fn validate_retention_admission(
        &self,
        event: &PersistedEvent,
        guarded: bool,
    ) -> GlobalLedgerResult<()> {
        if self.recovering_retention
            && event.event_type() != EventType::LedgerRecovered
            && !(guarded && event.event_type() == EventType::ArtifactEvictionOutcome)
        {
            return Err(GlobalLedgerError::request(
                LedgerCode::ArtifactRetentionRecoveryPending.code(),
                LedgerLocation::AppendEvent.location(),
            ));
        }
        self.retention
            .validate(event, &self.events, &self.indexes, guarded, true)
    }

    pub(super) fn persist_retention_checked(
        &mut self,
        event: PersistedEvent,
        guarded: bool,
    ) -> GlobalLedgerResult<PersistedEvent> {
        self.persist_retention_checked_observed(event, guarded, &mut None)
    }

    fn persist_retention_checked_observed(
        &mut self,
        event: PersistedEvent,
        guarded: bool,
        observation: &mut Option<LedgerAppendObservation>,
    ) -> GlobalLedgerResult<PersistedEvent> {
        let following_sequence = increment_sequence(self.next_sequence)?;
        if self.indexes.contains_event_id(event.event_id()) {
            return Err(GlobalLedgerError::request(
                LedgerCode::DuplicateEventId.code(),
                LedgerLocation::AppendEvent.location(),
            ));
        }
        self.validate_retention_admission(&event, guarded)?;
        if let Some(value) = observation {
            value.durable_started_at = Some(Instant::now());
        }
        let result = self.persist_retried(&event, None);
        if let Some(value) = observation {
            value.persisted(result.is_ok());
        }
        let write_sync_ns = result?;
        self.publish_committed(event, following_sequence, write_sync_ns)
    }

    pub(super) fn publish_committed(
        &mut self,
        event: PersistedEvent,
        following_sequence: u64,
        write_sync_ns: Option<u64>,
    ) -> GlobalLedgerResult<PersistedEvent> {
        self.next_sequence = following_sequence;
        self.retention.apply(&event);
        self.indexes.insert(&event, self.events.len());
        self.events.push(event.clone());
        self.commit_statistics
            .record_success(event.sequence(), write_sync_ns)?;
        Ok(event)
    }

    pub(super) fn query(&self, query: &actingcommand_contract::EventQuery) -> Vec<PersistedEvent> {
        let mut events = self.indexes.query(&self.events, query);
        for event in &mut events {
            self.retention.annotate_event(event);
        }
        events
    }

    pub(super) fn query_page(
        &self,
        query: &actingcommand_contract::EventQuery,
        after_sequence: u64,
        through_sequence: u64,
        page_events: usize,
    ) -> Vec<PersistedEvent> {
        let mut events = self.indexes.query_page(
            &self.events,
            query,
            after_sequence,
            through_sequence,
            page_events,
        );
        for event in &mut events {
            self.retention.annotate_event(event);
        }
        events
    }

    pub(super) fn latest_sequence(&self) -> u64 {
        self.events.last().map_or(0, PersistedEvent::sequence)
    }

    pub(super) fn resolve_artifact(
        &self,
        selection: &super::LedgerArtifactSelection,
        deadline: Instant,
    ) -> GlobalLedgerResult<super::ResolvedLedgerArtifact> {
        if let Some(resolved) = self.backend.resolve_artifact(selection, deadline) {
            return resolved;
        }
        super::evidence::resolve_artifact_from_events(
            &self.events,
            selection,
            self.latest_sequence(),
            true,
            deadline,
            Some(&self.retention),
        )
    }

    pub(super) fn project_view_page(
        &self,
        query: &actingcommand_contract::EventQuery,
        profile: actingcommand_contract::ProjectionProfile,
        request: &actingcommand_contract::RuntimeEventQueryPageRequest,
    ) -> GlobalLedgerResult<actingcommand_contract::RuntimeEventQueryPage> {
        self.project_view_page_observed(query, profile, request, &mut None)
    }

    pub(super) fn project_view_page_observed(
        &self,
        query: &actingcommand_contract::EventQuery,
        profile: actingcommand_contract::ProjectionProfile,
        request: &actingcommand_contract::RuntimeEventQueryPageRequest,
        observation: &mut Option<LedgerProjectViewObservation>,
    ) -> GlobalLedgerResult<actingcommand_contract::RuntimeEventQueryPage> {
        if let Some(page) = self
            .backend
            .project_view_page(query, profile, request, observation)
        {
            return page;
        }
        *observation = None;
        self.indexes.project_view_page(
            &self.events,
            query,
            profile,
            request,
            actingcommand_contract::LedgerReadScope {
                source: actingcommand_contract::LedgerReadSource::Runtime,
                material_read: actingcommand_contract::LedgerMaterialReadState::NotRequested,
                scanned_through_position: self.latest_sequence(),
                read_complete: true,
                limits: Vec::new(),
            },
            super::projection::PageSelection {
                through_sequence: self.latest_sequence(),
                sequences: None,
                retention: Some(&self.retention),
            },
        )
    }

    pub(super) fn replay_page(
        &self,
        after_sequence: u64,
        through_sequence: u64,
        page_events: usize,
    ) -> Vec<PersistedEvent> {
        let start = self
            .events
            .partition_point(|event| event.sequence() <= after_sequence);
        let mut events = self.events[start..]
            .iter()
            .take_while(|event| event.sequence() <= through_sequence)
            .take(page_events)
            .cloned()
            .collect::<Vec<_>>();
        for event in &mut events {
            self.retention.annotate_event(event);
        }
        events
    }

    pub(super) fn close(mut self) -> GlobalLedgerResult<()> {
        self.backend.close()
    }

    pub(super) fn append_recovery(
        &mut self,
        reason: RecoveryReason,
        previous_owner: Option<String>,
        tail: Option<(u64, u64)>,
        event_id: Option<EventId>,
    ) -> GlobalLedgerResult<PersistedEvent> {
        let mut audit = AuditInput::new();
        if let Some(owner_id) = previous_owner {
            audit = audit.with_account(owner_id);
        }
        let (segment_index, affected_bytes) =
            tail.map_or((None, 0), |(segment, bytes)| (Some(segment), bytes));
        let now = unix_ms_now()?;
        let payload = LedgerPayloadDraft::recovered(reason, segment_index, affected_bytes, audit);
        let identifiers = IdentifierIssuer::new().map_err(|_| {
            GlobalLedgerError::fatal(
                LedgerCode::RecoveryEventFailed.code(),
                LedgerLocation::CreateIdentifierIssuer.location(),
            )
        })?;
        let draft = EventDraft::new(
            identifiers.mint_event_id().map_err(|_| {
                GlobalLedgerError::fatal(
                    LedgerCode::RecoveryEventFailed.code(),
                    LedgerLocation::IssueRecoveryEventId.location(),
                )
            })?,
            now,
            EventSeverity::Warning,
            EventOrigin::new(
                EventSource::System,
                OriginModule::GlobalLedger,
                EventActor::System,
            ),
            EventLinksDraft::default(),
            payload.into(),
        )
        .sanitize(
            &Sha256SecretFingerprinter::new(b"actingcommand-ledger-recovery-v2").map_err(|_| {
                GlobalLedgerError::fatal(
                    LedgerCode::RecoveryEventFailed.code(),
                    LedgerLocation::ConfigureFingerprinter.location(),
                )
            })?,
        )
        .map_err(|_| {
            GlobalLedgerError::fatal(
                LedgerCode::RecoveryEventFailed.code(),
                LedgerLocation::SanitizeRecoveryEvent.location(),
            )
        })?;
        self.append_with_event_id(draft, event_id, &mut None)
    }
}

#[cfg(test)]
impl DurableStorage for SegmentStorage {
    fn material_root(&self) -> &Path {
        &self.root
    }

    fn persist(&mut self, event: &PersistedEvent) -> GlobalLedgerResult<Option<u64>> {
        let mut bytes = serde_json::to_vec(&StoredLine {
            line_type: LINE_TYPE.to_string(),
            event: StoredEventRecord::from_event(event),
        })
        .map_err(|error| {
            GlobalLedgerError::json_untyped("event_serialization_failed", "serialize_event", &error)
        })?;
        bytes.push(b'\n');
        if self.active_bytes > 0
            && self.active_bytes.saturating_add(bytes.len() as u64) > self.segment_max_bytes
        {
            self.rotate()?;
        }
        let write_started = Instant::now();
        self.active_file
            .write_all(&bytes)
            .map_err(|error| GlobalLedgerError::io_untyped("ledger_io", "append_event", &error))?;
        self.active_file
            .sync_all()
            .map_err(|error| GlobalLedgerError::io_untyped("ledger_io", "sync_event", &error))?;
        let write_sync_ns = Instant::now()
            .checked_duration_since(write_started)
            .and_then(|value| u64::try_from(value.as_nanos()).ok());
        self.active_bytes = self.active_bytes.saturating_add(bytes.len() as u64);
        Ok(write_sync_ns)
    }

    fn close(&mut self) -> GlobalLedgerResult<()> {
        self.active_file
            .sync_all()
            .map_err(|error| GlobalLedgerError::io_untyped("ledger_io", "sync_on_close", &error))?;
        self.ownership.close()
    }
}

#[cfg(test)]
impl SegmentStorage {
    fn rotate(&mut self) -> GlobalLedgerResult<()> {
        self.active_file.sync_all().map_err(|error| {
            GlobalLedgerError::io_untyped("ledger_io", "sync_before_rotate", &error)
        })?;
        self.active_index = self.active_index.saturating_add(1);
        let path = segment_path(&self.segments_dir, self.active_index);
        self.active_file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .read(true)
            .open(path)
            .map_err(|error| {
                GlobalLedgerError::io_untyped("ledger_io", "create_segment", &error)
            })?;
        self.active_bytes = 0;
        Ok(())
    }
}

fn scheduled_policy_execution_recovery_draft(
    execution: &PolicyExecutionEventData,
) -> GlobalLedgerResult<SanitizedEventDraft> {
    // The stored severity is the record's category (ledger revision 3, Workflow #378 A3): a
    // failure's outcome's, a success's event type's (Workflow #378 A2-P).
    scheduled_policy_recovery_draft(
        execution.observed_at_unix_ms,
        EventSeverity::Info,
        PolicyPayloadDraft::execution_recorded(execution.clone(), AuditInput::new()).into(),
    )
}

fn scheduled_policy_recovery_draft(
    timestamp_unix_ms: u64,
    severity: EventSeverity,
    payload: EventPayloadDraft,
) -> GlobalLedgerResult<SanitizedEventDraft> {
    let identifiers = IdentifierIssuer::new().map_err(|_| {
        GlobalLedgerError::fatal(
            LedgerCode::ScheduledRecoveryEventIdFailed.code(),
            LedgerLocation::BuildScheduledPolicyRecovery.location(),
        )
    })?;
    EventDraft::new(
        identifiers.mint_event_id().map_err(|_| {
            GlobalLedgerError::fatal(
                LedgerCode::ScheduledRecoveryEventIdFailed.code(),
                LedgerLocation::BuildScheduledPolicyRecovery.location(),
            )
        })?,
        timestamp_unix_ms,
        severity,
        EventOrigin::new(
            EventSource::Scheduler,
            OriginModule::Policy,
            EventActor::Scheduler,
        ),
        EventLinksDraft::default(),
        payload,
    )
    .sanitize(
        &Sha256SecretFingerprinter::new(b"actingcommand-ledger-scheduled-settlement-v1").map_err(
            |_| {
                GlobalLedgerError::fatal(
                    LedgerCode::ScheduledRecoverySanitizeFailed.code(),
                    LedgerLocation::BuildScheduledPolicyRecovery.location(),
                )
            },
        )?,
    )
    .map_err(|_| {
        GlobalLedgerError::fatal(
            LedgerCode::ScheduledRecoverySanitizeFailed.code(),
            LedgerLocation::BuildScheduledPolicyRecovery.location(),
        )
    })
}

fn dispatch_event_data(payload: &PolicyDispatchPayload) -> PolicyDispatchEventData {
    PolicyDispatchEventData {
        decision_id: payload.decision_id().to_owned(),
        task_id: payload.task_id().to_owned(),
        instance_id: payload.instance_id().to_owned(),
        operation_id: payload.operation_id().to_owned(),
        package_digest: payload.package_digest().to_owned(),
        procedure_binding_digest: payload.procedure_binding_digest().to_owned(),
        reason_chain_id: payload.reason_chain_id().to_owned(),
        reasons: payload.reasons().to_vec(),
        catalog_hash: payload.catalog_hash().to_owned(),
        catalog_version: payload.catalog_version(),
        input_ledger_position: payload.input_ledger_position(),
        fact_snapshot_id: payload.fact_snapshot_id().to_owned(),
        approval_fact_ids: payload.approval_fact_ids().to_vec(),
        urgency_milli: payload.urgency_milli(),
    }
}

fn issue_scheduled_recovery_action_id() -> GlobalLedgerResult<IssuedActionId> {
    IdentifierIssuer::new()
        .and_then(|issuer| issuer.mint_action_id())
        .map_err(|_| {
            GlobalLedgerError::fatal(
                LedgerCode::ScheduledRecoveryActionIdFailed.code(),
                ContractLocation::RecoverPolicyExecution.location(),
            )
        })
}

fn event_links_empty(links: &EventLinks) -> bool {
    links.instance_id().is_none()
        && links.request_id().is_none()
        && links.correlation_id().is_none()
        && links.causation_id().is_none()
        && links.task_id().is_none()
        && links.run_id().is_none()
        && links.lease_id().is_none()
        && links.frame_id().is_none()
        && links.action_id().is_none()
        && links.recognition_id().is_none()
}

fn is_unlinked_scheduled_settlement_draft(draft: &SanitizedEventDraft) -> bool {
    matches!(
        draft.event_type(),
        EventType::PolicyExecutionRecorded | EventType::PolicyDispatchCompleted
    ) && draft.origin().source() == EventSource::Scheduler
        && draft.origin().module() == OriginModule::Policy
        && draft.origin().actor() == EventActor::Scheduler
        && event_links_empty(draft.links())
}

fn same_chain_without_lease(left: &EventLinks, right: &EventLinks) -> bool {
    left.instance_id().is_some()
        && left.request_id().is_some()
        && left.correlation_id().is_some()
        && left.task_id().is_some()
        && left.run_id().is_some()
        && left.instance_id() == right.instance_id()
        && left.request_id() == right.request_id()
        && left.correlation_id() == right.correlation_id()
        && left.causation_id() == right.causation_id()
        && left.task_id() == right.task_id()
        && left.run_id() == right.run_id()
        && left.lease_id().is_none()
        && right.lease_id().is_none()
        && left.frame_id().is_none()
        && right.frame_id().is_none()
        && left.recognition_id().is_none()
        && right.recognition_id().is_none()
}

fn same_chain_with_required_lease(dispatch: &EventLinks, lease: &EventLinks) -> bool {
    dispatch.instance_id().is_some()
        && dispatch.request_id().is_some()
        && dispatch.correlation_id().is_some()
        && dispatch.task_id().is_some()
        && dispatch.run_id().is_some()
        && dispatch.instance_id() == lease.instance_id()
        && dispatch.request_id() == lease.request_id()
        && dispatch.correlation_id() == lease.correlation_id()
        && dispatch.causation_id() == lease.causation_id()
        && dispatch.task_id() == lease.task_id()
        && dispatch.run_id() == lease.run_id()
        && dispatch.lease_id().is_none()
        && lease.lease_id().is_some()
        && dispatch.frame_id().is_none()
        && lease.frame_id().is_none()
        && dispatch.recognition_id().is_none()
        && lease.recognition_id().is_none()
}

fn same_scheduled_chain(actual: &EventLinks, expected: &EventLinks) -> bool {
    actual.instance_id() == expected.instance_id()
        && actual.request_id() == expected.request_id()
        && actual.correlation_id() == expected.correlation_id()
        && actual.causation_id() == expected.causation_id()
        && actual.task_id() == expected.task_id()
        && actual.run_id() == expected.run_id()
        && actual.lease_id() == expected.lease_id()
        && actual.frame_id().is_none()
        && actual.action_id().is_some()
        && actual.recognition_id().is_none()
}

fn same_task_run_chain(actual: &EventLinks, scheduled: &EventLinks) -> bool {
    actual.instance_id() == scheduled.instance_id()
        && actual.request_id().is_some()
        && actual.correlation_id() == scheduled.correlation_id()
        && actual.causation_id() == scheduled.causation_id()
        && actual.task_id() == scheduled.task_id()
        && actual.run_id() == scheduled.run_id()
        && actual.lease_id() == scheduled.lease_id()
        && actual.frame_id().is_none()
        && actual.recognition_id().is_none()
}

/// The one event that ended the lease of `granted` without a run-linked release, on its
/// instance: a `lease.transferred` that handed it on, its `lease.expired`, or a `lease.released`
/// without run links (the host's close). An expiry handed on to a waiting claim writes a
/// transfer and the lease's expiry; the two are one end, at the earlier (#670 final review
/// M-2). Any other set of ends is no one end. The host's `single_lease_end` takes the same end.
/// Also returns whether the lease expired (an expiry is among the ends).
fn unique_lease_end<'a>(
    events: &'a [PersistedEvent],
    granted: &EventLinks,
) -> Option<(&'a PersistedEvent, bool)> {
    let lease_id = granted.lease_id()?;
    let ends = events
        .iter()
        .filter(|event| {
            event.links().instance_id() == granted.instance_id()
                && (matches!(
                    event.payload(),
                    EventPayload::Lease(LeasePayload::Transferred(transfer))
                        if *lease_id == transfer.from_lease_id()
                ) || (event.links().lease_id() == Some(lease_id)
                    && (event.event_type() == EventType::LeaseExpired
                        || (event.event_type() == EventType::LeaseReleased
                            && event.links().run_id().is_none()))))
        })
        .collect::<Vec<_>>();
    match ends.as_slice() {
        [end] => Some((*end, end.event_type() == EventType::LeaseExpired)),
        [first, second] => {
            let types = [first.event_type(), second.event_type()];
            (types.contains(&EventType::LeaseTransferred)
                && types.contains(&EventType::LeaseExpired))
            .then_some(if first.sequence() < second.sequence() {
                (*first, true)
            } else {
                (*second, true)
            })
        }
        _ => None,
    }
}

/// The interrupted settlement of a scheduled run the daemon's own end cut short
/// (`policy_settlement_interrupted`, no failure identity), as the startup reconciliation
/// records it.
fn is_interrupted_settlement(outcome: &PolicyExecutionOutcome) -> bool {
    matches!(
        outcome,
        PolicyExecutionOutcome::Failed { failure }
            if failure.outcome.is(SharedCode::PolicySettlementInterrupted)
                && failure.failure_identity.is_none()
                && failure.original_class == PolicyFailureClass::Severe
                && failure.effective_class == PolicyFailureClass::Severe
                && failure.disposition == PolicyFailureDisposition::PausedTask
                && failure.retry_attempt == 0
                && failure.retry_at_unix_ms.is_none()
                && !failure.reported_success
                && failure.runtime_ms == 0
    )
}

/// The request a recovered scheduled run is measured from: a fixture-simulated run's client
/// intent (`lab.request`), or, for a physical run, which records none, its scheduler
/// `command.received` (`runtime.task_run`) under the run's links.
fn matching_scheduled_task_requests<'a>(
    events: &'a [PersistedEvent],
    terminal: &PersistedEvent,
    scheduled: &EventLinks,
) -> Vec<&'a PersistedEvent> {
    let same_request = |event: &PersistedEvent| {
        event.links().instance_id() == scheduled.instance_id()
            && event.links().correlation_id() == scheduled.correlation_id()
            && event.links().causation_id() == scheduled.causation_id()
            && event.links().task_id() == scheduled.task_id()
            && event.links().run_id() == scheduled.run_id()
            && event.links().lease_id().is_none()
            && event.links().frame_id().is_none()
            && event.links().recognition_id().is_none()
            && event.links().request_id() == terminal.links().request_id()
    };
    let client_intents = events
        .iter()
        .filter(|event| event.event_type() == EventType::LabRequest && same_request(event))
        .collect::<Vec<_>>();
    if !client_intents.is_empty() {
        return client_intents;
    }
    events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::CommandReceived
                && event.origin().source() == EventSource::Scheduler
                && event.payload().action() == EventAction::RuntimeTaskRun
                && same_request(event)
        })
        .collect()
}

fn same_dispatch_definition(
    expected: &PolicyDispatchPayload,
    actual: &PolicyDispatchPayload,
) -> bool {
    expected.decision_id() == actual.decision_id()
        && expected.task_id() == actual.task_id()
        && expected.instance_id() == actual.instance_id()
        && expected.operation_id() == actual.operation_id()
        && expected.package_digest() == actual.package_digest()
        && expected.procedure_binding_digest() == actual.procedure_binding_digest()
        && expected.reason_chain_id() == actual.reason_chain_id()
        && expected.reasons() == actual.reasons()
        && expected.catalog_hash() == actual.catalog_hash()
        && expected.catalog_version() == actual.catalog_version()
        && expected.input_ledger_position() == actual.input_ledger_position()
        && expected.fact_snapshot_id() == actual.fact_snapshot_id()
        && expected.approval_fact_ids() == actual.approval_fact_ids()
        && expected.urgency_milli() == actual.urgency_milli()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredLine {
    pub(super) line_type: String,
    pub(super) event: StoredEventRecord,
}

#[cfg(test)]
struct RecoveryState {
    next_sequence: u64,
    events: Vec<PersistedEvent>,
    last_segment_index: u64,
    pending_repairs: Vec<TailRepairRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RepairJournalState {
    Prepared,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TailRepairRecord {
    schema_version: String,
    repair_id: String,
    state: RepairJournalState,
    segment_index: u64,
    original_len: u64,
    repaired_len: u64,
    tail_sha256: String,
    quarantine_key: String,
}

impl TailRepairRecord {
    #[cfg(test)]
    fn prepared(
        segment_index: u64,
        original_len: u64,
        repaired_len: u64,
        tail: &[u8],
    ) -> GlobalLedgerResult<Self> {
        let tail_digest = Sha256::digest(tail);
        let repair_id = derive_repair_id(
            segment_index,
            original_len,
            repaired_len,
            tail_digest.as_slice(),
        );
        let quarantine_key = quarantine_key(&repair_id)?;
        Ok(Self {
            schema_version: REPAIR_SCHEMA_VERSION.to_string(),
            repair_id,
            state: RepairJournalState::Prepared,
            segment_index,
            original_len,
            repaired_len,
            tail_sha256: format!("sha256:{tail_digest:x}"),
            quarantine_key,
        })
    }

    #[cfg(test)]
    fn completed(&self) -> Self {
        let mut completed = self.clone();
        completed.state = RepairJournalState::Completed;
        completed
    }

    fn affected_bytes(&self) -> u64 {
        self.original_len - self.repaired_len
    }

    fn event_id(&self) -> GlobalLedgerResult<EventId> {
        let digest = self.repair_id.strip_prefix("sha256:").ok_or_else(|| {
            GlobalLedgerError::fatal(
                LedgerCode::InvalidRepairRecord.code(),
                LedgerLocation::DeriveRecoveryEventId.location(),
            )
        })?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::InvalidRepairRecord.code(),
                LedgerLocation::DeriveRecoveryEventId.location(),
            ));
        }
        let canonical = format!("evt_{}", &digest[..32]);
        serde_json::from_value(Value::String(canonical)).map_err(|_| {
            GlobalLedgerError::fatal(
                LedgerCode::InvalidRepairRecord.code(),
                LedgerLocation::DeriveRecoveryEventId.location(),
            )
        })
    }

    fn validate(&self) -> GlobalLedgerResult<()> {
        if self.schema_version != REPAIR_SCHEMA_VERSION {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::UnsupportedRepairSchema.code(),
                LedgerLocation::ValidateRepairRecord.location(),
            ));
        }
        if self.segment_index == 0 || self.original_len <= self.repaired_len {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::InvalidRepairRecord.code(),
                LedgerLocation::ValidateRepairLengths.location(),
            ));
        }
        let tail_digest = parse_sha256(&self.tail_sha256)?;
        let expected_id = derive_repair_id(
            self.segment_index,
            self.original_len,
            self.repaired_len,
            &tail_digest,
        );
        if self.repair_id != expected_id || self.quarantine_key != quarantine_key(&expected_id)? {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::InvalidRepairRecord.code(),
                LedgerLocation::ValidateRepairIdentity.location(),
            ));
        }
        self.event_id()?;
        Ok(())
    }

    fn same_definition(&self, other: &Self) -> bool {
        self.schema_version == other.schema_version
            && self.repair_id == other.repair_id
            && self.segment_index == other.segment_index
            && self.original_len == other.original_len
            && self.repaired_len == other.repaired_len
            && self.tail_sha256 == other.tail_sha256
            && self.quarantine_key == other.quarantine_key
    }
}

struct RepairProgress {
    prepared: TailRepairRecord,
    completed: bool,
}

pub(super) struct RepairRecordSnapshot {
    pub(super) schema_version: String,
    pub(super) repair_id: String,
    pub(super) completed: bool,
    pub(super) segment_index: u64,
    pub(super) original_len: u64,
    pub(super) repaired_len: u64,
    pub(super) tail_sha256: String,
    pub(super) quarantine_key: String,
}

#[derive(Default)]
pub(super) struct RepairJournal {
    repairs: BTreeMap<String, RepairProgress>,
}

impl RepairJournal {
    pub(super) fn load(root: &Path) -> GlobalLedgerResult<Self> {
        let path = root.join(REPAIR_JOURNAL_FILE);
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(GlobalLedgerError::io(
                    LedgerCode::LedgerIo.code(),
                    LedgerLocation::ReadRepairJournal.location(),
                    &error,
                ));
            }
        };
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        if bytes.last() != Some(&b'\n') {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::CorruptRepairJournal.code(),
                LedgerLocation::ParseRepairJournalTail.location(),
            ));
        }
        let mut journal = Self::default();
        for line in bytes[..bytes.len() - 1].split(|byte| *byte == b'\n') {
            if line.is_empty() {
                return Err(GlobalLedgerError::fatal(
                    LedgerCode::CorruptRepairJournal.code(),
                    LedgerLocation::ParseRepairJournalBlank.location(),
                ));
            }
            let unique = serde_json::from_slice::<UniqueJsonValue>(line).map_err(|error| {
                GlobalLedgerError::json(
                    LedgerCode::CorruptRepairJournal.code(),
                    LedgerLocation::ParseRepairJournal.location(),
                    &error,
                )
            })?;
            let record = serde_json::from_value::<TailRepairRecord>(unique.0).map_err(|error| {
                GlobalLedgerError::json(
                    LedgerCode::CorruptRepairJournal.code(),
                    LedgerLocation::DecodeRepairJournal.location(),
                    &error,
                )
            })?;
            journal.apply(record)?;
        }
        let mut unresolved_segments = BTreeSet::new();
        for progress in journal
            .repairs
            .values()
            .filter(|progress| !progress.completed)
        {
            if !unresolved_segments.insert(progress.prepared.segment_index) {
                return Err(GlobalLedgerError::fatal(
                    LedgerCode::RepairStateInconsistent.code(),
                    LedgerLocation::ValidateRepairSegments.location(),
                ));
            }
        }
        Ok(journal)
    }

    pub(super) fn snapshots(&self) -> Vec<RepairRecordSnapshot> {
        self.repairs
            .values()
            .map(|progress| RepairRecordSnapshot {
                schema_version: progress.prepared.schema_version.clone(),
                repair_id: progress.prepared.repair_id.clone(),
                completed: progress.completed,
                segment_index: progress.prepared.segment_index,
                original_len: progress.prepared.original_len,
                repaired_len: progress.prepared.repaired_len,
                tail_sha256: progress.prepared.tail_sha256.clone(),
                quarantine_key: progress.prepared.quarantine_key.clone(),
            })
            .collect()
    }

    fn apply(&mut self, record: TailRepairRecord) -> GlobalLedgerResult<()> {
        record.validate()?;
        match record.state {
            RepairJournalState::Prepared => {
                if self.repairs.contains_key(&record.repair_id) {
                    return Err(GlobalLedgerError::fatal(
                        LedgerCode::RepairStateInconsistent.code(),
                        LedgerLocation::ApplyRepairPrepared.location(),
                    ));
                }
                self.repairs.insert(
                    record.repair_id.clone(),
                    RepairProgress {
                        prepared: record,
                        completed: false,
                    },
                );
            }
            RepairJournalState::Completed => {
                let Some(progress) = self.repairs.get_mut(&record.repair_id) else {
                    return Err(GlobalLedgerError::fatal(
                        LedgerCode::RepairStateInconsistent.code(),
                        LedgerLocation::ApplyRepairCompleted.location(),
                    ));
                };
                if progress.completed || !progress.prepared.same_definition(&record) {
                    return Err(GlobalLedgerError::fatal(
                        LedgerCode::RepairStateInconsistent.code(),
                        LedgerLocation::ApplyRepairCompleted.location(),
                    ));
                }
                progress.completed = true;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn unresolved(&self) -> Vec<TailRepairRecord> {
        self.repairs
            .values()
            .filter(|progress| !progress.completed)
            .map(|progress| progress.prepared.clone())
            .collect()
    }

    #[cfg(test)]
    fn contains(&self, repair_id: &str) -> bool {
        self.repairs.contains_key(repair_id)
    }

    #[cfg(test)]
    fn prepare(&mut self, root: &Path, record: TailRepairRecord) -> GlobalLedgerResult<()> {
        append_repair_record(root, &record)?;
        self.apply(record)
    }

    #[cfg(test)]
    fn complete(&mut self, root: &Path, record: &TailRepairRecord) -> GlobalLedgerResult<()> {
        let completed = record.completed();
        append_repair_record(root, &completed)?;
        self.apply(completed)
    }
}

#[cfg(test)]
struct SegmentSnapshot {
    index: u64,
    path: PathBuf,
    bytes: Vec<u8>,
    complete_len: usize,
    is_last: bool,
}

#[cfg(test)]
fn recover_segments(
    root: &Path,
    segments_dir: &Path,
    verifier: &mut Option<&mut ArtifactVerifier<'_>>,
) -> GlobalLedgerResult<RecoveryState> {
    let segments = list_segments(segments_dir)?;
    let mut journal = RepairJournal::load(root)?;
    let snapshots = read_segment_snapshots(&segments)?;
    let mut next_sequence = 1_u64;
    let mut event_ids = BTreeSet::new();
    let mut records = Vec::new();
    for snapshot in &snapshots {
        parse_segment_records(snapshot, &mut next_sequence, &mut event_ids, &mut records)?;
    }

    let events = super::retention::restore_records(records, verifier, |_| Ok(()))?;

    let mut pending_repairs = Vec::new();
    for repair in journal.unresolved() {
        let event_id = repair.event_id()?;
        if let Some(event) = events.iter().find(|event| event.event_id() == &event_id) {
            verify_recovery_event(event, &repair)?;
            ensure_quarantine(root, &repair, None)?;
            journal.complete(root, &repair)?;
            repair_test_barrier("after_completion")?;
        } else {
            resume_prepared_repair(root, &snapshots, &repair)?;
            pending_repairs.push(repair);
        }
    }

    if let Some(last) = snapshots.last() {
        let bytes = fs::read(&last.path).map_err(|error| {
            GlobalLedgerError::io_untyped("ledger_io", "read_final_segment", &error)
        })?;
        let complete_len = complete_record_len(&bytes);
        if bytes.len() > complete_len {
            let record = TailRepairRecord::prepared(
                last.index,
                bytes.len() as u64,
                complete_len as u64,
                &bytes[complete_len..],
            )?;
            if journal.contains(&record.repair_id) {
                return Err(GlobalLedgerError::fatal_untyped(
                    "repair_state_inconsistent",
                    "discover_tail_repair",
                ));
            }
            journal.prepare(root, record.clone())?;
            repair_test_barrier("after_prepare")?;
            resume_prepared_repair(root, &snapshots, &record)?;
            pending_repairs.push(record);
        }
    }
    Ok(RecoveryState {
        next_sequence,
        events,
        last_segment_index: segments.last().map_or(1, |(index, _)| *index),
        pending_repairs,
    })
}

#[cfg(test)]
fn read_segment_snapshots(segments: &[(u64, PathBuf)]) -> GlobalLedgerResult<Vec<SegmentSnapshot>> {
    let mut snapshots = Vec::with_capacity(segments.len());
    for (position, (index, path)) in segments.iter().enumerate() {
        let is_last = position + 1 == segments.len();
        let bytes = fs::read(path)
            .map_err(|error| GlobalLedgerError::io_untyped("ledger_io", "read_segment", &error))?;
        let complete_len = complete_record_len(&bytes);
        if bytes.is_empty() && !is_last {
            return Err(GlobalLedgerError::fatal_untyped(
                "corrupt_segment",
                "recover_empty_non_final_segment",
            ));
        }
        if bytes.len() > complete_len && !is_last {
            return Err(GlobalLedgerError::fatal_untyped(
                "corrupt_segment",
                "recover_non_final_tail",
            ));
        }
        snapshots.push(SegmentSnapshot {
            index: *index,
            path: path.clone(),
            bytes,
            complete_len,
            is_last,
        });
    }
    Ok(snapshots)
}

#[cfg(test)]
fn parse_segment_records(
    snapshot: &SegmentSnapshot,
    next_sequence: &mut u64,
    event_ids: &mut BTreeSet<EventId>,
    records: &mut Vec<StoredEventRecord>,
) -> GlobalLedgerResult<()> {
    let complete_records = if snapshot.complete_len == 0 {
        &snapshot.bytes[..0]
    } else {
        &snapshot.bytes[..snapshot.complete_len - 1]
    };
    if complete_records.is_empty() && snapshot.complete_len > 0 {
        return Err(GlobalLedgerError::fatal_untyped(
            "corrupt_segment",
            "parse_blank_record",
        ));
    }
    if complete_records.is_empty() {
        return Ok(());
    }
    for line in complete_records.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            return Err(GlobalLedgerError::fatal_untyped(
                "corrupt_segment",
                "parse_blank_record",
            ));
        }
        let unique = serde_json::from_slice::<UniqueJsonValue>(line).map_err(|error| {
            GlobalLedgerError::json_untyped("corrupt_segment", "parse_segment", &error)
        })?;
        let schema_version = unique
            .0
            .get("event")
            .and_then(|event| event.get("schema_version"))
            .and_then(Value::as_str);
        if schema_version != Some(GLOBAL_EVENT_SCHEMA_VERSION) {
            return Err(GlobalLedgerError::fatal_untyped(
                "unsupported_event_schema",
                "recover_event_schema",
            ));
        }
        let stored = serde_json::from_value::<StoredLine>(unique.0).map_err(|error| {
            GlobalLedgerError::json_untyped("corrupt_segment", "decode_segment", &error)
        })?;
        if stored.line_type != LINE_TYPE {
            return Err(GlobalLedgerError::fatal_untyped(
                "corrupt_segment",
                "validate_line_type",
            ));
        }
        let event = stored.event.clone().into_metadata().map_err(|error| {
            GlobalLedgerError::fatal(
                error.code(),
                LedgerLocation::ValidatePersistedEvent.location(),
            )
        })?;
        if event.sequence() != *next_sequence {
            return Err(GlobalLedgerError::fatal_untyped(
                "sequence_discontinuity",
                "recover_sequence",
            ));
        }
        if !event_ids.insert(*event.event_id()) {
            return Err(GlobalLedgerError::fatal_untyped(
                "duplicate_event_id",
                "recover_event_ids",
            ));
        }
        records.push(stored.event);
        *next_sequence = increment_sequence(*next_sequence)?;
    }
    Ok(())
}

#[cfg(test)]
fn resume_prepared_repair(
    root: &Path,
    snapshots: &[SegmentSnapshot],
    repair: &TailRepairRecord,
) -> GlobalLedgerResult<()> {
    let Some(snapshot) = snapshots
        .iter()
        .find(|snapshot| snapshot.index == repair.segment_index)
    else {
        return Err(GlobalLedgerError::fatal_untyped(
            "repair_segment_missing",
            "resume_tail_repair",
        ));
    };
    if !snapshot.is_last {
        return Err(GlobalLedgerError::fatal_untyped(
            "repair_segment_not_final",
            "resume_tail_repair",
        ));
    }
    let bytes = fs::read(&snapshot.path).map_err(|error| {
        GlobalLedgerError::io_untyped("ledger_io", "read_repair_segment", &error)
    })?;
    match bytes.len() as u64 {
        length if length == repair.original_len => {
            let repaired_len = usize::try_from(repair.repaired_len).map_err(|_| {
                GlobalLedgerError::fatal_untyped("invalid_repair_record", "resume_tail_repair")
            })?;
            if repaired_len != complete_record_len(&bytes) {
                return Err(GlobalLedgerError::fatal_untyped(
                    "repair_tail_mismatch",
                    "resume_tail_repair",
                ));
            }
            let tail = &bytes[repaired_len..];
            verify_tail_hash(repair, tail)?;
            ensure_quarantine(root, repair, Some(tail))?;
            repair_test_barrier("after_quarantine")?;
            let file = OpenOptions::new()
                .write(true)
                .open(&snapshot.path)
                .map_err(|error| {
                    GlobalLedgerError::io_untyped("ledger_io", "open_segment_repair", &error)
                })?;
            file.set_len(repair.repaired_len).map_err(|error| {
                GlobalLedgerError::io_untyped("ledger_io", "truncate_final_tail", &error)
            })?;
            file.sync_all().map_err(|error| {
                GlobalLedgerError::io_untyped("ledger_io", "sync_tail_repair", &error)
            })?;
            repair_test_barrier("after_truncate")?;
        }
        length if length == repair.repaired_len => {
            if !bytes.is_empty() && bytes.last() != Some(&b'\n') {
                return Err(GlobalLedgerError::fatal_untyped(
                    "repair_tail_mismatch",
                    "resume_tail_repair",
                ));
            }
            ensure_quarantine(root, repair, None)?;
        }
        _ => {
            return Err(GlobalLedgerError::fatal_untyped(
                "repair_segment_length_mismatch",
                "resume_tail_repair",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
fn ensure_quarantine(
    root: &Path,
    repair: &TailRepairRecord,
    tail: Option<&[u8]>,
) -> GlobalLedgerResult<()> {
    let path = root.join(&repair.quarantine_key);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            GlobalLedgerError::io_untyped("ledger_io", "create_quarantine", &error)
        })?;
    }
    match fs::read(&path) {
        Ok(existing) => {
            verify_tail_hash(repair, &existing)?;
            if tail.is_some_and(|tail| tail != existing) {
                return Err(GlobalLedgerError::fatal_untyped(
                    "quarantine_mismatch",
                    "verify_quarantine",
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let Some(tail) = tail else {
                return Err(GlobalLedgerError::fatal_untyped(
                    "quarantine_missing",
                    "verify_quarantine",
                ));
            };
            verify_tail_hash(repair, tail)?;
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(path)
                .map_err(|error| {
                    GlobalLedgerError::io_untyped("ledger_io", "create_quarantine_file", &error)
                })?;
            file.write_all(tail).map_err(|error| {
                GlobalLedgerError::io_untyped("ledger_io", "write_quarantine", &error)
            })?;
            file.sync_all().map_err(|error| {
                GlobalLedgerError::io_untyped("ledger_io", "sync_quarantine", &error)
            })
        }
        Err(error) => Err(GlobalLedgerError::io_untyped(
            "ledger_io",
            "read_quarantine",
            &error,
        )),
    }
}

fn verify_tail_hash(repair: &TailRepairRecord, tail: &[u8]) -> GlobalLedgerResult<()> {
    if tail.len() as u64 != repair.affected_bytes()
        || format!("sha256:{:x}", Sha256::digest(tail)) != repair.tail_sha256
    {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::RepairTailMismatch.code(),
            LedgerLocation::VerifyRepairTail.location(),
        ));
    }
    Ok(())
}

fn verify_recovery_event(
    event: &PersistedEvent,
    repair: &TailRepairRecord,
) -> GlobalLedgerResult<()> {
    let expected_id = repair.event_id()?;
    let payload_valid = matches!(
        event.payload(),
        EventPayload::Ledger(LedgerPayload::Recovered(payload))
            if payload.reason() == RecoveryReason::TruncatedFinalTail
                && payload.segment_index() == Some(repair.segment_index)
                && payload.affected_bytes() == repair.affected_bytes()
                && payload.audit().account_fingerprint().is_none()
                && !payload.audit().authentication_redacted()
                && payload.audit().machine_path().is_none()
                && payload.audit().device_endpoint().is_none()
    );
    if event.event_id() != &expected_id
        || event.event_type() != EventType::LedgerRecovered
        || event.severity() != record_severity(EventSeverity::Info, event.payload())
        || event.sensitivity() != Sensitivity::Public
        || event.origin()
            != &EventOrigin::new(
                EventSource::System,
                OriginModule::GlobalLedger,
                EventActor::System,
            )
        || event.links() != &EventLinks::default()
        || !event.artifacts().is_empty()
        || !payload_valid
    {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::RepairRecoveryEventMismatch.code(),
            LedgerLocation::VerifyRecoveryEvent.location(),
        ));
    }
    Ok(())
}

#[cfg(test)]
fn append_repair_record(root: &Path, record: &TailRepairRecord) -> GlobalLedgerResult<()> {
    record.validate()?;
    let mut bytes = serde_json::to_vec(record).map_err(|error| {
        GlobalLedgerError::json_untyped(
            "repair_serialization_failed",
            "serialize_repair_record",
            &error,
        )
    })?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(REPAIR_JOURNAL_FILE))
        .map_err(|error| {
            GlobalLedgerError::io_untyped("ledger_io", "open_repair_journal", &error)
        })?;
    file.write_all(&bytes).map_err(|error| {
        GlobalLedgerError::io_untyped("ledger_io", "write_repair_journal", &error)
    })?;
    file.sync_all()
        .map_err(|error| GlobalLedgerError::io_untyped("ledger_io", "sync_repair_journal", &error))
}

fn derive_repair_id(
    segment_index: u64,
    original_len: u64,
    repaired_len: u64,
    tail_digest: &[u8],
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"actingcommand.ledger-tail-repair.v1");
    digest.update(segment_index.to_be_bytes());
    digest.update(original_len.to_be_bytes());
    digest.update(repaired_len.to_be_bytes());
    digest.update(tail_digest);
    format!("sha256:{:x}", digest.finalize())
}

fn parse_sha256(value: &str) -> GlobalLedgerResult<[u8; 32]> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::InvalidRepairRecord.code(),
            LedgerLocation::ParseRepairHash.location(),
        ));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::InvalidRepairRecord.code(),
            LedgerLocation::ParseRepairHash.location(),
        ));
    }
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let pair = &hex.as_bytes()[index * 2..index * 2 + 2];
        *byte = decode_lower_hex(pair[0]) * 16 + decode_lower_hex(pair[1]);
    }
    Ok(bytes)
}

fn decode_lower_hex(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => unreachable!("validated lowercase hex"),
    }
}

fn quarantine_key(repair_id: &str) -> GlobalLedgerResult<String> {
    let digest = repair_id.strip_prefix("sha256:").ok_or_else(|| {
        GlobalLedgerError::fatal(
            LedgerCode::InvalidRepairRecord.code(),
            LedgerLocation::DeriveQuarantineKey.location(),
        )
    })?;
    if digest.len() != 64 {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::InvalidRepairRecord.code(),
            LedgerLocation::DeriveQuarantineKey.location(),
        ));
    }
    Ok(format!("quarantine/{digest}.bin"))
}

pub(super) fn complete_record_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1)
}

#[cfg(test)]
pub(super) fn repair_test_barrier(stage: &str) -> GlobalLedgerResult<()> {
    if std::env::var("ACTINGCOMMAND_TEST_REPAIR_FAILPOINT").as_deref() != Ok(stage) {
        return Ok(());
    }
    let ready = std::env::var_os("ACTINGCOMMAND_TEST_REPAIR_READY").ok_or_else(|| {
        GlobalLedgerError::fatal_untyped("repair_test_barrier_invalid", "repair_test_barrier")
    })?;
    let mut file = File::create(PathBuf::from(ready)).map_err(|error| {
        GlobalLedgerError::io_untyped("ledger_io", "repair_test_barrier", &error)
    })?;
    file.write_all(stage.as_bytes()).map_err(|error| {
        GlobalLedgerError::io_untyped("ledger_io", "repair_test_barrier", &error)
    })?;
    file.sync_all().map_err(|error| {
        GlobalLedgerError::io_untyped("ledger_io", "repair_test_barrier", &error)
    })?;
    loop {
        std::thread::park_timeout(std::time::Duration::from_secs(60));
    }
}

pub(super) fn list_segments(segments_dir: &Path) -> GlobalLedgerResult<Vec<(u64, PathBuf)>> {
    let mut segments = Vec::new();
    for entry in fs::read_dir(segments_dir).map_err(|error| {
        GlobalLedgerError::io(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::ListSegments.location(),
            &error,
        )
    })? {
        let path = entry
            .map_err(|error| {
                GlobalLedgerError::io(
                    LedgerCode::LedgerIo.code(),
                    LedgerLocation::ReadSegmentEntry.location(),
                    &error,
                )
            })?
            .path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(index) = name
            .strip_prefix("segment-")
            .and_then(|name| name.strip_suffix(".jsonl"))
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        segments.push((index, path));
    }
    segments.sort_by_key(|(index, _)| *index);
    for (expected, (actual, _)) in (1_u64..).zip(&segments) {
        if expected != *actual {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::SegmentDiscontinuity.code(),
                LedgerLocation::ListSegments.location(),
            ));
        }
    }
    Ok(segments)
}

#[cfg(test)]
fn segment_path(segments_dir: &Path, index: u64) -> PathBuf {
    segments_dir.join(format!("segment-{index:06}.jsonl"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WriterMetadata {
    pub(super) schema_version: String,
    pub(super) owner_id: String,
    pub(super) pid: u32,
    pub(super) active: bool,
    pub(super) started_at_unix_ms: u64,
    pub(super) closed_at_unix_ms: Option<u64>,
}

pub(super) struct WriterOwnership {
    newly_created: bool,
    file: File,
    metadata: WriterMetadata,
    closed: bool,
    compatibility_lock: Option<LockedWriterFile>,
}

pub(super) fn verify_completed_repairs(
    root: &Path,
    events: &[PersistedEvent],
) -> GlobalLedgerResult<()> {
    let journal = RepairJournal::load(root)?;
    for progress in journal.repairs.values() {
        if !progress.completed {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::RepairStateInconsistent.code(),
                LedgerLocation::ValidateImportRepairs.location(),
            ));
        }
        let repair = &progress.prepared;
        let id = repair.event_id()?;
        let event = events
            .iter()
            .find(|event| *event.event_id() == id)
            .ok_or_else(|| {
                GlobalLedgerError::fatal(
                    LedgerCode::RepairRecoveryEventMissing.code(),
                    LedgerLocation::ValidateImportRepairs.location(),
                )
            })?;
        verify_recovery_event(event, repair)?;
        let tail = fs::read(root.join(&repair.quarantine_key)).map_err(|error| {
            GlobalLedgerError::io(
                LedgerCode::LedgerIo.code(),
                LedgerLocation::VerifyImportQuarantine.location(),
                &error,
            )
        })?;
        verify_tail_hash(repair, &tail)?;
    }
    Ok(())
}

/// Acquires the existing OS lock without repairing or appending its journal.
pub(super) struct LockedWriterFile {
    file: File,
    created: bool,
    previous: Option<WriterMetadata>,
}

impl LockedWriterFile {
    pub(super) fn newly_created(&self) -> bool {
        self.created
    }
    pub(super) fn bytes(&self) -> GlobalLedgerResult<Vec<u8>> {
        let mut file = &self.file;
        file.rewind().map_err(|error| {
            GlobalLedgerError::io(
                LedgerCode::LedgerIo.code(),
                LedgerLocation::SeekLockedWriterMetadata.location(),
                &error,
            )
        })?;
        let mut bytes = Vec::new();
        file.take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                GlobalLedgerError::io(
                    LedgerCode::LedgerIo.code(),
                    LedgerLocation::ReadLockedWriterMetadata.location(),
                    &error,
                )
            })?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::WriterMetadataTooLarge.code(),
                LedgerLocation::ReadLockedWriterMetadata.location(),
            ));
        }
        Ok(bytes)
    }
    pub(super) fn open(path: &Path, create: bool) -> GlobalLedgerResult<Self> {
        let existing = match fs::symlink_metadata(path) {
            Ok(metadata) => {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    metadata.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse = false;
                if !metadata.is_file() || metadata.file_type().is_symlink() || reparse {
                    return Err(GlobalLedgerError::fatal(
                        LedgerCode::WriterLockUnsafe.code(),
                        LedgerLocation::LockWriterWithoutMetadataWrite.location(),
                    ));
                }
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => false,
            Err(error) => {
                return Err(GlobalLedgerError::io(
                    LedgerCode::LedgerIo.code(),
                    LedgerLocation::InspectWriterLock.location(),
                    &error,
                ));
            }
        };
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(!existing)
            .open(path)
            .map_err(|error| {
                GlobalLedgerError::io(
                    LedgerCode::LedgerIo.code(),
                    LedgerLocation::OpenMaintenanceWriterLock.location(),
                    &error,
                )
            })?;
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => GlobalLedgerError::fatal(
                LedgerCode::WriterConflict.code(),
                LedgerLocation::LockWriterWithoutMetadataWrite.location(),
            ),
            std::fs::TryLockError::Error(error) => GlobalLedgerError::io(
                LedgerCode::LedgerIo.code(),
                LedgerLocation::LockWriterWithoutMetadataWrite.location(),
                &error,
            ),
        })?;
        let length = file
            .metadata()
            .map_err(|error| {
                GlobalLedgerError::io(
                    LedgerCode::LedgerIo.code(),
                    LedgerLocation::InspectLockedWriterMetadata.location(),
                    &error,
                )
            })?
            .len();
        if length > 4 * 1024 * 1024 {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::WriterMetadataTooLarge.code(),
                LedgerLocation::ReadLockedWriterMetadata.location(),
            ));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(length + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                GlobalLedgerError::io(
                    LedgerCode::LedgerIo.code(),
                    LedgerLocation::ReadLockedWriterMetadata.location(),
                    &error,
                )
            })?;
        if bytes.len() as u64 != length {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::WriterMetadataChanged.code(),
                LedgerLocation::ReadLockedWriterMetadata.location(),
            ));
        }
        let mut previous = None;
        if !bytes.is_empty() {
            if bytes.last() != Some(&b'\n') {
                return Err(GlobalLedgerError::fatal(
                    LedgerCode::MalformedOwnerMetadata.code(),
                    LedgerLocation::ReadLockedWriterMetadata.location(),
                ));
            }
            for line in bytes[..bytes.len() - 1].split(|byte| *byte == b'\n') {
                previous = Some(parse_writer_metadata(line)?);
            }
        } else if existing {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::MalformedOwnerMetadata.code(),
                LedgerLocation::ReadLockedWriterMetadata.location(),
            ));
        }
        Ok(Self {
            file,
            created: !existing,
            previous,
        })
    }

    pub(super) fn require_closed(&self) -> GlobalLedgerResult<()> {
        if self
            .previous
            .as_ref()
            .is_some_and(|metadata| metadata.active)
        {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::SourceWriterNotClosed.code(),
                LedgerLocation::ValidateMaintenanceWriter.location(),
            ));
        }
        Ok(())
    }

    pub(super) fn close(self) -> GlobalLedgerResult<()> {
        self.file.unlock().map_err(|error| {
            GlobalLedgerError::io(
                LedgerCode::LedgerIo.code(),
                LedgerLocation::UnlockMaintenanceWriter.location(),
                &error,
            )
        })
    }
}

impl WriterOwnership {
    pub(super) fn acquire(
        root: &Path,
        owner_id: &str,
    ) -> GlobalLedgerResult<(Self, Option<String>)> {
        let path = root.join("writer.lock");
        let (mut file, created) = match OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
        {
            Ok(file) => (file, true),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .map_err(|error| {
                        GlobalLedgerError::io(
                            LedgerCode::LedgerIo.code(),
                            LedgerLocation::OpenWriterLock.location(),
                            &error,
                        )
                    })?;
                (file, false)
            }
            Err(error) => {
                return Err(GlobalLedgerError::io(
                    LedgerCode::LedgerIo.code(),
                    LedgerLocation::CreateWriterLock.location(),
                    &error,
                ));
            }
        };
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => GlobalLedgerError::fatal(
                LedgerCode::WriterConflict.code(),
                LedgerLocation::AcquireWriterLock.location(),
            ),
            std::fs::TryLockError::Error(error) => GlobalLedgerError::io(
                LedgerCode::LedgerIo.code(),
                LedgerLocation::AcquireWriterLock.location(),
                &error,
            ),
        })?;
        let previous = read_writer_metadata(&mut file, created)?;
        let stale_owner = previous
            .as_ref()
            .filter(|metadata| metadata.active)
            .map(|metadata| metadata.owner_id.clone());
        let metadata = WriterMetadata {
            schema_version: WRITER_SCHEMA_VERSION.to_string(),
            owner_id: owner_id.to_string(),
            pid: process::id(),
            active: true,
            started_at_unix_ms: unix_ms_now()?,
            closed_at_unix_ms: None,
        };
        write_writer_metadata(&mut file, &metadata)?;
        Ok((
            Self {
                newly_created: created,
                file,
                metadata,
                closed: false,
                compatibility_lock: None,
            },
            stale_owner,
        ))
    }

    pub(super) fn is_new(&self) -> bool {
        self.newly_created
    }

    pub(super) fn from_locked(
        mut lock: LockedWriterFile,
        compatibility_lock: Option<LockedWriterFile>,
        owner_id: &str,
    ) -> GlobalLedgerResult<(Self, Option<String>)> {
        let stale = lock
            .previous
            .as_ref()
            .filter(|metadata| metadata.active)
            .map(|metadata| metadata.owner_id.clone());
        let metadata = WriterMetadata {
            schema_version: WRITER_SCHEMA_VERSION.into(),
            owner_id: owner_id.into(),
            pid: process::id(),
            active: true,
            started_at_unix_ms: unix_ms_now()?,
            closed_at_unix_ms: None,
        };
        write_writer_metadata(&mut lock.file, &metadata)?;
        Ok((
            Self {
                newly_created: lock.created,
                file: lock.file,
                metadata,
                closed: false,
                compatibility_lock,
            },
            stale,
        ))
    }

    pub(super) fn close(&mut self) -> GlobalLedgerResult<()> {
        if self.closed {
            return Ok(());
        }
        self.metadata.active = false;
        self.metadata.closed_at_unix_ms = Some(unix_ms_now()?);
        write_writer_metadata(&mut self.file, &self.metadata)?;
        self.file.unlock().map_err(|error| {
            GlobalLedgerError::io(
                LedgerCode::LedgerIo.code(),
                LedgerLocation::ReleaseWriterLock.location(),
                &error,
            )
        })?;
        self.closed = true;
        if let Some(lock) = self.compatibility_lock.take() {
            lock.close()?;
        }
        Ok(())
    }
}

fn read_writer_metadata(
    file: &mut File,
    created: bool,
) -> GlobalLedgerResult<Option<WriterMetadata>> {
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        GlobalLedgerError::io(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::SeekWriterMetadata.location(),
            &error,
        )
    })?;
    let mut content = Vec::new();
    file.read_to_end(&mut content).map_err(|error| {
        GlobalLedgerError::io(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::ReadWriterMetadata.location(),
            &error,
        )
    })?;
    if content.is_empty() {
        return if created {
            Ok(None)
        } else {
            Err(GlobalLedgerError::fatal(
                LedgerCode::MalformedOwnerMetadata.code(),
                LedgerLocation::ParseWriterMetadata.location(),
            ))
        };
    }

    let (complete, tail_len) = match content.iter().rposition(|byte| *byte == b'\n') {
        Some(last_newline) => (&content[..=last_newline], content.len() - last_newline - 1),
        None => (&content[..], 0),
    };
    let records = if complete.last() == Some(&b'\n') {
        &complete[..complete.len() - 1]
    } else {
        complete
    };
    if records.is_empty() {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::MalformedOwnerMetadata.code(),
            LedgerLocation::ParseWriterMetadata.location(),
        ));
    }
    let mut metadata = None;
    for record in records.split(|byte| *byte == b'\n') {
        if record.is_empty() {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::MalformedOwnerMetadata.code(),
                LedgerLocation::ParseWriterMetadata.location(),
            ));
        }
        metadata = Some(parse_writer_metadata(record)?);
    }
    if tail_len > 0 {
        file.set_len(complete.len() as u64).map_err(|error| {
            GlobalLedgerError::io(
                LedgerCode::LedgerIo.code(),
                LedgerLocation::TruncateWriterMetadataTail.location(),
                &error,
            )
        })?;
        file.sync_all().map_err(|error| {
            GlobalLedgerError::io(
                LedgerCode::LedgerIo.code(),
                LedgerLocation::SyncWriterMetadataTail.location(),
                &error,
            )
        })?;
    }
    Ok(metadata)
}

pub(super) fn parse_writer_metadata(record: &[u8]) -> GlobalLedgerResult<WriterMetadata> {
    let unique = serde_json::from_slice::<UniqueJsonValue>(record).map_err(|error| {
        GlobalLedgerError::json(
            LedgerCode::MalformedOwnerMetadata.code(),
            LedgerLocation::ParseWriterMetadata.location(),
            &error,
        )
    })?;
    let metadata = serde_json::from_value::<WriterMetadata>(unique.0).map_err(|error| {
        GlobalLedgerError::json(
            LedgerCode::MalformedOwnerMetadata.code(),
            LedgerLocation::DecodeWriterMetadata.location(),
            &error,
        )
    })?;
    if metadata.schema_version != WRITER_SCHEMA_VERSION {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::UnsupportedWriterSchema.code(),
            LedgerLocation::ValidateWriterMetadata.location(),
        ));
    }
    let lifecycle_valid = match (metadata.active, metadata.closed_at_unix_ms) {
        (true, None) => true,
        (false, Some(closed_at)) => closed_at > 0,
        _ => false,
    };
    let valid = is_identifier(&metadata.owner_id)
        && metadata.pid > 0
        && metadata.started_at_unix_ms > 0
        && lifecycle_valid;
    if !valid {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::MalformedOwnerMetadata.code(),
            LedgerLocation::ValidateWriterMetadata.location(),
        ));
    }
    Ok(metadata)
}

fn write_writer_metadata(file: &mut File, metadata: &WriterMetadata) -> GlobalLedgerResult<()> {
    let mut bytes = serde_json::to_vec(metadata).map_err(|error| {
        GlobalLedgerError::json(
            LedgerCode::OwnerMetadataSerializationFailed.code(),
            LedgerLocation::SerializeWriterMetadata.location(),
            &error,
        )
    })?;
    bytes.push(b'\n');
    let end = file.seek(SeekFrom::End(0)).map_err(|error| {
        GlobalLedgerError::io(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::SeekWriterMetadata.location(),
            &error,
        )
    })?;
    if end > 0 {
        file.seek(SeekFrom::End(-1)).map_err(|error| {
            GlobalLedgerError::io(
                LedgerCode::LedgerIo.code(),
                LedgerLocation::SeekWriterMetadataTail.location(),
                &error,
            )
        })?;
        let mut last = [0_u8; 1];
        file.read_exact(&mut last).map_err(|error| {
            GlobalLedgerError::io(
                LedgerCode::LedgerIo.code(),
                LedgerLocation::ReadWriterMetadataTail.location(),
                &error,
            )
        })?;
        file.seek(SeekFrom::End(0)).map_err(|error| {
            GlobalLedgerError::io(
                LedgerCode::LedgerIo.code(),
                LedgerLocation::SeekWriterMetadataAppend.location(),
                &error,
            )
        })?;
        if last[0] != b'\n' {
            file.write_all(b"\n").map_err(|error| {
                GlobalLedgerError::io(
                    LedgerCode::LedgerIo.code(),
                    LedgerLocation::CommitLegacyWriterMetadata.location(),
                    &error,
                )
            })?;
        }
    }
    file.write_all(&bytes).map_err(|error| {
        GlobalLedgerError::io(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::WriteWriterMetadata.location(),
            &error,
        )
    })?;
    file.sync_all().map_err(|error| {
        GlobalLedgerError::io(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::SyncWriterMetadata.location(),
            &error,
        )
    })?;
    Ok(())
}

fn unix_ms_now() -> GlobalLedgerResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .map_err(|_| {
            GlobalLedgerError::fatal(
                LedgerCode::ClockBeforeEpoch.code(),
                LedgerLocation::ReadClock.location(),
            )
        })
}

pub(super) fn increment_sequence(sequence: u64) -> GlobalLedgerResult<u64> {
    sequence.checked_add(1).ok_or_else(|| {
        GlobalLedgerError::fatal(
            LedgerCode::SequenceExhausted.code(),
            LedgerLocation::IncrementSequence.location(),
        )
    })
}

pub(super) struct UniqueJsonValue(pub(super) Value);

impl<'de> Deserialize<'de> for UniqueJsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueJsonVisitor)
    }
}

struct UniqueJsonVisitor;

impl<'de> Visitor<'de> for UniqueJsonVisitor {
    type Value = UniqueJsonValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JSON without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::Number(Number::from(value))))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::Number(Number::from(value))))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueJsonValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_string(value.to_string())
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::Null))
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        UniqueJsonValue::deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<UniqueJsonValue>()? {
            values.push(value.0);
        }
        Ok(UniqueJsonValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON key"));
            }
            let value = object.next_value::<UniqueJsonValue>()?;
            values.insert(key, value.0);
        }
        Ok(UniqueJsonValue(Value::Object(values)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn existing_empty_writer_metadata_is_not_treated_as_first_use() {
        let temp = TempDir::new().expect("temp");
        fs::write(temp.path().join("writer.lock"), []).expect("empty metadata");

        let error = match WriterOwnership::acquire(temp.path(), "new-owner") {
            Ok(_) => panic!("existing empty metadata must be fatal"),
            Err(error) => error,
        };

        assert_eq!(error.code(), "malformed_owner_metadata");
    }

    #[test]
    fn interrupted_metadata_append_preserves_the_last_active_owner() {
        let temp = TempDir::new().expect("temp");
        let (mut first, stale) =
            WriterOwnership::acquire(temp.path(), "previous-owner").expect("first owner");
        assert!(stale.is_none());
        first
            .file
            .seek(SeekFrom::End(0))
            .expect("seek metadata tail");
        first
            .file
            .write_all(br#"{"schema_version":"actingcommand.ledger-writer.v2"#)
            .expect("partial metadata append");
        first.file.sync_all().expect("sync partial append");
        drop(first);

        let (mut replacement, stale) =
            WriterOwnership::acquire(temp.path(), "replacement-owner").expect("replacement owner");

        assert_eq!(stale.as_deref(), Some("previous-owner"));
        replacement.close().expect("close replacement owner");
    }

    #[test]
    fn clean_close_appends_inactive_metadata_without_erasing_active_record() {
        let temp = TempDir::new().expect("temp");
        let (mut ownership, _) =
            WriterOwnership::acquire(temp.path(), "writer-one").expect("owner");
        ownership
            .file
            .seek(SeekFrom::Start(0))
            .expect("seek active metadata");
        let mut active_bytes = Vec::new();
        ownership
            .file
            .read_to_end(&mut active_bytes)
            .expect("read active metadata");

        ownership.close().expect("close owner");
        let closed_bytes = fs::read(temp.path().join("writer.lock")).expect("closed metadata");

        assert!(closed_bytes.starts_with(&active_bytes));
        assert!(closed_bytes.len() > active_bytes.len());
    }

    #[test]
    fn sequence_increment_fails_at_u64_max() {
        let error = increment_sequence(u64::MAX).expect_err("sequence must not wrap or repeat");

        assert_eq!(error.code(), "sequence_exhausted");
    }
}
