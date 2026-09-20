// SPDX-License-Identifier: AGPL-3.0-only

//! The foreground application gate (Runtime slice #316-B3).
//!
//! Before any pointer input (tap, long tap, swipe, drag) reaches a physical instance, the host
//! asks the ADB baseline which package Android reports in the foreground and refuses the
//! input unless it is the instance's assigned `application_id`. The check sits in the single
//! input choke point (`HostShared::input`), after the frame resolution and before the input is
//! prepared, so one check covers the adb and the Nemu touch backends alike; the Runtime never
//! taps into another application.
//!
//! The observation is the instance program fact `application.foreground` (ledger first,
//! `record_runtime_fact`), written whenever the observed value changes. An ADB failure is the
//! loss of the ADB baseline: `device.connected` and `application.foreground` are invalidated
//! with `adb_unreachable` before the input is refused, even while a Nemu session still
//! delivers frames. Refusals are `command.rejected` (diagnostic `runtime.diagnostic`, effect
//! `not_performed`) plus the `runtime.failed` record, receipt state `denied`, host codes
//! `application_not_foreground` / `application_foreground_unknown` (`invalid_request`).

use super::emulator_instance::DEVICE_CONNECTED_FACT_KEY;
use super::*;
use actingcommand_contract::{APPLICATION_FOREGROUND_FACT_KEY, FactValue};

const GATE_OPERATION: &str = "require_foreground_application";

impl HostShared {
    /// Refuses a pointer input whose foreground package is not the assigned application.
    /// Non-pointer actions (key, text, reset) and fixture instances pass untouched.
    pub(super) fn require_foreground_application(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        resolved: &RegisteredInstance,
        action: &InputAction,
        run_links: Option<RuntimeRunLinks>,
    ) -> Result<(), RequestFailure> {
        if resolved.provenance() != ExecutionBackendProvenance::PhysicalDevice
            || !matches!(
                action,
                InputAction::Tap { .. }
                    | InputAction::LongTap { .. }
                    | InputAction::Swipe { .. }
                    | InputAction::SingleTouchDragWithVerticalBrakeV1 { .. }
            )
        {
            return Ok(());
        }
        let instance_id = resolved.instance_id();
        let alias = resolved.instance_alias.as_str();
        let observation = match self
            .execution
            .observe_foreground_application(&resolved.instance_alias)
        {
            Ok(observation) => observation,
            Err(error) => {
                // The ADB baseline is gone; nothing device-bound is trusted from here on.
                self.invalidate_adb_baseline(instance_id)
                    .map_err(RequestFailure::poison_without_terminal)?;
                let error = gate_error(
                    "application_foreground_unknown",
                    instance_id,
                    format!("instance_alias={alias}; adb_failed={error}"),
                );
                return Err(self.reject_input(request, token, action, run_links, error)?);
            }
        };
        let Some(foreground) = observation.foreground else {
            let error = gate_error(
                "application_foreground_unknown",
                instance_id,
                format!(
                    "instance_alias={alias}; foreground=absent; assigned={}",
                    observation.assigned
                ),
            );
            return Err(self.reject_input(request, token, action, run_links, error)?);
        };
        self.record_foreground_application(instance_id, &foreground)
            .map_err(RequestFailure::poison_without_terminal)?;
        if foreground != observation.assigned {
            let error = gate_error(
                "application_not_foreground",
                instance_id,
                format!(
                    "instance_alias={alias}; foreground={foreground}; assigned={}",
                    observation.assigned
                ),
            );
            return Err(self.reject_input(request, token, action, run_links, error)?);
        }
        Ok(())
    }

    /// `command.rejected` for the input's own action plus the `runtime.failed` record; the
    /// rejected event is the receipt terminal.
    fn reject_input(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        action: &InputAction,
        run_links: Option<RuntimeRunLinks>,
        error: RuntimeHostError,
    ) -> Result<RequestFailure, RequestFailure> {
        let mut links = self.events.request_links(
            request,
            Some(token.instance_id()),
            Some(token.lease_id()),
            None,
        );
        if let Some(run_links) = run_links {
            links = run_links.apply(links);
        }
        let rejected = self.append_event(
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            CommandPayloadDraft::rejected(
                action.event_action(),
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            ),
        )?;
        self.record_required_failure(&error, &rejected, links)
            .map_err(RequestFailure::poison_without_terminal)?;
        Ok(RequestFailure::request(
            error,
            RuntimeReceiptState::Denied,
            Some(terminal(&rejected)),
        ))
    }

    /// Records `application.foreground` when the observed package differs from the stored
    /// one; an unchanged value appends nothing. Two observations inside one millisecond keep
    /// the first (`runtime_fact_stale` is not a gate failure).
    fn record_foreground_application(
        &self,
        instance_id: InstanceId,
        foreground: &str,
    ) -> RuntimeHostResult<()> {
        let scope = RuntimeFactScope::Instance { instance_id };
        let value = FactValue::String(foreground.to_owned());
        let unchanged = lock(&self.runtime_facts, "read_application_foreground")?
            .get(&scope, APPLICATION_FOREGROUND_FACT_KEY)
            .is_some_and(|record| record.value == value);
        if unchanged {
            return Ok(());
        }
        let observed_at_unix_ms = self.clock.sample()?.unix_ms;
        match self.record_runtime_fact(RuntimeFactRecord {
            scope,
            key: APPLICATION_FOREGROUND_FACT_KEY.to_owned(),
            value,
            observed_at_unix_ms,
            source: OriginModule::Runtime,
            ttl_ms: None,
        }) {
            Ok(_) => Ok(()),
            Err(error) if error.code() == "runtime_fact_stale" => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Drops `device.connected` and `application.foreground` with `adb_unreachable`; an
    /// absent key is not an error.
    fn invalidate_adb_baseline(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        let scope = RuntimeFactScope::Instance { instance_id };
        for key in [DEVICE_CONNECTED_FACT_KEY, APPLICATION_FOREGROUND_FACT_KEY] {
            match self.invalidate_runtime_fact(
                &scope,
                key,
                RuntimeFactInvalidationReason::AdbUnreachable,
            ) {
                Ok(_) => {}
                Err(error) if error.code() == "runtime_fact_missing" => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

fn gate_error(code: &'static str, instance_id: InstanceId, detail: String) -> RuntimeHostError {
    let mut error =
        RuntimeHostError::request(code, GATE_OPERATION, RuntimeErrorCode::InvalidRequest)
            .with_native_detail(detail);
    error.lifecycle.instance_id = Some(instance_id);
    error
}
