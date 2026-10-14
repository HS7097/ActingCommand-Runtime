// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #383 AO-1: test-only write faults of one state root (`contracts/ledger-observer.md`
//! §9). While a gate is armed for a state root, the ledger's write attempts of that root meet the
//! faults the test queued, in the order it queued them: an attempt that fails before it commits,
//! as on a full disk; an attempt that commits and then reports a failure, as when the commit's
//! result is lost; an integrity failure of the store. A number of attempts may pass first. A
//! dump place may be refused, as on a full disk. Dropping the gate removes what is left.
//!
//! Workflow #383 AO-2: the gate also lists the batches the ledger committed (their sequences and
//! whether they were FULL synced), and may hold the spill thread before each spill write, as a
//! slow disk would, until the test releases it.
//!
//! Workflow #382 L-3 (LED-I1): an attempt may also report a failure at its COMMIT while its
//! transaction did not take effect, the other side of an uncertain COMMIT.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

struct Entry {
    root: PathBuf,
    faults: Mutex<Faults>,
    spill_changed: Condvar,
}

#[derive(Default)]
struct Faults {
    /// Attempts that pass before the queued faults are met.
    passing: usize,
    queued: VecDeque<InjectedFault>,
    refused_dumps: Vec<PathBuf>,
    /// Attempts that met a fault since the gate was armed.
    met: usize,
    /// Workflow #383 AO-2: the batches committed since the gate was armed.
    committed: Vec<(Vec<u64>, bool)>,
    /// Workflow #383 AO-2: spill writes wait while this is set.
    spills_held: bool,
    /// Spill writes that reached the gate since it was armed.
    spills_reached: usize,
    /// Spill writes that landed in the dump file since the gate was armed.
    spills_written: usize,
}

/// One fault a write attempt meets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InjectedFault {
    /// The attempt fails before it commits (a full disk).
    FailBeforeCommit,
    /// The attempt commits, then reports a failure.
    CommitThenFail,
    /// Workflow #382 L-3 (LED-I1): the attempt reports a failure at its COMMIT, and its
    /// transaction did not take effect (it is rolled back).
    CommitFailedUncommitted,
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
            spill_changed: Condvar::new(),
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

    /// Workflow #382 L-3 (LED-I1): the next `count` write attempts report a failure at their
    /// COMMIT while their transaction does not take effect.
    pub fn commit_fails_uncommitted(&self, count: usize) {
        self.queue(InjectedFault::CommitFailedUncommitted, count);
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

    /// Workflow #383 AO-2: the batches committed since the gate was armed, in order: each
    /// batch's ledger sequences and whether it was FULL synced.
    pub fn commits(&self) -> Vec<(Vec<u64>, bool)> {
        self.faults().committed.clone()
    }

    /// Workflow #383 AO-2: spill writes of this root wait before they write, until
    /// [`Self::release_spills`] or the gate is dropped.
    pub fn hold_spills(&self) {
        self.faults().spills_held = true;
    }

    /// Lets the held spill writes go on.
    pub fn release_spills(&self) {
        self.faults().spills_held = false;
        self.entry.spill_changed.notify_all();
    }

    /// Whether a spill write reached the gate, waiting at most `wait` for one.
    pub fn wait_spill_reached(&self, wait: Duration) -> bool {
        let faults = self.faults();
        let (faults, _) = self
            .entry
            .spill_changed
            .wait_timeout_while(faults, wait, |faults| faults.spills_reached == 0)
            .unwrap_or_else(PoisonError::into_inner);
        faults.spills_reached > 0
    }

    /// Whether `count` spill writes landed in the dump file, waiting at most `wait` for them.
    pub fn wait_spills_written(&self, count: usize, wait: Duration) -> bool {
        let faults = self.faults();
        let (faults, _) = self
            .entry
            .spill_changed
            .wait_timeout_while(faults, wait, |faults| faults.spills_written < count)
            .unwrap_or_else(PoisonError::into_inner);
        faults.spills_written >= count
    }
}

impl Drop for LedgerWriteFaultGate {
    fn drop(&mut self) {
        armed()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|entry| !Arc::ptr_eq(entry, &self.entry));
        self.faults().spills_held = false;
        self.entry.spill_changed.notify_all();
    }
}

fn entry_of(root: &Path) -> Option<Arc<Entry>> {
    armed()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|entry| entry.root == root)
        .cloned()
}

/// Workflow #383 AO-2: called by a committed batch of `root`: its sequences and its sync level.
pub(crate) fn committed_batch(root: &Path, sequences: Vec<u64>, full: bool) {
    if let Some(entry) = entry_of(root) {
        entry
            .faults
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .committed
            .push((sequences, full));
    }
}

/// Workflow #383 AO-2: called by the spill thread once a spill landed in the dump file at
/// `dump_path` (in its state root).
pub(crate) fn after_spill(dump_path: &Path) {
    let Some(entry) = dump_path.parent().and_then(entry_of) else {
        return;
    };
    entry
        .faults
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .spills_written += 1;
    entry.spill_changed.notify_all();
}

/// Workflow #383 AO-2: called by the spill thread before it writes to the dump file at
/// `dump_path` (in its state root); waits while a gate holds the spills of that root.
pub(crate) fn before_spill(dump_path: &Path) {
    let Some(entry) = dump_path.parent().and_then(entry_of) else {
        return;
    };
    let mut faults = entry.faults.lock().unwrap_or_else(PoisonError::into_inner);
    faults.spills_reached += 1;
    entry.spill_changed.notify_all();
    while faults.spills_held {
        faults = entry
            .spill_changed
            .wait(faults)
            .unwrap_or_else(PoisonError::into_inner);
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
