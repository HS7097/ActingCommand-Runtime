// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #383 AO-1: test-only write faults of one state root (`contracts/ledger-observer.md`
//! §9). While a gate is armed for a state root, the ledger's write attempts of that root meet the
//! faults the test queued, in the order it queued them: an attempt that fails before it commits,
//! as on a full disk; an attempt that commits and then reports a failure, as when the commit's
//! result is lost; an integrity failure of the store. A number of attempts may pass first. A
//! dump place may be refused, as on a full disk. Dropping the gate removes what is left.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

struct Entry {
    root: PathBuf,
    faults: Mutex<Faults>,
}

#[derive(Default)]
struct Faults {
    /// Attempts that pass before the queued faults are met.
    passing: usize,
    queued: VecDeque<InjectedFault>,
    refused_dumps: Vec<PathBuf>,
    /// Attempts that met a fault since the gate was armed.
    met: usize,
}

/// One fault a write attempt meets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InjectedFault {
    /// The attempt fails before it commits (a full disk).
    FailBeforeCommit,
    /// The attempt commits, then reports a failure.
    CommitThenFail,
    /// The store reports an integrity failure.
    Integrity,
}

fn armed() -> &'static Mutex<Vec<Arc<Entry>>> {
    static ARMED: OnceLock<Mutex<Vec<Arc<Entry>>>> = OnceLock::new();
    ARMED.get_or_init(|| Mutex::new(Vec::new()))
}

/// The write faults of one state root, until the gate is dropped.
pub struct LedgerWriteFaultGate {
    entry: Arc<Entry>,
}

impl LedgerWriteFaultGate {
    /// Arms the gate for `state_root`, an existing directory (the Runtime's state root).
    pub fn arm(state_root: &Path) -> std::io::Result<Self> {
        let entry = Arc::new(Entry {
            root: state_root.canonicalize()?,
            faults: Mutex::new(Faults::default()),
        });
        armed()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::clone(&entry));
        Ok(Self { entry })
    }

    fn faults(&self) -> MutexGuard<'_, Faults> {
        self.entry
            .faults
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn queue(&self, fault: InjectedFault, count: usize) {
        self.faults()
            .queued
            .extend(std::iter::repeat_n(fault, count));
    }

    /// The next `count` write attempts fail before they commit, as on a full disk.
    pub fn fail_writes(&self, count: usize) {
        self.queue(InjectedFault::FailBeforeCommit, count);
    }

    /// The next `count` write attempts commit and then report a failure.
    pub fn commit_then_fail(&self, count: usize) {
        self.queue(InjectedFault::CommitThenFail, count);
    }

    /// The next `count` write attempts meet an integrity failure of the store.
    pub fn integrity_failure(&self, count: usize) {
        self.queue(InjectedFault::Integrity, count);
    }

    /// The next `passing` write attempts pass before the queued faults are met.
    pub fn pass_first(&self, passing: usize) {
        self.faults().passing = passing;
    }

    /// A dump file at `path` cannot be written (a full disk).
    pub fn refuse_dump_at(&self, path: &Path) {
        self.faults().refused_dumps.push(path.to_path_buf());
    }

    /// Write attempts that met a fault so far.
    pub fn met(&self) -> usize {
        self.faults().met
    }
}

impl Drop for LedgerWriteFaultGate {
    fn drop(&mut self) {
        armed()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|entry| !Arc::ptr_eq(entry, &self.entry));
    }
}

/// Called by a write attempt of `root` (the database's canonical root) before it commits: the
/// fault it meets, if a gate queued one.
pub(crate) fn before_commit(root: &Path) -> Option<InjectedFault> {
    let entry = armed()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|entry| entry.root == root)
        .cloned()?;
    let mut faults = entry.faults.lock().unwrap_or_else(PoisonError::into_inner);
    if faults.queued.is_empty() {
        return None;
    }
    if faults.passing > 0 {
        faults.passing -= 1;
        return None;
    }
    let fault = faults.queued.pop_front()?;
    faults.met += 1;
    Some(fault)
}

/// Whether a gate refuses the dump file at `path`.
pub(crate) fn dump_refused(path: &Path) -> bool {
    armed()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .any(|entry| {
            entry
                .faults
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .refused_dumps
                .iter()
                .any(|refused| refused == path)
        })
}
