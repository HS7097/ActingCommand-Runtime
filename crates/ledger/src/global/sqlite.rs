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
use crate::{
    PersistedEvent,
    fact::{LedgerEventMetadata, LedgerEventRead, StoredEventRecord},
};
use actingcommand_contract::{
    EventQuery, GLOBAL_EVENT_SCHEMA_VERSION, LedgerMaterialReadState, LedgerReadScope,
    LedgerReadSource, ProjectedArtifactReference, ProjectionProfile, RecoveryReason,
    RuntimeEventQueryPage, RuntimeEventQueryPageRequest, VerifiedArtifactReference,
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

mod views;

const FORMAL_FORMAT_VERSION: i64 = 1;
const SCHEMA: &str = "actingcommand.sqlite-ledger.v1";
const INTEGER_ENCODING: &str = "ordered-u64-v1";
const EVENT_COLUMNS: &str = "sequence,event_id,timestamp_unix_ms,event_type,severity,sensitivity,origin_source,origin_module,origin_actor,payload_schema,canonical_record,record_sha256,previous_record_sha256,integrity_tag";
const LINK_COLUMNS: &str = "sequence,instance_id,request_id,correlation_id,causation_id,task_id,run_id,lease_id,frame_id,action_id,recognition_id";
const ARTIFACT_COLUMNS: &str = "sequence,ordinal,artifact_id,kind,run_id,frame_id,correlation_id,object_key,media_type,byte_count,sha256,created_at_unix_ms,producer,retention_class,redaction_state";
const META_COLUMNS: &str = "singleton,schema_version,next_sequence,head_sequence,head_record_sha256,storage_backend,migration_id,cutover_state,integer_encoding,integrity_tag,migration_record";
type SqlRow = Vec<SqlValue>;
type ReadBudget = Option<(u64, usize, Instant)>;

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
        record
            .validate()
            .map_err(|error| failure(error.code(), "validate_cutover_marker"))?;
        Ok(Self {
            state: "ready",
            migration: Some(Box::new(record.clone())),
            material: Some(serde_json::to_string(record).map_err(|error| {
                GlobalLedgerError::json("invalid_cutover_marker", "encode_cutover_marker", &error)
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
                    GlobalLedgerError::json("invalid_cutover_marker", "read_cutover_marker", &error)
                })?;
                let record = serde_json::from_value(unique.0).map_err(|error| {
                    GlobalLedgerError::json(
                        "invalid_cutover_marker",
                        "decode_cutover_marker",
                        &error,
                    )
                })?;
                let marker = Self::migrated(&record)?;
                if marker.material.as_ref() != Some(material) {
                    return Err(failure(
                        "invalid_cutover_marker",
                        "verify_cutover_marker_bytes",
                    ));
                }
                marker
            }
            _ => return Err(failure("invalid_cutover_marker", "read_cutover_marker")),
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
                "migration_identity_mismatch",
                "read_cutover_marker",
            ));
        }
        Ok(marker)
    }
    fn verify_events(&self, events: &[PersistedEvent]) -> GlobalLedgerResult<()> {
        self.verify_records(
            &events
                .iter()
                .map(StoredEventRecord::from_event)
                .collect::<Vec<_>>(),
        )
    }
    fn verify_records(&self, events: &[StoredEventRecord]) -> GlobalLedgerResult<()> {
        if let Some(record) = &self.migration {
            let prefix_length = usize::try_from(record.source_event_count)
                .map_err(|_| failure("migration_prefix_invalid", "verify_cutover_prefix"))?;
            let prefix = events
                .get(..prefix_length)
                .ok_or_else(|| failure("migration_prefix_missing", "verify_cutover_prefix"))?;
            let completion = events
                .get(prefix_length)
                .ok_or_else(|| failure("migration_completion_missing", "verify_cutover_prefix"))?;
            let payload_record = match completion.payload() {
                actingcommand_contract::EventPayload::Ledger(
                    actingcommand_contract::LedgerPayload::Recovered(payload),
                ) => payload.migration(),
                _ => None,
            };
            let head = prefix
                .last()
                .map(super::migration::canonical_stored_record)
                .transpose()?
                .map_or_else(
                    || actingcommand_runtime_database::digest(&[]),
                    |bytes| actingcommand_runtime_database::digest(&bytes),
                );
            if completion.sequence() != record.cutover_sequence
                || payload_record != Some(record.as_ref())
                || super::migration::canonical_stored_digest(prefix)?
                    != record.imported_content_sha256
                || head != record.source_head_sha256
            {
                return Err(failure(
                    "migration_prefix_mismatch",
                    "verify_cutover_prefix",
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
    connection.query_row("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN ('ledger_events','ledger_links','ledger_artifacts','ledger_meta')", [], |row| row.get(0)).map_err(|error| sql_error(error, "inspect_ledger_schema"))
}

fn format_version(connection: &Connection) -> GlobalLedgerResult<i64> {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| sql_error(error, "read_database_format"))
}

fn require_empty_format(connection: &Connection) -> GlobalLedgerResult<()> {
    if table_count(connection)? != 0 || format_version(connection)? != 0 {
        return Err(failure(
            "ledger_initialization_conflict",
            "verify_empty_ledger_format",
        ));
    }
    Ok(())
}

fn mark_formal_format(connection: &Connection) -> GlobalLedgerResult<()> {
    connection
        .pragma_update(None, "user_version", FORMAL_FORMAT_VERSION)
        .map_err(|error| sql_error(error, "mark_database_format"))
}

pub(super) fn has_schema(database: &RuntimeDatabase) -> GlobalLedgerResult<bool> {
    let connection = database.connection("inspect_runtime_ledger")?;
    match (format_version(&connection)?, table_count(&connection)?) {
        (0, 0) => Ok(false),
        (0 | FORMAL_FORMAT_VERSION, 4) => Ok(true),
        _ => Err(failure(
            "ledger_schema_incomplete",
            "inspect_runtime_ledger",
        )),
    }
}

pub(super) fn storage_status<F>(
    database: &RuntimeDatabase,
    verifier: &mut F,
    budget: ReadBudget,
) -> GlobalLedgerResult<super::LedgerStorageStatus>
where
    F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
{
    if !has_schema(database)? {
        return Ok(super::LedgerStorageStatus::Missing);
    }
    let raw = read_snapshot(database, budget)?;
    let marker = SqliteMarker::parse(&raw.meta)?;
    let (events, hash) = verify_snapshot(database, raw, &mut Some(verifier))?;
    Ok(marker.status(&events, hash))
}

pub(super) fn initialize_formal_empty(database: &RuntimeDatabase) -> GlobalLedgerResult<()> {
    let mut connection = database.connection("initialize_runtime_ledger")?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sql_error(error, "begin_runtime_ledger"))?;
    let result = (|| {
        if table_count(&transaction)? != 0 || format_version(&transaction)? != 0 {
            return Err(failure(
                "ledger_initialization_conflict",
                "initialize_runtime_ledger",
            ));
        }
        transaction
            .execute_batch(include_str!("sqlite/schema.sql"))
            .map_err(|error| sql_error(error, "create_runtime_ledger"))?;
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
            .map_err(|error| sql_error(error, "commit_runtime_ledger")),
        Err(error) => Err(error.with_close_result(
            transaction
                .rollback()
                .map_err(|error| sql_error(error, "rollback_runtime_ledger")),
        )),
    }
}

pub(super) fn open_formal<F>(
    config: GlobalLedgerConfig,
    database: Arc<RuntimeDatabase>,
    lock: super::storage::LockedWriterFile,
    compatibility: Option<super::storage::LockedWriterFile>,
    mut verifier: F,
) -> GlobalLedgerResult<SqliteLedgerStore>
where
    F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
{
    let raw = read_snapshot(&database, None)?;
    let marker = SqliteMarker::parse(&raw.meta)?;
    if marker.state != "ready" {
        return Err(failure("ledger_migration_required", "open_runtime_ledger"));
    }
    let (events, head_hash) = verify_snapshot(&database, raw, &mut Some(&mut verifier))?;
    let head = events.last().map_or(0, PersistedEvent::sequence);
    upgrade_views(&database, head, head_hash.as_deref())?;
    let next = increment_sequence(head)?;
    let (ownership, stale) = WriterOwnership::from_locked(lock, compatibility, &config.owner_id)?;
    let backend = SqliteStorage {
        database,
        ownership,
        head,
        head_hash,
        marker,
    };
    let mut store = EventStore::recovered(backend, next, events)?;
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
    // This lookup only reuses ArtifactStore proofs acquired outside the database mutex.
    let mut cached = |reference: &ProjectedArtifactReference| {
        source
            .verified
            .iter()
            .find(|(projected, _)| projected == reference)
            .map(|(_, verified)| verified.clone())
    };
    match storage_status(database, &mut cached, budget)? {
        super::LedgerStorageStatus::Ready {
            migration: Some(existing),
            ..
        } if existing.as_ref() == record => return storage_status(database, &mut cached, budget),
        super::LedgerStorageStatus::Missing => {}
        _ => {
            return Err(failure(
                "ledger_migration_conflict",
                "import_segment_ledger",
            ));
        }
    }
    let completion = PersistedEvent::from_sanitized(record.cutover_sequence, completion)
        .map_err(|error| failure(error.code(), "validate_migration_completion"))?;
    let marker = SqliteMarker::migrated(record)?;
    let mut expected = source.events.clone();
    expected.push(completion);
    marker.verify_events(&expected)?;
    let mut connection = database.connection("import_segment_ledger")?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sql_error(error, "begin_segment_import"))?;
    let prepared = (|| {
        if table_count(&transaction)? != 0 || format_version(&transaction)? != 0 {
            return Err(failure(
                "ledger_migration_conflict",
                "import_segment_ledger",
            ));
        }
        transaction
            .execute_batch(include_str!("sqlite/schema.sql"))
            .map_err(|error| sql_error(error, "create_import_schema"))?;
        views::deploy(&transaction)?;
        mark_formal_format(&transaction)?;
        let mut head_hash = None;
        for (index, event) in expected.iter().enumerate() {
            check_read_budget(budget, 0, index + 1)?;
            if event.sequence() != (index as u64) + 1 {
                return Err(failure("sequence_discontinuity", "import_segment_ledger"));
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
        let (actual, hash) = verify_snapshot(database, raw, &mut Some(&mut cached))?;
        if actual != expected {
            return Err(failure(
                "migration_canonical_mismatch",
                "verify_import_transaction",
            ));
        }
        let expected_index = EventIndexes::from_events(&expected);
        let actual_index = EventIndexes::from_events(&actual);
        if actual_index.query(&actual, &EventQuery::default())
            != expected_index.query(&expected, &EventQuery::default())
        {
            return Err(failure(
                "migration_projection_mismatch",
                "verify_import_transaction",
            ));
        }
        Ok(marker.status(&actual, hash))
    })();
    let status = match prepared {
        Ok(status) => status,
        Err(error) => {
            return Err(error.with_close_result(
                transaction
                    .rollback()
                    .map_err(|error| sql_error(error, "rollback_segment_import")),
            ));
        }
    };
    if dry_run {
        transaction
            .rollback()
            .map_err(|error| sql_error(error, "rollback_import_dry_run"))?;
        return Ok(status);
    }
    match transaction.commit() {
        Ok(()) => Ok(status),
        Err(error) => {
            let mut original = sql_error(error, "commit_segment_import");
            drop(connection);
            let observed = storage_status(database, &mut cached, budget);
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
            let connection = database.connection("inspect_candidate_format")?;
            format_version(&connection)?
        };
        if format == FORMAL_FORMAT_VERSION {
            return Err(failure(
                "candidate_production_conflict",
                "open_sqlite_candidate",
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
                    "candidate_production_conflict",
                    "open_sqlite_candidate",
                ));
            }
            let (events, hash) = verify_snapshot(&database, raw, &mut verifier)?;
            upgrade_views(
                &database,
                events.last().map_or(0, PersistedEvent::sequence),
                hash.as_deref(),
            )?;
            Ok((events, hash))
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
            marker: SqliteMarker::candidate(),
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
    fn project_view_page(
        &self,
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
    ) -> Option<GlobalLedgerResult<RuntimeEventQueryPage>> {
        Some(
            SqliteViewSnapshot {
                database: Arc::clone(&self.database),
                through_sequence: self.head,
                head_hash: self.head_hash.clone(),
                budget: None,
                source: LedgerReadSource::Runtime,
            }
            .project_view_page(query, profile, request),
        )
    }

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
            != meta_row_with_marker(
                &self.database,
                event.sequence(),
                self.head,
                self.head_hash.as_deref(),
                &self.marker,
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
        let meta = meta_row_with_marker(
            &self.database,
            next,
            event.sequence(),
            Some(&projected.hash),
            &self.marker,
        );
        let changed = transaction.execute(
            "UPDATE ledger_meta SET schema_version=?2,next_sequence=?3,head_sequence=?4,head_record_sha256=?5,storage_backend=?6,migration_id=?7,cutover_state=?8,integer_encoding=?9,integrity_tag=?10,migration_record=?11 WHERE singleton=?1",
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

pub(super) fn open_metadata(
    database: Arc<RuntimeDatabase>,
    budget: ReadBudget,
) -> GlobalLedgerResult<(Vec<LedgerEventMetadata>, SqliteViewSnapshot)> {
    let raw = read_snapshot(&database, budget)?;
    let bytes = raw.bytes;
    let marker = SqliteMarker::parse(&raw.meta)?;
    let (records, head_hash) = verify_snapshot_records(&database, raw)?;
    if marker.state != "ready" {
        return Err(failure(
            "ledger_candidate_not_production",
            "open_runtime_evidence",
        ));
    }
    let through_sequence = records.last().map_or(0, StoredEventRecord::sequence);
    let mut events = Vec::with_capacity(records.len());
    for record in records {
        check_read_budget(budget, bytes, events.len() + 1)?;
        events.push(
            record
                .into_metadata()
                .map_err(|error| failure(error.code(), "validate_persisted_event"))?,
        );
    }
    check_read_budget(budget, bytes, events.len())?;
    Ok((
        events,
        SqliteViewSnapshot {
            database,
            through_sequence,
            head_hash,
            budget,
            source: LedgerReadSource::Offline,
        },
    ))
}

/// Retains the physical owner and authenticates the opened prefix on each read transaction.
pub(super) struct SqliteViewSnapshot {
    database: Arc<RuntimeDatabase>,
    pub(super) through_sequence: u64,
    head_hash: Option<String>,
    budget: ReadBudget,
    source: LedgerReadSource,
}

impl SqliteViewSnapshot {
    pub(super) fn project_view_page(
        &self,
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
    ) -> GlobalLedgerResult<RuntimeEventQueryPage> {
        let (snapshot, after) =
            super::projection::page_bounds(query, profile, request, self.through_sequence)?;
        check_read_budget(self.budget, 0, 0)?;
        let mut connection = self.database.connection("query_ledger_view")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|error| sql_error(error, "begin_ledger_view_snapshot"))?;
        let result = (|| {
            let raw = read_snapshot_connection(&transaction, self.budget)?;
            let bytes = raw.bytes;
            let prefix_hash = raw
                .events
                .iter()
                .find(|row| row.first() == Some(&SqlValue::Integer(encode(self.through_sequence))))
                .and_then(|row| row.get(11))
                .cloned();
            let (records, _) = verify_snapshot_records(&self.database, raw)?;
            if prefix_hash != self.head_hash.clone().map(SqlValue::Text)
                || records.last().map_or(0, StoredEventRecord::sequence) < self.through_sequence
            {
                return Err(failure(
                    "ledger_snapshot_boundary_mismatch",
                    "query_ledger_view",
                ));
            }
            let mut events = Vec::new();
            for record in records
                .into_iter()
                .take_while(|record| record.sequence() <= self.through_sequence)
            {
                check_read_budget(self.budget, bytes, events.len() + 1)?;
                events.push(
                    record
                        .into_metadata()
                        .map_err(|error| failure(error.code(), "validate_persisted_event"))?,
                );
            }
            let sequences = views::select_sequences(
                &transaction,
                &events,
                query,
                after,
                snapshot,
                usize::from(request.limit()) + 1,
                self.budget,
            )?;
            let indexes = EventIndexes::from_events(&events);
            let page = indexes.project_view_page(
                &events,
                query,
                profile,
                request,
                LedgerReadScope {
                    source: self.source,
                    material_read: LedgerMaterialReadState::NotRequested,
                    scanned_through_position: self.through_sequence,
                    read_complete: true,
                    limits: Vec::new(),
                },
                super::projection::PageSelection {
                    through_sequence: self.through_sequence,
                    sequences: Some(&sequences),
                },
            )?;
            check_read_budget(self.budget, bytes, events.len())?;
            Ok(page)
        })();
        match result {
            Ok(page) => {
                transaction
                    .commit()
                    .map_err(|error| sql_error(error, "close_ledger_view_snapshot"))?;
                Ok(page)
            }
            Err(error) => Err(error.with_close_result(
                transaction
                    .rollback()
                    .map_err(|error| sql_error(error, "rollback_ledger_view_snapshot")),
            )),
        }
    }
}

fn upgrade_views(
    database: &RuntimeDatabase,
    head: u64,
    hash: Option<&str>,
) -> GlobalLedgerResult<()> {
    let mut connection = database.connection("upgrade_ledger_views")?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sql_error(error, "begin_ledger_view_upgrade"))?;
    let result = (|| {
        if !views::installed(&transaction)? {
            let raw = read_snapshot_connection(&transaction, None)?;
            let (records, actual_hash) = verify_snapshot_records(database, raw)?;
            if records.last().map_or(0, StoredEventRecord::sequence) != head
                || actual_hash.as_deref() != hash
            {
                return Err(failure(
                    "ledger_snapshot_boundary_mismatch",
                    "upgrade_ledger_views",
                ));
            }
            views::deploy(&transaction)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => transaction
            .commit()
            .map_err(|error| sql_error(error, "commit_ledger_view_upgrade")),
        Err(error) => Err(error.with_close_result(
            transaction
                .rollback()
                .map_err(|error| sql_error(error, "rollback_ledger_view_upgrade")),
        )),
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
                "ledger_candidate_not_production",
                "open_runtime_evidence",
            ));
        }
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

fn initialize(database: &RuntimeDatabase, first_use: bool) -> GlobalLedgerResult<()> {
    let mut connection = database.connection("initialize_sqlite_ledger")?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sql_error(error, "begin_ledger_schema"))?;
    let result = (|| {
        let tables: i64 = transaction.query_row("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN ('ledger_events','ledger_links','ledger_artifacts','ledger_meta')", [], |row| row.get(0))
        .map_err(|error| sql_error(error, "inspect_ledger_schema"))?;
        match tables {
            0 if first_use => {
                require_empty_format(&transaction)?;
                transaction
                    .execute_batch(include_str!("sqlite/schema.sql"))
                    .map_err(|error| sql_error(error, "initialize_ledger_schema"))?;
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
                    "ledger_schema_incomplete",
                    "initialize_sqlite_ledger",
                ));
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => transaction
            .commit()
            .map_err(|error| sql_error(error, "commit_ledger_schema")),
        Err(error) => Err(error.with_close_result(
            transaction
                .rollback()
                .map_err(|error| sql_error(error, "rollback_ledger_schema")),
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
    let raw = read_snapshot_connection(&transaction, budget)?;
    transaction
        .commit()
        .map_err(|error| sql_error(error, "close_sqlite_snapshot"))?;
    Ok(raw)
}

fn read_snapshot_connection(
    connection: &Connection,
    budget: ReadBudget,
) -> GlobalLedgerResult<RawSnapshot> {
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
    let has_marker = connection
        .prepare("PRAGMA table_info(ledger_meta)")
        .and_then(|mut statement| {
            let columns = statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(columns.iter().any(|column| column == "migration_record"))
        })
        .map_err(|error| sql_error(error, "inspect_ledger_marker_schema"))?;
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
        return Err(failure("ledger_meta_missing", "read_sqlite_meta"));
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
    let (records, hash) = verify_snapshot_records(database, raw)?;
    let mut events = Vec::with_capacity(records.len());
    for stored in records {
        check_read_budget(budget, bytes, events.len() + 1)?;
        let event = match verifier.as_mut() {
            Some(verifier) => stored.into_event_with_artifact_verifier(verifier),
            None => stored.into_event(),
        }
        .map_err(|error| failure(error.code(), "validate_persisted_event"))?;
        check_read_budget(budget, bytes, events.len() + 1)?;
        events.push(event);
    }
    Ok((events, hash))
}

/// Authenticates the complete ledger snapshot without opening referenced material.
fn verify_snapshot_records(
    database: &RuntimeDatabase,
    raw: RawSnapshot,
) -> GlobalLedgerResult<(Vec<StoredEventRecord>, Option<String>)> {
    let marker = SqliteMarker::parse(&raw.meta)?;
    let expected_format = if marker.state == "ready" {
        FORMAL_FORMAT_VERSION
    } else {
        0
    };
    if raw.format_version != expected_format {
        return Err(failure(
            "ledger_format_marker_mismatch",
            "verify_database_format",
        ));
    }
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
        let event = stored
            .clone()
            .into_metadata()
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
        let projected = project_stored_record(database, &stored, head_hash.as_deref())?;
        if row != projected.event {
            return Err(failure("ledger_record_mismatch", "verify_sqlite_record"));
        }
        check_read_budget(raw.budget, raw.bytes, events.len() + 1)?;
        expected_links.push(projected.links);
        expected_artifacts.extend(projected.artifacts);
        head_hash = Some(projected.hash);
        events.push(stored);
        next = increment_sequence(next)?;
    }
    if raw.links != expected_links || raw.artifacts != expected_artifacts {
        return Err(failure("ledger_index_mismatch", "verify_sqlite_relations"));
    }
    if raw.meta
        != meta_row_with_marker(
            database,
            next,
            events.last().map_or(0, StoredEventRecord::sequence),
            head_hash.as_deref(),
            &marker,
        )
    {
        return Err(failure("ledger_meta_mismatch", "verify_sqlite_head"));
    }
    marker.verify_records(&events)?;
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
    project_stored_record(database, &StoredEventRecord::from_event(event), previous)
}

fn project_stored_record(
    database: &RuntimeDatabase,
    stored: &StoredEventRecord,
    previous: Option<&str>,
) -> GlobalLedgerResult<ProjectedRecord> {
    let bytes = serde_json::to_vec(stored).map_err(|error| {
        GlobalLedgerError::json("event_serialization_failed", "serialize_event", &error)
    })?;
    let value = serde_json::to_value(stored).map_err(|error| {
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
