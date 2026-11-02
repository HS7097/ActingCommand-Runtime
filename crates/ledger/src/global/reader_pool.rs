// SPDX-License-Identifier: AGPL-3.0-only

//! #422 QR-1 (Alice, 10-28: 「账本查询和清理用的是同一个写入线程……至少得异步，最好多线程执行」):
//! the Runtime writer's reader pool. A view page (a `QueryEvents` page, a Lab subscription's
//! page) is read by one of a few reader threads, each on a read-only SQLite connection of its own
//! (`RuntimeDatabase::read_snapshot_connection`), never on the writer thread and never waiting
//! for it: WAL lets the readers read while the writer commits.
//!
//! - **The point.** A reader reads at the point the writer last published ([`ReadPoint`]): its
//!   committed head, that head's record hash, the medium's marker and the eviction proofs as of
//!   that head. The writer publishes after every commit and before it answers the commit's
//!   appends, so a caller whose append was answered reads it; a point never names a record that
//!   is not committed. A page reads in one read transaction, checks the medium against the point
//!   (`verify_published_extent`) and reads at the point's head, so `through_sequence` is never
//!   past the committed head, and a record the writer commits meanwhile is not read.
//! - **The same page.** The page is the one the writer would read at that head: the same
//!   selection, the same row authentication, the same read budget and statement bound
//!   (`ledger_observer.read_budget_ms`, `read_check_vm_steps`, `history_page_rows`,
//!   `history_scan_rows`) counted from the reader's start, the same eviction observations.
//! - **The pool.** [`DEFAULT_READER_THREADS`] threads take pages from one queue of
//!   `ingress_capacity` pages. A page the queue cannot take now, or that no reader answers within
//!   the caller's reply limit, is `writer_response_timeout` of `project_ledger_view_page`, as a
//!   page the writer did not answer in time was: not fatal, the caller reads again.
//! - **No page nobody waits for** (fix 1, review M-1). A page carries its caller's deadline (the
//!   submit time plus the caller's reply limit, on the writer's clock); a reader that takes it
//!   past that deadline answers it with that same `writer_response_timeout` and does not read it.
//!   Once the pool is closing, a reader answers every page it takes as refused and reads none.
//! - **Fail loud.** A pool whose connections cannot open does not start; the ledger keeps its
//!   failure ([`super::GlobalLedger::reader_pool_finding`]) and the writer reads the pages, as
//!   before the pool, which the Runtime says as a warning at its start. A reader that ends while
//!   the pool runs (fix 1, review L-6) is said the same way while the Runtime runs
//!   ([`super::GlobalLedger::take_reader_pool_findings`]); once no reader is left, the writer
//!   reads the pages again, said as a warning too, instead of a fatal `writer_unavailable`.
//! - **Seen** (fix 1, review L-4). Each page a reader reads is recorded in the writer's commit
//!   statistics as a `ProjectViewPage` with its time, where the writer recorded its own pages
//!   (`GlobalLedger::take_writer_peak`). #422 W1-8 (P29): and in the pool's own page count and
//!   times, which the Runtime reads while it runs (`GlobalLedger::reader_pool_pages`).
//! - **The head** (#422 W1-8, P30). The published point's head is the ledger's committed head
//!   for a read that needs only that (`GlobalLedger::published_head`): it waits for no writer
//!   command.
//! - **The Lab relations** (#422 RB-1, 0.12.28). When the pool starts, a thread of its own builds
//!   the pool's Lab relations once, through the point published then, on a read-only connection
//!   of its own and within the open's deadline (`ledger_observer.open_timeout_ms`), outside every
//!   page's budget; the readers then read only the records after it
//!   (`sqlite::lab_relations`). Before it ends, and when it failed, the pages read the whole
//!   history's relations, as before; a failure is said once, as the readers' ends are
//!   (`take_findings`), and is never fatal. The close stops it within one look of its bound.
//! - **Artifact resolutions** (#422 RA-1, 0.12.38). The readers also answer
//!   `GlobalLedger::resolve_artifact` from the same queue, as they answer a page: one
//!   authenticated point read at the point the writer published, on the reader's own connection
//!   (`sqlite::resolve_artifact_at_point`), with the writer's answers and refusals at that head.
//!   A resolution carries its caller's deadline as a page does and is refused the same way (the
//!   pool closing, its caller's reply limit passed, the queue full, its reader ended), each refusal
//!   naming `resolve_ledger_artifact` where a page's names `project_ledger_view_page`; none is
//!   fatal. It is recorded in the writer's commit statistics as a `ResolveArtifact`, where the
//!   writer recorded its own, and not among the pool's pages. Once no reader is left, the writer
//!   resolves again, as it reads the pages.

use super::read_work::ReadWork;
use super::retention::PublishedProofs;
use super::sqlite::SnapshotMemo;
use super::sqlite::lab_relations::{self, LabRelationCell};
use super::store::LedgerStore;
use super::write_failure::LedgerMonotonicClock;
use super::{
    CommitStatistics, GlobalLedgerError, GlobalLedgerResult, LedgerArtifactSelection,
    LedgerProjectViewObservation, LedgerReaderPoolPages, LedgerWriterCommandKind, ProjectViewReply,
    ResolvedLedgerArtifact,
};
use crate::codes::{LedgerCode, LedgerLocation};
use actingcommand_contract::outcome::Location;
use actingcommand_contract::{EventQuery, ProjectionProfile, RuntimeEventQueryPageRequest};
use actingcommand_runtime_database::RuntimeDatabase;
use rusqlite::{Connection, InterruptHandle};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// #422 QR-1: the reader threads of the Runtime writer's pool, unless its configuration names
/// another number (`GlobalLedgerConfig::with_reader_threads`).
pub(super) const DEFAULT_READER_THREADS: usize = 2;
/// #422 QR-1: the most reader threads a pool may have.
pub(super) const MAX_READER_THREADS: usize = 8;

/// #422 QR-1: the committed point a reader reads at, as the writer published it.
pub(super) struct ReadPoint {
    pub(super) head: u64,
    pub(super) head_hash: Option<String>,
    pub(super) marker_state: &'static str,
    pub(super) proofs: Arc<PublishedProofs>,
}

/// #422 QR-1: where the writer publishes its committed point and the readers take it.
#[derive(Default)]
pub(super) struct ReadPoints {
    current: Mutex<Option<Arc<ReadPoint>>>,
}

impl ReadPoints {
    /// Publishes `store`'s committed point; only the writer's thread (or the open, before the
    /// writer runs) calls it, after a commit and before the commit's replies.
    pub(super) fn publish<S: LedgerStore>(&self, store: &S) {
        // Only the writer publishes, so the point is made outside the lock: a reader taking the
        // current point never waits for a copy of the proofs.
        let previous = self.current();
        if let Some(point) = store.read_point(previous.as_deref()) {
            *self.current.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(point));
        }
    }

    /// The point the writer published last; #422 W1-8 (P30): the ledger reads its head as the
    /// published head (`GlobalLedger::published_head`).
    pub(super) fn current(&self) -> Option<Arc<ReadPoint>> {
        self.current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// #422 QR-1: how the pool's readers read: the database, the writer's points, the read budget
/// on the writer's clock and the history reads' bounds, as the writer's own reads.
pub(super) struct ReaderSettings {
    pub(super) database: Arc<RuntimeDatabase>,
    pub(super) points: Arc<ReadPoints>,
    pub(super) clock: LedgerMonotonicClock,
    pub(super) read_budget: Option<Duration>,
    pub(super) read_check_vm_steps: u32,
    pub(super) history_page_rows: usize,
    pub(super) history_scan_rows: usize,
    /// #422 RB-1: the deadline of the build of the pool's Lab relations, counted from its start
    /// (the open's, `ledger_observer.open_timeout_ms`); `None`: the pool builds none.
    pub(super) lab_relations_timeout: Option<Duration>,
}

/// One job of the pool's queue: what to read, where its answer goes, and when its caller stops
/// waiting.
struct ReadJob {
    read: Read,
    reply: Reply,
    /// Fix 1 (review M-1): when the caller stops waiting, on the writer's clock: the submit time
    /// plus the caller's reply limit.
    deadline_ms: u64,
}

/// What a job reads: a view page, or (#422 RA-1) an artifact resolution.
enum Read {
    Page(Box<PageRead>),
    Resolve(Box<ResolveRead>),
}

/// A view page: what the writer's `ProjectViewPage` command carries.
struct PageRead {
    query: EventQuery,
    profile: ProjectionProfile,
    request: RuntimeEventQueryPageRequest,
}

/// #422 RA-1: an artifact resolution: what the writer's `ResolveArtifact` command carries.
struct ResolveRead {
    selection: LedgerArtifactSelection,
    deadline: Instant,
}

/// Where a job's answer goes, of the job's own kind.
enum Reply {
    Page(SyncSender<ProjectViewReply>),
    Resolve(SyncSender<GlobalLedgerResult<ResolvedLedgerArtifact>>),
}

impl Reply {
    /// The operation the job's own refusals name: `project_ledger_view_page` for a page,
    /// `resolve_ledger_artifact` for a resolution (#422 RA-1).
    fn operation(&self) -> Location {
        match self {
            Self::Page(_) => LedgerLocation::ProjectLedgerViewPage.location(),
            Self::Resolve(_) => LedgerLocation::ResolveLedgerArtifact.location(),
        }
    }

    /// Answers the job with `error`, nothing read.
    fn refuse(self, error: GlobalLedgerError) {
        match self {
            Self::Page(response) => {
                let _ = response.send((Err(error), None));
            }
            Self::Resolve(response) => {
                let _ = response.send(Err(error));
            }
        }
    }
}

/// #422 QR-1 fix 1: what the readers and the close share.
#[derive(Default)]
struct PoolState {
    /// Set by the close before it interrupts the readers (review M-1): a reader then answers
    /// every page it takes as refused and reads none.
    closing: AtomicBool,
    /// The readers still running (review L-6).
    live: AtomicUsize,
    /// Whether a reader already ended unexpectedly; only the first end is said.
    ended: AtomicBool,
    /// What the Runtime has not said yet: a reader's unexpected end, the pages' fallback to the
    /// writer.
    findings: Mutex<Vec<GlobalLedgerError>>,
    /// #422 W1-8 (P29): the pages the readers read and their times, since the pool started.
    pages: Mutex<LedgerReaderPoolPages>,
    /// Tests: the readers that end, unexpectedly, when they next take a page.
    #[cfg(test)]
    end_for_test: AtomicUsize,
}

/// #422 QR-1: the reader threads and the queue they take pages from.
pub(super) struct ReaderPool {
    sender: Option<SyncSender<ReadJob>>,
    threads: Vec<JoinHandle<()>>,
    /// Each reader's interrupt, which the close raises so that no read keeps it waiting.
    interrupts: Mutex<Vec<InterruptHandle>>,
    /// The readers' settings and shared state.
    reader: Arc<Reader>,
    /// Fix 1: the queue's end, kept while the pool lives, so that a page queued as the last
    /// reader ends waits for its caller's reply limit (not fatal) instead of being dropped.
    _queue_end: Arc<Mutex<Receiver<ReadJob>>>,
    /// #422 RB-1: the thread that builds the pool's Lab relations, joined by the close.
    lab_relations_build: Option<JoinHandle<()>>,
}

impl ReaderPool {
    /// Opens `threads` read-only connections, then starts one reader on each. A connection that
    /// cannot open, or a thread that cannot start, is the pool's failure; nothing is left running.
    /// Every page a reader reads is recorded in `statistics` (the writer's).
    pub(super) fn start(
        settings: ReaderSettings,
        statistics: Arc<CommitStatistics>,
        threads: usize,
        capacity: usize,
    ) -> GlobalLedgerResult<Self> {
        let operation = LedgerLocation::QueryLedgerView.location();
        let connections = (0..threads.max(1))
            .map(|_| {
                settings
                    .database
                    .read_snapshot_connection(operation)
                    .map_err(GlobalLedgerError::from)
            })
            .collect::<GlobalLedgerResult<Vec<_>>>()?;
        let interrupts = Mutex::new(
            connections
                .iter()
                .map(Connection::get_interrupt_handle)
                .collect(),
        );
        let (sender, receiver) = mpsc::sync_channel(capacity.max(1));
        let receiver = Arc::new(Mutex::new(receiver));
        let lab_relations_timeout = settings.lab_relations_timeout;
        let reader = Arc::new(Reader {
            #[cfg(any(test, feature = "test-hooks"))]
            hook_root: {
                let root = settings.database.root();
                root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
            },
            lab_relations: Arc::new(LabRelationCell::new(
                settings.history_scan_rows,
                lab_relations_timeout.is_some(),
            )),
            settings,
            statistics,
            state: PoolState {
                live: AtomicUsize::new(connections.len()),
                ..PoolState::default()
            },
        });
        let mut pool = Self {
            sender: Some(sender),
            threads: Vec::new(),
            interrupts,
            reader: Arc::clone(&reader),
            _queue_end: Arc::clone(&receiver),
            lab_relations_build: None,
        };
        for (index, connection) in connections.into_iter().enumerate() {
            let receiver = Arc::clone(&receiver);
            let reader = Arc::clone(&reader);
            let spawned = thread::Builder::new()
                .name(format!("actingcommand-ledger-reader-{index}"))
                .spawn(move || reader.serve(connection, &receiver));
            match spawned {
                Ok(thread) => pool.threads.push(thread),
                Err(error) => {
                    pool.close();
                    return Err(GlobalLedgerError::io(
                        LedgerCode::WriterSpawnFailed.code(),
                        LedgerLocation::SpawnWriter.location(),
                        &error,
                    ));
                }
            }
        }
        // #422 RB-1: the build of the pool's Lab relations; one that cannot start is said as a
        // failed build, and the pages read the whole history's relations.
        if let Some(timeout) = lab_relations_timeout {
            pool.lab_relations_build = reader.start_lab_relations_build(timeout);
        }
        Ok(pool)
    }

    /// Queues one page for a caller that waits `reply_timeout` for it; the reply comes on the
    /// returned channel. A full queue is `writer_response_timeout` (not fatal: the caller reads
    /// again); a closed pool is the fatal `writer_unavailable`.
    pub(super) fn submit(
        &self,
        query: EventQuery,
        profile: ProjectionProfile,
        request: RuntimeEventQueryPageRequest,
        reply_timeout: Duration,
    ) -> GlobalLedgerResult<Receiver<ProjectViewReply>> {
        let (response, receiver) = mpsc::sync_channel(1);
        self.enqueue(
            Read::Page(Box::new(PageRead {
                query,
                profile,
                request,
            })),
            Reply::Page(response),
            reply_timeout,
        )?;
        Ok(receiver)
    }

    /// #422 RA-1: queues one artifact resolution for a caller that waits `reply_timeout` for
    /// it, resolved within `deadline` as the writer resolves; the reply comes on the returned
    /// channel. Refused as `submit` refuses a page, naming `resolve_ledger_artifact`.
    pub(super) fn submit_resolution(
        &self,
        selection: LedgerArtifactSelection,
        deadline: Instant,
        reply_timeout: Duration,
    ) -> GlobalLedgerResult<Receiver<GlobalLedgerResult<ResolvedLedgerArtifact>>> {
        let (response, receiver) = mpsc::sync_channel(1);
        self.enqueue(
            Read::Resolve(Box::new(ResolveRead {
                selection,
                deadline,
            })),
            Reply::Resolve(response),
            reply_timeout,
        )?;
        Ok(receiver)
    }

    /// Queues one job, its caller's deadline the writer's clock now plus `reply_timeout`. A full
    /// queue is `writer_response_timeout` (not fatal: the caller reads again); a closed pool is
    /// the fatal `writer_unavailable`; both of the job's own operation.
    fn enqueue(&self, read: Read, reply: Reply, reply_timeout: Duration) -> GlobalLedgerResult<()> {
        let operation = reply.operation();
        let sender = self.sender.as_ref().ok_or_else(|| {
            GlobalLedgerError::fatal(LedgerCode::WriterUnavailable.code(), operation)
        })?;
        let deadline_ms = (self.reader.settings.clock)()
            .saturating_add(u64::try_from(reply_timeout.as_millis()).unwrap_or(u64::MAX));
        match sender.try_send(ReadJob {
            read,
            reply,
            deadline_ms,
        }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(GlobalLedgerError::request(
                LedgerCode::WriterResponseTimeout.code(),
                operation,
            )),
            Err(TrySendError::Disconnected(_)) => Err(GlobalLedgerError::fatal(
                LedgerCode::WriterUnavailable.code(),
                operation,
            )),
        }
    }

    /// The reply to a page `submit` queued, waited for at most `reply_timeout`: not in time is
    /// `writer_response_timeout`, not fatal. Fix 1 (review L-6): a reader that ended with the
    /// page unanswered (it panicked) is the page's `writer_panicked`, not fatal either: the
    /// caller reads again, on a reader left or, once none is, on the writer.
    pub(super) fn receive(
        receiver: &Receiver<ProjectViewReply>,
        reply_timeout: Duration,
    ) -> GlobalLedgerResult<ProjectViewReply> {
        receive_within(
            receiver,
            reply_timeout,
            LedgerLocation::ProjectLedgerViewPage.location(),
        )
    }

    /// #422 RA-1: the reply to a resolution `submit_resolution` queued, waited for as `receive`
    /// waits for a page's, its refusals naming `resolve_ledger_artifact`; none is fatal.
    pub(super) fn receive_resolution(
        receiver: &Receiver<GlobalLedgerResult<ResolvedLedgerArtifact>>,
        reply_timeout: Duration,
    ) -> GlobalLedgerResult<GlobalLedgerResult<ResolvedLedgerArtifact>> {
        receive_within(
            receiver,
            reply_timeout,
            LedgerLocation::ResolveLedgerArtifact.location(),
        )
    }

    /// Fix 1 (review L-6): whether a reader is left to read the pages; once none is, the writer
    /// reads them.
    pub(super) fn serving(&self) -> bool {
        self.reader.state.live.load(Ordering::Acquire) > 0
    }

    /// Fix 1 (review L-6): what the pool has to say that the Runtime has not said yet, each
    /// once: the first reader that ended unexpectedly (`writer_panicked` of `query_ledger_view`)
    /// and, once no reader is left, the pages' fallback to the writer (`writer_panicked` of
    /// `project_ledger_view_page`). The details name the readers left.
    pub(super) fn take_findings(&self) -> Vec<GlobalLedgerError> {
        std::mem::take(&mut *lock(&self.reader.state.findings))
    }

    /// #422 W1-8 (P29): the pages the readers read since the pool started, with their times.
    pub(super) fn pages(&self) -> LedgerReaderPoolPages {
        *lock(&self.reader.state.pages)
    }

    /// Ends the pool. From the close on, no reader starts a page: each page still queued is
    /// answered as refused, `writer_unavailable` of `shutdown_writer` (not fatal), and never read
    /// (review M-1). The readers' connections are interrupted, so a statement running then ends
    /// with its SQL failure; a read that is between two statements then goes on within its read
    /// budget. Every reader is then joined, so the close waits for the reads in progress only. No
    /// caller waits on any of these pages: the close owns the ledger.
    pub(super) fn close(&mut self) {
        self.stop();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        // #422 RB-1: the build stops at its bound's next look once the pool is closing.
        if let Some(build) = self.lab_relations_build.take() {
            let _ = build.join();
        }
    }

    /// #422 RB-1, tests: waits at most `wait` for the build of the pool's Lab relations to end
    /// (`LabRelationCell::settled_for_test`).
    #[cfg(test)]
    pub(super) fn lab_relations_settled_for_test(&self, wait: Duration) -> Option<Option<u64>> {
        self.reader.lab_relations.settled_for_test(wait)
    }

    /// The close's first step (a test takes it alone, then lets a held read go on): the pool is
    /// closing, takes no page any more, and its readers' connections are interrupted.
    pub(super) fn stop(&mut self) {
        self.reader.state.closing.store(true, Ordering::Release);
        self.sender = None;
        for interrupt in lock(&self.interrupts).iter() {
            interrupt.interrupt();
        }
    }

    /// Tests: the next `readers` readers that take a page end there, unexpectedly, without
    /// answering it, as a reader that panics does.
    #[cfg(test)]
    pub(super) fn end_readers_for_test(&self, readers: usize) {
        self.reader
            .state
            .end_for_test
            .store(readers, Ordering::Release);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A job's reply, waited for at most `reply_timeout`: not in time is `writer_response_timeout`
/// of `operation`, a reader that ended with the job unanswered `writer_panicked` of it; neither
/// is fatal.
fn receive_within<T>(
    receiver: &Receiver<T>,
    reply_timeout: Duration,
    operation: Location,
) -> GlobalLedgerResult<T> {
    match receiver.recv_timeout(reply_timeout) {
        Ok(reply) => Ok(reply),
        Err(RecvTimeoutError::Timeout) => Err(GlobalLedgerError::request(
            LedgerCode::WriterResponseTimeout.code(),
            operation,
        )),
        Err(RecvTimeoutError::Disconnected) => Err(GlobalLedgerError::request(
            LedgerCode::WriterPanicked.code(),
            operation,
        )),
    }
}

impl Drop for ReaderPool {
    fn drop(&mut self) {
        self.close();
    }
}

struct Reader {
    settings: ReaderSettings,
    /// Fix 1 (review L-4): the writer's commit statistics, where each page read is recorded.
    statistics: Arc<CommitStatistics>,
    state: PoolState,
    /// #422 RB-1: the pool's Lab relations, which its build makes and its readers read forward.
    lab_relations: Arc<LabRelationCell>,
    /// The root the test hooks name this ledger by (the writer's `hook_root`).
    #[cfg(any(test, feature = "test-hooks"))]
    hook_root: std::path::PathBuf,
}

impl Reader {
    /// #422 RB-1: starts the build of the pool's Lab relations on a read-only connection of its
    /// own, within `timeout` from its start. A connection that does not open, or a thread that
    /// does not start, is the build's failure: said once, never fatal, and the pages read the
    /// whole history's relations.
    fn start_lab_relations_build(self: &Arc<Self>, timeout: Duration) -> Option<JoinHandle<()>> {
        let operation = LedgerLocation::BuildLabRelations.location();
        let connection = match self.settings.database.read_snapshot_connection(operation) {
            Ok(connection) => connection,
            Err(error) => {
                self.say_lab_relations_failure(GlobalLedgerError::from(error));
                self.lab_relations.settle(None);
                return None;
            }
        };
        let builder = Arc::clone(self);
        let spawned = thread::Builder::new()
            .name("actingcommand-ledger-lab-relations".to_owned())
            .spawn(move || builder.build_lab_relations(connection, timeout));
        match spawned {
            Ok(thread) => Some(thread),
            Err(error) => {
                self.say_lab_relations_failure(GlobalLedgerError::io(
                    LedgerCode::WriterSpawnFailed.code(),
                    operation,
                    &error,
                ));
                self.lab_relations.settle(None);
                None
            }
        }
    }

    /// #422 RB-1: the build's thread: the pool's Lab relations through the point the writer
    /// published, before the deadline taken now. The close stops it (`closing`) at its bound's
    /// next look, unsaid; any other end without the relations is said once.
    fn build_lab_relations(self: Arc<Self>, mut connection: Connection, timeout: Duration) {
        let mut end = BuildEnd {
            reader: &self,
            settled: false,
        };
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        let built = match self.settings.points.current() {
            Some(point) => {
                let stopping = Arc::clone(&self);
                lab_relations::build(
                    &mut connection,
                    point.head,
                    deadline,
                    self.settings.read_check_vm_steps,
                    move || stopping.state.closing.load(Ordering::Acquire),
                )
            }
            // The open publishes the first point before the pool starts.
            None => Err(GlobalLedgerError::request(
                LedgerCode::WriterUnavailable.code(),
                LedgerLocation::BuildLabRelations.location(),
            )),
        };
        end.settled = true;
        // A failure is said before the pages are told the build ended.
        match built {
            Ok(index) => self.lab_relations.settle(Some(index)),
            Err(error) => {
                self.say_lab_relations_failure(error);
                self.lab_relations.settle(None);
            }
        }
    }

    /// #422 RB-1: a build of the pool's Lab relations that ended without them, said once as the
    /// readers' ends are (`ReaderPool::take_findings`), unless the close stopped it: a warning of
    /// `build_lab_relations` with the failure's code, never fatal.
    fn say_lab_relations_failure(&self, error: GlobalLedgerError) {
        if self.state.closing.load(Ordering::Acquire) {
            return;
        }
        lock(&self.state.findings).push(error.into_warning_of(
            LedgerLocation::BuildLabRelations.location(),
            "the pages read the whole history's Lab relations until the next start",
        ));
    }

    /// One reader: takes jobs until the pool closes. A job taken once the pool is closing, or
    /// past its caller's deadline, is answered and not read (review M-1). The page read is
    /// recorded in the writer's statistics as a `ProjectViewPage` before it is answered (L-4);
    /// #422 RA-1: a resolution as a `ResolveArtifact`, and not among the pool's pages.
    fn serve(&self, mut connection: Connection, receiver: &Mutex<Receiver<ReadJob>>) {
        let mut end = ReaderEnd {
            state: &self.state,
            receiver,
            answering: None,
        };
        // #422 QR-2: this reader's Lab relations and window starts, kept from one of its pages to
        // the next (fix 1: a few snapshots, the least recently used dropped first). #422 RB-1:
        // with the pool's Lab relations.
        let mut memo = SnapshotMemo::with_lab_relations(Arc::clone(&self.lab_relations));
        loop {
            let job = lock(receiver).recv();
            let Ok(ReadJob {
                read,
                reply,
                deadline_ms,
            }) = job
            else {
                return;
            };
            let operation = reply.operation();
            end.answering = Some(reply);
            #[cfg(test)]
            if self
                .state
                .end_for_test
                .try_update(Ordering::AcqRel, Ordering::Acquire, |left| {
                    left.checked_sub(1)
                })
                .is_ok()
            {
                return;
            }
            if let Some(refusal) = self.refusal(deadline_ms, operation) {
                end.refuse(refusal);
                continue;
            }
            let started = Instant::now();
            match read {
                Read::Page(page) => {
                    let PageRead {
                        query,
                        profile,
                        request,
                    } = *page;
                    let reply = self.page(&mut connection, &query, profile, &request, &mut memo);
                    let elapsed = started.elapsed();
                    self.statistics.record_writer_command(
                        LedgerWriterCommandKind::ProjectViewPage,
                        elapsed,
                        0,
                        false,
                    );
                    // #422 W1-8 (P29): and in the pool's own pages, which the Runtime shows
                    // while it runs (`GlobalLedger::reader_pool_pages`).
                    lock(&self.state.pages).record(elapsed);
                    end.answer_page(reply);
                }
                Read::Resolve(resolve) => {
                    let resolved =
                        self.resolve(&mut connection, &resolve.selection, resolve.deadline);
                    self.statistics.record_writer_command(
                        LedgerWriterCommandKind::ResolveArtifact,
                        started.elapsed(),
                        0,
                        false,
                    );
                    end.answer_resolution(resolved);
                }
            }
        }
    }

    /// Why a job taken now is not read: the pool is closing (`writer_unavailable` of
    /// `shutdown_writer`), or its caller's deadline passed and the caller already has its
    /// `writer_response_timeout` of the job's `operation`, which the job is answered with too.
    /// Neither is fatal.
    fn refusal(&self, deadline_ms: u64, operation: Location) -> Option<GlobalLedgerError> {
        if self.state.closing.load(Ordering::Acquire) {
            return Some(GlobalLedgerError::request(
                LedgerCode::WriterUnavailable.code(),
                LedgerLocation::ShutdownWriter.location(),
            ));
        }
        ((self.settings.clock)() >= deadline_ms).then(|| {
            GlobalLedgerError::request(LedgerCode::WriterResponseTimeout.code(), operation)
        })
    }

    /// #422 RA-1: one artifact resolution at the writer's latest point, within the caller's
    /// `deadline`, as the writer resolves at that head (`sqlite::resolve_artifact_at_point`).
    fn resolve(
        &self,
        connection: &mut Connection,
        selection: &LedgerArtifactSelection,
        deadline: Instant,
    ) -> GlobalLedgerResult<ResolvedLedgerArtifact> {
        let point = self.settings.points.current();
        // #411 A51, tests: a read hold of this root holds the pool's resolutions too, once they
        // took their point, so that the writer commits meanwhile (an eviction among them) while
        // the held resolution reads at the point it took.
        #[cfg(any(test, feature = "test-hooks"))]
        crate::read_hold::hold_reads_of(&self.hook_root);
        #[cfg(any(test, feature = "test-hooks"))]
        let recording = crate::writer_statements::start(&self.hook_root);
        #[cfg(any(test, feature = "test-hooks"))]
        let started = std::time::Instant::now();
        let resolved = match point {
            Some(point) => super::sqlite::resolve_artifact_at_point(
                &self.settings.database,
                connection,
                &point,
                selection,
                deadline,
            ),
            // The open publishes the first point before the pool starts.
            None => Err(GlobalLedgerError::fatal(
                LedgerCode::WriterUnavailable.code(),
                LedgerLocation::ResolveLedgerArtifact.location(),
            )),
        };
        #[cfg(any(test, feature = "test-hooks"))]
        crate::writer_statements::finish(
            recording,
            LedgerWriterCommandKind::ResolveArtifact,
            started.elapsed(),
            (0, 0),
        );
        resolved
    }

    /// One page at the writer's latest point, within the read budget counted from now. #422
    /// QR-2: with the reader's `memo` (`SnapshotMemo`).
    fn page(
        &self,
        connection: &mut Connection,
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
        memo: &mut SnapshotMemo,
    ) -> ProjectViewReply {
        // #411 A51, tests: a read hold of this root holds the pool's pages too, as it held them
        // on the writer; it no longer holds the writer's other work.
        #[cfg(any(test, feature = "test-hooks"))]
        crate::read_hold::hold_reads_of(&self.hook_root);
        #[cfg(any(test, feature = "test-hooks"))]
        let recording = crate::writer_statements::start(&self.hook_root);
        #[cfg(any(test, feature = "test-hooks"))]
        let started = std::time::Instant::now();
        let settings = &self.settings;
        let work = ReadWork::started(&settings.clock, (settings.clock)(), settings.read_budget)
            .within_statements(settings.read_check_vm_steps);
        let mut observation = Some(LedgerProjectViewObservation::default());
        let page = match settings.points.current() {
            Some(point) => super::sqlite::view_page_at_point(
                &settings.database,
                connection,
                &point,
                (query, profile, request),
                &mut observation,
                (
                    &work,
                    settings.history_page_rows,
                    settings.history_scan_rows,
                ),
                memo,
            ),
            // The open publishes the first point before the pool starts.
            None => Err(GlobalLedgerError::fatal(
                LedgerCode::WriterUnavailable.code(),
                LedgerLocation::ProjectLedgerViewPage.location(),
            )),
        };
        #[cfg(any(test, feature = "test-hooks"))]
        crate::writer_statements::finish(
            recording,
            super::LedgerWriterCommandKind::ProjectViewPage,
            started.elapsed(),
            work.bound_callbacks(),
        );
        (page, observation)
    }
}

/// #422 RB-1: the end of the build's thread. A build that unwinds from a panic leaves the pool
/// without its index, said once as `writer_panicked` of `build_lab_relations`, never fatal: the
/// pages read the whole history's relations.
struct BuildEnd<'a> {
    reader: &'a Reader,
    settled: bool,
}

impl Drop for BuildEnd<'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.reader
                .say_lab_relations_failure(GlobalLedgerError::request(
                    LedgerCode::WriterPanicked.code(),
                    LedgerLocation::BuildLabRelations.location(),
                ));
            self.reader.lab_relations.settle(None);
        }
    }
}

/// #422 QR-1 fix 1 (review L-6): the end of one reader's thread, also when it unwinds from a
/// panic. A page the reader took and has not answered (it ended inside it) is answered
/// `writer_panicked` of `project_ledger_view_page`, not fatal: its caller reads again. An end the
/// close did not ask for is unexpected and is said before that answer: the first one once
/// (`writer_panicked` of `query_ledger_view`), and the end that leaves no reader as the pages'
/// fallback to the writer (`writer_panicked` of `project_ledger_view_page`). The pages still
/// queued then are answered `writer_response_timeout` (not fatal) and not read; every later page
/// is read by the writer. #422 RA-1: a resolution is answered the same way, each answer naming
/// `resolve_ledger_artifact`, and once no reader is left the writer resolves again.
struct ReaderEnd<'a> {
    state: &'a PoolState,
    receiver: &'a Mutex<Receiver<ReadJob>>,
    /// The reply channel of the job the reader took and has not answered yet.
    answering: Option<Reply>,
}

impl ReaderEnd<'_> {
    /// Answers the page the reader took.
    fn answer_page(&mut self, reply: ProjectViewReply) {
        if let Some(Reply::Page(response)) = self.answering.take() {
            let _ = response.send(reply);
        }
    }

    /// #422 RA-1: answers the resolution the reader took.
    fn answer_resolution(&mut self, reply: GlobalLedgerResult<ResolvedLedgerArtifact>) {
        if let Some(Reply::Resolve(response)) = self.answering.take() {
            let _ = response.send(reply);
        }
    }

    /// Answers the job the reader took with `error`, nothing read.
    fn refuse(&mut self, error: GlobalLedgerError) {
        if let Some(reply) = self.answering.take() {
            reply.refuse(error);
        }
    }
}

impl Drop for ReaderEnd<'_> {
    fn drop(&mut self) {
        let live = self
            .state
            .live
            .fetch_sub(1, Ordering::AcqRel)
            .saturating_sub(1);
        let unexpected = !self.state.closing.load(Ordering::Acquire);
        if unexpected {
            let mut findings = lock(&self.state.findings);
            if !self.state.ended.swap(true, Ordering::AcqRel) {
                findings.push(
                    GlobalLedgerError::request(
                        LedgerCode::WriterPanicked.code(),
                        LedgerLocation::QueryLedgerView.location(),
                    )
                    .with_detail(format!("reader_ended readers_left={live}")),
                );
            }
            if live == 0 {
                findings.push(
                    GlobalLedgerError::request(
                        LedgerCode::WriterPanicked.code(),
                        LedgerLocation::ProjectLedgerViewPage.location(),
                    )
                    .with_detail("readers_left=0 fallback=writer".to_owned()),
                );
            }
        }
        if let Some(reply) = self.answering.take() {
            let operation = reply.operation();
            reply.refuse(GlobalLedgerError::request(
                LedgerCode::WriterPanicked.code(),
                operation,
            ));
        }
        if !unexpected || live > 0 {
            return;
        }
        let receiver = lock(self.receiver);
        while let Ok(job) = receiver.try_recv() {
            let operation = job.reply.operation();
            job.reply.refuse(GlobalLedgerError::request(
                LedgerCode::WriterResponseTimeout.code(),
                operation,
            ));
        }
    }
}
