// SPDX-License-Identifier: AGPL-3.0-only

//! §3.3-§3.7: the preview hub. It keeps the Runtime's lock graph `admission → queue_order →
//! scheduler` with same-shape locks, takes every sequence number at the atomic grant point under
//! all three locks, and implements each entry path (P1-P9 of §3.3.2) as a step machine whose
//! steps end at the barrier points of §4.4. Production drives a step machine to its end with
//! blocking acquisitions ([`run_blocking`]); the deterministic interleaving test drives several
//! of them on one thread without blocking and enumerates the orders.

use crate::backend::{BackendFailure, Floor, PreviewBackend, SampledFrame};
use crate::clock::{Clock, Handoff, Micros, ms};
use crate::gates::{reaction, refusing_gate};
use crate::locks::{
    LockCheck, LockOwner, RANK_ADMISSION, RANK_QUEUE_ORDER, RANK_SCHEDULER, RankedLock,
};
use crate::model::{BackendKind, CallOwner, Disposition, Entry, GateKind, GlobalGate, PathKind};
use crate::views::{PreviewFrame, Validity, ViewHub, ViewMessage};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One record of the hub's total order (§3.3.3): grants, registrations and gate signals share
/// one counter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SeqRecord {
    pub seq: u64,
    pub t_us: Micros,
    pub instance: Option<usize>,
    #[serde(flatten)]
    pub kind: RecordKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "rec")]
pub enum RecordKind {
    /// A preview sample was granted (`grant(seq)`).
    Grant {
        path: PathKind,
        generation: u64,
        owner: CallOwner,
    },
    /// A formal demand was registered (`register(seq)`).
    Register,
    /// The registration `registration` was granted to formal work (`formal_grant`).
    FormalGrant {
        registration: u64,
        by_pump: bool,
    },
    /// The registration was withdrawn after the bounded retry (`dispatch_contended`).
    Withdraw {
        registration: u64,
    },
    /// A gate was set or cleared (`gate(seq)`); `instance` is `None` for a global gate.
    GateSet {
        gate: String,
    },
    GateClear {
        gate: String,
    },
    /// The sample granted at `grant` was published.
    Publish {
        grant: u64,
        preview_sequence: u64,
        generation: u64,
    },
    /// The sample granted at `grant` was not published (`discarded_stale`, or a failure).
    Discard {
        grant: u64,
        reason: String,
        generation: u64,
    },
    /// A manual refresh (P5), outside I1 (§3.3.3).
    Manual,
    /// An auxiliary device call (P6) started or ended.
    AuxStart {
        entry: Entry,
    },
    AuxEnd {
        entry: Entry,
    },
    /// The backend generation moved (reopen, rotation, a gate cleared).
    Generation {
        generation: u64,
    },
    /// A lease began or ended; `active` is the instance's lease count after it.
    Lease {
        lease: u64,
        subscriber: u64,
        event: String,
        active: usize,
        renewed_us: Micros,
        ttl_us: Micros,
    },
}

/// Receives the records (a leaf: it is called under the scheduler lock).
pub trait RecordSink: Send + Sync {
    fn record(&self, record: &SeqRecord);
}

/// Keeps every record in memory (tests and the in-run checker's evidence).
#[derive(Default)]
pub struct MemorySink {
    records: Mutex<Vec<SeqRecord>>,
}

impl MemorySink {
    pub fn records(&self) -> Vec<SeqRecord> {
        lock(&self.records).clone()
    }
}

impl RecordSink for MemorySink {
    fn record(&self, record: &SeqRecord) {
        lock(&self.records).push(record.clone());
    }
}

/// The independent check of I1, I2 and I4 over the record stream (I3 is the lock check).
#[derive(Debug, Default, Clone)]
pub struct InvariantChecker {
    pending: BTreeMap<usize, BTreeSet<u64>>,
    gates: BTreeMap<Option<usize>, BTreeSet<String>>,
    outstanding: BTreeMap<u64, (usize, u64, BTreeSet<String>)>,
    generations: BTreeMap<usize, u64>,
    leases: BTreeMap<usize, usize>,
    pub violations: Vec<String>,
    pub inflight_before_register: u64,
    pub grants: u64,
    pub publishes: u64,
    pub discards: u64,
}

impl InvariantChecker {
    fn refusing(&self, instance: usize) -> Option<String> {
        let mut gates = Vec::new();
        if let Some(local) = self.gates.get(&Some(instance)) {
            gates.extend(local.iter().cloned());
        }
        if let Some(global) = self.gates.get(&None) {
            gates.extend(global.iter().cloned());
        }
        gates.into_iter().find(|gate| {
            gate_kind_of(gate)
                .is_none_or(|kind| !reaction(Entry::PreviewSample, kind).admits(false))
        })
    }

    pub fn observe(&mut self, record: &SeqRecord) {
        let seq = record.seq;
        match (&record.kind, record.instance) {
            (RecordKind::Grant { generation, .. }, Some(instance)) => {
                self.grants += 1;
                if let Some(pending) = self.pending.get(&instance)
                    && let Some(registration) = pending.iter().next()
                {
                    self.violations.push(format!(
                        "I1: grant({seq}) on instance {instance} after the pending register({registration})"
                    ));
                }
                if let Some(gate) = self.refusing(instance) {
                    self.violations.push(format!(
                        "I1: grant({seq}) on instance {instance} while gate {gate} is set"
                    ));
                }
                if self
                    .outstanding
                    .values()
                    .any(|(outstanding, _, _)| *outstanding == instance)
                {
                    self.violations.push(format!(
                        "§3.3.4: grant({seq}) on instance {instance} with a sample still in flight"
                    ));
                }
                if self.leases.get(&instance).copied().unwrap_or(0) == 0 {
                    self.violations.push(format!(
                        "§3.3.3: grant({seq}) on instance {instance} without a lease"
                    ));
                }
                if self
                    .generations
                    .get(&instance)
                    .is_some_and(|current| current != generation)
                {
                    self.violations.push(format!(
                        "§3.3.3: grant({seq}) on instance {instance} for an old generation {generation}"
                    ));
                }
                self.outstanding
                    .insert(seq, (instance, *generation, BTreeSet::new()));
            }
            (RecordKind::Register, Some(instance)) => {
                if self
                    .outstanding
                    .values()
                    .any(|(outstanding, _, _)| *outstanding == instance)
                {
                    self.inflight_before_register += 1;
                }
                self.pending.entry(instance).or_default().insert(seq);
            }
            (
                RecordKind::FormalGrant { registration, .. }
                | RecordKind::Withdraw { registration },
                Some(instance),
            ) => {
                let removed = self
                    .pending
                    .get_mut(&instance)
                    .is_some_and(|pending| pending.remove(registration));
                if !removed {
                    self.violations.push(format!(
                        "I1: seq {seq} ends register({registration}) on instance {instance}, which is not pending"
                    ));
                }
            }
            (RecordKind::GateSet { gate }, instance) => {
                self.gates.entry(instance).or_default().insert(gate.clone());
                let refused = gate_kind_of(gate)
                    .is_none_or(|kind| !reaction(Entry::PreviewSample, kind).admits(false));
                if refused {
                    for (outstanding_instance, _, changed) in self.outstanding.values_mut() {
                        if instance.is_none_or(|target| target == *outstanding_instance) {
                            changed.insert(format!("gate:{gate}"));
                        }
                    }
                }
            }
            (RecordKind::GateClear { gate }, instance) => {
                if let Some(gates) = self.gates.get_mut(&instance) {
                    gates.remove(gate);
                }
            }
            (RecordKind::Generation { generation }, Some(instance)) => {
                self.generations.insert(instance, *generation);
                for (outstanding_instance, _, changed) in self.outstanding.values_mut() {
                    if *outstanding_instance == instance {
                        changed.insert("generation".to_owned());
                    }
                }
            }
            (RecordKind::Lease { active, .. }, Some(instance)) => {
                self.leases.insert(instance, *active);
                if *active == 0 {
                    for (outstanding_instance, _, changed) in self.outstanding.values_mut() {
                        if *outstanding_instance == instance {
                            changed.insert("lease".to_owned());
                        }
                    }
                }
            }
            (RecordKind::Publish { grant, .. }, Some(instance)) => {
                self.publishes += 1;
                match self.outstanding.remove(grant) {
                    Some((owner, _, changed)) if owner == instance => {
                        if !changed.is_empty() {
                            self.violations.push(format!(
                                "I4: publish({seq}) of grant({grant}) after {changed:?} changed"
                            ));
                        }
                    }
                    _ => self.violations.push(format!(
                        "I4: publish({seq}) of grant({grant}) that is not in flight on instance {instance}"
                    )),
                }
            }
            (RecordKind::Discard { grant, .. }, Some(_)) => {
                self.discards += 1;
                if self.outstanding.remove(grant).is_none() {
                    self.violations.push(format!(
                        "I4: discard({seq}) of grant({grant}) that is not in flight"
                    ));
                }
            }
            _ => {}
        }
    }
}

fn gate_kind_of(name: &str) -> Option<GateKind> {
    GateKind::ALL
        .into_iter()
        .find(|gate| gate.as_str() == name)
        .or_else(|| (name == "cell_end" || name == "budget").then_some(GateKind::StopClass))
}

/// A subscriber's lease (§3.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lease {
    pub lease_id: u64,
    pub subscriber: u64,
    pub instance: usize,
    pub owner_epoch: u64,
    pub issued_us: Micros,
    pub renewed_us: Micros,
    pub ttl_us: Micros,
}

/// A formal demand waiting for admission (P2).
struct Demand {
    registration: u64,
    owner: Arc<LockOwner>,
    granted: Arc<Handoff<u64>>,
}

/// The S soft-unsubscribe grace state of one instance (§3.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct GraceCounts {
    pub entered: u64,
    pub reused: u64,
    pub expired_closed: u64,
    pub hard_closed: u64,
}

/// The scheduler lock's state of one instance.
pub struct InstanceState {
    pub name: String,
    pub gates: BTreeSet<GateKind>,
    pub generation: u64,
    pub leases: BTreeMap<u64, Lease>,
    demand: Option<Demand>,
    pub inflight: u32,
    pub aux_inflight: u32,
    pub last_grant: Option<u64>,
    pub last_publish_us: Option<Micros>,
    pub preview_sequence: u64,
    /// S grace: until when an S producer without leases may live.
    pub grace_until: Option<Micros>,
    pub grace: GraceCounts,
    pub last_frame_bytes: u64,
}

/// §3.6: the five budget caps.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BudgetConfig {
    pub max_samples_per_second: f64,
    pub max_bytes_per_second: f64,
    pub max_inflight_global: u32,
    pub max_buffer_bytes: u64,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_samples_per_second: 60.0,
            max_bytes_per_second: 400.0 * 1024.0 * 1024.0,
            max_inflight_global: 2,
            max_buffer_bytes: 256 * 1024 * 1024,
        }
    }
}

struct Budget {
    config: BudgetConfig,
    samples: f64,
    bytes: f64,
    refilled_us: Micros,
}

impl Budget {
    fn refill(&mut self, now: Micros) {
        let elapsed = now.saturating_sub(self.refilled_us) as f64 / 1_000_000.0;
        self.samples = (self.samples + elapsed * self.config.max_samples_per_second)
            .min(self.config.max_samples_per_second.max(1.0));
        self.bytes = (self.bytes + elapsed * self.config.max_bytes_per_second)
            .min(self.config.max_bytes_per_second.max(1.0));
        self.refilled_us = now;
    }
}

/// Everything the scheduler lock protects (§3.3.1 rank 3).
pub struct SchedulerState {
    pub seq: u64,
    pub owner_epoch: u64,
    pub global: BTreeSet<GlobalGate>,
    pub instances: Vec<InstanceState>,
    pub checker: InvariantChecker,
    budget: Budget,
    pub inflight_global: u32,
    pub buffer_bytes: u64,
    next_lease: u64,
}

impl SchedulerState {
    /// The gates in force on `instance`, the global gates as the stop class.
    pub fn active_gates(&self, instance: usize) -> Vec<GateKind> {
        let mut gates: Vec<GateKind> = self.instances[instance].gates.iter().copied().collect();
        if !self.global.is_empty() && !gates.contains(&GateKind::StopClass) {
            gates.push(GateKind::StopClass);
        }
        gates
    }
}

/// The instance's open backends (taken only by the admission lock's holder, or by the close
/// path when the instance is drained).
#[derive(Default)]
pub struct BackendSlots {
    pub nemu: Option<Box<dyn PreviewBackend>>,
    pub scrcpy: Option<Box<dyn PreviewBackend>>,
    pub material: Option<Box<dyn crate::backend::MaterialWriter>>,
}

impl BackendSlots {
    pub fn get(&mut self, kind: BackendKind) -> Option<&mut Box<dyn PreviewBackend>> {
        match kind {
            BackendKind::Nemu => self.nemu.as_mut(),
            BackendKind::Scrcpy => self.scrcpy.as_mut(),
        }
    }

    pub fn take(&mut self, kind: BackendKind) -> Option<Box<dyn PreviewBackend>> {
        match kind {
            BackendKind::Nemu => self.nemu.take(),
            BackendKind::Scrcpy => self.scrcpy.take(),
        }
    }

    pub fn put(&mut self, backend: Box<dyn PreviewBackend>) {
        match backend.kind() {
            BackendKind::Nemu => self.nemu = Some(backend),
            BackendKind::Scrcpy => self.scrcpy = Some(backend),
        }
    }
}

/// The hub.
pub struct Hub {
    pub clock: Arc<dyn Clock>,
    pub check: Arc<LockCheck>,
    pub admission: Vec<RankedLock>,
    pub queue_order: Vec<RankedLock>,
    pub scheduler_lock: RankedLock,
    scheduler: Mutex<SchedulerState>,
    pub backends: Vec<Mutex<BackendSlots>>,
    pub views: Arc<ViewHub>,
    sink: Arc<dyn RecordSink>,
}

impl Hub {
    pub fn new(
        clock: Arc<dyn Clock>,
        instances: &[String],
        owner_epoch: u64,
        budget: BudgetConfig,
        sink: Arc<dyn RecordSink>,
    ) -> Self {
        let check = LockCheck::new();
        let admission = instances
            .iter()
            .map(|name| {
                RankedLock::new(
                    RANK_ADMISSION,
                    format!("admission[{name}]"),
                    Arc::clone(&check),
                    Arc::clone(&clock),
                )
            })
            .collect();
        let queue_order = instances
            .iter()
            .map(|name| {
                RankedLock::new(
                    RANK_QUEUE_ORDER,
                    format!("queue_order[{name}]"),
                    Arc::clone(&check),
                    Arc::clone(&clock),
                )
            })
            .collect();
        let scheduler_lock = RankedLock::new(
            RANK_SCHEDULER,
            "scheduler",
            Arc::clone(&check),
            Arc::clone(&clock),
        );
        let now = clock.now_us();
        let state = SchedulerState {
            seq: 0,
            owner_epoch,
            global: BTreeSet::new(),
            instances: instances
                .iter()
                .map(|name| InstanceState {
                    name: name.clone(),
                    gates: BTreeSet::new(),
                    generation: 1,
                    leases: BTreeMap::new(),
                    demand: None,
                    inflight: 0,
                    aux_inflight: 0,
                    last_grant: None,
                    last_publish_us: None,
                    preview_sequence: 0,
                    grace_until: None,
                    grace: GraceCounts::default(),
                    last_frame_bytes: 0,
                })
                .collect(),
            checker: InvariantChecker::default(),
            budget: Budget {
                config: budget,
                samples: budget.max_samples_per_second.max(1.0),
                bytes: budget.max_bytes_per_second.max(1.0),
                refilled_us: now,
            },
            inflight_global: 0,
            buffer_bytes: 0,
            next_lease: 1,
        };
        Self {
            backends: instances
                .iter()
                .map(|_| Mutex::new(BackendSlots::default()))
                .collect(),
            clock,
            check,
            admission,
            queue_order,
            scheduler_lock,
            scheduler: Mutex::new(state),
            views: Arc::new(ViewHub::new()),
            sink,
        }
    }

    pub fn instance_count(&self) -> usize {
        self.admission.len()
    }

    /// The scheduler state; only its holder may read it.
    pub fn state(&self, who: &LockOwner) -> MutexGuard<'_, SchedulerState> {
        if !self.scheduler_lock.held_by(who) {
            self.check.record(format!(
                "lock_order_violation: {} reads the scheduler state without the scheduler lock",
                who.name()
            ));
        }
        lock(&self.scheduler)
    }

    /// A read of the scheduler state for reports outside any entry path (taken as P9 does:
    /// the scheduler lock alone).
    pub fn snapshot<T>(&self, who: &LockOwner, read: impl FnOnce(&SchedulerState) -> T) -> T {
        self.scheduler_lock.acquire(who);
        let value = read(&self.state(who));
        self.scheduler_lock.release(who);
        value
    }

    /// Appends one record under the scheduler lock: the next sequence number, the invariant
    /// check, the sink.
    fn append(&self, state: &mut SchedulerState, instance: Option<usize>, kind: RecordKind) -> u64 {
        state.seq += 1;
        let record = SeqRecord {
            seq: state.seq,
            t_us: self.clock.now_us(),
            instance,
            kind,
        };
        state.checker.observe(&record);
        self.sink.record(&record);
        state.seq
    }

    /// The I1-I4 and lock-order violations so far.
    pub fn violations(&self, who: &LockOwner) -> Vec<String> {
        let mut violations = self.check.violations();
        violations.extend(self.snapshot(who, |state| state.checker.violations.clone()));
        violations
    }

    // ---- P8: gate signals (scheduler lock only) ----

    /// Sets or clears an instance gate; returns its sequence number.
    pub fn set_gate(&self, who: &LockOwner, instance: usize, gate: GateKind, set: bool) -> u64 {
        let mut op = GateOp::new(Arc::new(GateTarget::Instance(instance, gate)), set, who);
        run_blocking(self, &mut op)
    }

    /// Sets or clears a global gate (the whole-run stop or the cell end).
    pub fn set_global(&self, who: &LockOwner, gate: GlobalGate, set: bool) -> u64 {
        let mut op = GateOp::new(Arc::new(GateTarget::Global(gate)), set, who);
        run_blocking(self, &mut op)
    }

    /// Moves the instance's generation (§3.8 代际推进) and returns the new one.
    pub fn bump_generation(&self, who: &LockOwner, instance: usize) -> u64 {
        self.scheduler_lock.acquire(who);
        let generation = {
            let mut state = self.state(who);
            state.instances[instance].generation += 1;
            let generation = state.instances[instance].generation;
            self.append(
                &mut state,
                Some(instance),
                RecordKind::Generation { generation },
            );
            generation
        };
        self.scheduler_lock.release(who);
        generation
    }

    // ---- P9: leases (scheduler lock only) ----

    pub fn subscribe(
        &self,
        who: &LockOwner,
        instance: usize,
        subscriber: u64,
        ttl_us: Micros,
    ) -> Lease {
        self.scheduler_lock.acquire(who);
        let lease = {
            let mut state = self.state(who);
            let now = self.clock.now_us();
            let lease = Lease {
                lease_id: state.next_lease,
                subscriber,
                instance,
                owner_epoch: state.owner_epoch,
                issued_us: now,
                renewed_us: now,
                ttl_us,
            };
            state.next_lease += 1;
            let entry = &mut state.instances[instance];
            if entry.leases.is_empty() && entry.grace_until.take().is_some() {
                entry.grace.reused += 1;
            }
            entry.leases.insert(lease.lease_id, lease.clone());
            let active = entry.leases.len();
            self.append(
                &mut state,
                Some(instance),
                RecordKind::Lease {
                    lease: lease.lease_id,
                    subscriber,
                    event: "subscribe".to_owned(),
                    active,
                    renewed_us: now,
                    ttl_us,
                },
            );
            lease
        };
        self.views.open_mailbox(subscriber, instance);
        self.scheduler_lock.release(who);
        lease
    }

    /// Renews a lease; false when it is gone (expired or ended).
    pub fn renew(&self, who: &LockOwner, instance: usize, lease_id: u64) -> bool {
        self.scheduler_lock.acquire(who);
        let renewed = {
            let mut state = self.state(who);
            let now = self.clock.now_us();
            match state.instances[instance].leases.get_mut(&lease_id) {
                Some(lease) => {
                    lease.renewed_us = now;
                    true
                }
                None => false,
            }
        };
        self.scheduler_lock.release(who);
        renewed
    }

    /// Ends a lease (an active unsubscribe, §3.4); a soft event.
    pub fn unsubscribe(&self, who: &LockOwner, instance: usize, lease_id: u64, grace_us: Micros) {
        self.scheduler_lock.acquire(who);
        let subscriber = {
            let mut state = self.state(who);
            let removed = state.instances[instance].leases.remove(&lease_id);
            if let Some(lease) = &removed {
                self.end_lease(&mut state, instance, lease, "unsubscribe", grace_us);
            }
            removed.map(|lease| lease.subscriber)
        };
        if let Some(subscriber) = subscriber {
            self.views.close_mailbox(subscriber);
        }
        self.scheduler_lock.release(who);
    }

    fn end_lease(
        &self,
        state: &mut SchedulerState,
        instance: usize,
        lease: &Lease,
        event: &str,
        grace_us: Micros,
    ) {
        let now = self.clock.now_us();
        let active = state.instances[instance].leases.len();
        if active == 0 {
            let entry = &mut state.instances[instance];
            entry.grace_until = Some(now.saturating_add(grace_us.min(ms(10_000))));
            entry.grace.entered += 1;
        }
        self.append(
            state,
            Some(instance),
            RecordKind::Lease {
                lease: lease.lease_id,
                subscriber: lease.subscriber,
                event: event.to_owned(),
                active,
                renewed_us: lease.renewed_us,
                ttl_us: lease.ttl_us,
            },
        );
    }

    /// The TTL sweep (P9, every check period): ends the leases not renewed within their TTL.
    pub fn sweep(&self, who: &LockOwner, grace_us: Micros) -> Vec<Lease> {
        self.scheduler_lock.acquire(who);
        let expired = {
            let mut state = self.state(who);
            let now = self.clock.now_us();
            let mut expired = Vec::new();
            for instance in 0..state.instances.len() {
                let ids: Vec<u64> = state.instances[instance]
                    .leases
                    .values()
                    .filter(|lease| now >= lease.renewed_us.saturating_add(lease.ttl_us))
                    .map(|lease| lease.lease_id)
                    .collect();
                for id in ids {
                    if let Some(lease) = state.instances[instance].leases.remove(&id) {
                        self.end_lease(&mut state, instance, &lease, "ttl_expired", grace_us);
                        expired.push(lease);
                    }
                }
            }
            expired
        };
        for lease in &expired {
            self.views.close_mailbox(lease.subscriber);
        }
        self.scheduler_lock.release(who);
        expired
    }

    /// Whether `instance` has a lease now.
    pub fn has_lease(&self, who: &LockOwner, instance: usize) -> bool {
        self.snapshot(who, |state| !state.instances[instance].leases.is_empty())
    }

    /// The S grace check: `Some(true)` when the grace ran out (the producer must close),
    /// `Some(false)` while it runs, `None` without one.
    pub fn grace_expired(&self, who: &LockOwner, instance: usize) -> Option<bool> {
        self.scheduler_lock.acquire(who);
        let expired = {
            let mut state = self.state(who);
            let now = self.clock.now_us();
            let entry = &mut state.instances[instance];
            match entry.grace_until {
                Some(until) if entry.leases.is_empty() && now >= until => {
                    entry.grace_until = None;
                    entry.grace.expired_closed += 1;
                    Some(true)
                }
                Some(_) if entry.leases.is_empty() => Some(false),
                Some(_) => {
                    entry.grace_until = None;
                    None
                }
                None => None,
            }
        };
        self.scheduler_lock.release(who);
        expired
    }

    /// A hard gate arrived during an S grace: the grace ends at once (§3.4 硬门不走宽限).
    pub fn end_grace_hard(&self, who: &LockOwner, instance: usize) -> bool {
        self.scheduler_lock.acquire(who);
        let ended = {
            let mut state = self.state(who);
            let entry = &mut state.instances[instance];
            if entry.grace_until.take().is_some() {
                entry.grace.hard_closed += 1;
                true
            } else {
                false
            }
        };
        self.scheduler_lock.release(who);
        ended
    }

    /// The in-flight count of `instance` (preview, manual and auxiliary calls).
    pub fn inflight(&self, who: &LockOwner, instance: usize) -> u32 {
        self.snapshot(who, |state| {
            state.instances[instance].inflight + state.instances[instance].aux_inflight
        })
    }
}

/// How a step machine takes a blocking lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Production: wait for the lock.
    Blocking,
    /// The interleaving test: a busy lock leaves the step undone (`Blocked`).
    NonBlocking,
}

/// One step's result.
#[derive(Debug)]
pub enum Step<T> {
    Progress,
    Blocked,
    Done(T),
}

/// An entry path as a step machine.
pub trait Op {
    type Output;
    /// The barrier the next step starts at (§4.4 g0-g5, r0-r2, s0-s1, d0-d1).
    fn barrier(&self) -> &'static str;
    fn step(&mut self, hub: &Hub, mode: Mode) -> Step<Self::Output>;
}

fn take(lock: &RankedLock, who: &LockOwner, mode: Mode) -> bool {
    match mode {
        Mode::Blocking => {
            lock.acquire(who);
            true
        }
        Mode::NonBlocking => {
            if lock.is_free() {
                lock.acquire(who);
                true
            } else {
                false
            }
        }
    }
}

/// Runs an op to its end, waiting for every lock.
pub fn run_blocking<O: Op>(hub: &Hub, op: &mut O) -> O::Output {
    loop {
        match op.step(hub, Mode::Blocking) {
            Step::Done(output) => return output,
            Step::Progress => {}
            Step::Blocked => unreachable!("a blocking step never reports Blocked"),
        }
    }
}

// ---- P8 ----

pub enum GateTarget {
    Instance(usize, GateKind),
    Global(GlobalGate),
}

/// P8: a gate signal takes only the scheduler lock (§3.3.2), so a stuck native call never holds
/// it up.
pub struct GateOp {
    target: Arc<GateTarget>,
    set: bool,
    who: Arc<LockOwner>,
    phase: u8,
    seq: u64,
}

impl GateOp {
    pub fn new(target: Arc<GateTarget>, set: bool, who: &LockOwner) -> Self {
        Self {
            target,
            set,
            who: LockOwner::new(format!("gate-signal/{}", who.name())),
            phase: 0,
            seq: 0,
        }
    }

    /// The op runs under its own owner so a caller holding no lock can drive it.
    pub fn with_owner(target: Arc<GateTarget>, set: bool, who: Arc<LockOwner>) -> Self {
        Self {
            target,
            set,
            who,
            phase: 0,
            seq: 0,
        }
    }
}

impl Op for GateOp {
    type Output = u64;

    fn barrier(&self) -> &'static str {
        match self.phase {
            0 => "s0",
            _ => "s1",
        }
    }

    fn step(&mut self, hub: &Hub, mode: Mode) -> Step<u64> {
        match self.phase {
            0 => {
                if !take(&hub.scheduler_lock, &self.who, mode) {
                    return Step::Blocked;
                }
                let mut state = hub.state(&self.who);
                let (instance, name, withdraw) = match &*self.target {
                    GateTarget::Instance(instance, gate) => {
                        let changed = if self.set {
                            state.instances[*instance].gates.insert(*gate)
                        } else {
                            state.instances[*instance].gates.remove(gate)
                        };
                        let _ = changed;
                        (
                            Some(*instance),
                            gate.as_str().to_owned(),
                            self.set && reaction(Entry::PreviewSample, *gate).withdraws(),
                        )
                    }
                    GateTarget::Global(gate) => {
                        if self.set {
                            state.global.insert(*gate);
                        } else {
                            state.global.remove(gate);
                        }
                        let name = match gate {
                            GlobalGate::RunStop => "stop",
                            GlobalGate::CellEnd => "cell_end",
                        };
                        (None, name.to_owned(), false)
                    }
                };
                let kind = if self.set {
                    RecordKind::GateSet { gate: name.clone() }
                } else {
                    RecordKind::GateClear { gate: name.clone() }
                };
                self.seq = hub.append(&mut state, instance, kind);
                drop(state);
                if withdraw && let Some(instance) = instance {
                    hub.views.broadcast(
                        Some(instance),
                        ViewMessage::Withdrawn {
                            instance,
                            gate: name,
                        },
                    );
                }
                self.phase = 1;
                Step::Progress
            }
            _ => {
                hub.scheduler_lock.release(&self.who);
                Step::Done(self.seq)
            }
        }
    }
}

// ---- P1 ----

/// What a preview sample ended with.
#[derive(Debug, Clone)]
pub struct SampleOutcome {
    pub disposition: Disposition,
    pub grant_seq: Option<u64>,
    pub grant_us: Option<Micros>,
    pub call_start_us: Option<Micros>,
    pub call_end_us: Option<Micros>,
    pub published_us: Option<Micros>,
    pub preview_sequence: Option<u64>,
    pub generation: Option<u64>,
    pub frame: Option<SampledFrame>,
    pub failure: Option<BackendFailure>,
    /// The formal demand the pump granted when this sample released admission.
    pub pumped: Option<u64>,
    pub material: Option<crate::backend::MaterialCost>,
}

impl SampleOutcome {
    fn skipped(disposition: Disposition) -> Self {
        Self {
            disposition,
            grant_seq: None,
            grant_us: None,
            call_start_us: None,
            call_end_us: None,
            published_us: None,
            preview_sequence: None,
            generation: None,
            frame: None,
            failure: None,
            pumped: None,
            material: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SamplePhase {
    TryAdmission,
    QueueOrder,
    Grant,
    ReleaseAfterGrant,
    CallStart,
    Call,
    Publish,
    PumpQueueOrder,
    PumpScheduler,
    Done,
}

/// P1: one preview sample (§3.3.2), with the atomic grant point under all three locks.
pub struct SampleOp {
    instance: usize,
    path: PathKind,
    who: Arc<LockOwner>,
    phase: SamplePhase,
    outcome: SampleOutcome,
    result: Option<Result<SampledFrame, BackendFailure>>,
    pump_target: Option<(Arc<LockOwner>, Arc<Handoff<u64>>, u64)>,
    expected_bytes: u64,
}

impl SampleOp {
    pub fn new(instance: usize, path: PathKind, who: Arc<LockOwner>) -> Self {
        Self {
            instance,
            path,
            who,
            phase: SamplePhase::TryAdmission,
            outcome: SampleOutcome::skipped(Disposition::SkippedBusy),
            result: None,
            pump_target: None,
            expected_bytes: 0,
        }
    }

    fn finish_skip(&mut self, hub: &Hub, disposition: Disposition) -> Step<SampleOutcome> {
        hub.scheduler_lock.release(&self.who);
        hub.queue_order[self.instance].release(&self.who);
        hub.admission[self.instance].release(&self.who);
        self.phase = SamplePhase::Done;
        self.outcome.disposition = disposition;
        Step::Done(self.outcome.clone())
    }
}

impl Op for SampleOp {
    type Output = SampleOutcome;

    fn barrier(&self) -> &'static str {
        match self.phase {
            SamplePhase::TryAdmission => "g0",
            SamplePhase::QueueOrder => "g1",
            SamplePhase::Grant => "g2",
            SamplePhase::ReleaseAfterGrant => "g3",
            SamplePhase::CallStart | SamplePhase::Call => "g4",
            SamplePhase::Publish => "g5",
            SamplePhase::PumpQueueOrder | SamplePhase::PumpScheduler | SamplePhase::Done => "pump",
        }
    }

    fn step(&mut self, hub: &Hub, mode: Mode) -> Step<SampleOutcome> {
        let instance = self.instance;
        match self.phase {
            SamplePhase::TryAdmission => {
                // Never waits for admission: a busy instance is `skipped_busy`, no other lock.
                if !hub.admission[instance].try_acquire(&self.who) {
                    self.phase = SamplePhase::Done;
                    return Step::Done(SampleOutcome::skipped(Disposition::SkippedBusy));
                }
                self.phase = SamplePhase::QueueOrder;
                Step::Progress
            }
            SamplePhase::QueueOrder => {
                if !take(&hub.queue_order[instance], &self.who, mode) {
                    return Step::Blocked;
                }
                self.phase = SamplePhase::Grant;
                Step::Progress
            }
            SamplePhase::Grant => {
                if !take(&hub.scheduler_lock, &self.who, mode) {
                    return Step::Blocked;
                }
                let now = hub.clock.now_us();
                let mut state = hub.state(&self.who);
                let gates = state.active_gates(instance);
                let refused = refusing_gate(Entry::PreviewSample, gates, false);
                let entry = &state.instances[instance];
                let skip = if let Some(gate) = refused {
                    Some(Disposition::SkippedGate(gate.as_str().to_owned()))
                } else if entry.leases.is_empty() {
                    Some(Disposition::SkippedNoLease)
                } else if entry.demand.is_some() {
                    Some(Disposition::SkippedDemand)
                } else if entry.inflight > 0 {
                    Some(Disposition::SkippedBusy)
                } else {
                    None
                };
                let skip = skip.or_else(|| {
                    state.budget.refill(now);
                    self.expected_bytes = state.instances[instance].last_frame_bytes;
                    let budget = &state.budget;
                    (budget.samples < 1.0
                        || budget.bytes < self.expected_bytes as f64
                        || state.inflight_global >= budget.config.max_inflight_global
                        || state.buffer_bytes.saturating_add(self.expected_bytes)
                            > budget.config.max_buffer_bytes)
                        .then_some(Disposition::SkippedBudget)
                });
                if let Some(disposition) = skip {
                    drop(state);
                    return self.finish_skip(hub, disposition);
                }
                state.budget.samples -= 1.0;
                state.budget.bytes -= self.expected_bytes as f64;
                state.inflight_global += 1;
                state.buffer_bytes = state.buffer_bytes.saturating_add(self.expected_bytes);
                let generation = state.instances[instance].generation;
                state.instances[instance].inflight += 1;
                let seq = hub.append(
                    &mut state,
                    Some(instance),
                    RecordKind::Grant {
                        path: self.path,
                        generation,
                        owner: CallOwner::Preview,
                    },
                );
                state.instances[instance].last_grant = Some(seq);
                self.outcome.grant_seq = Some(seq);
                self.outcome.grant_us = Some(now);
                self.outcome.generation = Some(generation);
                self.phase = SamplePhase::ReleaseAfterGrant;
                Step::Progress
            }
            SamplePhase::ReleaseAfterGrant => {
                hub.scheduler_lock.release(&self.who);
                hub.queue_order[instance].release(&self.who);
                self.phase = SamplePhase::CallStart;
                Step::Progress
            }
            SamplePhase::CallStart => {
                self.outcome.call_start_us = Some(hub.clock.now_us());
                self.phase = SamplePhase::Call;
                Step::Progress
            }
            SamplePhase::Call => {
                // The native call runs holding admission only (§3.3.1): never the queue order
                // or the scheduler lock.
                if self.who.holds_rank(RANK_QUEUE_ORDER) || self.who.holds_rank(RANK_SCHEDULER) {
                    hub.check.record(format!(
                        "lock_order_violation: {} calls the device holding queue order or scheduler",
                        self.who.name()
                    ));
                }
                let mut slots = lock(&hub.backends[instance]);
                let result = match slots.get(self.path.backend()) {
                    Some(backend) => backend.sample(None),
                    None => Err(BackendFailure::new(
                        "backend_closed",
                        crate::backend::FailureClass::CellPath,
                        "the path's backend is not open",
                    )
                    .without_native_call()),
                };
                let result = match (self.path, result) {
                    (PathKind::BModel, Ok(frame)) => match slots.material.as_mut() {
                        Some(writer) => match writer.write(&frame) {
                            Ok(cost) => {
                                self.outcome.material = Some(cost);
                                Ok(frame)
                            }
                            Err(failure) => Err(failure),
                        },
                        None => Err(BackendFailure::new(
                            "material_writer_closed",
                            crate::backend::FailureClass::CellPath,
                            "the B_model writer is not open",
                        )),
                    },
                    (_, result) => result,
                };
                drop(slots);
                self.outcome.call_end_us = Some(hub.clock.now_us());
                self.result = Some(result);
                self.phase = SamplePhase::Publish;
                Step::Progress
            }
            SamplePhase::Publish => {
                if !take(&hub.scheduler_lock, &self.who, mode) {
                    return Step::Blocked;
                }
                let now = hub.clock.now_us();
                let mut state = hub.state(&self.who);
                let grant = self.outcome.grant_seq.unwrap_or(0);
                let generation = self.outcome.generation.unwrap_or(0);
                let gates = state.active_gates(instance);
                let refused = refusing_gate(Entry::PreviewSample, gates, false);
                let entry = &state.instances[instance];
                let stale = if let Some(gate) = refused {
                    Some(format!("gate:{}", gate.as_str()))
                } else if entry.generation != generation {
                    Some("generation".to_owned())
                } else if entry.leases.is_empty() {
                    Some("lease".to_owned())
                } else {
                    None
                };
                match self.result.take() {
                    Some(Ok(mut frame)) => {
                        let bytes = frame.pixels.len() as u64;
                        state.instances[instance].last_frame_bytes = bytes;
                        if let Some(reason) = stale {
                            hub.append(
                                &mut state,
                                Some(instance),
                                RecordKind::Discard {
                                    grant,
                                    reason: reason.clone(),
                                    generation,
                                },
                            );
                            self.outcome.disposition = Disposition::DiscardedStale(reason);
                        } else {
                            state.instances[instance].preview_sequence += 1;
                            let preview_sequence = state.instances[instance].preview_sequence;
                            hub.append(
                                &mut state,
                                Some(instance),
                                RecordKind::Publish {
                                    grant,
                                    preview_sequence,
                                    generation,
                                },
                            );
                            state.instances[instance].last_publish_us = Some(now);
                            hub.views.publish(PreviewFrame {
                                instance,
                                path: self.path,
                                owner_epoch: state.owner_epoch,
                                generation,
                                preview_sequence,
                                grant_sequence: grant,
                                width: frame.width,
                                height: frame.height,
                                layout: frame.layout,
                                transform: "identity",
                                sample_start_us: self.outcome.call_start_us.unwrap_or(now),
                                sample_end_us: self.outcome.call_end_us.unwrap_or(now),
                                published_us: now,
                                stream_sequence: frame.stream_sequence,
                                device_pts_us: frame.device_pts_us,
                                received_us: frame.received_us,
                                validity: Validity::Live,
                                pixels: Arc::new(std::mem::take(&mut frame.pixels)),
                            });
                            self.outcome.preview_sequence = Some(preview_sequence);
                            self.outcome.published_us = Some(now);
                            self.outcome.disposition = Disposition::Delivered;
                        }
                        self.outcome.frame = Some(frame);
                    }
                    Some(Err(failure)) => {
                        hub.append(
                            &mut state,
                            Some(instance),
                            RecordKind::Discard {
                                grant,
                                reason: format!("failed:{}", failure.code),
                                generation,
                            },
                        );
                        self.outcome.disposition = Disposition::Failed(failure.code.clone());
                        self.outcome.failure = Some(failure);
                    }
                    None => {}
                }
                state.instances[instance].inflight =
                    state.instances[instance].inflight.saturating_sub(1);
                state.inflight_global = state.inflight_global.saturating_sub(1);
                state.buffer_bytes = state.buffer_bytes.saturating_sub(self.expected_bytes);
                drop(state);
                hub.scheduler_lock.release(&self.who);
                self.phase = SamplePhase::PumpQueueOrder;
                Step::Progress
            }
            SamplePhase::PumpQueueOrder => {
                // P3 pump: holding admission, queue order then scheduler (1 → 2 → 3).
                if !take(&hub.queue_order[instance], &self.who, mode) {
                    return Step::Blocked;
                }
                self.phase = SamplePhase::PumpScheduler;
                Step::Progress
            }
            SamplePhase::PumpScheduler => {
                if !take(&hub.scheduler_lock, &self.who, mode) {
                    return Step::Blocked;
                }
                let mut state = hub.state(&self.who);
                let demand = state.instances[instance].demand.take();
                if let Some(demand) = demand {
                    let seq = hub.append(
                        &mut state,
                        Some(instance),
                        RecordKind::FormalGrant {
                            registration: demand.registration,
                            by_pump: true,
                        },
                    );
                    self.pump_target = Some((demand.owner, demand.granted, seq));
                }
                drop(state);
                hub.scheduler_lock.release(&self.who);
                hub.queue_order[instance].release(&self.who);
                match self.pump_target.take() {
                    Some((owner, granted, seq)) => {
                        hub.admission[instance].transfer(&self.who, &owner);
                        self.outcome.pumped = Some(seq);
                        granted.put(seq);
                    }
                    None => hub.admission[instance].release(&self.who),
                }
                self.phase = SamplePhase::Done;
                Step::Done(self.outcome.clone())
            }
            SamplePhase::Done => Step::Done(self.outcome.clone()),
        }
    }
}

// ---- P2 / P3′ / P3 / P4 ----

/// What the formal work of a granted dispatch did (P4).
pub type FormalWork = Box<dyn FnMut(&Hub, usize) -> FormalResult + Send>;

#[derive(Debug, Clone, Default)]
pub struct FormalResult {
    pub capture_us: Option<Micros>,
    pub tap_us: Option<Micros>,
    pub floor_capture_us: Option<Micros>,
    pub failure: Option<BackendFailure>,
    pub floor_frame: Option<SampledFrame>,
    pub tap_done_us: Option<Micros>,
}

/// What one dispatch ended with.
#[derive(Debug, Clone)]
pub enum DispatchOutcome {
    /// Granted after `wait_us`; `by_pump` when a releasing sample handed admission over.
    Granted {
        registration: u64,
        formal_grant: u64,
        registered_us: Micros,
        granted_us: Micros,
        by_pump: bool,
        work: Box<FormalResult>,
    },
    /// `dispatch_contended`: admission stayed busy for the whole bounded retry.
    Contended { registration: u64 },
    /// The formal loop's entry is refused by a gate (E7).
    Refused { gate: GateKind },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchPhase {
    Gate,
    RegisterQueueOrder,
    Register,
    ReleaseAfterRegister,
    Retry,
    GrantQueueOrder,
    Grant,
    WithdrawQueueOrder,
    Withdraw,
    Work,
    Release,
    Done,
}

/// P2 (registration, shaped as `queue_lease`), P3′ (the bounded retry of policy dispatch), P3
/// (the grant) and P4 (the formal step holding admission).
pub struct DispatchOp {
    instance: usize,
    who: Arc<LockOwner>,
    phase: DispatchPhase,
    granted: Arc<Handoff<u64>>,
    registration: u64,
    formal_grant: u64,
    registered_us: Micros,
    granted_us: Micros,
    by_pump: bool,
    attempts: u32,
    /// The retry bound: 100 ms in time (blocking), or a count of attempts (non-blocking).
    retry_window_us: Micros,
    retry_period_us: Micros,
    max_attempts: u32,
    work: Option<FormalWork>,
    result: FormalResult,
    register_only: bool,
}

impl DispatchOp {
    pub fn new(
        instance: usize,
        who: Arc<LockOwner>,
        clock: Arc<dyn Clock>,
        work: Option<FormalWork>,
    ) -> Self {
        Self {
            instance,
            who,
            phase: DispatchPhase::Gate,
            granted: Arc::new(Handoff::new(clock)),
            registration: 0,
            formal_grant: 0,
            registered_us: 0,
            granted_us: 0,
            by_pump: false,
            attempts: 0,
            retry_window_us: ms(100),
            retry_period_us: ms(5),
            max_attempts: 2,
            work,
            result: FormalResult::default(),
            register_only: false,
        }
    }

    /// The interleaving test's bound: `attempts` failed tries before the withdrawal.
    pub fn with_max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = attempts;
        self
    }

    /// P2 alone: register and stop (the demand stays pending).
    pub fn register_only(mut self) -> Self {
        self.register_only = true;
        self
    }
}

impl Op for DispatchOp {
    type Output = DispatchOutcome;

    fn barrier(&self) -> &'static str {
        match self.phase {
            DispatchPhase::Gate | DispatchPhase::RegisterQueueOrder => "r0",
            DispatchPhase::Register => "r1",
            DispatchPhase::ReleaseAfterRegister => "r2",
            DispatchPhase::Retry => "d0",
            DispatchPhase::GrantQueueOrder | DispatchPhase::Grant => "d1",
            DispatchPhase::WithdrawQueueOrder | DispatchPhase::Withdraw => "d1",
            DispatchPhase::Work | DispatchPhase::Release | DispatchPhase::Done => "p4",
        }
    }

    fn step(&mut self, hub: &Hub, mode: Mode) -> Step<DispatchOutcome> {
        let instance = self.instance;
        match self.phase {
            DispatchPhase::Gate => {
                // E7 refuses new work under its gates (§3.7.1); the check is part of P2's
                // scheduler section below, so this phase only moves on.
                self.phase = DispatchPhase::RegisterQueueOrder;
                Step::Progress
            }
            DispatchPhase::RegisterQueueOrder => {
                if !take(&hub.queue_order[instance], &self.who, mode) {
                    return Step::Blocked;
                }
                self.phase = DispatchPhase::Register;
                Step::Progress
            }
            DispatchPhase::Register => {
                if !take(&hub.scheduler_lock, &self.who, mode) {
                    return Step::Blocked;
                }
                let mut state = hub.state(&self.who);
                let gates = state.active_gates(instance);
                if let Some(gate) = refusing_gate(Entry::FormalLoop, gates, false) {
                    drop(state);
                    hub.scheduler_lock.release(&self.who);
                    hub.queue_order[instance].release(&self.who);
                    self.phase = DispatchPhase::Done;
                    return Step::Done(DispatchOutcome::Refused { gate });
                }
                let seq = hub.append(&mut state, Some(instance), RecordKind::Register);
                state.instances[instance].demand = Some(Demand {
                    registration: seq,
                    owner: Arc::clone(&self.who),
                    granted: Arc::clone(&self.granted),
                });
                self.registration = seq;
                self.registered_us = hub.clock.now_us();
                drop(state);
                self.phase = DispatchPhase::ReleaseAfterRegister;
                Step::Progress
            }
            DispatchPhase::ReleaseAfterRegister => {
                hub.scheduler_lock.release(&self.who);
                hub.queue_order[instance].release(&self.who);
                if self.register_only {
                    self.phase = DispatchPhase::Done;
                    return Step::Done(DispatchOutcome::Contended {
                        registration: self.registration,
                    });
                }
                self.phase = DispatchPhase::Retry;
                Step::Progress
            }
            DispatchPhase::Retry => {
                if let Some(seq) = self.granted.try_take() {
                    self.formal_grant = seq;
                    self.by_pump = true;
                    self.granted_us = hub.clock.now_us();
                    self.phase = DispatchPhase::Work;
                    return Step::Progress;
                }
                if hub.admission[instance].try_acquire(&self.who) {
                    self.phase = DispatchPhase::GrantQueueOrder;
                    return Step::Progress;
                }
                self.attempts += 1;
                match mode {
                    Mode::NonBlocking => {
                        if self.attempts >= self.max_attempts {
                            self.phase = DispatchPhase::WithdrawQueueOrder;
                        }
                        Step::Progress
                    }
                    Mode::Blocking => {
                        let deadline = self.registered_us.saturating_add(self.retry_window_us);
                        let now = hub.clock.now_us();
                        if now >= deadline {
                            self.phase = DispatchPhase::WithdrawQueueOrder;
                            return Step::Progress;
                        }
                        let wake = now.saturating_add(self.retry_period_us).min(deadline);
                        if let Some(seq) = self.granted.take_until(wake) {
                            self.formal_grant = seq;
                            self.by_pump = true;
                            self.granted_us = hub.clock.now_us();
                            self.phase = DispatchPhase::Work;
                        }
                        Step::Progress
                    }
                }
            }
            DispatchPhase::GrantQueueOrder => {
                if !take(&hub.queue_order[instance], &self.who, mode) {
                    return Step::Blocked;
                }
                self.phase = DispatchPhase::Grant;
                Step::Progress
            }
            DispatchPhase::Grant => {
                if !take(&hub.scheduler_lock, &self.who, mode) {
                    return Step::Blocked;
                }
                let mut state = hub.state(&self.who);
                let mine = state.instances[instance]
                    .demand
                    .as_ref()
                    .is_some_and(|demand| demand.registration == self.registration);
                if mine {
                    state.instances[instance].demand = None;
                    self.formal_grant = hub.append(
                        &mut state,
                        Some(instance),
                        RecordKind::FormalGrant {
                            registration: self.registration,
                            by_pump: false,
                        },
                    );
                } else {
                    hub.check.record(format!(
                        "internal: dispatch {} holds admission but its registration {} is gone",
                        self.who.name(),
                        self.registration
                    ));
                }
                drop(state);
                hub.scheduler_lock.release(&self.who);
                hub.queue_order[instance].release(&self.who);
                self.granted_us = hub.clock.now_us();
                self.phase = DispatchPhase::Work;
                Step::Progress
            }
            DispatchPhase::WithdrawQueueOrder => {
                if !take(&hub.queue_order[instance], &self.who, mode) {
                    return Step::Blocked;
                }
                self.phase = DispatchPhase::Withdraw;
                Step::Progress
            }
            DispatchPhase::Withdraw => {
                if !take(&hub.scheduler_lock, &self.who, mode) {
                    return Step::Blocked;
                }
                let mut state = hub.state(&self.who);
                let mine = state.instances[instance]
                    .demand
                    .as_ref()
                    .is_some_and(|demand| demand.registration == self.registration);
                if mine {
                    state.instances[instance].demand = None;
                    hub.append(
                        &mut state,
                        Some(instance),
                        RecordKind::Withdraw {
                            registration: self.registration,
                        },
                    );
                }
                drop(state);
                hub.scheduler_lock.release(&self.who);
                hub.queue_order[instance].release(&self.who);
                if mine {
                    self.phase = DispatchPhase::Done;
                    return Step::Done(DispatchOutcome::Contended {
                        registration: self.registration,
                    });
                }
                // The pump granted the demand meanwhile and handed admission over.
                let seq = match mode {
                    Mode::Blocking => self.granted.take(),
                    Mode::NonBlocking => self.granted.try_take().unwrap_or(0),
                };
                self.formal_grant = seq;
                self.by_pump = true;
                self.granted_us = hub.clock.now_us();
                self.phase = DispatchPhase::Work;
                Step::Progress
            }
            DispatchPhase::Work => {
                if !hub.admission[instance].held_by(&self.who) {
                    hub.check.record(format!(
                        "internal: formal work of {} runs without admission",
                        self.who.name()
                    ));
                }
                if let Some(work) = self.work.as_mut() {
                    self.result = work(hub, instance);
                }
                self.phase = DispatchPhase::Release;
                Step::Progress
            }
            DispatchPhase::Release => {
                hub.admission[instance].release(&self.who);
                self.phase = DispatchPhase::Done;
                Step::Done(DispatchOutcome::Granted {
                    registration: self.registration,
                    formal_grant: self.formal_grant,
                    registered_us: self.registered_us,
                    granted_us: self.granted_us,
                    by_pump: self.by_pump,
                    work: Box::new(self.result.clone()),
                })
            }
            DispatchPhase::Done => Step::Done(DispatchOutcome::Contended {
                registration: self.registration,
            }),
        }
    }
}

// ---- P5 ----

/// P5: a manual refresh of one frame, as the read-only observation: blocking admission, the
/// gates, the call, the publish (§3.3.2; it ignores the queue, §3.3.3).
pub fn manual_refresh(
    hub: &Hub,
    who: &LockOwner,
    instance: usize,
    path: PathKind,
) -> Result<Option<u64>, Disposition> {
    hub.admission[instance].acquire(who);
    hub.scheduler_lock.acquire(who);
    let admitted = {
        let mut state = hub.state(who);
        let gates = state.active_gates(instance);
        match refusing_gate(Entry::ManualRefresh, gates, false) {
            Some(gate) => Err(Disposition::SkippedGate(gate.as_str().to_owned())),
            None => {
                state.instances[instance].inflight += 1;
                hub.append(&mut state, Some(instance), RecordKind::Manual);
                Ok(state.instances[instance].generation)
            }
        }
    };
    hub.scheduler_lock.release(who);
    let generation = match admitted {
        Ok(generation) => generation,
        Err(refused) => {
            hub.admission[instance].release(who);
            return Err(refused);
        }
    };
    let result = {
        let mut slots = lock(&hub.backends[instance]);
        match slots.get(path.backend()) {
            Some(backend) => backend.sample(None),
            None => Err(BackendFailure::new(
                "backend_closed",
                crate::backend::FailureClass::CellPath,
                "the path's backend is not open",
            )),
        }
    };
    hub.scheduler_lock.acquire(who);
    let published = {
        let mut state = hub.state(who);
        state.instances[instance].inflight = state.instances[instance].inflight.saturating_sub(1);
        match result {
            Ok(frame) if state.instances[instance].generation == generation => {
                state.instances[instance].preview_sequence += 1;
                let preview_sequence = state.instances[instance].preview_sequence;
                let now = hub.clock.now_us();
                hub.views.publish(PreviewFrame {
                    instance,
                    path,
                    owner_epoch: state.owner_epoch,
                    generation,
                    preview_sequence,
                    grant_sequence: 0,
                    width: frame.width,
                    height: frame.height,
                    layout: frame.layout,
                    transform: "identity",
                    sample_start_us: now,
                    sample_end_us: now,
                    published_us: now,
                    stream_sequence: frame.stream_sequence,
                    device_pts_us: frame.device_pts_us,
                    received_us: frame.received_us,
                    validity: Validity::Live,
                    pixels: Arc::new(frame.pixels),
                });
                Ok(Some(preview_sequence))
            }
            Ok(_) => Err(Disposition::DiscardedStale("generation".to_owned())),
            Err(failure) => Err(Disposition::Failed(failure.code)),
        }
    };
    hub.scheduler_lock.release(who);
    hub.admission[instance].release(who);
    published
}

// ---- P6 ----

/// P6: an auxiliary device call outside the capture backend (calibration, APK control, the
/// device-side check): the gates and the in-flight count under the scheduler lock, the call
/// without any bench lock (§3.3.2).
pub fn aux_call<T>(
    hub: &Hub,
    who: &LockOwner,
    instance: usize,
    entry: Entry,
    in_close_chain: bool,
    call: impl FnOnce() -> T,
) -> Result<T, GateKind> {
    hub.scheduler_lock.acquire(who);
    let admitted = {
        let mut state = hub.state(who);
        let gates = state.active_gates(instance);
        match refusing_gate(entry, gates, in_close_chain) {
            Some(gate) => Err(gate),
            None => {
                state.instances[instance].aux_inflight += 1;
                hub.append(&mut state, Some(instance), RecordKind::AuxStart { entry });
                Ok(())
            }
        }
    };
    hub.scheduler_lock.release(who);
    admitted?;
    let value = call();
    hub.scheduler_lock.acquire(who);
    {
        let mut state = hub.state(who);
        state.instances[instance].aux_inflight =
            state.instances[instance].aux_inflight.saturating_sub(1);
        hub.append(&mut state, Some(instance), RecordKind::AuxEnd { entry });
    }
    hub.scheduler_lock.release(who);
    Ok(value)
}

// ---- P7 ----

/// What an automatic reopen ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReopenOutcome {
    SkippedBusy,
    Refused(GateKind),
    StaleGeneration,
    Reopened { generation: u64 },
    Failed(String),
}

/// P7: an automatic reopen: try admission, the gates and the generation under queue order and
/// scheduler, the reopen holding admission, the generation +1 (§3.3.2).
pub fn auto_reopen(
    hub: &Hub,
    who: &LockOwner,
    instance: usize,
    expected_generation: u64,
    reopen: impl FnOnce(&mut BackendSlots) -> Result<(), String>,
) -> ReopenOutcome {
    if !hub.admission[instance].try_acquire(who) {
        return ReopenOutcome::SkippedBusy;
    }
    hub.queue_order[instance].acquire(who);
    hub.scheduler_lock.acquire(who);
    let admitted = {
        let state = hub.state(who);
        let gates = state.active_gates(instance);
        match refusing_gate(Entry::AutoReopen, gates, false) {
            Some(gate) => Err(ReopenOutcome::Refused(gate)),
            None if state.instances[instance].generation != expected_generation => {
                Err(ReopenOutcome::StaleGeneration)
            }
            None => Ok(()),
        }
    };
    hub.scheduler_lock.release(who);
    hub.queue_order[instance].release(who);
    if let Err(outcome) = admitted {
        hub.admission[instance].release(who);
        return outcome;
    }
    let reopened = {
        let mut slots = lock(&hub.backends[instance]);
        reopen(&mut slots)
    };
    let outcome = match reopened {
        Ok(()) => {
            hub.scheduler_lock.acquire(who);
            let generation = {
                let mut state = hub.state(who);
                state.instances[instance].generation += 1;
                let generation = state.instances[instance].generation;
                hub.append(
                    &mut state,
                    Some(instance),
                    RecordKind::Generation { generation },
                );
                generation
            };
            hub.scheduler_lock.release(who);
            ReopenOutcome::Reopened { generation }
        }
        Err(detail) => ReopenOutcome::Failed(detail),
    };
    hub.admission[instance].release(who);
    outcome
}

/// A floor capture by formal work (P4), holding admission.
pub fn formal_capture(
    hub: &Hub,
    instance: usize,
    kind: BackendKind,
    floor: Option<Floor>,
) -> Result<SampledFrame, BackendFailure> {
    let mut slots = lock(&hub.backends[instance]);
    match slots.get(kind) {
        Some(backend) => backend.sample(floor),
        None => Err(BackendFailure::new(
            "backend_closed",
            crate::backend::FailureClass::CellPath,
            "the formal capture backend is not open",
        )
        .without_native_call()),
    }
}

/// A formal tap (P4), holding admission.
pub fn formal_tap(
    hub: &Hub,
    instance: usize,
    kind: BackendKind,
    x: i32,
    y: i32,
) -> Result<(), BackendFailure> {
    let mut slots = lock(&hub.backends[instance]);
    match slots.get(kind) {
        Some(backend) => backend.tap(x, y),
        None => Err(BackendFailure::new(
            "backend_closed",
            crate::backend::FailureClass::CellPath,
            "the touch backend is not open",
        )
        .without_native_call()),
    }
}
