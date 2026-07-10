// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    GlobalLedgerConfig, GlobalLedgerError, GlobalLedgerResult, is_identifier,
    projection::EventIndexes,
};
use actingcommand_contract::{
    ClassifiedField, ErasedSanitizedEventDraft, EventActor, EventDraft, EventLinks, EventOrigin,
    EventSeverity, EventSource, EventType, FieldRedactor, LedgerPayloadDraft, LedgerTransition,
    PersistedEvent, SanitizationError,
};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Number, Value};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

const WRITER_SCHEMA_VERSION: &str = "actingcommand.ledger-writer.v1";
const LINE_TYPE: &str = "event";

pub(super) struct SegmentStore {
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
        fs::create_dir_all(&config.root)
            .map_err(|error| GlobalLedgerError::io("ledger_io", "create_ledger_root", &error))?;
        let segments_dir = config.root.join("segments");
        fs::create_dir_all(&segments_dir)
            .map_err(|error| GlobalLedgerError::io("ledger_io", "create_segments", &error))?;
        let (mut ownership, stale_owner) =
            WriterOwnership::acquire(&config.root, &config.owner_id)?;
        let recovery = match recover_segments(&config.root, &segments_dir) {
            Ok(recovery) => recovery,
            Err(error) => {
                let _ = ownership.close();
                return Err(error);
            }
        };
        let active_index = recovery.last_segment_index.max(1);
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
            segments_dir,
            ownership,
            segment_max_bytes: config.segment_max_bytes,
            active_index,
            active_bytes,
            active_file,
            next_sequence: recovery.next_sequence,
            indexes: EventIndexes::from_events(&recovery.events),
            events: recovery.events,
        };
        if let Some(owner_id) = stale_owner {
            store.append_recovery("stale_owner", Some(owner_id), None)?;
        }
        if let Some(tail) = recovery.truncated_tail {
            store.append_recovery(
                "truncated_final_tail",
                None,
                Some((tail.segment_index, tail.bytes)),
            )?;
        }
        Ok(store)
    }

    pub(super) fn append(
        &mut self,
        draft: ErasedSanitizedEventDraft,
    ) -> GlobalLedgerResult<PersistedEvent> {
        let following_sequence = increment_sequence(self.next_sequence)?;
        let event = PersistedEvent::from_draft(self.next_sequence, draft);
        event
            .validate()
            .map_err(|_| GlobalLedgerError::request("invalid_sanitized_event", "validate_event"))?;
        if self.indexes.contains_event_id(&event.event_id) {
            return Err(GlobalLedgerError::request(
                "duplicate_event_id",
                "append_event",
            ));
        }
        let mut bytes = serde_json::to_vec(&StoredLine {
            line_type: LINE_TYPE.to_string(),
            event: event.clone(),
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

    pub(super) fn events_after(&self, sequence: u64) -> Vec<PersistedEvent> {
        self.events
            .iter()
            .filter(|event| event.sequence > sequence)
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
        reason: &str,
        previous_owner: Option<String>,
        tail: Option<(u64, u64)>,
    ) -> GlobalLedgerResult<()> {
        let mut fields = vec![ClassifiedField::public("reason", reason).map_err(|_| {
            GlobalLedgerError::fatal("recovery_event_failed", "build_recovery_event")
        })?];
        if let Some(owner_id) = previous_owner {
            fields.push(
                ClassifiedField::internal("previous_owner", owner_id).map_err(|_| {
                    GlobalLedgerError::fatal("recovery_event_failed", "build_recovery_event")
                })?,
            );
        }
        if let Some((segment, bytes)) = tail {
            fields.push(
                ClassifiedField::public("segment", segment.to_string()).map_err(|_| {
                    GlobalLedgerError::fatal("recovery_event_failed", "build_recovery_event")
                })?,
            );
            fields.push(
                ClassifiedField::public("quarantined_bytes", bytes.to_string()).map_err(|_| {
                    GlobalLedgerError::fatal("recovery_event_failed", "build_recovery_event")
                })?,
            );
        }
        let now = unix_ms_now()?;
        let payload =
            LedgerPayloadDraft::new(LedgerTransition::Recovered, "writer_recovery", fields)
                .map_err(|_| {
                    GlobalLedgerError::fatal("recovery_event_failed", "build_recovery_event")
                })?;
        let draft = EventDraft::new(
            format!("ledger-recovery-{now}-{}", self.next_sequence),
            now,
            EventType::LedgerRecovered,
            EventSeverity::Warning,
            EventOrigin::new(EventSource::System, "global-ledger", EventActor::System).map_err(
                |_| GlobalLedgerError::fatal("recovery_event_failed", "build_recovery_event"),
            )?,
            EventLinks::default(),
            payload,
        )
        .sanitize(&PublicOnlyRedactor)
        .map_err(|_| GlobalLedgerError::fatal("recovery_event_failed", "sanitize_recovery_event"))?
        .erase()?;
        self.append(draft)?;
        Ok(())
    }
}

struct PublicOnlyRedactor;

impl FieldRedactor for PublicOnlyRedactor {
    fn fingerprint(&self, _field_name: &str, _value: &str) -> Result<String, SanitizationError> {
        Err(SanitizationError::redactor_failure())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLine {
    line_type: String,
    event: PersistedEvent,
}

struct RecoveryState {
    next_sequence: u64,
    events: Vec<PersistedEvent>,
    last_segment_index: u64,
    truncated_tail: Option<TruncatedTail>,
}

struct TruncatedTail {
    segment_index: u64,
    bytes: u64,
}

fn recover_segments(root: &Path, segments_dir: &Path) -> GlobalLedgerResult<RecoveryState> {
    let segments = list_segments(segments_dir)?;
    let mut next_sequence = 1_u64;
    let mut event_ids = BTreeSet::new();
    let mut events = Vec::new();
    let mut truncated_tail = None;
    for (position, (index, path)) in segments.iter().enumerate() {
        let is_last = position + 1 == segments.len();
        let mut bytes = fs::read(path)
            .map_err(|error| GlobalLedgerError::io("ledger_io", "read_segment", &error))?;
        let complete_len = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |position| position + 1);
        let tail_len = bytes.len().saturating_sub(complete_len);
        if tail_len > 0 {
            if !is_last {
                return Err(GlobalLedgerError::fatal(
                    "corrupt_segment",
                    "recover_non_final_tail",
                ));
            }
            let tail = bytes.split_off(complete_len);
            quarantine_tail(root, *index, &tail)?;
            let file = OpenOptions::new().write(true).open(path).map_err(|error| {
                GlobalLedgerError::io("ledger_io", "open_segment_repair", &error)
            })?;
            file.set_len(complete_len as u64).map_err(|error| {
                GlobalLedgerError::io("ledger_io", "truncate_final_tail", &error)
            })?;
            file.sync_all()
                .map_err(|error| GlobalLedgerError::io("ledger_io", "sync_tail_repair", &error))?;
            truncated_tail = Some(TruncatedTail {
                segment_index: *index,
                bytes: tail.len() as u64,
            });
        }
        let complete_records = if complete_len == 0 {
            &bytes[..0]
        } else {
            &bytes[..complete_len - 1]
        };
        if complete_records.is_empty() && complete_len > 0 {
            return Err(GlobalLedgerError::fatal(
                "corrupt_segment",
                "parse_blank_record",
            ));
        }
        if complete_records.is_empty() {
            continue;
        }
        for line in complete_records.split(|byte| *byte == b'\n') {
            if line.is_empty() {
                return Err(GlobalLedgerError::fatal(
                    "corrupt_segment",
                    "parse_blank_record",
                ));
            }
            let unique = serde_json::from_slice::<UniqueJsonValue>(line).map_err(|error| {
                GlobalLedgerError::json("corrupt_segment", "parse_segment", &error)
            })?;
            let stored = serde_json::from_value::<StoredLine>(unique.0).map_err(|error| {
                GlobalLedgerError::json("corrupt_segment", "decode_segment", &error)
            })?;
            if stored.line_type != LINE_TYPE {
                return Err(GlobalLedgerError::fatal(
                    "corrupt_segment",
                    "validate_line_type",
                ));
            }
            stored.event.validate().map_err(|_| {
                GlobalLedgerError::fatal("corrupt_segment", "validate_persisted_event")
            })?;
            if stored.event.sequence != next_sequence {
                return Err(GlobalLedgerError::fatal(
                    "sequence_discontinuity",
                    "recover_sequence",
                ));
            }
            if !event_ids.insert(stored.event.event_id.clone()) {
                return Err(GlobalLedgerError::fatal(
                    "duplicate_event_id",
                    "recover_event_ids",
                ));
            }
            events.push(stored.event);
            next_sequence = increment_sequence(next_sequence)?;
        }
    }
    Ok(RecoveryState {
        next_sequence,
        events,
        last_segment_index: segments.last().map_or(1, |(index, _)| *index),
        truncated_tail,
    })
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

fn quarantine_tail(root: &Path, segment_index: u64, tail: &[u8]) -> GlobalLedgerResult<()> {
    let quarantine_dir = root.join("quarantine");
    fs::create_dir_all(&quarantine_dir)
        .map_err(|error| GlobalLedgerError::io("ledger_io", "create_quarantine", &error))?;
    for suffix in 0_u16..100 {
        let path = quarantine_dir.join(format!(
            "segment-{segment_index:06}-tail-{}-{suffix:02}.bin",
            unix_ms_now()?
        ));
        match OpenOptions::new().create_new(true).write(true).open(path) {
            Ok(mut file) => {
                file.write_all(tail).map_err(|error| {
                    GlobalLedgerError::io("ledger_io", "write_quarantine", &error)
                })?;
                file.sync_all().map_err(|error| {
                    GlobalLedgerError::io("ledger_io", "sync_quarantine", &error)
                })?;
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(GlobalLedgerError::io(
                    "ledger_io",
                    "create_quarantine_file",
                    &error,
                ));
            }
        }
    }
    Err(GlobalLedgerError::fatal(
        "quarantine_name_exhausted",
        "create_quarantine_file",
    ))
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
    let lifecycle_valid = match (metadata.active, metadata.closed_at_unix_ms) {
        (true, None) => true,
        (false, Some(closed_at)) => closed_at >= metadata.started_at_unix_ms,
        _ => false,
    };
    let valid = metadata.schema_version == WRITER_SCHEMA_VERSION
        && is_identifier(&metadata.owner_id)
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
            .write_all(br#"{"schema_version":"actingcommand.ledger-writer.v1"#)
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
