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
//! - **Fail loud.** A pool whose connections cannot open does not start; the ledger keeps its
//!   failure ([`super::GlobalLedger::reader_pool_finding`]) and the writer reads the pages, as
//!   before the pool, which the Runtime says as a warning at its start.

use super::read_work::ReadWork;
use super::retention::PublishedProofs;
use super::store::LedgerStore;
use super::write_failure::LedgerMonotonicClock;
use super::{
    GlobalLedgerError, GlobalLedgerResult, LedgerProjectViewObservation, ProjectViewReply,
};
use crate::codes::{LedgerCode, LedgerLocation};
use actingcommand_contract::{EventQuery, ProjectionProfile, RuntimeEventQueryPageRequest};
use actingcommand_runtime_database::RuntimeDatabase;
use rusqlite::{Connection, InterruptHandle};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

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

    fn current(&self) -> Option<Arc<ReadPoint>> {
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
}

struct ReadJob {
    query: EventQuery,
    profile: ProjectionProfile,
    request: RuntimeEventQueryPageRequest,
    response: SyncSender<ProjectViewReply>,
}

/// #422 QR-1: the reader threads and the queue they take pages from.
pub(super) struct ReaderPool {
    sender: Option<SyncSender<ReadJob>>,
    threads: Vec<JoinHandle<()>>,
    /// Each reader's interrupt, which the close raises so that no read keeps it waiting.
    interrupts: Mutex<Vec<InterruptHandle>>,
}

impl ReaderPool {
    /// Opens `threads` read-only connections, then starts one reader on each. A connection that
    /// cannot open, or a thread that cannot start, is the pool's failure; nothing is left running.
    pub(super) fn start(
        settings: ReaderSettings,
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
        let reader = Arc::new(Reader {
            #[cfg(any(test, feature = "test-hooks"))]
            hook_root: {
                let root = settings.database.root();
                root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
            },
            settings,
        });
        let mut pool = Self {
            sender: Some(sender),
            threads: Vec::new(),
            interrupts,
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
        Ok(pool)
    }

    /// Queues one page; the reply comes on the returned channel. A full queue is
    /// `writer_response_timeout` (not fatal: the caller reads again); a pool whose readers all
    /// ended is the fatal `writer_unavailable`.
    pub(super) fn submit(
        &self,
        query: EventQuery,
        profile: ProjectionProfile,
        request: RuntimeEventQueryPageRequest,
    ) -> GlobalLedgerResult<Receiver<ProjectViewReply>> {
        let operation = LedgerLocation::ProjectLedgerViewPage.location();
        let sender = self.sender.as_ref().ok_or_else(|| {
            GlobalLedgerError::fatal(LedgerCode::WriterUnavailable.code(), operation)
        })?;
        let (response, receiver) = mpsc::sync_channel(1);
        match sender.try_send(ReadJob {
            query,
            profile,
            request,
            response,
        }) {
            Ok(()) => Ok(receiver),
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

    /// Ends the pool: no page is taken any more, a read in progress is interrupted (its caller
    /// gets its SQL failure) and every reader is joined.
    pub(super) fn close(&mut self) {
        self.sender = None;
        for interrupt in self
            .interrupts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            interrupt.interrupt();
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Drop for ReaderPool {
    fn drop(&mut self) {
        self.close();
    }
}

struct Reader {
    settings: ReaderSettings,
    /// The root the test hooks name this ledger by (the writer's `hook_root`).
    #[cfg(any(test, feature = "test-hooks"))]
    hook_root: std::path::PathBuf,
}

impl Reader {
    /// One reader: takes pages until the pool closes.
    fn serve(&self, mut connection: Connection, receiver: &Mutex<Receiver<ReadJob>>) {
        loop {
            let job = {
                let receiver = receiver.lock().unwrap_or_else(PoisonError::into_inner);
                receiver.recv()
            };
            let Ok(job) = job else {
                return;
            };
            let reply = self.page(&mut connection, &job.query, job.profile, &job.request);
            let _ = job.response.send(reply);
        }
    }

    /// One page at the writer's latest point, within the read budget counted from now.
    fn page(
        &self,
        connection: &mut Connection,
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
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
