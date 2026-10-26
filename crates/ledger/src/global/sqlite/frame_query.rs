// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-7c Q (`rd-1026/ln7c/final-query.md` §1; Alice's ruling 70: frame retention by ledger
//! query): the frame cleaner's queries. No retention state is kept anywhere: each pass asks the
//! ledger by its indexes, on a read-only connection of its own, never on the writer thread.
//!
//! - **The statements.** W (`frame_query/waits.sql`) answers the open runs of the current owner
//!   epoch once per pass; L (`live.sql`) answers each live frame's class and action for one chunk
//!   of the live listing; K (`kept.sql`) answers the evidence-frame rule for one chunk of the kept
//!   listing. Every access is pinned (`INDEXED BY`, `CROSS JOIN`, unary `+`, `MATERIALIZED`), so a
//!   missing index fails the statement instead of reading the whole history; their plans on an
//!   empty database are held to the §1.3 rule and to golden texts (`plan_*.txt`, test T-plan).
//! - **One read transaction per statement.** No snapshot is held from one statement to the next.
//!   In each, the statement runs only when the frame index `ledger_frame_refs` covers every record
//!   (its extent is `1..=` the snapshot's head); otherwise nothing is decided
//!   ([`FrameQueryOutcome::Incomplete`], the pass line `skipped=frame_refs_incomplete`).
//! - **The work.** Each answer carries its statement's SQLite virtual-machine steps, which the
//!   cleaner paces at `ledger_observer.frame_query_steps_per_second`.
//! - **The stop.** The query's stop is watched inside every statement (`sqlite3_progress_handler`
//!   on the query's own connection, every `ledger_observer.read_check_vm_steps` steps): a raised
//!   stop interrupts the statement in progress ([`FrameQueryOutcome::Stopped`]).
//! - **Acting.** Before a frame is removed or moved, its own `artifact.verified` row is read and
//!   authenticated ([`FrameQuery::frame`]); a moved frame's kept folder is named by the view's
//!   rules from its owner point, read by one keyed statement (`owner.sql`), the instance's alias at
//!   that point and the point's leaf code ([`FrameQuery::folder`]).
//!
//! A statement that fails is not fatal: it is the cleaner's error of the round (the scrub and
//! `ledger-maintenance verify` check the index). Only a record that fails its authentication is.

use super::{
    ARTIFACT_COLUMNS, EVENT_COLUMNS, LINK_COLUMNS, RowReader, SqlRow, decode, encode, failure,
    frame_refs, has_schema, read_rows_bound, sql_error, views,
};
use crate::codes::{LedgerCode, LedgerLocation};
use crate::fact::{LedgerEventMetadata, LedgerEventRead};
use crate::global::retention::{
    DAY_MS, ERROR_WINDOW_MS, LocalOffsetMs, READING_DAYS, SETTLE_AGE_MS, first_string, id_text,
    kept_file_name, leaf_name, leaf_part, local_date, type_leaf_name,
};
use crate::global::{
    FrameRetentionClass, FrameRetentionSwitches, GlobalLedgerError, GlobalLedgerResult,
    KeptFrameFolder,
};
use actingcommand_contract::outcome::Location;
use actingcommand_contract::{
    ArtifactId, ArtifactKind, ClientPayload, EventActor, EventPayload, EventSource, EventType,
    InstanceId, ProjectedArtifactReference, RunId, RuntimePayload, TaskOutcome, TaskPayload,
    TaskSemanticFact,
};
use actingcommand_runtime_database::RuntimeDatabase;
use rusqlite::{Connection, TransactionBehavior, types::Value as SqlValue};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

// #422 LN-7c V: `ledger-maintenance verify`'s check of these statements against the full view.
mod check;
pub(in crate::global) use check::check_frame_queries;
pub use check::{FrameQueryCheck, FrameQueryCheckSettings};

/// Statement W: the open runs of the current owner epoch.
pub(in crate::global) const WAITS: &str = include_str!("frame_query/waits.sql");
/// Statement L: the live frames' classes and actions, per chunk.
pub(in crate::global) const LIVE: &str = include_str!("frame_query/live.sql");
/// Statement K: the evidence-frame rule of the kept frames, per chunk.
pub(in crate::global) const KEPT: &str = include_str!("frame_query/kept.sql");
/// The owner point of one frame to move into an error folder.
pub(in crate::global) const OWNER: &str = include_str!("frame_query/owner.sql");

/// A frame's own `artifact.verified` row.
const VERIFIED_SEQUENCE: &str = "SELECT a.sequence FROM ledger_artifacts a INDEXED BY ledger_artifacts_artifact CROSS JOIN ledger_events e ON e.sequence = a.sequence WHERE a.artifact_id = ?1 AND a.kind = 'capture.frame' AND +e.event_type = 'artifact.verified' ORDER BY a.sequence LIMIT 1";
/// The latest binding of an instance at or before a sequence (its alias there).
const ALIAS_BINDING: &str = "SELECT e.sequence FROM ledger_events e INDEXED BY ledger_view_index_type_v1 CROSS JOIN ledger_links l ON l.sequence = e.sequence WHERE e.event_type = 'runtime.instance_bound' AND e.sequence > ?1 AND e.sequence <= ?2 AND l.instance_id = ?3 ORDER BY e.sequence DESC LIMIT 1";
/// A run's first terminal (`TerminalCommitted`).
const FIRST_TERMINAL: &str = "SELECT MIN(rl.sequence) FROM ledger_links rl INDEXED BY ledger_links_run CROSS JOIN ledger_events re ON re.sequence = rl.sequence WHERE rl.run_id = ?1 AND +re.event_type IN ('task.completed', 'task.failed', 'task.cancelled') AND json_extract(CAST(re.canonical_record AS TEXT), '$.payload.payload.kind') = 'semantic'";
/// The first read confirmation of a frame or its run at or after its capture.
const FIRST_CONFIRMATION: &str = "SELECT MIN(cr.sequence) FROM ledger_frame_refs cr CROSS JOIN ledger_events ce ON ce.sequence = cr.sequence WHERE ((cr.target_kind = 'artifact' AND cr.target_id = ?1) OR (cr.target_kind = 'run' AND cr.target_id = ?2)) AND cr.pointer = 'read_confirmed' AND ce.timestamp_unix_ms >= ?3";
/// The latest identity card declared in a correlation before a sequence.
const IDENTITY_CARD: &str = "SELECT MAX(gl.sequence) FROM ledger_links gl INDEXED BY ledger_links_correlation CROSS JOIN ledger_events ge ON ge.sequence = gl.sequence WHERE gl.correlation_id = ?1 AND gl.sequence < ?2 AND +ge.event_type = 'governance.identity_declared'";
/// The capture frames of a run.
const RUN_FRAMES: &str = "SELECT a.artifact_id FROM ledger_links l INDEXED BY ledger_links_run CROSS JOIN ledger_artifacts a ON a.sequence = l.sequence CROSS JOIN ledger_events e ON e.sequence = a.sequence WHERE l.run_id = ?1 AND a.kind = 'capture.frame' AND +e.event_type = 'artifact.verified'";
/// A record's time.
const RECORD_TIME: &str = "SELECT timestamp_unix_ms FROM ledger_events WHERE sequence = ?1";

/// The evidence age a confirmation's count uses: never reached.
const COVERS_MAX_AGE_MS: u64 = 3_650 * DAY_MS;

/// #422 LN-7c Q: the stop a frame query watches inside its statements.
pub type FrameQueryStop = Arc<dyn Fn() -> bool + Send + Sync>;

/// What one pass of the cleaner decides with besides the ledger: the clock, the two switches of
/// actingd-config revision 4 and `frame_retention_evidence_max_age_days` (in milliseconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameQuerySettings {
    pub now_unix_ms: u64,
    pub switches: FrameRetentionSwitches,
    pub evidence_max_age_ms: u64,
}

/// A frame's action in a statement's answer (final-query §1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameQueryAction {
    /// Not settled: nothing touches it.
    Running,
    /// A default frame younger than 1 day.
    Keep,
    /// A resource reading frame younger than 7 days.
    KeepResource,
    /// A Lab frame: moved into its Lab folder.
    MoveLab,
    /// An error frame: moved into the folder of its owner point.
    MoveError,
    /// A default frame of 1 day or a resource frame of 7 days: removed.
    Remove,
    /// An interior near-duplicate: removed.
    RemoveDuplicate,
    /// An evidence (Lab or error) frame whose reading an agent or a person confirmed: removed.
    RemoveConfirmed,
    /// A kept evidence frame of `frame_retention_evidence_max_age_days`: removed.
    RemoveExpired,
}

impl FrameQueryAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Keep => "keep",
            Self::KeepResource => "keep_resource",
            Self::MoveLab => "move_lab",
            Self::MoveError => "move_error",
            Self::Remove => "remove",
            Self::RemoveDuplicate => "remove_duplicate",
            Self::RemoveConfirmed => "remove_confirmed",
            Self::RemoveExpired => "remove_expired",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "running" => Self::Running,
            "keep" => Self::Keep,
            "keep_resource" => Self::KeepResource,
            "move_lab" => Self::MoveLab,
            "move_error" => Self::MoveError,
            "remove" => Self::Remove,
            "remove_duplicate" => Self::RemoveDuplicate,
            "remove_confirmed" => Self::RemoveConfirmed,
            "remove_expired" => Self::RemoveExpired,
            _ => return None,
        })
    }

    /// Whether the cleaner removes the frame.
    pub const fn removes(self) -> bool {
        matches!(
            self,
            Self::Remove | Self::RemoveDuplicate | Self::RemoveConfirmed | Self::RemoveExpired
        )
    }

    /// Whether the cleaner moves the frame into a kept folder.
    pub const fn moves(self) -> bool {
        matches!(self, Self::MoveLab | Self::MoveError)
    }
}

/// What one statement of a frame query gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameQueryOutcome<T> {
    /// The statement's answer and its SQLite virtual-machine steps.
    Answered { value: T, steps: u64 },
    /// The frame index `ledger_frame_refs` does not cover every record of the snapshot: nothing
    /// may be decided (`skipped=frame_refs_incomplete`).
    Incomplete,
    /// The query's stop was raised: the statement did not run or was interrupted.
    Stopped,
}

/// Statement W's answer: the instance and first frame time of each open run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrameWaits {
    /// `(instance key, first frame time as stored)`.
    rows: Vec<(String, i64)>,
}

impl FrameWaits {
    /// No open run (a test's waits).
    pub fn none() -> Self {
        Self::default()
    }

    /// The open runs that hold frames back.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// One live frame's row of statement L.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveFrameAnswer {
    pub artifact_id: ArtifactId,
    /// Its ingestion time, the time its due times count from (E1).
    pub created_at_unix_ms: u64,
    pub class: FrameRetentionClass,
    pub action: FrameQueryAction,
}

/// One kept frame's row of statement K.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptFrameAnswer {
    pub artifact_id: ArtifactId,
    pub created_at_unix_ms: u64,
    pub run_id: Option<RunId>,
    /// `keep`, `remove_confirmed` or `remove_expired`.
    pub action: FrameQueryAction,
}

/// A frame as its own authenticated `artifact.verified` record names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameQueryFrame {
    pub reference: ProjectedArtifactReference,
    pub instance_id: InstanceId,
    pub run_id: Option<RunId>,
    pub verified_sequence: u64,
}

/// Who confirmed reading an evidence frame (final-query §4): the confirming record's origin and
/// surface, the identity card declared in its correlation, its time and sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FrameConfirmation {
    pub actor: EventActor,
    pub source: EventSource,
    pub surface: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    pub at_unix_ms: u64,
    pub sequence: u64,
}

/// What a read confirmation names: one capture frame, or a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceTarget {
    Frame(ArtifactId),
    Run(RunId),
}

/// The evidence frames a read confirmation covers (final-query §4), counted by the cleaner's
/// classification and the kept listing: those in `kept\`, and those still live and classed Lab or
/// error, captured at or before the confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct EvidenceCovers {
    pub evidence_frames: u64,
    pub in_kept: u64,
}

/// #422 LN-7c Q: the frame cleaner's queries on one read-only connection of their own.
pub struct FrameQuery {
    database: Arc<RuntimeDatabase>,
    connection: Connection,
    stop: FrameQueryStop,
    local: LocalOffsetMs,
}

fn location() -> Location {
    LedgerLocation::QueryFrameRetention.location()
}

/// A failure of the query is the round's, not the Runtime's.
fn non_terminal(mut error: GlobalLedgerError) -> GlobalLedgerError {
    error.terminal = false;
    error
}

fn invalid_row(statement: &str) -> GlobalLedgerError {
    non_terminal(failure(LedgerCode::InvalidSqliteValue.code(), location()))
        .with_detail(format!("statement={statement}"))
}

fn clock_failed() -> GlobalLedgerError {
    GlobalLedgerError::request(LedgerCode::ArtifactRetentionClockFailed.code(), location())
}

fn integer(value: u64) -> SqlValue {
    SqlValue::Integer(i64::try_from(value).unwrap_or(i64::MAX))
}

fn stored(value: u64) -> SqlValue {
    SqlValue::Integer(encode(value))
}

/// An identifier from its canonical text.
fn identifier<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}

/// The ids as the JSON array a statement's `json_each` reads.
fn key_list<T: Serialize + Copy>(ids: &[T]) -> GlobalLedgerResult<String> {
    let keys = ids
        .iter()
        .map(|id| views::key(*id))
        .collect::<GlobalLedgerResult<Vec<_>>>()?;
    serde_json::to_string(&keys).map_err(|error| {
        GlobalLedgerError::json(
            LedgerCode::EventSerializationFailed.code(),
            location(),
            &error,
        )
    })
}

/// The one integer of a one-column answer, `None` for no row or NULL.
fn first_integer(rows: &[SqlRow]) -> Option<i64> {
    match rows.first().map(Vec::as_slice) {
        Some([SqlValue::Integer(value)]) => Some(*value),
        _ => None,
    }
}

/// Whether the frame index covers every record of this snapshot: its extent is `1..=head`. An
/// extent that cannot be read (absent, malformed, a root before the index) covers nothing.
fn covers_head(connection: &Connection) -> GlobalLedgerResult<bool> {
    let head = connection
        .query_row("SELECT MAX(sequence) FROM ledger_events", [], |row| {
            row.get::<_, Option<i64>>(0)
        })
        .map_err(|error| non_terminal(sql_error(error, location())))?
        .map_or(0, decode);
    if head == 0 {
        return Ok(true);
    }
    Ok(matches!(
        frame_refs::read_extent(connection),
        Ok(Some((1, through))) if through >= head
    ))
}

/// One statement's rows, its named parameters bound where it names them, and its steps.
fn statement_rows(
    connection: &Connection,
    sql: &str,
    parameters: &[(&str, SqlValue)],
) -> GlobalLedgerResult<(Vec<SqlRow>, u64)> {
    let sql_failed = |error| non_terminal(sql_error(error, location()));
    let mut statement = connection.prepare(sql).map_err(sql_failed)?;
    for (name, value) in parameters {
        if let Some(index) = statement.parameter_index(name).map_err(sql_failed)? {
            statement
                .raw_bind_parameter(index, value)
                .map_err(sql_failed)?;
        }
    }
    let columns = statement.column_count();
    let mut found = Vec::new();
    let mut rows = statement.raw_query();
    while let Some(row) = rows.next().map_err(sql_failed)? {
        let mut values = Vec::with_capacity(columns);
        for index in 0..columns {
            values.push(row.get::<_, SqlValue>(index).map_err(sql_failed)?);
        }
        found.push(values);
    }
    drop(rows);
    let steps = u64::try_from(statement.get_status(rusqlite::StatementStatus::VmStep)).unwrap_or(0);
    Ok((found, steps))
}

/// The record at `sequence`, read and authenticated on its own with its link and artifact rows,
/// as a range read authenticates its head.
fn authenticated_row(
    database: &RuntimeDatabase,
    connection: &Connection,
    sequence: u64,
) -> GlobalLedgerResult<LedgerEventMetadata> {
    let value = [stored(sequence)];
    let mut reader = RowReader::new(database, connection, None);
    let rows = read_rows_bound(
        connection,
        &format!("SELECT {EVENT_COLUMNS} FROM ledger_events WHERE sequence=?"),
        &value,
        None,
        &mut reader.bytes,
        true,
    )?;
    let [row] = <[SqlRow; 1]>::try_from(rows).map_err(|_| {
        failure(
            LedgerCode::SequenceDiscontinuity.code(),
            LedgerLocation::RecoverSequence.location(),
        )
    })?;
    let (event, links, artifacts) = reader.authenticate(row, 1)?;
    let stored_links = read_rows_bound(
        connection,
        &format!("SELECT {LINK_COLUMNS} FROM ledger_links WHERE sequence=?"),
        &value,
        None,
        &mut reader.bytes,
        false,
    )?;
    let stored_artifacts = read_rows_bound(
        connection,
        &format!(
            "SELECT {ARTIFACT_COLUMNS} FROM ledger_artifacts WHERE sequence=? ORDER BY ordinal"
        ),
        &value,
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
    Ok(event)
}

/// The instance's alias bound at `through` (the latest binding at or before it), made safe for a
/// folder name; its id when no binding precedes it (the view's `alias`).
fn alias_at(
    database: &RuntimeDatabase,
    connection: &Connection,
    instance: &InstanceId,
    through: u64,
) -> GlobalLedgerResult<String> {
    let rows = read_rows_bound(
        connection,
        ALIAS_BINDING,
        &[
            stored(0),
            stored(through),
            SqlValue::Text(views::key(instance)?),
        ],
        None,
        &mut 0,
        false,
    )?;
    let bound = match first_integer(&rows) {
        Some(sequence) => {
            match authenticated_row(database, connection, decode(sequence))?.payload() {
                EventPayload::Runtime(RuntimePayload::InstanceBound(binding)) => {
                    Some(binding.instance_alias().to_owned())
                }
                _ => None,
            }
        }
        None => None,
    };
    Ok(leaf_part(
        &bound.unwrap_or_else(|| id_text(instance)),
        usize::MAX,
    ))
}

/// The failure code of `run`'s first terminal when it is a failure and precedes `before`.
fn preceding_failure(
    database: &RuntimeDatabase,
    connection: &Connection,
    run: &RunId,
    before: u64,
) -> GlobalLedgerResult<Option<String>> {
    let rows = read_rows_bound(
        connection,
        FIRST_TERMINAL,
        &[SqlValue::Text(views::key(run)?)],
        None,
        &mut 0,
        false,
    )?;
    let Some(terminal) = first_integer(&rows)
        .map(decode)
        .filter(|terminal| *terminal < before)
    else {
        return Ok(None);
    };
    let event = authenticated_row(database, connection, terminal)?;
    Ok(match event.payload() {
        EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
            TaskSemanticFact::TerminalCommitted {
                task_result: TaskOutcome::Failure,
                outcome,
                ..
            } => outcome
                .as_ref()
                .map(|outcome| outcome.code().as_str().to_owned()),
            _ => None,
        },
        _ => None,
    })
}

impl FrameQuery {
    /// A query on a read-only connection of its own to `database`; `stop` is watched every
    /// `check_vm_steps` steps of a statement.
    pub(in crate::global) fn open(
        database: Arc<RuntimeDatabase>,
        stop: FrameQueryStop,
        check_vm_steps: u32,
    ) -> GlobalLedgerResult<Self> {
        let connection = database
            .read_snapshot_connection(location())
            .map_err(|error| non_terminal(error.into()))?;
        let watched = Arc::clone(&stop);
        connection
            .progress_handler(
                i32::try_from(check_vm_steps.max(1)).unwrap_or(i32::MAX),
                Some(move || watched()),
            )
            .map_err(|error| non_terminal(sql_error(error, location())))?;
        Ok(Self {
            database,
            connection,
            stop,
            local: crate::local_time::machine_local_offset_ms,
        })
    }

    /// A query on the state database of `state_root`, read-only, without a stop: the offline
    /// frame listing (`actingledger frames`) and a confirmation's count. `check_vm_steps` is the
    /// caller's `ledger_observer.read_check_vm_steps` (review F4: the offline tools read it from
    /// the key table, not from a constant of this crate).
    pub fn open_state_root(state_root: &Path, check_vm_steps: u32) -> GlobalLedgerResult<Self> {
        let database = RuntimeDatabase::open_existing_unchecked(state_root)?;
        if !has_schema(&database)? {
            return Err(GlobalLedgerError::request(
                LedgerCode::LedgerMetaMissing.code(),
                location(),
            ));
        }
        Self::open(Arc::new(database), Arc::new(|| false), check_vm_steps)
    }

    /// One statement in a read transaction of its own, run only while the frame index covers the
    /// snapshot and the stop is not raised.
    fn run<T>(
        &mut self,
        sql: &str,
        parameters: &[(&str, SqlValue)],
        answer: impl FnOnce(Vec<SqlRow>) -> GlobalLedgerResult<T>,
    ) -> GlobalLedgerResult<FrameQueryOutcome<T>> {
        if (self.stop)() {
            return Ok(FrameQueryOutcome::Stopped);
        }
        let read = (|| -> GlobalLedgerResult<Option<(Vec<SqlRow>, u64)>> {
            let transaction = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(|error| non_terminal(sql_error(error, location())))?;
            if !covers_head(&transaction)? {
                return Ok(None);
            }
            let rows = statement_rows(&transaction, sql, parameters)?;
            transaction
                .commit()
                .map_err(|error| non_terminal(sql_error(error, location())))?;
            Ok(Some(rows))
        })();
        match read {
            Ok(Some((rows, steps))) => Ok(FrameQueryOutcome::Answered {
                value: answer(rows)?,
                steps,
            }),
            Ok(None) => Ok(FrameQueryOutcome::Incomplete),
            // An interrupted statement is the stop's, not a failure.
            Err(_) if (self.stop)() => Ok(FrameQueryOutcome::Stopped),
            Err(error) => Err(error),
        }
    }

    /// `read` in a read transaction of its own.
    fn read<T>(
        &mut self,
        read: impl FnOnce(&Connection) -> GlobalLedgerResult<T>,
    ) -> GlobalLedgerResult<T> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|error| non_terminal(sql_error(error, location())))?;
        let value = read(&transaction)?;
        transaction
            .commit()
            .map_err(|error| non_terminal(sql_error(error, location())))?;
        Ok(value)
    }

    /// Statement W, once per pass.
    pub fn waits(&mut self) -> GlobalLedgerResult<FrameQueryOutcome<FrameWaits>> {
        self.run(WAITS, &[], |rows| {
            let mut waits = FrameWaits::default();
            for row in rows {
                match row.as_slice() {
                    [SqlValue::Text(instance), SqlValue::Integer(first_at)] => {
                        waits.rows.push((instance.clone(), *first_at));
                    }
                    // A run with no instance link holds no instance's frames back.
                    [SqlValue::Null, SqlValue::Integer(_)] => {}
                    _ => return Err(invalid_row("waits")),
                }
            }
            Ok(waits)
        })
    }

    /// Statement L for the live frames `artifacts` (one chunk) with the pass's `waits`.
    pub fn live(
        &mut self,
        artifacts: &[ArtifactId],
        waits: &FrameWaits,
        settings: &FrameQuerySettings,
    ) -> GlobalLedgerResult<FrameQueryOutcome<Vec<LiveFrameAnswer>>> {
        self.live_statement(LIVE, artifacts, waits, settings)
    }

    /// `live` with the statement text `sql`: statement L, or in a ledger test a variant of it
    /// that switches one of its terms off (review F3).
    pub(in crate::global) fn live_statement(
        &mut self,
        sql: &str,
        artifacts: &[ArtifactId],
        waits: &FrameWaits,
        settings: &FrameQuerySettings,
    ) -> GlobalLedgerResult<FrameQueryOutcome<Vec<LiveFrameAnswer>>> {
        let waits = serde_json::to_string(&waits.rows).map_err(|error| {
            GlobalLedgerError::json(
                LedgerCode::EventSerializationFailed.code(),
                location(),
                &error,
            )
        })?;
        let parameters = [
            (":live", SqlValue::Text(key_list(artifacts)?)),
            (":waits", SqlValue::Text(waits)),
            (":now", stored(settings.now_unix_ms)),
            (":settle_ms", integer(SETTLE_AGE_MS)),
            (":window_ms", integer(ERROR_WINDOW_MS)),
            (":default_ms", integer(DAY_MS)),
            (":resource_ms", integer(READING_DAYS * DAY_MS)),
            (
                ":dedup_error",
                SqlValue::Integer(i64::from(settings.switches.dedup_error)),
            ),
            (
                ":dedup_lab",
                SqlValue::Integer(i64::from(settings.switches.dedup_lab)),
            ),
        ];
        self.run(sql, &parameters, |rows| {
            rows.into_iter()
                .map(|row| match row.as_slice() {
                    [
                        SqlValue::Text(artifact),
                        SqlValue::Integer(at),
                        SqlValue::Text(class),
                        SqlValue::Text(action),
                    ] => Ok(LiveFrameAnswer {
                        artifact_id: identifier(artifact).ok_or_else(|| invalid_row("live"))?,
                        created_at_unix_ms: decode(*at),
                        class: class_of(class).ok_or_else(|| invalid_row("live"))?,
                        action: FrameQueryAction::parse(action)
                            .ok_or_else(|| invalid_row("live"))?,
                    }),
                    _ => Err(invalid_row("live")),
                })
                .collect()
        })
    }

    /// Statement K for the kept frames `artifacts` (one chunk).
    pub fn kept(
        &mut self,
        artifacts: &[ArtifactId],
        settings: &FrameQuerySettings,
    ) -> GlobalLedgerResult<FrameQueryOutcome<Vec<KeptFrameAnswer>>> {
        let parameters = [
            (":kept", SqlValue::Text(key_list(artifacts)?)),
            (":now", stored(settings.now_unix_ms)),
            (":max_age_ms", integer(settings.evidence_max_age_ms)),
        ];
        self.run(KEPT, &parameters, |rows| {
            rows.into_iter()
                .map(|row| match row.as_slice() {
                    [
                        SqlValue::Text(artifact),
                        SqlValue::Integer(at),
                        run,
                        SqlValue::Text(action),
                    ] => Ok(KeptFrameAnswer {
                        artifact_id: identifier(artifact).ok_or_else(|| invalid_row("kept"))?,
                        created_at_unix_ms: decode(*at),
                        run_id: match run {
                            SqlValue::Text(run) => {
                                Some(identifier(run).ok_or_else(|| invalid_row("kept"))?)
                            }
                            SqlValue::Null => None,
                            _ => return Err(invalid_row("kept")),
                        },
                        action: FrameQueryAction::parse(action)
                            .ok_or_else(|| invalid_row("kept"))?,
                    }),
                    _ => Err(invalid_row("kept")),
                })
                .collect()
        })
    }

    /// The frame as its own `artifact.verified` record names it, read and authenticated; `None`
    /// when the ledger has no such record.
    pub fn frame(&mut self, artifact: ArtifactId) -> GlobalLedgerResult<Option<FrameQueryFrame>> {
        let key = views::key(artifact)?;
        let database = Arc::clone(&self.database);
        self.read(move |connection| {
            let rows = read_rows_bound(
                connection,
                VERIFIED_SEQUENCE,
                &[SqlValue::Text(key)],
                None,
                &mut 0,
                false,
            )?;
            let Some(sequence) = first_integer(&rows).map(decode) else {
                return Ok(None);
            };
            let event = authenticated_row(&database, connection, sequence)?;
            let mismatch = || failure(LedgerCode::ArtifactEventLinksMismatch.code(), location());
            let reference = event
                .projected_artifacts(true)
                .into_iter()
                .find(|reference| {
                    reference.artifact_id == artifact
                        && reference.kind == ArtifactKind::CaptureFrame
                })
                .ok_or_else(mismatch)?;
            let instance_id = *event.links().instance_id().ok_or_else(mismatch)?;
            Ok(Some(FrameQueryFrame {
                reference,
                instance_id,
                run_id: event.links().run_id().copied(),
                verified_sequence: sequence,
            }))
        })
    }

    /// The kept folder of `frame`, moved as Lab output (`lab`) or as an error frame, by the
    /// view's rules (E16): `<local date>\lab-<alias>` at its capture, or `<local date of the
    /// owner point>\<alias>-<point sequence>-<code>` of its owner point, the lowest-sequence point
    /// whose window holds it; the file is `<HHmmss-fff>_<object file>` in local time.
    pub fn folder(
        &mut self,
        frame: &FrameQueryFrame,
        lab: bool,
    ) -> GlobalLedgerResult<KeptFrameFolder> {
        let local = self.local;
        let at = frame.reference.created_at_unix_ms;
        let file_name = kept_file_name(frame.reference.object_key.as_deref(), at, local)
            .ok_or_else(clock_failed)?;
        let database = Arc::clone(&self.database);
        if lab {
            return self.read(move |connection| {
                let alias = alias_at(
                    &database,
                    connection,
                    &frame.instance_id,
                    frame.verified_sequence,
                )?;
                Ok(KeptFrameFolder {
                    date: local_date(at, local).ok_or_else(clock_failed)?,
                    leaf: format!("lab-{alias}"),
                    file_name,
                })
            });
        }
        let parameters = [
            (
                ":artifact",
                SqlValue::Text(views::key(frame.reference.artifact_id)?),
            ),
            (
                ":frame",
                match frame.reference.frame_id {
                    Some(id) => SqlValue::Text(views::key(id)?),
                    None => SqlValue::Null,
                },
            ),
            (":instance", SqlValue::Text(views::key(frame.instance_id)?)),
            (":at", stored(at)),
            (":window_ms", integer(ERROR_WINDOW_MS)),
        ];
        self.read(move |connection| {
            let (rows, _) = statement_rows(connection, OWNER, &parameters)?;
            let (sequence, point_at, epoch_end, end_type, alias_sequence) = match rows.first() {
                Some(row) => match row.as_slice() {
                    [
                        SqlValue::Integer(sequence),
                        SqlValue::Integer(point_at),
                        SqlValue::Integer(epoch_end),
                        end_type,
                        SqlValue::Integer(alias_sequence),
                    ] => (
                        decode(*sequence),
                        decode(*point_at),
                        *epoch_end != 0,
                        match end_type {
                            SqlValue::Text(text) => Some(text.clone()),
                            _ => None,
                        },
                        decode(*alias_sequence),
                    ),
                    _ => return Err(invalid_row("owner")),
                },
                // L classed the frame error by these same points: one of them must hold it.
                None => {
                    return Err(non_terminal(failure(
                        LedgerCode::LedgerIndexMismatch.code(),
                        location(),
                    ))
                    .with_detail("statement=owner no point holds the frame".to_owned()));
                }
            };
            let (instance, code) = if epoch_end {
                let event_type = end_type
                    .as_deref()
                    .and_then(identifier::<EventType>)
                    .ok_or_else(|| invalid_row("owner"))?;
                (frame.instance_id, type_leaf_name(event_type))
            } else {
                let point = authenticated_row(&database, connection, sequence)?;
                let preceding = match point.links().run_id() {
                    Some(run) => preceding_failure(&database, connection, run, sequence)?,
                    None => None,
                };
                let own = serde_json::to_value(point.payload()).ok();
                (
                    point
                        .links()
                        .instance_id()
                        .copied()
                        .unwrap_or(frame.instance_id),
                    leaf_name(
                        point
                            .payload()
                            .outcome()
                            .map(|outcome| outcome.code().as_str().to_owned()),
                        own.as_ref().and_then(|value| first_string(value, "code")),
                        preceding,
                        point.event_type(),
                    ),
                )
            };
            let alias = alias_at(&database, connection, &instance, alias_sequence)?;
            Ok(KeptFrameFolder {
                date: local_date(point_at, local).ok_or_else(clock_failed)?,
                leaf: format!("{alias}-{sequence}-{code}"),
                file_name,
            })
        })
    }

    /// The first read confirmation of the frame `artifact` (or of its `run`) recorded at or after
    /// its capture `at`, with who confirmed it.
    pub fn confirmation(
        &mut self,
        artifact: ArtifactId,
        run: Option<RunId>,
        at: u64,
    ) -> GlobalLedgerResult<Option<FrameConfirmation>> {
        let parameters = [
            SqlValue::Text(views::key(artifact)?),
            match run {
                Some(run) => SqlValue::Text(views::key(run)?),
                None => SqlValue::Null,
            },
            stored(at),
        ];
        let database = Arc::clone(&self.database);
        self.read(move |connection| {
            let rows = read_rows_bound(
                connection,
                FIRST_CONFIRMATION,
                &parameters,
                None,
                &mut 0,
                false,
            )?;
            let Some(sequence) = first_integer(&rows).map(decode) else {
                return Ok(None);
            };
            let event = authenticated_row(&database, connection, sequence)?;
            let surface = match event.payload() {
                EventPayload::Client(ClientPayload::Action(payload)) => {
                    payload.record().surface_id().to_owned()
                }
                _ => return Err(invalid_row("confirmation")),
            };
            let client = match event.links().correlation_id() {
                Some(correlation) => {
                    let rows = read_rows_bound(
                        connection,
                        IDENTITY_CARD,
                        &[SqlValue::Text(views::key(correlation)?), stored(sequence)],
                        None,
                        &mut 0,
                        false,
                    )?;
                    match first_integer(&rows) {
                        Some(card) => match authenticated_row(&database, connection, decode(card))?
                            .payload()
                        {
                            EventPayload::Client(ClientPayload::GovernanceIdentityDeclared(
                                declared,
                            )) => Some(declared.card().client.clone()),
                            _ => None,
                        },
                        None => None,
                    }
                }
                None => None,
            };
            Ok(Some(FrameConfirmation {
                actor: event.origin().actor(),
                source: event.origin().source(),
                surface,
                client,
                at_unix_ms: event.timestamp_unix_ms(),
                sequence,
            }))
        })
    }

    /// The capture frames of `run` (their `artifact.verified` rows).
    pub fn run_frames(&mut self, run: RunId) -> GlobalLedgerResult<Vec<ArtifactId>> {
        let key = views::key(run)?;
        self.read(move |connection| {
            read_rows_bound(
                connection,
                RUN_FRAMES,
                &[SqlValue::Text(key)],
                None,
                &mut 0,
                false,
            )?
            .into_iter()
            .map(|row| match row.as_slice() {
                [SqlValue::Text(artifact)] => {
                    identifier(artifact).ok_or_else(|| invalid_row("run_frames"))
                }
                _ => Err(invalid_row("run_frames")),
            })
            .collect()
        })
    }

    /// The time of the record at `sequence`, `None` when there is none.
    pub fn record_time(&mut self, sequence: u64) -> GlobalLedgerResult<Option<u64>> {
        self.read(move |connection| {
            Ok(first_integer(&read_rows_bound(
                connection,
                RECORD_TIME,
                &[stored(sequence)],
                None,
                &mut 0,
                false,
            )?)
            .map(decode))
        })
    }
}

/// A class of statement L's answer.
fn class_of(text: &str) -> Option<FrameRetentionClass> {
    Some(match text {
        "running" => FrameRetentionClass::Running,
        "lab" => FrameRetentionClass::Lab,
        "error" => FrameRetentionClass::Error,
        "resource" => FrameRetentionClass::Resource,
        "duplicate" => FrameRetentionClass::Duplicate,
        "default" => FrameRetentionClass::Default,
        _ => return None,
    })
}

/// #422 LN-7c Q (final-query §4): the evidence frames the read confirmation at
/// `confirmation_sequence` of `target` covers on the state root `state_root`, counted by the
/// cleaner's classification (`switches`, at `now_unix_ms`) and the kept listing: the target's
/// frames captured at or before the confirmation that are in `kept\` (`in_kept`), and those still
/// live whose class is Lab or error. A frame index that does not cover the ledger counts nothing
/// and is this read's error (`ledger_index_mismatch`). `check_vm_steps` and `chunk_frames` are the
/// caller's `ledger_observer.read_check_vm_steps` and `ledger_observer.frame_query_chunk_frames`
/// (review F4: read from the key table, not from constants of this crate).
pub fn evidence_covers(
    state_root: &Path,
    target: EvidenceTarget,
    confirmation_sequence: u64,
    switches: FrameRetentionSwitches,
    now_unix_ms: u64,
    check_vm_steps: u32,
    chunk_frames: usize,
) -> GlobalLedgerResult<EvidenceCovers> {
    let mut query = FrameQuery::open_state_root(state_root, check_vm_steps)?;
    let confirmed_at = query.record_time(confirmation_sequence)?.ok_or_else(|| {
        GlobalLedgerError::request(LedgerCode::ArtifactEventNotFound.code(), location())
    })?;
    let artifacts = match target {
        EvidenceTarget::Frame(artifact) => vec![artifact],
        EvidenceTarget::Run(run) => query.run_frames(run)?,
    };
    let listing_failed = |error: std::io::Error| {
        GlobalLedgerError::io(LedgerCode::LedgerIo.code(), location(), &error)
    };
    let kept = actingcommand_artifact_store::list_kept_folders(state_root)
        .map_err(|error| {
            GlobalLedgerError::request(LedgerCode::LedgerIo.code(), location())
                .with_detail(error.to_string())
        })?
        .into_iter()
        .flat_map(|leaf| leaf.files)
        .filter_map(|file| {
            file.object_file
                .strip_suffix(".png")
                .and_then(identifier::<ArtifactId>)
        })
        .collect::<BTreeSet<_>>();
    let live = crate::global::retention::live_frame_ids(state_root).map_err(listing_failed)?;
    let settings = FrameQuerySettings {
        now_unix_ms,
        switches,
        evidence_max_age_ms: COVERS_MAX_AGE_MS,
    };
    let incomplete = || {
        GlobalLedgerError::request(LedgerCode::LedgerIndexMismatch.code(), location())
            .with_detail("table=ledger_frame_refs frame_refs_incomplete".to_owned())
    };
    let mut covers = EvidenceCovers::default();
    let in_kept = artifacts
        .iter()
        .filter(|artifact| kept.contains(artifact))
        .copied()
        .collect::<Vec<_>>();
    if !in_kept.is_empty() {
        match query.kept(&in_kept, &settings)? {
            FrameQueryOutcome::Answered { value, .. } => {
                covers.in_kept = value
                    .iter()
                    .filter(|answer| answer.created_at_unix_ms <= confirmed_at)
                    .count() as u64;
            }
            FrameQueryOutcome::Incomplete | FrameQueryOutcome::Stopped => {
                return Err(incomplete());
            }
        }
    }
    let in_live = artifacts
        .iter()
        .filter(|artifact| live.contains(artifact))
        .copied()
        .collect::<Vec<_>>();
    let mut live_evidence = 0_u64;
    if !in_live.is_empty() {
        let waits = match query.waits()? {
            FrameQueryOutcome::Answered { value, .. } => value,
            FrameQueryOutcome::Incomplete | FrameQueryOutcome::Stopped => {
                return Err(incomplete());
            }
        };
        for chunk in in_live.chunks(chunk_frames.max(1)) {
            match query.live(chunk, &waits, &settings)? {
                FrameQueryOutcome::Answered { value, .. } => {
                    live_evidence += value
                        .iter()
                        .filter(|answer| {
                            answer.created_at_unix_ms <= confirmed_at
                                && matches!(
                                    answer.class,
                                    FrameRetentionClass::Lab | FrameRetentionClass::Error
                                )
                        })
                        .count() as u64;
                }
                FrameQueryOutcome::Incomplete | FrameQueryOutcome::Stopped => {
                    return Err(incomplete());
                }
            }
        }
    }
    covers.evidence_frames = covers.in_kept + live_evidence;
    Ok(covers)
}

#[cfg(test)]
mod tests;
