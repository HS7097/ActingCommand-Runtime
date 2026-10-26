// SPDX-License-Identifier: AGPL-3.0-only

//! P0-LO: a test-only slow writer open. While a gate is armed for a state root, every formal
//! writer open of that root takes the gate's delay longer in its first phase, without waiting:
//! the open starts as if that much of its time had already passed, so its deadline comes that
//! much sooner and its measured first phase (LN-8: `meta_ms`) is that much longer. Removed by
//! dropping the gate. LN-8d (#422): a gate armed with [`LedgerOpenDelayGate::arm_index`] moves
//! the deadline instead when the open's `index` phase begins (its retention reads), after the
//! `meta` and `tail` phases ran at their real speed.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

struct Delay {
    root: PathBuf,
    delay: Duration,
    /// LN-8d: whether the delay falls at the `index` phase's start instead of the open's.
    index: bool,
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
        Self::armed_at(state_root, delay, false)
    }

    /// LN-8d (#422): makes the `index` phase of every writer open of one state root start
    /// `delay` late, until the gate is dropped: its deadline comes that much sooner.
    pub fn arm_index(state_root: &Path, delay: Duration) -> std::io::Result<Self> {
        Self::armed_at(state_root, delay, true)
    }

    fn armed_at(state_root: &Path, delay: Duration, index: bool) -> std::io::Result<Self> {
        let delay = Arc::new(Delay {
            root: state_root.canonicalize()?,
            delay,
            index,
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

/// The delay armed for `root` at the open's start (`index` false) or at its `index` phase.
fn armed_for(root: &Path, index: bool) -> Option<Duration> {
    let delays = armed()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if delays.is_empty() {
        return None;
    }
    let root = root.canonicalize().ok()?;
    delays
        .iter()
        .find(|delay| delay.root == root && delay.index == index)
        .map(|delay| delay.delay)
}

/// Called by the writer open of `root` before its first read: the open's start and its
/// deadline, each moved back by the delay armed for that root; unchanged when none is armed.
pub(crate) fn slowed(root: &Path, started: Instant, deadline: Instant) -> (Instant, Instant) {
    let Some(delay) = armed_for(root, false) else {
        return (started, deadline);
    };
    // An instant earlier than the platform clock's origin cannot be formed (a delay longer than
    // the time since boot); the open then runs at its real speed.
    started
        .checked_sub(delay)
        .zip(deadline.checked_sub(delay))
        .unwrap_or((started, deadline))
}

/// LN-8d (#422): called by the writer open of `root` when its `index` phase begins: its
/// deadline moved back by the index delay armed for that root; unchanged when none is armed.
pub(crate) fn slowed_index(root: &Path, deadline: Instant) -> Instant {
    armed_for(root, true)
        .and_then(|delay| deadline.checked_sub(delay))
        .unwrap_or(deadline)
}
