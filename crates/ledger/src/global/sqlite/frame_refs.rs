// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-7c slice R (`rd-1026/ln7c/final-query.md` §3, F5; Alice's ruling 70: frame retention
//! by ledger query): the frame index links of the provenance pointers a record carries only in
//! its payload, so a later query finds them by key instead of reading payloads.
//!
//! - **The rows.** [`frame_refs`] is a pure function of a record: one row per frame, artifact or
//!   run that a capture summary, a publication, an input intent, a dedup marker or an evidence
//!   read confirmation names in its payload (`ledger_frame_refs`: the target's kind and id, the
//!   record's sequence, the pointer and, for a resource reading, the run its snapshot id names).
//!   The writer writes them in the transaction of the record's batch (the append, the synthetic
//!   appender, the segment import), so no producer changes and a future producer is covered by
//!   construction.
//! - **The rule.** The table is derived and can be rebuilt, and nothing reads it yet, so no
//!   state of it fails an open or an append (`contracts/ledger-observer.md` §11.7).
//! - **The extent.** `ledger_frame_refs_extent` holds the sequences `from_sequence..=
//!   through_sequence` whose records' rows are all written; a row outside it is never trusted. A
//!   batch writes its rows and moves `through_sequence` to its last sequence only when the extent
//!   ends right before the batch. Otherwise, or when a statement of the index fails, the batch's
//!   rows are not written (a savepoint takes them back), the extent stays and the batch commits as
//!   always; the next open fills the gap and reports it.
//! - **The fill at open.** An older build appends records without rows, and a copied or restored
//!   root may have no extent. The writer open fills the gap `(through_sequence, head]` upwards
//!   and, for a root without the extent, every record from the head down to 1 (the backfill;
//!   ageing out is not safe, §3.3). It reads pages of at most `ledger_observer.history_page_rows`
//!   pointer records, selected by the event type index with a sequence bound, authenticates each
//!   record on its own, and commits each page with the extent moved, within the open's deadline.
//!   One page is held in memory. An untrusted extent (past the head, malformed, or missing on an
//!   existing table) is removed with every row and rebuilt. A record that fails its
//!   authentication, the deadline, or any failure of the index stops the fill: the page in
//!   progress rolls back, so the extent stops short of the record and never covers the whole
//!   history, and the next open tries again. The open itself never fails for the index; it
//!   reports one warning ([`LedgerFrameRefsNote`], a `note` link of `ledger_opened`).
//! - **The checks.** The scrub (LN-9) and `ledger-maintenance verify` compare, for every record they
//!   authenticate inside the extent, its rows with [`frame_refs`]; a difference is
//!   `ledger_index_mismatch` of `verify_ledger_frame_refs` (`table=ledger_frame_refs`). Either marks
//!   the extent untrusted ([`mark_untrusted`]), so the next writer open rebuilds the index. The
//!   scrub's finding is non-fatal and never stops the Runtime; `verify`, a tool run, still fails.
//!
//! Nothing decides on the rows yet (slice Q will). No name starts with `ledger_view_`, so an older
//! build opens, writes and scrubs the database unchanged (§8).

use super::super::retention::LedgerFrameRefsNote;
use super::{
    ARTIFACT_COLUMNS, EVENT_COLUMNS, LINK_COLUMNS, RowReader, SqlRow, decode, encode, failure,
    increment_sequence, read_rows_bound, sql_error, views,
};
use crate::PersistedEvent;
use crate::codes::{LedgerCode, LedgerLocation};
use crate::fact::{LedgerEventMetadata, LedgerEventRead};
use crate::global::{GlobalLedgerError, GlobalLedgerResult};
use actingcommand_contract::outcome::Location;
use actingcommand_contract::{
    ArtifactId, CapturePayload, ClientActionValue, ClientPayload, EventActor, EventOrigin,
    EventPayload, EventSource, EventType, FactContent, FactPayload, InputPayload,
    PinnedFrameReason, RESOURCE_READING_DETECTOR_PREFIX, RunId, parse_resource_reading_snapshot_id,
};
use actingcommand_runtime_database::RuntimeDatabase;
use rusqlite::{Connection, TransactionBehavior, params_from_iter, types::Value as SqlValue};
use std::path::Path;
use std::time::Instant;

/// The tables and the index, as `schema.sql` declares them for a new root; the writer open of an
/// older root creates them (`IF NOT EXISTS`).
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS ledger_frame_refs (target_kind TEXT NOT NULL CHECK (target_kind IN ('frame', 'artifact', 'run')), target_id TEXT NOT NULL, sequence INTEGER NOT NULL, pointer TEXT NOT NULL CHECK (pointer IN ('summary_frame', 'summary_pin', 'published_artifact', 'published_frame', 'reading', 'before_frame', 'preserved_frame', 'read_confirmed')), run_id TEXT, PRIMARY KEY (target_kind, target_id, pointer, sequence)) STRICT, WITHOUT ROWID; CREATE INDEX IF NOT EXISTS ledger_frame_refs_sequence ON ledger_frame_refs(sequence); CREATE TABLE IF NOT EXISTS ledger_frame_refs_extent (singleton INTEGER PRIMARY KEY CHECK (singleton = 1), from_sequence INTEGER NOT NULL, through_sequence INTEGER NOT NULL) STRICT;";

/// The record types whose payload can name a pointer; the fill selects only them.
const POINTER_TYPES: [EventType; 5] = [
    EventType::CaptureSummaryCommitted,
    EventType::FactPublished,
    EventType::InputIntent,
    EventType::CaptureDedupWindow,
    EventType::ClientAction,
];

// Target kinds.
const FRAME: &str = "frame";
const ARTIFACT: &str = "artifact";
const RUN: &str = "run";
// Pointers.
const SUMMARY_FRAME: &str = "summary_frame";
const SUMMARY_PIN: &str = "summary_pin";
const PUBLISHED_ARTIFACT: &str = "published_artifact";
const PUBLISHED_FRAME: &str = "published_frame";
const READING: &str = "reading";
const BEFORE_FRAME: &str = "before_frame";
const PRESERVED_FRAME: &str = "preserved_frame";
const READ_CONFIRMED: &str = "read_confirmed";

/// The controls of an evidence read confirmation (`final-query.md` §4), the same on every
/// surface: the value names an artifact or a run.
const EVIDENCE_READ_FRAME: &str = "evidence_read_frame";
const EVIDENCE_READ_RUN: &str = "evidence_read_run";

const INSERT_REF: &str = "INSERT INTO ledger_frame_refs (target_kind,target_id,sequence,pointer,run_id) VALUES (?1,?2,?3,?4,?5)";
const READ_EXTENT: &str =
    "SELECT from_sequence,through_sequence FROM ledger_frame_refs_extent WHERE singleton=1";
const REF_COLUMNS: &str = "sequence,target_kind,target_id,pointer,run_id";
const REF_ORDER: &str = "ORDER BY sequence,target_kind,target_id,pointer";

/// One row of `ledger_frame_refs` without its sequence: the target, the pointer and the run a
/// resource reading names. The order is the table's key order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FrameRef {
    pub(crate) target_kind: &'static str,
    pub(crate) target_id: String,
    pub(crate) pointer: &'static str,
    pub(crate) run_id: Option<String>,
}

/// A stored row with its sequence, as read back.
type StoredRef = (u64, String, String, String, Option<String>);

/// The rows of `event`, in key order, one per target and pointer (`final-query.md` §3.2):
/// - `capture.summary_committed`: each summary frame's artifact (`summary_frame`), and each pinned
///   artifact whose reason is not `recognition_evidence` (`summary_pin`);
/// - `fact.published`: for each record whose snapshot id is `run:<run>/frame:<frame>/…`, the frame,
///   as a `reading` of that run when the detector is a resource reading's, else as a
///   `published_frame`; and an artifact the record holds as its content (`published_artifact`);
/// - `input.intent`: its provenance's before-frame (`before_frame`);
/// - `capture.dedup_window`: the frame it preserves (`preserved_frame`);
/// - `client.action` with the control `evidence_read_frame` or `evidence_read_run` from an agent
///   through an adapter, the command line or a person through the UI: the artifact or the run its
///   path-safe value names (`read_confirmed`).
///
/// Two pointers of one record to the same target give one row (the run of a reading is the
/// lowest named).
pub(crate) fn frame_refs<E: LedgerEventRead>(event: &E) -> GlobalLedgerResult<Vec<FrameRef>> {
    let mut refs = Vec::new();
    let mut push = |target_kind: &'static str,
                    target: String,
                    pointer: &'static str,
                    run_id: Option<String>| {
        refs.push(FrameRef {
            target_kind,
            target_id: target,
            pointer,
            run_id,
        });
    };
    match event.payload() {
        EventPayload::Capture(CapturePayload::SummaryCommitted(payload)) => {
            for frame in payload.summary().frames() {
                push(
                    ARTIFACT,
                    views::key(frame.artifact().artifact_id)?,
                    SUMMARY_FRAME,
                    None,
                );
            }
            for pin in payload.summary().pinned() {
                if pin.reason() == PinnedFrameReason::RecognitionEvidence {
                    continue;
                }
                if let Some(artifact) = pin.artifact() {
                    push(
                        ARTIFACT,
                        views::key(artifact.artifact_id)?,
                        SUMMARY_PIN,
                        None,
                    );
                }
            }
        }
        EventPayload::Fact(FactPayload::Published(payload)) => {
            for record in payload.records() {
                if let Some((run, frame)) =
                    parse_resource_reading_snapshot_id(&record.source_snapshot_id)
                {
                    if record
                        .source_detector
                        .starts_with(RESOURCE_READING_DETECTOR_PREFIX)
                    {
                        push(FRAME, views::key(frame)?, READING, Some(views::key(run)?));
                    } else {
                        push(FRAME, views::key(frame)?, PUBLISHED_FRAME, None);
                    }
                }
                if let FactContent::Artifact { artifact } = &record.content {
                    push(
                        ARTIFACT,
                        views::key(artifact.artifact_id)?,
                        PUBLISHED_ARTIFACT,
                        None,
                    );
                }
            }
        }
        EventPayload::Input(InputPayload::Intent(input)) => {
            if let Some(frame) = input
                .provenance()
                .and_then(|provenance| provenance.before_frame_id)
            {
                push(FRAME, views::key(frame)?, BEFORE_FRAME, None);
            }
        }
        EventPayload::Capture(CapturePayload::DedupWindow(window)) => {
            if let Some(frame) = window.preserved_frame_id() {
                push(FRAME, views::key(frame)?, PRESERVED_FRAME, None);
            }
        }
        EventPayload::Client(ClientPayload::Action(payload)) if confirmer(event.origin()) => {
            let record = payload.record();
            if let Some(ClientActionValue::PathSafeString(value)) = record.value() {
                match record.control_id() {
                    EVIDENCE_READ_FRAME => {
                        if let Some(artifact) = parsed::<ArtifactId>(value) {
                            push(ARTIFACT, views::key(artifact)?, READ_CONFIRMED, None);
                        }
                    }
                    EVIDENCE_READ_RUN => {
                        if let Some(run) = parsed::<RunId>(value) {
                            push(RUN, views::key(run)?, READ_CONFIRMED, None);
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    refs.sort();
    refs.dedup_by(|later, earlier| {
        (later.target_kind, &later.target_id, later.pointer)
            == (earlier.target_kind, &earlier.target_id, earlier.pointer)
    });
    Ok(refs)
}

/// The origins whose confirmation counts (`final-query.md` §4): an agent through MCP, the command
/// line, a person through the UI.
fn confirmer(origin: &EventOrigin) -> bool {
    matches!(
        (origin.actor(), origin.source()),
        (EventActor::Agent, EventSource::Adapter)
            | (EventActor::Cli, EventSource::Cli)
            | (EventActor::User, EventSource::Ui)
    )
}

/// An identifier from its canonical text; `None` for any other text.
fn parsed<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}

/// `ledger_index_mismatch` of `operation`, with `key=value` tokens naming the table and where.
fn mismatch(operation: Location, detail: String) -> GlobalLedgerError {
    failure(LedgerCode::LedgerIndexMismatch.code(), operation).with_detail(detail)
}

fn integer(value: u64) -> SqlValue {
    SqlValue::Integer(encode(value))
}

/// The rows of one record, in its batch's transaction.
fn insert_refs(
    connection: &Connection,
    sequence: u64,
    refs: &[FrameRef],
) -> GlobalLedgerResult<()> {
    if refs.is_empty() {
        return Ok(());
    }
    let operation = LedgerLocation::WriteLedgerFrameRefs.location();
    let mut statement = connection
        .prepare_cached(INSERT_REF)
        .map_err(|error| sql_error(error, operation))?;
    for frame_ref in refs {
        let changed = statement
            .execute(params_from_iter([
                SqlValue::Text(frame_ref.target_kind.to_owned()),
                SqlValue::Text(frame_ref.target_id.clone()),
                integer(sequence),
                SqlValue::Text(frame_ref.pointer.to_owned()),
                frame_ref
                    .run_id
                    .clone()
                    .map_or(SqlValue::Null, SqlValue::Text),
            ]))
            .map_err(|error| sql_error(error, operation))?;
        if changed != 1 {
            return Err(failure(
                LedgerCode::LedgerInsertIncomplete.code(),
                operation,
            ));
        }
    }
    Ok(())
}

/// The rows of `event` in the caller's transaction (the segment import writes its records
/// one by one).
pub(super) fn write_record(
    connection: &Connection,
    event: &PersistedEvent,
) -> GlobalLedgerResult<()> {
    insert_refs(connection, event.sequence(), &frame_refs(event)?)
}

/// The extent row of a new root (`from..=through`), in its creating transaction.
pub(super) fn insert_extent(
    connection: &Connection,
    from: u64,
    through: u64,
) -> GlobalLedgerResult<()> {
    let operation = LedgerLocation::WriteLedgerFrameRefs.location();
    let changed = connection
        .execute(
            "INSERT INTO ledger_frame_refs_extent (singleton,from_sequence,through_sequence) VALUES (1,?1,?2)",
            params_from_iter([integer(from), integer(through)]),
        )
        .map_err(|error| sql_error(error, operation))?;
    if changed != 1 {
        return Err(failure(
            LedgerCode::LedgerInsertIncomplete.code(),
            operation,
        ));
    }
    Ok(())
}

/// The rows of a batch (consecutive records) in its transaction, and the extent moved from the
/// sequence before the batch to its last, when the extent ends right before the batch. Otherwise,
/// or when a statement of the index fails, the batch's rows are not written (the savepoint takes
/// back what was written), the extent stays and the batch goes on; the next open fills the gap.
/// Only a savepoint that cannot be taken back (the batch's own transaction is gone, as on a full
/// disk) returns its error, which the batch's commit would meet anyway.
pub(super) fn write_batch(
    database: &RuntimeDatabase,
    connection: &Connection,
    events: &[PersistedEvent],
) -> GlobalLedgerResult<()> {
    let (Some(first), Some(last)) = (events.first(), events.last()) else {
        return Ok(());
    };
    if skipped(database.root()) {
        return Ok(());
    }
    let before = first.sequence().saturating_sub(1);
    // The extent must end right before the batch; any other state of the index leaves it be.
    if !matches!(read_extent(connection), Ok(Some((_, through))) if through == before) {
        return Ok(());
    }
    let operation = LedgerLocation::WriteLedgerFrameRefs.location();
    if connection
        .execute_batch("SAVEPOINT ledger_frame_refs")
        .is_err()
    {
        return Ok(());
    }
    let written = (|| -> GlobalLedgerResult<()> {
        for event in events {
            write_record(connection, event)?;
        }
        let changed = connection
            .execute(
                "UPDATE ledger_frame_refs_extent SET through_sequence=?1 WHERE singleton=1 AND through_sequence=?2",
                params_from_iter([integer(last.sequence()), integer(before)]),
            )
            .map_err(|error| sql_error(error, operation))?;
        if changed != 1 {
            return Err(mismatch(
                operation,
                format!("table=ledger_frame_refs_extent through_sequence={before}"),
            ));
        }
        Ok(())
    })();
    let closed = if written.is_ok() {
        "RELEASE ledger_frame_refs"
    } else {
        "ROLLBACK TO ledger_frame_refs; RELEASE ledger_frame_refs"
    };
    connection
        .execute_batch(closed)
        .map_err(|error| sql_error(error, operation))
}

/// The extent row, when there is one (`from_sequence`, `through_sequence`).
pub(super) fn read_extent(connection: &Connection) -> GlobalLedgerResult<Option<(u64, u64)>> {
    let rows = read_rows_bound(connection, READ_EXTENT, &[], None, &mut 0, false)?;
    match rows.as_slice() {
        [] => Ok(None),
        [row] => match row.as_slice() {
            [SqlValue::Integer(from), SqlValue::Integer(through)] => {
                Ok(Some((decode(*from), decode(*through))))
            }
            _ => Err(mismatch(
                LedgerLocation::VerifyLedgerFrameRefs.location(),
                "table=ledger_frame_refs_extent".to_owned(),
            )),
        },
        _ => Err(mismatch(
            LedgerLocation::VerifyLedgerFrameRefs.location(),
            "table=ledger_frame_refs_extent".to_owned(),
        )),
    }
}

/// An extent is `1 <= from <= through + 1` and ends at or before the head.
fn extent_fits((from, through): (u64, u64), head: u64) -> bool {
    from != 0 && through <= head && from <= through.saturating_add(1)
}

/// [`extent_fits`], or `ledger_index_mismatch` of `operation`.
fn check_extent(
    (from, through): (u64, u64),
    head: u64,
    operation: Location,
) -> GlobalLedgerResult<()> {
    if !extent_fits((from, through), head) {
        return Err(mismatch(
            operation,
            format!(
                "table=ledger_frame_refs_extent from_sequence={from} through_sequence={through} head_sequence={head}"
            ),
        ));
    }
    Ok(())
}

/// `work` in one write transaction of `connection`, committed when it returns a value.
fn in_write<T>(
    connection: &mut Connection,
    operation: Location,
    work: impl FnOnce(&Connection) -> GlobalLedgerResult<T>,
) -> GlobalLedgerResult<T> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sql_error(error, operation))?;
    match work(&transaction) {
        Ok(value) => {
            transaction
                .commit()
                .map_err(|error| sql_error(error, operation))?;
            Ok(value)
        }
        Err(error) => Err(error.with_close_result(
            transaction
                .rollback()
                .map_err(|error| sql_error(error, operation)),
        )),
    }
}

/// The open's deadline, checked before each page: `ledger_read_budget_exceeded` of the open, as
/// its retention reads report it.
fn check_deadline(deadline: Option<Instant>) -> GlobalLedgerResult<()> {
    match deadline {
        Some(deadline) if Instant::now() >= deadline => Err(GlobalLedgerError::request(
            LedgerCode::LedgerReadBudgetExceeded.code(),
            LedgerLocation::OpenRuntimeLedger.location(),
        )
        .with_detail("deadline reached at the open's frame index fill".to_owned())),
        _ => Ok(()),
    }
}

/// The writer open's step (`final-query.md` §3.3), after the views and indexes exist and before
/// any record is appended; `head` is the head the open verified. It never fails: it returns the
/// open's one warning about the index, if any ([`LedgerFrameRefsNote`]).
///
/// It creates the tables a root lacks. An untrusted extent (missing on an existing table, past
/// the head or malformed) is removed with every row, and the index is rebuilt from the extent
/// `head + 1..=head`; rows outside the extent are removed. Then the gap `(through_sequence, head]`
/// is filled upwards and the backfill goes down to 1, in pages of at most `page_rows` pointer
/// records, each committed with the extent moved; afterwards the extent is `1..=head`. A failure
/// (a record's authentication, the deadline, a statement) stops the fill where it is: the page in
/// progress rolls back, and the next open goes on from the extent left.
pub(super) fn complete(
    database: &RuntimeDatabase,
    head: u64,
    deadline: Option<Instant>,
    page_rows: usize,
) -> Option<LedgerFrameRefsNote> {
    let mut found = None;
    let mut at = None;
    match fill_index(
        database,
        head,
        deadline,
        page_rows.max(1),
        &mut found,
        &mut at,
    ) {
        Ok(()) => found,
        Err(error) => Some(LedgerFrameRefsNote {
            stage: LedgerLocation::BuildLedgerFrameRefs,
            sequence: at,
            cause: Some((error.code(), error.operation())),
        }),
    }
}

/// [`complete`]'s work: `found` gets what the open found (an untrusted extent, a gap), `at` the
/// sequence the fill is at when it stops.
fn fill_index(
    database: &RuntimeDatabase,
    head: u64,
    deadline: Option<Instant>,
    rows: usize,
    found: &mut Option<LedgerFrameRefsNote>,
    at: &mut Option<u64>,
) -> GlobalLedgerResult<()> {
    let operation = LedgerLocation::BuildLedgerFrameRefs.location();
    let mut connection = database.connection(operation)?;
    let existed: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name='ledger_frame_refs_extent'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| sql_error(error, operation))?;
    connection
        .execute_batch(SCHEMA)
        .map_err(|error| sql_error(error, operation))?;
    let (mut from, mut through) = in_write(&mut connection, operation, |transaction| {
        let stored = read_extent(transaction)?;
        let extent = match stored {
            Some(extent) if extent_fits(extent, head) => extent,
            _ => {
                // An untrusted extent, or none on a table that exists: every row goes and the
                // index is rebuilt. A table this open created starts empty, without a warning.
                if stored.is_some() || existed != 0 {
                    *found = Some(LedgerFrameRefsNote {
                        stage: LedgerLocation::VerifyLedgerFrameRefs,
                        sequence: stored.map(|(_, through)| through),
                        cause: None,
                    });
                }
                for sql in [
                    "DELETE FROM ledger_frame_refs",
                    "DELETE FROM ledger_frame_refs_extent",
                ] {
                    transaction
                        .execute(sql, [])
                        .map_err(|error| sql_error(error, operation))?;
                }
                let top = increment_sequence(head)?;
                insert_extent(transaction, top, head)?;
                (top, head)
            }
        };
        // Rows outside the extent are never trusted; each statement is one range of the index.
        for (sql, bound) in [
            ("DELETE FROM ledger_frame_refs WHERE sequence<?1", extent.0),
            ("DELETE FROM ledger_frame_refs WHERE sequence>?1", extent.1),
        ] {
            transaction
                .execute(sql, params_from_iter([integer(bound)]))
                .map_err(|error| sql_error(error, operation))?;
        }
        Ok(extent)
    })?;
    if through < head && found.is_none() {
        // Records appended without their rows: an older build's, or batches the extent did not
        // end before.
        *found = Some(LedgerFrameRefsNote {
            stage: LedgerLocation::WriteLedgerFrameRefs,
            sequence: Some(through),
            cause: None,
        });
    }
    while through < head {
        *at = Some(through.saturating_add(1));
        check_deadline(deadline)?;
        through = in_write(&mut connection, operation, |transaction| {
            let (sequences, upper) =
                select_page(transaction, Direction::Up, (through, head), rows)?;
            fill(database, transaction, &sequences, at)?;
            move_extent(transaction, "through_sequence", through, upper)?;
            Ok(upper)
        })?;
    }
    while from > 1 {
        *at = Some(from - 1);
        check_deadline(deadline)?;
        from = in_write(&mut connection, operation, |transaction| {
            let (sequences, lower) = select_page(transaction, Direction::Down, (1, from), rows)?;
            fill(database, transaction, &sequences, at)?;
            move_extent(transaction, "from_sequence", from, lower)?;
            Ok(lower)
        })?;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// The sequences `low < sequence <= high`, from the lowest.
    Up,
    /// The sequences `low <= sequence < high`, from the highest.
    Down,
}

/// The pointer records of one page, in ledger order, and the bound the page covers to: up, the
/// highest sequence it covers (`high` when the range has no more pointer records); down, the
/// lowest (`low` likewise). Each type is read in the type index with a sequence bound, ordered and
/// limited there; the page is the first `rows` of their union, so every pointer record between
/// the range's start and the bound is in it.
fn select_page(
    connection: &Connection,
    direction: Direction,
    (low, high): (u64, u64),
    rows: usize,
) -> GlobalLedgerResult<(Vec<u64>, u64)> {
    let sql = match direction {
        Direction::Up => {
            "SELECT sequence FROM ledger_events INDEXED BY ledger_view_index_type_v1 WHERE event_type=?1 AND sequence>?2 AND sequence<=?3 ORDER BY sequence LIMIT ?4"
        }
        Direction::Down => {
            "SELECT sequence FROM ledger_events INDEXED BY ledger_view_index_type_v1 WHERE event_type=?1 AND sequence>=?2 AND sequence<?3 ORDER BY sequence DESC LIMIT ?4"
        }
    };
    let limit = SqlValue::Integer(i64::try_from(rows).unwrap_or(i64::MAX));
    let mut selected = Vec::new();
    for event_type in POINTER_TYPES {
        let values = [
            SqlValue::Text(views::key(event_type)?),
            integer(low),
            integer(high),
            limit.clone(),
        ];
        for row in read_rows_bound(connection, sql, &values, None, &mut 0, false)? {
            let [SqlValue::Integer(sequence)] = row.as_slice() else {
                return Err(failure(
                    LedgerCode::InvalidEventInteger.code(),
                    LedgerLocation::BuildLedgerFrameRefs.location(),
                ));
            };
            selected.push(decode(*sequence));
        }
    }
    match direction {
        Direction::Up => selected.sort_unstable(),
        Direction::Down => selected.sort_unstable_by(|left, right| right.cmp(left)),
    }
    let bound = if selected.len() >= rows {
        selected.truncate(rows);
        selected[rows - 1]
    } else {
        match direction {
            Direction::Up => high,
            Direction::Down => low,
        }
    };
    selected.sort_unstable();
    Ok((selected, bound))
}

/// The rows of the records `sequences` (ascending), each record read and authenticated on its own
/// with its link and artifact rows, as a history page reads them; `at` is the record being
/// authenticated.
fn fill(
    database: &RuntimeDatabase,
    connection: &Connection,
    sequences: &[u64],
    at: &mut Option<u64>,
) -> GlobalLedgerResult<()> {
    if sequences.is_empty() {
        return Ok(());
    }
    let values = sequences
        .iter()
        .map(|sequence| integer(*sequence))
        .collect::<Vec<_>>();
    let placeholders = vec!["?"; values.len()].join(",");
    let mut reader = RowReader::new(database, connection, None);
    let rows = read_rows_bound(
        connection,
        &format!(
            "SELECT {EVENT_COLUMNS} FROM ledger_events WHERE sequence IN ({placeholders}) ORDER BY sequence"
        ),
        &values,
        None,
        &mut reader.bytes,
        true,
    )?;
    if rows.len() != sequences.len() {
        return Err(failure(
            LedgerCode::LedgerRecordMissing.code(),
            LedgerLocation::BuildLedgerFrameRefs.location(),
        ));
    }
    let mut events: Vec<LedgerEventMetadata> = Vec::with_capacity(rows.len());
    let mut expected_links: Vec<SqlRow> = Vec::with_capacity(rows.len());
    let mut expected_artifacts: Vec<SqlRow> = Vec::new();
    for row in rows {
        if let Some(SqlValue::Integer(sequence)) = row.first() {
            *at = Some(decode(*sequence));
        }
        let (event, links, artifacts) = reader.authenticate(row, events.len() + 1)?;
        expected_links.push(links);
        expected_artifacts.extend(artifacts);
        events.push(event);
    }
    let stored_links = read_rows_bound(
        connection,
        &format!(
            "SELECT {LINK_COLUMNS} FROM ledger_links WHERE sequence IN ({placeholders}) ORDER BY sequence"
        ),
        &values,
        None,
        &mut reader.bytes,
        false,
    )?;
    let stored_artifacts = read_rows_bound(
        connection,
        &format!(
            "SELECT {ARTIFACT_COLUMNS} FROM ledger_artifacts WHERE sequence IN ({placeholders}) ORDER BY sequence,ordinal"
        ),
        &values,
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
    for event in &events {
        insert_refs(connection, event.sequence(), &frame_refs(event)?)?;
    }
    Ok(())
}

/// Moves one end of the extent from `old` to `new`; an extent no longer at `old` is
/// `ledger_index_mismatch`.
fn move_extent(
    connection: &Connection,
    column: &'static str,
    old: u64,
    new: u64,
) -> GlobalLedgerResult<()> {
    let operation = LedgerLocation::BuildLedgerFrameRefs.location();
    let changed = connection
        .execute(
            &format!(
                "UPDATE ledger_frame_refs_extent SET {column}=?1 WHERE singleton=1 AND {column}=?2"
            ),
            params_from_iter([integer(new), integer(old)]),
        )
        .map_err(|error| sql_error(error, operation))?;
    if changed != 1 {
        return Err(mismatch(
            operation,
            format!("table=ledger_frame_refs_extent {column}={old}"),
        ));
    }
    Ok(())
}

/// The stored rows with a sequence in `after < sequence <= through` (all rows for `None`), in
/// key order.
fn stored_rows(
    connection: &Connection,
    range: Option<(u64, u64)>,
) -> GlobalLedgerResult<Vec<StoredRef>> {
    let (sql, values) = match range {
        Some((after, through)) => (
            format!(
                "SELECT {REF_COLUMNS} FROM ledger_frame_refs INDEXED BY ledger_frame_refs_sequence WHERE sequence>?1 AND sequence<=?2 {REF_ORDER}"
            ),
            vec![integer(after), integer(through)],
        ),
        None => (
            format!(
                "SELECT {REF_COLUMNS} FROM ledger_frame_refs INDEXED BY ledger_frame_refs_sequence {REF_ORDER}"
            ),
            Vec::new(),
        ),
    };
    read_rows_bound(connection, &sql, &values, None, &mut 0, false)?
        .into_iter()
        .map(|row| match <[SqlValue; 5]>::try_from(row) {
            Ok(
                [
                    SqlValue::Integer(sequence),
                    SqlValue::Text(kind),
                    SqlValue::Text(target),
                    SqlValue::Text(pointer),
                    run,
                ],
            ) => {
                let run = match run {
                    SqlValue::Null => None,
                    SqlValue::Text(run) => Some(run),
                    _ => return Err(row_mismatch(decode(sequence))),
                };
                Ok((decode(sequence), kind, target, pointer, run))
            }
            _ => Err(mismatch(
                LedgerLocation::VerifyLedgerFrameRefs.location(),
                "table=ledger_frame_refs".to_owned(),
            )),
        })
        .collect()
}

fn row_mismatch(sequence: u64) -> GlobalLedgerError {
    mismatch(
        LedgerLocation::VerifyLedgerFrameRefs.location(),
        format!("table=ledger_frame_refs sequence={sequence}"),
    )
}

/// The rows `expected` (each record's [`frame_refs`] by its sequence, ascending) equal `stored`;
/// otherwise `ledger_index_mismatch` naming the first sequence that differs.
fn compare(expected: &[(u64, Vec<FrameRef>)], stored: &[StoredRef]) -> GlobalLedgerResult<()> {
    let mut wanted = expected.iter().flat_map(|(sequence, refs)| {
        refs.iter().map(move |frame_ref| {
            (
                *sequence,
                frame_ref.target_kind,
                frame_ref.target_id.as_str(),
                frame_ref.pointer,
                frame_ref.run_id.as_deref(),
            )
        })
    });
    let mut found = stored.iter();
    loop {
        match (wanted.next(), found.next()) {
            (None, None) => return Ok(()),
            (Some(want), Some(row))
                if want
                    == (
                        row.0,
                        row.1.as_str(),
                        row.2.as_str(),
                        row.3.as_str(),
                        row.4.as_deref(),
                    ) => {}
            (want, row) => {
                let sequence = match (want, row) {
                    (Some(want), Some(row)) => want.0.min(row.0),
                    (Some(want), None) => want.0,
                    (None, Some(row)) => row.0,
                    (None, None) => 0,
                };
                return Err(row_mismatch(sequence));
            }
        }
    }
}

/// #422 LN-7c R: marks the extent untrusted after a check found the index wrong: the extent becomes
/// `through + 1..=through`, which covers no record. Appends go on extending it, and the next
/// writer open removes every row below it and rebuilds them by its backfill; a later query never
/// trusts a row outside the extent. One statement on the shared connection.
pub(super) fn mark_untrusted(database: &RuntimeDatabase) -> GlobalLedgerResult<()> {
    let operation = LedgerLocation::VerifyLedgerFrameRefs.location();
    let connection = database.connection(operation)?;
    connection
        .execute(
            "UPDATE ledger_frame_refs_extent SET from_sequence=through_sequence+1 WHERE singleton=1",
            [],
        )
        .map_err(|error| sql_error(error, operation))?;
    Ok(())
}

/// The scrub's non-fatal finding about the derived index: `ledger_index_mismatch` of
/// `verify_ledger_frame_refs` with what and where (`error`'s detail, and its code when it is
/// another), `context`, and whether the extent was marked untrusted.
pub(super) fn untrusted_finding(
    error: &GlobalLedgerError,
    marked: &GlobalLedgerResult<()>,
    context: &str,
) -> GlobalLedgerError {
    let mut detail = error
        .detail()
        .unwrap_or("table=ledger_frame_refs")
        .to_owned();
    if error.code() != LedgerCode::LedgerIndexMismatch.as_str() {
        detail.push_str(&format!(
            "; cause={} cause_operation={}",
            error.code(),
            error.operation()
        ));
    }
    detail.push_str(&format!("; {context}"));
    match marked {
        Ok(()) => detail.push_str("; extent=untrusted"),
        Err(mark) => detail.push_str(&format!("; extent_mark_failed={}", mark.code())),
    }
    GlobalLedgerError::request(
        LedgerCode::LedgerIndexMismatch.code(),
        LedgerLocation::VerifyLedgerFrameRefs.location(),
    )
    .with_detail(detail)
}

/// LN-9's scrub, in the read transaction of one chunk: the rows of the records `after < sequence
/// <= read` inside `extent` equal `expected`, their [`frame_refs`] as the chunk authenticated them.
pub(super) fn check_chunk(
    connection: &Connection,
    (from, through): (u64, u64),
    (after, read): (u64, u64),
    expected: &[(u64, Vec<FrameRef>)],
) -> GlobalLedgerResult<()> {
    let low = after.max(from.saturating_sub(1));
    let high = read.min(through);
    if low >= high {
        return Ok(());
    }
    let stored = stored_rows(connection, Some((low, high)))?;
    compare(expected, &stored)
}

/// What `ledger-maintenance verify` read of the frame index in its snapshot: the extent and every
/// row.
pub(super) struct StoredFrameRefs {
    extent: (u64, u64),
    rows: Vec<StoredRef>,
}

/// The extent and the rows in the caller's read transaction; `None` for a root that has no extent
/// (no writer of this release opened it, or its extent is gone), whose rows are not trusted.
pub(super) fn read_stored(connection: &Connection) -> GlobalLedgerResult<Option<StoredFrameRefs>> {
    let operation = LedgerLocation::VerifyLedgerFrameRefs.location();
    let tables: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name='ledger_frame_refs_extent'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| sql_error(error, operation))?;
    if tables == 0 {
        return Ok(None);
    }
    let Some(extent) = read_extent(connection)? else {
        return Ok(None);
    };
    Ok(Some(StoredFrameRefs {
        extent,
        rows: stored_rows(connection, None)?,
    }))
}

/// `ledger-maintenance verify`: the extent ends at or before the head of `events` (the whole
/// verified ledger), and the rows inside it are those [`frame_refs`] gives its records.
pub(super) fn verify_stored(
    events: &[PersistedEvent],
    stored: &StoredFrameRefs,
) -> GlobalLedgerResult<()> {
    let head = events.last().map_or(0, PersistedEvent::sequence);
    let (from, through) = stored.extent;
    check_extent(
        stored.extent,
        head,
        LedgerLocation::VerifyLedgerFrameRefs.location(),
    )?;
    let mut expected = Vec::new();
    for event in events {
        if (from..=through).contains(&event.sequence()) {
            expected.push((event.sequence(), frame_refs(event)?));
        }
    }
    let inside = stored
        .rows
        .iter()
        .filter(|row| (from..=through).contains(&row.0))
        .cloned()
        .collect::<Vec<_>>();
    compare(&expected, &inside)
}

/// Tests: the roots whose writers write no frame index, as an older build's writer does.
#[cfg(test)]
fn skipping_roots() -> &'static std::sync::Mutex<Vec<std::path::PathBuf>> {
    static ROOTS: std::sync::OnceLock<std::sync::Mutex<Vec<std::path::PathBuf>>> =
        std::sync::OnceLock::new();
    ROOTS.get_or_init(Default::default)
}

#[cfg(test)]
fn skipped(root: &Path) -> bool {
    skipping_roots()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|skipping| skipping == root)
}

#[cfg(not(test))]
fn skipped(_root: &Path) -> bool {
    false
}

/// Tests: while armed for a state root, every batch the writer (or the synthetic appender) commits
/// there writes no frame index row and leaves the extent where it is, as an older build does.
#[cfg(test)]
pub(crate) struct LedgerFrameRefsSkipGate {
    root: std::path::PathBuf,
}

#[cfg(test)]
impl LedgerFrameRefsSkipGate {
    /// Arms the gate for `state_root`, an existing directory.
    pub(crate) fn arm(state_root: &Path) -> std::io::Result<Self> {
        let root = state_root.canonicalize()?;
        skipping_roots()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(root.clone());
        Ok(Self { root })
    }
}

#[cfg(test)]
impl Drop for LedgerFrameRefsSkipGate {
    fn drop(&mut self) {
        let mut roots = skipping_roots()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(position) = roots.iter().position(|root| *root == self.root) {
            roots.remove(position);
        }
    }
}
