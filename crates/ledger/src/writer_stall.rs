// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 A (LED-I3 = APPS-I2 = HOST-I4): a test-only stall barrier before the durable
//! commit. While a gate is armed for a state root, every ledger append of that root stops inside
//! its write transaction, just before the commit: the writer thread and the shared SQLite
//! connection stay busy, as during a long commit or checkpoint. Releasing (or dropping) the gate
//! lets the stopped commits finish. Released by the test, never by a timer.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};

struct Barrier {
    root: PathBuf,
    released: Mutex<bool>,
    changed: Condvar,
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
            released: Mutex::new(false),
            changed: Condvar::new(),
        });
        armed()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::clone(&barrier));
        Ok(Self { barrier })
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
        *self
            .barrier
            .released
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = true;
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
    let mut released = barrier
        .released
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    while !*released {
        released = barrier
            .changed
            .wait(released)
            .unwrap_or_else(PoisonError::into_inner);
    }
}
