// SPDX-License-Identifier: AGPL-3.0-only

//! §3.3.1: the bench's same-shape locks and the lock-order check (`bench-lockcheck`).
//!
//! The locks are logical: a lock has an owner (a [`LockOwner`], one per logical thread of the
//! bench) instead of a guard, so an operation can hold it across its steps (§3.3.2) and a
//! deterministic test can run several logical threads on one OS thread (§4.4). Ranks:
//! admission 1, queue order 2, scheduler 3, leaf 9. A blocking acquisition must raise the rank
//! strictly (1 → 2 → 3, 1 → 3, 2 → 3); nothing is taken under a leaf. A `try_acquire` never
//! waits, so it may run against the order ("持有秩 2 或 3 时从不等秩 1，只许 try_lock"). Every
//! violation is recorded as `lock_order_violation`, an internal invariant violation (§3.11.3).

use crate::clock::Clock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

pub const RANK_ADMISSION: u8 = 1;
pub const RANK_QUEUE_ORDER: u8 = 2;
pub const RANK_SCHEDULER: u8 = 3;
pub const RANK_LEAF: u8 = 9;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The run's lock-order violations.
#[derive(Default)]
pub struct LockCheck {
    violations: Mutex<Vec<String>>,
}

impl LockCheck {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn record(&self, violation: String) {
        lock(&self.violations).push(violation);
    }

    pub fn violations(&self) -> Vec<String> {
        lock(&self.violations).clone()
    }

    pub fn count(&self) -> usize {
        lock(&self.violations).len()
    }
}

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

/// One logical thread's identity and the locks it holds.
pub struct LockOwner {
    id: u64,
    name: String,
    held: Mutex<Vec<(u8, String)>>,
}

impl LockOwner {
    pub fn new(name: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            id: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
            name: name.into(),
            held: Mutex::new(Vec::new()),
        })
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The ranks this owner holds, in acquisition order.
    pub fn held_ranks(&self) -> Vec<u8> {
        lock(&self.held).iter().map(|(rank, _)| *rank).collect()
    }

    /// Whether this owner holds any lock of `rank`.
    pub fn holds_rank(&self, rank: u8) -> bool {
        lock(&self.held).iter().any(|(held, _)| *held == rank)
    }

    fn push(&self, rank: u8, name: &str) {
        lock(&self.held).push((rank, name.to_owned()));
    }

    fn remove(&self, rank: u8, name: &str) {
        let mut held = lock(&self.held);
        if let Some(position) = held
            .iter()
            .rposition(|(held_rank, held_name)| *held_rank == rank && held_name == name)
        {
            held.remove(position);
        }
    }
}

struct LockState {
    owner: Option<u64>,
    waiters: Vec<u64>,
}

/// One logical lock.
pub struct RankedLock {
    rank: u8,
    name: String,
    state: Mutex<LockState>,
    released: Condvar,
    check: Arc<LockCheck>,
    clock: Arc<dyn Clock>,
}

impl RankedLock {
    pub fn new(
        rank: u8,
        name: impl Into<String>,
        check: Arc<LockCheck>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            rank,
            name: name.into(),
            state: Mutex::new(LockState {
                owner: None,
                waiters: Vec::new(),
            }),
            released: Condvar::new(),
            check,
            clock,
        }
    }

    pub fn rank(&self) -> u8 {
        self.rank
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    #[cfg(feature = "bench-lockcheck")]
    fn check_blocking_order(&self, who: &LockOwner) {
        let held = who.held_ranks();
        let violation = if self.rank == RANK_LEAF {
            held.contains(&RANK_LEAF)
        } else {
            held.iter().any(|rank| *rank >= self.rank)
        };
        if violation {
            self.check.record(format!(
                "lock_order_violation: {} waits for {} (rank {}) while holding ranks {held:?}",
                who.name, self.name, self.rank
            ));
        }
    }

    #[cfg(not(feature = "bench-lockcheck"))]
    fn check_blocking_order(&self, _who: &LockOwner) {}

    /// Takes the lock, waiting while another owner holds it.
    pub fn acquire(&self, who: &LockOwner) {
        self.check_blocking_order(who);
        let mut state = lock(&self.state);
        if state.owner == Some(who.id) {
            self.check.record(format!(
                "lock_order_violation: {} takes {} a second time",
                who.name, self.name
            ));
            return;
        }
        let participant = self.clock.current();
        let mut waited = false;
        while state.owner.is_some() {
            if !waited {
                state.waiters.push(participant);
                waited = true;
            }
            self.clock.block_begin();
            state = self
                .released
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if waited {
            if let Some(position) = state.waiters.iter().position(|id| *id == participant) {
                state.waiters.remove(position);
            }
            self.clock.block_end();
        }
        state.owner = Some(who.id);
        drop(state);
        who.push(self.rank, &self.name);
    }

    /// Takes the lock when it is free; never waits.
    pub fn try_acquire(&self, who: &LockOwner) -> bool {
        let mut state = lock(&self.state);
        if state.owner.is_some() {
            return false;
        }
        state.owner = Some(who.id);
        drop(state);
        who.push(self.rank, &self.name);
        true
    }

    /// Whether the lock is free now (a non-blocking step checks before it takes).
    pub fn is_free(&self) -> bool {
        lock(&self.state).owner.is_none()
    }

    pub fn held_by(&self, who: &LockOwner) -> bool {
        lock(&self.state).owner == Some(who.id)
    }

    /// Releases the lock held by `who`; a release by anyone else is a violation.
    pub fn release(&self, who: &LockOwner) {
        let mut state = lock(&self.state);
        if state.owner != Some(who.id) {
            self.check.record(format!(
                "lock_order_violation: {} releases {} it does not hold",
                who.name, self.name
            ));
            return;
        }
        state.owner = None;
        let waiters = state.waiters.clone();
        drop(state);
        who.remove(self.rank, &self.name);
        for waiter in waiters {
            if waiter != 0 {
                self.clock.wake(waiter);
            }
        }
        self.released.notify_all();
    }

    /// Hands the lock from `from` to `to` without releasing it (the pump's grant, §3.3.2 P3).
    pub fn transfer(&self, from: &LockOwner, to: &LockOwner) {
        let mut state = lock(&self.state);
        if state.owner != Some(from.id) {
            self.check.record(format!(
                "lock_order_violation: {} hands over {} it does not hold",
                from.name, self.name
            ));
            return;
        }
        state.owner = Some(to.id);
        drop(state);
        from.remove(self.rank, &self.name);
        to.push(self.rank, &self.name);
    }
}
