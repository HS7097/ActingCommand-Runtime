// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-WS: a test-only record of the SQLite statements one state root's writer runs for its
//! read commands. While a recorder is armed for a state root, the writer of that root records,
//! for every read command it handles, the statements the SQLite medium read rows with (each
//! statement's SQL text, its first bound value read as a stored sequence and its
//! virtual-machine steps, `SQLITE_STMTSTATUS_VM_STEP`), the command's processing time and the
//! progress callbacks of the command's in-statement read bound. A command is recorded on the
//! thread its reads run on (the writer's, or, #422 QR-1, a reader's of the pool), once it has
//! replied: a caller reads the record after a later command of the same writer has answered.
//! FLAKY-LNWS1: each thread's command keeps its own record, so a command on one thread that
//! starts or ends while one on another thread runs leaves that one's statements to it. Recording
//! changes nothing the writer reads or returns. Dropping the recorder stops recording.

use crate::global::LedgerWriterCommandKind;
use rusqlite::types::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::{self, ThreadId};
use std::time::Duration;

/// One statement the SQLite medium read rows with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerStatementRun {
    /// The statement's SQL text.
    pub sql: String,
    /// Its first bound value read as a stored sequence (a history page's lower bound, the
    /// statement's `e.sequence>?`), when that value is an integer.
    pub after: Option<u64>,
    /// Its SQLite virtual-machine steps.
    pub vm_steps: u64,
    /// #422 RB-1: the rows it returned.
    pub rows: u64,
}

/// What one read command of the writer did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerWriterCommandStatements {
    pub command: LedgerWriterCommandKind,
    /// The command's handling in the writer, its reply included.
    pub processing: Duration,
    /// The statements it read rows with, in the order they ended.
    pub statements: Vec<LedgerStatementRun>,
    /// LN-WS: the progress callbacks of its in-statement read bound.
    pub bound_callbacks: u64,
    /// LN-WS: those of them that ran at or after the read's budget point.
    pub late_callbacks: u64,
}

struct Recorder {
    root: PathBuf,
    /// The commands being recorded, one per thread they run on (FLAKY-LNWS1: the writer's and
    /// the pool's readers' at once): each command's statements so far.
    current: Mutex<HashMap<ThreadId, Vec<LedgerStatementRun>>>,
    commands: Mutex<Vec<LedgerWriterCommandStatements>>,
}

fn armed() -> &'static Mutex<Vec<Arc<Recorder>>> {
    static ARMED: OnceLock<Mutex<Vec<Arc<Recorder>>>> = OnceLock::new();
    ARMED.get_or_init(|| Mutex::new(Vec::new()))
}

/// Records the read commands of one state root's writer until it is dropped.
pub struct WriterStatementRecorder {
    recorder: Arc<Recorder>,
}

impl WriterStatementRecorder {
    /// Arms the recorder for `state_root`, an existing directory (the ledger's root).
    pub fn arm(state_root: &Path) -> std::io::Result<Self> {
        let recorder = Arc::new(Recorder {
            root: state_root.canonicalize()?,
            current: Mutex::new(HashMap::new()),
            commands: Mutex::new(Vec::new()),
        });
        armed()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::clone(&recorder));
        Ok(Self { recorder })
    }

    /// The read commands recorded since the last call, oldest first. A command is recorded once
    /// it has replied; one whose caller has the answer of a later command of the same writer is
    /// here.
    pub fn take(&self) -> Vec<LedgerWriterCommandStatements> {
        std::mem::take(
            &mut *self
                .recorder
                .commands
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }
}

impl Drop for WriterStatementRecorder {
    fn drop(&mut self) {
        armed()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|recorder| !Arc::ptr_eq(recorder, &self.recorder));
    }
}

/// One read command being recorded on the thread it runs on.
pub(crate) struct Recording {
    recorder: Arc<Recorder>,
    /// The thread it started on, whose record it is.
    thread: ThreadId,
}

/// Called by the writer of `root` (its canonical root), or a reader of its pool, when a read
/// command starts on the calling thread; `None` unless a recorder is armed for that root.
pub(crate) fn start(root: &Path) -> Option<Recording> {
    let recorder = armed()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|recorder| recorder.root == root)
        .cloned()?;
    let thread = thread::current().id();
    recorder
        .current
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(thread, Vec::new());
    Some(Recording { recorder, thread })
}

/// Called by the writer, or a reader of its pool, when the recorded command ended, with its
/// kind, its processing time and its read bound's callbacks (all of them, and those at or after
/// its budget point). It takes only its own thread's record.
pub(crate) fn finish(
    recording: Option<Recording>,
    command: LedgerWriterCommandKind,
    processing: Duration,
    (bound_callbacks, late_callbacks): (u64, u64),
) {
    let Some(recording) = recording else {
        return;
    };
    let statements = recording
        .recorder
        .current
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&recording.thread)
        .unwrap_or_default();
    recording
        .recorder
        .commands
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(LedgerWriterCommandStatements {
            command,
            processing,
            statements,
            bound_callbacks,
            late_callbacks,
        });
}

/// Called by the SQLite medium when a statement it read rows with is done, with its SQL text,
/// its bound values, its virtual-machine steps and (#422 RB-1) the rows it returned: recorded
/// when a command of this thread is.
pub(crate) fn statement_done(sql: &str, parameters: &[Value], steps: i32, rows: usize) {
    let recorders = armed()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if recorders.is_empty() {
        return;
    }
    let thread = thread::current().id();
    for recorder in recorders {
        let mut current = recorder
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(statements) = current.get_mut(&thread) {
            statements.push(LedgerStatementRun {
                sql: sql.to_owned(),
                after: match parameters.first() {
                    // The medium's order-preserving `encode` (`global/sqlite.rs`) undone.
                    Some(Value::Integer(value)) => Some((*value as u64) ^ (1_u64 << 63)),
                    _ => None,
                },
                vm_steps: u64::try_from(steps).unwrap_or(0),
                rows: u64::try_from(rows).unwrap_or(u64::MAX),
            });
        }
    }
}

#[cfg(test)]
#[path = "tests/writer_statements.rs"]
mod tests;
