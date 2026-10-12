// SPDX-License-Identifier: AGPL-3.0-only

use super::projection::EventIndexes;
use super::storage::{
    LINE_TYPE, RepairJournal, StoredLine, UniqueJsonValue, WriterMetadata, complete_record_len,
    list_segments, parse_writer_metadata,
};
use super::{GlobalLedgerError, GlobalLedgerResult, MAX_QUERY_PAGE_EVENTS};
use crate::codes::{LedgerCode, LedgerLocation};
use crate::{
    PersistedEvent,
    fact::{LedgerEventMetadata, LedgerEventRead, StoredEventRecord},
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Code;
use actingcommand_contract::{
    EventId, EventQuery, GLOBAL_EVENT_SCHEMA_VERSION, ProjectedArtifactReference,
    VerifiedArtifactReference,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone)]
pub struct GlobalLedgerReadOnlyConfig {
    pub(super) root: PathBuf,
    pub(super) budget: Option<(u64, usize, Instant)>,
}

impl GlobalLedgerReadOnlyConfig {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            budget: None,
        }
    }

    /// Bounds an owner request without changing the ordinary forensic reader.
    pub fn with_budget(mut self, bytes: u64, events: usize, deadline: Instant) -> Self {
        self.budget = Some((bytes, events, deadline));
        self
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

#[derive(Clone, PartialEq, Eq)]
pub struct GlobalLedgerCorruptTail {
    pub code: Code,
    pub segment_index: u64,
    pub byte_offset: u64,
    pub dangling_byte_count: u64,
    pub tail_sha256: String,
}

/// The derived form this record printed before its code was typed: the code prints as a quoted
/// spelling (Workflow #378 keeps developer text unchanged).
impl std::fmt::Debug for GlobalLedgerCorruptTail {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GlobalLedgerCorruptTail")
            .field("code", &self.code.as_str())
            .field("segment_index", &self.segment_index)
            .field("byte_offset", &self.byte_offset)
            .field("dangling_byte_count", &self.dangling_byte_count)
            .field("tail_sha256", &self.tail_sha256)
            .finish()
    }
}

pub struct GlobalLedgerReadOnly {
    events: Vec<PersistedEvent>,
    indexes: EventIndexes,
    writer_metadata: GlobalLedgerWriterMetadataObservation,
    listed_through_segment: Option<u64>,
    repairs: Vec<GlobalLedgerRepairRecord>,
    corrupt_tail: Option<GlobalLedgerCorruptTail>,
    storage_snapshot: GlobalLedgerStorageSnapshot,
}

/// Physical observations from the single listing and bounded per-file reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GlobalLedgerStorageSnapshot {
    pub segment_count: u64,
    pub observed_bytes: u64,
    pub read_bytes: u64,
    pub verified_prefix_bytes: u64,
    pub read_complete: bool,
    pub atomic: bool,
}

pub(super) struct ReadOnlySnapshot<E> {
    pub(super) events: Vec<E>,
    pub(super) writer_metadata: GlobalLedgerWriterMetadataObservation,
    pub(super) listed_through_segment: Option<u64>,
    pub(super) repairs: Vec<GlobalLedgerRepairRecord>,
    pub(super) corrupt_tail: Option<GlobalLedgerCorruptTail>,
    pub(super) storage_snapshot: GlobalLedgerStorageSnapshot,
    segment_snapshots: Vec<ReadOnlySegmentSnapshot>,
}

pub(super) fn open_metadata(
    config: GlobalLedgerReadOnlyConfig,
) -> GlobalLedgerResult<ReadOnlySnapshot<LedgerEventMetadata>> {
    let budget = config.budget;
    let mut snapshot = open_snapshot(config, |line| {
        parse_record(line)?.into_metadata().map_err(|error| {
            GlobalLedgerError::fatal(
                error.code(),
                LedgerLocation::ValidateReadOnlyPersistedEvent.location(),
            )
        })
    })?;
    let bytes = snapshot.storage_snapshot.read_bytes;
    snapshot.segment_snapshots.clear();
    super::retention::annotate_metadata_checked(&mut snapshot.events, |count| {
        check_read_budget(budget, bytes, count)
    })?;
    Ok(snapshot)
}

fn open_snapshot<E: LedgerEventRead>(
    config: GlobalLedgerReadOnlyConfig,
    mut parse: impl FnMut(&[u8]) -> GlobalLedgerResult<E>,
) -> GlobalLedgerResult<ReadOnlySnapshot<E>> {
    if config.root.as_os_str().is_empty() {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::InvalidLedgerConfig.code(),
            LedgerLocation::ValidateReadOnlyRoot.location(),
        ));
    }
    let root = config.root.canonicalize().map_err(|error| {
        GlobalLedgerError::io(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::CanonicalizeReadOnlyRoot.location(),
            &error,
        )
    })?;
    let root_metadata = fs::metadata(&root).map_err(|error| {
        GlobalLedgerError::io(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::StatReadOnlyRoot.location(),
            &error,
        )
    })?;
    if !root_metadata.is_dir() {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::InvalidLedgerConfig.code(),
            LedgerLocation::ValidateReadOnlyRoot.location(),
        ));
    }

    let segments = list_segments(&root.join("segments"))?;
    let listed_through_segment = segments.last().map(|(index, _)| *index);
    let (segment_snapshots, mut storage_snapshot) =
        read_segment_snapshots(&segments, config.budget)?;
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
        check_read_budget(config.budget, storage_snapshot.read_bytes, events.len())?;
        if let Some(corruption) = scan_segment(
            snapshot,
            &mut next_sequence,
            &mut event_ids,
            &mut events,
            &mut parse,
            config.budget,
            None,
        )? {
            storage_snapshot.verified_prefix_bytes = storage_snapshot
                .verified_prefix_bytes
                .checked_add(corruption.byte_offset)
                .ok_or_else(|| {
                    GlobalLedgerError::fatal(
                        LedgerCode::LedgerSnapshotOverflow.code(),
                        LedgerLocation::CountVerifiedPrefixBytes.location(),
                    )
                })?;
            corrupt_tail = Some(corruption);
            break;
        }
        storage_snapshot.verified_prefix_bytes = storage_snapshot
            .verified_prefix_bytes
            .checked_add(snapshot.bytes.len() as u64)
            .ok_or_else(|| {
                GlobalLedgerError::fatal(
                    LedgerCode::LedgerSnapshotOverflow.code(),
                    LedgerLocation::CountVerifiedPrefixBytes.location(),
                )
            })?;
    }
    Ok(ReadOnlySnapshot {
        events,
        writer_metadata,
        listed_through_segment,
        repairs,
        corrupt_tail,
        storage_snapshot,
        segment_snapshots,
    })
}

impl GlobalLedgerReadOnly {
    pub(super) fn open<F>(
        config: GlobalLedgerReadOnlyConfig,
        verify_artifact: F,
    ) -> GlobalLedgerResult<Self>
    where
        F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
    {
        let mut verifier = Some(verify_artifact);
        Self::open_restoring(config, |retention, record| {
            retention.restore_record(record, &mut verifier)
        })
    }

    /// Workflow #375 R5a: `open` without reading artifact material (the Segment import
    /// source of `ledger-maintenance`). A reference without an eviction proof stays
    /// `Unrecorded`, so a missing artifact file never ends the snapshot.
    pub(super) fn open_unread(config: GlobalLedgerReadOnlyConfig) -> GlobalLedgerResult<Self> {
        Self::open_restoring(
            config,
            super::retention::RetentionIndex::restore_record_unread,
        )
    }

    fn open_restoring(
        config: GlobalLedgerReadOnlyConfig,
        mut restore: impl FnMut(
            &super::retention::RetentionIndex,
            StoredEventRecord,
        ) -> GlobalLedgerResult<PersistedEvent>,
    ) -> GlobalLedgerResult<Self> {
        let budget = config.budget;
        let mut snapshot = open_snapshot(config, |line| {
            parse_record(line)?.into_metadata().map_err(|error| {
                GlobalLedgerError::fatal(
                    error.code(),
                    LedgerLocation::ValidateReadOnlyPersistedEvent.location(),
                )
            })
        })?;
        let bytes = snapshot.storage_snapshot.read_bytes;
        let retention = super::retention::RetentionIndex::from_events_checked(
            &snapshot.events,
            &mut |count| check_read_budget(budget, bytes, count),
        )?;
        let mut events = Vec::with_capacity(snapshot.events.len());
        let mut event_ids = BTreeSet::new();
        let mut next_sequence = 1;
        let mut parse = |line: &[u8]| restore(&retention, parse_record(line)?);
        let mut material_prefix_bytes = 0_u64;
        // Metadata and proof validation finished above. Reuse the already bounded bytes;
        // the ordinary scanner retains its exact first-tail position and failure behavior.
        for segment in &snapshot.segment_snapshots {
            if events.len() == snapshot.events.len() {
                break;
            }
            check_read_budget(budget, bytes, events.len())?;
            if let Some(corruption) = scan_segment(
                segment,
                &mut next_sequence,
                &mut event_ids,
                &mut events,
                &mut parse,
                budget,
                Some(snapshot.events.len()),
            )? {
                snapshot.storage_snapshot.verified_prefix_bytes = material_prefix_bytes
                    .checked_add(corruption.byte_offset)
                    .ok_or_else(|| {
                        GlobalLedgerError::fatal(
                            LedgerCode::LedgerSnapshotOverflow.code(),
                            LedgerLocation::CountVerifiedPrefixBytes.location(),
                        )
                    })?;
                snapshot.corrupt_tail = Some(corruption);
                break;
            }
            material_prefix_bytes = material_prefix_bytes
                .checked_add(segment.bytes.len() as u64)
                .ok_or_else(|| {
                    GlobalLedgerError::fatal(
                        LedgerCode::LedgerSnapshotOverflow.code(),
                        LedgerLocation::CountVerifiedPrefixBytes.location(),
                    )
                })?;
        }
        Ok(Self {
            indexes: EventIndexes::from_events(&events),
            events,
            writer_metadata: snapshot.writer_metadata,
            listed_through_segment: snapshot.listed_through_segment,
            repairs: snapshot.repairs,
            corrupt_tail: snapshot.corrupt_tail,
            storage_snapshot: snapshot.storage_snapshot,
        })
    }

    pub fn events(&self) -> &[PersistedEvent] {
        &self.events
    }

    pub const fn storage_snapshot(&self) -> &GlobalLedgerStorageSnapshot {
        &self.storage_snapshot
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
                LedgerCode::InvalidQueryPage.code(),
                LedgerLocation::QueryReadOnlyEventPage.location(),
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
    budget: Option<(u64, usize, Instant)>,
) -> GlobalLedgerResult<(Vec<ReadOnlySegmentSnapshot>, GlobalLedgerStorageSnapshot)> {
    let mut snapshots = Vec::with_capacity(segments.len());
    let mut storage = GlobalLedgerStorageSnapshot {
        segment_count: u64::try_from(segments.len()).map_err(|_| {
            GlobalLedgerError::fatal(
                LedgerCode::LedgerSnapshotOverflow.code(),
                LedgerLocation::CountReadOnlySegments.location(),
            )
        })?,
        observed_bytes: 0,
        read_bytes: 0,
        verified_prefix_bytes: 0,
        read_complete: true,
        atomic: false,
    };
    for (position, (index, path)) in segments.iter().enumerate() {
        let mut file = File::open(path).map_err(|error| {
            GlobalLedgerError::io(
                LedgerCode::LedgerIo.code(),
                LedgerLocation::OpenReadOnlySegment.location(),
                &error,
            )
        })?;
        let byte_count = file
            .metadata()
            .map_err(|error| {
                GlobalLedgerError::io(
                    LedgerCode::LedgerIo.code(),
                    LedgerLocation::StatReadOnlySegment.location(),
                    &error,
                )
            })?
            .len();
        let total = storage
            .observed_bytes
            .checked_add(byte_count)
            .ok_or_else(|| {
                GlobalLedgerError::fatal(
                    LedgerCode::LedgerSnapshotOverflow.code(),
                    LedgerLocation::BoundReadOnlySnapshot.location(),
                )
            })?;
        check_read_budget(budget, total, 0)?;
        let capacity = usize::try_from(byte_count).map_err(|_| {
            GlobalLedgerError::fatal(
                LedgerCode::CorruptSegment.code(),
                LedgerLocation::BoundReadOnlySegment.location(),
            )
        })?;
        let mut bytes = Vec::with_capacity(capacity);
        file.by_ref()
            .take(byte_count)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                GlobalLedgerError::io(
                    LedgerCode::LedgerIo.code(),
                    LedgerLocation::ReadReadOnlySegment.location(),
                    &error,
                )
            })?;
        let read_bytes = u64::try_from(bytes.len()).map_err(|_| {
            GlobalLedgerError::fatal(
                LedgerCode::LedgerSnapshotOverflow.code(),
                LedgerLocation::CountReadOnlyBytes.location(),
            )
        })?;
        storage.observed_bytes =
            storage
                .observed_bytes
                .checked_add(byte_count)
                .ok_or_else(|| {
                    GlobalLedgerError::fatal(
                        LedgerCode::LedgerSnapshotOverflow.code(),
                        LedgerLocation::CountObservedBytes.location(),
                    )
                })?;
        storage.read_bytes = storage.read_bytes.checked_add(read_bytes).ok_or_else(|| {
            GlobalLedgerError::fatal(
                LedgerCode::LedgerSnapshotOverflow.code(),
                LedgerLocation::CountReadOnlyBytes.location(),
            )
        })?;
        storage.read_complete &= read_bytes == byte_count;
        snapshots.push(ReadOnlySegmentSnapshot {
            index: *index,
            bytes,
            is_final: position + 1 == segments.len(),
        });
    }
    Ok((snapshots, storage))
}

/// The detail names the bound that tripped (Workflow #363). Callers may pass 0 for a count
/// they do not track, so the counts after a deadline are only those given at this check.
pub(super) fn check_read_budget(
    budget: Option<(u64, usize, Instant)>,
    bytes: u64,
    events: usize,
) -> GlobalLedgerResult<()> {
    let Some((max_bytes, max_events, deadline)) = budget else {
        return Ok(());
    };
    let detail = if bytes > max_bytes {
        format!("bytes {bytes} > {max_bytes}")
    } else if events > max_events {
        format!("events {events} > {max_events}")
    } else if Instant::now() >= deadline {
        format!("deadline reached at check (bytes={bytes}, events={events})")
    } else {
        return Ok(());
    };
    Err(GlobalLedgerError::request(
        LedgerCode::LedgerReadBudgetExceeded.code(),
        LedgerLocation::ReadOnlySnapshot.location(),
    )
    .with_detail(detail))
}

pub(super) fn read_writer_metadata(
    root: &Path,
) -> GlobalLedgerResult<GlobalLedgerWriterMetadataObservation> {
    let path = root.join("writer.lock");
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(GlobalLedgerWriterMetadataObservation::Absent);
        }
        Err(error) => {
            return Err(GlobalLedgerError::io(
                LedgerCode::LedgerIo.code(),
                LedgerLocation::OpenReadOnlyWriterMetadata.location(),
                &error,
            ));
        }
    };
    let byte_count = file
        .metadata()
        .map_err(|error| {
            GlobalLedgerError::io(
                LedgerCode::LedgerIo.code(),
                LedgerLocation::StatReadOnlyWriterMetadata.location(),
                &error,
            )
        })?
        .len();
    let capacity = usize::try_from(byte_count).map_err(|_| {
        GlobalLedgerError::fatal(
            LedgerCode::MalformedOwnerMetadata.code(),
            LedgerLocation::BoundReadOnlyWriterMetadata.location(),
        )
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    if let Err(error) = file.by_ref().take(byte_count).read_to_end(&mut bytes) {
        if writer_metadata_is_locked(&error) {
            return Ok(GlobalLedgerWriterMetadataObservation::Locked { byte_count });
        }
        return Err(GlobalLedgerError::io(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::ReadReadOnlyWriterMetadata.location(),
            &error,
        ));
    }
    if bytes.is_empty() {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::MalformedOwnerMetadata.code(),
            LedgerLocation::ParseReadOnlyWriterMetadata.location(),
        ));
    }
    let complete_len = complete_record_len(&bytes);
    let records = if complete_len == 0 {
        &bytes[..]
    } else {
        if complete_len != bytes.len() {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::MalformedOwnerMetadata.code(),
                LedgerLocation::ParseReadOnlyWriterMetadataTail.location(),
            ));
        }
        &bytes[..complete_len - 1]
    };
    let mut metadata = None;
    for record in records.split(|byte| *byte == b'\n') {
        if record.is_empty() {
            return Err(GlobalLedgerError::fatal(
                LedgerCode::MalformedOwnerMetadata.code(),
                LedgerLocation::ParseReadOnlyWriterMetadataBlank.location(),
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

fn scan_segment<E: LedgerEventRead>(
    snapshot: &ReadOnlySegmentSnapshot,
    next_sequence: &mut u64,
    event_ids: &mut BTreeSet<EventId>,
    events: &mut Vec<E>,
    parse: &mut impl FnMut(&[u8]) -> GlobalLedgerResult<E>,
    budget: Option<(u64, usize, Instant)>,
    authenticated_event_count: Option<usize>,
) -> GlobalLedgerResult<Option<GlobalLedgerCorruptTail>> {
    let complete_len = complete_record_len(&snapshot.bytes);
    let mut record_start = 0_usize;
    while record_start < complete_len {
        if authenticated_event_count == Some(events.len()) {
            return Ok(None);
        }
        check_read_budget(budget, 0, events.len().saturating_add(1))?;
        let newline = snapshot.bytes[record_start..complete_len]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|position| record_start + position)
            .expect("complete record ceiling ends at a newline");
        let line = &snapshot.bytes[record_start..newline];
        if line.is_empty() {
            return corrupt_tail(LedgerCode::CorruptSegment.code(), snapshot, record_start)
                .map(Some);
        }
        let event = match parse(line) {
            Ok(event) => event,
            Err(error) => {
                return corrupt_tail(corruption_code(&error), snapshot, record_start).map(Some);
            }
        };
        if event.sequence() != *next_sequence {
            return corrupt_tail(
                LedgerCode::SequenceDiscontinuity.code(),
                snapshot,
                record_start,
            )
            .map(Some);
        }
        if !event_ids.insert(*event.event_id()) {
            return corrupt_tail(LedgerCode::DuplicateEventId.code(), snapshot, record_start)
                .map(Some);
        }
        *next_sequence = next_sequence.checked_add(1).ok_or_else(|| {
            GlobalLedgerError::fatal(
                LedgerCode::SequenceExhausted.code(),
                LedgerLocation::IncrementReadOnlySequence.location(),
            )
        })?;
        events.push(event);
        record_start = newline + 1;
    }

    if authenticated_event_count != Some(events.len())
        && (snapshot.bytes.len() > complete_len
            || (!snapshot.is_final && snapshot.bytes.is_empty()))
    {
        return corrupt_tail(LedgerCode::CorruptSegment.code(), snapshot, complete_len).map(Some);
    }
    Ok(None)
}

fn parse_record(line: &[u8]) -> GlobalLedgerResult<StoredEventRecord> {
    let unique = serde_json::from_slice::<UniqueJsonValue>(line).map_err(|error| {
        GlobalLedgerError::json(
            LedgerCode::CorruptSegment.code(),
            LedgerLocation::ParseReadOnlySegment.location(),
            &error,
        )
    })?;
    let schema_version = unique
        .0
        .get("event")
        .and_then(|event| event.get("schema_version"))
        .and_then(Value::as_str);
    if schema_version != Some(GLOBAL_EVENT_SCHEMA_VERSION) {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::UnsupportedEventSchema.code(),
            LedgerLocation::ReadOnlyEventSchema.location(),
        ));
    }
    let stored = serde_json::from_value::<StoredLine>(unique.0).map_err(|error| {
        GlobalLedgerError::json(
            LedgerCode::CorruptSegment.code(),
            LedgerLocation::DecodeReadOnlySegment.location(),
            &error,
        )
    })?;
    if stored.line_type != LINE_TYPE {
        return Err(GlobalLedgerError::fatal(
            LedgerCode::CorruptSegment.code(),
            LedgerLocation::ValidateReadOnlyLineType.location(),
        ));
    }
    Ok(stored.event)
}

/// The corrupt-tail code of a record that does not parse: a schema refusal keeps its own code,
/// an artifact verification failure is reported as one code, anything else is a corrupt
/// segment.
fn corruption_code(error: &GlobalLedgerError) -> Code {
    let failed = error.code();
    if failed == LedgerCode::UnsupportedEventSchema.as_str() {
        return LedgerCode::UnsupportedEventSchema.code();
    }
    let verification = [
        LedgerCode::ArtifactStoreVerificationFailed.code(),
        LedgerCode::ArtifactStoreVerificationMismatch.code(),
        LedgerCode::ArtifactStoreVerificationUnavailable.code(),
        SharedCode::InvalidArtifactReference.code(),
    ];
    if verification.iter().any(|code| code.as_str() == failed) {
        return LedgerCode::ArtifactStoreVerificationFailed.code();
    }
    LedgerCode::CorruptSegment.code()
}

fn corrupt_tail(
    code: Code,
    snapshot: &ReadOnlySegmentSnapshot,
    byte_offset: usize,
) -> GlobalLedgerResult<GlobalLedgerCorruptTail> {
    let suffix = snapshot.bytes.get(byte_offset..).ok_or_else(|| {
        GlobalLedgerError::fatal(
            LedgerCode::CorruptSegment.code(),
            LedgerLocation::LocateReadOnlyCorruptTail.location(),
        )
    })?;
    Ok(GlobalLedgerCorruptTail {
        code,
        segment_index: snapshot.index,
        byte_offset: u64::try_from(byte_offset).map_err(|_| {
            GlobalLedgerError::fatal(
                LedgerCode::CorruptSegment.code(),
                LedgerLocation::ConvertReadOnlyCorruptOffset.location(),
            )
        })?,
        dangling_byte_count: u64::try_from(suffix.len()).map_err(|_| {
            GlobalLedgerError::fatal(
                LedgerCode::CorruptSegment.code(),
                LedgerLocation::ConvertReadOnlyCorruptLength.location(),
            )
        })?,
        tail_sha256: format!("sha256:{:x}", Sha256::digest(suffix)),
    })
}
