// SPDX-License-Identifier: AGPL-3.0-only

use super::projection::EventIndexes;
use super::storage::{
    LINE_TYPE, RepairJournal, StoredLine, UniqueJsonValue, WriterMetadata, complete_record_len,
    list_segments, parse_writer_metadata,
};
use super::{GlobalLedgerError, GlobalLedgerResult, MAX_QUERY_PAGE_EVENTS};
use crate::PersistedEvent;
use actingcommand_contract::{
    EventId, EventQuery, GLOBAL_EVENT_SCHEMA_VERSION, ProjectedArtifactReference,
    VerifiedArtifactReference,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct GlobalLedgerReadOnlyConfig {
    root: PathBuf,
}

impl GlobalLedgerReadOnlyConfig {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }
}

impl fmt::Debug for GlobalLedgerReadOnlyConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GlobalLedgerReadOnlyConfig")
            .field("root", &"<redacted-root>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalLedgerWriterMetadata {
    schema_version: String,
    owner_id: String,
    pid: u32,
    active: bool,
    started_at_unix_ms: u64,
    closed_at_unix_ms: Option<u64>,
}

impl GlobalLedgerWriterMetadata {
    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    pub fn owner_id(&self) -> &str {
        &self.owner_id
    }

    pub const fn pid(&self) -> u32 {
        self.pid
    }

    pub const fn active(&self) -> bool {
        self.active
    }

    pub const fn started_at_unix_ms(&self) -> u64 {
        self.started_at_unix_ms
    }

    pub const fn closed_at_unix_ms(&self) -> Option<u64> {
        self.closed_at_unix_ms
    }
}

impl From<WriterMetadata> for GlobalLedgerWriterMetadata {
    fn from(metadata: WriterMetadata) -> Self {
        Self {
            schema_version: metadata.schema_version,
            owner_id: metadata.owner_id,
            pid: metadata.pid,
            active: metadata.active,
            started_at_unix_ms: metadata.started_at_unix_ms,
            closed_at_unix_ms: metadata.closed_at_unix_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobalLedgerWriterMetadataObservation {
    Absent,
    Readable(GlobalLedgerWriterMetadata),
    Locked { byte_count: u64 },
}

impl GlobalLedgerWriterMetadataObservation {
    pub const fn readable(&self) -> Option<&GlobalLedgerWriterMetadata> {
        match self {
            Self::Readable(metadata) => Some(metadata),
            Self::Absent | Self::Locked { .. } => None,
        }
    }

    pub const fn locked_byte_count(&self) -> Option<u64> {
        match self {
            Self::Locked { byte_count } => Some(*byte_count),
            Self::Absent | Self::Readable(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalLedgerRepairRecord {
    schema_version: String,
    repair_id: String,
    completed: bool,
    segment_index: u64,
    original_len: u64,
    repaired_len: u64,
    tail_sha256: String,
    quarantine_key: String,
}

impl GlobalLedgerRepairRecord {
    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    pub fn repair_id(&self) -> &str {
        &self.repair_id
    }

    pub const fn completed(&self) -> bool {
        self.completed
    }

    pub const fn segment_index(&self) -> u64 {
        self.segment_index
    }

    pub const fn original_len(&self) -> u64 {
        self.original_len
    }

    pub const fn repaired_len(&self) -> u64 {
        self.repaired_len
    }

    pub fn tail_sha256(&self) -> &str {
        &self.tail_sha256
    }

    pub fn quarantine_key(&self) -> &str {
        &self.quarantine_key
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalLedgerCorruptTail {
    pub code: &'static str,
    pub segment_index: u64,
    pub byte_offset: u64,
    pub dangling_byte_count: u64,
    pub tail_sha256: String,
}

pub struct GlobalLedgerReadOnly {
    events: Vec<PersistedEvent>,
    indexes: EventIndexes,
    writer_metadata: GlobalLedgerWriterMetadataObservation,
    listed_through_segment: Option<u64>,
    repairs: Vec<GlobalLedgerRepairRecord>,
    corrupt_tail: Option<GlobalLedgerCorruptTail>,
}

impl GlobalLedgerReadOnly {
    pub(super) fn open<F>(
        config: GlobalLedgerReadOnlyConfig,
        mut verify_artifact: F,
    ) -> GlobalLedgerResult<Self>
    where
        F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
    {
        if config.root.as_os_str().is_empty() {
            return Err(GlobalLedgerError::fatal(
                "invalid_ledger_config",
                "validate_read_only_root",
            ));
        }
        let root = config.root.canonicalize().map_err(|error| {
            GlobalLedgerError::io("ledger_io", "canonicalize_read_only_root", &error)
        })?;
        let root_metadata = fs::metadata(&root)
            .map_err(|error| GlobalLedgerError::io("ledger_io", "stat_read_only_root", &error))?;
        if !root_metadata.is_dir() {
            return Err(GlobalLedgerError::fatal(
                "invalid_ledger_config",
                "validate_read_only_root",
            ));
        }

        let segments = list_segments(&root.join("segments"))?;
        let listed_through_segment = segments.last().map(|(index, _)| *index);
        let segment_snapshots = read_segment_snapshots(&segments)?;
        let writer_metadata = read_writer_metadata(&root)?;
        let repairs = RepairJournal::load(&root)?
            .snapshots()
            .into_iter()
            .map(|repair| GlobalLedgerRepairRecord {
                schema_version: repair.schema_version,
                repair_id: repair.repair_id,
                completed: repair.completed,
                segment_index: repair.segment_index,
                original_len: repair.original_len,
                repaired_len: repair.repaired_len,
                tail_sha256: repair.tail_sha256,
                quarantine_key: repair.quarantine_key,
            })
            .collect();

        let mut events = Vec::new();
        let mut event_ids = BTreeSet::new();
        let mut next_sequence = 1_u64;
        let mut corrupt_tail = None;
        for snapshot in &segment_snapshots {
            if let Some(corruption) = scan_segment(
                snapshot,
                &mut next_sequence,
                &mut event_ids,
                &mut events,
                &mut verify_artifact,
            )? {
                corrupt_tail = Some(corruption);
                break;
            }
        }
        let indexes = EventIndexes::from_events(&events);
        Ok(Self {
            events,
            indexes,
            writer_metadata,
            listed_through_segment,
            repairs,
            corrupt_tail,
        })
    }

    pub fn events(&self) -> &[PersistedEvent] {
        &self.events
    }

    pub fn query(&self, query: &EventQuery) -> Vec<PersistedEvent> {
        self.indexes.query(&self.events, query)
    }

    pub fn query_page(
        &self,
        query: &EventQuery,
        after: u64,
        through: u64,
        limit: usize,
    ) -> GlobalLedgerResult<Vec<PersistedEvent>> {
        if !(1..=MAX_QUERY_PAGE_EVENTS).contains(&limit) || after > through {
            return Err(GlobalLedgerError::request(
                "invalid_query_page",
                "query_read_only_event_page",
            ));
        }
        Ok(self
            .indexes
            .query_page(&self.events, query, after, through, limit))
    }

    pub fn latest_sequence(&self) -> u64 {
        self.events.last().map_or(0, PersistedEvent::sequence)
    }

    pub const fn writer_metadata(&self) -> &GlobalLedgerWriterMetadataObservation {
        &self.writer_metadata
    }

    pub const fn listed_through_segment(&self) -> Option<u64> {
        self.listed_through_segment
    }

    pub fn repairs(&self) -> &[GlobalLedgerRepairRecord] {
        &self.repairs
    }

    pub const fn corrupt_tail(&self) -> Option<&GlobalLedgerCorruptTail> {
        self.corrupt_tail.as_ref()
    }
}

struct ReadOnlySegmentSnapshot {
    index: u64,
    bytes: Vec<u8>,
    is_final: bool,
}

fn read_segment_snapshots(
    segments: &[(u64, PathBuf)],
) -> GlobalLedgerResult<Vec<ReadOnlySegmentSnapshot>> {
    let mut snapshots = Vec::with_capacity(segments.len());
    for (position, (index, path)) in segments.iter().enumerate() {
        let mut file = File::open(path).map_err(|error| {
            GlobalLedgerError::io("ledger_io", "open_read_only_segment", &error)
        })?;
        let byte_count = file
            .metadata()
            .map_err(|error| GlobalLedgerError::io("ledger_io", "stat_read_only_segment", &error))?
            .len();
        let capacity = usize::try_from(byte_count)
            .map_err(|_| GlobalLedgerError::fatal("corrupt_segment", "bound_read_only_segment"))?;
        let mut bytes = Vec::with_capacity(capacity);
        file.by_ref()
            .take(byte_count)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                GlobalLedgerError::io("ledger_io", "read_read_only_segment", &error)
            })?;
        snapshots.push(ReadOnlySegmentSnapshot {
            index: *index,
            bytes,
            is_final: position + 1 == segments.len(),
        });
    }
    Ok(snapshots)
}

fn read_writer_metadata(root: &Path) -> GlobalLedgerResult<GlobalLedgerWriterMetadataObservation> {
    let path = root.join("writer.lock");
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(GlobalLedgerWriterMetadataObservation::Absent);
        }
        Err(error) => {
            return Err(GlobalLedgerError::io(
                "ledger_io",
                "open_read_only_writer_metadata",
                &error,
            ));
        }
    };
    let byte_count = file
        .metadata()
        .map_err(|error| {
            GlobalLedgerError::io("ledger_io", "stat_read_only_writer_metadata", &error)
        })?
        .len();
    let capacity = usize::try_from(byte_count).map_err(|_| {
        GlobalLedgerError::fatal(
            "malformed_owner_metadata",
            "bound_read_only_writer_metadata",
        )
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    if let Err(error) = file.by_ref().take(byte_count).read_to_end(&mut bytes) {
        if writer_metadata_is_locked(&error) {
            return Ok(GlobalLedgerWriterMetadataObservation::Locked { byte_count });
        }
        return Err(GlobalLedgerError::io(
            "ledger_io",
            "read_read_only_writer_metadata",
            &error,
        ));
    }
    if bytes.is_empty() {
        return Err(GlobalLedgerError::fatal(
            "malformed_owner_metadata",
            "parse_read_only_writer_metadata",
        ));
    }
    let complete_len = complete_record_len(&bytes);
    let records = if complete_len == 0 {
        &bytes[..]
    } else {
        if complete_len != bytes.len() {
            return Err(GlobalLedgerError::fatal(
                "malformed_owner_metadata",
                "parse_read_only_writer_metadata_tail",
            ));
        }
        &bytes[..complete_len - 1]
    };
    let mut metadata = None;
    for record in records.split(|byte| *byte == b'\n') {
        if record.is_empty() {
            return Err(GlobalLedgerError::fatal(
                "malformed_owner_metadata",
                "parse_read_only_writer_metadata_blank",
            ));
        }
        metadata = Some(parse_writer_metadata(record)?.into());
    }
    Ok(GlobalLedgerWriterMetadataObservation::Readable(
        metadata.expect("non-empty metadata contains at least one record"),
    ))
}

fn writer_metadata_is_locked(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock || error.raw_os_error() == Some(33)
}

fn scan_segment<F>(
    snapshot: &ReadOnlySegmentSnapshot,
    next_sequence: &mut u64,
    event_ids: &mut BTreeSet<EventId>,
    events: &mut Vec<PersistedEvent>,
    verify_artifact: &mut F,
) -> GlobalLedgerResult<Option<GlobalLedgerCorruptTail>>
where
    F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
{
    let complete_len = complete_record_len(&snapshot.bytes);
    let mut record_start = 0_usize;
    while record_start < complete_len {
        let newline = snapshot.bytes[record_start..complete_len]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|position| record_start + position)
            .expect("complete record ceiling ends at a newline");
        let line = &snapshot.bytes[record_start..newline];
        if line.is_empty() {
            return corrupt_tail("corrupt_segment", snapshot, record_start).map(Some);
        }
        let event = match parse_event(line, verify_artifact) {
            Ok(event) => event,
            Err(error) => {
                return corrupt_tail(corruption_code(error.code()), snapshot, record_start)
                    .map(Some);
            }
        };
        if event.sequence() != *next_sequence {
            return corrupt_tail("sequence_discontinuity", snapshot, record_start).map(Some);
        }
        if !event_ids.insert(*event.event_id()) {
            return corrupt_tail("duplicate_event_id", snapshot, record_start).map(Some);
        }
        *next_sequence = next_sequence.checked_add(1).ok_or_else(|| {
            GlobalLedgerError::fatal("sequence_exhausted", "increment_read_only_sequence")
        })?;
        events.push(event);
        record_start = newline + 1;
    }

    if snapshot.bytes.len() > complete_len || (!snapshot.is_final && snapshot.bytes.is_empty()) {
        return corrupt_tail("corrupt_segment", snapshot, complete_len).map(Some);
    }
    Ok(None)
}

fn parse_event<F>(line: &[u8], verify_artifact: &mut F) -> GlobalLedgerResult<PersistedEvent>
where
    F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
{
    let unique = serde_json::from_slice::<UniqueJsonValue>(line).map_err(|error| {
        GlobalLedgerError::json("corrupt_segment", "parse_read_only_segment", &error)
    })?;
    let schema_version = unique
        .0
        .get("event")
        .and_then(|event| event.get("schema_version"))
        .and_then(Value::as_str);
    if schema_version != Some(GLOBAL_EVENT_SCHEMA_VERSION) {
        return Err(GlobalLedgerError::fatal(
            "unsupported_event_schema",
            "read_only_event_schema",
        ));
    }
    let stored = serde_json::from_value::<StoredLine>(unique.0).map_err(|error| {
        GlobalLedgerError::json("corrupt_segment", "decode_read_only_segment", &error)
    })?;
    if stored.line_type != LINE_TYPE {
        return Err(GlobalLedgerError::fatal(
            "corrupt_segment",
            "validate_read_only_line_type",
        ));
    }
    stored
        .event
        .into_event_with_artifact_verifier(verify_artifact)
        .map_err(|error| {
            GlobalLedgerError::fatal(error.code(), "validate_read_only_persisted_event")
        })
}

fn corruption_code(code: &'static str) -> &'static str {
    match code {
        "unsupported_event_schema" => "unsupported_event_schema",
        "artifact_store_verification_failed"
        | "artifact_store_verification_mismatch"
        | "artifact_store_verification_unavailable" => "artifact_store_verification_failed",
        _ => "corrupt_segment",
    }
}

fn corrupt_tail(
    code: &'static str,
    snapshot: &ReadOnlySegmentSnapshot,
    byte_offset: usize,
) -> GlobalLedgerResult<GlobalLedgerCorruptTail> {
    let suffix = snapshot.bytes.get(byte_offset..).ok_or_else(|| {
        GlobalLedgerError::fatal("corrupt_segment", "locate_read_only_corrupt_tail")
    })?;
    Ok(GlobalLedgerCorruptTail {
        code,
        segment_index: snapshot.index,
        byte_offset: u64::try_from(byte_offset).map_err(|_| {
            GlobalLedgerError::fatal("corrupt_segment", "convert_read_only_corrupt_offset")
        })?,
        dangling_byte_count: u64::try_from(suffix.len()).map_err(|_| {
            GlobalLedgerError::fatal("corrupt_segment", "convert_read_only_corrupt_length")
        })?,
        tail_sha256: format!("sha256:{:x}", Sha256::digest(suffix)),
    })
}
