// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A48 (0.12.9): a self-check reads the emulator status first.
//!
//! On the production host (10-21, after a reboot) the Runtime started before the emulators,
//! which were then started outside it. The Runtime still held each instance stopped (its
//! discovery binding pending), so `actingctl selfcheck` opened nothing and reported
//! `input_backend_open_failed` with an unobserved provider and no attempt, as if adb were broken.
//!
//! Now a self-check of a discovery-bound instance (`SelfCheckInstance` and an instance resume's
//! reconnect: the preparation on an operator claim, `prepare_on_operator_claim`) reads the
//! emulator's status once (`MuMuManager info`, bounded; a status read that never starts
//! anything) under the instance's admission guard, after the preparation closed any retained
//! session and before its opens:
//!
//! - **Stopped** (no emulator process): `instance_not_running` at stage `read_instance_state`;
//!   nothing opens and nothing is started.
//! - **Running, but the Runtime holds it stopped or unrecognized** (its binding pending, its
//!   reading not `on`, or bound to another port than the one reported): the recognition that
//!   follows a Runtime start once its emulator runs (`emulator_instance`), without the start:
//!   the reported port is bound (`runtime.instance_bound` when the binding changed, the reading
//!   `on`), the ADB baseline is awaited as after a start, and `device.connected` is recorded
//!   true. The opens then go on as before; the check passes when they do.
//! - **Running, but the device cannot be recognized**: its Android has not started or it
//!   reports no ADB port (stage `read_instance_state`), or adbd stays silent through the ADB
//!   wait after the reported port was bound (stage `probe_adb_baseline`; the last probe's
//!   failure is the cause, and an adb child the wait left unconfirmed fences the instance as the
//!   start's wait does): `emulator_device_unrecognized` with the status read (`process_started`,
//!   `android_started`, `adb_port_present`, `adb_port`) and `elapsed_ms`; nothing opens.
//! - **Running and recognized**: the reading is renewed and the opens go on as before.
//! - **The read failed**: `emulator_state_read_failed` (warning, the read's failure as its cause)
//!   is recorded and the check goes on from the Runtime's own state: a bound instance opens as
//!   before; a pending one is `instance_not_running` at stage `require_bound_adb_endpoint`, as
//!   every device-facing request on a pending binding is (the registry refuses its opens before
//!   adb runs).
//!
//! A check that ends here is recorded as a failed preparation step: one connection-preparation
//! `runtime.failed` record with the code, its values and the step as its `stage`, then the
//! preparation's own `instance_preparation_finished`, and the self-check reports the code as its
//! `failure_code`. An instance without a discovery binding (an explicit HOST:PORT
//! instance, a handset, a fixture) has no emulator status to read and is checked as before;
//! every other preparation (the daemon start's, an emulator control's, a ladder's, a handset's
//! restore) reads none either. No emulator is started or stopped, and the ladder and its
//! configuration are unchanged.

use super::emulator_instance::{
    ADB_BASELINE_POLL_INTERVAL, ADB_BASELINE_WAIT, DEVICE_CONNECTED_FACT_KEY,
};
use super::*;
use crate::InstanceState;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::{ContractLocation, RuntimeErrorCode};
use actingcommand_contract::outcome::{Location, Outcome};
use actingcommand_contract::{EmulatorRunningState, FactValue};

/// The operation of every outcome here: the preparation phase the self-check runs.
const OPERATION: HostLocation = HostLocation::PrepareInstanceConnection;

/// #411 A48: whether a preparation first reads the emulator status.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum EmulatorStatusRead {
    /// A self-check: `SelfCheckInstance` and an instance resume's reconnect.
    First,
    /// Every other preparation: the daemon start's (its discovery has just read the emulators),
    /// an emulator control's and a ladder's (each follows its own start), a handset's restore.
    Skip,
}

/// How the ADB wait of a recognition ended.
enum AdbWait {
    Answered,
    /// adbd did not answer before the deadline or the shutdown request: the last probe's
    /// failure (`None` when no probe ran).
    Silent(Option<ExecutionKernelError>),
    /// A probe left its adb child or pipe unconfirmed.
    Unconfirmed(ExecutionKernelError),
}

impl HostShared {
    /// #411 A48: the emulator status read of a self-check, before its opens (see the module
    /// documentation). `None`: the opens go on. `Some(code)`: the check ended here with `code`,
    /// whose `runtime.failed` record is written. The caller holds the instance's admission
    /// guard and has closed any retained session. Only a failure the Runtime does not survive
    /// is returned as an error.
    pub(super) fn read_emulator_status_first(
        &self,
        status: EmulatorStatusRead,
        instance_id: InstanceId,
        links: &EventLinksDraft,
    ) -> RuntimeHostResult<Option<HostCode>> {
        if status == EmulatorStatusRead::Skip {
            return Ok(None);
        }
        let record = self.selfcheck_instance_record(instance_id)?;
        if record
            .adb_endpoint
            .as_ref()
            .and_then(ResolvedInstanceEndpoint::discovered_binding)
            .is_none()
        {
            return Ok(None);
        }
        let started = Instant::now();
        let state = match self
            .execution()?
            .read_emulator_instance_state(&record.instance_alias)
        {
            Ok(state) => state,
            Err(error) => {
                let mut unread = RuntimeHostError::request(
                    HostCode::EmulatorStateReadFailed,
                    OPERATION,
                    RuntimeErrorCode::BackendOperationFailed,
                )
                .with_failure_stage(ContractLocation::ReadInstanceState)
                .with_native_detail(error.to_string());
                unread.lifecycle.instance_id = Some(instance_id);
                self.record_selfcheck_failure(&unread, links)?;
                if !record.endpoint_pending() {
                    return Ok(None);
                }
                return self.end_selfcheck(
                    not_running(instance_id, HostLocation::RequireBoundAdbEndpoint),
                    links,
                );
            }
        };
        if !state.process_started {
            self.note_emulator_reading(instance_id, false)?;
            // A48 red proof 2 (T2's mutation, do not merge): today's behaviour, a stopped
            // emulator is not reported; the opens go on as before A48.
            return Ok(None);
        }
        let Some(adb_port) = state.adb_port.filter(|port| *port != 0 && state.running()) else {
            self.note_emulator_reading(instance_id, false)?;
            return self.end_selfcheck(
                unrecognized(
                    instance_id,
                    &state,
                    ContractLocation::ReadInstanceState,
                    started,
                    None,
                ),
                links,
            );
        };
        let recognized = matches!(record.emulator_state, EmulatorRunningState::On { .. })
            && record
                .bound_adb_endpoint()
                .is_some_and(|endpoint| endpoint.port() == adb_port);
        if recognized {
            self.note_emulator_reading(instance_id, true)?;
            return Ok(None);
        }
        self.recognize_running_emulator(&record, &state, adb_port, links, started)
    }

    /// The recognition after a Runtime start once its emulator runs, without the start: the
    /// reported port bound under the registry lock (the reading `on`), `runtime.instance_bound`
    /// when the binding changed, the ADB baseline awaited, and `device.connected` recorded
    /// true. `None` once adbd answered.
    fn recognize_running_emulator(
        &self,
        record: &RegisteredInstance,
        state: &InstanceState,
        adb_port: u16,
        links: &EventLinksDraft,
        started: Instant,
    ) -> RuntimeHostResult<Option<HostCode>> {
        let instance_id = record.instance_id();
        let previous_port = record.bound_adb_endpoint().map(ResolvedAdbEndpoint::port);
        let rebound = self.bind_observed_endpoint(instance_id, Some(adb_port))?;
        if previous_port != Some(adb_port) {
            self.append_event_raw(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links.clone(),
                instance_bound_payload(&rebound),
            )?;
        }
        let (silent, cause) = match self.await_selfcheck_adb(&rebound.instance_alias)? {
            AdbWait::Answered => (false, None),
            AdbWait::Silent(last) => (
                true,
                last.map(|last| {
                    RuntimeHostError::execution(HostLocation::ProbeAdbBaseline, &last)
                        .record_outcome()
                }),
            ),
            // Workflow #387 D1c-G, as the start's ADB wait: an adb child or pipe the probe left
            // unconfirmed belongs to this instance and fences it; only where no fence may take
            // it is it the Runtime's fatal.
            AdbWait::Unconfirmed(error) => {
                let unconfirmed =
                    RuntimeHostError::execution(HostLocation::ProbeAdbBaseline, &error);
                (
                    true,
                    Some(
                        self.settle_unconfirmed_adb(unconfirmed, instance_id, links.clone())?
                            .outcome(),
                    ),
                )
            }
        };
        if silent {
            return self.end_selfcheck(
                unrecognized(
                    instance_id,
                    state,
                    HostLocation::ProbeAdbBaseline,
                    started,
                    cause,
                ),
                links,
            );
        }
        let observed_at_unix_ms = self.clock.sample()?.unix_ms;
        self.record_runtime_fact(RuntimeFactRecord {
            scope: RuntimeFactScope::Instance { instance_id },
            key: DEVICE_CONNECTED_FACT_KEY.to_owned(),
            value: FactValue::Boolean(true),
            observed_at_unix_ms,
            source: OriginModule::Runtime,
            ttl_ms: None,
        })?;
        Ok(None)
    }

    /// The start's ADB baseline wait (`emulator_instance`) on the freshly bound endpoint: one
    /// probe every `ADB_BASELINE_POLL_INTERVAL` until adbd answers `device`, at most
    /// `ADB_BASELINE_WAIT`, ended early by a shutdown request.
    fn await_selfcheck_adb(&self, instance_alias: &str) -> RuntimeHostResult<AdbWait> {
        let deadline = Instant::now() + ADB_BASELINE_WAIT;
        let stopped = || self.fatal.is_shutdown_requested();
        let mut last = None;
        while !stopped() && Instant::now() < deadline {
            match self
                .execution()?
                .probe_adb_baseline_until(instance_alias, deadline, &stopped)
            {
                Ok(()) => return Ok(AdbWait::Answered),
                Err(error)
                    if error.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed) =>
                {
                    return Ok(AdbWait::Unconfirmed(error));
                }
                Err(error) => {
                    last = Some(error);
                    thread::sleep(
                        ADB_BASELINE_POLL_INTERVAL
                            .min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
            }
        }
        Ok(AdbWait::Silent(last))
    }

    /// The self-check ends before any open with `failure`, built with `code`: its record, and
    /// its code.
    fn end_selfcheck(
        &self,
        (code, failure): (HostCode, RuntimeHostError),
        links: &EventLinksDraft,
    ) -> RuntimeHostResult<Option<HostCode>> {
        self.record_selfcheck_failure(&failure, links)?;
        Ok(Some(code))
    }

    /// One connection-preparation `runtime.failed` record of a self-check step (its `stage` the
    /// step), under the self-check's links.
    fn record_selfcheck_failure(
        &self,
        failure: &RuntimeHostError,
        links: &EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        self.append_lifecycle_failure(
            RuntimeLifecycleFailureStage::ConnectionPreparation,
            RuntimeLifecycleFailure::Host(failure),
            links.clone(),
            None,
        )
    }

    /// The instance's record as the Runtime holds it now.
    fn selfcheck_instance_record(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<RegisteredInstance> {
        lock(
            &self.registered_instances,
            HostLocation::ReadObservedBinding,
        )?
        .get(&instance_id)
        .cloned()
        .ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::RuntimeInstanceRegistryIncomplete,
                HostLocation::ReadObservedBinding,
                RuntimeErrorCode::RuntimeHalted,
            )
        })
    }

    /// Workflow #390 九处-1: the emulator's running state as this read saw it (`on` only when
    /// it runs with Android and an ADB port, as a discovery reads it), at the read's time.
    fn note_emulator_reading(
        &self,
        instance_id: InstanceId,
        running: bool,
    ) -> RuntimeHostResult<()> {
        let observed_at_unix_ms = self.clock.sample()?.unix_ms;
        if let Some(record) = lock(
            &self.registered_instances,
            HostLocation::ReadObservedBinding,
        )?
        .get_mut(&instance_id)
        {
            record.emulator_state = EmulatorRunningState::observed(running, observed_at_unix_ms);
        }
        Ok(())
    }
}

/// `instance_not_running` of a self-check, at `stage`: the status read saw no emulator process
/// (`read_instance_state`), or the read failed and the Runtime holds the binding pending
/// (`require_bound_adb_endpoint`).
fn not_running(
    instance_id: InstanceId,
    stage: impl Into<Location>,
) -> (HostCode, RuntimeHostError) {
    let code = HostCode::InstanceNotRunning;
    let mut error = RuntimeHostError::request(code, OPERATION, RuntimeErrorCode::InvalidRequest)
        .with_failure_stage(stage);
    error.lifecycle.instance_id = Some(instance_id);
    (code, error)
}

/// `emulator_device_unrecognized` at `stage`, with the status read, the time the attempt took
/// since the read began, and the failure behind it (`cause`) as its `caused_by` link.
fn unrecognized(
    instance_id: InstanceId,
    state: &InstanceState,
    stage: impl Into<Location>,
    started: Instant,
    cause: Option<Outcome>,
) -> (HostCode, RuntimeHostError) {
    let code = HostCode::EmulatorDeviceUnrecognized;
    let stage: Location = stage.into();
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut error =
        RuntimeHostError::request(code, OPERATION, RuntimeErrorCode::BackendOperationFailed)
            .with_outcome_values(|outcome| {
                status_values(outcome, state)
                    .stage(stage)
                    .elapsed_ms(elapsed_ms)
            })
            .with_failure_stage(stage);
    if let Some(cause) = cause {
        error = error.with_caused_by(cause);
    }
    error.lifecycle.instance_id = Some(instance_id);
    (code, error)
}

/// The status read as typed values.
fn status_values(outcome: Outcome, state: &InstanceState) -> Outcome {
    let outcome = outcome
        .process_started(state.process_started)
        .android_started(state.android_started)
        .adb_port_present(state.adb_port.is_some());
    match state.adb_port {
        Some(adb_port) => outcome.adb_port(adb_port),
        None => outcome,
    }
}
