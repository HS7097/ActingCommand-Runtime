// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    EventId, EventLinks, EventOrigin, EventPayload, EventType, ReleasePayload, StatePayload,
};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const RELEASE_BASELINE_STATE_KEY: &str = "release.legacy.baseline";
const SOURCE_SCHEMA: &str = "actingcommand.release-ledger-source.v1";
const BASELINE_PAGE_EVENTS: usize = 256;
const OPERATION: &str = "verify_release_ledger_source";

/// A persistable locator, not a fact or material capability. Every use requires verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseLedgerSourceReference {
    schema_version: String,
    event_id: EventId,
    sequence: u64,
    record_sha256: String,
}

impl ReleaseLedgerSourceReference {
    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    pub fn event_id(&self) -> &EventId {
        &self.event_id
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn record_sha256(&self) -> &str {
        &self.record_sha256
    }
}

/// A checked Release result or Release baseline migration. It grants no artifact access.
pub struct VerifiedReleaseLedgerSource {
    reference: ReleaseLedgerSourceReference,
    event: LedgerEventMetadata,
}

impl VerifiedReleaseLedgerSource {
    pub fn reference(&self) -> &ReleaseLedgerSourceReference {
        &self.reference
    }

    pub fn event_id(&self) -> &EventId {
        self.event.event_id()
    }

    pub fn sequence(&self) -> u64 {
        self.event.sequence()
    }

    pub fn event_type(&self) -> EventType {
        self.event.event_type()
    }

    pub fn origin(&self) -> &EventOrigin {
        self.event.origin()
    }

    pub fn links(&self) -> &EventLinks {
        self.event.links()
    }

    pub fn payload(&self) -> &EventPayload {
        self.event.payload()
    }
}

impl fmt::Debug for VerifiedReleaseLedgerSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedReleaseLedgerSource")
            .field("reference", &self.reference)
            .field("event_type", &self.event.event_type())
            .finish()
    }
}

/// Captures the original canonical identity after checking the fact in this same transaction.
pub fn capture_release_source_reference(
    database: &RuntimeDatabase,
    transaction: &RuntimeTransaction<'_, '_>,
    event: &PersistedEvent,
) -> GlobalLedgerResult<ReleaseLedgerSourceReference> {
    require_owner(database, transaction)?;
    require_release_payload(event.payload())?;
    verify_transaction_event(database, transaction, event)?;
    let bytes = super::super::migration::canonical_record(event)?;
    Ok(ReleaseLedgerSourceReference {
        schema_version: SOURCE_SCHEMA.to_owned(),
        event_id: *event.event_id(),
        sequence: event.sequence(),
        record_sha256: format!("sha256:{:x}", Sha256::digest(&bytes)),
    })
}

/// Verifies a stored locator against the original row, its predecessor and native indexes.
/// The caller retains its transaction; this function opens no connection or material.
pub fn verify_release_source_reference(
    database: &RuntimeDatabase,
    transaction: &RuntimeTransaction<'_, '_>,
    reference: &ReleaseLedgerSourceReference,
) -> GlobalLedgerResult<VerifiedReleaseLedgerSource> {
    require_owner(database, transaction)?;
    if reference.schema_version != SOURCE_SCHEMA || reference.sequence == 0 {
        return Err(failure(
            "release_ledger_source_reference_invalid",
            OPERATION,
        ));
    }
    let head = read_source_head(database, transaction.sql())?
        .ok_or_else(|| failure("release_ledger_source_storage_missing", OPERATION))?;
    if reference.sequence > head.sequence {
        return Err(failure("ledger_record_missing", OPERATION));
    }
    let previous = if reference.sequence == 1 {
        None
    } else {
        let row = read_source_row(transaction.sql(), reference.sequence - 1)?;
        let previous = previous_hash(&row)?;
        let (_, projected) = verify_source_row(database, &row, previous)?;
        verify_source_indexes(transaction.sql(), &projected)?;
        Some(projected.hash)
    };
    let row = read_source_row(transaction.sql(), reference.sequence)?;
    let (event, projected) = verify_source_row(database, &row, previous.as_deref())?;
    verify_source_indexes(transaction.sql(), &projected)?;
    if event.sequence() != reference.sequence
        || event.event_id() != &reference.event_id
        || projected.hash != reference.record_sha256
        || event.sequence() == head.sequence && Some(&projected.hash) != head.hash.as_ref()
    {
        return Err(failure(
            "release_ledger_source_reference_mismatch",
            OPERATION,
        ));
    }
    require_release_payload(event.payload())?;
    Ok(VerifiedReleaseLedgerSource {
        reference: reference.clone(),
        event,
    })
}

/// Checks the fixed Release baseline key through the original authenticated global prefix.
/// A complete scan prevents changed filter columns from hiding an existing boundary. Pages
/// bound memory; duplicate boundaries fail with both original locators, never a first-row win.
pub fn read_release_baseline_source(
    database: &RuntimeDatabase,
    transaction: &RuntimeTransaction<'_, '_>,
) -> GlobalLedgerResult<Option<VerifiedReleaseLedgerSource>> {
    require_owner(database, transaction)?;
    let connection = transaction.sql();
    let Some(head) = read_source_head(database, connection)? else {
        return Ok(None);
    };
    let mut after = 0;
    let mut previous = None;
    let mut artifact_count = 0_u64;
    let mut found: Option<VerifiedReleaseLedgerSource> = None;
    loop {
        let rows = read_rows(
            connection,
            &format!(
                "SELECT {EVENT_COLUMNS} FROM ledger_events WHERE sequence>{} ORDER BY sequence LIMIT {BASELINE_PAGE_EVENTS}",
                encode(after),
            ),
            None,
            &mut 0,
            true,
        )?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let (event, projected) = verify_source_row(database, &row, previous.as_deref())?;
            if event.sequence() != increment_sequence(after)? || event.sequence() > head.sequence {
                return Err(failure("sequence_discontinuity", OPERATION));
            }
            verify_source_indexes(connection, &projected)?;
            artifact_count = artifact_count
                .checked_add(projected.artifacts.len() as u64)
                .ok_or_else(|| failure("ledger_snapshot_overflow", OPERATION))?;
            after = event.sequence();
            previous = Some(projected.hash.clone());
            if is_release_baseline(event.payload()) {
                let current = VerifiedReleaseLedgerSource {
                    reference: ReleaseLedgerSourceReference {
                        schema_version: SOURCE_SCHEMA.to_owned(),
                        event_id: *event.event_id(),
                        sequence: event.sequence(),
                        record_sha256: projected.hash,
                    },
                    event,
                };
                if let Some(first) = found {
                    let mut error = failure("release_baseline_source_conflict", OPERATION);
                    error.detail = Some(format!(
                        "first={:?}; conflicting={:?}",
                        first.reference, current.reference,
                    ));
                    return Err(error);
                }
                found = Some(current);
            }
        }
    }
    if after != head.sequence || previous != head.hash {
        return Err(failure("ledger_meta_mismatch", OPERATION));
    }
    let (events, links, artifacts): (i64, i64, i64) = connection
        .query_row(
            "SELECT (SELECT count(*) FROM ledger_events), (SELECT count(*) FROM ledger_links), (SELECT count(*) FROM ledger_artifacts)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| sql_error(error, OPERATION))?;
    if u64::try_from(events).ok() != Some(after)
        || u64::try_from(links).ok() != Some(after)
        || u64::try_from(artifacts).ok() != Some(artifact_count)
    {
        return Err(failure("ledger_index_mismatch", OPERATION));
    }
    Ok(found)
}

struct SourceHead {
    sequence: u64,
    hash: Option<String>,
}

fn require_owner(
    database: &RuntimeDatabase,
    transaction: &RuntimeTransaction<'_, '_>,
) -> GlobalLedgerResult<()> {
    if !transaction.belongs_to(database) {
        return Err(failure("ledger_transaction_owner_mismatch", OPERATION));
    }
    Ok(())
}

fn read_source_head(
    database: &RuntimeDatabase,
    connection: &Connection,
) -> GlobalLedgerResult<Option<SourceHead>> {
    match (format_version(connection)?, table_count(connection)?) {
        (0, 0) => {
            let declared: i64 = connection
                .query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'ledger_*' OR tbl_name GLOB 'ledger_*'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| sql_error(error, OPERATION))?;
            if declared != 0 {
                return Err(failure("ledger_schema_incomplete", OPERATION));
            }
            return Ok(None);
        }
        (0 | FORMAL_FORMAT_VERSION, 4) => {}
        _ => return Err(failure("ledger_schema_incomplete", OPERATION)),
    }
    let meta = read_meta(connection)?;
    let marker = SqliteMarker::parse(&meta)?;
    let expected_format = if marker.state == "ready" {
        FORMAL_FORMAT_VERSION
    } else {
        0
    };
    if format_version(connection)? != expected_format {
        return Err(failure("ledger_format_marker_mismatch", OPERATION));
    }
    let (Some(SqlValue::Integer(next)), Some(SqlValue::Integer(head)), Some(hash)) =
        (meta.get(2), meta.get(3), meta.get(4))
    else {
        return Err(failure("ledger_meta_mismatch", OPERATION));
    };
    let head = decode(*head);
    let hash = match hash {
        SqlValue::Null if head == 0 => None,
        SqlValue::Text(hash) if head != 0 => Some(hash.as_str()),
        _ => return Err(failure("ledger_meta_mismatch", OPERATION)),
    };
    if decode(*next) != increment_sequence(head)?
        || meta != meta_row_with_marker(database, decode(*next), head, hash, &marker)
    {
        return Err(failure("ledger_meta_mismatch", OPERATION));
    }
    Ok(Some(SourceHead {
        sequence: head,
        hash: hash.map(str::to_owned),
    }))
}

fn read_source_row(connection: &Connection, sequence: u64) -> GlobalLedgerResult<SqlRow> {
    let mut rows = read_rows(
        connection,
        &format!(
            "SELECT {EVENT_COLUMNS} FROM ledger_events WHERE sequence={}",
            encode(sequence),
        ),
        None,
        &mut 0,
        true,
    )?;
    if rows.len() != 1 {
        return Err(failure("ledger_record_missing", OPERATION));
    }
    Ok(rows.remove(0))
}

fn previous_hash(row: &SqlRow) -> GlobalLedgerResult<Option<&str>> {
    match row.get(12) {
        Some(SqlValue::Null) => Ok(None),
        Some(SqlValue::Text(hash)) => Ok(Some(hash)),
        _ => Err(failure("ledger_record_mismatch", OPERATION)),
    }
}

fn verify_source_row(
    database: &RuntimeDatabase,
    row: &SqlRow,
    previous: Option<&str>,
) -> GlobalLedgerResult<(LedgerEventMetadata, ProjectedRecord)> {
    let Some(SqlValue::Blob(bytes)) = row.get(10) else {
        return Err(failure("corrupt_ledger_record", OPERATION));
    };
    let unique: UniqueJsonValue = serde_json::from_slice(bytes)
        .map_err(|error| GlobalLedgerError::json("corrupt_ledger_record", OPERATION, &error))?;
    let stored: StoredEventRecord = serde_json::from_value(unique.0)
        .map_err(|error| GlobalLedgerError::json("corrupt_ledger_record", OPERATION, &error))?;
    let event = stored
        .clone()
        .into_metadata()
        .map_err(|error| failure(error.code(), OPERATION))?;
    let projected = project_stored_record(database, &stored, previous)?;
    if *row != projected.event {
        return Err(failure("ledger_record_mismatch", OPERATION));
    }
    Ok((event, projected))
}

fn verify_source_indexes(
    connection: &Connection,
    projected: &ProjectedRecord,
) -> GlobalLedgerResult<()> {
    let Some(SqlValue::Integer(sequence)) = projected.event.first() else {
        return Err(failure("ledger_record_mismatch", OPERATION));
    };
    let links = read_rows(
        connection,
        &format!("SELECT {LINK_COLUMNS} FROM ledger_links WHERE sequence={sequence}"),
        None,
        &mut 0,
        false,
    )?;
    let artifact_limit = projected.artifacts.len().saturating_add(1);
    let artifacts = read_rows(
        connection,
        &format!(
            "SELECT {ARTIFACT_COLUMNS} FROM ledger_artifacts WHERE sequence={sequence} ORDER BY ordinal LIMIT {artifact_limit}",
        ),
        None,
        &mut 0,
        false,
    )?;
    if links != vec![projected.links.clone()] || artifacts != projected.artifacts {
        return Err(failure("ledger_index_mismatch", OPERATION));
    }
    Ok(())
}

fn is_release_baseline(payload: &EventPayload) -> bool {
    matches!(payload, EventPayload::State(StatePayload::Migrated(value))
        if value.migration().state_key() == RELEASE_BASELINE_STATE_KEY)
}

fn require_release_payload(payload: &EventPayload) -> GlobalLedgerResult<()> {
    if matches!(
        payload,
        EventPayload::Release(
            ReleasePayload::Staged(_)
                | ReleasePayload::Activated(_)
                | ReleasePayload::RolledBack(_)
        )
    ) || is_release_baseline(payload)
    {
        Ok(())
    } else {
        Err(failure("release_ledger_source_type_unsupported", OPERATION))
    }
}
