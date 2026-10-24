// SPDX-License-Identifier: AGPL-3.0-only

//! LN-1 (Workflow #422, LEAN-L model §7.2): appends records straight into a formal root's
//! tables, without the writer and without the in-memory event list, so a history far larger
//! than memory can be written for the scale harness. Each batch is one transaction, as the
//! writer's own batch commit (`SqliteStorage::commit_batch`) is: the records' projections
//! chained from the authenticated head (record hash, keyed tag, predecessor), their link rows,
//! their `ledger_index` rows under the default index policy and the keyed meta row. The formal
//! open and every authenticated read therefore accept the result. Test hooks only.

use super::{
    ARTIFACT_COLUMNS, BatchRows, EVENT_COLUMNS, IndexRows, LINK_COLUMNS, SqliteMarker,
    authenticated_extent, failure, increment_sequence, insert_row, ledger_meta_has_marker,
    meta_row_with_marker, project_batch, read_meta, read_meta_columns, sql_error, write_index_rows,
};
use crate::PersistedEvent;
use crate::codes::{LedgerCode, LedgerLocation};
use crate::global::GlobalLedgerResult;
use crate::global::index::IndexPolicy;
use actingcommand_contract::codes::SharedCode;
use actingcommand_runtime_database::RuntimeDatabase;
use rusqlite::{TransactionBehavior, params_from_iter, types::Value as SqlValue};

/// The position of `canonical_record` among the `ledger_events` columns (`EVENT_COLUMNS`).
const CANONICAL_RECORD: usize = 10;

/// What one committed batch wrote: each record's `record_sha256`, in sequence order, and the
/// bytes of their canonical records.
#[derive(Debug, Default)]
pub(crate) struct AppendedBatch {
    pub(crate) record_hashes: Vec<String>,
    pub(crate) canonical_bytes: u64,
}

/// The appender of one formal root: its authenticated head, its marker and the index policy the
/// running writer uses by default.
pub(crate) struct SyntheticAppender<'a> {
    database: &'a RuntimeDatabase,
    marker: SqliteMarker,
    meta_has_marker: bool,
    head: u64,
    head_hash: Option<String>,
    policy: IndexPolicy,
}

impl<'a> SyntheticAppender<'a> {
    /// Authenticates the keyed meta row (format, marker and head) of a formal root whose ledger
    /// is ready; no history row is read.
    pub(crate) fn open(database: &'a RuntimeDatabase) -> GlobalLedgerResult<Self> {
        let mut connection =
            database.connection(LedgerLocation::InspectRuntimeLedger.location())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|error| {
                sql_error(error, LedgerLocation::BeginLedgerClassification.location())
            })?;
        let Some((marker, head)) = authenticated_extent(database, &transaction)? else {
            return Err(failure(
                LedgerCode::LedgerSchemaIncomplete.code(),
                LedgerLocation::InspectRuntimeLedger.location(),
            ));
        };
        if marker.state != "ready" {
            return Err(failure(
                SharedCode::LedgerMigrationRequired.code(),
                LedgerLocation::OpenRuntimeLedger.location(),
            ));
        }
        let head_hash = match read_meta(&transaction)?.get(4) {
            Some(SqlValue::Text(hash)) => Some(hash.clone()),
            _ => None,
        };
        let meta_has_marker = ledger_meta_has_marker(&transaction)?;
        transaction.commit().map_err(|error| {
            sql_error(error, LedgerLocation::CloseLedgerClassification.location())
        })?;
        Ok(Self {
            database,
            marker,
            meta_has_marker,
            head,
            head_hash,
            policy: IndexPolicy::default(),
        })
    }

    /// The sequence the next appended record must carry.
    pub(crate) fn next_sequence(&self) -> GlobalLedgerResult<u64> {
        increment_sequence(self.head)
    }

    /// Commits `events` (consecutive, from [`Self::next_sequence`]) with their index rows and
    /// the new head in one transaction; the head moves only once the commit returned.
    pub(crate) fn append(
        &mut self,
        events: &[PersistedEvent],
    ) -> GlobalLedgerResult<AppendedBatch> {
        let (Some(first), Some(last)) = (events.first(), events.last()) else {
            return Ok(AppendedBatch::default());
        };
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
        let projected = project_batch(self.database, events, self.head_hash.as_deref())?;
        let last_hash = projected
            .last()
            .map(|record| record.hash.clone())
            .ok_or_else(|| {
                failure(
                    LedgerCode::SequenceDiscontinuity.code(),
                    LedgerLocation::AppendSqliteEvent.location(),
                )
            })?;
        let rows = IndexRows::of(events, &self.policy);
        let previous_meta = meta_row_with_marker(
            self.database,
            first.sequence(),
            self.head,
            self.head_hash.as_deref(),
            &self.marker,
        );
        let meta = meta_row_with_marker(
            self.database,
            increment_sequence(last.sequence())?,
            last.sequence(),
            Some(&last_hash),
            &self.marker,
        );
        let mut connection = self
            .database
            .connection(LedgerLocation::AppendSqliteEvent.location())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| sql_error(error, LedgerLocation::BeginSqliteAppend.location()))?;
        let written: GlobalLedgerResult<()> = (|| {
            let current = read_meta_columns(&transaction, self.meta_has_marker, None, &mut 0)?;
            if current != previous_meta {
                return Err(failure(
                    LedgerCode::LedgerMetaMismatch.code(),
                    LedgerLocation::AppendSqliteEvent.location(),
                ));
            }
            for record in &projected {
                insert_row(&transaction, "ledger_events", EVENT_COLUMNS, &record.event)?;
                insert_row(&transaction, "ledger_links", LINK_COLUMNS, &record.links)?;
                for artifact in &record.artifacts {
                    insert_row(&transaction, "ledger_artifacts", ARTIFACT_COLUMNS, artifact)?;
                }
            }
            let changed = transaction
                .execute(
                    "UPDATE ledger_meta SET schema_version=?2,next_sequence=?3,head_sequence=?4,head_record_sha256=?5,storage_backend=?6,migration_id=?7,cutover_state=?8,integer_encoding=?9,integrity_tag=?10,migration_record=?11 WHERE singleton=?1",
                    params_from_iter(meta.iter()),
                )
                .map_err(|error| sql_error(error, LedgerLocation::UpdateSqliteHead.location()))?;
            if changed != 1 {
                return Err(failure(
                    LedgerCode::LedgerMetaMismatch.code(),
                    LedgerLocation::UpdateSqliteHead.location(),
                ));
            }
            write_index_rows(
                &transaction,
                &BatchRows {
                    events,
                    projected: &projected,
                    rows: &rows,
                    meta: &meta,
                },
            )
        })();
        if let Err(error) = written {
            let rolled_back = transaction
                .rollback()
                .map_err(|error| sql_error(error, LedgerLocation::RollbackJointEvent.location()));
            return Err(error.with_close_result(rolled_back));
        }
        transaction
            .commit()
            .map_err(|error| sql_error(error, LedgerLocation::CommitSqliteEvent.location()))?;
        self.head = last.sequence();
        self.head_hash = Some(last_hash);
        let canonical_bytes = projected
            .iter()
            .map(|record| match record.event.get(CANONICAL_RECORD) {
                Some(SqlValue::Blob(bytes)) => bytes.len() as u64,
                _ => 0,
            })
            .sum();
        Ok(AppendedBatch {
            record_hashes: projected.into_iter().map(|record| record.hash).collect(),
            canonical_bytes,
        })
    }
}
