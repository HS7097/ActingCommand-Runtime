// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::ContainedTaskCancellationReason;
use actingcommand_contract::codes::ContractLocation;
use actingcommand_device::{
    DeviceError, DeviceResult, InputCheckPhase, InputCheckStop, InputOperationCheck,
};

pub(super) struct RuntimeInputCheck {
    scheduler: Arc<Mutex<SeedScheduler>>,
    connection_id: ConnectionId,
    clock: Arc<dyn RuntimeClock>,
    clock_origin: u64,
    hard_deadline: u64,
    /// ISO-2b (#419 R1, ISO-M row E08): the expiry of the lease that admitted the input.
    lease_expiry: u64,
    control: Option<Arc<ContainedRunControl>>,
    fatal: FatalState,
    closing: bool,
}

struct FencedInputCheck {
    check: RuntimeInputCheck,
    witness: Arc<FencedWrite>,
}

impl InputOperationCheck for FencedInputCheck {
    fn check(&self, phase: InputCheckPhase) -> DeviceResult<Duration> {
        self.check.check(phase, &self.witness)
    }
}

impl RuntimeInputCheck {
    pub(super) fn fenced(self, witness: Arc<FencedWrite>) -> Arc<dyn InputOperationCheck> {
        Arc::new(FencedInputCheck {
            check: self,
            witness,
        })
    }

    fn check(&self, phase: InputCheckPhase, witness: &FencedWrite) -> DeviceResult<Duration> {
        let sample = self
            .clock
            .sample()
            .map_err(|error| DeviceError::fatal(format!("{error:?}")))?;
        let now = sample
            .monotonic_ms
            .checked_sub(self.clock_origin)
            .ok_or_else(|| DeviceError::fatal("Nemu input monotonic clock regressed"))?;
        let mut deadline = self.hard_deadline;
        if let Some(control) = &self.control {
            if control.deadline() == 0 {
                return Err(DeviceError::fatal(
                    "Nemu input task deadline is unavailable",
                ));
            }
            deadline = deadline.min(control.deadline());
        }
        // ISO-2b (#419 R1, ISO-M row E08): an input that the run's own deadline, its lease or
        // its cancellation stops is stopped by the work that admitted it, not by the device: the
        // check says which (`DeviceError::input_check_stopped`), and the input fails with the
        // run's or the lease's own code (`input_check_stop_failure`). The input's own command
        // timeout, the clock, the scheduler and a close's check keep failing as the device's.
        if now >= deadline {
            if self
                .control
                .as_ref()
                .is_some_and(|control| now >= control.deadline())
            {
                return Err(self.stopped(
                    InputCheckStop::Deadline,
                    "Nemu input original deadline expired",
                ));
            }
            if now >= self.lease_expiry {
                return Err(self.stopped(InputCheckStop::Lease, "Nemu input lease expired"));
            }
            return Err(DeviceError::fatal("Nemu input original deadline expired"));
        }
        let scheduler = self
            .scheduler
            .lock()
            .map_err(|_| DeviceError::fatal("Nemu input scheduler is poisoned"))?;
        // The witness check covers the step's current lease, connection, expiry and cooldown;
        // its Business lease was write-admitted when `begin_destructive_step` minted it.
        scheduler
            .validate_destructive_step(witness, self.connection_id, now)
            .map_err(|error| {
                let message = format!("Nemu input fencing failed: {error}");
                if error.is_fatal() {
                    DeviceError::fatal(message)
                } else {
                    self.stopped(InputCheckStop::Lease, message)
                }
            })?;
        if matches!(phase, InputCheckPhase::Continue) {
            if self.closing || self.fatal.is_shutdown_requested() {
                return Err(DeviceError::fatal("Nemu input continuation is stopped"));
            }
            if self
                .control
                .as_ref()
                .is_some_and(|control| control.cancellation_reason(now).is_some())
            {
                return Err(self.stopped(
                    InputCheckStop::Cancelled,
                    "Nemu input continuation is stopped",
                ));
            }
        }
        Ok(Duration::from_millis(deadline - now))
    }

    /// ISO-2b (#419 R1, ISO-M row E08): the input stopped by `stop`; a close's check keeps
    /// failing as the device's, as before.
    fn stopped(&self, stop: InputCheckStop, message: impl Into<String>) -> DeviceError {
        if self.closing {
            DeviceError::fatal(message)
        } else {
            DeviceError::input_check_stopped(stop, message)
        }
    }
}

#[cfg(test)]
impl RuntimeHost {
    /// ISO-2b (#419, ISO-M row E08) tests: the Nemu input check of `alias` under `token` (on the
    /// connection that holds it) for a run of `request_id` whose deadline is `deadline_after_ms`
    /// from now and whose client asked it to stop when `cancelled`. Its input's step begins now;
    /// after `advance` (the test's clock), one check of `phase` answers; with it comes the
    /// failure the input path makes of a stopped input (`HostShared::input_check_stop_failure`).
    /// The step ends before this returns.
    #[allow(
        clippy::too_many_arguments,
        reason = "one test seam of the check's inputs"
    )]
    pub(crate) fn nemu_input_check_for_test(
        &self,
        alias: &str,
        token: &LeaseToken,
        request_id: RequestId,
        deadline_after_ms: u64,
        cancelled: bool,
        advance: impl FnOnce(),
        phase: InputCheckPhase,
    ) -> RuntimeHostResult<(DeviceResult<Duration>, Option<RuntimeHostError>)> {
        let shared =
            self.shared_ref(crate::codes::test_spellings::TestLocation::ContainFailureForTest)?;
        let missing = || {
            RuntimeHostError::fatal(
                crate::codes::test_spellings::TestCode::TestInstanceMissing,
                crate::codes::test_spellings::TestLocation::ContainFailureForTest,
                RuntimeErrorCode::InvalidRequest,
            )
        };
        let connection_id = lock(&shared.scheduler, HostLocation::BeginDestructiveInput)?
            .active_lease(token.instance_id())
            .map(|lease| lease.connection_id())
            .ok_or_else(missing)?;
        let now = shared.monotonic_ms()?;
        let control = Arc::new(ContainedRunControl::for_test(
            request_id,
            token.instance_id(),
            now.saturating_add(deadline_after_ms),
            cancelled,
        )?);
        let check = shared
            .nemu_input_check(
                alias,
                token,
                connection_id,
                Some(Arc::clone(&control)),
                false,
            )?
            .ok_or_else(missing)?;
        let witness = Arc::new(
            lock(&shared.scheduler, HostLocation::BeginDestructiveInput)?
                .begin_destructive_step(token, connection_id, now)
                .map_err(|error| {
                    RuntimeHostError::scheduler(HostLocation::BeginDestructiveInput, &error)
                })?,
        );
        advance();
        let answer = check.fenced(Arc::clone(&witness)).check(phase);
        let failure = match answer
            .as_ref()
            .err()
            .and_then(DeviceError::input_check_stop)
        {
            Some(stop) => {
                shared.input_check_stop_failure(stop, Some(&control), token, connection_id)?
            }
            None => None,
        };
        if let Ok(witness) = Arc::try_unwrap(witness) {
            // A lease that lapsed under the check has no step left to finish.
            let _ = lock(&shared.scheduler, HostLocation::FinishDestructiveInput)?
                .finish_destructive_step(witness, connection_id);
        }
        Ok((answer, failure))
    }
}

/// ISO-2b (#419 R1, ISO-M row E08): the failure of a run whose own `reason` stopped it, as the
/// run itself ends on it (`ensure_active`): its deadline, its client's cancel or a pause's drain.
/// `None` for a reason no run's control holds yet.
fn run_stop_failure(reason: ContainedTaskCancellationReason) -> Option<RuntimeHostError> {
    let code = match reason {
        ContainedTaskCancellationReason::DeadlineExceeded => {
            RuntimeErrorCode::ContainedTaskDeadlineExceeded
        }
        ContainedTaskCancellationReason::ClientRequested => {
            RuntimeErrorCode::ContainedTaskCancelled
        }
        ContainedTaskCancellationReason::PausedByOperator => RuntimeErrorCode::ContainedTaskPaused,
        ContainedTaskCancellationReason::RecoveredAfterRestart
        | ContainedTaskCancellationReason::BlackoutStop
        | ContainedTaskCancellationReason::PerformanceYield => return None,
    };
    Some(RuntimeHostError::request(
        code.code(),
        ContractLocation::RunContainedTask,
        code,
    ))
}

impl HostShared {
    pub(super) fn nemu_input_check(
        &self,
        alias: &str,
        token: &LeaseToken,
        connection_id: ConnectionId,
        control: Option<Arc<ContainedRunControl>>,
        closing: bool,
    ) -> RuntimeHostResult<Option<RuntimeInputCheck>> {
        let resolved = self.execution()?.resolve(alias).map_err(|error| {
            RuntimeHostError::execution(HostLocation::ResolveNemuInputConfiguration, &error)
        })?;
        let Some(configuration) = resolved
            .configuration()
            .filter(|value| value.input_backend == "nemu_ipc")
        else {
            return Ok(None);
        };
        let now = self.monotonic_ms()?;
        let mut hard_deadline = now
            .checked_add(configuration.input_command_timeout_ms)
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    HostCode::NemuInputDeadlineOverflow,
                    HostLocation::PrepareNemuInput,
                    RuntimeErrorCode::RuntimeHalted,
                )
            })?
            .min(token.expires_at_monotonic_ms());
        if let Some(control) = &control {
            if control.deadline() == 0 {
                // ISO-2 (#419, ISO-M row E47): the run's own control has no deadline, a fault
                // of that run's instance alone.
                return Err(RuntimeHostError::instance_fault(
                    HostCode::NemuInputTaskDeadlineMissing,
                    HostLocation::PrepareNemuInput,
                    RuntimeErrorCode::RuntimeHalted,
                    token.instance_id(),
                ));
            }
            hard_deadline = hard_deadline.min(control.deadline());
        }
        Ok(Some(RuntimeInputCheck {
            scheduler: Arc::clone(&self.scheduler),
            connection_id,
            clock: Arc::clone(&self.clock),
            clock_origin: self.clock_origin_monotonic_ms,
            hard_deadline,
            lease_expiry: token.expires_at_monotonic_ms(),
            control,
            fatal: self.fatal.clone(),
            closing,
        }))
    }

    /// ISO-2b (#419 R1, ISO-M row E08): the failure of an input on `token` that its operation
    /// check stopped (`stop`, the kernel's failure carries it), in the terms of the work that
    /// admitted it: the run's own deadline, cancel or pause code (its `control`), or the lease's
    /// own refusal. The input fails only its operation with it, non-fatal; the instance and its
    /// device are sound. `None` when that work has not ended after all (the run goes on, the
    /// lease still admits writes) or its refusal is the scheduler's own failure: the input then
    /// fails as its device's failure, as before.
    pub(super) fn input_check_stop_failure(
        &self,
        stop: InputCheckStop,
        control: Option<&ContainedRunControl>,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<Option<RuntimeHostError>> {
        let now = self.monotonic_ms()?;
        Ok(match stop {
            InputCheckStop::Deadline | InputCheckStop::Cancelled => control
                .and_then(|control| control.cancellation_reason(now))
                .and_then(run_stop_failure),
            InputCheckStop::Lease => lock(&self.scheduler, HostLocation::ExecuteInputBackend)?
                .validate_write(token, connection_id, now)
                .err()
                .filter(|refusal| !refusal.is_fatal())
                .map(|refusal| {
                    RuntimeHostError::scheduler(HostLocation::ExecuteInputBackend, &refusal)
                        .naming_lease(token.lease_id())
                }),
        })
    }

    pub(super) fn nemu_close_check(
        &self,
        token: &LeaseToken,
        witness: Arc<FencedWrite>,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<Option<Arc<dyn InputOperationCheck>>> {
        let alias = lock(
            &self.registered_instances,
            HostLocation::ResolveNemuCloseInstance,
        )?
        .get(&token.instance_id())
        .map(|instance| instance.instance_alias.clone())
        .ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::NemuCloseInstanceMissing,
                HostLocation::PrepareNemuClose,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        self.nemu_input_check(&alias, token, connection_id, None, true)
            .map(|check| check.map(|check| check.fenced(witness)))
    }
}
