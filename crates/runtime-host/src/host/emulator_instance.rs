// SPDX-License-Identifier: AGPL-3.0-only

//! Emulator instance control (`RuntimeOperation::ControlEmulatorInstance`, Runtime slice
//! #316-B).
//!
//! One explicit User+Ui or Cli request drives the provider's documented `control` surface
//! once. The request is fenced per instance exactly like monitor recovery: an active or
//! expired lease, an active destructive step, a pending preemption, the takeover cooldown or
//! queued lease requests deny it with `emulator_control_busy` (`RuntimeBusy`). The instance's
//! retained device session is closed first for every action while the daemon keeps running
//! (a session must not outlive the endpoint it was opened on); a refusal there is the same
//! busy denial. The provider is driven outside the fact write gate and outside any device
//! session, and the session is NOT reopened afterwards: it opens lazily on the next lease,
//! as before.
//!
//! Slice #316-B2: a discovery-bound instance may be registered with a PENDING endpoint (it
//! was stopped at startup). After a successful `Start` / `Restart` the registry and the host
//! record are bound to the discovered host and the reported port while the admission guard is
//! still held, and one more `runtime.instance_bound` event records the binding; after `Stop`
//! they return to pending. A running outcome without a port fails typed
//! (`emulator_control_endpoint_unresolved`) instead of guessing.
//!
//! Every request is recorded intent -> result: `client.cli_command` / `client.ui_action` plus
//! `command.received`, then `command.validated` (Performed) on success, or `command.rejected`
//! plus one `runtime.failed` whose record carries the tool exit code (`raw_os_error`), the
//! bounded Sensitive vendor output (`native_detail`) and a typed primary detail. Success also
//! records the instance program fact `device.connected` (and, after `Stop`, invalidates it
//! with `device_closed`).
//!
//! Slice #316-B4: the stuck-recovery ladder's emulator-restart rung drives the same action
//! through `drive_emulator_control` after recording its own `command.received` intent.

use super::runtime_facts::{TASK_GAME_FACT_KEY, TASK_PAGE_FACT_KEY, TASK_SERVER_FACT_KEY};
use super::*;
use crate::{EmulatorControlFailure, EmulatorControlOutcome};
use actingcommand_contract::{EmulatorInstanceAction, FactValue};

const CONTROL_OPERATION: &str = "control_emulator_instance";
pub(super) const DEVICE_CONNECTED_FACT_KEY: &str = "device.connected";
/// Longest wait for adbd after the vendor reports the instance running (#316-B3).
const ADB_BASELINE_WAIT: Duration = Duration::from_secs(30);
const ADB_BASELINE_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// A performed control action: its result terminal and, after `start` / `restart` of an
/// instance with a startup package, that package with its scheduling intent recorded but
/// not yet queued.
pub(super) struct EmulatorControlDriven {
    outcome: EmulatorControlOutcome,
    adb_wait_ms: u64,
    pub(super) terminal: TerminalEvent,
    pub(super) startup_package: Option<super::startup_package::PendingStartupPackage>,
}

impl HostShared {
    pub(super) fn control_emulator_instance(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        action: EmulatorInstanceAction,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        let links = self.append_client_command_intent(
            original,
            request,
            resolved.instance_id(),
            action.event_action(),
            None,
        )?;
        let driven =
            self.drive_emulator_control(&resolved, links, action, original.request_id())?;
        // Slice #316-B3: after `start` / `restart` the configured startup package is only
        // scheduled here (intent event + queue); it runs on the host's own scheduling thread.
        let startup_package = self.schedule_startup_package(driven.startup_package)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(driven.terminal),
            result: RuntimeResult::EmulatorInstanceControlled {
                instance_alias: instance_alias.to_owned(),
                action,
                instance_index: driven.outcome.instance_index,
                running: driven.outcome.running,
                adb_port: driven.outcome.adb_port,
                elapsed_ms: driven.outcome.elapsed_ms.saturating_add(driven.adb_wait_ms),
                startup_package,
            },
        })
    }

    /// Performs one control action whose intent is already recorded under `links`: fence,
    /// session close, provider control, rebinding, ADB baseline, `command.validated` (or
    /// `command.rejected` + `runtime.failed`), `runtime.instance_bound`, `device.connected`,
    /// and the startup package's scheduling intent.
    pub(super) fn drive_emulator_control(
        &self,
        resolved: &RegisteredInstance,
        links: EventLinksDraft,
        action: EmulatorInstanceAction,
        control_request_id: RequestId,
    ) -> Result<EmulatorControlDriven, RequestFailure> {
        let instance_id = resolved.instance_id();
        let event_action = action.event_action();
        let instance_guard = self.instance_guard(instance_id)?;
        let admission = lock(&instance_guard, "lock_instance_admission")?;
        let fence = self
            .monitor_recovery_admission(instance_id)
            .map_err(RequestFailure::poison_without_terminal)?;
        if !fence.admitted() {
            return Err(self.emulator_control_busy(
                links,
                event_action,
                instance_id,
                fence_reason_code(fence.reason),
            )?);
        }
        // A retained session was opened on the endpoint in force before the action; it must
        // not survive the (re)binding below. Closing with no session open is a no-op.
        match self.close_retained_instance_while_guarded(
            instance_id,
            links.clone(),
            false,
            &admission,
        ) {
            Ok(Ok(())) => {}
            Ok(Err(close_error)) => {
                let mut error =
                    RuntimeHostError::execution("close_emulator_device_session", &close_error);
                error.lifecycle.instance_id = Some(instance_id);
                return Err(self.emulator_control_failure(
                    links,
                    event_action,
                    error,
                    RuntimeReceiptState::Failed,
                    EffectDisposition::NotPerformed,
                )?);
            }
            Err(error) if error.projection().code == RuntimeErrorCode::LeaseBusy => {
                return Err(self.emulator_control_busy(
                    links,
                    event_action,
                    instance_id,
                    "active_lease",
                )?);
            }
            Err(error) => {
                return Err(self.emulator_control_failure(
                    links,
                    event_action,
                    error,
                    RuntimeReceiptState::Failed,
                    EffectDisposition::NotPerformed,
                )?);
            }
        }
        // Outside the fact write gate and outside any device session; the per-instance
        // admission guard stays held so no lease is granted while the emulator changes state.
        let outcome = match self
            .execution
            .control_instance(&resolved.instance_alias, action)
        {
            Ok(outcome) => outcome,
            Err(failure) => {
                let (error, state, effect) = emulator_control_error(&failure, instance_id);
                return Err(self.emulator_control_failure(
                    links,
                    event_action,
                    error,
                    state,
                    effect,
                )?);
            }
        };
        // Still under the admission guard: bind the reported port (or return to pending)
        // before any lease can be granted on the instance.
        let rebound = match self.rebind_instance_endpoint(resolved, action, &outcome) {
            Ok(rebound) => rebound,
            Err(error) => {
                return Err(self.emulator_control_failure(
                    links,
                    event_action,
                    error,
                    RuntimeReceiptState::Failed,
                    EffectDisposition::Indeterminate,
                )?);
            }
        };
        // Slice #316-B3 (ADB baseline): the vendor reports `running` a few seconds before
        // adbd answers. Still under the admission guard, a bound `start` / `restart` succeeds
        // only once the ADB baseline answers; a timeout is a non-fatal backend failure that
        // records neither `device.connected` nor a startup package.
        let adb_wait_ms = if action == EmulatorInstanceAction::Stop {
            0
        } else {
            match self.await_adb_baseline(&rebound) {
                Ok(waited_ms) => waited_ms,
                Err(error) => {
                    return Err(self.emulator_control_failure(
                        links,
                        event_action,
                        error,
                        RuntimeReceiptState::Failed,
                        EffectDisposition::Indeterminate,
                    )?);
                }
            }
        };
        drop(admission);
        let validated = self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            CommandPayloadDraft::validated(
                event_action,
                EffectDisposition::Performed,
                AuditInput::new(),
            ),
        )?;
        let terminal_event = terminal(&validated);
        if action != EmulatorInstanceAction::Stop {
            self.append_event(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links.clone(),
                instance_bound_payload(&rebound),
            )?;
        }
        self.record_device_connected(instance_id, action, outcome.running, terminal_event)?;
        let startup_package = if action == EmulatorInstanceAction::Stop {
            None
        } else {
            self.prepare_startup_package(&rebound, links, control_request_id)?
        };
        Ok(EmulatorControlDriven {
            outcome,
            adb_wait_ms,
            terminal: terminal_event,
            startup_package,
        })
    }

    /// Polls the ADB baseline of the freshly bound endpoint until adbd answers `device`
    /// (`ADB_BASELINE_WAIT` at most, one probe every `ADB_BASELINE_POLL_INTERVAL`). Returns
    /// the milliseconds waited; the probes themselves are not recorded, the wait is part of
    /// the receipt's `elapsed_ms`. A timeout is `emulator_control_adb_not_ready`
    /// (`backend_operation_failed`) whose native detail carries the port, the wait and the
    /// last ADB error.
    fn await_adb_baseline(&self, rebound: &RegisteredInstance) -> RuntimeHostResult<u64> {
        let started = Instant::now();
        let deadline = started + ADB_BASELINE_WAIT;
        let stopped = || self.fatal.is_shutdown_requested();
        let mut last_error = None;
        loop {
            if stopped() || Instant::now() >= deadline {
                break;
            }
            match self.execution.probe_adb_baseline_until(
                &rebound.instance_alias,
                deadline,
                &stopped,
            ) {
                Ok(()) => {
                    if stopped() || Instant::now() >= deadline {
                        break;
                    }
                    return Ok(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
                }
                Err(error) => {
                    if error.resource_quiescence()
                        == Some(actingcommand_contract::ResourceQuiescence::Unconfirmed)
                    {
                        return Err(RuntimeHostError::execution(CONTROL_OPERATION, &error));
                    }
                    last_error = Some(error);
                    if Instant::now() >= deadline || stopped() {
                        break;
                    }
                    thread::sleep(
                        ADB_BASELINE_POLL_INTERVAL
                            .min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
            }
        }
        let waited_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut error = RuntimeHostError::request(
            "emulator_control_adb_not_ready",
            CONTROL_OPERATION,
            RuntimeErrorCode::BackendOperationFailed,
        )
        .with_native_detail(format!(
            "instance_alias={}; adb_port={}; waited_ms={waited_ms}; stopped={}; last_error={last_error:?}",
            rebound.instance_alias,
            rebound.bound_adb_endpoint().map_or_else(
                || "absent".to_owned(),
                |endpoint| endpoint.port().to_string()
            ),
            stopped()
        ));
        error.lifecycle.instance_id = Some(rebound.instance_id());
        Err(error)
    }

    /// Applies the control outcome to the discovery binding: `Start` / `Restart` bind the
    /// reported port, `Stop` returns the entry to pending. The registry and the host record
    /// change together under the registry lock (readers check identity under that lock), so
    /// the resolved instance is re-read from the registry, never assumed.
    fn rebind_instance_endpoint(
        &self,
        resolved: &RegisteredInstance,
        action: EmulatorInstanceAction,
        outcome: &EmulatorControlOutcome,
    ) -> RuntimeHostResult<RegisteredInstance> {
        let instance_id = resolved.instance_id();
        let adb_port = match action {
            EmulatorInstanceAction::Stop => None,
            EmulatorInstanceAction::Start | EmulatorInstanceAction::Restart => {
                let Some(adb_port) = outcome.adb_port.filter(|port| *port != 0) else {
                    let mut error = RuntimeHostError::request(
                        "emulator_control_endpoint_unresolved",
                        CONTROL_OPERATION,
                        RuntimeErrorCode::BackendOperationFailed,
                    )
                    .with_native_detail(format!(
                        "running={} adb_port=absent: the started instance reported no ADB port, so the binding stays as it was",
                        outcome.running
                    ));
                    error.lifecycle.instance_id = Some(instance_id);
                    return Err(error);
                };
                Some(adb_port)
            }
        };
        let mut registry = lock(&self.registered_instances, "rebind_instance_endpoint")?;
        let record = registry.get_mut(&instance_id).ok_or_else(|| {
            RuntimeHostError::fatal(
                "runtime_instance_registry_incomplete",
                "rebind_instance_endpoint",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        self.execution
            .rebind_discovered_endpoint(&record.instance_alias, adb_port)
            .map_err(|error| {
                let mut error = RuntimeHostError::execution("rebind_instance_endpoint", &error);
                error.lifecycle.instance_id = Some(instance_id);
                error
            })?;
        let rebound = self
            .execution
            .resolve(&record.instance_alias)
            .map_err(|error| RuntimeHostError::execution("rebind_instance_endpoint", &error))?;
        record.audit_endpoint = rebound.audit_endpoint().to_owned();
        record.adb_endpoint = rebound.adb_endpoint().cloned();
        Ok(record.clone())
    }

    /// The program-fact producer: `device.connected` = the observed running state. After
    /// `Stop` the key is additionally invalidated with `device_closed`; an absent key is not
    /// an error there.
    fn record_device_connected(
        &self,
        instance_id: InstanceId,
        action: EmulatorInstanceAction,
        running: bool,
        terminal_event: TerminalEvent,
    ) -> Result<(), RequestFailure> {
        let scope = RuntimeFactScope::Instance { instance_id };
        let observed_at_unix_ms = self
            .clock
            .sample()
            .map_err(RequestFailure::poison_without_terminal)?
            .unix_ms;
        self.record_runtime_fact(RuntimeFactRecord {
            scope: scope.clone(),
            key: DEVICE_CONNECTED_FACT_KEY.to_owned(),
            value: FactValue::Boolean(running),
            observed_at_unix_ms,
            source: OriginModule::Runtime,
            ttl_ms: None,
        })
        .map_err(|error| fact_failure(error, terminal_event))?;
        if action == EmulatorInstanceAction::Stop {
            // A stopped instance has no foreground (#316-B3) and runs no task either.
            for key in [
                DEVICE_CONNECTED_FACT_KEY,
                actingcommand_contract::APPLICATION_FOREGROUND_FACT_KEY,
                TASK_GAME_FACT_KEY,
                TASK_SERVER_FACT_KEY,
                TASK_PAGE_FACT_KEY,
            ] {
                match self.invalidate_runtime_fact(
                    &scope,
                    key,
                    RuntimeFactInvalidationReason::DeviceClosed,
                ) {
                    Ok(_) => {}
                    Err(error) if error.code() == "runtime_fact_missing" => {}
                    Err(error) => return Err(fact_failure(error, terminal_event)),
                }
            }
        }
        Ok(())
    }

    fn emulator_control_busy(
        &self,
        links: EventLinksDraft,
        event_action: EventAction,
        instance_id: InstanceId,
        reason: &'static str,
    ) -> Result<RequestFailure, RequestFailure> {
        let mut error = RuntimeHostError::request(
            "emulator_control_busy",
            CONTROL_OPERATION,
            RuntimeErrorCode::RuntimeBusy,
        )
        .with_native_detail(format!("fence={reason}"));
        error.lifecycle.instance_id = Some(instance_id);
        self.emulator_control_failure(
            links,
            event_action,
            error,
            RuntimeReceiptState::Denied,
            EffectDisposition::NotPerformed,
        )
    }

    /// Appends `command.rejected` then the `runtime.failed` record (exit code, native detail,
    /// primary detail) through the required-failure path; the rejected event is the terminal.
    fn emulator_control_failure(
        &self,
        links: EventLinksDraft,
        event_action: EventAction,
        error: RuntimeHostError,
        state: RuntimeReceiptState,
        effect: EffectDisposition,
    ) -> Result<RequestFailure, RequestFailure> {
        let diagnostic = match error.projection().code {
            RuntimeErrorCode::BackendOperationFailed => DiagnosticCode::BackendOperationFailed,
            RuntimeErrorCode::RuntimeBusy => DiagnosticCode::LeaseFencingDenied,
            _ => DiagnosticCode::RuntimeDiagnostic,
        };
        let rejected = self.append_event(
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            CommandPayloadDraft::rejected(event_action, diagnostic, effect, AuditInput::new()),
        )?;
        self.record_required_failure(&error, &rejected, links)
            .map_err(RequestFailure::poison_without_terminal)?;
        let terminal_event = terminal(&rejected);
        if error.is_fatal() {
            Ok(RequestFailure::poison(error, Some(terminal_event)))
        } else {
            Ok(RequestFailure::request(error, state, Some(terminal_event)))
        }
    }
}

/// Maps the typed device failure onto the host error: code, projection, receipt state and the
/// effect claim. The vendor output goes to `native_detail` only; the primary detail message
/// is a controlled template (no paths), so the sanitizer accepts it as Sensitive.
fn emulator_control_error(
    failure: &EmulatorControlFailure,
    instance_id: InstanceId,
) -> (RuntimeHostError, RuntimeReceiptState, EffectDisposition) {
    let diagnostic = failure.error.diagnostic();
    let stage = diagnostic.map_or("mumu_manager.control", |diagnostic| diagnostic.stage());
    let category = diagnostic.map_or("child_exit", |diagnostic| diagnostic.category().as_str());
    let (code, runtime_code, state) = match stage {
        "emulator_control.unavailable" | "emulator_control.unregistered" => (
            "emulator_control_unavailable",
            RuntimeErrorCode::InvalidRequest,
            RuntimeReceiptState::Denied,
        ),
        "emulator_control.unsupported" => (
            "emulator_control_unsupported",
            RuntimeErrorCode::InvalidRequest,
            RuntimeReceiptState::Denied,
        ),
        "mumu_manager.wait_timeout" => (
            "emulator_control_wait_timeout",
            RuntimeErrorCode::BackendOperationFailed,
            RuntimeReceiptState::Failed,
        ),
        _ => (
            "emulator_control_failed",
            RuntimeErrorCode::BackendOperationFailed,
            RuntimeReceiptState::Failed,
        ),
    };
    // Once the vendor command has run, its effect on the emulator is not known.
    let effect = if failure.exit_code.is_some() {
        EffectDisposition::Indeterminate
    } else {
        EffectDisposition::NotPerformed
    };
    let mut message = format!(
        "{code}: stage={stage} exit_status={} elapsed_ms={}",
        failure
            .exit_code
            .map_or_else(|| "none".to_owned(), |exit_code| exit_code.to_string()),
        failure.elapsed_ms
    );
    if let Some(state) = &failure.last_state {
        message.push_str(&format!(
            " process_started={} android_started={} adb_port_present={} launch_err_code={}",
            state.process_started,
            state.android_started,
            state.adb_port.is_some(),
            state.launch_err_code
        ));
    }
    let mut error = RuntimeHostError::request(code, CONTROL_OPERATION, runtime_code)
        .with_diagnostic_detail(DiagnosticDetailDraft::new(
            category,
            stage,
            "mumu_manager",
            "control_instance",
            message,
            Sensitivity::Sensitive,
        ))
        .with_native_detail(failure.output_summary.clone());
    error.lifecycle.raw_os_error = failure.exit_code;
    error.lifecycle.instance_id = Some(instance_id);
    (error, state, effect)
}

fn fact_failure(error: RuntimeHostError, terminal_event: TerminalEvent) -> RequestFailure {
    if error.is_fatal() {
        RequestFailure::poison(error, Some(terminal_event))
    } else {
        RequestFailure::request(error, RuntimeReceiptState::Failed, Some(terminal_event))
    }
}

const fn fence_reason_code(reason: MonitorRecoveryCoordinationReason) -> &'static str {
    match reason {
        MonitorRecoveryCoordinationReason::SchedulerAvailable => "scheduler_available",
        MonitorRecoveryCoordinationReason::ActiveLease => "active_lease",
        MonitorRecoveryCoordinationReason::LeaseExpired => "lease_expired",
        MonitorRecoveryCoordinationReason::DestructiveStepActive => "destructive_step_active",
        MonitorRecoveryCoordinationReason::PreemptionPending => "preemption_pending",
        MonitorRecoveryCoordinationReason::TakeoverCooldown => "takeover_cooldown",
        MonitorRecoveryCoordinationReason::QueuedLeaseRequests => "queued_lease_requests",
    }
}
