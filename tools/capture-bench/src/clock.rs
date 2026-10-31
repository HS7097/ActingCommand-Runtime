// SPDX-License-Identifier: AGPL-3.0-only

//! The bench's clocks. Every time the bench reads or waits on goes through [`Clock`]:
//!
//! - [`SystemClock`] is the host's monotonic clock (QueryPerformanceCounter through `Instant`) and
//!   wall clock, with real sleeps.
//! - [`VirtualClock`] is the deterministic test clock of the D1 rehearsal (plan v2.1 §6.2): no
//!   test reads a real clock or sleeps. Threads that take part in a run register as
//!   participants. Virtual time stands still while any participant runs; when every participant
//!   sleeps or waits, the clock jumps to the earliest wake time and wakes exactly that one
//!   participant (ties by registration order). A participant that waits on another thread
//!   (a lock, a handoff, a join) says so with [`Clock::block_begin`] and [`Clock::block_end`],
//!   and whoever releases it wakes it with [`Clock::wake`], so time never jumps over work that
//!   is ready to run.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Microseconds on a clock's monotonic scale (since the clock's origin).
pub type Micros = u64;

pub const MICROS_PER_MILLI: u64 = 1_000;
pub const MICROS_PER_SECOND: u64 = 1_000_000;

pub fn ms(value: u64) -> Micros {
    value.saturating_mul(MICROS_PER_MILLI)
}

/// A participant's id on its clock; `0` means "not a participant".
pub type ParticipantId = u64;

pub trait Clock: Send + Sync {
    /// Monotonic microseconds since the clock's origin.
    fn now_us(&self) -> Micros;
    /// Wall-clock Unix milliseconds.
    fn unix_ms(&self) -> u64;
    /// Sleeps until `deadline` or until [`Clock::wake`] names the calling participant.
    fn sleep_until_us(&self, deadline: Micros);
    /// Registers a new participant (the caller's future child) and returns its id; the child
    /// adopts it with [`Clock::adopt`]. Registering in the parent keeps time from moving between
    /// the spawn and the child's first step.
    fn register(&self) -> ParticipantId;
    /// The calling thread becomes participant `id`.
    fn adopt(&self, id: ParticipantId);
    /// Participant `id` leaves.
    fn retire(&self, id: ParticipantId);
    /// The calling thread's participant id, `0` when it is none.
    fn current(&self) -> ParticipantId;
    /// The calling participant starts waiting on another thread.
    fn block_begin(&self);
    /// The calling participant stopped waiting.
    fn block_end(&self);
    /// Makes participant `id` runnable now (it was sleeping or waiting).
    fn wake(&self, id: ParticipantId);
    /// The `Instant` of a time on this clock, for device calls that take one; `None` on a
    /// virtual clock.
    fn instant_at(&self, _at: Micros) -> Option<Instant> {
        None
    }
    /// The time on this clock of an `Instant`; `None` on a virtual clock.
    fn micros_of(&self, _instant: Instant) -> Option<Micros> {
        None
    }

    fn sleep_us(&self, duration: Micros) {
        self.sleep_until_us(self.now_us().saturating_add(duration));
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Sleeps until `deadline` even when a wake ends a sleep early.
pub fn pause_until(clock: &dyn Clock, deadline: Micros) {
    while clock.now_us() < deadline {
        clock.sleep_until_us(deadline);
    }
}

/// Runs `wait` as a wait on another thread.
pub fn blocking<T>(clock: &dyn Clock, wait: impl FnOnce() -> T) -> T {
    clock.block_begin();
    let value = wait();
    clock.block_end();
    value
}

thread_local! {
    static PARTICIPANT: Cell<(usize, ParticipantId)> = const { Cell::new((0, 0)) };
}

fn set_participant(clock: usize, id: ParticipantId) {
    PARTICIPANT.with(|cell| cell.set((clock, id)));
}

fn participant_of(clock: usize) -> ParticipantId {
    PARTICIPANT.with(|cell| {
        let (owner, id) = cell.get();
        if owner == clock { id } else { 0 }
    })
}

/// The host's clocks. `now_us` is the performance counter in microseconds (the same scale in
/// every process of the host, §5.1 跨进程共同时间基准), anchored to an `Instant` for device calls.
pub struct SystemClock {
    origin: Instant,
    origin_us: Micros,
    next: AtomicU64,
    parked: Mutex<BTreeMap<ParticipantId, (Thread, Arc<AtomicBool>)>>,
}

impl SystemClock {
    pub fn new() -> Self {
        let origin = Instant::now();
        Self {
            origin,
            origin_us: crate::winproc::qpc_micros().unwrap_or(0),
            next: AtomicU64::new(1),
            parked: Mutex::new(BTreeMap::new()),
        }
    }

    fn address(&self) -> usize {
        self as *const Self as usize
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now_us(&self) -> Micros {
        self.origin_us
            .saturating_add(u64::try_from(self.origin.elapsed().as_micros()).unwrap_or(u64::MAX))
    }

    fn unix_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            })
    }

    fn sleep_until_us(&self, deadline: Micros) {
        let id = participant_of(self.address());
        let woken = (id != 0)
            .then(|| {
                lock(&self.parked)
                    .get(&id)
                    .map(|(_, flag)| Arc::clone(flag))
            })
            .flatten();
        loop {
            let now = self.now_us();
            if now >= deadline {
                return;
            }
            if let Some(flag) = &woken
                && flag.swap(false, Ordering::AcqRel)
            {
                return;
            }
            let remaining = Duration::from_micros(deadline - now);
            if woken.is_some() {
                thread::park_timeout(remaining);
            } else {
                thread::sleep(remaining);
            }
        }
    }

    fn register(&self) -> ParticipantId {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    fn adopt(&self, id: ParticipantId) {
        set_participant(self.address(), id);
        lock(&self.parked).insert(id, (thread::current(), Arc::new(AtomicBool::new(false))));
    }

    fn retire(&self, id: ParticipantId) {
        lock(&self.parked).remove(&id);
        if participant_of(self.address()) == id {
            set_participant(0, 0);
        }
    }

    fn current(&self) -> ParticipantId {
        participant_of(self.address())
    }

    fn block_begin(&self) {}

    fn block_end(&self) {}

    fn wake(&self, id: ParticipantId) {
        if let Some((thread, flag)) = lock(&self.parked).get(&id) {
            flag.store(true, Ordering::Release);
            thread.unpark();
        }
    }

    fn instant_at(&self, at: Micros) -> Option<Instant> {
        match at.checked_sub(self.origin_us) {
            Some(after) => self.origin.checked_add(Duration::from_micros(after)),
            None => self
                .origin
                .checked_sub(Duration::from_micros(self.origin_us - at)),
        }
    }

    fn micros_of(&self, instant: Instant) -> Option<Micros> {
        match instant.checked_duration_since(self.origin) {
            Some(since) => u64::try_from(since.as_micros())
                .ok()
                .map(|since| self.origin_us.saturating_add(since)),
            None => u64::try_from(self.origin.duration_since(instant).as_micros())
                .ok()
                .and_then(|before| self.origin_us.checked_sub(before)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParticipantState {
    Running,
    Sleeping(Micros),
    Blocked,
}

struct VirtualState {
    now: Micros,
    next: ParticipantId,
    participants: BTreeMap<ParticipantId, ParticipantState>,
}

/// The deterministic test clock (see the module documentation).
pub struct VirtualClock {
    state: Mutex<VirtualState>,
    changed: Condvar,
    unix_origin_ms: u64,
}

impl VirtualClock {
    pub fn new(unix_origin_ms: u64) -> Self {
        Self {
            state: Mutex::new(VirtualState {
                now: 0,
                next: 1,
                participants: BTreeMap::new(),
            }),
            changed: Condvar::new(),
            unix_origin_ms,
        }
    }

    fn address(&self) -> usize {
        self as *const Self as usize
    }

    /// When no participant runs, wakes the one that sleeps until the earliest time.
    fn schedule(state: &mut VirtualState) {
        if state
            .participants
            .values()
            .any(|participant| *participant == ParticipantState::Running)
        {
            return;
        }
        let next = state
            .participants
            .iter()
            .filter_map(|(id, participant)| match participant {
                ParticipantState::Sleeping(deadline) => Some((*deadline, *id)),
                _ => None,
            })
            .min();
        if let Some((deadline, id)) = next {
            state.now = state.now.max(deadline);
            state.participants.insert(id, ParticipantState::Running);
        }
    }

    /// Moves virtual time forward by `duration` from outside the run (a test that is no
    /// participant); the participants whose time came are woken in order.
    pub fn advance(&self, duration: Micros) {
        let mut state = lock(&self.state);
        state.now = state.now.saturating_add(duration);
        let now = state.now;
        let due: Vec<ParticipantId> = state
            .participants
            .iter()
            .filter_map(|(id, participant)| match participant {
                ParticipantState::Sleeping(deadline) if *deadline <= now => Some(*id),
                _ => None,
            })
            .collect();
        if let Some(first) = due.first() {
            state.participants.insert(*first, ParticipantState::Running);
        }
        drop(state);
        self.changed.notify_all();
    }

    /// The number of registered participants (for tests).
    pub fn participants(&self) -> usize {
        lock(&self.state).participants.len()
    }
}

impl Clock for VirtualClock {
    fn now_us(&self) -> Micros {
        lock(&self.state).now
    }

    fn unix_ms(&self) -> u64 {
        self.unix_origin_ms + self.now_us() / MICROS_PER_MILLI
    }

    fn sleep_until_us(&self, deadline: Micros) {
        let id = participant_of(self.address());
        let mut state = lock(&self.state);
        if id == 0 || !state.participants.contains_key(&id) {
            while state.now < deadline {
                state = self
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            return;
        }
        if state.now >= deadline {
            return;
        }
        state
            .participants
            .insert(id, ParticipantState::Sleeping(deadline));
        Self::schedule(&mut state);
        self.changed.notify_all();
        while state.participants.get(&id) != Some(&ParticipantState::Running) {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn register(&self) -> ParticipantId {
        let mut state = lock(&self.state);
        let id = state.next;
        state.next += 1;
        state.participants.insert(id, ParticipantState::Running);
        id
    }

    fn adopt(&self, id: ParticipantId) {
        set_participant(self.address(), id);
    }

    fn retire(&self, id: ParticipantId) {
        let mut state = lock(&self.state);
        state.participants.remove(&id);
        Self::schedule(&mut state);
        drop(state);
        self.changed.notify_all();
        if participant_of(self.address()) == id {
            set_participant(0, 0);
        }
    }

    fn current(&self) -> ParticipantId {
        participant_of(self.address())
    }

    fn block_begin(&self) {
        let id = participant_of(self.address());
        if id == 0 {
            return;
        }
        let mut state = lock(&self.state);
        if state.participants.contains_key(&id) {
            state.participants.insert(id, ParticipantState::Blocked);
            Self::schedule(&mut state);
        }
        drop(state);
        self.changed.notify_all();
    }

    fn block_end(&self) {
        let id = participant_of(self.address());
        if id == 0 {
            return;
        }
        let mut state = lock(&self.state);
        if state.participants.contains_key(&id) {
            state.participants.insert(id, ParticipantState::Running);
        }
    }

    fn wake(&self, id: ParticipantId) {
        let mut state = lock(&self.state);
        if matches!(
            state.participants.get(&id),
            Some(ParticipantState::Sleeping(_) | ParticipantState::Blocked)
        ) {
            state.participants.insert(id, ParticipantState::Running);
        }
        drop(state);
        self.changed.notify_all();
    }
}

/// A one-value handoff between threads that wakes its waiter on the clock.
pub struct Handoff<T> {
    clock: Arc<dyn Clock>,
    slot: Mutex<(Option<T>, ParticipantId)>,
    filled: Condvar,
}

impl<T> Handoff<T> {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            slot: Mutex::new((None, 0)),
            filled: Condvar::new(),
        }
    }

    pub fn put(&self, value: T) {
        let mut slot = lock(&self.slot);
        slot.0 = Some(value);
        let waiter = slot.1;
        if waiter != 0 {
            self.clock.wake(waiter);
        }
        drop(slot);
        self.filled.notify_all();
    }

    /// Waits for the value.
    pub fn take(&self) -> T {
        let mut slot = lock(&self.slot);
        loop {
            if let Some(value) = slot.0.take() {
                slot.1 = 0;
                drop(slot);
                self.clock.block_end();
                return value;
            }
            slot.1 = self.clock.current();
            self.clock.block_begin();
            slot = self
                .filled
                .wait(slot)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// The value when it is there by `deadline` on the clock, otherwise `None`.
    pub fn take_until(&self, deadline: Micros) -> Option<T> {
        loop {
            {
                let mut slot = lock(&self.slot);
                if let Some(value) = slot.0.take() {
                    slot.1 = 0;
                    return Some(value);
                }
                if self.clock.now_us() >= deadline {
                    slot.1 = 0;
                    return None;
                }
                slot.1 = self.clock.current();
            }
            self.clock.sleep_until_us(deadline);
        }
    }

    pub fn try_take(&self) -> Option<T> {
        lock(&self.slot).0.take()
    }

    pub fn is_filled(&self) -> bool {
        lock(&self.slot).0.is_some()
    }
}

/// A participant thread whose result its joiner waits for on the clock.
pub struct Participant<T> {
    result: Arc<Handoff<std::thread::Result<T>>>,
    handle: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> Participant<T> {
    /// Waits for the thread's result; a panic in the thread returns `Err`.
    pub fn join(mut self) -> std::thread::Result<T> {
        let result = self.result.take();
        if let Some(handle) = self.handle.take() {
            // The thread has handed its result over and only retires now.
            let _ = handle.join();
        }
        result
    }

    pub fn is_finished(&self) -> bool {
        self.result.is_filled()
    }
}

/// Spawns `body` on a participant thread named `name`.
pub fn spawn_participant<T, F>(
    clock: &Arc<dyn Clock>,
    name: &str,
    body: F,
) -> std::io::Result<Participant<T>>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let id = clock.register();
    let result = Arc::new(Handoff::new(Arc::clone(clock)));
    let thread_result = Arc::clone(&result);
    let thread_clock = Arc::clone(clock);
    let spawned = thread::Builder::new().name(name.to_owned()).spawn(move || {
        thread_clock.adopt(id);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        thread_result.put(outcome);
        thread_clock.retire(id);
    });
    match spawned {
        Ok(handle) => Ok(Participant {
            result,
            handle: Some(handle),
        }),
        Err(error) => {
            clock.retire(id);
            Err(error)
        }
    }
}
