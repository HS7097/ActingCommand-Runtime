// SPDX-License-Identifier: AGPL-3.0-only

//! §3.5.2 and §3.11: the close path, the supervising thread, the heartbeat and the control
//! files.
//!
//! - Every close is announced on disk first (`closing` in `events.jsonl` and `closing.json`,
//!   synced), then runs on a close thread of its own; the caller waits for it, the supervisor
//!   watches its fixed deadline (S 20 s, N 15 s, device-side check 10 s, written here and not
//!   changeable by a plan).
//! - The supervisor takes none of the bench's rank 1-3 locks. When a close passes its deadline
//!   it records `close_unconfirmed {reason: close_not_returned}`, puts the whole run into its
//!   terminal state with the result `unconfirmed`, lets the other backends close under their own
//!   deadlines, writes `summary.json` and exits the process with code 20. With self-exit turned
//!   off (a test switch) it writes `terminal_pending_since` into the heartbeat instead.
//! - The heartbeat (every second) and the control-file watcher (every 250 ms) never wait for a
//!   close, so a `STOP` or `ABORT` written while a close hangs is still recorded.

use crate::backend::{IdentityTuple, PreviewBackend, StreamFacts};
use crate::clock::{Clock, Handoff, Micros, ms, pause_until, spawn_participant};
use crate::journal::Journal;
use crate::model::{BackendKind, CloseOutcome, RunResult, StopCause};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The bench's fixed limits (§3.5.2, §3.10, ruling 141). Production uses
/// [`FixedLimits::production`]; only the tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FixedLimits {
    pub scrcpy_close_deadline_us: Micros,
    pub nemu_close_deadline_us: Micros,
    pub device_check_deadline_us: Micros,
    /// The bench's own working-set cap, against its own leaks (ruling 141 keeps only this one
    /// memory limit).
    pub working_set_cap_bytes: u64,
    /// The D: free-space floor (§3.11.2 B).
    pub disk_min_bytes: u64,
    pub supervisor_period_us: Micros,
    pub heartbeat_period_us: Micros,
    pub control_period_us: Micros,
    pub metrics_period_us: Micros,
    pub controller_period_us: Micros,
    /// Whether the supervisor exits the process after `close_not_returned`.
    pub self_exit: bool,
}

impl FixedLimits {
    pub const fn production() -> Self {
        Self {
            scrcpy_close_deadline_us: 20_000_000,
            nemu_close_deadline_us: 15_000_000,
            device_check_deadline_us: 10_000_000,
            working_set_cap_bytes: 1024 * 1024 * 1024,
            disk_min_bytes: 10 * 1000 * 1000 * 1000,
            supervisor_period_us: 250_000,
            heartbeat_period_us: 1_000_000,
            control_period_us: 250_000,
            metrics_period_us: 1_000_000,
            controller_period_us: 100_000,
            self_exit: true,
        }
    }

    pub fn close_deadline(&self, kind: BackendKind) -> Micros {
        match kind {
            BackendKind::Scrcpy => self.scrcpy_close_deadline_us,
            BackendKind::Nemu => self.nemu_close_deadline_us,
        }
    }
}

/// How the process ends: production exits, a test records the code.
pub trait ExitHook: Send + Sync {
    fn exit(&self, code: i32);
    /// Self-exit is off (a test switch) and the run is `terminal_pending` since `since_us`.
    fn pending(&self, _since_us: Micros) {}
}

/// The production exit.
pub struct ProcessExit;

impl ExitHook for ProcessExit {
    fn exit(&self, code: i32) {
        std::process::exit(code);
    }
}

/// The run's terminal requests (§3.11.2 B): the first cause decides what is recorded as the
/// stop reason; every cause is kept for `summary.json`.
#[derive(Default)]
pub struct Terminal {
    causes: Mutex<Vec<(Micros, StopCause)>>,
    /// The supervisor ended the run (`close_not_returned`); the runner must not write on.
    finished_by_supervisor: AtomicBool,
    pending_since_us: AtomicU64,
}

impl Terminal {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Records a terminal request; returns true when it is the first.
    pub fn request(&self, at: Micros, cause: StopCause) -> bool {
        let mut causes = lock(&self.causes);
        let first = causes.is_empty();
        if !causes.iter().any(|(_, known)| *known == cause) {
            causes.push((at, cause));
        }
        first
    }

    pub fn first(&self) -> Option<StopCause> {
        lock(&self.causes).first().map(|(_, cause)| cause.clone())
    }

    pub fn all(&self) -> Vec<StopCause> {
        lock(&self.causes)
            .iter()
            .map(|(_, cause)| cause.clone())
            .collect()
    }

    pub fn requested(&self) -> bool {
        !lock(&self.causes).is_empty()
    }

    pub fn finished_by_supervisor(&self) -> bool {
        self.finished_by_supervisor.load(Ordering::Acquire)
    }

    fn mark_finished_by_supervisor(&self) {
        self.finished_by_supervisor.store(true, Ordering::Release);
    }

    /// `terminal_pending_since` (µs), 0 when none.
    pub fn pending_since(&self) -> Micros {
        self.pending_since_us.load(Ordering::Acquire)
    }
}

/// What the close path needs from the run when the supervisor has to end it.
pub trait Emergency: Send + Sync {
    /// Closes every backend still open that no stuck call holds, each under its deadline, and
    /// returns their records.
    fn close_remaining(&self) -> Vec<Value>;
    /// Writes `summary.json` with `result` forced in.
    fn write_summary(&self, result: RunResult, reason: &str);
}

/// One close announced in `closing.json`.
#[derive(Debug, Clone, Serialize)]
pub struct CloseEntry {
    pub close_id: u64,
    pub reason: String,
    pub instance: String,
    pub backend: BackendKind,
    pub generation: u64,
    pub stream: Option<StreamFacts>,
    pub identity: Option<IdentityTuple>,
    pub started_us: Micros,
    pub deadline_us: Micros,
    pub state: String,
}

/// What a close is for.
#[derive(Debug, Clone)]
pub struct CloseInfo {
    pub reason: String,
    pub instance: String,
    pub generation: u64,
    pub identity: Option<IdentityTuple>,
}

/// The close path and its supervisor.
pub struct Closer {
    clock: Arc<dyn Clock>,
    journal: Arc<Journal>,
    terminal: Arc<Terminal>,
    limits: FixedLimits,
    exit: Arc<dyn ExitHook>,
    entries: Mutex<BTreeMap<u64, CloseEntry>>,
    next: AtomicU64,
    emergency: OnceLock<Arc<dyn Emergency>>,
    stop: AtomicBool,
    declared: AtomicBool,
}

impl Closer {
    pub fn new(
        clock: Arc<dyn Clock>,
        journal: Arc<Journal>,
        terminal: Arc<Terminal>,
        limits: FixedLimits,
        exit: Arc<dyn ExitHook>,
    ) -> Arc<Self> {
        Arc::new(Self {
            clock,
            journal,
            terminal,
            limits,
            exit,
            entries: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(1),
            emergency: OnceLock::new(),
            stop: AtomicBool::new(false),
            declared: AtomicBool::new(false),
        })
    }

    pub fn set_emergency(&self, emergency: Arc<dyn Emergency>) {
        let _ = self.emergency.set(emergency);
    }

    pub fn limits(&self) -> &FixedLimits {
        &self.limits
    }

    /// The closes in progress, for the heartbeat.
    pub fn in_progress(&self) -> Vec<Value> {
        let now = self.clock.now_us();
        lock(&self.entries)
            .values()
            .filter(|entry| entry.state == "closing")
            .map(|entry| {
                json!({
                    "close_id": entry.close_id,
                    "instance": entry.instance,
                    "backend": entry.backend,
                    "reason": entry.reason,
                    "elapsed_ms": now.saturating_sub(entry.started_us) / 1_000,
                })
            })
            .collect()
    }

    fn write_closing_file(&self) {
        let entries: Vec<CloseEntry> = lock(&self.entries)
            .values()
            .filter(|entry| entry.state == "closing")
            .cloned()
            .collect();
        let _ = self.journal.write_json("closing.json", &entries);
    }

    /// Closes `backend` (§3.5.2): `closing` on disk first, the close on its own thread, the
    /// caller waits; the supervisor watches the deadline. Returns the outcome, or unconfirmed
    /// when the `closing` record could not be written (nothing is closed then).
    pub fn close(&self, mut backend: Box<dyn PreviewBackend>, info: CloseInfo) -> CloseOutcome {
        let close_id = self.next.fetch_add(1, Ordering::Relaxed);
        let kind = backend.kind();
        let now = self.clock.now_us();
        let entry = CloseEntry {
            close_id,
            reason: info.reason.clone(),
            instance: info.instance.clone(),
            backend: kind,
            generation: info.generation,
            stream: backend.stream(),
            identity: info.identity.clone(),
            started_us: now,
            deadline_us: now.saturating_add(self.limits.close_deadline(kind)),
            state: "closing".to_owned(),
        };
        let announced = serde_json::to_value(&entry).unwrap_or(Value::Null);
        lock(&self.entries).insert(close_id, entry);
        self.write_closing_file();
        if let Err(error) = self.journal.event_durable("closing", announced) {
            // The record of the close is not on disk: the close is not made, and the resource
            // stays open and unconfirmed (fail loud; the run stops).
            if let Some(entry) = lock(&self.entries).get_mut(&close_id) {
                entry.state = "not_started".to_owned();
            }
            let outcome = CloseOutcome::Unconfirmed(format!("closing record not written: {error}"));
            std::mem::forget(backend);
            return outcome;
        }
        let returned: Arc<Handoff<CloseOutcome>> = Arc::new(Handoff::new(Arc::clone(&self.clock)));
        let thread_returned = Arc::clone(&returned);
        let spawned = spawn_participant(&self.clock, &format!("close-{close_id}"), move || {
            let outcome = backend.close();
            thread_returned.put(outcome);
            // The backend's host resources are closed now (or the close failed and the outcome
            // says so); dropping it repeats nothing (both device backends cache their first close).
            drop(backend);
        });
        let outcome = match spawned {
            Ok(_thread) => returned.take(),
            Err(error) => {
                CloseOutcome::Unconfirmed(format!("the close thread did not start: {error}"))
            }
        };
        let declared = {
            let mut entries = lock(&self.entries);
            match entries.get_mut(&close_id) {
                Some(entry) if entry.state == "declared_unconfirmed" => true,
                Some(entry) => {
                    entry.state = if outcome.is_confirmed() {
                        "closed".to_owned()
                    } else {
                        "close_unconfirmed".to_owned()
                    };
                    false
                }
                None => false,
            }
        };
        if declared {
            // The supervisor recorded this close as not returned; a late return is recorded as
            // information and changes nothing (§3.5 item 2).
            self.journal.event(
                "close_returned_late",
                json!({ "close_id": close_id, "outcome": outcome }),
            );
            return CloseOutcome::Unconfirmed("close_not_returned".to_owned());
        }
        let kind_name = if outcome.is_confirmed() {
            "closed"
        } else {
            "close_unconfirmed"
        };
        let _ = self.journal.event_durable(
            kind_name,
            json!({
                "close_id": close_id,
                "instance": info.instance,
                "backend": kind,
                "reason": info.reason,
                "result": if outcome.is_confirmed() { "Confirmed" } else { "Unconfirmed" },
                "release_ok": outcome.is_confirmed(),
                "outcome": outcome,
            }),
        );
        self.write_closing_file();
        outcome
    }

    /// The supervising thread: never takes a bench lock, reads only the close table.
    pub fn spawn_supervisor(self: &Arc<Self>) -> std::io::Result<crate::clock::Participant<()>> {
        let closer = Arc::clone(self);
        spawn_participant(&self.clock, "supervisor", move || {
            let mut next = closer.clock.now_us();
            while !closer.stop.load(Ordering::Acquire) {
                next = next.saturating_add(closer.limits.supervisor_period_us);
                pause_until(closer.clock.as_ref(), next);
                if closer.stop.load(Ordering::Acquire) {
                    break;
                }
                closer.check_deadlines();
            }
        })
    }

    pub fn stop_supervisor(&self) {
        self.stop.store(true, Ordering::Release);
    }

    fn check_deadlines(&self) {
        let now = self.clock.now_us();
        let overdue: Vec<CloseEntry> = {
            let mut entries = lock(&self.entries);
            let mut overdue = Vec::new();
            for entry in entries.values_mut() {
                if entry.state == "closing" && now >= entry.deadline_us {
                    entry.state = "declared_unconfirmed".to_owned();
                    overdue.push(entry.clone());
                }
            }
            overdue
        };
        if overdue.is_empty() {
            return;
        }
        for entry in &overdue {
            let _ = self.journal.event_durable(
                "close_unconfirmed",
                json!({
                    "close_id": entry.close_id,
                    "reason": "close_not_returned",
                    "t_deadline_us": entry.deadline_us,
                    "instance": entry.instance,
                    "backend": entry.backend,
                    "stream": entry.stream,
                }),
            );
            self.terminal.request(
                now,
                StopCause::Unconfirmed(format!(
                    "close {} of {} {} did not return by its deadline",
                    entry.close_id,
                    entry.instance,
                    entry.backend.as_str()
                )),
            );
        }
        self.write_closing_file();
        if self.declared.swap(true, Ordering::AcqRel) {
            return;
        }
        self.finish_unconfirmed();
    }

    /// Ends the run after `close_not_returned` (§3.5.2 item 5).
    fn finish_unconfirmed(&self) {
        let emergency = self.emergency.get().cloned();
        if let Some(emergency) = &emergency {
            let closed = emergency.close_remaining();
            self.journal
                .event("emergency_closes", json!({ "closes": closed }));
            emergency.write_summary(RunResult::Unconfirmed, "close_not_returned");
        }
        self.journal.event(
            "terminal",
            json!({ "result": "unconfirmed", "exit_code": 20, "by": "supervisor" }),
        );
        self.journal.flush();
        self.terminal.mark_finished_by_supervisor();
        if self.limits.self_exit {
            let _ = self.journal.event_durable(
                "self_exit",
                json!({ "code": RunResult::Unconfirmed.exit_code() }),
            );
            self.exit.exit(RunResult::Unconfirmed.exit_code());
        } else {
            let since = self.clock.now_us().max(1);
            self.terminal
                .pending_since_us
                .store(since, Ordering::Release);
            heartbeat_tick(
                &self.journal,
                self,
                &self.terminal,
                json!({ "by": "supervisor" }),
            );
            self.exit.pending(since);
        }
    }
}

/// A background loop on a participant thread: runs `tick` every `period` until `stop`.
pub fn spawn_ticker(
    clock: &Arc<dyn Clock>,
    name: &str,
    period: Micros,
    stop: Arc<AtomicBool>,
    mut tick: impl FnMut() + Send + 'static,
) -> std::io::Result<crate::clock::Participant<()>> {
    let ticker_clock = Arc::clone(clock);
    spawn_participant(clock, name, move || {
        let mut next = ticker_clock.now_us();
        while !stop.load(Ordering::Acquire) {
            tick();
            next = next.saturating_add(period.max(1));
            pause_until(ticker_clock.as_ref(), next);
        }
    })
}

/// The control-file watcher (§3.11.2 B): `STOP` and `ABORT` in the run directory, each recorded
/// once at the moment it is seen.
pub fn control_tick(journal: &Journal, terminal: &Terminal, seen: &mut [bool; 2]) {
    for (index, (name, cause)) in [
        ("STOP", StopCause::StopFile),
        ("ABORT", StopCause::AbortFile),
    ]
    .into_iter()
    .enumerate()
    {
        if !seen[index] && journal.path(name).exists() {
            seen[index] = true;
            journal.event("control_file", json!({ "file": name }));
            terminal.request(journal.now_us(), cause);
        }
    }
}

/// The heartbeat (§3.11.1): time, cell, in-flight, memory, closes in progress, the pending
/// terminal state.
pub fn heartbeat_tick(journal: &Journal, closer: &Closer, terminal: &Terminal, dynamic: Value) {
    let pending = terminal.pending_since();
    let closing = closer.in_progress();
    if !closing.is_empty() {
        // The closes in progress over time (§3.5.2 心跳里列出所有关闭中的项和已过时长).
        journal.append(
            "heartbeat-closing.jsonl",
            &json!({ "t_us": closer.clock.now_us(), "closing": closing }),
        );
    }
    let body = json!({
        "unix_ms": closer.clock.unix_ms(),
        "t_us": closer.clock.now_us(),
        "run": dynamic,
        "closing": closer.in_progress(),
        "terminal_requested": terminal.first(),
        "terminal_pending_since_us": (pending > 0).then_some(pending),
    });
    let _ = journal.write_json("heartbeat.json", &body);
}

/// The deadline a device-side check of a close gets.
pub fn device_check_deadline(limits: &FixedLimits) -> Micros {
    limits.device_check_deadline_us.max(ms(1))
}
