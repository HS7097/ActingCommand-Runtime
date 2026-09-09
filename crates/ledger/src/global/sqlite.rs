// SPDX-License-Identifier: AGPL-3.0-only

//! Candidate durable medium. Typed semantics remain in EventStore/GlobalLedger.

use super::projection::EventIndexes;
use super::read_only::check_read_budget;
use super::storage::{
    DurableStorage, EventStore, UniqueJsonValue, WriterOwnership, increment_sequence,
};
use super::{
    GlobalLedgerConfig, GlobalLedgerError, GlobalLedgerReadOnlyConfig, GlobalLedgerResult,
    MAX_QUERY_PAGE_EVENTS,
};
use crate::{PersistedEvent, fact::StoredEventRecord};
use actingcommand_contract::{
    EventQuery, GLOBAL_EVENT_SCHEMA_VERSION, ProjectedArtifactReference, RecoveryReason,
    VerifiedArtifactReference,
};
use actingcommand_runtime_database::{RuntimeDatabase, RuntimeDatabaseError};
use rusqlite::{
    Connection, TransactionBehavior, params_from_iter,
    types::{Value as SqlValue, ValueRef},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

const SCHEMA: &str = "actingcommand.sqlite-ledger.v1";
const INTEGER_ENCODING: &str = "ordered-u64-v1";
const EVENT_COLUMNS: &str = "sequence,event_id,timestamp_unix_ms,event_type,severity,sensitivity,origin_source,origin_module,origin_actor,payload_schema,canonical_record,record_sha256,previous_record_sha256,integrity_tag";
const LINK_COLUMNS: &str = "sequence,instance_id,request_id,correlation_id,causation_id,task_id,run_id,lease_id,frame_id,action_id,recognition_id";
const ARTIFACT_COLUMNS: &str = "sequence,ordinal,artifact_id,kind,run_id,frame_id,correlation_id,object_key,media_type,byte_count,sha256,created_at_unix_ms,producer,retention_class,redaction_state";
const META_COLUMNS: &str = "singleton,schema_version,next_sequence,head_sequence,head_record_sha256,storage_backend,migration_id,cutover_state,integer_encoding,integrity_tag";
type SqlRow = Vec<SqlValue>;
type ReadBudget = Option<(u64, usize, Instant)>;

pub(super) type SqliteLedgerStore = EventStore<SqliteStorage>;

pub(super) struct SqliteStorage {
    database: Arc<RuntimeDatabase>,
    ownership: WriterOwnership,
    head: u64,
    head_hash: Option<String>,
}

impl From<RuntimeDatabaseError> for GlobalLedgerError {
    fn from(error: RuntimeDatabaseError) -> Self {
        Self::fatal(error.code(), error.operation())
    }
}

impl SqliteLedgerStore {
    pub(super) fn open<F>(
        config: GlobalLedgerConfig,
        database: Arc<RuntimeDatabase>,
        mut verifier: Option<F>,
    ) -> GlobalLedgerResult<Self>
    where
        F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
    {
        validate_root(&config.root, &database)?;
        // Same physical database, same existing OS writer lock, including separate Arcs.
        let (mut ownership, stale_owner) =
            WriterOwnership::acquire(database.root(), &config.owner_id)?;
        let recovered = (|| {
            initialize(&database)?;
            let raw = read_snapshot(&database, None)?;
            verify_snapshot(&database, raw, &mut verifier)
        })();
        let (events, head_hash) = match recovered {
            Ok(recovered) => recovered,
            Err(error) => {
                return Err(error.with_close_result(ownership.close()));
            }
        };
        let head = events.last().map_or(0, PersistedEvent::sequence);
        let next = increment_sequence(head)?;
        let backend = SqliteStorage {
            database,
            ownership,
            head,
            head_hash,
        };
        let mut store = Self::recovered(backend, next, events)?;
        if let Some(previous_owner) = stale_owner
            && let Err(error) =
                store.append_recovery(RecoveryReason::StaleOwner, Some(previous_owner), None, None)
        {
            return Err(error.with_close_result(store.backend.close()));
        }
        Ok(store)
    }
}

impl DurableStorage for SqliteStorage {
    fn persist(&mut self, event: &PersistedEvent) -> GlobalLedgerResult<Option<u64>> {
        let next = increment_sequence(event.sequence())?;
        if event.sequence() != increment_sequence(self.head)? {
            return Err(failure("sequence_discontinuity", "append_sqlite_event"));
        }
        let projected = project_record(&self.database, event, self.head_hash.as_deref())?;
        let started = Instant::now();
        let mut connection = self.database.connection("append_sqlite_event")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| sql_error(error, "begin_sqlite_append"))?;
        let current = read_meta(&transaction)?;
        if current
            != meta_row(
                &self.database,
                event.sequence(),
                self.head,
                self.head_hash.as_deref(),
            )
        {
            return Err(failure("ledger_meta_mismatch", "append_sqlite_event"));
        }
        insert_row(
            &transaction,
            "ledger_events",
            EVENT_COLUMNS,
            &projected.event,
        )?;
        insert_row(&transaction, "ledger_links", LINK_COLUMNS, &projected.links)?;
        for artifact in &projected.artifacts {
            insert_row(&transaction, "ledger_artifacts", ARTIFACT_COLUMNS, artifact)?;
        }
        let meta = meta_row(
            &self.database,
            next,
            event.sequence(),
            Some(&projected.hash),
        );
        let changed = transaction.execute(
            "UPDATE ledger_meta SET schema_version=?2,next_sequence=?3,head_sequence=?4,head_record_sha256=?5,storage_backend=?6,migration_id=?7,cutover_state=?8,integer_encoding=?9,integrity_tag=?10 WHERE singleton=?1",
            params_from_iter(meta.iter()),
        ).map_err(|error| sql_error(error, "update_sqlite_head"))?;
        if changed != 1 {
            return Err(failure("ledger_meta_mismatch", "update_sqlite_head"));
        }
        transaction
            .commit()
            .map_err(|error| sql_error(error, "commit_sqlite_event"))?;
        #[cfg(test)]
        super::storage::repair_test_barrier("after_sqlite_commit")?;
        let elapsed = Instant::now()
            .checked_duration_since(started)
            .and_then(|value| u64::try_from(value.as_nanos()).ok());
        self.head = event.sequence();
        self.head_hash = Some(projected.hash);
        // The common store updates indexes/statistics only after this durable return.
        Ok(elapsed)
    }

    fn close(&mut self) -> GlobalLedgerResult<()> {
        // Every append committed under WAL/FULL; the shared connection belongs to RuntimeDatabase.
        self.ownership.close()
    }
}

/// Verified immutable candidate facts. Physical observations belong to their backend.
pub struct SqliteLedgerReadOnly {
    events: Vec<PersistedEvent>,
    indexes: EventIndexes,
}

impl SqliteLedgerReadOnly {
    pub(super) fn open<F>(
        config: GlobalLedgerReadOnlyConfig,
        database: Arc<RuntimeDatabase>,
        mut verifier: F,
    ) -> GlobalLedgerResult<Self>
    where
        F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
    {
        validate_root(&config.root, &database)?;
        let raw = read_snapshot(&database, config.budget)?;
        let (events, _) = verify_snapshot(&database, raw, &mut Some(&mut verifier))?;
        Ok(Self {
            indexes: EventIndexes::from_events(&events),
            events,
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
}

fn validate_root(root: &std::path::Path, database: &RuntimeDatabase) -> GlobalLedgerResult<()> {
    let root = root
        .canonicalize()
        .map_err(|error| GlobalLedgerError::io("ledger_io", "resolve_candidate_root", &error))?;
    if root.join("segments").exists() {
        return Err(failure(
            "candidate_segment_root_present",
            "open_sqlite_ledger",
        ));
    }
    let database_root = database
        .root()
        .canonicalize()
        .map_err(|error| GlobalLedgerError::io("ledger_io", "resolve_database_root", &error))?;
    if root != database_root {
        return Err(failure(
            "ledger_database_root_mismatch",
            "open_sqlite_ledger",
        ));
    }
    Ok(())
}

fn initialize(database: &RuntimeDatabase) -> GlobalLedgerResult<()> {
    let mut connection = database.connection("initialize_sqlite_ledger")?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sql_error(error, "begin_ledger_schema"))?;
    let tables: i64 = transaction.query_row("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN ('ledger_events','ledger_links','ledger_artifacts','ledger_meta')", [], |row| row.get(0))
        .map_err(|error| sql_error(error, "inspect_ledger_schema"))?;
    match tables {
        0 => {
            transaction
                .execute_batch(include_str!("sqlite/schema.sql"))
                .map_err(|error| sql_error(error, "initialize_ledger_schema"))?;
            insert_row(
                &transaction,
                "ledger_meta",
                META_COLUMNS,
                &meta_row(database, 1, 0, None),
            )?;
        }
        4 => {}
        _ => {
            return Err(failure(
                "ledger_schema_incomplete",
                "initialize_sqlite_ledger",
            ));
        }
    }
    transaction
        .commit()
        .map_err(|error| sql_error(error, "commit_ledger_schema"))
}

struct RawSnapshot {
    events: Vec<SqlRow>,
    links: Vec<SqlRow>,
    artifacts: Vec<SqlRow>,
    meta: SqlRow,
    budget: ReadBudget,
    bytes: u64,
}

fn read_snapshot(
    database: &RuntimeDatabase,
    budget: ReadBudget,
) -> GlobalLedgerResult<RawSnapshot> {
    check_read_budget(budget, 0, 0)?;
    let mut connection = database.connection("read_sqlite_snapshot")?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|error| sql_error(error, "begin_sqlite_snapshot"))?;
    let mut bytes = 0;
    let meta = read_meta_with_budget(&transaction, budget, &mut bytes)?;
    let events = read_rows(
        &transaction,
        &format!("SELECT {EVENT_COLUMNS} FROM ledger_events ORDER BY sequence"),
        budget,
        &mut bytes,
        true,
    )?;
    let links = read_rows(
        &transaction,
        &format!("SELECT {LINK_COLUMNS} FROM ledger_links ORDER BY sequence"),
        budget,
        &mut bytes,
        false,
    )?;
    let artifacts = read_rows(
        &transaction,
        &format!("SELECT {ARTIFACT_COLUMNS} FROM ledger_artifacts ORDER BY sequence,ordinal"),
        budget,
        &mut bytes,
        false,
    )?;
    transaction
        .commit()
        .map_err(|error| sql_error(error, "close_sqlite_snapshot"))?;
    // Release the connection before typed reconstruction invokes the artifact owner.
    Ok(RawSnapshot {
        events,
        links,
        artifacts,
        meta,
        budget,
        bytes,
    })
}

fn read_rows(
    connection: &Connection,
    sql: &str,
    budget: ReadBudget,
    bytes: &mut u64,
    count_events: bool,
) -> GlobalLedgerResult<Vec<SqlRow>> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| sql_error(error, "prepare_sqlite_read"))?;
    let columns = statement.column_count();
    let mut rows = statement
        .query([])
        .map_err(|error| sql_error(error, "query_sqlite_rows"))?;
    let mut result = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|error| sql_error(error, "read_sqlite_row"))?
    {
        check_read_budget(
            budget,
            *bytes,
            if count_events { result.len() + 1 } else { 0 },
        )?;
        let mut values = Vec::with_capacity(columns);
        for index in 0..columns {
            let value = row
                .get_ref(index)
                .map_err(|error| sql_error(error, "decode_sqlite_column"))?;
            let length = match value {
                ValueRef::Null => 0,
                ValueRef::Integer(_) | ValueRef::Real(_) => 8,
                ValueRef::Text(value) | ValueRef::Blob(value) => value.len() as u64,
            };
            *bytes = bytes
                .checked_add(length)
                .ok_or_else(|| failure("ledger_snapshot_overflow", "count_sqlite_bytes"))?;
            check_read_budget(budget, *bytes, 0)?;
            values.push(
                SqlValue::try_from(value)
                    .map_err(|_| failure("invalid_sqlite_value", "decode_sqlite_column"))?,
            );
        }
        result.push(values);
    }
    Ok(result)
}

fn read_meta(connection: &Connection) -> GlobalLedgerResult<SqlRow> {
    read_meta_with_budget(connection, None, &mut 0)
}

fn read_meta_with_budget(
    connection: &Connection,
    budget: ReadBudget,
    bytes: &mut u64,
) -> GlobalLedgerResult<SqlRow> {
    let mut rows = read_rows(
        connection,
        &format!("SELECT {META_COLUMNS} FROM ledger_meta ORDER BY singleton"),
        budget,
        bytes,
        false,
    )?;
    if rows.len() != 1 {
        return Err(failure("ledger_meta_missing", "read_sqlite_meta"));
    }
    Ok(rows.remove(0))
}

fn verify_snapshot<F>(
    database: &RuntimeDatabase,
    raw: RawSnapshot,
    verifier: &mut Option<F>,
) -> GlobalLedgerResult<(Vec<PersistedEvent>, Option<String>)>
where
    F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
{
    let mut events = Vec::with_capacity(raw.events.len());
    let mut ids = BTreeSet::new();
    let mut next = 1;
    let mut head_hash: Option<String> = None;
    let mut expected_links = Vec::new();
    let mut expected_artifacts = Vec::new();
    for row in raw.events {
        check_read_budget(raw.budget, raw.bytes, events.len() + 1)?;
        let Some(SqlValue::Blob(bytes)) = row.get(10) else {
            return Err(failure("corrupt_ledger_record", "read_canonical_record"));
        };
        let value = serde_json::from_slice::<UniqueJsonValue>(bytes)
            .map_err(|error| {
                GlobalLedgerError::json("corrupt_ledger_record", "parse_sqlite_record", &error)
            })?
            .0;
        if value.get("schema_version").and_then(Value::as_str) != Some(GLOBAL_EVENT_SCHEMA_VERSION)
        {
            return Err(failure("unsupported_event_schema", "recover_event_schema"));
        }
        let stored: StoredEventRecord = serde_json::from_value(value).map_err(|error| {
            GlobalLedgerError::json("corrupt_ledger_record", "decode_sqlite_record", &error)
        })?;
        let event = match verifier.as_mut() {
            Some(verifier) => stored.into_event_with_artifact_verifier(verifier),
            None => stored.into_event(),
        }
        .map_err(|error| failure(error.code(), "validate_persisted_event"))?;
        let Some(SqlValue::Integer(stored_sequence)) = row.first() else {
            return Err(failure("invalid_event_integer", "recover_sqlite_sequence"));
        };
        if decode(*stored_sequence) != event.sequence() {
            return Err(failure("ledger_record_mismatch", "recover_sqlite_sequence"));
        }
        if event.sequence() != next {
            return Err(failure("sequence_discontinuity", "recover_sequence"));
        }
        if !ids.insert(*event.event_id()) {
            return Err(failure("duplicate_event_id", "recover_event_ids"));
        }
        let projected = project_record(database, &event, head_hash.as_deref())?;
        if row != projected.event {
            return Err(failure("ledger_record_mismatch", "verify_sqlite_record"));
        }
        check_read_budget(raw.budget, raw.bytes, events.len() + 1)?;
        expected_links.push(projected.links);
        expected_artifacts.extend(projected.artifacts);
        head_hash = Some(projected.hash);
        events.push(event);
        next = increment_sequence(next)?;
    }
    if raw.links != expected_links || raw.artifacts != expected_artifacts {
        return Err(failure("ledger_index_mismatch", "verify_sqlite_relations"));
    }
    if raw.meta
        != meta_row(
            database,
            next,
            events.last().map_or(0, PersistedEvent::sequence),
            head_hash.as_deref(),
        )
    {
        return Err(failure("ledger_meta_mismatch", "verify_sqlite_head"));
    }
    Ok((events, head_hash))
}

struct ProjectedRecord {
    event: SqlRow,
    links: SqlRow,
    artifacts: Vec<SqlRow>,
    hash: String,
}

fn project_record(
    database: &RuntimeDatabase,
    event: &PersistedEvent,
    previous: Option<&str>,
) -> GlobalLedgerResult<ProjectedRecord> {
    let stored = StoredEventRecord::from_event(event);
    let bytes = serde_json::to_vec(&stored).map_err(|error| {
        GlobalLedgerError::json("event_serialization_failed", "serialize_event", &error)
    })?;
    let value = serde_json::to_value(&stored).map_err(|error| {
        GlobalLedgerError::json(
            "event_serialization_failed",
            "project_sqlite_record",
            &error,
        )
    })?;
    let hash = format!("sha256:{:x}", Sha256::digest(&bytes));
    let tag = database.integrity_tag(
        "ledger-event-v1",
        &[&bytes, hash.as_bytes(), previous.unwrap_or("").as_bytes()],
    );
    let sequence = SqlValue::Integer(encode(event.sequence()));
    let row = vec![
        sequence.clone(),
        text(&value, "event_id")?,
        SqlValue::Integer(encode(event.timestamp_unix_ms())),
        text(&value, "event_type")?,
        text(&value, "severity")?,
        text(&value, "sensitivity")?,
        text(&value["origin"], "source")?,
        text(&value["origin"], "module")?,
        text(&value["origin"], "actor")?,
        text(&value, "payload_schema")?,
        SqlValue::Blob(bytes),
        SqlValue::Text(hash.clone()),
        optional_text(previous),
        SqlValue::Text(tag),
    ];
    let mut links = vec![sequence.clone()];
    for name in [
        "instance_id",
        "request_id",
        "correlation_id",
        "causation_id",
        "task_id",
        "run_id",
        "lease_id",
        "frame_id",
        "action_id",
        "recognition_id",
    ] {
        links.push(nullable_text(&value["links"], name)?);
    }
    let mut artifacts = Vec::new();
    let Some(references) = value["artifacts"].as_array() else {
        return Err(failure("invalid_artifact_record", "project_sqlite_record"));
    };
    for (ordinal, artifact) in references.iter().enumerate() {
        artifacts.push(vec![
            sequence.clone(),
            SqlValue::Integer(encode(ordinal as u64)),
            text(artifact, "artifact_id")?,
            text(artifact, "kind")?,
            nullable_text(artifact, "run_id")?,
            nullable_text(artifact, "frame_id")?,
            nullable_text(artifact, "correlation_id")?,
            text(artifact, "object_key")?,
            text(artifact, "media_type")?,
            integer(artifact, "byte_count")?,
            text(artifact, "sha256")?,
            integer(artifact, "created_at_unix_ms")?,
            text(artifact, "producer")?,
            text(artifact, "retention_class")?,
            text(artifact, "redaction_state")?,
        ]);
    }
    Ok(ProjectedRecord {
        event: row,
        links,
        artifacts,
        hash,
    })
}

fn meta_row(database: &RuntimeDatabase, next: u64, head: u64, hash: Option<&str>) -> SqlRow {
    let tag = database.integrity_tag(
        "ledger-meta-v1",
        &[
            SCHEMA.as_bytes(),
            &next.to_be_bytes(),
            &head.to_be_bytes(),
            hash.unwrap_or("").as_bytes(),
            b"sqlite",
            b"candidate",
            INTEGER_ENCODING.as_bytes(),
        ],
    );
    vec![
        SqlValue::Integer(1),
        SqlValue::Text(SCHEMA.into()),
        SqlValue::Integer(encode(next)),
        SqlValue::Integer(encode(head)),
        optional_text(hash),
        SqlValue::Text("sqlite".into()),
        SqlValue::Null,
        SqlValue::Text("candidate".into()),
        SqlValue::Text(INTEGER_ENCODING.into()),
        SqlValue::Text(tag),
    ]
}

fn insert_row(
    connection: &Connection,
    table: &'static str,
    columns: &'static str,
    row: &[SqlValue],
) -> GlobalLedgerResult<()> {
    let placeholders = (1..=row.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(",");
    let changed = connection
        .execute(
            &format!("INSERT INTO {table} ({columns}) VALUES ({placeholders})"),
            params_from_iter(row.iter()),
        )
        .map_err(|error| sql_error(error, "insert_sqlite_record"))?;
    if changed != 1 {
        return Err(failure("ledger_insert_incomplete", "insert_sqlite_record"));
    }
    Ok(())
}

// Full-domain order-preserving bijection; canonical and public values remain u64.
fn encode(value: u64) -> i64 {
    (value ^ (1_u64 << 63)) as i64
}
fn decode(value: i64) -> u64 {
    (value as u64) ^ (1_u64 << 63)
}
fn optional_text(value: Option<&str>) -> SqlValue {
    value.map_or(SqlValue::Null, |value| SqlValue::Text(value.into()))
}
fn text(value: &Value, key: &str) -> GlobalLedgerResult<SqlValue> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(|value| SqlValue::Text(value.into()))
        .ok_or_else(|| failure("invalid_event_column", "project_sqlite_record"))
}
fn nullable_text(value: &Value, key: &str) -> GlobalLedgerResult<SqlValue> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(SqlValue::Null),
        Some(Value::String(value)) => Ok(SqlValue::Text(value.clone())),
        _ => Err(failure("invalid_event_link", "project_sqlite_record")),
    }
}
fn integer(value: &Value, key: &str) -> GlobalLedgerResult<SqlValue> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| SqlValue::Integer(encode(value)))
        .ok_or_else(|| failure("invalid_event_integer", "project_sqlite_record"))
}
fn failure(code: &'static str, operation: &'static str) -> GlobalLedgerError {
    GlobalLedgerError::fatal(code, operation)
}
fn sql_error(error: rusqlite::Error, operation: &'static str) -> GlobalLedgerError {
    let detail = match error {
        rusqlite::Error::SqliteFailure(code, _) => {
            Some(format!("SQLite extended code {}", code.extended_code))
        }
        _ => None,
    };
    GlobalLedgerError {
        code: "ledger_sqlite_failed",
        operation,
        detail,
        terminal: true,
    }
}

#[cfg(test)]
#[path = "sqlite/tests.rs"]
mod tests;
