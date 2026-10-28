// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A51: a test-only hold of the writer inside a read. While a gate is armed for a state
//! root, the writer of that root stops at the start of every read command (a query, a page, a
//! projection, the frame view, an artifact lookup, the head) until the gate is released or
//! dropped: the writer is busy in a read, as during a read that does not end, and every later
//! command, an append included, waits behind it. Released by the test, never by a timer.
//!
//! #422 QR-1: a view page of a writer with a reader pool is read by the pool, off the writer; the
//! gate holds it there, at the start of its read, and the writer goes on with its other commands.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::Duration;

struct Hold {
    root: PathBuf,
    state: Mutex<HoldState>,
    changed: Condvar,
}

#[derive(Default)]
struct HoldState {
    released: bool,
    /// Reads that reached the hold since it was armed.
    held: usize,
    /// #419 DIAG-RUN: the reads that still pass the hold before it holds one.
    passing: usize,
}

fn armed() -> &'static Mutex<Vec<Arc<Hold>>> {
    static ARMED: OnceLock<Mutex<Vec<Arc<Hold>>>> = OnceLock::new();
    ARMED.get_or_init(|| Mutex::new(Vec::new()))
}

/// Holds every read command of one state root's writer at its start, until the gate is
/// released or dropped.
pub struct WriterReadHoldGate {
    hold: Arc<Hold>,
}

impl WriterReadHoldGate {
    /// Arms the gate for `state_root`, an existing directory (the ledger's root).
    pub fn arm(state_root: &Path) -> std::io::Result<Self> {
        Self::arm_after(state_root, 0)
    }

    /// #419 DIAG-RUN: arms the gate for `state_root`; the next `passing` reads pass it, and it
    /// holds the reads after them.
    pub fn arm_after(state_root: &Path, passing: usize) -> std::io::Result<Self> {
        let hold = Arc::new(Hold {
            root: state_root.canonicalize()?,
            state: Mutex::new(HoldState {
                passing,
                ..HoldState::default()
            }),
            changed: Condvar::new(),
        });
        armed()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::clone(&hold));
        Ok(Self { hold })
    }

    /// Whether a read is held at the gate, waiting at most `wait` for one to arrive.
    pub fn wait_held(&self, wait: Duration) -> bool {
        let state = self
            .hold
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (state, _) = self
            .hold
            .changed
            .wait_timeout_while(state, wait, |state| state.held == 0)
            .unwrap_or_else(PoisonError::into_inner);
        state.held > 0
    }

    /// Lets the held read go on; later reads no longer stop.
    pub fn release(self) {
        drop(self);
    }
}

impl Drop for WriterReadHoldGate {
    fn drop(&mut self) {
        armed()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|hold| !Arc::ptr_eq(hold, &self.hold));
        self.hold
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .released = true;
        self.hold.changed.notify_all();
    }
}

/// Called by the writer of `root` (its canonical root) at the start of each read command, and by
/// its reader pool at the start of each page (#422 QR-1); returns at once unless a gate is armed
/// for that root.
pub(crate) fn hold_reads_of(root: &Path) {
    let hold = armed()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|hold| hold.root == root)
        .cloned();
    let Some(hold) = hold else {
        return;
    };
    let mut state = hold.state.lock().unwrap_or_else(PoisonError::into_inner);
    if state.passing > 0 {
        state.passing -= 1;
        return;
    }
    state.held += 1;
    hold.changed.notify_all();
    while !state.released {
        state = hold
            .changed
            .wait(state)
            .unwrap_or_else(PoisonError::into_inner);
    }
}
