// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-OFF-1 (design-ln-off §4.2 to §4.6, option A): the bounded offline snapshot of
//! `GlobalLedger::open_metadata` on an SQLite root. The UI's offline face reached 12 GB in 60 s on
//! the 8.1 GB production ledger (Workflow #422 comments 6503130154, 6508064467): the opening read
//! and authenticated the whole ledger in memory, ran an unbudgeted whole-file `quick_check(1)`,
//! and every page read the whole snapshot again. Now:
//!
//! - **The opening** makes only the writer open's checks (`verify_tail`): the format and the
//!   views, the keyed meta row (head, head hash, `ready` marker), the stored rows ending at the
//!   head, their contiguity (`COUNT`, `MIN`, `MAX` through an index) and the newest
//!   `open_verify_tail_records` rows authenticated one by one, chained to the head hash. It then
//!   reads the eviction intent and outcome rows by type, each authenticated, as the eviction proof
//!   set the writer keeps resident, and pins one point (head, head hash, marker, proofs). No
//!   whole-file check, no other history row, no migrated-prefix digest.
//! - **A page** is the writer's reader pool's page at that point (`ViewReader`, as
//!   `view_page_at_point`): one short read transaction on the snapshot's own read-only connection
//!   that checks the medium against the point (`verify_published_extent`), selects through the
//!   indexes and authenticates each row it reads; its scope source is `offline`.
//! - **A material fetch** (`resolve_artifact`) is one authenticated point read, with its proof
//!   from the proof set.
//! - **Budgets.** The opening runs within its deadline (an explicit one, else
//!   `ledger_observer.open_timeout_ms`'s factory value); each later read within its own deadline
//!   taken when it starts (`ledger_observer.writer_reply_timeout_ms`'s factory value, and no later
//!   than a deadline the caller set with `set_read_deadline` or passed). The deadline holds inside
//!   a statement too (`ledger_observer.read_check_vm_steps`). A page reads under its deadline
//!   alone and is refused whole when it passes, unless the caller asked for a per-page work budget
//!   (`ledger_observer.read_budget_ms`, A51), which answers an incomplete page with its cursor.
//! - **How much one read returns** (Alice, pending decision 115, 10-30: 「可以，并且限定一次可读的
//!   数据总量」, then 「我们直接限定UI每次可以读多少就行」): a page returns at most its request's
//!   limit (`MAX_RUNTIME_EVENT_QUERY_EVENTS`, 256 records, at most) and
//!   `MAX_RUNTIME_EVENT_QUERY_RESPONSE_BYTES` (768 KiB); a page cut there says so (`has_more`,
//!   its limit `event_count` or `response_bytes`) and its cursor reads the rest. A material fetch
//!   returns one record; the opening returns none.
//!
//! Every refusal of a deadline is `ledger_read_budget_exceeded` (a request error) whose detail
//! names the bound and the phase (`phase=open|proofs|page|resolve`).
//!
//! **A live root** (pending decision 115, recommendation A): every read is a short read
//! transaction at the pinned head; WAL lets the writer commit meanwhile, and a page never reads a
//! record committed after the point (the ledger tables are only ever appended to).

use super::{
    HistorySelection, ReadBudget, RowReader, SnapshotMemo, StatementGuard, TailOpen, ViewExtent,
    ViewReader, append_detail, read_history_rows, sql_error, verify_published_extent, verify_tail,
    views,
};
use crate::codes::{LedgerCode, LedgerLocation};
use crate::fact::LedgerEventMetadata;
use crate::global::read_only::check_read_budget;
use crate::global::read_work::ReadWork;
use crate::global::reader_pool::ReadPoint;
use crate::global::retention::{PageProofs, RetentionIndex};
use crate::global::{
    GlobalLedgerError, GlobalLedgerResult, LedgerArtifactSelection, LedgerMonotonicClock,
    ResolvedLedgerArtifact,
};
use actingcommand_contract::outcome::Location;
use actingcommand_contract::{
    EventQuery, EventType, LedgerReadSource, ProjectionProfile, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest,
};
use actingcommand_runtime_database::RuntimeDatabase;
use rusqlite::{Connection, TransactionBehavior, types::Value as SqlValue};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// The deadline of an offline opening without an explicit one: the factory value of
/// `ledger_observer.open_timeout_ms` (600 000 ms), the deadline of the Runtime's own open.
pub(in crate::global) const DEFAULT_OFFLINE_OPEN_TIMEOUT: Duration = Duration::from_millis(600_000);
/// The deadline of one later offline read (a page, a material fetch), from its start: the factory
/// value of `ledger_observer.writer_reply_timeout_ms` (10 000 ms), how long a Runtime caller waits
/// for one ledger reply.
pub(in crate::global) const DEFAULT_OFFLINE_READ_TIMEOUT: Duration = Duration::from_millis(10_000);
/// The clock step that stands for a passed deadline in a read's work budget (`deadline_work`).
const DEADLINE_STEP: Duration = Duration::from_millis(1);

/// How an offline snapshot reads (`GlobalLedgerEvidenceConfig`): the factory values of the
/// `ledger_observer` keys named on each, unless the caller set another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::global) struct OfflineReads {
    /// The deadline of the opening when the caller gave none (`open_timeout_ms`).
    pub(in crate::global) open_timeout: Duration,
    /// The deadline of one later read from its start (`writer_reply_timeout_ms`).
    pub(in crate::global) read_timeout: Duration,
    /// The newest records the opening authenticates (`open_verify_tail_records`).
    pub(in crate::global) tail_records: usize,
    /// SQLite steps between two looks at a read's deadline (`read_check_vm_steps`).
    pub(in crate::global) read_check_vm_steps: u32,
    /// Rows of one history statement (`history_page_rows`).
    pub(in crate::global) history_page_rows: usize,
    /// Sequences one history statement spans (`history_scan_rows`).
    pub(in crate::global) history_scan_rows: usize,
    /// A page's work budget (`read_budget_ms`); `None`: a page reads under its deadline alone.
    pub(in crate::global) page_read_budget: Option<Duration>,
}

impl Default for OfflineReads {
    fn default() -> Self {
        Self {
            open_timeout: DEFAULT_OFFLINE_OPEN_TIMEOUT,
            read_timeout: DEFAULT_OFFLINE_READ_TIMEOUT,
            tail_records: super::DEFAULT_OPEN_VERIFY_TAIL_RECORDS,
            read_check_vm_steps: crate::global::read_work::DEFAULT_READ_CHECK_VM_STEPS,
            history_page_rows: crate::global::history::DEFAULT_HISTORY_PAGE_ROWS,
            history_scan_rows: crate::global::history::DEFAULT_HISTORY_SCAN_ROWS,
            page_read_budget: None,
        }
    }
}

impl OfflineReads {
    /// The ranges the `ledger_observer` keys allow (a tail and a statement bound of one at least,
    /// a non-zero page budget); anything else is the request error `invalid_ledger_config` of the
    /// opening.
    pub(in crate::global) fn validate(&self) -> GlobalLedgerResult<()> {
        if !(1..=super::MAX_OPEN_VERIFY_TAIL_RECORDS).contains(&self.tail_records)
            || !(1..=crate::global::read_work::MAX_READ_CHECK_VM_STEPS)
                .contains(&self.read_check_vm_steps)
            || self.page_read_budget.is_some_and(|budget| budget.is_zero())
        {
            return Err(GlobalLedgerError::request(
                LedgerCode::InvalidLedgerConfig.code(),
                LedgerLocation::OpenLedgerMetadata.location(),
            ));
        }
        Ok(())
    }
}

/// An offline snapshot (`GlobalLedger::open_metadata`) of an SQLite root: the point its opening
/// pinned, the eviction proof set read there, and its own read-only connection, on which each
/// later read runs one short read transaction. Nothing of the history stays resident.
pub(in crate::global) struct SqliteViewSnapshot {
    database: Arc<RuntimeDatabase>,
    connection: Mutex<Connection>,
    /// #422 QR-2: the Lab relations and window starts kept from one page to the next.
    memo: Mutex<SnapshotMemo>,
    point: ReadPoint,
    /// The proof set as of the head (`RetentionIndex::offline_proofs`), for a material fetch.
    retention: RetentionIndex,
    reads: OfflineReads,
    /// Whether the root has the derived views and their indexes (`views::installed`).
    time_index: bool,
    /// Workflow #395: a later read ends no later than this (`set_read_deadline`).
    read_deadline: Option<Instant>,
}

impl SqliteViewSnapshot {
    /// The opening (design-ln-off §4.2): within `budget` (bytes, records and deadline), two short
    /// read transactions on a read-only connection of its own: the tail check (`phase=open`), then
    /// the eviction proof set (`phase=proofs`). `database` is opened without the whole-file check.
    pub(in crate::global) fn open(
        database: Arc<RuntimeDatabase>,
        budget: (u64, usize, Instant),
        reads: OfflineReads,
    ) -> GlobalLedgerResult<Self> {
        let opening = Some(budget);
        let operation = LedgerLocation::OpenLedgerMetadata.location();
        check_read_budget(opening, 0, 0).map_err(|error| in_phase(error, "open"))?;
        let mut connection = database
            .read_snapshot_connection(operation)
            .map_err(GlobalLedgerError::from)?;
        let work = deadline_work(budget.2, reads.read_check_vm_steps);
        let (marker_state, head, head_hash, time_index) =
            in_read(&mut connection, &work, (operation, "open"), |transaction| {
                let (marker, head, hash) = verify_tail(
                    &database,
                    transaction,
                    opening,
                    (reads.tail_records, TailOpen::Offline),
                    (&mut None, &mut 0),
                )?;
                // A root no writer of the views opened has no time index (`ViewReader`).
                Ok((marker.state, head, hash, views::installed(transaction)?))
            })?;
        let retention = in_read(
            &mut connection,
            &work,
            (operation, "proofs"),
            |transaction| read_proofs(&database, transaction, head, opening),
        )?;
        let point = ReadPoint {
            head,
            head_hash,
            marker_state,
            proofs: Arc::new(retention.published_proofs()),
        };
        Ok(Self {
            database,
            connection: Mutex::new(connection),
            memo: Mutex::new(SnapshotMemo::default()),
            point,
            retention,
            reads,
            time_index,
            read_deadline: None,
        })
    }

    /// The pinned head; `1..=head` was found contiguous at the opening.
    pub(in crate::global) fn head(&self) -> u64 {
        self.point.head
    }

    /// Workflow #395: see `GlobalLedgerMetadata::set_read_deadline`.
    pub(in crate::global) fn set_read_deadline(&mut self, deadline: Instant) {
        self.read_deadline = Some(deadline);
    }

    /// The deadline of one read that starts now: its own (`read_timeout` from now), and no later
    /// than the caller's (`set_read_deadline`, `given`).
    fn read_deadline(&self, given: Option<Instant>) -> Instant {
        let now = Instant::now();
        let own = now.checked_add(self.reads.read_timeout).unwrap_or(now);
        [self.read_deadline, given]
            .into_iter()
            .flatten()
            .fold(own, Instant::min)
    }

    /// One view page at the pinned point (design-ln-off §4.3): the reader pool's page, its scope
    /// source `offline`, at most its request's limit and the response bytes, with a cursor to the
    /// rest. Without a page work budget a page whose deadline passed is refused whole
    /// (`phase=page`); with one it answers incomplete with its cursor (A51).
    pub(in crate::global) fn project_view_page(
        &self,
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
    ) -> GlobalLedgerResult<RuntimeEventQueryPage> {
        const PHASE: &str = "page";
        let deadline = self.read_deadline(None);
        check_read_budget(Some((u64::MAX, usize::MAX, deadline)), 0, 0)
            .map_err(|error| in_phase(error, PHASE))?;
        let work_deadline = match self.reads.page_read_budget {
            Some(budget) => Instant::now()
                .checked_add(budget)
                .map_or(deadline, |at| at.min(deadline)),
            None => deadline,
        };
        let work = deadline_work(work_deadline, self.reads.read_check_vm_steps);
        let mut held = lock(&self.connection);
        let connection: &mut Connection = &mut held;
        let mut memo = lock(&self.memo);
        let page = ViewReader {
            database: self.database.as_ref(),
            head: self.point.head,
            extent: ViewExtent::Published {
                head_hash: self.point.head_hash.as_deref(),
                marker_state: self.point.marker_state,
            },
            proofs: PageProofs::Published {
                proofs: self.point.proofs.as_ref(),
                through_sequence: self.point.head,
            },
            source: LedgerReadSource::Offline,
            time_index: self.time_index,
        }
        .page(
            || Ok(connection),
            (query, profile, request),
            &mut None,
            (
                &work,
                self.reads.history_page_rows,
                self.reads.history_scan_rows,
            ),
            Some(&mut *memo),
        )
        .map_err(|error| in_phase(error, PHASE))?;
        if self.reads.page_read_budget.is_none() && work.was_spent() {
            return Err(in_phase(
                ReadWork::exceeded(LedgerLocation::ProjectLedgerViewPage.location())
                    .with_detail("deadline reached within the page".to_owned()),
                PHASE,
            ));
        }
        Ok(page)
    }

    /// The selected artifact by one authenticated point read at the pinned point (design-ln-off
    /// §4.4), its eviction proof from the proof set, within `deadline` and the read's own.
    pub(in crate::global) fn resolve_artifact(
        &self,
        selection: &LedgerArtifactSelection,
        deadline: Instant,
    ) -> GlobalLedgerResult<ResolvedLedgerArtifact> {
        const PHASE: &str = "resolve";
        selection.validate()?;
        let deadline = self.read_deadline(Some(deadline));
        let budget = Some((u64::MAX, usize::MAX, deadline));
        check_read_budget(budget, 0, 0).map_err(|error| in_phase(error, PHASE))?;
        let sequence = selection.event.sequence;
        let events = if sequence > self.point.head {
            Vec::new()
        } else {
            let work = deadline_work(deadline, self.reads.read_check_vm_steps);
            let query = EventQuery::default();
            let mut connection = lock(&self.connection);
            in_read(
                &mut connection,
                &work,
                (LedgerLocation::ResolveLedgerArtifact.location(), PHASE),
                |transaction| {
                    verify_published_extent(
                        &self.database,
                        transaction,
                        (self.point.head, self.point.head_hash.as_deref()),
                        self.point.marker_state,
                    )?;
                    read_history_rows(
                        &self.database,
                        transaction,
                        (&HistorySelection::of(&query), None),
                        (sequence - 1, sequence),
                        1,
                    )
                },
            )?
        };
        check_read_budget(budget, 0, events.len()).map_err(|error| in_phase(error, PHASE))?;
        crate::global::evidence::resolve_artifact_from_events(
            &events,
            selection,
            self.point.head,
            true,
            deadline,
            Some(&self.retention),
        )
        .map_err(|error| in_phase(error, PHASE))
    }
}

/// The eviction intent and outcome rows through `head`, selected by type and each authenticated
/// on its own, as the eviction proof set (`RetentionIndex::offline_proofs`).
fn read_proofs(
    database: &RuntimeDatabase,
    connection: &Connection,
    head: u64,
    budget: ReadBudget,
) -> GlobalLedgerResult<RetentionIndex> {
    let types = [
        SqlValue::Text(views::key(EventType::ArtifactEvictionIntent)?),
        SqlValue::Text(views::key(EventType::ArtifactEvictionOutcome)?),
    ];
    let mut records: Vec<LedgerEventMetadata> = Vec::new();
    RowReader::new(database, connection, budget).read_chunks(
        &types,
        0,
        head,
        false,
        &mut |event: LedgerEventMetadata| -> GlobalLedgerResult<bool> {
            records.push(event);
            Ok(true)
        },
    )?;
    Ok(RetentionIndex::offline_proofs(&records, head))
}

/// One short read transaction on the snapshot's connection, its statements under `work`'s
/// statement bound: a statement the bound interrupts is `ledger_read_budget_exceeded` of
/// `read.0`, and every `ledger_read_budget_exceeded` names its phase `read.1`.
fn in_read<T>(
    connection: &mut Connection,
    work: &ReadWork,
    (operation, phase): (Location, &str),
    read: impl FnOnce(&Connection) -> GlobalLedgerResult<T>,
) -> GlobalLedgerResult<T> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|error| sql_error(error, LedgerLocation::BeginSqliteSnapshot.location()))?;
    let result = StatementGuard::install(&transaction, work, operation).and_then(|guard| {
        let result = read(&transaction);
        guard.remove(operation).and(result)
    });
    let result = result.map_err(|error| {
        if work.interrupted(&error) {
            ReadWork::exceeded(operation)
                .with_detail("deadline reached within a statement".to_owned())
        } else {
            error
        }
    });
    let closed = match result {
        Ok(value) => transaction
            .commit()
            .map(|()| value)
            .map_err(|error| sql_error(error, LedgerLocation::CloseSqliteSnapshot.location())),
        Err(error) => Err(error.with_close_result(
            transaction
                .rollback()
                .map_err(|error| sql_error(error, LedgerLocation::CloseSqliteSnapshot.location())),
        )),
    };
    closed.map_err(|error| in_phase(error, phase))
}

/// A read's work budget that is spent once `deadline` has passed (`crate::read_deadline_passed`,
/// which a test's `LedgerReadClock` drives), looked at between statements, every
/// `READ_BUDGET_CHECK_ROWS` rows and every `check_vm_steps` SQLite steps inside a statement.
fn deadline_work(deadline: Instant, check_vm_steps: u32) -> ReadWork {
    let clock: LedgerMonotonicClock = Arc::new(move || {
        if crate::read_deadline_passed(deadline) {
            u64::MAX
        } else {
            0
        }
    });
    ReadWork::started(&clock, 0, Some(DEADLINE_STEP)).within_statements(check_vm_steps)
}

/// `ledger_read_budget_exceeded` names the phase it happened in; any other failure is unchanged.
fn in_phase(mut error: GlobalLedgerError, phase: &str) -> GlobalLedgerError {
    if error.code() == LedgerCode::LedgerReadBudgetExceeded.as_str() {
        append_detail(&mut error, &format!("phase={phase}"));
    }
    error
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
