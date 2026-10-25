// SPDX-License-Identifier: AGPL-3.0-only

//! Candidate durable medium. Typed semantics remain in EventStore/GlobalLedger.

use super::history::{HistorySelection, LabRelations};
use super::index::{IndexChange, IndexRows};
use super::projection::{EventIndexes, LAB_ANCHOR_SOURCE, LAB_ANCHOR_TYPE, LabKeys};
use super::read_only::check_read_budget;
use super::read_work::ReadWork;
use super::retention::RetentionIndex;
use super::retention::{LedgerOpenTiming, elapsed_ms};
use super::storage::{
    DurableStorage, EventStore, ReadBack, UniqueJsonValue, WriterOwnership, increment_sequence,
};
use super::{
    GlobalLedgerConfig, GlobalLedgerError, GlobalLedgerReadOnlyConfig, GlobalLedgerResult,
    LedgerProjectViewCount, LedgerProjectViewObservation, MAX_QUERY_PAGE_EVENTS,
};
use crate::codes::{LedgerCode, LedgerLocation};
use crate::{
    PersistedEvent,
    fact::{LedgerEventMetadata, LedgerEventRead, StoredEventRecord},
};
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::{Code, IoKind, Location};
use actingcommand_contract::{
    EventId, EventQuery, EventType, GLOBAL_EVENT_SCHEMA_VERSION, LedgerMaterialReadState,
    LedgerReadScope, LedgerReadSource, LedgerView, ProjectedArtifactReference, ProjectionProfile,
    RecoveryReason, RunId, RuntimeEventQueryPage, RuntimeEventQueryPageRequest,
    VerifiedArtifactReference,
};
use actingcommand_runtime_database::{
    CommitSync, RuntimeDatabase, RuntimeDatabaseError, RuntimeTransaction,
};
use rusqlite::{
    Connection, TransactionBehavior, params_from_iter,
    types::{Value as SqlValue, ValueRef},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod release_source;
// LN-1 (Workflow #422): the scale harness's synthetic ledger commits its batches here.
#[cfg(any(test, feature = "test-hooks"))]
pub(super) mod synthetic;
mod tagged;
mod views;
pub use release_source::*;

// Interface anchor `ledger` (contracts/component-interfaces.md): raise its revision in
// distribution/windows/component-interfaces.json when older readers can no longer read this.
// Ledger revision 3 (Workflow #378 A3): SQLite format 2 (`user_version` 2).
const FORMAL_FORMAT_VERSION: i64 = 2;
/// The format v0.11 wrote (ledger revision 2). A database whose ledger tables exist under it is
/// refused with `ledger_format_unsupported` after reading only its format and before any ledger
/// row is read or written, and the Runtime's start refuses it before its owner journal or writer
/// lock; there is no compatibility path (Workflow #378 A3, model section 8.1).
const UNSUPPORTED_FORMAT_VERSION: i64 = 1;
const SCHEMA: &str = "actingcommand.sqlite-ledger.v1";
const INTEGER_ENCODING: &str = "ordered-u64-v1";
const EVENT_COLUMNS: &str = "sequence,event_id,timestamp_unix_ms,event_type,severity,sensitivity,origin_source,origin_module,origin_actor,payload_schema,canonical_record,record_sha256,previous_record_sha256,integrity_tag";
const LINK_COLUMNS: &str = "sequence,instance_id,request_id,correlation_id,causation_id,task_id,run_id,lease_id,frame_id,action_id,recognition_id";
const ARTIFACT_COLUMNS: &str = "sequence,ordinal,artifact_id,kind,run_id,frame_id,correlation_id,object_key,media_type,byte_count,sha256,created_at_unix_ms,producer,retention_class,redaction_state";
const META_COLUMNS: &str = "singleton,schema_version,next_sequence,head_sequence,head_record_sha256,storage_backend,migration_id,cutover_state,integer_encoding,integrity_tag,migration_record";
type SqlRow = Vec<SqlValue>;
type ReadBudget = Option<(u64, usize, Instant)>;

/// Checks an already verified opaque fact against its rows in the caller's transaction:
/// the stored row is authenticated by hash, tag and predecessor, its index, link and
/// artifact rows must match the fact, and so must its stored record (payload and schema
/// version included): equal canonical bytes accept, otherwise the strictly decoded stored
/// record must equal the fact's. Re-serialization alone never rejects a row.
/// This synchronous read borrows the existing owner; it does not lock, commit, append,
/// or read artifact bytes. Full-ledger recovery remains the source of the input fact.
pub fn verify_transaction_event(
    database: &RuntimeDatabase,
    transaction: &RuntimeTransaction<'_, '_>,
    event: &PersistedEvent,
) -> GlobalLedgerResult<()> {
    verify_transaction_row(database, transaction, event).map(|_| ())
}

/// `verify_transaction_event`, returning the authenticated stored row's record hash.
fn verify_transaction_row(
    database: &RuntimeDatabase,
    transaction: &RuntimeTransaction<'_, '_>,
    event: &PersistedEvent,
) -> GlobalLedgerResult<String> {
    const OPERATION: Location = LedgerLocation::VerifyTransactionEvent.location();
    if !transaction.belongs_to(database) {
        return Err(failure(
            LedgerCode::LedgerTransactionOwnerMismatch.code(),
            OPERATION,
        ));
    }
    let connection = transaction.sql();
    refuse_unsupported_format(connection)?;
    let meta = read_meta(connection)?;
    let marker = SqliteMarker::parse(&meta)?;
    let expected_format = if marker.state == "ready" {
        FORMAL_FORMAT_VERSION
    } else {
        0
    };
    if format_version(connection)? != expected_format {
        return Err(failure(
            LedgerCode::LedgerFormatMarkerMismatch.code(),
            OPERATION,
        ));
    }
    let (
        Some(SqlValue::Integer(next)),
        Some(SqlValue::Integer(head)),
        Some(SqlValue::Text(head_hash)),
    ) = (meta.get(2), meta.get(3), meta.get(4))
    else {
        return Err(failure(LedgerCode::LedgerMetaMismatch.code(), OPERATION));
    };
    let head = decode(*head);
    if head == 0
        || decode(*next) != increment_sequence(head)?
        || meta != meta_row_with_marker(database, decode(*next), head, Some(head_hash), &marker)
    {
        return Err(failure(LedgerCode::LedgerMetaMismatch.code(), OPERATION));
    }
    if event.sequence() == 0 || event.sequence() > head {
        return Err(failure(LedgerCode::LedgerRecordMissing.code(), OPERATION));
    }
    let sequence = encode(event.sequence());
    let mut bytes = 0;
    let rows = read_rows(
        connection,
        &format!("SELECT {EVENT_COLUMNS} FROM ledger_events WHERE sequence={sequence}"),
        None,
        &mut bytes,
        true,
    )?;
    let [row] = rows.as_slice() else {
        return Err(failure(LedgerCode::LedgerRecordMissing.code(), OPERATION));
    };
    let previous = if event.sequence() == 1 {
        None
    } else {
        let previous_sequence = encode(event.sequence() - 1);
        let rows = read_rows(
            connection,
            &format!("SELECT record_sha256 FROM ledger_events WHERE sequence={previous_sequence}"),
            None,
            &mut bytes,
            true,
        )?;
        match rows.as_slice() {
            [row] => match row.as_slice() {
                [SqlValue::Text(hash)] => Some(hash.clone()),
                _ => return Err(failure(LedgerCode::LedgerRecordMismatch.code(), OPERATION)),
            },
            _ => return Err(failure(LedgerCode::LedgerRecordMissing.code(), OPERATION)),
        }
    };
    let Some(SqlValue::Blob(stored)) = row.get(10) else {
        return Err(failure(LedgerCode::LedgerRecordMismatch.code(), OPERATION));
    };
    let expected = StoredEventRecord::from_event(event);
    let projected = project_stored_bytes(database, &expected, stored.clone(), previous.as_deref())?;
    if *row != projected.event || (event.sequence() == head && projected.hash != *head_hash) {
        return Err(failure(LedgerCode::LedgerRecordMismatch.code(), OPERATION));
    }
    let links = read_rows(
        connection,
        &format!("SELECT {LINK_COLUMNS} FROM ledger_links WHERE sequence={sequence}"),
        None,
        &mut bytes,
        true,
    )?;
    let artifact_limit = event.artifacts().len().saturating_add(1);
    let artifacts = read_rows(
        connection,
        &format!(
            "SELECT {ARTIFACT_COLUMNS} FROM ledger_artifacts WHERE sequence={sequence} ORDER BY ordinal LIMIT {artifact_limit}"
        ),
        None,
        &mut bytes,
        false,
    )?;
    if links != vec![projected.links] || artifacts != projected.artifacts {
        return Err(failure(LedgerCode::LedgerIndexMismatch.code(), OPERATION));
    }
    // Canonical bytes are a sufficient condition only; a non-canonical stored row (an
    // older shape of the same value) is compared as its strictly decoded record.
    let canonical = serde_json::to_vec(&expected).map_err(|error| {
        GlobalLedgerError::json(
            LedgerCode::EventSerializationFailed.code(),
            OPERATION,
            &error,
        )
    })?;
    if canonical != *stored {
        let value = serde_json::from_slice::<UniqueJsonValue>(stored)
            .map_err(|error| {
                GlobalLedgerError::json(LedgerCode::CorruptLedgerRecord.code(), OPERATION, &error)
            })?
            .0;
        let decoded: StoredEventRecord = serde_json::from_value(value).map_err(|error| {
            GlobalLedgerError::json(LedgerCode::CorruptLedgerRecord.code(), OPERATION, &error)
        })?;
        if decoded != expected {
            return Err(failure(LedgerCode::LedgerRecordMismatch.code(), OPERATION));
        }
    }
    Ok(projected.hash)
}

#[derive(Clone)]
struct SqliteMarker {
    state: &'static str,
    migration: Option<Box<actingcommand_contract::LedgerMigrationRecord>>,
    material: Option<String>,
}

impl SqliteMarker {
    fn candidate() -> Self {
        Self {
            state: "candidate",
            migration: None,
            material: None,
        }
    }
    fn empty() -> Self {
        Self {
            state: "ready",
            migration: None,
            material: None,
        }
    }
    fn migrated(
        record: &actingcommand_contract::LedgerMigrationRecord,
    ) -> GlobalLedgerResult<Self> {
        record.validate().map_err(|error| {
            failure(
                error.code(),
                LedgerLocation::ValidateCutoverMarker.location(),
            )
        })?;
        Ok(Self {
            state: "ready",
            migration: Some(Box::new(record.clone())),
            material: Some(serde_json::to_string(record).map_err(|error| {
                GlobalLedgerError::json(
                    LedgerCode::InvalidCutoverMarker.code(),
                    LedgerLocation::EncodeCutoverMarker.location(),
                    &error,
                )
            })?),
        })
    }
    fn parse(row: &SqlRow) -> GlobalLedgerResult<Self> {
        let marker = match (row.get(7), row.get(10)) {
            (Some(SqlValue::Text(state)), Some(SqlValue::Null)) if state == "candidate" => {
                Self::candidate()
            }
            (Some(SqlValue::Text(state)), Some(SqlValue::Null)) if state == "ready" => {
                Self::empty()
            }
            (Some(SqlValue::Text(state)), Some(SqlValue::Text(material))) if state == "ready" => {
                let unique: UniqueJsonValue = serde_json::from_str(material).map_err(|error| {
                    GlobalLedgerError::json(
                        LedgerCode::InvalidCutoverMarker.code(),
                        LedgerLocation::ReadCutoverMarker.location(),
                        &error,
                    )
                })?;
                let record: actingcommand_contract::LedgerMigrationRecord =
                    serde_json::from_value(unique.0).map_err(|error| {
                        GlobalLedgerError::json(
                            LedgerCode::InvalidCutoverMarker.code(),
                            LedgerLocation::DecodeCutoverMarker.location(),
                            &error,
                        )
                    })?;
                record.validate().map_err(|error| {
                    failure(
                        error.code(),
                        LedgerLocation::ValidateCutoverMarker.location(),
                    )
                })?;
                // The stored text is authoritative: the keyed meta tag covers these exact
                // bytes, and the decoded record is never re-encoded for comparison.
                Self {
                    state: "ready",
                    migration: Some(Box::new(record)),
                    material: Some(material.clone()),
                }
            }
            _ => {
                return Err(failure(
                    LedgerCode::InvalidCutoverMarker.code(),
                    LedgerLocation::ReadCutoverMarker.location(),
                ));
            }
        };
        if row.get(6)
            != Some(&optional_text(
                marker
                    .migration
                    .as_ref()
                    .map(|record| record.migration_id.as_str()),
            ))
        {
            return Err(failure(
                LedgerCode::MigrationIdentityMismatch.code(),
                LedgerLocation::ReadCutoverMarker.location(),
            ));
        }
        Ok(marker)
    }
    /// Import side: the prefix identity is taken over the write-side canonical bytes that
    /// the import is about to store.
    fn verify_events(&self, events: &[PersistedEvent]) -> GlobalLedgerResult<()> {
        let prefix = match &self.migration {
            Some(record) => {
                let mut digest = super::migration::PrefixDigest::new();
                for event in events {
                    if digest.count() == record.source_event_count {
                        break;
                    }
                    digest.push(&super::migration::canonical_record(event)?);
                }
                Some(digest.finish())
            }
            None => None,
        };
        self.verify_records(
            &events
                .iter()
                .map(StoredEventRecord::from_event)
                .collect::<Vec<_>>(),
            prefix.as_ref(),
        )
    }
    /// `prefix` is the identity of the first `source_event_count` records' bytes: the stored
    /// blobs on verification, the write-side canonical bytes on import.
    fn verify_records(
        &self,
        events: &[StoredEventRecord],
        prefix: Option<&super::migration::PrefixIdentity>,
    ) -> GlobalLedgerResult<()> {
        if let Some(record) = &self.migration {
            let prefix_length = usize::try_from(record.source_event_count).map_err(|_| {
                failure(
                    LedgerCode::MigrationPrefixInvalid.code(),
                    LedgerLocation::VerifyCutoverPrefix.location(),
                )
            })?;
            events.get(..prefix_length).ok_or_else(|| {
                failure(
                    LedgerCode::MigrationPrefixMissing.code(),
                    LedgerLocation::VerifyCutoverPrefix.location(),
                )
            })?;
            let completion = events.get(prefix_length).ok_or_else(|| {
                failure(
                    LedgerCode::MigrationCompletionMissing.code(),
                    LedgerLocation::VerifyCutoverPrefix.location(),
                )
            })?;
            let payload_record = match completion.payload() {
                actingcommand_contract::EventPayload::Ledger(
                    actingcommand_contract::LedgerPayload::Recovered(payload),
                ) => payload.migration(),
                _ => None,
            };
            let Some(prefix) = prefix.filter(|prefix| prefix.count == record.source_event_count)
            else {
                return Err(failure(
                    LedgerCode::MigrationPrefixMissing.code(),
                    LedgerLocation::VerifyCutoverPrefix.location(),
                ));
            };
            if completion.sequence() != record.cutover_sequence
                || payload_record != Some(record.as_ref())
                || prefix.content_sha256 != record.imported_content_sha256
                || prefix.head_sha256 != record.source_head_sha256
            {
                return Err(failure(
                    LedgerCode::MigrationPrefixMismatch.code(),
                    LedgerLocation::VerifyCutoverPrefix.location(),
                ));
            }
        }
        Ok(())
    }
    fn status(
        &self,
        events: &[PersistedEvent],
        hash: Option<String>,
    ) -> super::LedgerStorageStatus {
        if self.state == "candidate" {
            super::LedgerStorageStatus::Candidate
        } else {
            super::LedgerStorageStatus::Ready {
                head_sequence: events.last().map_or(0, PersistedEvent::sequence),
                head_sha256: hash,
                migration: self.migration.clone(),
            }
        }
    }
}

fn table_count(connection: &Connection) -> GlobalLedgerResult<i64> {
    connection.query_row("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN ('ledger_events','ledger_links','ledger_artifacts','ledger_meta')", [], |row| row.get(0)).map_err(|error| sql_error(error, LedgerLocation::InspectLedgerSchema.location()))
}

fn format_version(connection: &Connection) -> GlobalLedgerResult<i64> {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| sql_error(error, LedgerLocation::ReadDatabaseFormat.location()))
}

/// `ledger_format_unsupported` (fatal): the database holds a format-1 ledger.
fn unsupported_format() -> GlobalLedgerError {
    failure(
        LedgerCode::LedgerFormatUnsupported.code(),
        LedgerLocation::VerifyDatabaseFormat.location(),
    )
}

/// Refuses a format-1 ledger before anything else of the database is read or written.
fn refuse_unsupported_format(connection: &Connection) -> GlobalLedgerResult<()> {
    if format_version(connection)? == UNSUPPORTED_FORMAT_VERSION && table_count(connection)? != 0 {
        return Err(unsupported_format());
    }
    Ok(())
}

fn require_empty_format(connection: &Connection) -> GlobalLedgerResult<()> {
    refuse_unsupported_format(connection)?;
    if table_count(connection)? != 0 || format_version(connection)? != 0 {
        return Err(failure(
            LedgerCode::LedgerInitializationConflict.code(),
            LedgerLocation::VerifyEmptyLedgerFormat.location(),
        ));
    }
    Ok(())
}

fn mark_formal_format(connection: &Connection) -> GlobalLedgerResult<()> {
    connection
        .pragma_update(None, "user_version", FORMAL_FORMAT_VERSION)
        .map_err(|error| sql_error(error, LedgerLocation::MarkDatabaseFormat.location()))
}

/// Workflow #378 A3 (acceptance CL-M1): refuses a format-1 ledger under `state_root` before
/// the Runtime's start writes its owner journal or takes the writer lock. It reads only the
/// database's `user_version` and its ledger table count, through a read-only open without the
/// whole-file check; a root without the runtime database is fresh and passes, and nothing is
/// created.
pub fn refuse_unsupported_ledger_format(state_root: &std::path::Path) -> GlobalLedgerResult<()> {
    let present = state_root
        .join(actingcommand_runtime_database::DATABASE_FILE)
        .try_exists()
        .map_err(|error| {
            GlobalLedgerError::io(
                LedgerCode::LedgerIo.code(),
                LedgerLocation::InspectRuntimeLedger.location(),
                &error,
            )
        })?;
    if !present {
        return Ok(());
    }
    let database = RuntimeDatabase::open_existing_unchecked(state_root)?;
    let connection = database.connection(LedgerLocation::ReadDatabaseFormat.location())?;
    refuse_unsupported_format(&connection)
}

pub(super) fn has_schema(database: &RuntimeDatabase) -> GlobalLedgerResult<bool> {
    let connection = database.connection(LedgerLocation::InspectRuntimeLedger.location())?;
    match (format_version(&connection)?, table_count(&connection)?) {
        (UNSUPPORTED_FORMAT_VERSION, tables) if tables != 0 => Err(unsupported_format()),
        (0, 0) => Ok(false),
        (0 | FORMAL_FORMAT_VERSION, 4) => Ok(true),
        _ => Err(failure(
            LedgerCode::LedgerSchemaIncomplete.code(),
            LedgerLocation::InspectRuntimeLedger.location(),
        )),
    }
}

/// Authenticates the keyed meta row alone (format header, marker and head extent) with
/// the error identities of complete verification. No history row is read.
fn authenticated_extent(
    database: &RuntimeDatabase,
    connection: &Connection,
) -> GlobalLedgerResult<Option<(SqliteMarker, u64)>> {
    let format = format_version(connection)?;
    match (format, table_count(connection)?) {
        (UNSUPPORTED_FORMAT_VERSION, tables) if tables != 0 => return Err(unsupported_format()),
        (0, 0) => return Ok(None),
        (0 | FORMAL_FORMAT_VERSION, 4) => {}
        _ => {
            return Err(failure(
                LedgerCode::LedgerSchemaIncomplete.code(),
                LedgerLocation::InspectRuntimeLedger.location(),
            ));
        }
    }
    let meta = read_meta(connection)?;
    let marker = SqliteMarker::parse(&meta)?;
    let expected_format = if marker.state == "ready" {
        FORMAL_FORMAT_VERSION
    } else {
        0
    };
    if format != expected_format {
        return Err(failure(
            LedgerCode::LedgerFormatMarkerMismatch.code(),
            LedgerLocation::VerifyDatabaseFormat.location(),
        ));
    }
    let (Some(SqlValue::Integer(next)), Some(SqlValue::Integer(head)), Some(hash)) =
        (meta.get(2), meta.get(3), meta.get(4))
    else {
        return Err(failure(
            LedgerCode::LedgerMetaMismatch.code(),
            LedgerLocation::VerifySqliteHead.location(),
        ));
    };
    let head = decode(*head);
    let hash = match hash {
        SqlValue::Null if head == 0 => None,
        SqlValue::Text(hash) if head != 0 => Some(hash.as_str()),
        _ => {
            return Err(failure(
                LedgerCode::LedgerMetaMismatch.code(),
                LedgerLocation::VerifySqliteHead.location(),
            ));
        }
    };
    if decode(*next) != increment_sequence(head)?
        || meta != meta_row_with_marker(database, decode(*next), head, hash, &marker)
    {
        return Err(failure(
            LedgerCode::LedgerMetaMismatch.code(),
            LedgerLocation::VerifySqliteHead.location(),
        ));
    }
    Ok(Some((marker, head)))
}

/// Classifies the formal medium from its keyed meta row without reading history.
/// True does not mean the history is verified; the writer open is that verification.
pub(super) fn formal_ready(database: &RuntimeDatabase) -> GlobalLedgerResult<bool> {
    let mut connection = database.connection(LedgerLocation::InspectRuntimeLedger.location())?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|error| sql_error(error, LedgerLocation::BeginLedgerClassification.location()))?;
    let extent = authenticated_extent(database, &transaction)?;
    transaction
        .commit()
        .map_err(|error| sql_error(error, LedgerLocation::CloseLedgerClassification.location()))?;
    Ok(extent.is_some_and(|(marker, _)| marker.state == "ready"))
}

/// Workflow #375 R5a: the status of a formal or candidate medium from one complete
/// authenticated read that reads no artifact material.
pub(super) fn storage_status(
    database: &RuntimeDatabase,
    budget: ReadBudget,
) -> GlobalLedgerResult<super::LedgerStorageStatus> {
    if !has_schema(database)? {
        return Ok(super::LedgerStorageStatus::Missing);
    }
    let raw = read_snapshot(database, budget)?;
    let marker = SqliteMarker::parse(&raw.meta)?;
    let (events, hash) = verify_snapshot_unread(database, raw, &mut LedgerOpenTiming::default())?;
    Ok(marker.status(&events, hash))
}

pub(super) fn initialize_formal_empty(database: &RuntimeDatabase) -> GlobalLedgerResult<()> {
    let mut connection = database.connection(LedgerLocation::InitializeRuntimeLedger.location())?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sql_error(error, LedgerLocation::BeginRuntimeLedger.location()))?;
    let result = (|| {
        refuse_unsupported_format(&transaction)?;
        if table_count(&transaction)? != 0 || format_version(&transaction)? != 0 {
            return Err(failure(
                LedgerCode::LedgerInitializationConflict.code(),
                LedgerLocation::InitializeRuntimeLedger.location(),
            ));
        }
        transaction
            .execute_batch(include_str!("sqlite/schema.sql"))
            .map_err(|error| sql_error(error, LedgerLocation::CreateRuntimeLedger.location()))?;
        views::deploy(&transaction)?;
        mark_formal_format(&transaction)?;
        insert_row(
            &transaction,
            "ledger_meta",
            META_COLUMNS,
            &meta_row_with_marker(database, 1, 0, None, &SqliteMarker::empty()),
        )
    })();
    match result {
        Ok(()) => transaction
            .commit()
            .map_err(|error| sql_error(error, LedgerLocation::CommitRuntimeLedger.location())),
        Err(error) => Err(error.with_close_result(
            transaction.rollback().map_err(|error| {
                sql_error(error, LedgerLocation::RollbackRuntimeLedger.location())
            }),
        )),
    }
}

/// Workflow #375 R5a: the writer open reads no artifact material; `timing` keeps the phases
/// that ran, also when an error is returned.
pub(super) fn open_formal(
    config: GlobalLedgerConfig,
    database: Arc<RuntimeDatabase>,
    lock: super::storage::LockedWriterFile,
    compatibility: Option<super::storage::LockedWriterFile>,
    deadline: Instant,
    timing: &mut LedgerOpenTiming,
) -> GlobalLedgerResult<SqliteLedgerStore> {
    let started = Instant::now();
    // P0-LO: a test gate may make this open slower by a delay it does not wait out.
    #[cfg(any(test, feature = "test-hooks"))]
    let (started, deadline) = crate::open_delay::slowed(database.root(), started, deadline);
    let raw = read_formal_snapshot(&database, deadline);
    timing.sql_read_ms = elapsed_ms(started);
    let raw = raw?;
    timing.events = raw.events.len() as u64;
    let marker = SqliteMarker::parse(&raw.meta)?;
    if marker.state != "ready" {
        return Err(failure(
            SharedCode::LedgerMigrationRequired.code(),
            LedgerLocation::OpenRuntimeLedger.location(),
        ));
    }
    let (events, head_hash) = verify_snapshot_unread(&database, raw, timing)?;
    let head = events.last().map_or(0, PersistedEvent::sequence);
    upgrade_views(&database, head, head_hash.as_deref())?;
    ensure_history_index(&database)?;
    let meta_has_marker = {
        let connection =
            database.connection(LedgerLocation::InspectLedgerMarkerSchema.location())?;
        ledger_meta_has_marker(&connection)?
    };
    let next = increment_sequence(head)?;
    let (ownership, stale) = WriterOwnership::from_locked(lock, compatibility, &config.owner_id)?;
    let backend = SqliteStorage {
        database,
        ownership,
        head,
        head_hash,
        marker,
        meta_has_marker,
    };
    // LN-5 (#422, LEAN-L §5 LN-5): the open still reads every record (until LN-8), derives the
    // retention state from them and drops them; the writer reads its history from the store.
    let mut store = EventStore::recovered(backend, next, &events)?;
    drop(events);
    // Workflow #383 AO-1 (`contracts/ledger-observer.md` §9.4, §12 step 2): a dump an earlier run
    // left is imported after the ledger is verified and before any new record; AO-2 (§10, §12
    // step 5): then the previous epoch's unconfirmed tail.
    if let Err(error) = store.open_running(&config, stale.is_some()) {
        return Err(error.with_close_result(store.backend.close()));
    }
    if let Some(owner) = stale
        && let Err(error) =
            store.append_recovery(RecoveryReason::StaleOwner, Some(owner), None, None)
    {
        return Err(error.with_close_result(store.backend.close()));
    }
    Ok(store)
}

pub(super) fn import_source(
    database: &RuntimeDatabase,
    source: &super::FrozenLedgerSource,
    record: &actingcommand_contract::LedgerMigrationRecord,
    completion: actingcommand_contract::SanitizedEventDraft,
    dry_run: bool,
    budget: ReadBudget,
) -> GlobalLedgerResult<super::LedgerStorageStatus> {
    // Workflow #375 R5a: neither the source nor the imported medium reads artifact material.
    match storage_status(database, budget)? {
        super::LedgerStorageStatus::Ready {
            migration: Some(existing),
            ..
        } if existing.as_ref() == record => return storage_status(database, budget),
        super::LedgerStorageStatus::Missing => {}
        _ => {
            return Err(failure(
                LedgerCode::LedgerMigrationConflict.code(),
                ContractLocation::ImportSegmentLedger.location(),
            ));
        }
    }
    let completion =
        PersistedEvent::from_sanitized(record.cutover_sequence, completion).map_err(|error| {
            failure(
                error.code(),
                LedgerLocation::ValidateMigrationCompletion.location(),
            )
        })?;
    let marker = SqliteMarker::migrated(record)?;
    let mut expected = source.events.clone();
    expected.push(completion);
    marker.verify_events(&expected)?;
    let mut connection = database.connection(ContractLocation::ImportSegmentLedger.location())?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sql_error(error, LedgerLocation::BeginSegmentImport.location()))?;
    let prepared = (|| {
        refuse_unsupported_format(&transaction)?;
        if table_count(&transaction)? != 0 || format_version(&transaction)? != 0 {
            return Err(failure(
                LedgerCode::LedgerMigrationConflict.code(),
                ContractLocation::ImportSegmentLedger.location(),
            ));
        }
        transaction
            .execute_batch(include_str!("sqlite/schema.sql"))
            .map_err(|error| sql_error(error, LedgerLocation::CreateImportSchema.location()))?;
        views::deploy(&transaction)?;
        mark_formal_format(&transaction)?;
        let mut head_hash = None;
        for (index, event) in expected.iter().enumerate() {
            check_read_budget(budget, 0, index + 1)?;
            if event.sequence() != (index as u64) + 1 {
                return Err(failure(
                    LedgerCode::SequenceDiscontinuity.code(),
                    ContractLocation::ImportSegmentLedger.location(),
                ));
            }
            let projected = project_record(database, event, head_hash.as_deref())?;
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
            head_hash = Some(projected.hash);
        }
        insert_row(
            &transaction,
            "ledger_meta",
            META_COLUMNS,
            &meta_row_with_marker(
                database,
                increment_sequence(record.cutover_sequence)?,
                record.cutover_sequence,
                head_hash.as_deref(),
                &marker,
            ),
        )?;
        let raw = read_snapshot_connection(&transaction, budget)?;
        let (actual, hash) =
            verify_snapshot_unread(database, raw, &mut LedgerOpenTiming::default())?;
        if actual != expected {
            return Err(failure(
                LedgerCode::MigrationCanonicalMismatch.code(),
                LedgerLocation::VerifyImportTransaction.location(),
            ));
        }
        let expected_index = EventIndexes::from_events(&expected);
        let actual_index = EventIndexes::from_events(&actual);
        if actual_index.query(&actual, &EventQuery::default())
            != expected_index.query(&expected, &EventQuery::default())
        {
            return Err(failure(
                LedgerCode::MigrationProjectionMismatch.code(),
                LedgerLocation::VerifyImportTransaction.location(),
            ));
        }
        Ok(marker.status(&actual, hash))
    })();
    let status = match prepared {
        Ok(status) => status,
        Err(error) => {
            return Err(
                error.with_close_result(transaction.rollback().map_err(|error| {
                    sql_error(error, LedgerLocation::RollbackSegmentImport.location())
                })),
            );
        }
    };
    if dry_run {
        transaction
            .rollback()
            .map_err(|error| sql_error(error, LedgerLocation::RollbackImportDryRun.location()))?;
        return Ok(status);
    }
    match transaction.commit() {
        Ok(()) => Ok(status),
        Err(error) => {
            let mut original = sql_error(error, LedgerLocation::CommitSegmentImport.location());
            drop(connection);
            let observed = storage_status(database, budget);
            original.detail = Some(format!(
                "{}; commit_readback={}",
                original.detail.as_deref().unwrap_or("sqlite_error"),
                match observed {
                    Ok(super::LedgerStorageStatus::Ready {
                        migration: Some(observed),
                        ..
                    }) if observed.as_ref() == record => "matching_committed_marker".to_owned(),
                    Ok(super::LedgerStorageStatus::Missing) => "not_committed".to_owned(),
                    Ok(_) => "conflicting_material".to_owned(),
                    Err(error) => format!(
                        "unavailable: {error}; {}",
                        error.detail().unwrap_or("no_additional_detail")
                    ),
                }
            ));
            Err(original)
        }
    }
}

pub(super) type SqliteLedgerStore = EventStore<SqliteStorage>;

pub(super) struct SqliteStorage {
    database: Arc<RuntimeDatabase>,
    ownership: WriterOwnership,
    head: u64,
    head_hash: Option<String>,
    marker: SqliteMarker,
    /// Whether `ledger_meta` has `migration_record`, inspected once at open: the column
    /// set cannot change while this writer holds the writer lock.
    meta_has_marker: bool,
}

impl From<RuntimeDatabaseError> for GlobalLedgerError {
    fn from(error: RuntimeDatabaseError) -> Self {
        let mut converted = Self::fatal(error.code(), error.operation());
        converted.detail = error.detail().map(str::to_owned);
        converted
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
        let format = {
            let connection =
                database.connection(LedgerLocation::InspectCandidateFormat.location())?;
            refuse_unsupported_format(&connection)?;
            format_version(&connection)?
        };
        if format == FORMAL_FORMAT_VERSION {
            return Err(failure(
                LedgerCode::CandidateProductionConflict.code(),
                LedgerLocation::OpenSqliteCandidate.location(),
            ));
        }
        // Same physical database, same existing OS writer lock, including separate Arcs.
        let (mut ownership, stale_owner) =
            WriterOwnership::acquire(database.root(), &config.owner_id)?;
        let recovered = (|| {
            initialize(&database, ownership.is_new())?;
            let raw = read_snapshot(&database, None)?;
            if SqliteMarker::parse(&raw.meta)?.state != "candidate" {
                return Err(failure(
                    LedgerCode::CandidateProductionConflict.code(),
                    LedgerLocation::OpenSqliteCandidate.location(),
                ));
            }
            let (events, head_hash) = verify_snapshot(&database, raw, &mut verifier)?;
            upgrade_views(
                &database,
                events.last().map_or(0, PersistedEvent::sequence),
                head_hash.as_deref(),
            )?;
            ensure_history_index(&database)?;
            let meta_has_marker = {
                let connection =
                    database.connection(LedgerLocation::InspectLedgerMarkerSchema.location())?;
                ledger_meta_has_marker(&connection)?
            };
            Ok((events, head_hash, meta_has_marker))
        })();
        let (events, head_hash, meta_has_marker) = match recovered {
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
            marker: SqliteMarker::candidate(),
            meta_has_marker,
        };
        // LN-5 (#422): the records are dropped once the retention state is derived from them.
        let mut store = Self::recovered(backend, next, &events)?;
        drop(events);
        // Workflow #383 AO-1: a candidate opened with a failure plan writes and imports as the
        // running Runtime's writer does (`contracts/ledger-observer.md` §9); AO-2: and records
        // the previous epoch's unconfirmed tail (§10).
        if let Err(error) = store.open_running(&config, stale_owner.is_some()) {
            return Err(error.with_close_result(store.backend.close()));
        }
        if let Some(previous_owner) = stale_owner
            && let Err(error) =
                store.append_recovery(RecoveryReason::StaleOwner, Some(previous_owner), None, None)
        {
            return Err(error.with_close_result(store.backend.close()));
        }
        Ok(store)
    }
}

impl SqliteStorage {
    /// Workflow #383 AO-2 (`contracts/ledger-observer.md` §8.1, §11.4): one transaction commits
    /// `events` (consecutive, from the head's next sequence), their index rows and, for a joint
    /// record, its state work, under `sync`. The head moves only once the commit returned.
    fn persist_batch_rows(
        &mut self,
        events: &[PersistedEvent],
        rows: &IndexRows,
        work: Option<&dyn super::LedgerTransactionWork>,
        sync: CommitSync,
    ) -> GlobalLedgerResult<Option<u64>> {
        let (Some(first), Some(last)) = (events.first(), events.last()) else {
            return Ok(None);
        };
        if work.is_some() && events.len() != 1 {
            return Err(GlobalLedgerError::request(
                LedgerCode::LedgerJointTransactionUnsupported.code(),
                LedgerLocation::AppendTransaction.location(),
            ));
        }
        let mut expected = increment_sequence(self.head)?;
        for event in events {
            if event.sequence() != expected {
                return Err(failure(
                    LedgerCode::SequenceDiscontinuity.code(),
                    LedgerLocation::AppendSqliteEvent.location(),
                ));
            }
            expected = increment_sequence(expected)?;
        }
        let next = increment_sequence(last.sequence())?;
        let projected = project_batch(&self.database, events, self.head_hash.as_deref())?;
        let last_hash = projected
            .last()
            .map(|record| record.hash.clone())
            .ok_or_else(|| {
                failure(
                    LedgerCode::SequenceDiscontinuity.code(),
                    LedgerLocation::AppendSqliteEvent.location(),
                )
            })?;
        let started = Instant::now();
        let mut connection = self
            .database
            .connection(LedgerLocation::AppendSqliteEvent.location())?;
        // An observational batch commits under NORMAL for this one transaction; FULL is
        // restored and read back on every exit before the guard is released.
        if sync == CommitSync::Normal
            && let Err(error) = self
                .database
                .set_commit_sync(&connection, CommitSync::Normal)
        {
            let restored = self.database.set_commit_sync(&connection, CommitSync::Full);
            let mut failed = GlobalLedgerError::from(error);
            append_detail(
                &mut failed,
                &match restored {
                    Ok(()) => "sync_restore=ok".to_owned(),
                    Err(restore) => sync_restore_detail(&restore),
                },
            );
            return Err(failed);
        }
        let previous_meta = meta_row_with_marker(
            &self.database,
            first.sequence(),
            self.head,
            self.head_hash.as_deref(),
            &self.marker,
        );
        let meta = meta_row_with_marker(
            &self.database,
            next,
            last.sequence(),
            Some(&last_hash),
            &self.marker,
        );
        let attempt = self.commit_batch(
            &mut connection,
            &BatchRows {
                events,
                projected: &projected,
                rows,
                meta: &meta,
            },
            work,
        );
        let restored = match sync {
            CommitSync::Full => Ok(()),
            CommitSync::Normal => self.database.set_commit_sync(&connection, CommitSync::Full),
        };
        let commit_failure = match attempt {
            JointCommit::Committed => {
                if let Err(restore) = restored {
                    // The batch is committed but the connection stays refused; the head
                    // does not advance and the failure does not establish absence.
                    let mut failed = GlobalLedgerError::from(restore);
                    failed.detail = Some(match failed.detail.take() {
                        Some(detail) => {
                            format!("committed sequence={}; {detail}", last.sequence())
                        }
                        None => format!("committed sequence={}", last.sequence()),
                    });
                    return Err(failed);
                }
                None
            }
            JointCommit::RolledBack(original) => {
                return Err(with_sync_restore(original, restored));
            }
            JointCommit::CommitFailed(original) => Some(with_sync_restore(original, restored)),
        };
        if let Some(mut original) = commit_failure {
            // The consumed transaction/guard must be released before any readback.
            drop(connection);
            if let (Some(work), Some(record)) = (work, projected.first()) {
                let observed: GlobalLedgerResult<&str> = (|| {
                    let mut connection = self
                        .database
                        .try_connection(LedgerLocation::ReadJointCommit.location())?;
                    let snapshot = connection
                        .transaction_with_behavior(TransactionBehavior::Deferred)
                        .map_err(|error| {
                            sql_error(error, LedgerLocation::ReadJointCommit.location())
                        })?;
                    let budget = Some((
                        2 * 1024 * 1024,
                        MAX_QUERY_PAGE_EVENTS,
                        Instant::now() + std::time::Duration::from_secs(2),
                    ));
                    let mut bytes = 0;
                    let sequence = encode(first.sequence());
                    let event_rows = read_rows(
                        &snapshot,
                        &format!(
                            "SELECT {EVENT_COLUMNS} FROM ledger_events WHERE sequence={sequence}"
                        ),
                        budget,
                        &mut bytes,
                        true,
                    )?;
                    let links = read_rows(
                        &snapshot,
                        &format!(
                            "SELECT {LINK_COLUMNS} FROM ledger_links WHERE sequence={sequence}"
                        ),
                        budget,
                        &mut bytes,
                        true,
                    )?;
                    let artifacts = read_rows(
                        &snapshot,
                        &format!(
                            "SELECT {ARTIFACT_COLUMNS} FROM ledger_artifacts WHERE sequence={sequence} ORDER BY ordinal"
                        ),
                        budget,
                        &mut bytes,
                        true,
                    )?;
                    let observed_meta = read_meta_with_budget(&snapshot, budget, &mut bytes)?;
                    let state = work
                        .observe(&self.database.borrow_transaction(&snapshot))
                        .map_err(|error| {
                            let mut failed =
                                GlobalLedgerError::relayed_fatal(error.code, error.operation);
                            failed.detail = Some(error.detail);
                            failed
                        })?;
                    check_read_budget(budget, bytes, 0)?;
                    let observed = if event_rows == vec![record.event.clone()]
                        && links == vec![record.links.clone()]
                        && artifacts == record.artifacts
                        && observed_meta == meta
                        && state == super::TransactionStateObservation::Applied
                    {
                        "committed"
                    } else if event_rows.is_empty()
                        && links.is_empty()
                        && artifacts.is_empty()
                        && observed_meta == previous_meta
                        && state == super::TransactionStateObservation::Unchanged
                    {
                        "not_committed"
                    } else {
                        "unknown"
                    };
                    snapshot.rollback().map_err(|error| {
                        sql_error(error, LedgerLocation::CloseJointReadback.location())
                    })?;
                    Ok(observed)
                })();
                original.detail = Some(format!(
                    "{}; joint_commit_readback={observed:?}",
                    original.detail.as_deref().unwrap_or("sqlite_error")
                ));
            }
            return Err(original);
        }
        #[cfg(test)]
        super::storage::repair_test_barrier("after_sqlite_commit")?;
        let elapsed = Instant::now()
            .checked_duration_since(started)
            .and_then(|value| u64::try_from(value.as_nanos()).ok());
        self.head = last.sequence();
        self.head_hash = Some(last_hash);
        // Workflow #383 AO-2: a test gate may observe each committed batch and its sync level.
        #[cfg(any(test, feature = "test-hooks"))]
        crate::write_fault::committed_batch(
            self.database.root(),
            events.iter().map(PersistedEvent::sequence).collect(),
            sync == CommitSync::Full,
        );
        // The common store updates indexes/statistics only after this durable return.
        Ok(elapsed)
    }

    /// Begin through commit of one batch transaction on the caller's guard; the caller restores
    /// the commit synchronization afterwards, whatever the outcome.
    fn commit_batch(
        &self,
        connection: &mut Connection,
        batch: &BatchRows<'_>,
        work: Option<&dyn super::LedgerTransactionWork>,
    ) -> JointCommit {
        let transaction = match connection.transaction_with_behavior(TransactionBehavior::Immediate)
        {
            Ok(transaction) => transaction,
            Err(error) => {
                return JointCommit::RolledBack(sql_error(
                    error,
                    LedgerLocation::BeginSqliteAppend.location(),
                ));
            }
        };
        let prepared: GlobalLedgerResult<()> = (|| {
            let current = read_meta_columns(&transaction, self.meta_has_marker, None, &mut 0)?;
            let first = batch.events.first().map_or(0, PersistedEvent::sequence);
            if current
                != meta_row_with_marker(
                    &self.database,
                    first,
                    self.head,
                    self.head_hash.as_deref(),
                    &self.marker,
                )
            {
                return Err(failure(
                    LedgerCode::LedgerMetaMismatch.code(),
                    LedgerLocation::AppendSqliteEvent.location(),
                ));
            }
            for projected in batch.projected {
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
            }
            let changed = transaction.execute(
            "UPDATE ledger_meta SET schema_version=?2,next_sequence=?3,head_sequence=?4,head_record_sha256=?5,storage_backend=?6,migration_id=?7,cutover_state=?8,integer_encoding=?9,integrity_tag=?10,migration_record=?11 WHERE singleton=?1",
            params_from_iter(batch.meta.iter()),
        ).map_err(|error| sql_error(error, LedgerLocation::UpdateSqliteHead.location()))?;
            if changed != 1 {
                return Err(failure(
                    LedgerCode::LedgerMetaMismatch.code(),
                    LedgerLocation::UpdateSqliteHead.location(),
                ));
            }
            write_index_rows(&transaction, batch)?;
            if let (Some(work), Some(event)) = (work, batch.events.first()) {
                work.apply(&self.database.borrow_transaction(&transaction), event)
                    .map_err(GlobalLedgerError::work_failure)?;
            }
            Ok(())
        })();
        if let Err(mut original) = prepared {
            if let Err(rollback) = transaction.rollback() {
                let rollback = sql_error(rollback, LedgerLocation::RollbackJointEvent.location());
                original.detail = Some(format!(
                    "original={}; detail={:?}; rollback={}; detail={:?}",
                    original, original.detail, rollback, rollback.detail
                ));
                original.terminal = true;
                original.rolled_back_work = None;
            }
            return JointCommit::RolledBack(original);
        }
        // Workflow #381 A (LED-I3): a test gate may hold the writer here, inside the write
        // transaction, as a slow commit would.
        #[cfg(any(test, feature = "test-hooks"))]
        crate::writer_stall::before_durable_commit(self.database.root());
        // Workflow #383 AO-1: a test gate may fail this attempt here (a full disk, a lost commit
        // result, an integrity failure).
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(fault) = crate::write_fault::before_commit(self.database.root()) {
            return injected_fault(fault, transaction);
        }
        match transaction.commit() {
            Ok(()) => JointCommit::Committed,
            Err(error) => JointCommit::CommitFailed(sql_error(
                error,
                LedgerLocation::CommitSqliteEvent.location(),
            )),
        }
    }
}

/// One batch's rows: its records, their projections, the index rows they change and the head
/// they leave.
struct BatchRows<'a> {
    events: &'a [PersistedEvent],
    projected: &'a [ProjectedRecord],
    rows: &'a IndexRows,
    meta: &'a SqlRow,
}

/// The projections of a batch, each record chained to the one before it.
fn project_batch(
    database: &RuntimeDatabase,
    events: &[PersistedEvent],
    head_hash: Option<&str>,
) -> GlobalLedgerResult<Vec<ProjectedRecord>> {
    let mut previous = head_hash.map(str::to_owned);
    let mut projected = Vec::with_capacity(events.len());
    for event in events {
        let record = project_record(database, event, previous.as_deref())?;
        previous = Some(record.hash.clone());
        projected.push(record);
    }
    Ok(projected)
}

// The positions of the `ledger_links` columns (LINK_COLUMNS) the index reads.
const LINK_REQUEST_ID: usize = 2;
const LINK_ACTION_ID: usize = 9;
// The positions of the `ledger_events` columns (EVENT_COLUMNS) the index reads.
const EVENT_SEQUENCE: usize = 0;
const EVENT_ID: usize = 1;

/// Workflow #383 AO-2 (`contracts/ledger-observer.md` §11.1, §11.4): the batch's index rows, in
/// its transaction. A `state`, `open`, `request` or `lifecycle` row is overwritten by its key's
/// latest record; an intent's result deletes the `open` rows of the intents with its action id;
/// the `request` rows keep the latest `request_index_records`.
fn write_index_rows(connection: &Connection, batch: &BatchRows<'_>) -> GlobalLedgerResult<()> {
    let mut requests = false;
    for change in batch.rows.changes() {
        match change {
            IndexChange::State { key, at } => {
                upsert_index_row(connection, batch, "state", SqlValue::Text(key.clone()), *at)?;
            }
            IndexChange::Open { at } => {
                let key = column(&batch.projected[*at].event, EVENT_ID)?;
                upsert_index_row(connection, batch, "open", key, *at)?;
            }
            IndexChange::Close { at } => {
                let action = column(&batch.projected[*at].links, LINK_ACTION_ID)?;
                connection
                    .execute(
                        "DELETE FROM ledger_index WHERE role='open' AND sequence IN (SELECT sequence FROM ledger_links WHERE action_id=?1)",
                        params_from_iter([action]),
                    )
                    .map_err(|error| {
                        sql_error(error, LedgerLocation::LedgerIndexRows.location())
                    })?;
            }
            IndexChange::Request { at } => {
                let key = column(&batch.projected[*at].links, LINK_REQUEST_ID)?;
                upsert_index_row(connection, batch, "request", key, *at)?;
                requests = true;
            }
            IndexChange::Lifecycle { slot, at } => {
                upsert_index_row(
                    connection,
                    batch,
                    "lifecycle",
                    SqlValue::Text(slot.index_key().to_owned()),
                    *at,
                )?;
            }
        }
    }
    if requests {
        let skip = i64::try_from(batch.rows.request_ring().saturating_sub(1)).unwrap_or(i64::MAX);
        connection
            .execute(
                "DELETE FROM ledger_index WHERE role='request' AND sequence < (SELECT sequence FROM ledger_index WHERE role='request' ORDER BY sequence DESC LIMIT 1 OFFSET ?1)",
                params_from_iter([skip]),
            )
            .map_err(|error| sql_error(error, LedgerLocation::LedgerIndexRows.location()))?;
    }
    Ok(())
}

fn upsert_index_row(
    connection: &Connection,
    batch: &BatchRows<'_>,
    role: &'static str,
    key: SqlValue,
    at: usize,
) -> GlobalLedgerResult<()> {
    let projected = &batch.projected[at];
    let source = batch.events[at]
        .capture()
        .and_then(|capture| capture.source);
    let lineage = source
        .and_then(|source| super::index::id_text(&source.lineage))
        .map_or(SqlValue::Null, SqlValue::Text);
    let revision = source.map_or(SqlValue::Null, |source| {
        SqlValue::Integer(encode(source.revision))
    });
    let changed = connection
        .execute(
            "INSERT INTO ledger_index (role,key,sequence,event_id,value_sha256,source_lineage,source_revision) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(role,key) DO UPDATE SET sequence=excluded.sequence,event_id=excluded.event_id,value_sha256=excluded.value_sha256,source_lineage=excluded.source_lineage,source_revision=excluded.source_revision",
            params_from_iter([
                SqlValue::Text(role.to_owned()),
                key,
                column(&projected.event, EVENT_SEQUENCE)?,
                column(&projected.event, EVENT_ID)?,
                SqlValue::Text(projected.hash.clone()),
                lineage,
                revision,
            ]),
        )
        .map_err(|error| sql_error(error, LedgerLocation::LedgerIndexRows.location()))?;
    if changed != 1 {
        return Err(failure(
            LedgerCode::LedgerInsertIncomplete.code(),
            LedgerLocation::LedgerIndexRows.location(),
        ));
    }
    Ok(())
}

/// Workflow #422 LN-4 (`contracts/ledger-observer.md` §11.5 step 1): the index rows of `role`
/// whose key starts with one of `prefixes`, each with the record hash of the ledger row its
/// sequence names (none when no ledger row has that sequence).
fn read_index_rows(
    database: &RuntimeDatabase,
    role: super::index_read::LedgerIndexRole,
    prefixes: &[String],
) -> GlobalLedgerResult<Vec<super::index_read::StoredIndexRow>> {
    let operation = LedgerLocation::LedgerIndexRows.location();
    let connection = database.connection(operation)?;
    let mut statement = connection
        .prepare(
            "SELECT i.sequence,i.key,i.event_id,i.value_sha256,e.record_sha256 FROM ledger_index i LEFT JOIN ledger_events e ON e.sequence=i.sequence WHERE i.role=?1 AND substr(i.key,1,length(?2))=?2 ORDER BY i.sequence,i.key",
        )
        .map_err(|error| sql_error(error, operation))?;
    let mut rows = Vec::new();
    for prefix in prefixes {
        let selected = statement
            .query_map(
                params_from_iter([
                    SqlValue::Text(role.column().to_owned()),
                    SqlValue::Text(prefix.clone()),
                ]),
                |row| {
                    Ok(super::index_read::StoredIndexRow {
                        sequence: decode(row.get::<_, i64>(0)?),
                        key: row.get::<_, String>(1)?,
                        event_id: row.get::<_, String>(2)?,
                        value_sha256: row.get::<_, String>(3)?,
                        record_sha256: row.get::<_, Option<String>>(4)?,
                    })
                },
            )
            .map_err(|error| sql_error(error, operation))?;
        for row in selected {
            rows.push(row.map_err(|error| sql_error(error, operation))?);
        }
    }
    Ok(rows)
}

fn column(row: &SqlRow, position: usize) -> GlobalLedgerResult<SqlValue> {
    row.get(position).cloned().ok_or_else(|| {
        failure(
            LedgerCode::InvalidEventColumn.code(),
            LedgerLocation::LedgerIndexRows.location(),
        )
    })
}

/// Workflow #383 AO-1: the outcome a test gate's fault gives the attempt in progress
/// (Workflow #382 L-3, LED-I1: also a COMMIT failure whose transaction did not take effect).
#[cfg(any(test, feature = "test-hooks"))]
fn injected_fault(
    fault: crate::write_fault::InjectedFault,
    transaction: rusqlite::Transaction<'_>,
) -> JointCommit {
    use crate::write_fault::InjectedFault;
    let full_disk = || {
        sql_error(
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
                None,
            ),
            LedgerLocation::CommitSqliteEvent.location(),
        )
    };
    match fault {
        InjectedFault::FailBeforeCommit => match transaction.rollback() {
            Ok(()) => JointCommit::RolledBack(full_disk()),
            Err(error) => JointCommit::RolledBack(sql_error(
                error,
                LedgerLocation::RollbackJointEvent.location(),
            )),
        },
        InjectedFault::CommitThenFail => match transaction.commit() {
            Ok(()) => JointCommit::CommitFailed(full_disk()),
            Err(error) => JointCommit::CommitFailed(sql_error(
                error,
                LedgerLocation::CommitSqliteEvent.location(),
            )),
        },
        // Workflow #382 L-3 (LED-I1): the COMMIT reports the failure and the transaction did not
        // take effect; the outcome stays uncertain to the writer, as a real COMMIT failure is.
        InjectedFault::CommitFailedUncommitted => match transaction.rollback() {
            Ok(()) => JointCommit::CommitFailed(full_disk()),
            Err(error) => JointCommit::CommitFailed(sql_error(
                error,
                LedgerLocation::RollbackJointEvent.location(),
            )),
        },
        InjectedFault::Integrity => {
            let refused = failure(
                LedgerCode::LedgerMetaMismatch.code(),
                LedgerLocation::AppendSqliteEvent.location(),
            );
            match transaction.rollback() {
                Ok(()) => JointCommit::RolledBack(refused),
                Err(error) => JointCommit::RolledBack(sql_error(
                    error,
                    LedgerLocation::RollbackJointEvent.location(),
                )),
            }
        }
    }
}

/// One event transaction's outcome, before the connection's FULL level is restored.
enum JointCommit {
    Committed,
    /// Begin failed, or the transaction was rolled back (a failed rollback is recorded
    /// in the error).
    RolledBack(GlobalLedgerError),
    CommitFailed(GlobalLedgerError),
}

/// Records a failed FULL restore on an append error, which then stays fatal; the
/// connection is refused to every later user.
fn with_sync_restore(
    mut error: GlobalLedgerError,
    restored: Result<(), RuntimeDatabaseError>,
) -> GlobalLedgerError {
    if let Err(restore) = restored {
        append_detail(&mut error, &sync_restore_detail(&restore));
        error.terminal = true;
    }
    error
}

fn sync_restore_detail(restore: &RuntimeDatabaseError) -> String {
    match restore.detail() {
        Some(detail) => format!("sync_restore={}({detail})", restore.code()),
        None => format!("sync_restore={}", restore.code()),
    }
}

fn append_detail(error: &mut GlobalLedgerError, suffix: &str) {
    error.detail = Some(match error.detail.take() {
        Some(detail) => format!("{detail}; {suffix}"),
        None => suffix.to_owned(),
    });
}

impl DurableStorage for SqliteStorage {
    fn material_root(&self) -> &std::path::Path {
        self.database.root()
    }

    fn index_rows(
        &self,
        role: super::index_read::LedgerIndexRole,
        prefixes: &[String],
    ) -> Option<GlobalLedgerResult<Vec<super::index_read::StoredIndexRow>>> {
        Some(read_index_rows(&self.database, role, prefixes))
    }

    fn head(&self) -> u64 {
        self.head
    }

    fn resolve_artifact(
        &self,
        selection: &super::LedgerArtifactSelection,
        deadline: Instant,
        retention: &RetentionIndex,
    ) -> GlobalLedgerResult<super::ResolvedLedgerArtifact> {
        self.resolve_artifact_on_disk(selection, deadline, retention)
    }

    fn project_view_page(
        &self,
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
        observation: &mut Option<LedgerProjectViewObservation>,
        read: (&ReadWork, usize),
        retention: &RetentionIndex,
    ) -> GlobalLedgerResult<RuntimeEventQueryPage> {
        self.view_page_on_disk(query, profile, request, observation, read, retention)
    }

    fn state_index_rows(
        &self,
        prefixes: &[String],
    ) -> Option<GlobalLedgerResult<Vec<super::tagged::StateIndexRow>>> {
        Some(tagged::state_index_rows(&self.database, prefixes))
    }

    fn history_page(
        &self,
        selection: &HistorySelection<'_>,
        (after, through): (u64, u64),
        rows: usize,
        retention: &RetentionIndex,
    ) -> GlobalLedgerResult<Vec<PersistedEvent>> {
        let through = through.min(self.head);
        if after >= through || rows == 0 {
            return Ok(Vec::new());
        }
        let mut connection = self
            .database
            .connection(LedgerLocation::ReadSqliteSnapshot.location())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|error| sql_error(error, LedgerLocation::BeginSqliteSnapshot.location()))?;
        let metadata = match read_history_rows(
            &self.database,
            &transaction,
            (selection, None),
            (after, through),
            rows,
        ) {
            Ok(metadata) => metadata,
            Err(error) => {
                return Err(
                    error.with_close_result(transaction.rollback().map_err(|error| {
                        sql_error(error, LedgerLocation::CloseSqliteSnapshot.location())
                    })),
                );
            }
        };
        transaction
            .commit()
            .map_err(|error| sql_error(error, LedgerLocation::CloseSqliteSnapshot.location()))?;
        drop(connection);
        metadata
            .into_iter()
            .map(|event| {
                event
                    .into_event_with_artifact_availability(&mut |reference| {
                        retention.proven_availability(reference)
                    })
                    .map_err(|error| {
                        failure(
                            error.code(),
                            LedgerLocation::ValidatePersistedEvent.location(),
                        )
                    })
            })
            .collect()
    }

    fn committed_sequence(&self, event_id: &EventId) -> GlobalLedgerResult<Option<u64>> {
        let connection = self
            .database
            .connection(LedgerLocation::ReadSqliteSnapshot.location())?;
        let rows = read_rows_bound(
            &connection,
            "SELECT sequence FROM ledger_events WHERE event_id=?",
            &[SqlValue::Text(views::key(event_id)?)],
            None,
            &mut 0,
            false,
        )?;
        match rows.as_slice() {
            [] => Ok(None),
            [row] => match row.as_slice() {
                [SqlValue::Integer(sequence)] => Ok(Some(decode(*sequence))),
                _ => Err(failure(
                    LedgerCode::InvalidEventInteger.code(),
                    LedgerLocation::RecoverSqliteSequence.location(),
                )),
            },
            _ => Err(failure(
                LedgerCode::DuplicateEventId.code(),
                LedgerLocation::RecoverEventIds.location(),
            )),
        }
    }

    fn lab_relations(&self, through: u64, seed: &LabRelations) -> GlobalLedgerResult<LabRelations> {
        let connection = self
            .database
            .connection(LedgerLocation::ReadSqliteSnapshot.location())?;
        read_lab_relations(&connection, through, seed)
    }

    fn latest_timestamp(&self) -> GlobalLedgerResult<Option<u64>> {
        let connection = self
            .database
            .connection(LedgerLocation::ReadSqliteSnapshot.location())?;
        let rows = read_rows(
            &connection,
            "SELECT MAX(timestamp_unix_ms) FROM ledger_events",
            None,
            &mut 0,
            false,
        )?;
        match rows.as_slice() {
            [row] => match row.as_slice() {
                [SqlValue::Null] => Ok(None),
                [SqlValue::Integer(timestamp)] => Ok(Some(decode(*timestamp))),
                _ => Err(failure(
                    LedgerCode::InvalidEventInteger.code(),
                    LedgerLocation::ReadSqliteRow.location(),
                )),
            },
            _ => Err(failure(
                LedgerCode::InvalidEventInteger.code(),
                LedgerLocation::ReadSqliteRow.location(),
            )),
        }
    }

    fn persist(&mut self, event: &PersistedEvent) -> GlobalLedgerResult<Option<u64>> {
        self.persist_batch_rows(
            std::slice::from_ref(event),
            &IndexRows::default(),
            None,
            crate::durability::commit_sync(event.payload()),
        )
    }

    fn persist_batch(
        &mut self,
        write: &super::storage::BatchWrite<'_>,
    ) -> GlobalLedgerResult<Option<u64>> {
        self.persist_batch_rows(write.events, write.rows, write.work, write.sync)
    }

    /// Workflow #383 AO-1, AO-2 (`contracts/ledger-observer.md` §9.1; `ledger-store.md`: a failed
    /// append is not proof of absence): reads the database head and the batch's last row. A head
    /// still where the write left it means the batch is absent (one transaction commits all of it
    /// or nothing); a head that names exactly its last record means it is committed, and the
    /// store's head moves to it; anything else is an integrity failure.
    fn read_back_batch(&mut self, events: &[PersistedEvent]) -> GlobalLedgerResult<ReadBack> {
        let (Some(first), Some(last)) = (events.first(), events.last()) else {
            return Ok(ReadBack::Absent);
        };
        let next = increment_sequence(last.sequence())?;
        let projected = project_batch(&self.database, events, self.head_hash.as_deref())?;
        let Some(last_record) = projected.last() else {
            return Ok(ReadBack::Absent);
        };
        let previous_meta = meta_row_with_marker(
            &self.database,
            first.sequence(),
            self.head,
            self.head_hash.as_deref(),
            &self.marker,
        );
        let committed_meta = meta_row_with_marker(
            &self.database,
            next,
            last.sequence(),
            Some(&last_record.hash),
            &self.marker,
        );
        let (meta, rows) = {
            let connection = self
                .database
                .connection(LedgerLocation::ReadBackLedgerWrite.location())?;
            let meta = read_meta_columns(&connection, self.meta_has_marker, None, &mut 0)?;
            let sequence = encode(last.sequence());
            let rows = read_rows(
                &connection,
                &format!("SELECT {EVENT_COLUMNS} FROM ledger_events WHERE sequence={sequence}"),
                None,
                &mut 0,
                true,
            )?;
            (meta, rows)
        };
        if meta == previous_meta && rows.is_empty() {
            return Ok(ReadBack::Absent);
        }
        if meta == committed_meta && rows == vec![last_record.event.clone()] {
            self.head = last.sequence();
            self.head_hash = Some(last_record.hash.clone());
            return Ok(ReadBack::Committed);
        }
        Err(failure(
            LedgerCode::LedgerMetaMismatch.code(),
            LedgerLocation::ReadBackLedgerWrite.location(),
        ))
    }

    fn close(&mut self) -> GlobalLedgerResult<()> {
        // Durable appends were synced before they returned. An observational tail reaches
        // storage at the next durable commit or checkpoint; close adds no barrier (the
        // Host's last append before a successful close is durable). The shared
        // connection belongs to RuntimeDatabase.
        self.ownership.close()
    }
}

/// What one `open_metadata` read and how long each of its phases took (Workflow #363).
pub(super) struct SqliteMetadataRead {
    pub(super) through_sequence: u64,
    pub(super) head_sequence: u64,
    pub(super) bytes: u64,
    pub(super) sql_read: Duration,
    pub(super) verify: Duration,
    pub(super) annotate: Duration,
}

/// With `prefix`, only the declared prefix is read and authenticated (`read_prefix_snapshot`);
/// eviction annotation is then as of that prefix.
pub(super) fn open_metadata(
    database: Arc<RuntimeDatabase>,
    budget: ReadBudget,
    prefix: Option<u64>,
) -> GlobalLedgerResult<(
    Vec<LedgerEventMetadata>,
    SqliteViewSnapshot,
    SqliteMetadataRead,
)> {
    let started = Instant::now();
    let raw = match prefix {
        Some(through) => read_prefix_snapshot(&database, budget, through)?,
        None => read_snapshot(&database, budget)?,
    };
    let sql_read = started.elapsed();
    let bytes = raw.bytes;
    let bound = raw.bound;
    let marker = SqliteMarker::parse(&raw.meta)?;
    let verify_started = Instant::now();
    let verified = verify_snapshot_records(&database, raw)?;
    let verify = verify_started.elapsed();
    if marker.state != "ready" {
        return Err(failure(
            LedgerCode::LedgerCandidateNotProduction.code(),
            LedgerLocation::OpenRuntimeEvidence.location(),
        ));
    }
    let annotate_started = Instant::now();
    let through_sequence = verified
        .records
        .last()
        .map_or(0, StoredEventRecord::sequence);
    let mut events = Vec::with_capacity(verified.records.len());
    let VerifiedSnapshotRecords {
        records,
        metadata,
        head_hash,
    } = verified;
    drop(records);
    for event in metadata {
        check_read_budget(budget, bytes, events.len() + 1)?;
        events.push(event);
    }
    super::retention::annotate_metadata_checked(&mut events, |count| {
        check_read_budget(budget, bytes, count)
    })?;
    check_read_budget(budget, bytes, events.len())?;
    let read = SqliteMetadataRead {
        through_sequence,
        // A full read authenticated the meta row against its last row.
        head_sequence: bound.map_or(through_sequence, |bound| bound.head),
        bytes,
        sql_read,
        verify,
        annotate: annotate_started.elapsed(),
    };
    Ok((
        events,
        SqliteViewSnapshot {
            database,
            through_sequence,
            head_hash,
            budget,
        },
        read,
    ))
}

/// An offline snapshot (`open_metadata`): it retains the physical owner, and each view read reads
/// and authenticates the whole snapshot in one read transaction; the stored view selects the
/// page's rows. The Runtime's reads are the writer's (`SqliteStorage::view_page_on_disk`).
pub(super) struct SqliteViewSnapshot {
    database: Arc<RuntimeDatabase>,
    pub(super) through_sequence: u64,
    head_hash: Option<String>,
    budget: ReadBudget,
}

impl SqliteViewSnapshot {
    /// Workflow #395: see `GlobalLedgerMetadata::set_read_deadline`.
    pub(super) fn set_read_deadline(&mut self, deadline: Instant) {
        self.budget = Some(super::evidence::phase_read_budget(self.budget, deadline));
    }

    pub(super) fn project_view_page(
        &self,
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
    ) -> GlobalLedgerResult<RuntimeEventQueryPage> {
        let bounds =
            super::projection::page_bounds(query, profile, request, self.through_sequence)?;
        check_read_budget(self.budget, 0, 0)?;
        let mut connection = self
            .database
            .connection(LedgerLocation::QueryLedgerView.location())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|error| {
                sql_error(error, LedgerLocation::BeginLedgerViewSnapshot.location())
            })?;
        match self.read_view_page(&transaction, query, profile, request, bounds) {
            Ok(page) => {
                transaction.commit().map_err(|error| {
                    sql_error(error, LedgerLocation::CloseLedgerViewSnapshot.location())
                })?;
                Ok(page)
            }
            Err(error) => Err(
                error.with_close_result(transaction.rollback().map_err(|error| {
                    sql_error(error, LedgerLocation::RollbackLedgerViewSnapshot.location())
                })),
            ),
        }
    }

    fn read_view_page(
        &self,
        connection: &Connection,
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
        (snapshot, after): (u64, u64),
    ) -> GlobalLedgerResult<RuntimeEventQueryPage> {
        let (bytes, verified) = self.verify_view_full(connection)?;
        let VerifiedSnapshotRecords {
            records, metadata, ..
        } = verified;
        // The original records go; the metadata is from this same authenticated snapshot.
        drop(records);
        let mut events = Vec::new();
        for event in metadata
            .into_iter()
            .take_while(|event| event.sequence() <= self.through_sequence)
        {
            check_read_budget(self.budget, bytes, events.len() + 1)?;
            events.push(event);
        }
        super::retention::annotate_metadata_checked(&mut events, |count| {
            check_read_budget(self.budget, bytes, count)
        })?;
        let (sequences, _) = views::select_sequences(
            connection,
            &events,
            query,
            after,
            snapshot,
            usize::from(request.limit()) + 1,
            self.budget,
        )?;
        let page = EventIndexes::from_events(&events).project_view_page(
            &events,
            query,
            profile,
            request,
            LedgerReadScope {
                source: LedgerReadSource::Offline,
                material_read: LedgerMaterialReadState::NotRequested,
                scanned_through_position: self.through_sequence,
                read_complete: true,
                limits: Vec::new(),
            },
            super::projection::PageSelection {
                through_sequence: self.through_sequence,
                sequences: Some(&sequences),
                retention: None,
                work: None,
            },
        )?;
        check_read_budget(self.budget, bytes, events.len())?;
        Ok(page)
    }

    fn verify_view_full(
        &self,
        connection: &Connection,
    ) -> GlobalLedgerResult<(u64, VerifiedSnapshotRecords)> {
        let raw = read_snapshot_connection(connection, self.budget)?;
        let bytes = raw.bytes;
        let prefix_hash = raw
            .events
            .iter()
            .find(|row| row.first() == Some(&SqlValue::Integer(encode(self.through_sequence))))
            .and_then(|row| row.get(11))
            .cloned();
        let verified = verify_snapshot_records(&self.database, raw)?;
        if prefix_hash != self.head_hash.clone().map(SqlValue::Text)
            || verified
                .records
                .last()
                .map_or(0, StoredEventRecord::sequence)
                < self.through_sequence
        {
            return Err(failure(
                LedgerCode::LedgerSnapshotBoundaryMismatch.code(),
                LedgerLocation::QueryLedgerView.location(),
            ));
        }
        Ok((bytes, verified))
    }
}

/// LN-6 (#422, LEAN-L §3.3): the Runtime's view pages and artifact resolution read the store, on
/// the writer's thread. A read authenticates the keyed meta row against the writer's head and
/// head hash (and the stored rows end at that head), selects its rows through the indexes and
/// the view's condition, and reads and authenticates each row it uses on its own; nothing of the
/// history stays resident between reads.
impl SqliteStorage {
    /// The keyed meta row is the writer's (its head and head hash), the stored rows end at that
    /// head, and the stored views are the ones this build declares.
    fn verify_extent(&self, connection: &Connection) -> GlobalLedgerResult<()> {
        refuse_unsupported_format(connection)?;
        views::installed(connection)?;
        let meta = read_meta_columns(connection, self.meta_has_marker, None, &mut 0)?;
        let last = read_rows(
            connection,
            "SELECT MAX(sequence) FROM ledger_events",
            None,
            &mut 0,
            false,
        )?;
        let expected_last = if self.head == 0 {
            SqlValue::Null
        } else {
            SqlValue::Integer(encode(self.head))
        };
        if meta
            != meta_row_with_marker(
                &self.database,
                increment_sequence(self.head)?,
                self.head,
                self.head_hash.as_deref(),
                &self.marker,
            )
            || last != [vec![expected_last]]
        {
            return Err(failure(
                LedgerCode::LedgerMetaMismatch.code(),
                LedgerLocation::VerifySqliteHead.location(),
            ));
        }
        Ok(())
    }

    /// The selected record by one authenticated point read; its eviction proof from the
    /// writer's proof set (`retention`), as of the head.
    fn resolve_artifact_on_disk(
        &self,
        selection: &super::LedgerArtifactSelection,
        deadline: Instant,
        retention: &RetentionIndex,
    ) -> GlobalLedgerResult<super::ResolvedLedgerArtifact> {
        selection.validate()?;
        let budget = Some(super::evidence::artifact_read_budget(None, deadline));
        check_read_budget(budget, 0, 0)?;
        let sequence = selection.event.sequence;
        let events = if sequence > self.head {
            Vec::new()
        } else {
            let mut connection = self
                .database
                .connection(LedgerLocation::ResolveLedgerArtifact.location())?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(|error| {
                    sql_error(error, LedgerLocation::BeginSqliteSnapshot.location())
                })?;
            let query = EventQuery::default();
            let read = self.verify_extent(&transaction).and_then(|()| {
                read_history_rows(
                    &self.database,
                    &transaction,
                    (&HistorySelection::of(&query), None),
                    (sequence - 1, sequence),
                    1,
                )
            });
            match read {
                Ok(events) => {
                    transaction.commit().map_err(|error| {
                        sql_error(error, LedgerLocation::CloseSqliteSnapshot.location())
                    })?;
                    events
                }
                Err(error) => {
                    return Err(
                        error.with_close_result(transaction.rollback().map_err(|error| {
                            sql_error(error, LedgerLocation::CloseSqliteSnapshot.location())
                        })),
                    );
                }
            }
        };
        check_read_budget(budget, 0, events.len())?;
        super::evidence::resolve_artifact_from_events(
            &events,
            selection,
            self.head,
            true,
            deadline,
            Some(retention),
        )
    }

    /// One view page in one read transaction. #411 A51 (R2): every row the selection reads counts
    /// on `read.0`; a page its budget stops answers incomplete. `read.1` is
    /// `ledger_observer.history_page_rows`, the rows of one statement.
    fn view_page_on_disk(
        &self,
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
        observation: &mut Option<LedgerProjectViewObservation>,
        read: (&ReadWork, usize),
        retention: &RetentionIndex,
    ) -> GlobalLedgerResult<RuntimeEventQueryPage> {
        if let Some(value) = observation {
            *value = LedgerProjectViewObservation {
                requested_limit: LedgerProjectViewCount::Observed(u64::from(request.limit())),
                max_page_events: LedgerProjectViewCount::Observed(u64::from(
                    actingcommand_contract::MAX_RUNTIME_EVENT_QUERY_EVENTS,
                )),
                max_response_bytes: LedgerProjectViewCount::from_len(
                    actingcommand_contract::MAX_RUNTIME_EVENT_QUERY_RESPONSE_BYTES,
                ),
                max_recovery_context_events: LedgerProjectViewCount::from_len(
                    MAX_QUERY_PAGE_EVENTS,
                ),
                ..LedgerProjectViewObservation::default()
            };
            value.admission.begin(Instant::now());
        }
        let admission = super::projection::page_bounds(query, profile, request, self.head);
        if let Some(value) = observation {
            value.admission.finish(Instant::now(), admission.is_ok());
        }
        let bounds = admission?;
        if let Some(value) = observation {
            value.connection.begin(Instant::now());
        }
        let acquired = self
            .database
            .connection(LedgerLocation::QueryLedgerView.location());
        if let Some(value) = observation {
            value.connection.finish(Instant::now(), acquired.is_ok());
        }
        let mut connection = acquired?;
        if let Some(value) = observation {
            value.with_connection.begin(Instant::now());
        }
        let completed = (|| {
            if let Some(value) = observation {
                value.begin_transaction.begin(Instant::now());
            }
            let begun = connection
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(|error| {
                    sql_error(error, LedgerLocation::BeginLedgerViewSnapshot.location())
                });
            if let Some(value) = observation {
                value
                    .begin_transaction
                    .finish(Instant::now(), begun.is_ok());
            }
            let transaction = begun?;
            match self.read_view_page(
                &transaction,
                (query, profile, request),
                bounds,
                observation,
                read,
                retention,
            ) {
                Ok(page) => {
                    if let Some(value) = observation {
                        value.commit.begin(Instant::now());
                    }
                    let closed = transaction.commit().map_err(|error| {
                        sql_error(error, LedgerLocation::CloseLedgerViewSnapshot.location())
                    });
                    if let Some(value) = observation {
                        value.commit.finish(Instant::now(), closed.is_ok());
                    }
                    closed?;
                    Ok(page)
                }
                Err(error) => {
                    if let Some(value) = observation {
                        value.rollback.begin(Instant::now());
                    }
                    let closed = transaction.rollback().map_err(|error| {
                        sql_error(error, LedgerLocation::RollbackLedgerViewSnapshot.location())
                    });
                    if let Some(value) = observation {
                        value.rollback.finish(Instant::now(), closed.is_ok());
                    }
                    Err(error.with_close_result(closed))
                }
            }
        })();
        if let Some(value) = observation {
            value
                .with_connection
                .finish(Instant::now(), completed.is_ok());
            if let Ok(page) = &completed {
                value.returned_events =
                    LedgerProjectViewCount::Observed(u64::from(page.returned_count()));
                value.returned_recovery_groups =
                    LedgerProjectViewCount::from_len(page.run_recovery().len());
            }
        }
        completed
    }

    fn read_view_page(
        &self,
        connection: &Connection,
        (query, profile, request): (
            &EventQuery,
            ProjectionProfile,
            &RuntimeEventQueryPageRequest,
        ),
        (snapshot, after): (u64, u64),
        observation: &mut Option<LedgerProjectViewObservation>,
        read: (&ReadWork, usize),
        retention: &RetentionIndex,
    ) -> GlobalLedgerResult<RuntimeEventQueryPage> {
        self.verify_extent(connection)?;
        // Every row's view memberships name the Lab view as of the snapshot.
        let lab = read_lab_relations(connection, snapshot, &LabRelations::default())?;
        let view = query
            .view
            .map(|view| ViewCondition::of(view, &lab))
            .transpose()?;
        let selection = HistorySelection::of(query);
        let selection_limit = usize::from(request.limit()) + 1;
        if let Some(value) = observation {
            value.selection_limit = LedgerProjectViewCount::from_len(selection_limit);
            value.select_sequences.begin(Instant::now());
        }
        let selected = self.select_view_rows(
            connection,
            (&selection, view.as_ref()),
            (after, snapshot),
            selection_limit,
            &lab,
            read,
        );
        if let Some(value) = observation {
            value
                .select_sequences
                .finish(Instant::now(), selected.is_ok());
            value.selection_work = LedgerProjectViewCount::from_len(read.0.visited());
        }
        let (selected, budget_stop) = selected?;
        if let Some(value) = observation {
            value.selected_sequences = LedgerProjectViewCount::from_len(selected.len());
            value.project_page.begin(Instant::now());
        }
        let lab_related = |keys: &LabKeys| lab.related(keys);
        let mut run_context = |run: &RunId| self.run_rows(connection, run, snapshot, read.1);
        let page = super::projection::project_selected(
            selected,
            budget_stop,
            query,
            profile,
            request,
            LedgerReadScope {
                source: LedgerReadSource::Runtime,
                material_read: LedgerMaterialReadState::NotRequested,
                scanned_through_position: snapshot,
                read_complete: true,
                limits: Vec::new(),
            },
            snapshot,
            super::projection::PageContext {
                lab_related: &lab_related,
                run_context: &mut run_context,
                retention: Some(retention),
            },
        );
        if let Some(value) = observation {
            value.project_page.finish(Instant::now(), page.is_ok());
        }
        page
    }

    /// The rows of one view page: the committed rows the query and the view select after
    /// `range.0` through `range.1`, in ledger order, at most `limit`, each read and authenticated
    /// on its own, at most `read.1` rows per statement. Every row read counts on `read.0`; when
    /// its budget runs out, the selection ends after that row and names it (#411 A51).
    fn select_view_rows(
        &self,
        connection: &Connection,
        (selection, view): (&HistorySelection<'_>, Option<&ViewCondition>),
        (after, snapshot): (u64, u64),
        limit: usize,
        lab: &LabRelations,
        (work, page_rows): (&ReadWork, usize),
    ) -> GlobalLedgerResult<(Vec<LedgerEventMetadata>, Option<u64>)> {
        let query = selection.query;
        let mut cursor = after.max(query.from_sequence.map_or(0, |from| from.saturating_sub(1)));
        let through = snapshot.min(query.to_sequence.unwrap_or(u64::MAX));
        let mut selected = Vec::new();
        while cursor < through {
            // #422 LN-TUNE: a statement asks for no more rows than the page can still select,
            // so no statement holds a row past the one the limit is reached at; the next page
            // starts after that row, and each row is read and authenticated once.
            let rows = page_rows.min(limit.saturating_sub(selected.len()));
            let page = read_history_rows(
                &self.database,
                connection,
                (selection, view),
                (cursor, through),
                rows,
            )?;
            let full = page.len() >= rows;
            for event in page {
                cursor = event.sequence();
                let going_on = work.visit();
                if selection.selects(&event) && in_view(query.view, &event, lab) {
                    selected.push(event);
                    if selected.len() == limit {
                        return Ok((selected, None));
                    }
                }
                if !going_on {
                    return Ok((selected, Some(cursor)));
                }
            }
            if !full {
                break;
            }
        }
        Ok((selected, None))
    }

    /// The records of `run` at or before `snapshot` in ledger order, at most one more than a
    /// page's recovery context holds (`MAX_QUERY_PAGE_EVENTS`), through the run's link index.
    fn run_rows(
        &self,
        connection: &Connection,
        run: &RunId,
        snapshot: u64,
        page_rows: usize,
    ) -> GlobalLedgerResult<Vec<LedgerEventMetadata>> {
        let query = EventQuery {
            run_id: Some(*run),
            ..EventQuery::default()
        };
        let selection = HistorySelection::of(&query);
        let wanted = MAX_QUERY_PAGE_EVENTS + 1;
        let mut cursor = 0;
        let mut events = Vec::new();
        while cursor < snapshot && events.len() < wanted {
            let rows = page_rows.min(wanted - events.len());
            let page = read_history_rows(
                &self.database,
                connection,
                (&selection, None),
                (cursor, snapshot),
                rows,
            )?;
            let full = page.len() >= rows;
            if let Some(last) = page.last() {
                cursor = last.sequence();
            }
            events.extend(page.into_iter().filter(|event| selection.selects(event)));
            if !full {
                break;
            }
        }
        Ok(events)
    }
}

/// Whether `event` is in `view` at a page's snapshot (`lab`: the Lab relations through it).
fn in_view<E: LedgerEventRead>(view: Option<LedgerView>, event: &E, lab: &LabRelations) -> bool {
    view.is_none_or(|view| {
        view.contains(
            event.event_type(),
            event.severity(),
            event.origin().source(),
            view == LedgerView::Lab && lab.related(&LabKeys::of(event.links())),
        )
    })
}

fn upgrade_views(
    database: &RuntimeDatabase,
    head: u64,
    hash: Option<&str>,
) -> GlobalLedgerResult<()> {
    let mut connection = database.connection(LedgerLocation::UpgradeLedgerViews.location())?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sql_error(error, LedgerLocation::BeginLedgerViewUpgrade.location()))?;
    let result = (|| {
        refuse_unsupported_format(&transaction)?;
        if !views::installed(&transaction)? {
            let raw = read_snapshot_connection(&transaction, None)?;
            let VerifiedSnapshotRecords {
                records,
                head_hash: actual_hash,
                ..
            } = verify_snapshot_records(database, raw)?;
            if records.last().map_or(0, StoredEventRecord::sequence) != head
                || actual_hash.as_deref() != hash
            {
                return Err(failure(
                    LedgerCode::LedgerSnapshotBoundaryMismatch.code(),
                    LedgerLocation::UpgradeLedgerViews.location(),
                ));
            }
            views::deploy(&transaction)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => transaction
            .commit()
            .map_err(|error| sql_error(error, LedgerLocation::CommitLedgerViewUpgrade.location())),
        Err(error) => Err(
            error.with_close_result(transaction.rollback().map_err(|error| {
                sql_error(error, LedgerLocation::RollbackLedgerViewUpgrade.location())
            })),
        ),
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

    pub(super) fn open_formal<F>(
        database: &RuntimeDatabase,
        budget: ReadBudget,
        mut verifier: F,
    ) -> GlobalLedgerResult<Self>
    where
        F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
    {
        let raw = read_snapshot(database, budget)?;
        let marker = SqliteMarker::parse(&raw.meta)?;
        let (events, _) = verify_snapshot(database, raw, &mut Some(&mut verifier))?;
        if marker.state != "ready" {
            return Err(failure(
                LedgerCode::LedgerCandidateNotProduction.code(),
                LedgerLocation::OpenRuntimeEvidence.location(),
            ));
        }
        Ok(Self {
            indexes: EventIndexes::from_events(&events),
            events,
        })
    }

    /// Workflow #375 R5a: `open_formal` for the maintenance binding read, which reads no
    /// artifact material; a reference without an eviction proof stays `Unrecorded`.
    pub(super) fn open_formal_unread(
        database: &RuntimeDatabase,
        budget: ReadBudget,
    ) -> GlobalLedgerResult<Self> {
        let raw = read_snapshot(database, budget)?;
        let marker = SqliteMarker::parse(&raw.meta)?;
        let (events, _) = verify_snapshot_unread(database, raw, &mut LedgerOpenTiming::default())?;
        if marker.state != "ready" {
            return Err(failure(
                LedgerCode::LedgerCandidateNotProduction.code(),
                LedgerLocation::OpenRuntimeEvidence.location(),
            ));
        }
        Ok(Self {
            indexes: EventIndexes::from_events(&events),
            events,
        })
    }

    /// Workflow #375 H2: the single complete read of `ledger-maintenance verify` on a formal
    /// root. Like the writer open it is bounded by the head of the keyed meta row,
    /// authenticated first in the same read transaction, and by the deadline; it has no
    /// event or byte cap. Returns the restored events and the root's storage status (the
    /// receipt's `ledger`, as `status` computes it). Workflow #375 R5a: it reads no artifact
    /// material. `timing` keeps the phases that ran, also when an error is returned.
    pub(super) fn read_formal_verified(
        database: &RuntimeDatabase,
        deadline: Instant,
        timing: &mut LedgerOpenTiming,
    ) -> GlobalLedgerResult<(Vec<PersistedEvent>, super::LedgerStorageStatus)> {
        let started = Instant::now();
        let raw = read_formal_snapshot(database, deadline);
        timing.sql_read_ms = elapsed_ms(started);
        let raw = raw?;
        timing.events = raw.events.len() as u64;
        let marker = SqliteMarker::parse(&raw.meta)?;
        let (events, hash) = verify_snapshot_unread(database, raw, timing)?;
        let status = marker.status(&events, hash);
        Ok((events, status))
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
}

fn validate_root(root: &std::path::Path, database: &RuntimeDatabase) -> GlobalLedgerResult<()> {
    let root = root.canonicalize().map_err(|error| {
        GlobalLedgerError::io(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::ResolveCandidateRoot.location(),
            &error,
        )
    })?;
    if root.join("segments").exists() {
        return Err(failure(
            LedgerCode::CandidateSegmentRootPresent.code(),
            LedgerLocation::OpenSqliteLedger.location(),
        ));
    }
    let database_root = database.root().canonicalize().map_err(|error| {
        GlobalLedgerError::io(
            LedgerCode::LedgerIo.code(),
            LedgerLocation::ResolveDatabaseRoot.location(),
            &error,
        )
    })?;
    if root != database_root {
        return Err(failure(
            LedgerCode::LedgerDatabaseRootMismatch.code(),
            LedgerLocation::OpenSqliteLedger.location(),
        ));
    }
    Ok(())
}

fn initialize(database: &RuntimeDatabase, first_use: bool) -> GlobalLedgerResult<()> {
    let mut connection = database.connection(LedgerLocation::InitializeSqliteLedger.location())?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sql_error(error, LedgerLocation::BeginLedgerSchema.location()))?;
    let result = (|| {
        refuse_unsupported_format(&transaction)?;
        let tables: i64 = transaction.query_row("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN ('ledger_events','ledger_links','ledger_artifacts','ledger_meta')", [], |row| row.get(0))
        .map_err(|error| sql_error(error, LedgerLocation::InspectLedgerSchema.location()))?;
        match tables {
            0 if first_use => {
                require_empty_format(&transaction)?;
                transaction
                    .execute_batch(include_str!("sqlite/schema.sql"))
                    .map_err(|error| {
                        sql_error(error, LedgerLocation::InitializeLedgerSchema.location())
                    })?;
                views::deploy(&transaction)?;
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
                    LedgerCode::LedgerSchemaIncomplete.code(),
                    LedgerLocation::InitializeSqliteLedger.location(),
                ));
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => transaction
            .commit()
            .map_err(|error| sql_error(error, LedgerLocation::CommitLedgerSchema.location())),
        Err(error) => Err(error.with_close_result(
            transaction
                .rollback()
                .map_err(|error| sql_error(error, LedgerLocation::RollbackLedgerSchema.location())),
        )),
    }
}

struct RawSnapshot {
    format_version: i64,
    events: Vec<SqlRow>,
    links: Vec<SqlRow>,
    artifacts: Vec<SqlRow>,
    meta: SqlRow,
    budget: ReadBudget,
    bytes: u64,
    /// Present only for `read_prefix_snapshot`.
    bound: Option<PrefixBound>,
}

/// Workflow #363: the extent of a prefix read, taken from the keyed head row that was
/// authenticated in the same read transaction before any history row was read.
#[derive(Clone, Copy)]
struct PrefixBound {
    /// The last row read: the declared prefix, or a migrated root's cutover completion.
    through: u64,
    /// The authenticated head, at least `through`.
    head: u64,
}

fn read_snapshot(
    database: &RuntimeDatabase,
    budget: ReadBudget,
) -> GlobalLedgerResult<RawSnapshot> {
    check_read_budget(budget, 0, 0)?;
    let mut connection = database.connection(LedgerLocation::ReadSqliteSnapshot.location())?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|error| sql_error(error, LedgerLocation::BeginSqliteSnapshot.location()))?;
    let raw = read_snapshot_connection(&transaction, budget)?;
    transaction
        .commit()
        .map_err(|error| sql_error(error, LedgerLocation::CloseSqliteSnapshot.location()))?;
    Ok(raw)
}

/// `read_snapshot` for the writer open: the keyed meta row is authenticated first in the
/// same read transaction, and the read is bounded by its head sequence and the caller's
/// deadline rather than by maintenance material limits.
fn read_formal_snapshot(
    database: &RuntimeDatabase,
    deadline: Instant,
) -> GlobalLedgerResult<RawSnapshot> {
    check_read_budget(Some((u64::MAX, usize::MAX, deadline)), 0, 0)?;
    let mut connection = database.connection(LedgerLocation::ReadSqliteSnapshot.location())?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|error| sql_error(error, LedgerLocation::BeginSqliteSnapshot.location()))?;
    let head = match authenticated_extent(database, &transaction)? {
        Some((marker, head)) if marker.state == "ready" => head,
        _ => {
            return Err(failure(
                SharedCode::LedgerMigrationRequired.code(),
                LedgerLocation::OpenRuntimeLedger.location(),
            ));
        }
    };
    let head = usize::try_from(head).map_err(|_| {
        failure(
            LedgerCode::LedgerSnapshotOverflow.code(),
            LedgerLocation::BoundSqliteSnapshot.location(),
        )
    })?;
    let raw = read_snapshot_connection(&transaction, Some((u64::MAX, head, deadline)))?;
    transaction
        .commit()
        .map_err(|error| sql_error(error, LedgerLocation::CloseSqliteSnapshot.location()))?;
    Ok(raw)
}

/// Workflow #363: `read_snapshot` bounded by a declared prefix. In one read transaction the
/// keyed meta row is authenticated first, then only the rows `1..=L` of the three tables are
/// read, where `L` is `through` or, on a migrated root, at least its cutover completion
/// (the marker check needs it). Rows after `L` are never read.
fn read_prefix_snapshot(
    database: &RuntimeDatabase,
    budget: ReadBudget,
    through: u64,
) -> GlobalLedgerResult<RawSnapshot> {
    check_read_budget(budget, 0, 0)?;
    let mut connection = database.connection(LedgerLocation::ReadSqliteSnapshot.location())?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|error| sql_error(error, LedgerLocation::BeginSqliteSnapshot.location()))?;
    refuse_unsupported_format(&transaction)?;
    views::installed(&transaction)?;
    // The caller found the four ledger tables; their absence now is a fatal inconsistency.
    let Some((marker, head)) = authenticated_extent(database, &transaction)? else {
        return Err(failure(
            LedgerCode::LedgerMetaMissing.code(),
            LedgerLocation::ReadSqlitePrefix.location(),
        ));
    };
    if marker.state != "ready" {
        return Err(failure(
            LedgerCode::LedgerCandidateNotProduction.code(),
            LedgerLocation::OpenRuntimeEvidence.location(),
        ));
    }
    if through > head {
        return Err(GlobalLedgerError::request(
            LedgerCode::LedgerPrefixBeyondHead.code(),
            LedgerLocation::ReadSqlitePrefix.location(),
        )
        .with_detail(format!("through_sequence {through} exceeds head {head}")));
    }
    let through = marker
        .migration
        .as_ref()
        .map_or(through, |record| through.max(record.cutover_sequence));
    let last = encode(through);
    let mut bytes = 0;
    let meta = read_meta_with_budget(&transaction, budget, &mut bytes)?;
    let events = read_rows(
        &transaction,
        &format!(
            "SELECT {EVENT_COLUMNS} FROM ledger_events WHERE sequence<={last} ORDER BY sequence"
        ),
        budget,
        &mut bytes,
        true,
    )?;
    let links = read_rows(
        &transaction,
        &format!(
            "SELECT {LINK_COLUMNS} FROM ledger_links WHERE sequence<={last} ORDER BY sequence"
        ),
        budget,
        &mut bytes,
        false,
    )?;
    let artifacts = read_rows(
        &transaction,
        &format!(
            "SELECT {ARTIFACT_COLUMNS} FROM ledger_artifacts WHERE sequence<={last} ORDER BY sequence,ordinal"
        ),
        budget,
        &mut bytes,
        false,
    )?;
    let raw = RawSnapshot {
        format_version: format_version(&transaction)?,
        events,
        links,
        artifacts,
        meta,
        budget,
        bytes,
        bound: Some(PrefixBound { through, head }),
    };
    transaction
        .commit()
        .map_err(|error| sql_error(error, LedgerLocation::CloseSqliteSnapshot.location()))?;
    Ok(raw)
}

/// What one `read_selected` read (Workflow #375 R375-3).
pub(super) struct SelectedRead {
    /// The authenticated rows of the selected types, in ledger order.
    pub(super) events: Vec<LedgerEventMetadata>,
    /// The authenticated head of the keyed meta row.
    pub(super) head_sequence: u64,
}

/// Workflow #375 R375-3: in one read transaction, the keyed meta row, the contiguity of the
/// stored sequences (`1..=head`), the head row and the rows of `event_types`, each read and
/// authenticated once on its own (decode, metadata, stored sequence, predecessor hash, tag and
/// index columns) with its link and artifact rows. Rows of other types are never read, so this
/// read does not establish their integrity; the migrated-prefix digest is not run.
pub(super) fn read_selected(
    database: &RuntimeDatabase,
    event_types: &[EventType],
    budget: ReadBudget,
) -> GlobalLedgerResult<SelectedRead> {
    check_read_budget(budget, 0, 0)?;
    let mut connection = database.connection(LedgerLocation::ReadSqliteSnapshot.location())?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|error| sql_error(error, LedgerLocation::BeginSqliteSnapshot.location()))?;
    let read = read_selected_rows(database, &transaction, event_types, budget)?;
    transaction
        .commit()
        .map_err(|error| sql_error(error, LedgerLocation::CloseSqliteSnapshot.location()))?;
    Ok(read)
}

fn read_selected_rows(
    database: &RuntimeDatabase,
    connection: &Connection,
    event_types: &[EventType],
    budget: ReadBudget,
) -> GlobalLedgerResult<SelectedRead> {
    refuse_unsupported_format(connection)?;
    // A different view schema refuses; absent views leave the selection to a table scan.
    views::installed(connection)?;
    let (head, head_hash) = authenticated_head(database, connection)?;
    let mut reader = RowReader::new(database, connection, budget);
    verify_sequence_extent(connection, head, budget, &mut reader.bytes)?;
    let mut types = Vec::with_capacity(event_types.len());
    for event_type in event_types {
        types.push(SqlValue::Text(views::key(event_type)?));
    }
    let placeholders = vec!["?"; types.len()].join(",");
    let selected =
        format!("SELECT sequence FROM ledger_events WHERE event_type IN ({placeholders})");
    let mut rows = read_rows_bound(
        connection,
        &format!(
            "SELECT {EVENT_COLUMNS} FROM ledger_events WHERE event_type IN ({placeholders}) ORDER BY sequence"
        ),
        &types,
        budget,
        &mut reader.bytes,
        true,
    )?;
    let selected_count = rows.len();
    let head_value = [SqlValue::Integer(encode(head))];
    // The head is read once: a selected head is already the last selected row.
    let head_unselected =
        head != 0 && rows.last().and_then(|row| row.first()) != Some(&head_value[0]);
    if head_unselected {
        let head_rows = read_rows_bound(
            connection,
            &format!("SELECT {EVENT_COLUMNS} FROM ledger_events WHERE sequence=?"),
            &head_value,
            budget,
            &mut reader.bytes,
            true,
        )?;
        let [head_row] = <[SqlRow; 1]>::try_from(head_rows).map_err(|_| {
            failure(
                LedgerCode::SequenceDiscontinuity.code(),
                LedgerLocation::RecoverSequence.location(),
            )
        })?;
        rows.push(head_row);
    }
    let mut metadata = Vec::with_capacity(rows.len());
    let mut expected_links = Vec::with_capacity(rows.len());
    let mut expected_artifacts = Vec::new();
    for row in rows {
        let (event, links, artifacts) = reader.authenticate(row, metadata.len() + 1)?;
        expected_links.push(links);
        expected_artifacts.extend(artifacts);
        metadata.push(event);
    }
    let mut links = read_rows_bound(
        connection,
        &format!(
            "SELECT {LINK_COLUMNS} FROM ledger_links WHERE sequence IN ({selected}) ORDER BY sequence"
        ),
        &types,
        budget,
        &mut reader.bytes,
        false,
    )?;
    let mut artifacts = read_rows_bound(
        connection,
        &format!(
            "SELECT {ARTIFACT_COLUMNS} FROM ledger_artifacts WHERE sequence IN ({selected}) ORDER BY sequence,ordinal"
        ),
        &types,
        budget,
        &mut reader.bytes,
        false,
    )?;
    if head_unselected {
        links.extend(read_rows_bound(
            connection,
            &format!("SELECT {LINK_COLUMNS} FROM ledger_links WHERE sequence=?"),
            &head_value,
            budget,
            &mut reader.bytes,
            false,
        )?);
        artifacts.extend(read_rows_bound(
            connection,
            &format!(
                "SELECT {ARTIFACT_COLUMNS} FROM ledger_artifacts WHERE sequence=? ORDER BY ordinal"
            ),
            &head_value,
            budget,
            &mut reader.bytes,
            false,
        )?);
    }
    if links != expected_links || artifacts != expected_artifacts {
        return Err(failure(
            LedgerCode::LedgerIndexMismatch.code(),
            LedgerLocation::VerifySqliteRelations.location(),
        ));
    }
    // The last row read is the head: its authenticated hash is the meta row's head hash.
    if reader.last.take().map(|(_, hash)| hash) != head_hash {
        return Err(failure(
            LedgerCode::LedgerMetaMismatch.code(),
            LedgerLocation::VerifySqliteHead.location(),
        ));
    }
    check_read_budget(budget, reader.bytes, metadata.len())?;
    metadata.truncate(selected_count);
    Ok(SelectedRead {
        events: metadata,
        head_sequence: head,
    })
}

/// The keyed meta row of a ready ledger, authenticated in this read transaction: its head
/// sequence and head record hash (Workflow #375 R375-3; shared with Workflow #411 A4). No history
/// row is read.
fn authenticated_head(
    database: &RuntimeDatabase,
    connection: &Connection,
) -> GlobalLedgerResult<(u64, Option<String>)> {
    let Some((marker, head)) = authenticated_extent(database, connection)? else {
        return Err(failure(
            LedgerCode::LedgerMetaMissing.code(),
            LedgerLocation::ReadSqliteMeta.location(),
        ));
    };
    if marker.state != "ready" {
        return Err(failure(
            LedgerCode::LedgerCandidateNotProduction.code(),
            LedgerLocation::OpenRuntimeEvidence.location(),
        ));
    }
    // The meta row was authenticated above, in this read transaction.
    let meta = read_meta(connection)?;
    let head_hash = match meta.get(4) {
        Some(SqlValue::Text(hash)) if head != 0 => Some(hash.clone()),
        Some(SqlValue::Null) if head == 0 => None,
        _ => {
            return Err(failure(
                LedgerCode::LedgerMetaMismatch.code(),
                LedgerLocation::VerifySqliteHead.location(),
            ));
        }
    };
    Ok((head, head_hash))
}

/// Rows per statement of a range read (Workflow #411 A4).
#[cfg(not(test))]
const RANGE_CHUNK_ROWS: usize = 256;
/// Tests read their ranges in small chunks, so that a small root spans several.
#[cfg(test)]
const RANGE_CHUNK_ROWS: usize = 4;

/// Stored event rows read in one read transaction, each authenticated on its own (Workflow #375
/// R375-3; Workflow #411 A4 reads its range with it).
struct RowReader<'a> {
    database: &'a RuntimeDatabase,
    connection: &'a Connection,
    budget: ReadBudget,
    bytes: u64,
    ids: BTreeSet<EventId>,
    /// The sequence and authenticated record hash of the previous row read.
    last: Option<(u64, String)>,
}

impl<'a> RowReader<'a> {
    fn new(database: &'a RuntimeDatabase, connection: &'a Connection, budget: ReadBudget) -> Self {
        Self {
            database,
            connection,
            budget,
            bytes: 0,
            ids: BTreeSet::new(),
            last: None,
        }
    }

    /// One stored event row, decoded and authenticated on its own: its record decodes under the
    /// global schema and validates, its stored sequence is its record's, its event id is unique
    /// among the rows read, and the row equals its projection over the stored bytes chained from
    /// its predecessor's stored hash (the previous row read when that is the predecessor). Returns
    /// its metadata and the link and artifact rows it requires. `count` is the budget's event
    /// count with this row.
    fn authenticate(
        &mut self,
        row: SqlRow,
        count: usize,
    ) -> GlobalLedgerResult<(LedgerEventMetadata, SqlRow, Vec<SqlRow>)> {
        check_read_budget(self.budget, self.bytes, count)?;
        let Some(SqlValue::Blob(record)) = row.get(10) else {
            return Err(failure(
                LedgerCode::CorruptLedgerRecord.code(),
                LedgerLocation::ReadCanonicalRecord.location(),
            ));
        };
        let value = serde_json::from_slice::<UniqueJsonValue>(record)
            .map_err(|error| {
                GlobalLedgerError::json(
                    LedgerCode::CorruptLedgerRecord.code(),
                    LedgerLocation::ParseSqliteRecord.location(),
                    &error,
                )
            })?
            .0;
        if value.get("schema_version").and_then(Value::as_str) != Some(GLOBAL_EVENT_SCHEMA_VERSION)
        {
            return Err(failure(
                LedgerCode::UnsupportedEventSchema.code(),
                LedgerLocation::RecoverEventSchema.location(),
            ));
        }
        let stored: StoredEventRecord = serde_json::from_value(value).map_err(|error| {
            GlobalLedgerError::json(
                LedgerCode::CorruptLedgerRecord.code(),
                LedgerLocation::DecodeSqliteRecord.location(),
                &error,
            )
        })?;
        let event = stored.clone().into_metadata().map_err(|error| {
            failure(
                error.code(),
                LedgerLocation::ValidatePersistedEvent.location(),
            )
        })?;
        let Some(SqlValue::Integer(stored_sequence)) = row.first() else {
            return Err(failure(
                LedgerCode::InvalidEventInteger.code(),
                LedgerLocation::RecoverSqliteSequence.location(),
            ));
        };
        let sequence = event.sequence();
        if decode(*stored_sequence) != sequence {
            return Err(failure(
                LedgerCode::LedgerRecordMismatch.code(),
                LedgerLocation::RecoverSqliteSequence.location(),
            ));
        }
        if !self.ids.insert(*event.event_id()) {
            return Err(failure(
                LedgerCode::DuplicateEventId.code(),
                LedgerLocation::RecoverEventIds.location(),
            ));
        }
        // The predecessor's stored hash, as `verify_transaction_row` reads it; the previous
        // row read, when it is the predecessor, was authenticated with that same hash.
        let previous = match sequence.checked_sub(1) {
            Some(0) => None,
            Some(predecessor) => match self.last.take() {
                Some((read, hash)) if read == predecessor => Some(hash),
                _ => Some(stored_record_hash(
                    self.connection,
                    predecessor,
                    self.budget,
                    &mut self.bytes,
                )?),
            },
            None => {
                return Err(failure(
                    LedgerCode::SequenceDiscontinuity.code(),
                    LedgerLocation::RecoverSequence.location(),
                ));
            }
        };
        let projected =
            project_stored_bytes(self.database, &stored, record.clone(), previous.as_deref())?;
        if row != projected.event {
            return Err(failure(
                LedgerCode::LedgerRecordMismatch.code(),
                LedgerLocation::VerifySqliteRecord.location(),
            ));
        }
        check_read_budget(self.budget, self.bytes, count)?;
        self.last = Some((sequence, projected.hash));
        Ok((event, projected.links, projected.artifacts))
    }

    /// Workflow #411 A4: the rows `after < sequence <= through` (only those whose type is one of
    /// `types` when it is not empty), read in chunks of `RANGE_CHUNK_ROWS` rows. Each row is
    /// authenticated on its own and each chunk's link and artifact rows must be the ones its rows
    /// require; then every row of the chunk is passed to `visit` in ledger order. No further chunk
    /// is read after a chunk in which `visit` returned false. A `contiguous` read requires the
    /// rows `after + 1, after + 2, …` up to `through`. Returns the last sequence read.
    fn read_chunks(
        &mut self,
        types: &[SqlValue],
        after: u64,
        through: u64,
        contiguous: bool,
        visit: &mut dyn FnMut(LedgerEventMetadata) -> GlobalLedgerResult<bool>,
    ) -> GlobalLedgerResult<Option<u64>> {
        let filter = if types.is_empty() {
            String::new()
        } else {
            format!("event_type IN ({}) AND ", vec!["?"; types.len()].join(","))
        };
        let events_sql = format!(
            "SELECT {EVENT_COLUMNS} FROM ledger_events WHERE {filter}sequence>? AND sequence<=? ORDER BY sequence LIMIT {RANGE_CHUNK_ROWS}"
        );
        let selected =
            format!("SELECT sequence FROM ledger_events WHERE {filter}sequence>? AND sequence<=?");
        let links_sql = format!(
            "SELECT {LINK_COLUMNS} FROM ledger_links WHERE sequence IN ({selected}) ORDER BY sequence"
        );
        let artifacts_sql = format!(
            "SELECT {ARTIFACT_COLUMNS} FROM ledger_artifacts WHERE sequence IN ({selected}) ORDER BY sequence,ordinal"
        );
        let bounded = |from: u64, to: u64| {
            let mut values = types.to_vec();
            values.push(SqlValue::Integer(encode(from)));
            values.push(SqlValue::Integer(encode(to)));
            values
        };
        let mut cursor = after;
        let mut last_read = None;
        let mut more = true;
        while more && cursor < through {
            let rows = read_rows_bound(
                self.connection,
                &events_sql,
                &bounded(cursor, through),
                self.budget,
                &mut self.bytes,
                true,
            )?;
            if rows.is_empty() {
                if contiguous {
                    return Err(failure(
                        LedgerCode::SequenceDiscontinuity.code(),
                        LedgerLocation::RecoverSequence.location(),
                    ));
                }
                break;
            }
            let mut chunk = Vec::with_capacity(rows.len());
            let mut expected_links = Vec::with_capacity(rows.len());
            let mut expected_artifacts = Vec::new();
            let mut expected = cursor;
            for row in rows {
                let (event, links, artifacts) = self.authenticate(row, chunk.len() + 1)?;
                expected += 1;
                if contiguous && event.sequence() != expected {
                    return Err(failure(
                        LedgerCode::SequenceDiscontinuity.code(),
                        LedgerLocation::RecoverSequence.location(),
                    ));
                }
                expected_links.push(links);
                expected_artifacts.extend(artifacts);
                chunk.push(event);
            }
            let chunk_last = chunk.last().map_or(cursor, |event| event.sequence());
            let stored_links = read_rows_bound(
                self.connection,
                &links_sql,
                &bounded(cursor, chunk_last),
                self.budget,
                &mut self.bytes,
                false,
            )?;
            let stored_artifacts = read_rows_bound(
                self.connection,
                &artifacts_sql,
                &bounded(cursor, chunk_last),
                self.budget,
                &mut self.bytes,
                false,
            )?;
            if stored_links != expected_links || stored_artifacts != expected_artifacts {
                return Err(failure(
                    LedgerCode::LedgerIndexMismatch.code(),
                    LedgerLocation::VerifySqliteRelations.location(),
                ));
            }
            for event in chunk {
                let wants_more = visit(event)?;
                more = more && wants_more;
            }
            last_read = Some(chunk_last);
            cursor = chunk_last;
        }
        Ok(last_read)
    }
}

/// What one `read_range` read (Workflow #411 A4).
pub(super) struct RangeRead {
    /// The matching events in ledger order, with the eviction proofs of their artifacts when
    /// they were asked for.
    pub(super) events: Vec<PersistedEvent>,
    /// The authenticated head of the keyed meta row.
    pub(super) head_sequence: u64,
}

/// Workflow #411 A4: in one read transaction, the keyed meta row, the contiguity of the stored
/// sequences (`1..=head`) and the head row, then the rows `after < sequence <= bound` in chunks
/// until `request.matches` of them match `request.query` or `bound` is reached. `bound` is
/// `request.through_sequence`, else the head of this transaction, and never beyond that head.
/// With `request.eviction_proofs`, the eviction intent and outcome rows after the last row read
/// are read too (selected by their type) and give the matching events their eviction proofs, each
/// carrying this head. Every row is read and authenticated once, on its own, as `read_selected`
/// reads its rows. Other rows are never read, so this read does not establish their integrity;
/// there is no retention replay and no migrated-prefix digest.
pub(super) fn read_range(
    database: &RuntimeDatabase,
    request: &super::GlobalLedgerRangeRequest,
) -> GlobalLedgerResult<RangeRead> {
    let mut connection = database.connection(LedgerLocation::ReadSqliteSnapshot.location())?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|error| sql_error(error, LedgerLocation::BeginSqliteSnapshot.location()))?;
    let read = read_range_rows(database, &transaction, request)?;
    transaction
        .commit()
        .map_err(|error| sql_error(error, LedgerLocation::CloseSqliteSnapshot.location()))?;
    Ok(read)
}

fn read_range_rows(
    database: &RuntimeDatabase,
    connection: &Connection,
    request: &super::GlobalLedgerRangeRequest,
) -> GlobalLedgerResult<RangeRead> {
    refuse_unsupported_format(connection)?;
    // A different view schema refuses; absent views leave the eviction rows to a table scan.
    views::installed(connection)?;
    let (head, head_hash) = authenticated_head(database, connection)?;
    let mut reader = RowReader::new(database, connection, None);
    verify_sequence_extent(connection, head, None, &mut reader.bytes)?;
    // Workflow #411 A4: a test may let a writer append here; this read stays at `head`.
    #[cfg(test)]
    crate::read_count::range_head_read();
    let bound = request
        .through_sequence
        .map_or(head, |through| through.min(head));
    let mut proofs = super::retention::RangeProofs::new(head);
    let mut matched: Vec<LedgerEventMetadata> = Vec::new();
    let last_read = reader.read_chunks(
        &[],
        request.after_sequence,
        bound,
        true,
        &mut |event: LedgerEventMetadata| -> GlobalLedgerResult<bool> {
            let matching = matched.len() < request.matches
                && super::projection::query_matches_fields(&request.query, &event);
            if request.eviction_proofs {
                if matching {
                    proofs.want(&event)?;
                }
                proofs.apply(&event)?;
            }
            if matching {
                matched.push(event);
            }
            Ok(matched.len() < request.matches)
        },
    )?;
    if head != 0 && last_read != Some(head) {
        if request.eviction_proofs && proofs.wants_any() {
            // The eviction rows after the last row read and before the head; the head follows.
            let types = [
                SqlValue::Text(views::key(EventType::ArtifactEvictionIntent)?),
                SqlValue::Text(views::key(EventType::ArtifactEvictionOutcome)?),
            ];
            reader.read_chunks(
                &types,
                last_read.unwrap_or(request.after_sequence),
                head - 1,
                false,
                &mut |event: LedgerEventMetadata| -> GlobalLedgerResult<bool> {
                    proofs.apply(&event)?;
                    Ok(true)
                },
            )?;
        }
        let head_value = [SqlValue::Integer(encode(head))];
        let head_rows = read_rows_bound(
            connection,
            &format!("SELECT {EVENT_COLUMNS} FROM ledger_events WHERE sequence=?"),
            &head_value,
            None,
            &mut reader.bytes,
            true,
        )?;
        let [head_row] = <[SqlRow; 1]>::try_from(head_rows).map_err(|_| {
            failure(
                LedgerCode::SequenceDiscontinuity.code(),
                LedgerLocation::RecoverSequence.location(),
            )
        })?;
        let (event, links, artifacts) = reader.authenticate(head_row, 1)?;
        let stored_links = read_rows_bound(
            connection,
            &format!("SELECT {LINK_COLUMNS} FROM ledger_links WHERE sequence=?"),
            &head_value,
            None,
            &mut reader.bytes,
            false,
        )?;
        let stored_artifacts = read_rows_bound(
            connection,
            &format!(
                "SELECT {ARTIFACT_COLUMNS} FROM ledger_artifacts WHERE sequence=? ORDER BY ordinal"
            ),
            &head_value,
            None,
            &mut reader.bytes,
            false,
        )?;
        if stored_links != [links] || stored_artifacts != artifacts {
            return Err(failure(
                LedgerCode::LedgerIndexMismatch.code(),
                LedgerLocation::VerifySqliteRelations.location(),
            ));
        }
        if request.eviction_proofs && proofs.wants_any() {
            proofs.apply(&event)?;
        }
    }
    // The last row read is the head: its authenticated hash is the meta row's head hash.
    if reader.last.take().map(|(_, hash)| hash) != head_hash {
        return Err(failure(
            LedgerCode::LedgerMetaMismatch.code(),
            LedgerLocation::VerifySqliteHead.location(),
        ));
    }
    let mut events = Vec::with_capacity(matched.len());
    for event in matched {
        let evictions = proofs.event_proofs(&event);
        let mut event = event
            .into_event_with_artifact_availability(&mut |reference| proofs.availability(reference))
            .map_err(|error| {
                failure(
                    error.code(),
                    LedgerLocation::ValidatePersistedEvent.location(),
                )
            })?;
        event.apply_artifact_evictions(evictions);
        events.push(event);
    }
    Ok(RangeRead {
        events,
        head_sequence: head,
    })
}

/// LN-5 (#422, LEAN-L §3.3): the index the history reads of artifact identities require
/// (`ledger_artifacts` by artifact id); a database without it gets it once, at the first writer
/// open of a build that requires it.
const HISTORY_INDEX: &str = "CREATE INDEX IF NOT EXISTS ledger_artifacts_artifact ON ledger_artifacts(artifact_id, sequence)";

fn ensure_history_index(database: &RuntimeDatabase) -> GlobalLedgerResult<()> {
    let connection = database.connection(LedgerLocation::UpgradeLedgerViews.location())?;
    connection
        .execute_batch(HISTORY_INDEX)
        .map_err(|error| sql_error(error, LedgerLocation::CreateLedgerViews.location()))
}

/// The JSON path of a policy dispatch or execution record's decision in its stored record.
const DECISION_PATH: &str = "$.payload.payload.data.decision_id";

/// LN-5 (#422, LEAN-L §3.3): one page of the committed rows `selection` selects with
/// `after < sequence <= through`, at most `rows`, in ledger order. The statement selects through
/// the indexes (the sequence key; the event type, origin, time and severity indexes; the link
/// indexes; a decision through the stored record); each row is then authenticated on its own
/// (`RowReader::authenticate`), and its link and artifact rows must be the ones it requires.
fn read_history_rows(
    database: &RuntimeDatabase,
    connection: &Connection,
    (selection, view): (&HistorySelection<'_>, Option<&ViewCondition>),
    range: (u64, u64),
    rows: usize,
) -> GlobalLedgerResult<Vec<LedgerEventMetadata>> {
    let (sql, values) = history_statement(selection, view, range, rows)?;
    let mut reader = RowReader::new(database, connection, None);
    let selected = read_rows_bound(connection, &sql, &values, None, &mut reader.bytes, true)?;
    let mut sequences = Vec::with_capacity(selected.len());
    let mut events = Vec::with_capacity(selected.len());
    let mut expected_links = Vec::with_capacity(selected.len());
    let mut expected_artifacts = Vec::new();
    for row in selected {
        let Some(sequence) = row.first().cloned() else {
            return Err(failure(
                LedgerCode::InvalidEventInteger.code(),
                LedgerLocation::RecoverSqliteSequence.location(),
            ));
        };
        let (event, links, artifacts) = reader.authenticate(row, events.len() + 1)?;
        sequences.push(sequence);
        expected_links.push(links);
        expected_artifacts.extend(artifacts);
        events.push(event);
    }
    if sequences.is_empty() {
        return Ok(events);
    }
    let placeholders = vec!["?"; sequences.len()].join(",");
    let stored_links = read_rows_bound(
        connection,
        &format!(
            "SELECT {LINK_COLUMNS} FROM ledger_links WHERE sequence IN ({placeholders}) ORDER BY sequence"
        ),
        &sequences,
        None,
        &mut reader.bytes,
        false,
    )?;
    let stored_artifacts = read_rows_bound(
        connection,
        &format!(
            "SELECT {ARTIFACT_COLUMNS} FROM ledger_artifacts WHERE sequence IN ({placeholders}) ORDER BY sequence,ordinal"
        ),
        &sequences,
        None,
        &mut reader.bytes,
        false,
    )?;
    if stored_links != expected_links || stored_artifacts != expected_artifacts {
        return Err(failure(
            LedgerCode::LedgerIndexMismatch.code(),
            LedgerLocation::VerifySqliteRelations.location(),
        ));
    }
    Ok(events)
}

/// LN-6 (#422, LEAN-L §3.3): a view's condition on a history page: its SQL over the event row
/// `e` (and its link row `l` when `links`), with its bound values in placeholder order.
struct ViewCondition {
    clause: String,
    values: Vec<SqlValue>,
    links: bool,
}

impl ViewCondition {
    /// The condition of `view`: its stored view's predicate (`views::predicate`); for the Lab
    /// view, a Lab anchor or a record whose request, correlation or run is among `lab`'s.
    fn of(view: LedgerView, lab: &LabRelations) -> GlobalLedgerResult<Self> {
        if let Some(predicate) = views::predicate(view)? {
            return Ok(Self {
                clause: format!("({predicate})"),
                values: Vec::new(),
                links: false,
            });
        }
        let keys = |values: Vec<String>| {
            serde_json::to_string(&values)
                .map(SqlValue::Text)
                .map_err(|error| {
                    GlobalLedgerError::json(
                        LedgerCode::EventSerializationFailed.code(),
                        LedgerLocation::EncodeLedgerQuery.location(),
                        &error,
                    )
                })
        };
        Ok(Self {
            clause: "(e.origin_source=? OR e.event_type=? OR l.request_id IN (SELECT value FROM json_each(?)) OR l.correlation_id IN (SELECT value FROM json_each(?)) OR l.run_id IN (SELECT value FROM json_each(?)))".to_owned(),
            values: vec![
                SqlValue::Text(views::key(LAB_ANCHOR_SOURCE)?),
                SqlValue::Text(views::key(LAB_ANCHOR_TYPE)?),
                keys(lab.requests.iter().map(views::key).collect::<GlobalLedgerResult<_>>()?)?,
                keys(
                    lab.correlations
                        .iter()
                        .map(views::key)
                        .collect::<GlobalLedgerResult<_>>()?,
                )?,
                keys(lab.runs.iter().map(views::key).collect::<GlobalLedgerResult<_>>()?)?,
            ],
            links: true,
        })
    }
}

/// The statement of one history page and its bound values: the query's conditions that have a
/// column or an index, and `view`'s, every value bound, in placeholder order.
fn history_statement(
    selection: &HistorySelection<'_>,
    view: Option<&ViewCondition>,
    (after, through): (u64, u64),
    rows: usize,
) -> GlobalLedgerResult<(String, Vec<SqlValue>)> {
    let query = selection.query;
    let columns = EVENT_COLUMNS
        .split(',')
        .map(|column| format!("e.{column}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut clauses = vec!["e.sequence>?".to_owned(), "e.sequence<=?".to_owned()];
    let mut values = vec![
        SqlValue::Integer(encode(after)),
        SqlValue::Integer(encode(through)),
    ];
    for (clause, value) in [
        ("e.sequence>=?", query.from_sequence),
        ("e.sequence<=?", query.to_sequence),
        ("e.timestamp_unix_ms>=?", query.from_timestamp_unix_ms),
        ("e.timestamp_unix_ms<?", query.to_timestamp_unix_ms),
    ] {
        if let Some(value) = value {
            clauses.push(clause.to_owned());
            values.push(SqlValue::Integer(encode(value)));
        }
    }
    if let Some(event_type) = query.event_type {
        clauses.push("e.event_type=?".to_owned());
        values.push(SqlValue::Text(views::key(event_type)?));
    }
    for (event_types, operator) in [
        (&query.event_types, "IN"),
        (&query.exclude_event_types, "NOT IN"),
    ] {
        if event_types.is_empty() {
            continue;
        }
        clauses.push(format!(
            "e.event_type {operator} ({})",
            vec!["?"; event_types.len()].join(",")
        ));
        for event_type in event_types {
            values.push(SqlValue::Text(views::key(event_type)?));
        }
    }
    if let Some(source) = query.source {
        clauses.push("e.origin_source=?".to_owned());
        values.push(SqlValue::Text(views::key(source)?));
    }
    if let Some(module) = query.origin_module {
        clauses.push("e.origin_module=?".to_owned());
        values.push(SqlValue::Text(views::key(module)?));
    }
    if query.minimum_severity.is_some() || query.maximum_severity.is_some() {
        let allowed = views::severities(query.minimum_severity, query.maximum_severity)?;
        clauses.push(format!(
            "e.severity IN ({})",
            vec!["?"; allowed.len()].join(",")
        ));
        values.extend(allowed);
    }
    let mut links = false;
    for (column, value) in [
        ("l.instance_id", query.instance_id.as_ref().map(views::key)),
        ("l.request_id", query.request_id.as_ref().map(views::key)),
        (
            "l.correlation_id",
            query.correlation_id.as_ref().map(views::key),
        ),
        (
            "l.causation_id",
            query.causation_id.as_ref().map(views::key),
        ),
        ("l.task_id", query.task_id.as_ref().map(views::key)),
        ("l.run_id", query.run_id.as_ref().map(views::key)),
        ("l.lease_id", query.lease_id.as_ref().map(views::key)),
        ("l.frame_id", query.frame_id.as_ref().map(views::key)),
        ("l.action_id", query.action_id.as_ref().map(views::key)),
        (
            "l.recognition_id",
            query.recognition_id.as_ref().map(views::key),
        ),
    ] {
        if let Some(value) = value.transpose()? {
            links = true;
            clauses.push(format!("{column}=?"));
            values.push(SqlValue::Text(value));
        }
    }
    if !query.instance_ids.is_empty() {
        links = true;
        clauses.push(format!(
            "l.instance_id IN ({})",
            vec!["?"; query.instance_ids.len()].join(",")
        ));
        for instance_id in &query.instance_ids {
            values.push(SqlValue::Text(views::key(instance_id)?));
        }
    }
    if let Some(decision) = selection.decision_id {
        clauses.push(format!(
            "json_extract(CAST(e.canonical_record AS TEXT),'{DECISION_PATH}')=?"
        ));
        values.push(SqlValue::Text(decision.to_owned()));
    }
    if let Some(view) = view {
        links |= view.links;
        clauses.push(view.clause.clone());
        values.extend(view.values.iter().cloned());
    }
    values.push(SqlValue::Integer(i64::try_from(rows).map_err(|_| {
        failure(
            LedgerCode::InvalidPageLimit.code(),
            LedgerLocation::EncodeLedgerQuery.location(),
        )
    })?));
    let join = if links {
        " JOIN ledger_links l ON l.sequence=e.sequence"
    } else {
        ""
    };
    Ok((
        format!(
            "SELECT {columns} FROM ledger_events e{join} WHERE {} ORDER BY e.sequence LIMIT ?",
            clauses.join(" AND ")
        ),
        values,
    ))
}

/// LN-5 (#422, LEAN-L §3.3): the Lab relations of the committed rows through `through`
/// (`LAB_RELATIONS`), selected through the view and link indexes as the stored Lab view selects
/// them: the requests and correlations of the Lab anchors, with `seed`'s; then the runs of the
/// rows that name one of them, with `seed`'s.
fn read_lab_relations(
    connection: &Connection,
    through: u64,
    seed: &LabRelations,
) -> GlobalLedgerResult<LabRelations> {
    let mut relations = seed.clone();
    let anchors = [
        SqlValue::Text(views::key(LAB_ANCHOR_SOURCE)?),
        SqlValue::Text(views::key(LAB_ANCHOR_TYPE)?),
        SqlValue::Integer(encode(through)),
    ];
    let anchored = |column: &str| {
        format!(
            "SELECT al.{column} FROM ledger_events a JOIN ledger_links al ON al.sequence=a.sequence WHERE a.origin_source=?1 AND a.sequence<=?3 AND al.{column} IS NOT NULL UNION SELECT al.{column} FROM ledger_events a JOIN ledger_links al ON al.sequence=a.sequence WHERE a.event_type=?2 AND a.sequence<=?3 AND al.{column} IS NOT NULL"
        )
    };
    for text in link_texts(read_rows_bound(
        connection,
        &anchored("request_id"),
        &anchors,
        None,
        &mut 0,
        false,
    )?)? {
        relations.requests.insert(id_from_text(&text)?);
    }
    for text in link_texts(read_rows_bound(
        connection,
        &anchored("correlation_id"),
        &anchors,
        None,
        &mut 0,
        false,
    )?)? {
        relations.correlations.insert(id_from_text(&text)?);
    }
    let requests = relations
        .requests
        .iter()
        .map(views::key)
        .collect::<GlobalLedgerResult<Vec<_>>>()?;
    let correlations = relations
        .correlations
        .iter()
        .map(views::key)
        .collect::<GlobalLedgerResult<Vec<_>>>()?;
    let encoded = |values: &[String]| {
        serde_json::to_string(values).map_err(|error| {
            GlobalLedgerError::json(
                LedgerCode::EventSerializationFailed.code(),
                LedgerLocation::EncodeLedgerQuery.location(),
                &error,
            )
        })
    };
    let runs = read_rows_bound(
        connection,
        "SELECT r.run_id FROM ledger_links r WHERE r.request_id IN (SELECT value FROM json_each(?2)) AND r.run_id IS NOT NULL AND r.sequence<=?1 UNION SELECT r.run_id FROM ledger_links r WHERE r.correlation_id IN (SELECT value FROM json_each(?3)) AND r.run_id IS NOT NULL AND r.sequence<=?1",
        &[
            SqlValue::Integer(encode(through)),
            SqlValue::Text(encoded(requests.as_slice())?),
            SqlValue::Text(encoded(correlations.as_slice())?),
        ],
        None,
        &mut 0,
        false,
    )?;
    for text in link_texts(runs)? {
        relations.runs.insert(id_from_text(&text)?);
    }
    Ok(relations)
}

/// The text of each one-column row.
fn link_texts(rows: Vec<SqlRow>) -> GlobalLedgerResult<Vec<String>> {
    rows.into_iter()
        .map(|row| match <[SqlValue; 1]>::try_from(row) {
            Ok([SqlValue::Text(text)]) => Ok(text),
            _ => Err(failure(
                LedgerCode::InvalidEventLink.code(),
                LedgerLocation::ReadLedgerView.location(),
            )),
        })
        .collect()
}

/// An identifier from its stored text (its serialization).
fn id_from_text<T: serde::de::DeserializeOwned>(text: &str) -> GlobalLedgerResult<T> {
    serde_json::from_value(Value::String(text.to_owned())).map_err(|_| {
        failure(
            LedgerCode::InvalidEventLink.code(),
            LedgerLocation::ReadLedgerView.location(),
        )
    })
}

/// The stored sequences are exactly `1..=head`: `sequence` is the integer primary key, so
/// its values are distinct, and the encoding preserves order. Answered from an index.
fn verify_sequence_extent(
    connection: &Connection,
    head: u64,
    budget: ReadBudget,
    bytes: &mut u64,
) -> GlobalLedgerResult<()> {
    let rows = read_rows(
        connection,
        "SELECT COUNT(*),MIN(sequence),MAX(sequence) FROM ledger_events",
        budget,
        bytes,
        false,
    )?;
    let (count, first, last) = match rows.as_slice() {
        [row] => match row.as_slice() {
            [SqlValue::Integer(count), SqlValue::Null, SqlValue::Null] => (*count, None, None),
            [
                SqlValue::Integer(count),
                SqlValue::Integer(first),
                SqlValue::Integer(last),
            ] => (*count, Some(decode(*first)), Some(decode(*last))),
            _ => {
                return Err(failure(
                    LedgerCode::SequenceDiscontinuity.code(),
                    LedgerLocation::RecoverSequence.location(),
                ));
            }
        },
        _ => {
            return Err(failure(
                LedgerCode::SequenceDiscontinuity.code(),
                LedgerLocation::RecoverSequence.location(),
            ));
        }
    };
    // Rows past the meta head, as the full read reports them.
    if last.is_some_and(|last| last > head) || (head == 0 && count != 0) {
        return Err(failure(
            LedgerCode::LedgerMetaMismatch.code(),
            LedgerLocation::VerifySqliteHead.location(),
        ));
    }
    if u64::try_from(count) != Ok(head) || (head != 0 && (first, last) != (Some(1), Some(head))) {
        return Err(failure(
            LedgerCode::SequenceDiscontinuity.code(),
            LedgerLocation::RecoverSequence.location(),
        ));
    }
    Ok(())
}

/// One stored record hash, read by its sequence.
fn stored_record_hash(
    connection: &Connection,
    sequence: u64,
    budget: ReadBudget,
    bytes: &mut u64,
) -> GlobalLedgerResult<String> {
    let rows = read_rows_bound(
        connection,
        "SELECT record_sha256 FROM ledger_events WHERE sequence=?",
        &[SqlValue::Integer(encode(sequence))],
        budget,
        bytes,
        false,
    )?;
    match rows.as_slice() {
        [row] => match row.as_slice() {
            [SqlValue::Text(hash)] => Ok(hash.clone()),
            _ => Err(failure(
                LedgerCode::LedgerRecordMismatch.code(),
                LedgerLocation::VerifySqliteRecord.location(),
            )),
        },
        _ => Err(failure(
            LedgerCode::SequenceDiscontinuity.code(),
            LedgerLocation::RecoverSequence.location(),
        )),
    }
}

fn read_snapshot_connection(
    connection: &Connection,
    budget: ReadBudget,
) -> GlobalLedgerResult<RawSnapshot> {
    refuse_unsupported_format(connection)?;
    views::installed(connection)?;
    let mut bytes = 0;
    let meta = read_meta_with_budget(connection, budget, &mut bytes)?;
    let events = read_rows(
        connection,
        &format!("SELECT {EVENT_COLUMNS} FROM ledger_events ORDER BY sequence"),
        budget,
        &mut bytes,
        true,
    )?;
    let links = read_rows(
        connection,
        &format!("SELECT {LINK_COLUMNS} FROM ledger_links ORDER BY sequence"),
        budget,
        &mut bytes,
        false,
    )?;
    let artifacts = read_rows(
        connection,
        &format!("SELECT {ARTIFACT_COLUMNS} FROM ledger_artifacts ORDER BY sequence,ordinal"),
        budget,
        &mut bytes,
        false,
    )?;
    Ok(RawSnapshot {
        format_version: format_version(connection)?,
        events,
        links,
        artifacts,
        meta,
        budget,
        bytes,
        bound: None,
    })
}

fn read_rows(
    connection: &Connection,
    sql: &str,
    budget: ReadBudget,
    bytes: &mut u64,
    count_events: bool,
) -> GlobalLedgerResult<Vec<SqlRow>> {
    read_rows_bound(connection, sql, &[], budget, bytes, count_events)
}

/// `read_rows` with bound parameter values; values are never interpolated into the SQL.
fn read_rows_bound(
    connection: &Connection,
    sql: &str,
    parameters: &[SqlValue],
    budget: ReadBudget,
    bytes: &mut u64,
    count_events: bool,
) -> GlobalLedgerResult<Vec<SqlRow>> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| sql_error(error, LedgerLocation::PrepareSqliteRead.location()))?;
    let columns = statement.column_count();
    let mut rows = statement
        .query(params_from_iter(parameters.iter()))
        .map_err(|error| sql_error(error, LedgerLocation::QuerySqliteRows.location()))?;
    let mut result = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|error| sql_error(error, LedgerLocation::ReadSqliteRow.location()))?
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
                .map_err(|error| sql_error(error, LedgerLocation::DecodeSqliteColumn.location()))?;
            let length = match value {
                ValueRef::Null => 0,
                ValueRef::Integer(_) | ValueRef::Real(_) => 8,
                ValueRef::Text(value) | ValueRef::Blob(value) => value.len() as u64,
            };
            *bytes = bytes.checked_add(length).ok_or_else(|| {
                failure(
                    LedgerCode::LedgerSnapshotOverflow.code(),
                    LedgerLocation::CountSqliteBytes.location(),
                )
            })?;
            check_read_budget(budget, *bytes, 0)?;
            values.push(SqlValue::try_from(value).map_err(|_| {
                failure(
                    LedgerCode::InvalidSqliteValue.code(),
                    LedgerLocation::DecodeSqliteColumn.location(),
                )
            })?);
        }
        // Workflow #382 L-3 (LED-I5): counts the row on this thread while a counter runs.
        #[cfg(test)]
        crate::read_count::row_read(sql, &values);
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
    let has_marker = ledger_meta_has_marker(connection)?;
    read_meta_columns(connection, has_marker, budget, bytes)
}

/// Whether `ledger_meta` has the `migration_record` column.
fn ledger_meta_has_marker(connection: &Connection) -> GlobalLedgerResult<bool> {
    connection
        .prepare("PRAGMA table_info(ledger_meta)")
        .and_then(|mut statement| {
            let columns = statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(columns.iter().any(|column| column == "migration_record"))
        })
        .map_err(|error| sql_error(error, LedgerLocation::InspectLedgerMarkerSchema.location()))
}

/// Reads the single meta row with the given column set; a missing marker reads as Null.
fn read_meta_columns(
    connection: &Connection,
    has_marker: bool,
    budget: ReadBudget,
    bytes: &mut u64,
) -> GlobalLedgerResult<SqlRow> {
    let columns = if has_marker {
        META_COLUMNS
    } else {
        META_COLUMNS.trim_end_matches(",migration_record")
    };
    let mut rows = read_rows(
        connection,
        &format!("SELECT {columns} FROM ledger_meta ORDER BY singleton"),
        budget,
        bytes,
        false,
    )?;
    if rows.len() != 1 {
        return Err(failure(
            LedgerCode::LedgerMetaMissing.code(),
            LedgerLocation::ReadSqliteMeta.location(),
        ));
    }
    let mut row = rows.remove(0);
    if !has_marker {
        row.push(SqlValue::Null);
    }
    Ok(row)
}

fn verify_snapshot<F>(
    database: &RuntimeDatabase,
    raw: RawSnapshot,
    verifier: &mut Option<F>,
) -> GlobalLedgerResult<(Vec<PersistedEvent>, Option<String>)>
where
    F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
{
    let budget = raw.budget;
    let bytes = raw.bytes;
    // Workflow #382 L-3 (LED-I5): counts the material files the verifier is asked to open.
    #[cfg(test)]
    let verifier = &mut crate::read_count::counting_material(verifier);
    let VerifiedSnapshotRecords {
        records,
        head_hash: hash,
        ..
    } = verify_snapshot_records(database, raw)?;
    let events = super::retention::restore_records(records, verifier, |count| {
        check_read_budget(budget, bytes, count)
    })?;
    Ok((events, hash))
}

/// Workflow #375 R5a: `verify_snapshot` for a formal opening, which reads no artifact
/// material: a reference without an eviction proof stays `Unrecorded`. The retention index
/// is derived from the metadata authenticated with the records. `timing` keeps the phases that
/// ran, also when an error is returned.
fn verify_snapshot_unread(
    database: &RuntimeDatabase,
    raw: RawSnapshot,
    timing: &mut LedgerOpenTiming,
) -> GlobalLedgerResult<(Vec<PersistedEvent>, Option<String>)> {
    let budget = raw.budget;
    let bytes = raw.bytes;
    let mut check = move |count: usize| check_read_budget(budget, bytes, count);
    let started = Instant::now();
    let verified = verify_snapshot_records(database, raw).and_then(|verified| {
        let retention =
            super::retention::RetentionIndex::from_events_checked(&verified.metadata, &mut check)?;
        Ok((verified.records, verified.head_hash, retention))
    });
    timing.verify_ms = elapsed_ms(started);
    let (records, hash, retention) = verified?;
    let events = super::retention::restore_records_unread(&retention, records, check, timing)?;
    Ok((events, hash))
}

/// Returned only after complete row, relation, head and marker authentication.
/// A query still checks its requested prefix before consuming the retained metadata.
struct VerifiedSnapshotRecords {
    records: Vec<StoredEventRecord>,
    metadata: Vec<LedgerEventMetadata>,
    head_hash: Option<String>,
}

/// Authenticates the complete ledger snapshot without opening referenced material; for a
/// prefix read (Workflow #363), the complete prefix and the already authenticated head row.
fn verify_snapshot_records(
    database: &RuntimeDatabase,
    raw: RawSnapshot,
) -> GlobalLedgerResult<VerifiedSnapshotRecords> {
    let marker = SqliteMarker::parse(&raw.meta)?;
    let expected_format = if marker.state == "ready" {
        FORMAL_FORMAT_VERSION
    } else {
        0
    };
    if raw.format_version != expected_format {
        return Err(failure(
            LedgerCode::LedgerFormatMarkerMismatch.code(),
            LedgerLocation::VerifyDatabaseFormat.location(),
        ));
    }
    // The migrated prefix identity over the stored bytes, before the rows are consumed. It
    // is compared only after every row, relation and the meta row are authenticated; a
    // non-Blob row stops the walk and is reported by the row checks below.
    let prefix = marker.migration.as_ref().map(|record| {
        let mut digest = super::migration::PrefixDigest::new();
        for row in &raw.events {
            if digest.count() == record.source_event_count {
                break;
            }
            let Some(SqlValue::Blob(bytes)) = row.get(10) else {
                break;
            };
            digest.push(bytes);
        }
        digest.finish()
    });
    let mut ids = BTreeSet::new();
    let mut next = 1;
    let mut head_hash: Option<String> = None;
    let VerifiedRows {
        records: events,
        metadata,
        links: expected_links,
        artifacts: expected_artifacts,
    } = verify_event_rows(
        database,
        raw.events,
        raw.budget,
        raw.bytes,
        &mut next,
        &mut head_hash,
        &mut ids,
    )?;
    if raw.links != expected_links || raw.artifacts != expected_artifacts {
        return Err(failure(
            LedgerCode::LedgerIndexMismatch.code(),
            LedgerLocation::VerifySqliteRelations.location(),
        ));
    }
    let last = events.last().map_or(0, StoredEventRecord::sequence);
    match raw.bound {
        // Workflow #363: a prefix read authenticates every row through its bound.
        Some(bound) if last != bound.through => {
            return Err(failure(
                LedgerCode::LedgerPrefixIncomplete.code(),
                LedgerLocation::VerifySqlitePrefix.location(),
            ));
        }
        // The keyed head row, authenticated in this read transaction before any history row,
        // records a head after the prefix; the later rows were not read.
        Some(bound) if bound.through < bound.head => {}
        // A full read, or a prefix at the head: the meta row must equal the head computed
        // from the rows.
        _ => {
            if raw.meta != meta_row_with_marker(database, next, last, head_hash.as_deref(), &marker)
            {
                return Err(failure(
                    LedgerCode::LedgerMetaMismatch.code(),
                    LedgerLocation::VerifySqliteHead.location(),
                ));
            }
        }
    }
    marker.verify_records(&events, prefix.as_ref())?;
    Ok(VerifiedSnapshotRecords {
        records: events,
        metadata,
        head_hash,
    })
}

/// Rows authenticated as one continuation of a hash chain, with the link and artifact
/// rows their records project.
struct VerifiedRows {
    records: Vec<StoredEventRecord>,
    metadata: Vec<LedgerEventMetadata>,
    links: Vec<SqlRow>,
    artifacts: Vec<SqlRow>,
}

/// The per-row checks shared by full and tail verification: record schema, sequence
/// continuity from `next`, unique event ids, and each row equal to its projection over
/// the stored bytes chained from `head_hash`. Budget event counts are those of `rows`.
fn verify_event_rows(
    database: &RuntimeDatabase,
    rows: Vec<SqlRow>,
    budget: ReadBudget,
    read_bytes: u64,
    next: &mut u64,
    head_hash: &mut Option<String>,
    ids: &mut BTreeSet<EventId>,
) -> GlobalLedgerResult<VerifiedRows> {
    let mut events = Vec::with_capacity(rows.len());
    let mut metadata = Vec::with_capacity(rows.len());
    let mut expected_links = Vec::new();
    let mut expected_artifacts = Vec::new();
    for row in rows {
        check_read_budget(budget, read_bytes, events.len() + 1)?;
        let Some(SqlValue::Blob(bytes)) = row.get(10) else {
            return Err(failure(
                LedgerCode::CorruptLedgerRecord.code(),
                LedgerLocation::ReadCanonicalRecord.location(),
            ));
        };
        let value = serde_json::from_slice::<UniqueJsonValue>(bytes)
            .map_err(|error| {
                GlobalLedgerError::json(
                    LedgerCode::CorruptLedgerRecord.code(),
                    LedgerLocation::ParseSqliteRecord.location(),
                    &error,
                )
            })?
            .0;
        if value.get("schema_version").and_then(Value::as_str) != Some(GLOBAL_EVENT_SCHEMA_VERSION)
        {
            return Err(failure(
                LedgerCode::UnsupportedEventSchema.code(),
                LedgerLocation::RecoverEventSchema.location(),
            ));
        }
        let stored: StoredEventRecord = serde_json::from_value(value).map_err(|error| {
            GlobalLedgerError::json(
                LedgerCode::CorruptLedgerRecord.code(),
                LedgerLocation::DecodeSqliteRecord.location(),
                &error,
            )
        })?;
        let event = stored.clone().into_metadata().map_err(|error| {
            failure(
                error.code(),
                LedgerLocation::ValidatePersistedEvent.location(),
            )
        })?;
        let Some(SqlValue::Integer(stored_sequence)) = row.first() else {
            return Err(failure(
                LedgerCode::InvalidEventInteger.code(),
                LedgerLocation::RecoverSqliteSequence.location(),
            ));
        };
        if decode(*stored_sequence) != event.sequence() {
            return Err(failure(
                LedgerCode::LedgerRecordMismatch.code(),
                LedgerLocation::RecoverSqliteSequence.location(),
            ));
        }
        if event.sequence() != *next {
            return Err(failure(
                LedgerCode::SequenceDiscontinuity.code(),
                LedgerLocation::RecoverSequence.location(),
            ));
        }
        if !ids.insert(*event.event_id()) {
            return Err(failure(
                LedgerCode::DuplicateEventId.code(),
                LedgerLocation::RecoverEventIds.location(),
            ));
        }
        let projected =
            project_stored_bytes(database, &stored, bytes.clone(), head_hash.as_deref())?;
        if row != projected.event {
            return Err(failure(
                LedgerCode::LedgerRecordMismatch.code(),
                LedgerLocation::VerifySqliteRecord.location(),
            ));
        }
        check_read_budget(budget, read_bytes, events.len() + 1)?;
        expected_links.push(projected.links);
        expected_artifacts.extend(projected.artifacts);
        *head_hash = Some(projected.hash);
        events.push(stored);
        metadata.push(event);
        *next = increment_sequence(*next)?;
    }
    Ok(VerifiedRows {
        records: events,
        metadata,
        links: expected_links,
        artifacts: expected_artifacts,
    })
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
    project_stored_record(database, &StoredEventRecord::from_event(event), previous)
}

/// Write side: the canonical bytes are the complete stored record's serialization.
fn project_stored_record(
    database: &RuntimeDatabase,
    stored: &StoredEventRecord,
    previous: Option<&str>,
) -> GlobalLedgerResult<ProjectedRecord> {
    let bytes = serde_json::to_vec(stored).map_err(|error| {
        GlobalLedgerError::json(
            LedgerCode::EventSerializationFailed.code(),
            LedgerLocation::SerializeEvent.location(),
            &error,
        )
    })?;
    project_stored_bytes(database, stored, bytes, previous)
}

/// Verification side: hash and tag over the bytes as stored, index columns from the
/// decoded record. The record is not re-serialized, so an additive `serde(default)` field
/// leaves an existing row verifiable.
fn project_stored_bytes(
    database: &RuntimeDatabase,
    stored: &StoredEventRecord,
    bytes: Vec<u8>,
    previous: Option<&str>,
) -> GlobalLedgerResult<ProjectedRecord> {
    let value = serde_json::to_value(stored.index_fields()).map_err(|error| {
        GlobalLedgerError::json(
            LedgerCode::EventSerializationFailed.code(),
            LedgerLocation::ProjectSqliteRecord.location(),
            &error,
        )
    })?;
    let hash = format!("sha256:{:x}", Sha256::digest(&bytes));
    let tag = database.integrity_tag(
        "ledger-event-v1",
        &[&bytes, hash.as_bytes(), previous.unwrap_or("").as_bytes()],
    );
    let sequence = SqlValue::Integer(encode(stored.sequence()));
    let row = vec![
        sequence.clone(),
        text(&value, "event_id")?,
        SqlValue::Integer(encode(stored.timestamp_unix_ms())),
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
        return Err(failure(
            LedgerCode::InvalidArtifactRecord.code(),
            LedgerLocation::ProjectSqliteRecord.location(),
        ));
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
    meta_row_with_marker(database, next, head, hash, &SqliteMarker::candidate())
}
fn meta_row_with_marker(
    database: &RuntimeDatabase,
    next: u64,
    head: u64,
    hash: Option<&str>,
    marker: &SqliteMarker,
) -> SqlRow {
    let next_bytes = next.to_be_bytes();
    let head_bytes = head.to_be_bytes();
    let mut fields: Vec<&[u8]> = vec![
        SCHEMA.as_bytes(),
        &next_bytes,
        &head_bytes,
        hash.unwrap_or("").as_bytes(),
        b"sqlite",
        marker.state.as_bytes(),
        INTEGER_ENCODING.as_bytes(),
    ];
    if marker.state != "candidate" {
        fields.push(
            marker
                .migration
                .as_ref()
                .map_or("", |record| record.migration_id.as_str())
                .as_bytes(),
        );
        fields.push(marker.material.as_deref().unwrap_or("").as_bytes());
    }
    let tag = database.integrity_tag("ledger-meta-v1", &fields);
    vec![
        SqlValue::Integer(1),
        SqlValue::Text(SCHEMA.into()),
        SqlValue::Integer(encode(next)),
        SqlValue::Integer(encode(head)),
        optional_text(hash),
        SqlValue::Text("sqlite".into()),
        optional_text(
            marker
                .migration
                .as_ref()
                .map(|record| record.migration_id.as_str()),
        ),
        SqlValue::Text(marker.state.into()),
        SqlValue::Text(INTEGER_ENCODING.into()),
        SqlValue::Text(tag),
        optional_text(marker.material.as_deref()),
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
        .map_err(|error| sql_error(error, LedgerLocation::InsertSqliteRecord.location()))?;
    if changed != 1 {
        return Err(failure(
            LedgerCode::LedgerInsertIncomplete.code(),
            LedgerLocation::InsertSqliteRecord.location(),
        ));
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
        .ok_or_else(|| {
            failure(
                LedgerCode::InvalidEventColumn.code(),
                LedgerLocation::ProjectSqliteRecord.location(),
            )
        })
}
fn nullable_text(value: &Value, key: &str) -> GlobalLedgerResult<SqlValue> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(SqlValue::Null),
        Some(Value::String(value)) => Ok(SqlValue::Text(value.clone())),
        _ => Err(failure(
            LedgerCode::InvalidEventLink.code(),
            LedgerLocation::ProjectSqliteRecord.location(),
        )),
    }
}
fn integer(value: &Value, key: &str) -> GlobalLedgerResult<SqlValue> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| SqlValue::Integer(encode(value)))
        .ok_or_else(|| {
            failure(
                LedgerCode::InvalidEventInteger.code(),
                LedgerLocation::ProjectSqliteRecord.location(),
            )
        })
}
fn failure(code: Code, operation: Location) -> GlobalLedgerError {
    GlobalLedgerError::fatal(code, operation)
}
fn sql_error(error: rusqlite::Error, operation: Location) -> GlobalLedgerError {
    let (detail, write_failure) = match error {
        rusqlite::Error::SqliteFailure(code, _) => (
            Some(format!("SQLite extended code {}", code.extended_code)),
            write_failure_kind(code.code),
        ),
        _ => (None, None),
    };
    GlobalLedgerError {
        code: LedgerCode::LedgerSqliteFailed.as_str(),
        operation: operation.as_str(),
        detail,
        terminal: true,
        rolled_back_work: None,
        io_kind: None,
        write_failure,
        values: None,
    }
}

/// Workflow #383 AO-1 (`contracts/ledger-observer.md` §9.1): the SQLite results that are write
/// failures (I/O, busy, a full disk), read back and tried again, with the I/O kind they report.
/// Every other result (a constraint, a corrupt image, a misuse) is not tried again.
fn write_failure_kind(code: rusqlite::ErrorCode) -> Option<IoKind> {
    match code {
        rusqlite::ErrorCode::DiskFull => Some(IoKind::StorageFull),
        rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
            Some(IoKind::ResourceBusy)
        }
        rusqlite::ErrorCode::ReadOnly => Some(IoKind::ReadOnlyFilesystem),
        rusqlite::ErrorCode::PermissionDenied => Some(IoKind::PermissionDenied),
        rusqlite::ErrorCode::OutOfMemory => Some(IoKind::OutOfMemory),
        rusqlite::ErrorCode::OperationInterrupted => Some(IoKind::Interrupted),
        rusqlite::ErrorCode::SystemIoFailure
        | rusqlite::ErrorCode::CannotOpen
        | rusqlite::ErrorCode::FileLockingProtocolFailed => Some(IoKind::Other),
        _ => None,
    }
}

#[cfg(test)]
#[path = "sqlite/tests.rs"]
mod tests;
