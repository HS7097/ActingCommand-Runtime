// SPDX-License-Identifier: AGPL-3.0-only

//! Installation control belongs to the Host lifecycle owner. Only its two managed fact
//! keys are durable; admission, deadlines and tickets remain in this Host's memory.

use super::*;
use crate::codes::HostCode;
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

pub(super) struct LifecycleAdmission {
    closed: bool,
    stopping: bool,
    preparing: bool,
    held_deadline: Option<Instant>,
    active: std::collections::HashMap<thread::ThreadId, usize>,
    held: Option<InstallHeldStartup>,
    transition: Option<LiveTransition>,
    previous_pauses: Option<SchedulingPauseTable>,
}

struct LiveTransition {
    status: InstallTransitionStatus,
    deadline: Instant,
    entered_event_id: Option<EventId>,
}

impl LifecycleAdmission {
    pub(super) fn new(held: Option<InstallHeldStartup>) -> RuntimeHostResult<Self> {
        if let Some(held) = &held {
            held.validate()
                .map_err(|error| install_error(error.code()))?;
        }
        Ok(Self {
            closed: true,
            stopping: false,
            preparing: held.is_none(),
            held_deadline: held
                .as_ref()
                .map(|held| Instant::now() + Duration::from_millis(held.timeout_ms)),
            active: std::collections::HashMap::new(),
            held,
            transition: None,
            previous_pauses: None,
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
            }
        }
    }
}

fn install_error(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(code, "install_transition", RuntimeErrorCode::InvalidRequest)
}

fn prepared<T>(value: &OnceLock<T>) -> RuntimeHostResult<&T> {
    value.get().ok_or_else(|| {
        RuntimeHostError::fatal(
            "runtime_component_not_prepared",
            "access_prepared_runtime",
            RuntimeErrorCode::RuntimeFatal,
        )
    })
}

pub(super) fn set_prepared<T>(slot: &OnceLock<T>, value: T) -> RuntimeHostResult<()> {
    slot.set(value).map_err(|_| {
        RuntimeHostError::fatal(
            "runtime_component_already_prepared",
            "prepare_runtime",
            RuntimeErrorCode::RuntimeFatal,
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

    fn count_work<'a>(&'a self, admission: &mut LifecycleAdmission) -> LifecycleWork<'a> {
        *admission.active.entry(thread::current().id()).or_default() += 1;
        LifecycleWork {
            shared: self,
            _same_thread: PhantomData,
        }
    }

    pub(super) fn begin_work(&self) -> RuntimeHostResult<Option<LifecycleWork<'_>>> {
        let mut admission = lock(&self.lifecycle_admission, "admit_runtime_work")?;
        if admission.stopping
            || (admission.closed && !admission.active.contains_key(&thread::current().id()))
        {
            return Ok(None);
        }
        Ok(Some(self.count_work(&mut admission)))
    }

    /// Cleanup is part of admitted work even when shutdown has already closed the root gate.
    pub(super) fn work_guard(&self) -> RuntimeHostResult<LifecycleWork<'_>> {
        let mut admission = lock(&self.lifecycle_admission, "admit_runtime_cleanup")?;
        Ok(self.count_work(&mut admission))
    }

    pub(super) fn begin_continuation(&self) -> RuntimeHostResult<Option<LifecycleWork<'_>>> {
        let mut admission = lock(&self.lifecycle_admission, "admit_runtime_continuation")?;
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
        let admission = lock(&self.lifecycle_admission, "read_lifecycle_draining")?;
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
            let mut admission = lock(&self.lifecycle_admission, "admit_install_reset_cleanup")?;
            if admission.stopping {
                return Ok(None);
            }
            if admission.closed && !admission.active.contains_key(&thread::current().id()) {
                let draining = admission.transition.as_ref().is_some_and(|transition| {
                    matches!(transition.status.phase, Phase::Draining | Phase::Drained)
                });
                if !draining
                    || !lock(&self.scheduling_pause, "read_install_reset_cleanup")?
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

    fn installation_idle(&self, admission: &LifecycleAdmission) -> RuntimeHostResult<bool> {
        if !admission.active.is_empty() {
            return Ok(false);
        }
        Ok(lock(&self.scheduler, "check_install_leases")?
            .active_tokens()
            .is_empty()
            && lock(&self.queued_requests, "check_install_queue")?.is_empty()
            && lock(&self.pending_host_work, "check_install_host_work")?.is_empty()
            && !lock(&self.recovery_ladders, "check_install_recovery_work")?
                .values()
                .any(recovery_ladder::RecoveryLadderWindow::is_running)
            && !lock(&self.scheduling_pause, "check_install_cleanup")?.install_cleanup_pending())
    }

    pub(super) fn request_shutdown(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        target: RuntimeShutdownTarget,
    ) -> Result<OperationSuccess, RequestFailure> {
        let mut admission = lock(&self.lifecycle_admission, "request_runtime_shutdown")?;
        self.shutdown_under_gate(request, target, &mut admission)
    }

    fn shutdown_under_gate(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        target: RuntimeShutdownTarget,
        admission: &mut LifecycleAdmission,
    ) -> Result<OperationSuccess, RequestFailure> {
        if let Some(error) = self.fatal.current()? {
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
            })
        } else {
            Err(RequestFailure::request(
                RuntimeHostError::request(
                    "runtime_shutdown_denied",
                    "request_runtime_shutdown",
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
        let previous = lock(&self.runtime_facts, "read_install_fact_clock")?
            .get(&RuntimeFactScope::Runtime, key)
            .map(|record| record.observed_at_unix_ms);
        let observed_at_unix_ms = match previous {
            Some(previous) => now.max(
                previous
                    .checked_add(1)
                    .ok_or_else(|| install_error("install_fact_clock_overflow"))?,
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
        status
            .validate()
            .map_err(|error| install_error(error.code()))?;
        self.record_install_value(
            TRANSITION_KEY,
            vec![BTreeMap::from([
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
            ])],
        )
    }

    fn install_refusal(
        &self,
        admission: &LifecycleAdmission,
        action: &InstallTransitionAction,
        code: &'static str,
    ) -> RequestFailure {
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
        let mut admission = lock(&self.lifecycle_admission, "install_transition")?;
        if target != self.shutdown_target {
            return Err(self.install_refusal(&admission, action, "install_owner_mismatch"));
        }
        if !lock(&self.governance_connections, "authorize_install_transition")?
            .contains(&connection)
        {
            return Err(self.install_refusal(&admission, action, "install_identity_required"));
        }
        self.tick_install_under_gate(&mut admission)?;
        if admission.stopping {
            return Err(self.install_refusal(&admission, action, "install_owner_stopping"));
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
                        "install_transition_conflict",
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
                admission.transition = Some(LiveTransition {
                    status,
                    deadline: Instant::now() + Duration::from_millis(*timeout_ms),
                    entered_event_id,
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
                        "install_transition_unknown",
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
                        "install_transition_unknown",
                    ));
                };
                if transition.status.ticket != *ticket {
                    return Err(self.install_refusal(
                        &admission,
                        action,
                        "install_ticket_mismatch",
                    ));
                }
                let mut status = transition.status.clone();
                match action {
                    InstallTransitionAction::Abort { .. } => {
                        if !matches!(status.phase, Phase::Draining | Phase::Drained) {
                            return Err(self.install_refusal(
                                &admission,
                                action,
                                "install_abort_state_invalid",
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
                                "install_not_drained",
                            ));
                        }
                        return self.shutdown_under_gate(request, ticket.target, &mut admission);
                    }
                    InstallTransitionAction::Release { timeout_ms, .. } => {
                        if status.phase != Phase::Held {
                            return Err(self.install_refusal(
                                &admission,
                                action,
                                "install_release_state_invalid",
                            ));
                        }
                        status.phase = Phase::Preparing;
                        status.timeout_ms = *timeout_ms;
                        self.record_install_status(&status)?;
                        admission.preparing = true;
                        admission
                            .transition
                            .as_mut()
                            .expect("validated transition")
                            .deadline = Instant::now() + Duration::from_millis(*timeout_ms);
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
        })
    }

    pub(super) fn invalid_install_request(
        &self,
        action: &InstallTransitionAction,
        code: &'static str,
    ) -> RuntimeHostResult<RequestFailure> {
        let admission = lock(&self.lifecycle_admission, "reject_install_request")?;
        let failure = self.install_refusal(&admission, action, code);
        if failure.poison_runtime {
            Err(*failure.error)
        } else {
            Ok(failure)
        }
    }

    pub(super) fn tick_install(&self) -> RuntimeHostResult<()> {
        let mut admission = lock(&self.lifecycle_admission, "advance_install_transition")?;
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
        if Instant::now() >= transition.deadline {
            let (stage, code, reopen) = match status.phase {
                Phase::Draining | Phase::Drained => (
                    RuntimeLifecycleFailureStage::InstallDrain,
                    "drain_timeout",
                    true,
                ),
                Phase::Held => (
                    RuntimeLifecycleFailureStage::InstallHeld,
                    HostCode::HeldTimeout.as_str(),
                    false,
                ),
                _ => (
                    RuntimeLifecycleFailureStage::InstallRelease,
                    HostCode::ReleaseTimeout.as_str(),
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
            status.failure_code = Some(code.to_owned());
            self.record_install_status(&status)?;
            admission.closed = !reopen;
        } else if status.phase == Phase::Draining && self.installation_idle(admission)? {
            self.record_install_pauses(&status.ticket)?;
            status.phase = Phase::Drained;
            self.record_install_status(&status)?;
        }
        admission
            .transition
            .as_mut()
            .expect("live transition")
            .status = status;
        Ok(())
    }

    pub(super) fn initialize_installation(&self) -> RuntimeHostResult<()> {
        let mut admission = lock(&self.lifecycle_admission, "initialize_install_transition")?;
        let held = admission.held.clone();
        // Read only this exact predecessor before invalidating both keys on every new epoch.
        let previous = held
            .as_ref()
            .and_then(|held| held.previous.as_ref())
            .map(|ticket| {
                self.recover_install_pauses(ticket, admission.held_deadline.expect("held deadline"))
            })
            .transpose();
        for key in [TRANSITION_KEY, PAUSES_KEY] {
            let present = lock(&self.runtime_facts, "read_old_install_transition")?
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
                deadline: admission.held_deadline.expect("held deadline"),
                entered_event_id,
            });
        }
        Ok(())
    }

    pub(super) fn wait_install_release(&self) -> RuntimeHostResult<()> {
        loop {
            if self.fatal.is_shutdown_requested() {
                return Err(install_error(HostCode::InstallStartupStopped.as_str()));
            }
            if let Some(error) = self.fatal.current()? {
                return Err(error);
            }
            let admission = lock(&self.lifecycle_admission, "wait_install_release")?;
            if admission.held.is_none()
                || admission
                    .transition
                    .as_ref()
                    .is_some_and(|transition| transition.status.phase == Phase::Preparing)
            {
                return Ok(());
            }
            drop(admission);
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub(super) fn check_install_preparation(&self) -> RuntimeHostResult<()> {
        self.tick_install()?;
        let admission = lock(&self.lifecycle_admission, "check_install_preparation")?;
        if admission.stopping || self.fatal.is_shutdown_requested() {
            return Err(install_error(HostCode::InstallStartupStopped.as_str()));
        }
        if admission.held.is_some()
            && admission
                .transition
                .as_ref()
                .is_none_or(|transition| transition.status.phase != Phase::Preparing)
        {
            return Err(install_error("install_preparation_not_authorized"));
        }
        Ok(())
    }

    pub(super) fn install_preparation_deadline(&self) -> RuntimeHostResult<Option<Instant>> {
        let admission = lock(
            &self.lifecycle_admission,
            "read_install_preparation_deadline",
        )?;
        Ok(admission
            .transition
            .as_ref()
            .filter(|_| admission.held.is_some() && admission.preparing)
            .map(|transition| transition.deadline))
    }

    pub(super) fn require_install_selfcheck(
        &self,
        instance_id: InstanceId,
        check: &SchedulingResumeSelfCheck,
    ) -> RuntimeHostResult<()> {
        let admission = lock(&self.lifecycle_admission, "check_install_selfcheck")?;
        if admission.held.is_some()
            && (check.failure_code.is_some() || !check.capture.ok || !check.touch.ok)
        {
            let mut error = install_error("install_preparation_selfcheck_failed");
            error.lifecycle.instance_id = Some(instance_id);
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn finish_install_preparation(&self) -> RuntimeHostResult<()> {
        self.tick_install()?;
        let mut admission = lock(&self.lifecycle_admission, "finish_install_preparation")?;
        if admission.stopping {
            return Err(install_error(HostCode::InstallStartupStopped.as_str()));
        }
        if let Some(transition) = &admission.transition {
            if transition.status.phase != Phase::Preparing {
                return Err(install_error("install_preparation_not_authorized"));
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
        let mut admission = lock(&self.lifecycle_admission, "restore_install_pauses")?;
        if let Some(pauses) = admission.previous_pauses.take() {
            pauses.validate_install_scopes(&*lock(
                &self.registered_instances,
                "validate_install_pause_scopes",
            )?)?;
            *lock(&self.scheduling_pause, "restore_install_pauses")? = pauses;
        }
        Ok(())
    }

    pub(super) fn fail_install_preparation(
        &self,
        error: &RuntimeHostError,
    ) -> RuntimeHostResult<()> {
        let mut admission = lock(&self.lifecycle_admission, "fail_install_preparation")?;
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
        failure.lifecycle.failure_stage = Some(stage.as_str());
        if let Some(complete) = &mut failure.lifecycle.complete_failure {
            complete.primary.lifecycle.failure_stage = Some(stage.as_str());
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
            self.record_install_status(&status)?;
            transition.status = status;
        }
        Ok(())
    }

    fn record_install_pauses(&self, ticket: &InstallTransitionTicket) -> RuntimeHostResult<()> {
        let instances = lock(&self.registered_instances, "read_install_pause_instances")?;
        let pauses = lock(&self.scheduling_pause, "read_install_pauses")?;
        let mut rows = vec![pause_identity(ticket, "transition")?];
        if let Some(pause) = pauses.global_state() {
            let mut row = pause_identity(ticket, "global")?;
            pause_fields(
                &mut row,
                pause.revision,
                &pause.reason_code,
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
                    &pause.reason_code,
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
            return Err(install_error("install_previous_owner_invalid"));
        }
        let store = lock(&self.runtime_facts, "read_install_previous_facts")?;
        let transition = store
            .get(&RuntimeFactScope::Runtime, TRANSITION_KEY)
            .filter(|record| record.source == OriginModule::Runtime)
            .ok_or_else(|| install_error("install_previous_transition_missing"))?
            .clone();
        let pauses = store
            .get(&RuntimeFactScope::Runtime, PAUSES_KEY)
            .filter(|record| record.source == OriginModule::Runtime)
            .ok_or_else(|| install_error("install_previous_pauses_missing"))?
            .clone();
        drop(store);
        let ContractFactValue::RecordList(rows) = &transition.value else {
            return Err(install_error("install_previous_transition_invalid"));
        };
        if rows.len() != 1
            || !matching_ticket(&rows[0], ticket)?
            || row_string(&rows[0], "phase")? != "drained"
            || rows[0].get("admission_closed") != Some(&FactScalar::Boolean(true))
        {
            return Err(install_error("install_previous_transition_invalid"));
        }
        // Bound recovery by the same Host control maximum; a missing or unread proof never
        // restores a pause. Pages are streamed, and only the exact record supplies a position.
        let through = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error("verify_install_previous"))?;
        let mut after = 0;
        let mut drained_at = None;
        loop {
            if Instant::now() >= deadline {
                return Err(install_error("install_previous_verification_timeout"));
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
                .map_err(|_| ledger_error("verify_install_previous"))?;
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
            drained_at.ok_or_else(|| install_error("install_previous_drain_unverified"))?;
        let mut closed = false;
        loop {
            if Instant::now() >= deadline {
                return Err(install_error("install_previous_verification_timeout"));
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
                .map_err(|_| ledger_error("verify_install_previous_shutdown"))?;
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
            return Err(install_error("install_previous_shutdown_unverified"));
        }
        let ContractFactValue::RecordList(rows) = &pauses.value else {
            return Err(install_error("install_previous_pauses_invalid"));
        };
        let mut global = None;
        let mut instances = BTreeMap::new();
        let mut identity = false;
        for row in rows {
            if !matching_ticket(row, ticket)? {
                return Err(install_error("install_previous_pause_identity_mismatch"));
            }
            let scope = row_string(row, "scope")?;
            if scope == "transition" {
                if identity {
                    return Err(install_error("install_previous_pauses_invalid"));
                }
                identity = true;
                continue;
            }
            let revision = row_string(row, "revision")?
                .parse::<u64>()
                .map_err(|_| install_error("install_previous_pause_revision_invalid"))?;
            let reason_code = row_string(row, "reason_code")?.to_owned();
            let since_unix_ms = match row.get("since_unix_ms") {
                Some(FactScalar::TimestampMs(value)) => *value,
                _ => return Err(install_error("install_previous_pause_time_invalid")),
            };
            match scope {
                "global" => {
                    let pause = actingcommand_contract::SchedulingPauseState {
                        revision,
                        reason_code,
                        since_unix_ms,
                    };
                    pause
                        .validate()
                        .map_err(|error| install_error(error.code()))?;
                    if global.replace(pause).is_some() {
                        return Err(install_error("install_previous_pauses_invalid"));
                    }
                }
                "instance" => {
                    let stage = match row_string(row, "stage")? {
                        "paused" => actingcommand_contract::InstancePauseStage::Paused,
                        "released" => actingcommand_contract::InstancePauseStage::Released,
                        _ => return Err(install_error("install_previous_pause_not_settled")),
                    };
                    let pause = actingcommand_contract::InstancePauseState {
                        revision,
                        reason_code,
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
                        return Err(install_error("install_previous_pauses_invalid"));
                    }
                }
                _ => return Err(install_error("install_previous_pause_scope_invalid")),
            }
        }
        if !identity {
            return Err(install_error("install_previous_pause_identity_missing"));
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

fn pause_fields(row: &mut BTreeMap<String, FactScalar>, revision: u64, reason: &str, since: u64) {
    row.insert(
        "revision".to_owned(),
        FactScalar::String(revision.to_string()),
    );
    row.insert(
        "reason_code".to_owned(),
        FactScalar::String(reason.to_owned()),
    );
    row.insert("since_unix_ms".to_owned(), FactScalar::TimestampMs(since));
}

fn row_string<'a>(row: &'a BTreeMap<String, FactScalar>, key: &str) -> RuntimeHostResult<&'a str> {
    match row.get(key) {
        Some(FactScalar::String(value)) => Ok(value),
        _ => Err(install_error("install_fact_field_invalid")),
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
        .map_err(|_| install_error("install_identity_encode_failed"))?
    {
        serde_json::Value::String(value) => Ok(FactScalar::String(value)),
        _ => Err(install_error("install_identity_encode_failed")),
    }
}
