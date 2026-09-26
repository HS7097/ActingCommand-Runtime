// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_device::{DeviceError, DeviceResult, InputCheckPhase, InputOperationCheck};

pub(super) struct RuntimeInputCheck {
    scheduler: Arc<Mutex<SeedScheduler>>,
    connection_id: ConnectionId,
    clock: Arc<dyn RuntimeClock>,
    clock_origin: u64,
    hard_deadline: u64,
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
        if now >= deadline {
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
            .map_err(|error| DeviceError::fatal(format!("Nemu input fencing failed: {error}")))?;
        if matches!(phase, InputCheckPhase::Continue)
            && (self.closing
                || self.fatal.is_shutdown_requested()
                || self
                    .control
                    .as_ref()
                    .is_some_and(|control| control.cancellation_reason(now).is_some()))
        {
            return Err(DeviceError::fatal("Nemu input continuation is stopped"));
        }
        Ok(Duration::from_millis(deadline - now))
    }
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
        let resolved = self.execution.resolve(alias).map_err(|error| {
            RuntimeHostError::execution("resolve_nemu_input_configuration", &error)
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
                    "nemu_input_deadline_overflow",
                    "prepare_nemu_input",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?
            .min(token.expires_at_monotonic_ms());
        if let Some(control) = &control {
            if control.deadline() == 0 {
                return Err(RuntimeHostError::fatal(
                    "nemu_input_task_deadline_missing",
                    "prepare_nemu_input",
                    RuntimeErrorCode::RuntimeFatal,
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
            control,
            fatal: self.fatal.clone(),
            closing,
        }))
    }

    pub(super) fn nemu_close_check(
        &self,
        token: &LeaseToken,
        witness: Arc<FencedWrite>,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<Option<Arc<dyn InputOperationCheck>>> {
        let alias = lock(&self.registered_instances, "resolve_nemu_close_instance")?
            .get(&token.instance_id())
            .map(|instance| instance.instance_alias.clone())
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "nemu_close_instance_missing",
                    "prepare_nemu_close",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        self.nemu_input_check(&alias, token, connection_id, None, true)
            .map(|check| check.map(|check| check.fenced(witness)))
    }
}
