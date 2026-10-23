// SPDX-License-Identifier: AGPL-3.0-only

//! P0-LO: a test-only slow writer open. While a gate is armed for a state root, every formal
//! writer open of that root takes the gate's delay longer in its SQL read, without waiting: the
//! read starts as if that much of the open's time had already passed, so its deadline comes that
//! much sooner and its measured `sql_read_ms` is that much longer. Removed by dropping the gate.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

struct Delay {
    root: PathBuf,
    delay: Duration,
}

fn armed() -> &'static Mutex<Vec<Arc<Delay>>> {
    static ARMED: OnceLock<Mutex<Vec<Arc<Delay>>>> = OnceLock::new();
    ARMED.get_or_init(|| Mutex::new(Vec::new()))
}

/// Makes every writer open of one state root `delay` slower, until the gate is dropped.
pub struct LedgerOpenDelayGate {
    delay: Arc<Delay>,
}

impl LedgerOpenDelayGate {
    /// Arms the gate for `state_root`, an existing directory (the Runtime's state root).
    pub fn arm(state_root: &Path, delay: Duration) -> std::io::Result<Self> {
        let delay = Arc::new(Delay {
            root: state_root.canonicalize()?,
            delay,
        });
        armed()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::clone(&delay));
        Ok(Self { delay })
    }
}

impl Drop for LedgerOpenDelayGate {
    fn drop(&mut self) {
        armed()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|delay| !Arc::ptr_eq(delay, &self.delay));
    }
}

/// Called by the writer open of `root` before its SQL read: the read's start and the open's
/// deadline, each moved back by the delay armed for that root; unchanged when none is armed.
pub(crate) fn slowed(root: &Path, started: Instant, deadline: Instant) -> (Instant, Instant) {
    let delays = armed()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if delays.is_empty() {
        return (started, deadline);
    }
    let Ok(root) = root.canonicalize() else {
        return (started, deadline);
    };
    let Some(delay) = delays.iter().find(|delay| delay.root == root) else {
        return (started, deadline);
    };
    // An instant earlier than the platform clock's origin cannot be formed (a delay longer than
    // the time since boot); the open then runs at its real speed.
    started
        .checked_sub(delay.delay)
        .zip(deadline.checked_sub(delay.delay))
        .unwrap_or((started, deadline))
}
