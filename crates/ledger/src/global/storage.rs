// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    GlobalLedgerConfig, GlobalLedgerError, GlobalLedgerResult, Sha256SecretFingerprinter,
    is_identifier, projection::EventIndexes,
};
use crate::PersistedEvent;
use crate::fact::StoredEventRecord;
use actingcommand_contract::{
    AuditInput, EventActor, EventDraft, EventId, EventLinks, EventLinksDraft, EventOrigin,
    EventPayload, EventSeverity, EventSource, EventType, GLOBAL_EVENT_SCHEMA_VERSION,
    IdentifierIssuer, LedgerPayload, LedgerPayloadDraft, OriginModule, ProjectedArtifactReference,
    RecoveryReason, SanitizedEventDraft, Sensitivity, VerifiedArtifactReference,
};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

const WRITER_SCHEMA_VERSION: &str = "actingcommand.ledger-writer.v2";
const REPAIR_SCHEMA_VERSION: &str = "actingcommand.ledger-repair.v1";
const REPAIR_JOURNAL_FILE: &str = "repair-journal.jsonl";
const LINE_TYPE: &str = "event";

type ArtifactVerifier<'a> =
    dyn FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference> + 'a;

pub(super) struct SegmentStore {
    root: PathBuf,
    segments_dir: PathBuf,
    ownership: WriterOwnership,
    segment_max_bytes: u64,
    active_index: u64,
    active_bytes: u64,
    active_file: File,
    next_sequence: u64,
    events: Vec<PersistedEvent>,
    indexes: EventIndexes,
}

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
        fs::create_dir_all(&config.root)
            .map_err(|error| GlobalLedgerError::io("ledger_io", "create_ledger_root", &error))?;
        let segments_dir = config.root.join("segments");
        fs::create_dir_all(&segments_dir)
            .map_err(|error| GlobalLedgerError::io("ledger_io", "create_segments", &error))?;
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
                return Err(GlobalLedgerError::io(
                    "ledger_io",
                    "open_active_segment",
                    &error,
                ));
            }
        };
        let active_bytes = active_file
            .metadata()
            .map_err(|error| GlobalLedgerError::io("ledger_io", "stat_segment", &error))?
            .len();
        let mut store = Self {
            root: config.root.clone(),
            segments_dir,
            ownership,
            segment_max_bytes: config.segment_max_bytes,
            active_index,
            active_bytes,
            active_file,
            next_sequence,
            indexes: EventIndexes::from_events(&events),
            events,
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
                append_repair_record(&store.root, &repair.completed())?;
                repair_test_barrier("after_completion")?;
            }
            Ok(())
        })();
        if let Err(error) = recovery_result {
            store.ownership.close()?;
            return Err(error);
        }
        Ok(store)
    }

    pub(super) fn append(
        &mut self,
        draft: SanitizedEventDraft,
    ) -> GlobalLedgerResult<PersistedEvent> {
        self.append_with_event_id(draft, None)
    }

    fn append_with_event_id(
        &mut self,
        draft: SanitizedEventDraft,
        event_id: Option<EventId>,
    ) -> GlobalLedgerResult<PersistedEvent> {
        let following_sequence = increment_sequence(self.next_sequence)?;
        let event = match event_id {
            Some(event_id) => {
                PersistedEvent::from_sanitized_with_event_id(self.next_sequence, draft, event_id)
            }
            None => PersistedEvent::from_sanitized(self.next_sequence, draft),
        }
        .map_err(|error| GlobalLedgerError::request(error.code(), "validate_sanitized_event"))?;
        if self.indexes.contains_event_id(event.event_id()) {
            return Err(GlobalLedgerError::request(
                "duplicate_event_id",
                "append_event",
            ));
        }
        let mut bytes = serde_json::to_vec(&StoredLine {
            line_type: LINE_TYPE.to_string(),
            event: StoredEventRecord::from_event(&event),
        })
        .map_err(|error| {
            GlobalLedgerError::json("event_serialization_failed", "serialize_event", &error)
        })?;
        bytes.push(b'\n');
        if self.active_bytes > 0
            && self.active_bytes.saturating_add(bytes.len() as u64) > self.segment_max_bytes
        {
            self.rotate()?;
        }
        self.active_file
            .write_all(&bytes)
            .map_err(|error| GlobalLedgerError::io("ledger_io", "append_event", &error))?;
        self.active_file
            .sync_all()
            .map_err(|error| GlobalLedgerError::io("ledger_io", "sync_event", &error))?;
        self.active_bytes = self.active_bytes.saturating_add(bytes.len() as u64);
        self.next_sequence = following_sequence;
        self.indexes.insert(&event, self.events.len());
        self.events.push(event.clone());
        Ok(event)
    }

    pub(super) fn query(&self, query: &actingcommand_contract::EventQuery) -> Vec<PersistedEvent> {
        self.indexes.query(&self.events, query)
    }

    pub(super) fn query_page(
        &self,
        query: &actingcommand_contract::EventQuery,
        after_sequence: u64,
        through_sequence: u64,
        page_events: usize,
    ) -> Vec<PersistedEvent> {
        self.indexes.query_page(
            &self.events,
            query,
            after_sequence,
            through_sequence,
            page_events,
        )
    }

    pub(super) fn latest_sequence(&self) -> u64 {
        self.events.last().map_or(0, PersistedEvent::sequence)
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
        self.events[start..]
            .iter()
            .take_while(|event| event.sequence() <= through_sequence)
            .take(page_events)
            .cloned()
            .collect()
    }

    pub(super) fn close(mut self) -> GlobalLedgerResult<()> {
        self.active_file
            .sync_all()
            .map_err(|error| GlobalLedgerError::io("ledger_io", "sync_on_close", &error))?;
        self.ownership.close()
    }

    fn rotate(&mut self) -> GlobalLedgerResult<()> {
        self.active_file
            .sync_all()
            .map_err(|error| GlobalLedgerError::io("ledger_io", "sync_before_rotate", &error))?;
        self.active_index = self.active_index.saturating_add(1);
        let path = segment_path(&self.segments_dir, self.active_index);
        self.active_file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .read(true)
            .open(path)
            .map_err(|error| GlobalLedgerError::io("ledger_io", "create_segment", &error))?;
        self.active_bytes = 0;
        Ok(())
    }

    fn append_recovery(
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
            GlobalLedgerError::fatal("recovery_event_failed", "create_identifier_issuer")
        })?;
        let draft = EventDraft::new(
            identifiers.mint_event_id().map_err(|_| {
                GlobalLedgerError::fatal("recovery_event_failed", "issue_recovery_event_id")
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
                GlobalLedgerError::fatal("recovery_event_failed", "configure_fingerprinter")
            })?,
        )
        .map_err(|_| {
            GlobalLedgerError::fatal("recovery_event_failed", "sanitize_recovery_event")
        })?;
        self.append_with_event_id(draft, event_id)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLine {
    line_type: String,
    event: StoredEventRecord,
}

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
            GlobalLedgerError::fatal("invalid_repair_record", "derive_recovery_event_id")
        })?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(GlobalLedgerError::fatal(
                "invalid_repair_record",
                "derive_recovery_event_id",
            ));
        }
        let canonical = format!("evt_{}", &digest[..32]);
        serde_json::from_value(Value::String(canonical)).map_err(|_| {
            GlobalLedgerError::fatal("invalid_repair_record", "derive_recovery_event_id")
        })
    }

    fn validate(&self) -> GlobalLedgerResult<()> {
        if self.schema_version != REPAIR_SCHEMA_VERSION {
            return Err(GlobalLedgerError::fatal(
                "unsupported_repair_schema",
                "validate_repair_record",
            ));
        }
        if self.segment_index == 0 || self.original_len <= self.repaired_len {
            return Err(GlobalLedgerError::fatal(
                "invalid_repair_record",
                "validate_repair_lengths",
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
                "invalid_repair_record",
                "validate_repair_identity",
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

#[derive(Default)]
struct RepairJournal {
    repairs: BTreeMap<String, RepairProgress>,
}

impl RepairJournal {
    fn load(root: &Path) -> GlobalLedgerResult<Self> {
        let path = root.join(REPAIR_JOURNAL_FILE);
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(GlobalLedgerError::io(
                    "ledger_io",
                    "read_repair_journal",
                    &error,
                ));
            }
        };
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        if bytes.last() != Some(&b'\n') {
            return Err(GlobalLedgerError::fatal(
                "corrupt_repair_journal",
                "parse_repair_journal_tail",
            ));
        }
        let mut journal = Self::default();
        for line in bytes[..bytes.len() - 1].split(|byte| *byte == b'\n') {
            if line.is_empty() {
                return Err(GlobalLedgerError::fatal(
                    "corrupt_repair_journal",
                    "parse_repair_journal_blank",
                ));
            }
            let unique = serde_json::from_slice::<UniqueJsonValue>(line).map_err(|error| {
                GlobalLedgerError::json("corrupt_repair_journal", "parse_repair_journal", &error)
            })?;
            let record = serde_json::from_value::<TailRepairRecord>(unique.0).map_err(|error| {
                GlobalLedgerError::json("corrupt_repair_journal", "decode_repair_journal", &error)
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
                    "repair_state_inconsistent",
                    "validate_repair_segments",
                ));
            }
        }
        Ok(journal)
    }

    fn apply(&mut self, record: TailRepairRecord) -> GlobalLedgerResult<()> {
        record.validate()?;
        match record.state {
            RepairJournalState::Prepared => {
                if self.repairs.contains_key(&record.repair_id) {
                    return Err(GlobalLedgerError::fatal(
                        "repair_state_inconsistent",
                        "apply_repair_prepared",
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
                        "repair_state_inconsistent",
                        "apply_repair_completed",
                    ));
                };
                if progress.completed || !progress.prepared.same_definition(&record) {
                    return Err(GlobalLedgerError::fatal(
                        "repair_state_inconsistent",
                        "apply_repair_completed",
                    ));
                }
                progress.completed = true;
            }
        }
        Ok(())
    }

    fn unresolved(&self) -> Vec<TailRepairRecord> {
        self.repairs
            .values()
            .filter(|progress| !progress.completed)
            .map(|progress| progress.prepared.clone())
            .collect()
    }

    fn contains(&self, repair_id: &str) -> bool {
        self.repairs.contains_key(repair_id)
    }

    fn prepare(&mut self, root: &Path, record: TailRepairRecord) -> GlobalLedgerResult<()> {
        append_repair_record(root, &record)?;
        self.apply(record)
    }

    fn complete(&mut self, root: &Path, record: &TailRepairRecord) -> GlobalLedgerResult<()> {
        let completed = record.completed();
        append_repair_record(root, &completed)?;
        self.apply(completed)
    }
}

struct SegmentSnapshot {
    index: u64,
    path: PathBuf,
    bytes: Vec<u8>,
    complete_len: usize,
    is_last: bool,
}

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
    let mut events = Vec::new();
    for snapshot in &snapshots {
        parse_segment_records(
            snapshot,
            &mut next_sequence,
            &mut event_ids,
            &mut events,
            verifier,
        )?;
    }

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
        let bytes = fs::read(&last.path)
            .map_err(|error| GlobalLedgerError::io("ledger_io", "read_final_segment", &error))?;
        let complete_len = complete_record_len(&bytes);
        if bytes.len() > complete_len {
            let record = TailRepairRecord::prepared(
                last.index,
                bytes.len() as u64,
                complete_len as u64,
                &bytes[complete_len..],
            )?;
            if journal.contains(&record.repair_id) {
                return Err(GlobalLedgerError::fatal(
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

fn read_segment_snapshots(segments: &[(u64, PathBuf)]) -> GlobalLedgerResult<Vec<SegmentSnapshot>> {
    let mut snapshots = Vec::with_capacity(segments.len());
    for (position, (index, path)) in segments.iter().enumerate() {
        let is_last = position + 1 == segments.len();
        let bytes = fs::read(path)
            .map_err(|error| GlobalLedgerError::io("ledger_io", "read_segment", &error))?;
        let complete_len = complete_record_len(&bytes);
        if bytes.is_empty() && !is_last {
            return Err(GlobalLedgerError::fatal(
                "corrupt_segment",
                "recover_empty_non_final_segment",
            ));
        }
        if bytes.len() > complete_len && !is_last {
            return Err(GlobalLedgerError::fatal(
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

fn parse_segment_records(
    snapshot: &SegmentSnapshot,
    next_sequence: &mut u64,
    event_ids: &mut BTreeSet<EventId>,
    events: &mut Vec<PersistedEvent>,
    verifier: &mut Option<&mut ArtifactVerifier<'_>>,
) -> GlobalLedgerResult<()> {
    let complete_records = if snapshot.complete_len == 0 {
        &snapshot.bytes[..0]
    } else {
        &snapshot.bytes[..snapshot.complete_len - 1]
    };
    if complete_records.is_empty() && snapshot.complete_len > 0 {
        return Err(GlobalLedgerError::fatal(
            "corrupt_segment",
            "parse_blank_record",
        ));
    }
    if complete_records.is_empty() {
        return Ok(());
    }
    for line in complete_records.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            return Err(GlobalLedgerError::fatal(
                "corrupt_segment",
                "parse_blank_record",
            ));
        }
        let unique = serde_json::from_slice::<UniqueJsonValue>(line)
            .map_err(|error| GlobalLedgerError::json("corrupt_segment", "parse_segment", &error))?;
        let schema_version = unique
            .0
            .get("event")
            .and_then(|event| event.get("schema_version"))
            .and_then(Value::as_str);
        if schema_version != Some(GLOBAL_EVENT_SCHEMA_VERSION) {
            return Err(GlobalLedgerError::fatal(
                "unsupported_event_schema",
                "recover_event_schema",
            ));
        }
        let stored = serde_json::from_value::<StoredLine>(unique.0).map_err(|error| {
            GlobalLedgerError::json("corrupt_segment", "decode_segment", &error)
        })?;
        if stored.line_type != LINE_TYPE {
            return Err(GlobalLedgerError::fatal(
                "corrupt_segment",
                "validate_line_type",
            ));
        }
        let event = if let Some(verifier) = verifier.as_deref_mut() {
            stored.event.into_event_with_artifact_verifier(verifier)
        } else {
            stored.event.into_event()
        }
        .map_err(|error| GlobalLedgerError::fatal(error.code(), "validate_persisted_event"))?;
        if event.sequence() != *next_sequence {
            return Err(GlobalLedgerError::fatal(
                "sequence_discontinuity",
                "recover_sequence",
            ));
        }
        if !event_ids.insert(*event.event_id()) {
            return Err(GlobalLedgerError::fatal(
                "duplicate_event_id",
                "recover_event_ids",
            ));
        }
        events.push(event);
        *next_sequence = increment_sequence(*next_sequence)?;
    }
    Ok(())
}

fn resume_prepared_repair(
    root: &Path,
    snapshots: &[SegmentSnapshot],
    repair: &TailRepairRecord,
) -> GlobalLedgerResult<()> {
    let Some(snapshot) = snapshots
        .iter()
        .find(|snapshot| snapshot.index == repair.segment_index)
    else {
        return Err(GlobalLedgerError::fatal(
            "repair_segment_missing",
            "resume_tail_repair",
        ));
    };
    if !snapshot.is_last {
        return Err(GlobalLedgerError::fatal(
            "repair_segment_not_final",
            "resume_tail_repair",
        ));
    }
    let bytes = fs::read(&snapshot.path)
        .map_err(|error| GlobalLedgerError::io("ledger_io", "read_repair_segment", &error))?;
    match bytes.len() as u64 {
        length if length == repair.original_len => {
            let repaired_len = usize::try_from(repair.repaired_len).map_err(|_| {
                GlobalLedgerError::fatal("invalid_repair_record", "resume_tail_repair")
            })?;
            if repaired_len != complete_record_len(&bytes) {
                return Err(GlobalLedgerError::fatal(
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
                    GlobalLedgerError::io("ledger_io", "open_segment_repair", &error)
                })?;
            file.set_len(repair.repaired_len).map_err(|error| {
                GlobalLedgerError::io("ledger_io", "truncate_final_tail", &error)
            })?;
            file.sync_all()
                .map_err(|error| GlobalLedgerError::io("ledger_io", "sync_tail_repair", &error))?;
            repair_test_barrier("after_truncate")?;
        }
        length if length == repair.repaired_len => {
            if !bytes.is_empty() && bytes.last() != Some(&b'\n') {
                return Err(GlobalLedgerError::fatal(
                    "repair_tail_mismatch",
                    "resume_tail_repair",
                ));
            }
            ensure_quarantine(root, repair, None)?;
        }
        _ => {
            return Err(GlobalLedgerError::fatal(
                "repair_segment_length_mismatch",
                "resume_tail_repair",
            ));
        }
    }
    Ok(())
}

fn ensure_quarantine(
    root: &Path,
    repair: &TailRepairRecord,
    tail: Option<&[u8]>,
) -> GlobalLedgerResult<()> {
    let path = root.join(&repair.quarantine_key);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| GlobalLedgerError::io("ledger_io", "create_quarantine", &error))?;
    }
    match fs::read(&path) {
        Ok(existing) => {
            verify_tail_hash(repair, &existing)?;
            if tail.is_some_and(|tail| tail != existing) {
                return Err(GlobalLedgerError::fatal(
                    "quarantine_mismatch",
                    "verify_quarantine",
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let Some(tail) = tail else {
                return Err(GlobalLedgerError::fatal(
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
                    GlobalLedgerError::io("ledger_io", "create_quarantine_file", &error)
                })?;
            file.write_all(tail)
                .map_err(|error| GlobalLedgerError::io("ledger_io", "write_quarantine", &error))?;
            file.sync_all()
                .map_err(|error| GlobalLedgerError::io("ledger_io", "sync_quarantine", &error))
        }
        Err(error) => Err(GlobalLedgerError::io(
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
            "repair_tail_mismatch",
            "verify_repair_tail",
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
        || event.severity() != EventSeverity::Warning
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
            "repair_recovery_event_mismatch",
            "verify_recovery_event",
        ));
    }
    Ok(())
}

fn append_repair_record(root: &Path, record: &TailRepairRecord) -> GlobalLedgerResult<()> {
    record.validate()?;
    let mut bytes = serde_json::to_vec(record).map_err(|error| {
        GlobalLedgerError::json(
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
        .map_err(|error| GlobalLedgerError::io("ledger_io", "open_repair_journal", &error))?;
    file.write_all(&bytes)
        .map_err(|error| GlobalLedgerError::io("ledger_io", "write_repair_journal", &error))?;
    file.sync_all()
        .map_err(|error| GlobalLedgerError::io("ledger_io", "sync_repair_journal", &error))
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
            "invalid_repair_record",
            "parse_repair_hash",
        ));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(GlobalLedgerError::fatal(
            "invalid_repair_record",
            "parse_repair_hash",
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
        GlobalLedgerError::fatal("invalid_repair_record", "derive_quarantine_key")
    })?;
    if digest.len() != 64 {
        return Err(GlobalLedgerError::fatal(
            "invalid_repair_record",
            "derive_quarantine_key",
        ));
    }
    Ok(format!("quarantine/{digest}.bin"))
}

fn complete_record_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1)
}

#[cfg(test)]
fn repair_test_barrier(stage: &str) -> GlobalLedgerResult<()> {
    if std::env::var("ACTINGCOMMAND_TEST_REPAIR_FAILPOINT").as_deref() != Ok(stage) {
        return Ok(());
    }
    let ready = std::env::var_os("ACTINGCOMMAND_TEST_REPAIR_READY").ok_or_else(|| {
        GlobalLedgerError::fatal("repair_test_barrier_invalid", "repair_test_barrier")
    })?;
    let mut file = File::create(PathBuf::from(ready))
        .map_err(|error| GlobalLedgerError::io("ledger_io", "repair_test_barrier", &error))?;
    file.write_all(stage.as_bytes())
        .map_err(|error| GlobalLedgerError::io("ledger_io", "repair_test_barrier", &error))?;
    file.sync_all()
        .map_err(|error| GlobalLedgerError::io("ledger_io", "repair_test_barrier", &error))?;
    loop {
        std::thread::park_timeout(std::time::Duration::from_secs(60));
    }
}

#[cfg(not(test))]
fn repair_test_barrier(_stage: &str) -> GlobalLedgerResult<()> {
    Ok(())
}

fn list_segments(segments_dir: &Path) -> GlobalLedgerResult<Vec<(u64, PathBuf)>> {
    let mut segments = Vec::new();
    for entry in fs::read_dir(segments_dir)
        .map_err(|error| GlobalLedgerError::io("ledger_io", "list_segments", &error))?
    {
        let path = entry
            .map_err(|error| GlobalLedgerError::io("ledger_io", "read_segment_entry", &error))?
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
                "segment_discontinuity",
                "list_segments",
            ));
        }
    }
    Ok(segments)
}

fn segment_path(segments_dir: &Path, index: u64) -> PathBuf {
    segments_dir.join(format!("segment-{index:06}.jsonl"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WriterMetadata {
    schema_version: String,
    owner_id: String,
    pid: u32,
    active: bool,
    started_at_unix_ms: u64,
    closed_at_unix_ms: Option<u64>,
}

struct WriterOwnership {
    file: File,
    metadata: WriterMetadata,
    closed: bool,
}

impl WriterOwnership {
    fn acquire(root: &Path, owner_id: &str) -> GlobalLedgerResult<(Self, Option<String>)> {
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
                        GlobalLedgerError::io("ledger_io", "open_writer_lock", &error)
                    })?;
                (file, false)
            }
            Err(error) => {
                return Err(GlobalLedgerError::io(
                    "ledger_io",
                    "create_writer_lock",
                    &error,
                ));
            }
        };
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => {
                GlobalLedgerError::fatal("writer_conflict", "acquire_writer_lock")
            }
            std::fs::TryLockError::Error(error) => {
                GlobalLedgerError::io("ledger_io", "acquire_writer_lock", &error)
            }
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
                file,
                metadata,
                closed: false,
            },
            stale_owner,
        ))
    }

    fn close(&mut self) -> GlobalLedgerResult<()> {
        if self.closed {
            return Ok(());
        }
        self.metadata.active = false;
        self.metadata.closed_at_unix_ms = Some(unix_ms_now()?);
        write_writer_metadata(&mut self.file, &self.metadata)?;
        self.file
            .unlock()
            .map_err(|error| GlobalLedgerError::io("ledger_io", "release_writer_lock", &error))?;
        self.closed = true;
        Ok(())
    }
}

fn read_writer_metadata(
    file: &mut File,
    created: bool,
) -> GlobalLedgerResult<Option<WriterMetadata>> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| GlobalLedgerError::io("ledger_io", "seek_writer_metadata", &error))?;
    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .map_err(|error| GlobalLedgerError::io("ledger_io", "read_writer_metadata", &error))?;
    if content.is_empty() {
        return if created {
            Ok(None)
        } else {
            Err(GlobalLedgerError::fatal(
                "malformed_owner_metadata",
                "parse_writer_metadata",
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
            "malformed_owner_metadata",
            "parse_writer_metadata",
        ));
    }
    let mut metadata = None;
    for record in records.split(|byte| *byte == b'\n') {
        if record.is_empty() {
            return Err(GlobalLedgerError::fatal(
                "malformed_owner_metadata",
                "parse_writer_metadata",
            ));
        }
        metadata = Some(parse_writer_metadata(record)?);
    }
    if tail_len > 0 {
        file.set_len(complete.len() as u64).map_err(|error| {
            GlobalLedgerError::io("ledger_io", "truncate_writer_metadata_tail", &error)
        })?;
        file.sync_all().map_err(|error| {
            GlobalLedgerError::io("ledger_io", "sync_writer_metadata_tail", &error)
        })?;
    }
    Ok(metadata)
}

fn parse_writer_metadata(record: &[u8]) -> GlobalLedgerResult<WriterMetadata> {
    let unique = serde_json::from_slice::<UniqueJsonValue>(record).map_err(|error| {
        GlobalLedgerError::json("malformed_owner_metadata", "parse_writer_metadata", &error)
    })?;
    let metadata = serde_json::from_value::<WriterMetadata>(unique.0).map_err(|error| {
        GlobalLedgerError::json("malformed_owner_metadata", "decode_writer_metadata", &error)
    })?;
    if metadata.schema_version != WRITER_SCHEMA_VERSION {
        return Err(GlobalLedgerError::fatal(
            "unsupported_writer_schema",
            "validate_writer_metadata",
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
            "malformed_owner_metadata",
            "validate_writer_metadata",
        ));
    }
    Ok(metadata)
}

fn write_writer_metadata(file: &mut File, metadata: &WriterMetadata) -> GlobalLedgerResult<()> {
    let mut bytes = serde_json::to_vec(metadata).map_err(|error| {
        GlobalLedgerError::json(
            "owner_metadata_serialization_failed",
            "serialize_writer_metadata",
            &error,
        )
    })?;
    bytes.push(b'\n');
    let end = file
        .seek(SeekFrom::End(0))
        .map_err(|error| GlobalLedgerError::io("ledger_io", "seek_writer_metadata", &error))?;
    if end > 0 {
        file.seek(SeekFrom::End(-1)).map_err(|error| {
            GlobalLedgerError::io("ledger_io", "seek_writer_metadata_tail", &error)
        })?;
        let mut last = [0_u8; 1];
        file.read_exact(&mut last).map_err(|error| {
            GlobalLedgerError::io("ledger_io", "read_writer_metadata_tail", &error)
        })?;
        file.seek(SeekFrom::End(0)).map_err(|error| {
            GlobalLedgerError::io("ledger_io", "seek_writer_metadata_append", &error)
        })?;
        if last[0] != b'\n' {
            file.write_all(b"\n").map_err(|error| {
                GlobalLedgerError::io("ledger_io", "commit_legacy_writer_metadata", &error)
            })?;
        }
    }
    file.write_all(&bytes)
        .map_err(|error| GlobalLedgerError::io("ledger_io", "write_writer_metadata", &error))?;
    file.sync_all()
        .map_err(|error| GlobalLedgerError::io("ledger_io", "sync_writer_metadata", &error))?;
    Ok(())
}

fn unix_ms_now() -> GlobalLedgerResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .map_err(|_| GlobalLedgerError::fatal("clock_before_epoch", "read_clock"))
}

fn increment_sequence(sequence: u64) -> GlobalLedgerResult<u64> {
    sequence
        .checked_add(1)
        .ok_or_else(|| GlobalLedgerError::fatal("sequence_exhausted", "increment_sequence"))
}

struct UniqueJsonValue(Value);

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
