// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 A (LED-I3 = APPS-I2 = HOST-I4): a test-only stall barrier before the durable
//! commit. While a gate is armed for a state root, every ledger append of that root stops inside
//! its write transaction, just before the commit: the writer thread and the shared SQLite
//! connection stay busy, as during a long commit or checkpoint. Releasing (or dropping) the gate
//! lets the stopped commits finish. Released by the test, never by a timer.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::Duration;

struct Barrier {
    root: PathBuf,
    state: Mutex<BarrierState>,
    changed: Condvar,
}

#[derive(Default)]
struct BarrierState {
    released: bool,
    /// Commits that reached the barrier since it was armed.
    held: usize,
}

fn armed() -> &'static Mutex<Vec<Arc<Barrier>>> {
    static ARMED: OnceLock<Mutex<Vec<Arc<Barrier>>>> = OnceLock::new();
    ARMED.get_or_init(|| Mutex::new(Vec::new()))
}

/// Holds every durable ledger commit of one state root before it commits, until the gate is
/// released or dropped.
pub struct WriterStallGate {
    barrier: Arc<Barrier>,
}

impl WriterStallGate {
    /// Arms the gate for `state_root`, an existing directory (the Runtime's state root).
    pub fn arm(state_root: &Path) -> std::io::Result<Self> {
        let barrier = Arc::new(Barrier {
            root: state_root.canonicalize()?,
            state: Mutex::new(BarrierState::default()),
            changed: Condvar::new(),
        });
        armed()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::clone(&barrier));
        Ok(Self { barrier })
    }

    /// Whether a commit is held at the gate, waiting at most `wait` for one to arrive.
    pub fn wait_held_commit(&self, wait: Duration) -> bool {
        let state = self
            .barrier
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (state, _) = self
            .barrier
            .changed
            .wait_timeout_while(state, wait, |state| state.held == 0)
            .unwrap_or_else(PoisonError::into_inner);
        state.held > 0
    }

    /// Lets every stopped commit finish; later commits no longer stop.
    pub fn release(self) {
        drop(self);
    }
}

impl Drop for WriterStallGate {
    fn drop(&mut self) {
        armed()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|barrier| !Arc::ptr_eq(barrier, &self.barrier));
        self.barrier
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .released = true;
        self.barrier.changed.notify_all();
    }
}

/// Called by the durable commit of `root` (the database's canonical root) right before it
/// commits; returns at once unless a gate is armed for that root.
pub(crate) fn before_durable_commit(root: &Path) {
    let barrier = armed()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|barrier| barrier.root == root)
        .cloned();
    let Some(barrier) = barrier else {
        return;
    };
    let mut state = barrier.state.lock().unwrap_or_else(PoisonError::into_inner);
    state.held += 1;
    barrier.changed.notify_all();
    while !state.released {
        state = barrier
            .changed
            .wait(state)
            .unwrap_or_else(PoisonError::into_inner);
    }
}
