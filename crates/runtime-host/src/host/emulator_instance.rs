// SPDX-License-Identifier: AGPL-3.0-only

//! Emulator instance control (`RuntimeOperation::ControlEmulatorInstance`, Runtime slice
//! #316-B).
//!
//! One explicit User+Ui or Cli request drives the provider's documented `control` surface
//! once. The request is fenced per instance exactly like monitor recovery: an active or
//! expired lease, an active destructive step, a pending preemption, the takeover cooldown or
//! queued lease requests deny it with `emulator_control_busy` (`RuntimeBusy`). For `Stop` and
//! `Restart` the instance's retained device session is closed first while the daemon keeps
//! running; a refusal there is the same busy denial. The provider is driven outside the fact
//! write gate and outside any device session, and the session is NOT reopened afterwards: it
//! opens lazily on the next lease, as before.
//!
//! Every request is recorded intent -> result: `client.cli_command` / `client.ui_action` plus
//! `command.received`, then `command.validated` (Performed) on success, or `command.rejected`
//! plus one `runtime.failed` whose record carries the tool exit code (`raw_os_error`), the
//! bounded Sensitive vendor output (`native_detail`) and a typed primary detail. Success also
//! records the instance program fact `device.connected` (and, after `Stop`, invalidates it
//! with `device_closed`).

use super::*;
use crate::EmulatorControlFailure;
use actingcommand_contract::{EmulatorInstanceAction, FactValue};

const CONTROL_OPERATION: &str = "control_emulator_instance";
const DEVICE_CONNECTED_FACT_KEY: &str = "device.connected";

impl HostShared {
    pub(super) fn control_emulator_instance(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        action: EmulatorInstanceAction,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        let instance_id = resolved.instance_id();
        let event_action = action.event_action();
        let links =
            self.append_client_command_intent(original, request, instance_id, event_action, None)?;
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
        if matches!(
            action,
            EmulatorInstanceAction::Stop | EmulatorInstanceAction::Restart
        ) {
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
        drop(admission);
        let validated = self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            CommandPayloadDraft::validated(
                event_action,
                EffectDisposition::Performed,
                AuditInput::new(),
            ),
        )?;
        let terminal_event = terminal(&validated);
        self.record_device_connected(instance_id, action, outcome.running, terminal_event)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal_event),
            result: RuntimeResult::EmulatorInstanceControlled {
                instance_alias: instance_alias.to_owned(),
                action,
                instance_index: outcome.instance_index,
                running: outcome.running,
                adb_port: outcome.adb_port,
                elapsed_ms: outcome.elapsed_ms,
            },
        })
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
            match self.invalidate_runtime_fact(
                &scope,
                DEVICE_CONNECTED_FACT_KEY,
                RuntimeFactInvalidationReason::DeviceClosed,
            ) {
                Ok(_) => {}
                Err(error) if error.code() == "runtime_fact_missing" => {}
                Err(error) => return Err(fact_failure(error, terminal_event)),
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
