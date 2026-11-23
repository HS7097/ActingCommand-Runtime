// SPDX-License-Identifier: AGPL-3.0-only

//! The Host lifecycle owner's admission: which work may begin, the idle test a shutdown request
//! is decided by, and the checks of a start's preparation. Admission is closed while the start
//! prepares, opens once it is ready and closes for good once a shutdown is accepted. Workflow
//! #434 LAB-6S4a (0.12.63): the installer's transition (`InstallTransition`, `install-control`
//! revision 1, and the upgrade state planned beside it) and actingd's held start are gone; every
//! start is cold, and nothing of installation control is durable any more.
//!
//! Workflow #503 SD-1 (0.12.83, register §1.525): a shutdown request that finds nothing refusing
//! it but lifecycle work in flight on other threads, still in flight when its settle ends, opens a
//! settling window (memory only) and is answered `runtime_shutdown_settling` with `retry_after_ms`
//! (the Runtime error code stays `runtime_busy`). While the window is open no new policy cycle
//! begins and no new tick of the performance thread takes lifecycle work; it ends by itself after
//! `SHUTDOWN_SETTLING_WINDOW_MS`, and at once when a shutdown is accepted. Every other refusal is
//! answered at once, `runtime_shutdown_denied`, its busy cause named by `shutdown_obstacle`.

use super::*;
use crate::codes::HostCode;
use crate::codes::HostLocation;
use crate::codes::ShutdownObstacle;
use actingcommand_contract::codes::ContractLocation;
use actingcommand_contract::outcome::Code;
use actingcommand_contract::{RuntimeShutdownDecision, RuntimeShutdownTarget};
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Instant;

/// The two runtime facts the installer's transition recorded until 0.12.62. A ledger written by
/// such a build may still hold them (a transition that drained and committed its shutdown leaves
/// both); a start invalidates them once, as those builds' starts did, and nothing writes them any
/// more.
const TRANSITION_KEY: &str = "host.install_transition";
const PAUSES_KEY: &str = "host.install_transition.pauses";
/// The longest a shutdown request waits for lifecycle work already in flight on other threads
/// to end before it is decided (`settle_for_shutdown`); well inside a client's 5 s I/O budget.
const SHUTDOWN_SETTLE_TIMEOUT: Duration = Duration::from_secs(2);
/// Workflow #503 SD-1: how long a settling window lasts after the answer that opened it, on the
/// Runtime clock's monotonic milliseconds. Long enough for the work it lets end: a policy cycle of
/// today's length (4 to 5 s) and a retention step after it (up to 3.3 s), 8.3 s back to back, with
/// room for the client's next ask (`SHUTDOWN_SETTLING_RETRY_AFTER_MS`) and that ask's own settle
/// (`SHUTDOWN_SETTLE_TIMEOUT`); short enough that a window no ask follows holds new cycles and
/// ticks back for 15 s at most, its triggers kept and run after it.
pub(crate) const SHUTDOWN_SETTLING_WINDOW_MS: u64 = 15_000;
/// Workflow #503 SD-1: when a settling answer tells its client to ask again: one second, after
/// which the next ask lets the work end within its own settle of two seconds more.
pub(crate) const SHUTDOWN_SETTLING_RETRY_AFTER_MS: u64 = 1_000;
/// Workflow #503 SD-1 fix round 1: the longest a settling window lasts from its first opening,
/// however often it is opened again. It covers the installer's asks: they stop within
/// `install.stop.shutdown_wait_ms` of the first (60 s as shipped), the last ask's settle inside it,
/// with one window (15 s) of room; a client that never stops asking holds new cycles and ticks
/// back no longer than this.
pub(crate) const SHUTDOWN_SETTLING_LIMIT_MS: u64 = 75_000;

/// Workflow #503 SD-1: the settling window, in memory only, on the Runtime clock's monotonic
/// milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettlingWindow {
    /// No window is open, and one may open.
    Closed,
    /// Open since `since_ms`, its first opening, until `until_ms`, which no answer moves past
    /// `since_ms + SHUTDOWN_SETTLING_LIMIT_MS`.
    Open { since_ms: u64, until_ms: u64 },
    /// A window ended and no policy cycle has begun since: none may open until one has.
    Ended,
}

pub(super) struct LifecycleAdmission {
    closed: bool,
    stopping: bool,
    preparing: bool,
    active: std::collections::HashMap<thread::ThreadId, usize>,
    /// Workflow #503 SD-1: the settling window a shutdown request opens. Memory only.
    settling: SettlingWindow,
}

/// Workflow #503 SD-1: the work a settling window holds back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UnsettledWork {
    /// A new policy cycle (`RuntimeHost::begin_policy_work`); the first after a window ended lets
    /// a window open again.
    PolicyCycle,
    /// A new tick of the performance thread.
    PerformanceTick,
}

impl LifecycleAdmission {
    /// The admission of a starting host: closed, and preparing until the start is ready.
    pub(super) fn starting() -> Self {
        Self {
            closed: true,
            stopping: false,
            preparing: true,
            active: std::collections::HashMap::new(),
            settling: SettlingWindow::Closed,
        }
    }

    /// Workflow #503 SD-1: an open window whose end `now_ms` reached has ended, so that nothing it
    /// held back stays held.
    fn settling_seen_at(&mut self, now_ms: u64) {
        if let SettlingWindow::Open { until_ms, .. } = self.settling
            && now_ms >= until_ms
        {
            self.settling = SettlingWindow::Ended;
        }
    }

    /// Workflow #503 SD-1: opens the settling window at `now_ms` for `SHUTDOWN_SETTLING_WINDOW_MS`,
    /// or an open one again from `now_ms`, never past its first opening plus
    /// `SHUTDOWN_SETTLING_LIMIT_MS`; `false`, opening nothing, when a window ended (at its own end
    /// or at that bound) and no policy cycle has begun since.
    fn open_settling_at(&mut self, now_ms: u64) -> RuntimeHostResult<bool> {
        self.settling_seen_at(now_ms);
        let since_ms = match self.settling {
            SettlingWindow::Closed => now_ms,
            SettlingWindow::Open { since_ms, .. } => since_ms,
            SettlingWindow::Ended => return Ok(false),
        };
        let until_ms = deadline_after(now_ms, SHUTDOWN_SETTLING_WINDOW_MS)?
            .min(deadline_after(since_ms, SHUTDOWN_SETTLING_LIMIT_MS)?);
        self.settling = SettlingWindow::Open { since_ms, until_ms };
        Ok(true)
    }
}

/// What a shutdown request of this Runtime's own target meets once its settle ended, while the
/// Runtime is not stopping (Workflow #503 SD-1). `None` from [`shutdown_busy`] is an accepted
/// shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownBusy {
    /// Something refuses it at once, the word naming what: `runtime_shutdown_denied` with
    /// `runtime_busy`, and no settling window opens.
    Refused(ShutdownObstacle),
    /// Nothing refuses it but lifecycle work in flight on other threads, which the settle waits
    /// for and did not see end: `runtime_shutdown_settling`, and the settling window opens; once
    /// a window ended and no policy cycle has begun since, `runtime_shutdown_denied` with
    /// `work_outlived_window` instead, which opens nothing.
    Settling,
}

/// Workflow #503 SD-1: the one order of what refuses a shutdown request of this Runtime's own
/// target: a start that prepares, then what holds or waits (`obstacle`: a lease held, a waiting
/// entry no pause holds, a client's reset still owed, a policy run a worker holds), then
/// lifecycle work of the requesting thread itself (`own_work`), which cannot end while it waits;
/// only lifecycle work in flight on other threads alone (`other_work`) settles.
pub(crate) fn shutdown_busy(
    preparing: bool,
    obstacle: Option<ShutdownObstacle>,
    own_work: bool,
    other_work: bool,
) -> Option<ShutdownBusy> {
    if preparing {
        return Some(ShutdownBusy::Refused(ShutdownObstacle::StartPreparing));
    }
    if let Some(obstacle) = obstacle {
        return Some(ShutdownBusy::Refused(obstacle));
    }
    if own_work {
        return Some(ShutdownBusy::Refused(
            ShutdownObstacle::RequestingThreadWork,
        ));
    }
    other_work.then_some(ShutdownBusy::Settling)
}

// Scoped work cannot move to another thread: nested work inherits only the same admitted
// call stack. Queued continuations enter explicitly while the owning queue is still held.
pub(super) struct LifecycleWork<'a> {
    shared: &'a HostShared,
    _same_thread: PhantomData<Rc<()>>,
}

impl Drop for LifecycleWork<'_> {
    fn drop(&mut self) {
        // Poison never reopens admission. Removing this completed work is still necessary
        // for cleanup; every later admission reports the original poisoned lock as fatal.
        let mut admission = self
            .shared
            .lifecycle_admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = thread::current().id();
        if let Some(count) = admission.active.get_mut(&id) {
            *count -= 1;
            if *count == 0 {
                admission.active.remove(&id);
                // A shutdown request may be waiting for the work in flight to end.
                if admission.active.is_empty() {
                    self.shared.lifecycle_settled.notify_all();
                }
            }
        }
    }
}

/// The deadline `timeout_ms` after `now_ms` on the host's monotonic clock (HOST-I2); the
/// recovery ladder's readiness window reads the same clock.
pub(super) fn deadline_after(now_ms: u64, timeout_ms: u64) -> RuntimeHostResult<u64> {
    now_ms.checked_add(timeout_ms).ok_or_else(|| {
        RuntimeHostError::fatal(
            HostCode::MonotonicClockOverflow,
            HostLocation::ReadRuntimeClock,
            RuntimeErrorCode::RuntimeHalted,
        )
    })
}

fn install_error(code: impl Into<Code>) -> RuntimeHostError {
    RuntimeHostError::request(
        code,
        ContractLocation::InstallTransition,
        RuntimeErrorCode::InvalidRequest,
    )
}

fn prepared<T>(value: &OnceLock<T>) -> RuntimeHostResult<&T> {
    value.get().ok_or_else(|| {
        RuntimeHostError::fatal(
            HostCode::RuntimeComponentNotPrepared,
            HostLocation::AccessPreparedRuntime,
            RuntimeErrorCode::RuntimeHalted,
        )
    })
}

pub(super) fn set_prepared<T>(slot: &OnceLock<T>, value: T) -> RuntimeHostResult<()> {
    slot.set(value).map_err(|_| {
        RuntimeHostError::fatal(
            HostCode::RuntimeComponentAlreadyPrepared,
            HostLocation::PrepareRuntime,
            RuntimeErrorCode::RuntimeHalted,
        )
    })
}

impl HostShared {
    pub(super) fn execution(&self) -> RuntimeHostResult<&ExecutionKernel> {
        prepared(&self.execution)
    }
    pub(super) fn policy(&self) -> RuntimeHostResult<&Mutex<PolicyHost>> {
        prepared(&self.policy)
    }
    pub(super) fn monitor_registry(&self) -> RuntimeHostResult<&Mutex<MonitorRegistry>> {
        prepared(&self.monitor_registry)
    }
    pub(super) fn startup_packages(
        &self,
    ) -> RuntimeHostResult<&BTreeMap<InstanceId, ContainedTaskRequest>> {
        prepared(&self.startup_packages)
    }
    pub(super) fn stuck_recovery(
        &self,
    ) -> RuntimeHostResult<&BTreeMap<InstanceId, actingcommand_contract::InstanceStuckRecovery>>
    {
        prepared(&self.stuck_recovery)
    }
    /// Workflow #369 S6c: the configured emulator starts, in configuration order.
    pub(super) fn emulator_autostart(&self) -> RuntimeHostResult<&[(InstanceId, String)]> {
        prepared(&self.emulator_autostart).map(Vec::as_slice)
    }

    fn count_work<'a>(&'a self, admission: &mut LifecycleAdmission) -> LifecycleWork<'a> {
        *admission.active.entry(thread::current().id()).or_default() += 1;
        LifecycleWork {
            shared: self,
            _same_thread: PhantomData,
        }
    }

    pub(super) fn begin_work(&self) -> RuntimeHostResult<Option<LifecycleWork<'_>>> {
        let mut admission = lock(&self.lifecycle_admission, HostLocation::AdmitRuntimeWork)?;
        if admission.stopping
            || (admission.closed && !admission.active.contains_key(&thread::current().id()))
        {
            return Ok(None);
        }
        Ok(Some(self.count_work(&mut admission)))
    }

    /// Workflow #503 SD-1: the work a settling window holds back, `work` (a new policy cycle,
    /// `RuntimeHost::begin_policy_work`, or a new tick of the performance thread): as
    /// [`Self::begin_work`], and none while a settling window is open. The Runtime clock is read
    /// only while a window is open; one whose end has come has ended here, and the first policy
    /// cycle that begins after a window ended lets a window open again.
    pub(super) fn begin_unsettled_work(
        &self,
        work: UnsettledWork,
    ) -> RuntimeHostResult<Option<LifecycleWork<'_>>> {
        let mut admission = lock(&self.lifecycle_admission, HostLocation::AdmitRuntimeWork)?;
        if matches!(admission.settling, SettlingWindow::Open { .. }) {
            admission.settling_seen_at(self.monotonic_ms()?);
            if matches!(admission.settling, SettlingWindow::Open { .. }) {
                return Ok(None);
            }
        }
        if admission.stopping
            || (admission.closed && !admission.active.contains_key(&thread::current().id()))
        {
            return Ok(None);
        }
        if work == UnsettledWork::PolicyCycle && admission.settling == SettlingWindow::Ended {
            admission.settling = SettlingWindow::Closed;
        }
        Ok(Some(self.count_work(&mut admission)))
    }

    /// Workflow #503 SD-1 (ruling 7): whether the frame cleaner's round gives way now, at its two
    /// stop checks (before each statement of a round, between the frames it acts on) and the
    /// listing's hand-over at a pass start: the Runtime's stop was requested, or a shutdown request
    /// opened a settling window. A round of a tick that began before the window opened then ends
    /// there, its pass kept for a later round; a pass start that gives way while it hands over
    /// its listing does not begin, and the next pass starts only after the cleaner's interval
    /// (`SWEEP_INTERVAL_MS`, ten minutes), as a pass start the stop ended. No tick begins while the
    /// window is open. What the cleaner examines and removes is unchanged. A poisoned admission
    /// lock is read as it is: every later admission reports it as fatal.
    pub(super) fn retention_gives_way(&self) -> bool {
        self.fatal.is_shutdown_requested()
            || matches!(
                self.lifecycle_admission
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .settling,
                SettlingWindow::Open { .. }
            )
    }

    /// Cleanup is part of admitted work even when shutdown has already closed the root gate.
    pub(super) fn work_guard(&self) -> RuntimeHostResult<LifecycleWork<'_>> {
        let mut admission = lock(&self.lifecycle_admission, HostLocation::AdmitRuntimeCleanup)?;
        Ok(self.count_work(&mut admission))
    }

    pub(super) fn begin_continuation(&self) -> RuntimeHostResult<Option<LifecycleWork<'_>>> {
        let mut admission = lock(
            &self.lifecycle_admission,
            HostLocation::AdmitRuntimeContinuation,
        )?;
        if admission.stopping || admission.closed {
            return Ok(None);
        }
        Ok(Some(self.count_work(&mut admission)))
    }

    /// Workflow #369-1 (review L1): whether the host is stopping, so a long wait on the
    /// host-work thread ends at its next poll. Workflow #434 LAB-6S4a: an install transition no
    /// longer drains the host.
    pub(super) fn lifecycle_draining(&self) -> RuntimeHostResult<bool> {
        let admission = lock(
            &self.lifecycle_admission,
            HostLocation::ReadLifecycleDraining,
        )?;
        Ok(admission.stopping)
    }

    pub(super) fn request_is_lifecycle_control(operation: &RuntimeOperation) -> bool {
        matches!(
            operation,
            RuntimeOperation::RequestShutdown { .. }
                | RuntimeOperation::Health
                | RuntimeOperation::RuntimeFactSnapshot
                | RuntimeOperation::QueryEvents { .. }
                | RuntimeOperation::ReadMaterial { .. }
                // Workflow #389 section 3.6: accepted while the start prepares.
                | RuntimeOperation::BindChannel { .. }
        )
    }

    pub(super) fn begin_request(
        &self,
        operation: &RuntimeOperation,
    ) -> RuntimeHostResult<Option<LifecycleWork<'_>>> {
        if Self::request_is_lifecycle_control(operation) {
            return Ok(None);
        }
        if matches!(
            operation,
            RuntimeOperation::RenewLease { .. }
                | RuntimeOperation::ReleaseLease { .. }
                | RuntimeOperation::Input { .. }
                | RuntimeOperation::PollQueuedLease { .. }
                | RuntimeOperation::CancelQueuedLease { .. }
                | RuntimeOperation::CancelContainedTask { .. }
        ) {
            // These formal handlers validate the original lease/request/connection before
            // doing anything. They cannot acquire a new root lease.
            self.begin_continuation()
        } else {
            self.begin_work()
        }
    }

    /// What holds or waits so that a shutdown request is refused at once, the first in this
    /// order: a holder (`lease`), a waiting entry (`waiting_entry`; the scheduler's own read), an
    /// install cleanup pending (`client_reset`), a policy run a worker holds (`policy_run`);
    /// `None` when nothing does. Workflow #369 §3.5 (review2 L1): with no lifecycle work in flight
    /// (`active`, which counts every running instance worker) and no start preparing, `None` is
    /// an idle Runtime, which `request-shutdown` accepts (Workflow #503 SD-1: [`shutdown_busy`]).
    ///
    /// Workflow #411 A3: a routine entry on an instance whose routine gate a scheduling pause
    /// holds (the global pause or the instance's own) is not a waiting entry: it does no work
    /// until a resume, and a shutdown disconnects it (`lease.queue_disconnected`) while the
    /// pause is kept and restored, so the next start enqueues it again. Only the pause half of
    /// the routine gate is read (`routine_gate` without the #403 resource fence): a routine
    /// entry a fence holds still waits, since the fence lifts without a resume. The pauses are
    /// read first, then the scheduler, as `pump` reads the gate; a resume done before this read
    /// leaves its entries eligible or granted, so they count.
    fn shutdown_obstacle(&self) -> RuntimeHostResult<Option<ShutdownObstacle>> {
        let registered = lock(
            &self.registered_instances,
            HostLocation::ReadInstanceRegistry,
        )?
        .iter()
        .map(|(instance_id, instance)| (*instance_id, instance.instance_alias.clone()))
        .collect::<Vec<_>>();
        let (global_pause, paused_instances) = {
            let pauses = lock(&self.scheduling_pause, HostLocation::ReadRoutineGate)?;
            (
                pauses.holds_global(),
                registered
                    .iter()
                    .filter(|(_, instance_alias)| pauses.holds_instance(instance_alias))
                    .map(|(instance_id, _)| *instance_id)
                    .collect::<BTreeSet<_>>(),
            )
        };
        // Workflow #386 SR-4a3: a startup package or an autostart claim that blackout silence
        // holds does no work until the blackout ends, as a paused routine entry does.
        let mut gates = BTreeMap::new();
        for (instance_id, instance_alias) in &registered {
            let pause = if global_pause || paused_instances.contains(instance_id) {
                ClaimGate::RoutineHeld
            } else {
                ClaimGate::Open
            };
            gates.insert(
                *instance_id,
                self.silenced_gate(*instance_id, instance_alias, pause)?,
            );
        }
        let held_or_waiting = {
            let scheduler = lock(&self.scheduler, HostLocation::CheckInstallLeases)?;
            if !scheduler.active_tokens().is_empty() {
                Some(ShutdownObstacle::Lease)
            } else if scheduler.waiting_total_gated(|instance_id| {
                gates.get(&instance_id).copied().unwrap_or(
                    if global_pause || paused_instances.contains(&instance_id) {
                        ClaimGate::RoutineHeld
                    } else {
                        ClaimGate::Open
                    },
                )
            }) != 0
            {
                Some(ShutdownObstacle::WaitingEntry)
            } else {
                None
            }
        };
        if held_or_waiting.is_some() {
            return Ok(held_or_waiting);
        }
        if lock(&self.scheduling_pause, HostLocation::CheckInstallCleanup)?
            .install_cleanup_pending()
        {
            return Ok(Some(ShutdownObstacle::ClientReset));
        }
        // Workflow #427-A (C2): a policy run an instance worker still holds, from its hand-off to
        // its settlement, is in flight; a shutdown decision waits for it.
        Ok(self
            .any_policy_run_working()?
            .then_some(ShutdownObstacle::PolicyRun))
    }

    /// An idle Runtime has lifecycle work in flight now and then as well: the lease sweep every
    /// 50 ms, the performance tick, a closed connection's cleanup, a reply still being written
    /// (train 11 boot smoke, run 38560460358: `runtime_busy` right after `runtime_ready`). A
    /// shutdown request that finds such work on other threads, and no holder, no waiting entry
    /// (an entry the pause gate holds is none, Workflow #411 A3) and no install cleanup, lets it
    /// end, at most `SHUTDOWN_SETTLE_TIMEOUT`; it is then decided with the admission still
    /// held, so no new work starts in between. Work of the requesting thread itself cannot end
    /// while it waits, and every other refusal is answered at once. Workflow #503 SD-1: work that
    /// did not end within it opens the settling window ([`Self::shutdown_under_gate`]). Fix round
    /// 1: the settle waits exactly while the one order ([`Self::shutdown_decision`]) finds the
    /// request kept busy by work in flight on other threads alone, and returns at every other
    /// answer.
    fn settle_for_shutdown<'a>(
        &'a self,
        mut admission: MutexGuard<'a, LifecycleAdmission>,
        target: RuntimeShutdownTarget,
    ) -> RuntimeHostResult<MutexGuard<'a, LifecycleAdmission>> {
        let deadline = Instant::now() + SHUTDOWN_SETTLE_TIMEOUT;
        loop {
            let (_, busy) = self.shutdown_decision(&admission, target)?;
            if busy != Some(ShutdownBusy::Settling) {
                return Ok(admission);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(admission);
            }
            admission = self
                .lifecycle_settled
                .wait_timeout(admission, remaining)
                .map_err(|_| lock_poison_error(ContractLocation::RequestRuntimeShutdown))?
                .0;
        }
    }

    pub(super) fn request_shutdown(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        target: RuntimeShutdownTarget,
    ) -> Result<OperationSuccess, RequestFailure> {
        let admission = lock(
            &self.lifecycle_admission,
            ContractLocation::RequestRuntimeShutdown,
        )?;
        let mut admission = self.settle_for_shutdown(admission, target)?;
        self.shutdown_under_gate(request, target, &mut admission)
    }

    /// Workflow #503 SD-1 fix round 1: the decision of a shutdown request of `target` now, and
    /// what keeps it busy, in the one order both the settle and the decision read: another owner's
    /// target, a Runtime already stopping, then [`shutdown_busy`].
    fn shutdown_decision(
        &self,
        admission: &LifecycleAdmission,
        target: RuntimeShutdownTarget,
    ) -> RuntimeHostResult<(RuntimeShutdownDecision, Option<ShutdownBusy>)> {
        if target != self.shutdown_target {
            return Ok((RuntimeShutdownDecision::OwnerMismatch, None));
        }
        if admission.stopping || self.fatal.is_shutdown_requested() {
            return Ok((RuntimeShutdownDecision::AlreadyStopping, None));
        }
        let obstacle = if admission.preparing {
            None
        } else {
            self.shutdown_obstacle()?
        };
        Ok(
            match shutdown_busy(
                admission.preparing,
                obstacle,
                admission.active.contains_key(&thread::current().id()),
                !admission.active.is_empty(),
            ) {
                Some(busy) => (RuntimeShutdownDecision::Busy, Some(busy)),
                None => (RuntimeShutdownDecision::Accepted, None),
            },
        )
    }

    /// The decision of a shutdown request once its settle ended. Workflow #503 SD-1: a request of
    /// this Runtime's own target that only lifecycle work in flight on other threads keeps busy
    /// opens the settling window, to `SHUTDOWN_SETTLING_WINDOW_MS` from its answer (a request
    /// answered so while a window is open opens it again from its own answer, fix round 1: never
    /// past `SHUTDOWN_SETTLING_LIMIT_MS` from its first opening), and is answered
    /// `runtime_shutdown_settling` with `retry_after_ms`; its record keeps the decision `busy`.
    /// Once a window ended (at its end or at that bound) and no policy cycle has begun since,
    /// such a request is refused `runtime_shutdown_denied` with `work_outlived_window` and opens
    /// nothing. An accepted shutdown closes the window. Every other refusal opens none.
    fn shutdown_under_gate(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        target: RuntimeShutdownTarget,
        admission: &mut LifecycleAdmission,
    ) -> Result<OperationSuccess, RequestFailure> {
        if let Some(error) = self.fatal.answer()? {
            return Err(error.into());
        }
        let (decision, busy) = self.shutdown_decision(admission, target)?;
        let event = self.append_event(
            if decision == RuntimeShutdownDecision::Accepted {
                EventSeverity::Info
            } else {
                EventSeverity::Warning
            },
            request.source(),
            OriginModule::Runtime,
            request.actor(),
            request.event_links(None, None, None),
            RuntimePayloadDraft::lifecycle_observed(
                self.owner_epoch,
                RuntimeLifecyclePhase::ShutdownRequest { target, decision },
                AuditInput::new(),
            ),
        )?;
        if decision == RuntimeShutdownDecision::Accepted {
            admission.closed = true;
            admission.stopping = true;
            admission.settling = SettlingWindow::Closed;
            self.fatal.request_shutdown();
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Admitted,
                terminal: Some(terminal(&event)),
                result: RuntimeResult::ShutdownAccepted { target },
                failure_outcome: None,
            });
        }
        // Fix round 1: work in flight that outlived a window, while none may open, is refused.
        let busy = if busy == Some(ShutdownBusy::Settling)
            && !admission.open_settling_at(self.monotonic_ms()?)?
        {
            Some(ShutdownBusy::Refused(ShutdownObstacle::WorkOutlivedWindow))
        } else {
            busy
        };
        let refusal = match busy {
            Some(ShutdownBusy::Settling) => RuntimeHostError::with_projection(
                HostCode::RuntimeShutdownSettling,
                ContractLocation::RequestRuntimeShutdown,
                RuntimeErrorProjection::new(RuntimeErrorCode::RuntimeBusy, false)
                    .with_retry_after(SHUTDOWN_SETTLING_RETRY_AFTER_MS),
            )
            .with_outcome_values(|outcome| {
                outcome.retry_after_ms(SHUTDOWN_SETTLING_RETRY_AFTER_MS)
            }),
            Some(ShutdownBusy::Refused(obstacle)) => RuntimeHostError::request(
                HostCode::RuntimeShutdownDenied,
                ContractLocation::RequestRuntimeShutdown,
                RuntimeErrorCode::RuntimeBusy,
            )
            .with_outcome_values(|outcome| outcome.shutdown_obstacle(obstacle)),
            None => RuntimeHostError::request(
                HostCode::RuntimeShutdownDenied,
                ContractLocation::RequestRuntimeShutdown,
                if decision == RuntimeShutdownDecision::OwnerMismatch {
                    RuntimeErrorCode::RuntimeOwnerMismatch
                } else {
                    RuntimeErrorCode::RuntimeUnavailable
                },
            ),
        };
        Err(RequestFailure::request(
            refusal,
            RuntimeReceiptState::Denied,
            Some(terminal(&event)),
        ))
    }

    /// The start's one conversion of what a ledger of 0.12.62 or earlier may hold of the
    /// installer's transition (Workflow #434 LAB-6S4a): each of its two runtime facts still in the
    /// store is invalidated as a takeover, as every start of those builds did on a new epoch.
    /// Nothing reads them; a store without them is left as it is.
    pub(super) fn invalidate_install_transition_facts(&self) -> RuntimeHostResult<()> {
        for key in [TRANSITION_KEY, PAUSES_KEY] {
            let present = lock(&self.runtime_facts, HostLocation::ReadOldInstallTransition)?
                .get(&RuntimeFactScope::Runtime, key)
                .is_some();
            if present {
                self.invalidate_runtime_fact(
                    &RuntimeFactScope::Runtime,
                    key,
                    RuntimeFactInvalidationReason::RuntimeTakeover,
                )?;
            }
        }
        Ok(())
    }

    /// The error that stops a start, if anything stops it: a latched failure ends it as
    /// `install_startup_stopped` with that condition named (Workflow #381 A R3′), a halt a
    /// restart may fix. Workflow #434 LAB-6S4a (0.12.63): a shutdown request is answered busy
    /// while the start prepares (`shutdown_under_gate`), so no accepted shutdown can stop it, and
    /// `install_startup_shut_down`, which that stop of a held start raised, is retired.
    fn install_start_stopped(&self) -> RuntimeHostResult<Option<RuntimeHostError>> {
        Ok(self
            .fatal
            .current()?
            .map(|cause| install_error(HostCode::InstallStartupStopped).with_latched_cause(&cause)))
    }

    /// A boundary of the start's preparation: the start goes on unless a latched failure stops
    /// it.
    pub(super) fn check_install_preparation(&self) -> RuntimeHostResult<()> {
        if let Some(stopped) = self.install_start_stopped()? {
            return Err(stopped);
        }
        Ok(())
    }

    /// The end of a start that prepared: admission opens.
    pub(super) fn finish_install_preparation(&self) -> RuntimeHostResult<()> {
        let mut admission = lock(
            &self.lifecycle_admission,
            HostLocation::FinishInstallPreparation,
        )?;
        if let Some(stopped) = self.install_start_stopped()? {
            return Err(stopped);
        }
        admission.closed = false;
        admission.preparing = false;
        Ok(())
    }

    /// A start that failed: admission stays closed and the host is no longer preparing.
    pub(super) fn fail_install_preparation(&self) -> RuntimeHostResult<()> {
        let mut admission = lock(
            &self.lifecycle_admission,
            HostLocation::FailInstallPreparation,
        )?;
        admission.closed = true;
        admission.preparing = false;
        Ok(())
    }
}
