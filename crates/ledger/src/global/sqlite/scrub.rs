// SPDX-License-Identifier: AGPL-3.0-only

//! LN-9 (#422, LEAN-L model §6; Alice's ruling 57): the paced background scrub of the Runtime's
//! writer. Since LN-8 a start verifies the authenticated head and the tail only, so damage in an
//! old row that nobody reads is found here, within one pass (about 40 minutes on the production
//! ledger).
//!
//! - **Its own connection.** Each pass opens a read-only connection of its own
//!   (`RuntimeDatabase::read_snapshot_connection`). It never takes the shared connection the
//!   writer commits on, and in WAL mode a reader never blocks a commit, so the writer's flush does
//!   not wait for it.
//! - **Chunked transactions.** In one read transaction the pass authenticates the keyed meta row
//!   (the head and the head hash it walks to) and checks `MAX(sequence)` against the head. It then
//!   reads the rows `1..=head` in chunks of `ledger_observer.scrub_chunk_rows`, each chunk in a
//!   read transaction of its own, so no snapshot is held from one chunk to the next. Every row is
//!   authenticated on its own as a history read authenticates it (`RowReader`: decode, stored
//!   sequence, hash, keyed tag, predecessor link, link and artifact rows; event ids unique within
//!   a chunk, across chunks by the `event_id` unique index), chained to the row before it across
//!   chunks by its computed hash; the last row's hash is the meta row's head hash. The pass ends
//!   with the whole-file `PRAGMA quick_check(1)` on its connection.
//! - **The pace.** A chunk starts no earlier than the previous chunk's start plus that chunk's rows
//!   at `ledger_observer.scrub_rows_per_second`. The rate in effect is read after each chunk (the
//!   key is settable), so the rows read since a pass began never exceed the rate by more than one
//!   chunk. A pass starts at most once every `ledger_observer.scrub_pass_interval_ms`, the first
//!   when the writer opens.
//! - **The frame index** (#422 LN-7c R). For every row it authenticates inside the extent of
//!   `ledger_frame_refs` (read with the head), a chunk compares the record's frame index rows with
//!   the ones its payload gives (`frame_refs::frame_refs`), in the chunk's read transaction. The
//!   table is derived, so what the scrub finds there never stops the Runtime: a difference, or an
//!   extent it cannot read, is a non-fatal finding (`ledger_index_mismatch` of
//!   `verify_ledger_frame_refs`, [`ScrubThread::frame_refs_finding`]). The scrub marks the extent
//!   untrusted (`frame_refs::mark_untrusted`, one statement on the shared connection, the scrub's
//!   only write), stops checking the index for the rest of the pass and goes on with the chain;
//!   the next writer open rebuilds the index by its backfill.
//! - **A finding** is the failure a full verification (`ledger-maintenance verify`, and the open
//!   before LN-8) gives for the same damage, with its code and operation (`ledger_record_mismatch`,
//!   `ledger_index_mismatch`, `sequence_discontinuity`, `ledger_meta_mismatch`, ...;
//!   `state_database_corrupt` from `quick_check`), and `phase=scrub` in its detail. The scrub
//!   stops at it; the host reads it (`GlobalLedger::scrub_finding`) and latches it as a Global
//!   failure. Every other failure of the scrub (its connection, a statement, a panic) is reported
//!   the same way: a scrub that cannot check fails loud.
//! - **The stop.** The close raises the stop, ends the scrub's wait and interrupts its statement in
//!   progress, so it is delayed by one chunk at most.
//!
//! Nothing the scrub keeps grows with the ledger: its position is the last row it read and that
//! row's hash.

use super::{
    RowReader, append_detail, authenticated_head, decode, failure, frame_refs, read_rows,
    refuse_unsupported_format, sql_error, views,
};
use crate::codes::{LedgerCode, LedgerLocation};
use crate::fact::{LedgerEventMetadata, LedgerEventRead};
use crate::global::{GlobalLedgerError, GlobalLedgerResult};
use actingcommand_runtime_database::RuntimeDatabase;
use rusqlite::{Connection, InterruptHandle, TransactionBehavior, types::Value as SqlValue};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The largest `ledger_observer.scrub_rows_per_second`.
const MAX_ROWS_PER_SECOND: u64 = 100_000;
/// The largest `ledger_observer.scrub_chunk_rows`.
const MAX_CHUNK_ROWS: usize = 4_096;

/// LN-9: the scrub of the Runtime's writer (`ledger_observer.scrub_rows_per_second`,
/// `scrub_chunk_rows`, `scrub_pass_interval_ms`) and the clock it is paced on.
#[derive(Clone)]
pub struct LedgerScrubPolicy {
    rows_per_second: u64,
    chunk_rows: usize,
    pass_interval_ms: u64,
    clock: Arc<dyn LedgerScrubClock>,
}

impl fmt::Debug for LedgerScrubPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LedgerScrubPolicy")
            .field("rows_per_second", &self.rows_per_second)
            .field("chunk_rows", &self.chunk_rows)
            .field("pass_interval_ms", &self.pass_interval_ms)
            .finish()
    }
}

impl LedgerScrubPolicy {
    /// `rows_per_second` within `1..=100000`, `chunk_rows` within `1..=4096` and
    /// `pass_interval_ms` at least 1, paced on the process's monotonic clock; any other value is
    /// `invalid_ledger_config`.
    pub fn new(
        rows_per_second: u64,
        chunk_rows: usize,
        pass_interval_ms: u64,
    ) -> GlobalLedgerResult<Self> {
        if !valid_rate(rows_per_second)
            || !(1..=MAX_CHUNK_ROWS).contains(&chunk_rows)
            || pass_interval_ms == 0
        {
            return Err(invalid_config());
        }
        Ok(Self {
            rows_per_second,
            chunk_rows,
            pass_interval_ms,
            clock: Arc::new(ProcessScrubClock {
                origin: Instant::now(),
            }),
        })
    }

    /// The clock the scrub is paced on (a test drives its own).
    pub fn with_clock(mut self, clock: Arc<dyn LedgerScrubClock>) -> Self {
        self.clock = clock;
        self
    }
}

/// LN-9: the scrub's time in milliseconds and its waits. The Runtime's is the process's monotonic
/// clock; a test drives its own (`SteppingScrubClock`, test hooks). One scrub per clock.
pub trait LedgerScrubClock: Send + Sync {
    /// Now, in milliseconds.
    fn now_ms(&self) -> u64;
    /// Waits until `due_ms`, or less once `stop` is raised.
    fn wait_until(&self, due_ms: u64, stop: &LedgerScrubStop);
    /// The scrub's stop was raised: a wait that does not watch the stop ends now.
    fn wake(&self) {}
    /// Called inside each chunk's read transaction, after its rows were read.
    fn chunk_read(&self) {}
    /// Called once, when the scrub's thread ends.
    fn ended(&self) {}
}

/// LN-9: the stop of one scrub, which the ledger's close raises.
pub struct LedgerScrubStop {
    raised: Mutex<bool>,
    changed: Condvar,
}

impl LedgerScrubStop {
    fn new() -> Self {
        Self {
            raised: Mutex::new(false),
            changed: Condvar::new(),
        }
    }

    fn raise(&self) {
        *lock(&self.raised) = true;
        self.changed.notify_all();
    }

    /// Whether the stop is raised.
    pub fn is_raised(&self) -> bool {
        *lock(&self.raised)
    }

    /// Waits at most `timeout` for the stop; whether it is raised.
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let raised = lock(&self.raised);
        let (raised, _) = self
            .changed
            .wait_timeout_while(raised, timeout, |raised| !*raised)
            .unwrap_or_else(PoisonError::into_inner);
        *raised
    }
}

/// The Runtime's scrub clock: milliseconds since the policy was made.
struct ProcessScrubClock {
    origin: Instant,
}

impl LedgerScrubClock for ProcessScrubClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn wait_until(&self, due_ms: u64, stop: &LedgerScrubStop) {
        loop {
            let now = self.now_ms();
            if now >= due_ms || stop.wait_timeout(Duration::from_millis(due_ms - now)) {
                return;
            }
        }
    }
}

/// What the scrub thread and the ledger share.
struct ScrubShared {
    stop: LedgerScrubStop,
    /// `ledger_observer.scrub_rows_per_second` in effect.
    rows_per_second: AtomicU64,
    finding: Mutex<Option<GlobalLedgerError>>,
    /// #422 LN-7c R: the first non-fatal finding of the derived frame index.
    frame_refs_finding: Mutex<Option<GlobalLedgerError>>,
    /// The interrupt of the pass's connection while a pass runs.
    interrupt: Mutex<Option<InterruptHandle>>,
    clock: Arc<dyn LedgerScrubClock>,
}

/// LN-9: the scrub's thread, which starts and stops with the Runtime's writer (`global.rs`).
pub(in crate::global) struct ScrubThread {
    shared: Arc<ScrubShared>,
    thread: JoinHandle<()>,
}

impl ScrubThread {
    pub(in crate::global) fn start(
        database: Arc<RuntimeDatabase>,
        policy: LedgerScrubPolicy,
    ) -> GlobalLedgerResult<Self> {
        let shared = Arc::new(ScrubShared {
            stop: LedgerScrubStop::new(),
            rows_per_second: AtomicU64::new(policy.rows_per_second),
            finding: Mutex::new(None),
            frame_refs_finding: Mutex::new(None),
            interrupt: Mutex::new(None),
            clock: policy.clock,
        });
        let scrub = Arc::clone(&shared);
        let (chunk_rows, pass_interval_ms) = (policy.chunk_rows, policy.pass_interval_ms);
        let thread = thread::Builder::new()
            .name("actingcommand-ledger-scrub".to_string())
            .spawn(move || run(&scrub, &database, chunk_rows, pass_interval_ms))
            .map_err(|error| {
                GlobalLedgerError::io(
                    LedgerCode::WriterSpawnFailed.code(),
                    LedgerLocation::LedgerScrub.location(),
                    &error,
                )
            })?;
        Ok(Self { shared, thread })
    }

    /// What the scrub found, once it found it.
    pub(in crate::global) fn finding(&self) -> Option<GlobalLedgerError> {
        lock(&self.shared.finding).clone()
    }

    /// #422 LN-7c R: the first thing the scrub found wrong with the derived frame index, once it
    /// found it: non-fatal, the extent marked untrusted.
    pub(in crate::global) fn frame_refs_finding(&self) -> Option<GlobalLedgerError> {
        lock(&self.shared.frame_refs_finding).clone()
    }

    /// The rate from the next chunk on.
    pub(in crate::global) fn set_rows_per_second(&self, rows_per_second: u64) {
        self.shared
            .rows_per_second
            .store(rows_per_second, Ordering::Relaxed);
    }

    /// Raises the stop, ends the wait, interrupts the statement in progress and joins the thread.
    pub(in crate::global) fn finish(self) -> GlobalLedgerResult<()> {
        self.shared.stop.raise();
        self.shared.clock.wake();
        if let Some(interrupt) = lock(&self.shared.interrupt).as_ref() {
            interrupt.interrupt();
        }
        self.thread.join().map_err(|_| {
            GlobalLedgerError::fatal(
                LedgerCode::WriterPanicked.code(),
                LedgerLocation::LedgerScrub.location(),
            )
        })
    }
}

/// Whether `rows_per_second` is a rate the scrub takes (`1..=100000`).
pub(in crate::global) fn valid_rate(rows_per_second: u64) -> bool {
    (1..=MAX_ROWS_PER_SECOND).contains(&rows_per_second)
}

pub(in crate::global) fn invalid_config() -> GlobalLedgerError {
    GlobalLedgerError::fatal(
        LedgerCode::InvalidLedgerConfig.code(),
        LedgerLocation::LedgerScrub.location(),
    )
}

/// The thread: passes until the stop or a finding. A failure met after the stop was raised is
/// the stop's (its statement was interrupted) and is not a finding.
fn run(shared: &ScrubShared, database: &RuntimeDatabase, chunk_rows: usize, interval_ms: u64) {
    let end = ScrubEnd(shared);
    if let Err(failure) = passes(shared, database, chunk_rows, interval_ms) {
        end.found(failure);
    }
}

/// The end of the scrub's thread, also when the thread unwinds from a panic: the panic is the
/// finding `writer_panicked` (of `ledger_scrub`), and the clock learns that the thread ended.
struct ScrubEnd<'a>(&'a ScrubShared);

impl ScrubEnd<'_> {
    /// `failure` is the finding unless the stop was raised before it.
    fn found(&self, failure: GlobalLedgerError) {
        if !self.0.stop.is_raised() {
            *lock(&self.0.finding) = Some(failure);
        }
    }
}

impl Drop for ScrubEnd<'_> {
    fn drop(&mut self) {
        if thread::panicking() {
            self.found(GlobalLedgerError::fatal(
                LedgerCode::WriterPanicked.code(),
                LedgerLocation::LedgerScrub.location(),
            ));
        }
        self.0.clock.ended();
    }
}

fn passes(
    shared: &ScrubShared,
    database: &RuntimeDatabase,
    chunk_rows: usize,
    interval_ms: u64,
) -> GlobalLedgerResult<()> {
    let clock = &*shared.clock;
    let mut due = clock.now_ms();
    loop {
        clock.wait_until(due, &shared.stop);
        if shared.stop.is_raised() {
            return Ok(());
        }
        let started = clock.now_ms();
        let mut connection = database
            .read_snapshot_connection(LedgerLocation::LedgerScrub.location())
            .map_err(|error| in_scrub(error.into()))?;
        *lock(&shared.interrupt) = Some(connection.get_interrupt_handle());
        let walked = walk(shared, database, &mut connection, chunk_rows);
        *lock(&shared.interrupt) = None;
        walked?;
        due = started.saturating_add(interval_ms);
    }
}

/// Where one pass stands: the head and head hash its first read authenticated, and the last row
/// it authenticated with that row's computed hash. #422 LN-7c R: and the extent of the frame
/// index that read found (`from_sequence`, `through_sequence`).
struct ScrubCursor {
    head: u64,
    head_hash: Option<String>,
    read: u64,
    last: Option<(u64, String)>,
    frame_refs: Option<(u64, u64)>,
    /// The extent could not be read at the pass's start.
    frame_refs_error: Option<GlobalLedgerError>,
}

/// One pass on `connection`; returns early, without a finding, once the stop is raised.
fn walk(
    shared: &ScrubShared,
    database: &RuntimeDatabase,
    connection: &mut Connection,
    chunk_rows: usize,
) -> GlobalLedgerResult<()> {
    let clock = &*shared.clock;
    let mut cursor = in_read(connection, |read| begin(database, read)).map_err(in_scrub)?;
    if let Some(error) = cursor.frame_refs_error.take() {
        frame_refs_found(shared, database, error, 0);
    }
    let mut due = clock.now_ms();
    while cursor.read < cursor.head {
        clock.wait_until(due, &shared.stop);
        if shared.stop.is_raised() {
            return Ok(());
        }
        let started = clock.now_ms();
        let rows = in_read(connection, |read| {
            let rows = chunk(shared, database, read, &mut cursor, chunk_rows)?;
            clock.chunk_read();
            Ok(rows)
        })
        .map_err(in_scrub)?;
        let rows_per_second = shared.rows_per_second.load(Ordering::Relaxed).max(1);
        due = started.saturating_add(rows.saturating_mul(1_000).div_ceil(rows_per_second));
    }
    if shared.stop.is_raised() {
        return Ok(());
    }
    end(connection, &cursor).map_err(in_scrub)
}

/// `read` in one read transaction of `connection`.
fn in_read<T>(
    connection: &mut Connection,
    read: impl FnOnce(&Connection) -> GlobalLedgerResult<T>,
) -> GlobalLedgerResult<T> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|error| sql_error(error, LedgerLocation::BeginSqliteSnapshot.location()))?;
    let value = read(&transaction)?;
    transaction
        .commit()
        .map_err(|error| sql_error(error, LedgerLocation::CloseSqliteSnapshot.location()))?;
    Ok(value)
}

/// The pass's first read: the format, the views, the keyed meta row, and the stored head at the
/// meta row's head (rows past it or missing at its end are `ledger_meta_mismatch`, as the full
/// read reports them).
fn begin(database: &RuntimeDatabase, connection: &Connection) -> GlobalLedgerResult<ScrubCursor> {
    refuse_unsupported_format(connection)?;
    views::installed(connection)?;
    let (head, head_hash) = authenticated_head(database, connection)?;
    let rows = read_rows(
        connection,
        "SELECT MAX(sequence) FROM ledger_events",
        None,
        &mut 0,
        false,
    )?;
    let last = match rows.as_slice() {
        [row] => match row.as_slice() {
            [SqlValue::Null] => Some(0),
            [SqlValue::Integer(last)] => Some(decode(*last)),
            _ => None,
        },
        _ => None,
    };
    if last != Some(head) {
        return Err(failure(
            LedgerCode::LedgerMetaMismatch.code(),
            LedgerLocation::VerifySqliteHead.location(),
        ));
    }
    let (frame_refs, frame_refs_error) = match frame_refs::read_extent(connection) {
        Ok(extent) => (extent, None),
        Err(error) => (None, Some(error)),
    };
    Ok(ScrubCursor {
        head,
        head_hash,
        read: 0,
        last: None,
        frame_refs,
        frame_refs_error,
    })
}

/// The next rows of the pass, at most `chunk_rows`, each authenticated on its own and chained to
/// the row before it, and the frame index rows of those inside its extent (#422 LN-7c R); returns
/// how many were read.
fn chunk(
    shared: &ScrubShared,
    database: &RuntimeDatabase,
    connection: &Connection,
    cursor: &mut ScrubCursor,
    chunk_rows: usize,
) -> GlobalLedgerResult<u64> {
    let after = cursor.read;
    let through = cursor
        .head
        .min(after.saturating_add(u64::try_from(chunk_rows).unwrap_or(u64::MAX)));
    let extent = cursor.frame_refs;
    let mut expected = Vec::new();
    let mut derived = None;
    let mut reader = RowReader::new(database, connection, None);
    reader.last = cursor.last.take();
    let read = reader.read_chunks(
        &[],
        after,
        through,
        true,
        &mut |event: LedgerEventMetadata| -> GlobalLedgerResult<bool> {
            if extent.is_some_and(|(from, to)| (from..=to).contains(&event.sequence())) {
                match frame_refs::frame_refs(&event) {
                    Ok(refs) => expected.push((event.sequence(), refs)),
                    Err(error) => {
                        derived.get_or_insert(error);
                    }
                }
            }
            Ok(!shared.stop.is_raised())
        },
    );
    let read = read
        .map_err(|mut error| {
            append_detail(&mut error, &format!("after_sequence={after}"));
            error
        })?
        .unwrap_or(after);
    // #422 LN-7c R: the derived index's rows of the chunk; a difference is a non-fatal finding.
    if let Some(extent) = extent {
        let checked = match derived {
            Some(error) => Err(error),
            None => frame_refs::check_chunk(connection, extent, (after, read), &expected),
        };
        if let Err(error) = checked {
            frame_refs_found(shared, database, error, after);
            cursor.frame_refs = None;
        }
    }
    cursor.last = reader.last.take();
    cursor.read = read;
    Ok(read - after)
}

/// #422 LN-7c R: what the scrub found wrong with the derived frame index (a difference, an extent
/// it cannot read): the extent is marked untrusted, and the first finding is kept, non-fatal.
fn frame_refs_found(
    shared: &ScrubShared,
    database: &RuntimeDatabase,
    error: GlobalLedgerError,
    after: u64,
) {
    let marked = frame_refs::mark_untrusted(database);
    let finding = frame_refs::untrusted_finding(
        &error,
        &marked,
        &format!("after_sequence={after}; phase=scrub"),
    );
    lock(&shared.frame_refs_finding).get_or_insert(finding);
}

/// The pass's end: the last row read is the head with the meta row's head hash; then the
/// whole-file check on the pass's connection.
fn end(connection: &Connection, cursor: &ScrubCursor) -> GlobalLedgerResult<()> {
    if cursor.last.as_ref().map(|(_, hash)| hash) != cursor.head_hash.as_ref() {
        return Err(failure(
            LedgerCode::LedgerMetaMismatch.code(),
            LedgerLocation::VerifySqliteHead.location(),
        ));
    }
    RuntimeDatabase::quick_check(connection, LedgerLocation::LedgerScrub.location())?;
    Ok(())
}

fn in_scrub(mut error: GlobalLedgerError) -> GlobalLedgerError {
    append_detail(&mut error, "phase=scrub");
    error
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// LN-9, tests: a scrub clock the test drives; nothing waits on the wall clock. It stands at 0
/// and parks every wait until the test lets it run up to a limit ([`Self::run_until`]): a wait
/// due at or before the limit then returns at once with the clock at its due time, and a wait due
/// after it parks the scrub until the limit rises. The test can also hold the scrub inside a
/// chunk's read transaction ([`Self::hold_chunks`]). One scrub per clock.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Default)]
pub struct SteppingScrubClock {
    state: Mutex<Stepping>,
    changed: Condvar,
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Default)]
struct Stepping {
    now_ms: u64,
    /// Waits due at or before it return at once; `None` until the test lets the clock run.
    limit_ms: Option<u64>,
    /// The scrub waits for a time after the limit.
    parked: bool,
    hold_chunks: bool,
    /// The scrub is held inside a chunk's read transaction.
    held: bool,
    woken: bool,
    ended: bool,
    /// Every due time the scrub waited for, in order.
    waits: Vec<u64>,
}

#[cfg(any(test, feature = "test-hooks"))]
impl SteppingScrubClock {
    /// A clock at 0 that parks every wait.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Lets every wait due at or before `limit_ms` return at once.
    pub fn run_until(&self, limit_ms: u64) {
        let mut state = lock(&self.state);
        state.limit_ms = Some(limit_ms);
        state.parked = false;
        self.changed.notify_all();
    }

    /// Waits at most `wait` until the scrub is parked after the limit or its thread has ended;
    /// whether it is.
    pub fn wait_idle(&self, wait: Duration) -> bool {
        let state = lock(&self.state);
        let (state, _) = self
            .changed
            .wait_timeout_while(state, wait, |state| !state.parked && !state.ended)
            .unwrap_or_else(PoisonError::into_inner);
        state.parked || state.ended
    }

    /// Holds the scrub inside each chunk's read transaction from now on, or lets it go on.
    pub fn hold_chunks(&self, hold: bool) {
        lock(&self.state).hold_chunks = hold;
        self.changed.notify_all();
    }

    /// Waits at most `wait` until the scrub is held inside a chunk; whether it is.
    pub fn wait_held(&self, wait: Duration) -> bool {
        let state = lock(&self.state);
        let (state, _) = self
            .changed
            .wait_timeout_while(state, wait, |state| !state.held)
            .unwrap_or_else(PoisonError::into_inner);
        state.held
    }

    /// The clock's time.
    pub fn now(&self) -> u64 {
        lock(&self.state).now_ms
    }

    /// Every due time the scrub waited for, in order.
    pub fn waits(&self) -> Vec<u64> {
        lock(&self.state).waits.clone()
    }

    /// Whether the scrub's thread has ended.
    pub fn has_ended(&self) -> bool {
        lock(&self.state).ended
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl LedgerScrubClock for SteppingScrubClock {
    fn now_ms(&self) -> u64 {
        self.now()
    }

    fn wait_until(&self, due_ms: u64, _stop: &LedgerScrubStop) {
        let mut state = lock(&self.state);
        state.waits.push(due_ms);
        loop {
            if state.woken {
                return;
            }
            if state.limit_ms.is_some_and(|limit| due_ms <= limit) {
                state.now_ms = state.now_ms.max(due_ms);
                return;
            }
            if !state.parked {
                state.parked = true;
                self.changed.notify_all();
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn wake(&self) {
        lock(&self.state).woken = true;
        self.changed.notify_all();
    }

    fn chunk_read(&self) {
        let mut state = lock(&self.state);
        if !state.hold_chunks {
            return;
        }
        state.held = true;
        self.changed.notify_all();
        while state.hold_chunks && !state.woken {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.held = false;
    }

    fn ended(&self) {
        lock(&self.state).ended = true;
        self.changed.notify_all();
    }
}
