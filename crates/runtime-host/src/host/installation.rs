// SPDX-License-Identifier: AGPL-3.0-only

//! Installation control belongs to the Host lifecycle owner. Only its two managed fact
//! keys are durable; admission, deadlines and tickets remain in this Host's memory.

use super::*;
use crate::codes::HostCode;
use crate::codes::HostLocation;
use actingcommand_contract::codes::ContractLocation;
use actingcommand_contract::outcome::Code;
use actingcommand_contract::{
    InstallHeldStartup, InstallTransitionAction, InstallTransitionPhase as Phase,
    InstallTransitionStatus, InstallTransitionTicket, RuntimeShutdownDecision,
    RuntimeShutdownTarget, SchedulingResumeSelfCheck,
};
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Instant;

const TRANSITION_KEY: &str = "host.install_transition";
const PAUSES_KEY: &str = "host.install_transition.pauses";
/// The longest a shutdown request waits for lifecycle work already in flight on other threads
/// to end before it is decided (`settle_for_shutdown`); well inside a client's 5 s I/O budget.
const SHUTDOWN_SETTLE_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) struct LifecycleAdmission {
    closed: bool,
    stopping: bool,
    preparing: bool,
    /// The held start's deadline on the host's `RuntimeClock` (monotonic milliseconds; test plan
    /// v2.2 section 3.10 H-1-rest, HOST-I2).
    held_deadline_ms: Option<u64>,
    active: std::collections::HashMap<thread::ThreadId, usize>,
    held: Option<InstallHeldStartup>,
    transition: Option<LiveTransition>,
    previous_pauses: Option<SchedulingPauseTable>,
    /// Test only: called each time a shutdown request starts to wait for lifecycle work in
    /// flight on other threads (`settle_for_shutdown`), with the admission held.
    #[cfg(test)]
    settle_observer: Option<Box<dyn Fn() + Send>>,
}

struct LiveTransition {
    status: InstallTransitionStatus,
    /// The phase's deadline on the host's `RuntimeClock` (monotonic milliseconds, HOST-I2).
    deadline_ms: u64,
    entered_event_id: Option<EventId>,
    /// The recorded held or release timeout that failed this transition; a held start ends
    /// with it (Workflow #381 A R5′, #672 ruling).
    timed_out: Option<RuntimeHostError>,
}

impl LifecycleAdmission {
    /// `clock` is the host's `RuntimeClock`, the one every installation deadline reads (test plan
    /// v2.2 section 3.10 H-1-rest, HOST-I2): `SystemRuntimeClock` in production, monotonic
    /// milliseconds of the same source as `Instant::now()`.
    pub(super) fn new(
        held: Option<InstallHeldStartup>,
        clock: &dyn RuntimeClock,
    ) -> RuntimeHostResult<Self> {
        if let Some(held) = &held {
            held.validate()
                .map_err(|error| install_error(error.code()))?;
        }
        let held_deadline_ms = match &held {
            Some(held) => Some(deadline_after(
                clock.sample()?.monotonic_ms,
                held.timeout_ms,
            )?),
            None => None,
        };
        Ok(Self {
            closed: true,
            stopping: false,
            preparing: held.is_none(),
            held_deadline_ms,
            active: std::collections::HashMap::new(),
            held,
            transition: None,
            previous_pauses: None,
            #[cfg(test)]
            settle_observer: None,
        })
    }
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

    /// Now on the clock every installation deadline reads: the host's `RuntimeClock`, monotonic
    /// milliseconds (test plan v2.2 section 3.10 H-1-rest, HOST-I2).
    fn install_now_ms(&self) -> RuntimeHostResult<u64> {
        Ok(self.clock.sample()?.monotonic_ms)
    }

    /// The wall-clock instant of `deadline_ms`, for the waits HOST-I2 leaves on the wall clock
    /// (the previous transition's verification, a held start's backend opens): now plus what
    /// remains of it on the host clock, the same instant in production.
    fn install_wall_deadline(&self, deadline_ms: u64) -> RuntimeHostResult<Instant> {
        let remaining_ms = deadline_ms.saturating_sub(self.install_now_ms()?);
        Ok(Instant::now() + Duration::from_millis(remaining_ms))
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
        let draining = admission.transition.as_ref().is_some_and(|transition| {
            matches!(transition.status.phase, Phase::Draining | Phase::Drained)
        });
        if admission.stopping || (admission.closed && !draining) {
            return Ok(None);
        }
        Ok(Some(self.count_work(&mut admission)))
    }

    /// Workflow #369-1 (review L1): whether the host is stopping or an install transition is
    /// draining it, so a long wait on the host-work thread ends at its next poll.
    pub(super) fn lifecycle_draining(&self) -> RuntimeHostResult<bool> {
        let admission = lock(
            &self.lifecycle_admission,
            HostLocation::ReadLifecycleDraining,
        )?;
        Ok(admission.stopping
            || admission.transition.as_ref().is_some_and(|transition| {
                matches!(transition.status.phase, Phase::Draining | Phase::Drained)
            }))
    }

    pub(super) fn request_is_lifecycle_control(operation: &RuntimeOperation) -> bool {
        matches!(
            operation,
            RuntimeOperation::RequestShutdown { .. }
                | RuntimeOperation::InstallTransition { .. }
                | RuntimeOperation::Health
                | RuntimeOperation::RuntimeFactSnapshot
                | RuntimeOperation::QueryEvents { .. }
                | RuntimeOperation::ReadMaterial { .. }
                | RuntimeOperation::DeclareGovernanceIdentity { .. }
                // Workflow #389 section 3.6: accepted while draining, held and preparing.
                | RuntimeOperation::BindChannel { .. }
                | RuntimeOperation::UpgradeState { .. }
                | RuntimeOperation::UpgradeStateStatus { .. }
        )
    }

    pub(super) fn begin_request(
        &self,
        operation: &RuntimeOperation,
        connection: ConnectionId,
    ) -> RuntimeHostResult<Option<LifecycleWork<'_>>> {
        if Self::request_is_lifecycle_control(operation) {
            return Ok(None);
        }
        if let RuntimeOperation::SafeReset { instance_alias, .. } = operation {
            let mut admission = lock(
                &self.lifecycle_admission,
                HostLocation::AdmitInstallResetCleanup,
            )?;
            if admission.stopping {
                return Ok(None);
            }
            if admission.closed && !admission.active.contains_key(&thread::current().id()) {
                let draining = admission.transition.as_ref().is_some_and(|transition| {
                    matches!(transition.status.phase, Phase::Draining | Phase::Drained)
                });
                if !draining
                    || !lock(
                        &self.scheduling_pause,
                        HostLocation::ReadInstallResetCleanup,
                    )?
                    .install_reset_continuation(instance_alias, connection)
                {
                    return Ok(None);
                }
            }
            return Ok(Some(self.count_work(&mut admission)));
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

    /// Workflow #369 §3.5 (review2 L1): idle means no lifecycle work (`active`, which counts
    /// every running instance worker), no holder and no waiting entry (the scheduler's own
    /// read), and no install cleanup pending. Workflow #411 A3: an entry the pause gate holds is
    /// no waiting entry (`nothing_held_or_waiting`). `request-shutdown`, `restart`, an install
    /// CommitShutdown and a drain's completion all decide by this test.
    fn installation_idle(&self, admission: &LifecycleAdmission) -> RuntimeHostResult<bool> {
        if !admission.active.is_empty() {
            return Ok(false);
        }
        self.nothing_held_or_waiting()
    }

    /// No holder and no waiting entry (the scheduler's own read), and no install cleanup
    /// pending: the part of `installation_idle` that lifecycle work in flight does not decide.
    ///
    /// Workflow #411 A3: a routine entry on an instance whose routine gate a scheduling pause
    /// holds (the global pause or the instance's own) is not a waiting entry: it does no work
    /// until a resume, and a shutdown disconnects it (`lease.queue_disconnected`) while the
    /// pause is kept and restored, so the next start enqueues it again. Only the pause half of
    /// the routine gate is read (`routine_gate` without the #403 resource fence): a routine
    /// entry a fence holds still waits, since the fence lifts without a resume. The pauses are
    /// read first, then the scheduler, as `pump` reads the gate; a resume done before this read
    /// leaves its entries eligible or granted, so they count.
    fn nothing_held_or_waiting(&self) -> RuntimeHostResult<bool> {
        let registered = lock(
            &self.registered_instances,
            HostLocation::ReadInstanceRegistry,
        )?
        .iter()
        .map(|(instance_id, instance)| (*instance_id, instance.instance_alias.clone()))
        .collect::<Vec<_>>();
        // Red proof M4: the #403 fence half of the routine gate is read with the pause half.
        let mut fenced = BTreeSet::new();
        for (instance_id, _) in &registered {
            if self.resource_fence_holds_routine(*instance_id)? {
                fenced.insert(*instance_id);
            }
        }
        // Red proof M1: an instance's own pause is not read.
        // Red proof M3: the global pause is not read.
        // Red proof M2: while no pause is held at all, the routine entries of an instance that
        // no fence holds are skipped as if a pause held them.
        let no_pause_held =
            !lock(&self.scheduling_pause, HostLocation::ReadRoutineGate)?.holds_pauses();
        Ok({
            let scheduler = lock(&self.scheduler, HostLocation::CheckInstallLeases)?;
            scheduler.active_tokens().is_empty()
                && scheduler.waiting_total(|instance_id| {
                    if fenced.contains(&instance_id) {
                        true
                    } else {
                        no_pause_held
                    }
                }) == 0
        } && !lock(&self.scheduling_pause, HostLocation::CheckInstallCleanup)?
            .install_cleanup_pending())
    }

    /// An idle Runtime has lifecycle work in flight now and then as well: the lease sweep every
    /// 50 ms, the performance tick, a closed connection's cleanup, a reply still being written
    /// (train 11 boot smoke, run 38560460358: `runtime_busy` right after `runtime_ready`). A
    /// shutdown request that finds such work on other threads, and no holder, no waiting entry
    /// (an entry the pause gate holds is none, Workflow #411 A3) and no install cleanup, lets it
    /// end, at most `SHUTDOWN_SETTLE_TIMEOUT`; it is then decided with the admission still
    /// held, so no new work starts in between. Work of the requesting thread itself cannot end
    /// while it waits, and every other refusal is answered at once.
    ///
    /// SHUT-FU (Workflow #378): an install CommitShutdown (`commit`, its ticket) is decided by
    /// the same idle test, so it waits the same way, and only while its transition is the live
    /// one, drained and before its deadline; any other state of the transition is answered at
    /// once as well.
    fn settle_for_shutdown<'a>(
        &'a self,
        mut admission: MutexGuard<'a, LifecycleAdmission>,
        target: RuntimeShutdownTarget,
        commit: Option<&InstallTransitionTicket>,
    ) -> RuntimeHostResult<MutexGuard<'a, LifecycleAdmission>> {
        let location = if commit.is_some() {
            ContractLocation::InstallTransition
        } else {
            ContractLocation::RequestRuntimeShutdown
        };
        let deadline = Instant::now() + SHUTDOWN_SETTLE_TIMEOUT;
        loop {
            let commit_refused = match commit {
                Some(ticket) => !Self::commit_drained(&admission, ticket, self.install_now_ms()?),
                None => false,
            };
            if admission.active.is_empty()
                || admission.active.contains_key(&thread::current().id())
                || target != self.shutdown_target
                || admission.stopping
                || admission.preparing
                || self.fatal.is_shutdown_requested()
                || commit_refused
                || !self.nothing_held_or_waiting()?
            {
                return Ok(admission);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(admission);
            }
            #[cfg(test)]
            {
                if let Some(observer) = &admission.settle_observer {
                    observer();
                }
            }
            admission = self
                .lifecycle_settled
                .wait_timeout(admission, remaining)
                .map_err(|_| lock_poison_error(location))?
                .0;
        }
    }

    /// The live transition is `ticket`'s, drained and before its deadline at `now_ms` on the host
    /// clock: nothing about the transition itself refuses its CommitShutdown.
    fn commit_drained(
        admission: &LifecycleAdmission,
        ticket: &InstallTransitionTicket,
        now_ms: u64,
    ) -> bool {
        admission.transition.as_ref().is_some_and(|transition| {
            transition.status.ticket == *ticket
                && transition.status.phase == Phase::Drained
                && now_ms < transition.deadline_ms
        })
    }

    /// Test only: `observer` is called each time a shutdown request starts to wait for lifecycle
    /// work in flight on other threads.
    #[cfg(test)]
    pub(super) fn observe_shutdown_settle_for_test(
        &self,
        observer: Box<dyn Fn() + Send>,
    ) -> RuntimeHostResult<()> {
        lock(
            &self.lifecycle_admission,
            crate::codes::test_spellings::TestLocation::ObserveShutdownSettleForTest,
        )?
        .settle_observer = Some(observer);
        Ok(())
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
        let mut admission = self.settle_for_shutdown(admission, target, None)?;
        self.shutdown_under_gate(request, target, &mut admission)
    }

    fn shutdown_under_gate(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        target: RuntimeShutdownTarget,
        admission: &mut LifecycleAdmission,
    ) -> Result<OperationSuccess, RequestFailure> {
        if let Some(error) = self.fatal.answer()? {
            return Err(error.into());
        }
        let decision = if target != self.shutdown_target {
            RuntimeShutdownDecision::OwnerMismatch
        } else if admission.stopping || self.fatal.is_shutdown_requested() {
            RuntimeShutdownDecision::AlreadyStopping
        } else if admission.preparing || !self.installation_idle(admission)? {
            RuntimeShutdownDecision::Busy
        } else {
            RuntimeShutdownDecision::Accepted
        };
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
            self.fatal.request_shutdown();
            Ok(OperationSuccess {
                state: RuntimeReceiptState::Admitted,
                terminal: Some(terminal(&event)),
                result: RuntimeResult::ShutdownAccepted { target },
                failure_outcome: None,
            })
        } else {
            Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::RuntimeShutdownDenied,
                    ContractLocation::RequestRuntimeShutdown,
                    match decision {
                        RuntimeShutdownDecision::Busy => RuntimeErrorCode::RuntimeBusy,
                        RuntimeShutdownDecision::OwnerMismatch => {
                            RuntimeErrorCode::RuntimeOwnerMismatch
                        }
                        _ => RuntimeErrorCode::RuntimeUnavailable,
                    },
                ),
                RuntimeReceiptState::Denied,
                Some(terminal(&event)),
            ))
        }
    }

    fn record_install_value(
        &self,
        key: &str,
        rows: Vec<BTreeMap<String, FactScalar>>,
    ) -> RuntimeHostResult<Option<EventId>> {
        let now = self.clock.sample()?.unix_ms;
        let previous = lock(&self.runtime_facts, HostLocation::ReadInstallFactClock)?
            .get(&RuntimeFactScope::Runtime, key)
            .map(|record| record.observed_at_unix_ms);
        let observed_at_unix_ms = match previous {
            Some(previous) => now.max(
                previous
                    .checked_add(1)
                    .ok_or_else(|| install_error(HostCode::InstallFactClockOverflow))?,
            ),
            None => now,
        };
        self.record_runtime_fact_with_event(RuntimeFactRecord {
            scope: RuntimeFactScope::Runtime,
            key: key.to_owned(),
            value: ContractFactValue::RecordList(rows),
            observed_at_unix_ms,
            source: OriginModule::Runtime,
            ttl_ms: None,
        })
        .map(|(_, event)| event)
    }

    fn record_install_status(
        &self,
        status: &InstallTransitionStatus,
    ) -> RuntimeHostResult<Option<EventId>> {
        self.record_install_status_with_cause(status, None)
    }

    /// The status fact of a transition. A failed or aborted one also records its
    /// `failure_code`, and a start that a latched failure stopped records that condition as
    /// `cause` and `cause_operation` (Workflow #381 A R3′).
    fn record_install_status_with_cause(
        &self,
        status: &InstallTransitionStatus,
        cause: Option<(&'static str, &'static str)>,
    ) -> RuntimeHostResult<Option<EventId>> {
        status
            .validate()
            .map_err(|error| install_error(error.code()))?;
        let mut row = BTreeMap::from([
            (
                "owner_epoch".to_owned(),
                identity_scalar(&status.ticket.target.owner_epoch)?,
            ),
            (
                "transition_id".to_owned(),
                FactScalar::String(status.ticket.transition_id.clone()),
            ),
            (
                "request_id".to_owned(),
                identity_scalar(&status.ticket.request_id)?,
            ),
            (
                "phase".to_owned(),
                FactScalar::String(status.phase.as_str().to_owned()),
            ),
            (
                "admission_closed".to_owned(),
                FactScalar::Boolean(status.admission_closed),
            ),
            (
                "timeout_ms".to_owned(),
                FactScalar::Integer(status.timeout_ms as i64),
            ),
        ]);
        if let Some(failure_code) = &status.failure_code {
            row.insert(
                "failure_code".to_owned(),
                FactScalar::String(failure_code.clone()),
            );
        }
        if let Some((code, operation)) = cause {
            row.insert("cause".to_owned(), FactScalar::String(code.to_owned()));
            row.insert(
                "cause_operation".to_owned(),
                FactScalar::String(operation.to_owned()),
            );
        }
        self.record_install_value(TRANSITION_KEY, vec![row])
    }

    fn install_refusal(
        &self,
        admission: &LifecycleAdmission,
        action: &InstallTransitionAction,
        code: impl Into<Code>,
    ) -> RequestFailure {
        let code: Code = code.into();
        let mut error = install_error(code);
        error.lifecycle.installation_effect = Some(EffectDisposition::NotPerformed);
        let stage = match action {
            InstallTransitionAction::Release { .. } => RuntimeLifecycleFailureStage::InstallRelease,
            InstallTransitionAction::Query { .. } => admission.transition.as_ref().map_or(
                RuntimeLifecycleFailureStage::InstallDrain,
                |transition| match transition.status.phase {
                    Phase::Held => RuntimeLifecycleFailureStage::InstallHeld,
                    Phase::Preparing | Phase::Released => {
                        RuntimeLifecycleFailureStage::InstallRelease
                    }
                    _ => RuntimeLifecycleFailureStage::InstallDrain,
                },
            ),
            _ => RuntimeLifecycleFailureStage::InstallDrain,
        };
        match self.append_lifecycle_failure(
            stage,
            RuntimeLifecycleFailure::Host(&error),
            EventLinksDraft::default(),
            admission
                .transition
                .as_ref()
                .and_then(|transition| transition.entered_event_id),
        ) {
            Ok(()) => RequestFailure::request(error, RuntimeReceiptState::Denied, None),
            Err(error) => RequestFailure::poison_without_terminal(error),
        }
    }

    pub(super) fn install_transition(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        target: RuntimeShutdownTarget,
        action: &InstallTransitionAction,
        connection: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let mut admission = lock(
            &self.lifecycle_admission,
            ContractLocation::InstallTransition,
        )?;
        if target != self.shutdown_target {
            return Err(self.install_refusal(&admission, action, HostCode::InstallOwnerMismatch));
        }
        // Workflow #381 A R2: the read-only Query needs no governance identity; while the new
        // owner prepares and before its deadline it reads Host memory only and appends nothing.
        if !matches!(action, InstallTransitionAction::Query { .. })
            && !lock(
                &self.governance_connections,
                HostLocation::AuthorizeInstallTransition,
            )?
            .contains(&connection)
        {
            return Err(self.install_refusal(
                &admission,
                action,
                HostCode::InstallIdentityRequired,
            ));
        }
        // SHUT-FU (Workflow #378): a CommitShutdown that meets lifecycle work in flight on other
        // threads, and nothing else that refuses it, lets that work end as a shutdown request
        // does; everything below is then decided with the admission still held.
        if let InstallTransitionAction::CommitShutdown { ticket } = action {
            admission = self.settle_for_shutdown(admission, ticket.target, Some(ticket))?;
        }
        self.tick_install_under_gate(&mut admission)?;
        if admission.stopping {
            return Err(self.install_refusal(&admission, action, HostCode::InstallOwnerStopping));
        }
        match action {
            InstallTransitionAction::BeginDrain {
                transition_id,
                timeout_ms,
            } => {
                if admission.closed
                    || admission.transition.as_ref().is_some_and(|transition| {
                        transition.status.ticket.transition_id == *transition_id
                    })
                {
                    return Err(self.install_refusal(
                        &admission,
                        action,
                        HostCode::InstallTransitionConflict,
                    ));
                }
                let status = InstallTransitionStatus {
                    ticket: InstallTransitionTicket {
                        target,
                        transition_id: transition_id.clone(),
                        request_id: request.request_id(),
                    },
                    phase: Phase::Draining,
                    admission_closed: true,
                    timeout_ms: *timeout_ms,
                    failure_code: None,
                };
                // Holding this same gate prevents a root entering between the ledger fact
                // and the memory barrier. A failed append closes all later admission.
                let entered_event_id = self.record_install_status(&status).map_err(|error| {
                    admission.closed = true;
                    RequestFailure::poison_without_terminal(error)
                })?;
                admission.closed = true;
                let deadline_ms = deadline_after(self.install_now_ms()?, *timeout_ms)?;
                admission.transition = Some(LiveTransition {
                    status,
                    deadline_ms,
                    entered_event_id,
                    timed_out: None,
                });
                self.tick_install_under_gate(&mut admission)?;
            }
            InstallTransitionAction::Query { transition_id } => {
                if admission.transition.as_ref().is_none_or(|transition| {
                    transition.status.ticket.transition_id != *transition_id
                }) {
                    return Err(self.install_refusal(
                        &admission,
                        action,
                        HostCode::InstallTransitionUnknown,
                    ));
                }
            }
            InstallTransitionAction::Abort { ticket }
            | InstallTransitionAction::CommitShutdown { ticket }
            | InstallTransitionAction::Release { ticket, .. } => {
                let Some(transition) = admission.transition.as_ref() else {
                    return Err(self.install_refusal(
                        &admission,
                        action,
                        HostCode::InstallTransitionUnknown,
                    ));
                };
                if transition.status.ticket != *ticket {
                    return Err(self.install_refusal(
                        &admission,
                        action,
                        HostCode::InstallTicketMismatch,
                    ));
                }
                let mut status = transition.status.clone();
                match action {
                    InstallTransitionAction::Abort { .. } => {
                        if !matches!(status.phase, Phase::Draining | Phase::Drained) {
                            return Err(self.install_refusal(
                                &admission,
                                action,
                                HostCode::InstallAbortStateInvalid,
                            ));
                        }
                        status.phase = Phase::Aborted;
                        status.admission_closed = false;
                        self.record_install_status(&status)?;
                        admission.closed = false;
                    }
                    InstallTransitionAction::CommitShutdown { .. } => {
                        if status.phase != Phase::Drained || !self.installation_idle(&admission)? {
                            return Err(self.install_refusal(
                                &admission,
                                action,
                                HostCode::InstallNotDrained,
                            ));
                        }
                        return self.shutdown_under_gate(request, ticket.target, &mut admission);
                    }
                    InstallTransitionAction::Release { timeout_ms, .. } => {
                        if status.phase != Phase::Held {
                            return Err(self.install_refusal(
                                &admission,
                                action,
                                HostCode::InstallReleaseStateInvalid,
                            ));
                        }
                        status.phase = Phase::Preparing;
                        status.timeout_ms = *timeout_ms;
                        self.record_install_status(&status)?;
                        admission.preparing = true;
                        let deadline_ms = deadline_after(self.install_now_ms()?, *timeout_ms)?;
                        admission
                            .transition
                            .as_mut()
                            .expect("validated transition")
                            .deadline_ms = deadline_ms;
                    }
                    _ => unreachable!(),
                }
                admission
                    .transition
                    .as_mut()
                    .expect("validated transition")
                    .status = status;
            }
        }
        let status = admission
            .transition
            .as_ref()
            .expect("successful transition action")
            .status
            .clone();
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::InstallTransition { status },
            failure_outcome: None,
        })
    }

    pub(super) fn invalid_install_request(
        &self,
        action: &InstallTransitionAction,
        code: impl Into<Code>,
    ) -> RuntimeHostResult<RequestFailure> {
        let admission = lock(
            &self.lifecycle_admission,
            HostLocation::RejectInstallRequest,
        )?;
        let failure = self.install_refusal(&admission, action, code);
        if failure.poison_runtime {
            Err(*failure.error)
        } else {
            Ok(failure)
        }
    }

    pub(super) fn tick_install(&self) -> RuntimeHostResult<()> {
        let mut admission = lock(
            &self.lifecycle_admission,
            HostLocation::AdvanceInstallTransition,
        )?;
        self.tick_install_under_gate(&mut admission)
    }

    fn tick_install_under_gate(&self, admission: &mut LifecycleAdmission) -> RuntimeHostResult<()> {
        let Some(transition) = admission.transition.as_ref() else {
            return Ok(());
        };
        if !matches!(
            transition.status.phase,
            Phase::Draining | Phase::Drained | Phase::Held | Phase::Preparing
        ) {
            return Ok(());
        }
        let mut status = transition.status.clone();
        let mut timed_out = None;
        if self.install_now_ms()? >= transition.deadline_ms {
            let (stage, code, reopen) = match status.phase {
                Phase::Draining | Phase::Drained => (
                    RuntimeLifecycleFailureStage::InstallDrain,
                    HostCode::DrainTimeout,
                    true,
                ),
                Phase::Held => (
                    RuntimeLifecycleFailureStage::InstallHeld,
                    HostCode::HeldTimeout,
                    false,
                ),
                _ => (
                    RuntimeLifecycleFailureStage::InstallRelease,
                    HostCode::ReleaseTimeout,
                    false,
                ),
            };
            let mut error = install_error(code);
            error.lifecycle.installation_effect = Some(if status.phase == Phase::Preparing {
                EffectDisposition::Indeterminate
            } else {
                EffectDisposition::Performed
            });
            self.append_lifecycle_failure(
                stage,
                RuntimeLifecycleFailure::Host(&error),
                EventLinksDraft::default(),
                transition.entered_event_id,
            )?;
            status.phase = if reopen {
                Phase::Aborted
            } else {
                Phase::Failed
            };
            status.admission_closed = !reopen;
            status.failure_code = Some(code.as_str().to_owned());
            self.record_install_status(&status)?;
            admission.closed = !reopen;
            // A drain timeout reopens the old owner; a held or release timeout ends the start.
            timed_out = (!reopen).then_some(error);
        } else if status.phase == Phase::Draining && self.installation_idle(admission)? {
            self.record_install_pauses(&status.ticket)?;
            status.phase = Phase::Drained;
            self.record_install_status(&status)?;
        }
        let live = admission.transition.as_mut().expect("live transition");
        live.status = status;
        if timed_out.is_some() {
            live.timed_out = timed_out;
        }
        Ok(())
    }

    /// Workflow #381 A R5′ (#672 ruling): the recorded held or release timeout of this start's
    /// transition, if one failed it. The start ends with that error, a top code the watchdog
    /// restarts.
    fn install_start_timed_out(admission: &LifecycleAdmission) -> Option<RuntimeHostError> {
        admission
            .transition
            .as_ref()
            .and_then(|transition| transition.timed_out.clone())
    }

    pub(super) fn initialize_installation(&self) -> RuntimeHostResult<()> {
        let mut admission = lock(
            &self.lifecycle_admission,
            HostLocation::InitializeInstallTransition,
        )?;
        let held = admission.held.clone();
        // Read only this exact predecessor before invalidating both keys on every new epoch.
        let previous = held
            .as_ref()
            .and_then(|held| held.previous.as_ref())
            .map(|ticket| {
                self.install_wall_deadline(admission.held_deadline_ms.expect("held deadline"))
                    .and_then(|deadline| self.recover_install_pauses(ticket, deadline))
            })
            .transpose();
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
        admission.previous_pauses = previous?;
        if let Some(held) = held {
            let status = InstallTransitionStatus {
                ticket: InstallTransitionTicket {
                    target: self.shutdown_target,
                    transition_id: held.transition_id,
                    request_id: held.request_id,
                },
                phase: Phase::Held,
                admission_closed: true,
                timeout_ms: held.timeout_ms,
                failure_code: None,
            };
            let entered_event_id = self.record_install_status(&status)?;
            admission.transition = Some(LiveTransition {
                status,
                deadline_ms: admission.held_deadline_ms.expect("held deadline"),
                entered_event_id,
                timed_out: None,
            });
        }
        Ok(())
    }

    /// The error that stops an install start, if anything stops it. A latched failure ends it
    /// as `install_startup_stopped` with that condition named (Workflow #381 A R3′), a halt a
    /// restart may fix. An accepted shutdown (`stopping`, or the shutdown flag) ends it as
    /// `install_startup_shut_down` (info), a requested stop: actingd ends `runtime_stopped` with
    /// it as the link, and the watchdog never restarts it (#672 review M1; #378 A6a, A7-R).
    fn install_start_stopped(&self, stopping: bool) -> RuntimeHostResult<Option<RuntimeHostError>> {
        if let Some(cause) = self.fatal.current()? {
            return Ok(Some(
                install_error(HostCode::InstallStartupStopped).with_latched_cause(&cause),
            ));
        }
        if stopping || self.fatal.is_shutdown_requested() {
            return Ok(Some(install_error(HostCode::InstallStartupShutDown)));
        }
        Ok(None)
    }

    pub(super) fn wait_install_release(&self) -> RuntimeHostResult<()> {
        loop {
            if let Some(stopped) = self.install_start_stopped(false)? {
                return Err(stopped);
            }
            let mut admission = lock(&self.lifecycle_admission, HostLocation::WaitInstallRelease)?;
            if admission.held.is_none()
                || admission
                    .transition
                    .as_ref()
                    .is_some_and(|transition| transition.status.phase == Phase::Preparing)
            {
                return Ok(());
            }
            // #672 ruling (R5′): the held deadline passes even when nobody polls, and the start
            // then ends under `held_timeout` instead of staying held.
            self.tick_install_under_gate(&mut admission)?;
            if let Some(timed_out) = Self::install_start_timed_out(&admission) {
                return Err(timed_out);
            }
            drop(admission);
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub(super) fn check_install_preparation(&self) -> RuntimeHostResult<()> {
        self.tick_install()?;
        let admission = lock(
            &self.lifecycle_admission,
            HostLocation::CheckInstallPreparation,
        )?;
        if let Some(stopped) = self.install_start_stopped(admission.stopping)? {
            return Err(stopped);
        }
        if admission.held.is_some()
            && admission
                .transition
                .as_ref()
                .is_none_or(|transition| transition.status.phase != Phase::Preparing)
        {
            // #672 ruling (R5′): a release that timed out ends the start under its own code.
            return Err(Self::install_start_timed_out(&admission)
                .unwrap_or_else(|| install_error(HostCode::InstallPreparationNotAuthorized)));
        }
        Ok(())
    }

    /// The release deadline of a held start that prepares, as the wall-clock instant its backend
    /// opens are bounded by (HOST-I2 leaves those on the wall clock).
    pub(super) fn install_preparation_deadline(&self) -> RuntimeHostResult<Option<Instant>> {
        let deadline_ms = {
            let admission = lock(
                &self.lifecycle_admission,
                HostLocation::ReadInstallPreparationDeadline,
            )?;
            admission
                .transition
                .as_ref()
                .filter(|_| admission.held.is_some() && admission.preparing)
                .map(|transition| transition.deadline_ms)
        };
        deadline_ms
            .map(|deadline_ms| self.install_wall_deadline(deadline_ms))
            .transpose()
    }

    /// INST-1 (Workflow #390, FCM D-01): an install start's verdict on the self-checks of its
    /// daemon-start preparation, given every instance it prepared (`checks`). Outside a held
    /// start nothing changes: a failed preparation has already left its instance unavailable.
    /// In a held start:
    /// - no prepared instance passed and at least one failed: the new build reached no device,
    ///   and the start ends `install_preparation_selfcheck_failed` as before;
    /// - otherwise each failed instance stays unavailable and is recorded once as
    ///   `install_preparation_instance_unavailable` (warning) at stage `install_release`, with
    ///   its self-check failure as the outcome's `caused_by` link (ledger revision 3), and the
    ///   start goes on with the instances that passed.
    pub(super) fn settle_install_selfchecks(
        &self,
        checks: &[(InstanceId, SchedulingResumeSelfCheck)],
    ) -> RuntimeHostResult<()> {
        let admission = lock(
            &self.lifecycle_admission,
            HostLocation::CheckInstallSelfcheck,
        )?;
        if admission.held.is_none() {
            return Ok(());
        }
        let failed = checks
            .iter()
            .filter(|(_, check)| {
                check.failure_code.is_some() || !check.capture.ok || !check.touch.ok
            })
            .collect::<Vec<_>>();
        if failed.is_empty() {
            return Ok(());
        }
        if failed.len() == checks.len() {
            let mut error = install_error(HostCode::InstallPreparationSelfcheckFailed);
            if let [(instance_id, _)] = failed.as_slice() {
                error.lifecycle.instance_id = Some(*instance_id);
            }
            return Err(error);
        }
        let entered_event_id = admission
            .transition
            .as_ref()
            .and_then(|transition| transition.entered_event_id);
        for (instance_id, check) in failed {
            // Ledger revision 3: the instance is the outcome's required value `instance_id`.
            let mut unavailable = install_error(HostCode::InstallPreparationInstanceUnavailable)
                .with_outcome_values(|outcome| {
                    outcome.instance_id(crate::failure_identity::identifier_text(instance_id))
                });
            unavailable.lifecycle.instance_id = Some(*instance_id);
            // Ledger revision 3 (#378 A3): the self-check failure is the `caused_by` link (it
            // was the `cause=<code>` token).
            if let Some(cause) = &check.failure_code {
                unavailable.lifecycle.caused_by =
                    Some(actingcommand_contract::outcome::Outcome::relayed(cause));
            }
            self.append_lifecycle_failure_with_severity(
                RuntimeLifecycleFailureStage::InstallRelease,
                RuntimeLifecycleFailure::Host(&unavailable),
                self.events
                    .system_links()?
                    .with_instance_id(self.events.issuer().issue_registered_instance(*instance_id)),
                entered_event_id,
                Some(EventSeverity::Warning),
            )?;
        }
        Ok(())
    }

    pub(super) fn finish_install_preparation(&self) -> RuntimeHostResult<()> {
        self.tick_install()?;
        let mut admission = lock(
            &self.lifecycle_admission,
            HostLocation::FinishInstallPreparation,
        )?;
        if let Some(stopped) = self.install_start_stopped(admission.stopping)? {
            return Err(stopped);
        }
        if let Some(transition) = &admission.transition {
            if transition.status.phase != Phase::Preparing {
                // #672 ruling (R5′): a release that timed out ends the start under its own code.
                return Err(transition
                    .timed_out
                    .clone()
                    .unwrap_or_else(|| install_error(HostCode::InstallPreparationNotAuthorized)));
            }
            let mut status = transition.status.clone();
            status.phase = Phase::Released;
            status.admission_closed = false;
            self.record_install_status(&status)?;
            admission
                .transition
                .as_mut()
                .expect("preparing transition")
                .status = status;
        }
        admission.closed = false;
        admission.preparing = false;
        Ok(())
    }

    pub(super) fn restore_install_pauses(&self) -> RuntimeHostResult<()> {
        let mut admission = lock(
            &self.lifecycle_admission,
            HostLocation::RestoreInstallPauses,
        )?;
        if let Some(pauses) = admission.previous_pauses.take() {
            pauses.validate_install_scopes(&*lock(
                &self.registered_instances,
                HostLocation::ValidateInstallPauseScopes,
            )?)?;
            *lock(&self.scheduling_pause, HostLocation::RestoreInstallPauses)? = pauses;
        }
        Ok(())
    }

    pub(super) fn fail_install_preparation(
        &self,
        error: &RuntimeHostError,
    ) -> RuntimeHostResult<()> {
        let mut admission = lock(
            &self.lifecycle_admission,
            HostLocation::FailInstallPreparation,
        )?;
        let preparing = admission.preparing;
        admission.closed = true;
        admission.preparing = false;
        if admission.held.is_none() {
            return Ok(());
        }
        let stage = if preparing {
            RuntimeLifecycleFailureStage::InstallRelease
        } else {
            RuntimeLifecycleFailureStage::InstallHeld
        };
        let mut failure = error.clone();
        failure.lifecycle.failure_stage = Some(stage.location().location());
        if let Some(complete) = &mut failure.lifecycle.complete_failure {
            complete.primary.lifecycle.failure_stage = Some(stage.location().location());
        }
        self.append_lifecycle_failure(
            stage,
            RuntimeLifecycleFailure::Host(&failure),
            EventLinksDraft::default(),
            admission
                .transition
                .as_ref()
                .and_then(|transition| transition.entered_event_id),
        )?;
        if let Some(transition) = &mut admission.transition {
            let mut status = transition.status.clone();
            status.phase = Phase::Failed;
            status.admission_closed = true;
            status
                .failure_code
                .get_or_insert_with(|| error.code().to_owned());
            let cause = error
                .latched_cause()
                .filter(|_| status.failure_code.as_deref() == Some(error.code()));
            self.record_install_status_with_cause(&status, cause)?;
            transition.status = status;
        }
        Ok(())
    }

    fn record_install_pauses(&self, ticket: &InstallTransitionTicket) -> RuntimeHostResult<()> {
        let instances = lock(
            &self.registered_instances,
            HostLocation::ReadInstallPauseInstances,
        )?;
        let pauses = lock(&self.scheduling_pause, HostLocation::ReadInstallPauses)?;
        let mut rows = vec![pause_identity(ticket, "transition")?];
        if let Some(pause) = pauses.global_state() {
            let mut row = pause_identity(ticket, "global")?;
            pause_fields(
                &mut row,
                pause.revision,
                &pause.pause_reason,
                pause.since_unix_ms,
            );
            rows.push(row);
        }
        for instance in instances.values() {
            if let Some(pause) = pauses.instance_state(&instance.instance_alias) {
                let mut row = pause_identity(ticket, "instance")?;
                row.insert(
                    "instance_alias".to_owned(),
                    FactScalar::String(instance.instance_alias.clone()),
                );
                row.insert(
                    "stage".to_owned(),
                    FactScalar::String(
                        match pause.stage {
                            actingcommand_contract::InstancePauseStage::Draining => "draining",
                            actingcommand_contract::InstancePauseStage::Paused => "paused",
                            actingcommand_contract::InstancePauseStage::Released => "released",
                        }
                        .to_owned(),
                    ),
                );
                pause_fields(
                    &mut row,
                    pause.revision,
                    &pause.pause_reason,
                    pause.since_unix_ms,
                );
                rows.push(row);
            }
        }
        drop(pauses);
        drop(instances);
        self.record_install_value(PAUSES_KEY, rows).map(|_| ())
    }

    fn recover_install_pauses(
        &self,
        ticket: &InstallTransitionTicket,
        deadline: Instant,
    ) -> RuntimeHostResult<SchedulingPauseTable> {
        if ticket.target.owner_epoch == self.owner_epoch {
            return Err(install_error(HostCode::InstallPreviousOwnerInvalid));
        }
        let store = lock(&self.runtime_facts, HostLocation::ReadInstallPreviousFacts)?;
        let transition = store
            .get(&RuntimeFactScope::Runtime, TRANSITION_KEY)
            .filter(|record| record.source == OriginModule::Runtime)
            .ok_or_else(|| install_error(HostCode::InstallPreviousTransitionMissing))?
            .clone();
        let pauses = store
            .get(&RuntimeFactScope::Runtime, PAUSES_KEY)
            .filter(|record| record.source == OriginModule::Runtime)
            .ok_or_else(|| install_error(HostCode::InstallPreviousPausesMissing))?
            .clone();
        drop(store);
        let ContractFactValue::RecordList(rows) = &transition.value else {
            return Err(install_error(HostCode::InstallPreviousTransitionInvalid));
        };
        if rows.len() != 1
            || !matching_ticket(&rows[0], ticket)?
            || row_string(&rows[0], "phase")? != "drained"
            || rows[0].get("admission_closed") != Some(&FactScalar::Boolean(true))
        {
            return Err(install_error(HostCode::InstallPreviousTransitionInvalid));
        }
        // Bound recovery by the same Host control maximum; a missing or unread proof never
        // restores a pause. Pages are streamed, and only the exact record supplies a position.
        let through = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error(HostLocation::VerifyInstallPrevious))?;
        let mut after = 0;
        let mut drained_at = None;
        loop {
            if Instant::now() >= deadline {
                return Err(install_error(HostCode::InstallPreviousVerificationTimeout));
            }
            let page = self
                .ledger
                .query_page(
                    EventQuery {
                        from_timestamp_unix_ms: Some(ticket.target.started_at_unix_ms),
                        event_type: Some(EventType::RuntimeFactRecorded),
                        ..EventQuery::default()
                    },
                    after,
                    through,
                    128,
                )
                .map_err(|_| ledger_error(HostLocation::VerifyInstallPrevious))?;
            for event in &page {
                if matches!(event.payload(), EventPayload::Runtime(RuntimePayload::FactRecorded(payload)) if payload.record() == &transition)
                {
                    drained_at = Some(event.sequence());
                }
            }
            if let Some(event) = page.last() {
                after = event.sequence();
            }
            if page.len() < 128 {
                break;
            }
        }
        let mut after =
            drained_at.ok_or_else(|| install_error(HostCode::InstallPreviousDrainUnverified))?;
        let mut closed = false;
        loop {
            if Instant::now() >= deadline {
                return Err(install_error(HostCode::InstallPreviousVerificationTimeout));
            }
            let page = self
                .ledger
                .query_page(
                    EventQuery {
                        event_type: Some(EventType::RuntimeLifecycleObserved),
                        ..EventQuery::default()
                    },
                    after,
                    through,
                    128,
                )
                .map_err(|_| ledger_error(HostLocation::VerifyInstallPreviousShutdown))?;
            for event in &page {
                if matches!(event.payload(), EventPayload::Runtime(RuntimePayload::LifecycleObserved(payload))
                    if payload.owner_epoch() == ticket.target.owner_epoch
                        && payload.phase() == RuntimeLifecyclePhase::ShutdownRequest { target: ticket.target, decision: RuntimeShutdownDecision::Accepted })
                {
                    closed = true;
                }
            }
            if closed {
                break;
            }
            if let Some(event) = page.last() {
                after = event.sequence();
            }
            if page.len() < 128 {
                break;
            }
        }
        if !closed {
            return Err(install_error(HostCode::InstallPreviousShutdownUnverified));
        }
        let ContractFactValue::RecordList(rows) = &pauses.value else {
            return Err(install_error(HostCode::InstallPreviousPausesInvalid));
        };
        let mut global = None;
        let mut instances = BTreeMap::new();
        let mut identity = false;
        for row in rows {
            if !matching_ticket(row, ticket)? {
                return Err(install_error(
                    HostCode::InstallPreviousPauseIdentityMismatch,
                ));
            }
            let scope = row_string(row, "scope")?;
            if scope == "transition" {
                if identity {
                    return Err(install_error(HostCode::InstallPreviousPausesInvalid));
                }
                identity = true;
                continue;
            }
            let revision = row_string(row, "revision")?
                .parse::<u64>()
                .map_err(|_| install_error(HostCode::InstallPreviousPauseRevisionInvalid))?;
            let pause_reason = actingcommand_contract::SchedulingPauseReason::new(row_string(
                row,
                "pause_reason",
            )?)
            .map_err(|error| install_error(error.code()))?;
            let since_unix_ms = match row.get("since_unix_ms") {
                Some(FactScalar::TimestampMs(value)) => *value,
                _ => {
                    return Err(install_error(HostCode::InstallPreviousPauseTimeInvalid));
                }
            };
            match scope {
                "global" => {
                    let pause = actingcommand_contract::SchedulingPauseState {
                        revision,
                        pause_reason,
                        since_unix_ms,
                    };
                    pause
                        .validate()
                        .map_err(|error| install_error(error.code()))?;
                    if global.replace(pause).is_some() {
                        return Err(install_error(HostCode::InstallPreviousPausesInvalid));
                    }
                }
                "instance" => {
                    let stage = match row_string(row, "stage")? {
                        "paused" => actingcommand_contract::InstancePauseStage::Paused,
                        "released" => actingcommand_contract::InstancePauseStage::Released,
                        _ => {
                            return Err(install_error(HostCode::InstallPreviousPauseNotSettled));
                        }
                    };
                    let pause = actingcommand_contract::InstancePauseState {
                        revision,
                        pause_reason,
                        since_unix_ms,
                        stage,
                    };
                    pause
                        .validate()
                        .map_err(|error| install_error(error.code()))?;
                    if instances
                        .insert(row_string(row, "instance_alias")?.to_owned(), pause)
                        .is_some()
                    {
                        return Err(install_error(HostCode::InstallPreviousPausesInvalid));
                    }
                }
                _ => {
                    return Err(install_error(HostCode::InstallPreviousPauseScopeInvalid));
                }
            }
        }
        if !identity {
            return Err(install_error(HostCode::InstallPreviousPauseIdentityMissing));
        }
        Ok(SchedulingPauseTable::from_install_pauses(global, instances))
    }
}

fn pause_identity(
    ticket: &InstallTransitionTicket,
    scope: &str,
) -> RuntimeHostResult<BTreeMap<String, FactScalar>> {
    Ok(BTreeMap::from([
        (
            "owner_epoch".to_owned(),
            identity_scalar(&ticket.target.owner_epoch)?,
        ),
        (
            "transition_id".to_owned(),
            FactScalar::String(ticket.transition_id.clone()),
        ),
        (
            "request_id".to_owned(),
            identity_scalar(&ticket.request_id)?,
        ),
        ("scope".to_owned(), FactScalar::String(scope.to_owned())),
    ]))
}

fn pause_fields(
    row: &mut BTreeMap<String, FactScalar>,
    revision: u64,
    reason: &actingcommand_contract::SchedulingPauseReason,
    since: u64,
) {
    row.insert(
        "revision".to_owned(),
        FactScalar::String(revision.to_string()),
    );
    row.insert(
        "pause_reason".to_owned(),
        FactScalar::String(reason.as_str().to_owned()),
    );
    row.insert("since_unix_ms".to_owned(), FactScalar::TimestampMs(since));
}

fn row_string<'a>(row: &'a BTreeMap<String, FactScalar>, key: &str) -> RuntimeHostResult<&'a str> {
    match row.get(key) {
        Some(FactScalar::String(value)) => Ok(value),
        _ => Err(install_error(HostCode::InstallFactFieldInvalid)),
    }
}

fn matching_ticket(
    row: &BTreeMap<String, FactScalar>,
    ticket: &InstallTransitionTicket,
) -> RuntimeHostResult<bool> {
    Ok(
        row.get("owner_epoch") == Some(&identity_scalar(&ticket.target.owner_epoch)?)
            && row.get("transition_id") == Some(&FactScalar::String(ticket.transition_id.clone()))
            && row.get("request_id") == Some(&identity_scalar(&ticket.request_id)?),
    )
}

fn identity_scalar(value: &impl serde::Serialize) -> RuntimeHostResult<FactScalar> {
    match serde_json::to_value(value)
        .map_err(|_| install_error(HostCode::InstallIdentityEncodeFailed))?
    {
        serde_json::Value::String(value) => Ok(FactScalar::String(value)),
        _ => Err(install_error(HostCode::InstallIdentityEncodeFailed)),
    }
}
