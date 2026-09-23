// SPDX-License-Identifier: AGPL-3.0-only

//! On-demand instance discovery (`RuntimeOperation::DiscoverInstances`, slice #316 a2).
//!
//! One explicit User+Ui or Cli request re-runs the provider's instance discovery once and
//! reports every instance it found, each with the alias of the registered instance bound to
//! it. No lease, admission guard, device session or binding is touched. The answer is
//! committed as one `RuntimeStateFact::Observed` (`InstanceDiscovery`) in the request's
//! `command.validated` event and the response carries that event as `source`. A refusal
//! appends `command.rejected` (the receipt terminal) plus one `runtime.failed` whose native
//! detail keeps the provider's device error.

use super::*;
use crate::{InstanceDiscoveryFailure, ProviderDiscoveredInstance};
use actingcommand_contract::{
    RuntimeDiscoveredInstance, RuntimeInstanceDiscovery, RuntimeObservedState,
};

const DISCOVERY_OPERATION: &str = "discover_instances";

impl HostShared {
    pub(super) fn discover_instances(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let (state, source) = self.observe_runtime_state(validated, || {
            let discovery = match self.execution.discover_instances() {
                Ok(discovery) => discovery,
                Err(failure) => {
                    return Err(
                        self.instance_discovery_refused(validated, discovery_error(&failure))?
                    );
                }
            };
            // The registry lock is taken only after the vendor tool has answered.
            let registry = lock(
                &self.registered_instances,
                "read_instance_discovery_registry",
            )?;
            let mut instances = discovery
                .instances
                .into_iter()
                .map(|instance| RuntimeDiscoveredInstance {
                    bound_alias: bound_alias(&registry, &instance),
                    instance_index: instance.instance_index,
                    instance_name: instance.instance_name,
                    adb_host: instance.adb_host,
                    adb_port: instance.adb_port,
                    running: instance.running,
                    android_version: instance.android_version,
                })
                .collect::<Vec<_>>();
            drop(registry);
            instances.sort_by_key(|instance| instance.instance_index);
            if let Err(error) = RuntimeInstanceDiscovery::validate_instances(&instances) {
                let error = RuntimeHostError::request(
                    "instance_discovery_unavailable",
                    DISCOVERY_OPERATION,
                    RuntimeErrorCode::BackendOperationFailed,
                )
                .with_native_detail(format!(
                    "the provider reported an instance list the contract refuses: {}",
                    error.code()
                ));
                return Err(self.instance_discovery_refused(validated, error)?);
            }
            Ok(RuntimeObservedState::InstanceDiscovery {
                owner_epoch: self.owner_epoch,
                provider_version: discovery.provider_version,
                instances,
            })
        })?;
        let RuntimeObservedState::InstanceDiscovery {
            provider_version,
            instances,
            ..
        } = state
        else {
            return Err(RequestFailure::poison_without_terminal(ledger_error(
                "instance_discovery_observation_kind",
            )));
        };
        let discovery = RuntimeInstanceDiscovery::new(provider_version, source, instances)
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error("instance_discovery_source"))
            })?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::InstancesDiscovered { discovery },
        })
    }

    /// Appends `command.rejected` (the receipt terminal) then the `runtime.failed` record of a
    /// refused discovery. Nothing was performed.
    fn instance_discovery_refused(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        error: RuntimeHostError,
    ) -> Result<RequestFailure, RequestFailure> {
        let (diagnostic, state) =
            if error.projection().code == RuntimeErrorCode::BackendOperationFailed {
                (
                    DiagnosticCode::BackendOperationFailed,
                    RuntimeReceiptState::Failed,
                )
            } else {
                (
                    DiagnosticCode::RuntimeDiagnostic,
                    RuntimeReceiptState::Denied,
                )
            };
        let links = self.events.request_links(validated, None, None, None);
        let rejected = self.append_event(
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            CommandPayloadDraft::rejected(
                EventAction::RuntimeAction,
                diagnostic,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            ),
        )?;
        self.record_required_failure(&error, &rejected, links)
            .map_err(RequestFailure::poison_without_terminal)?;
        Ok(RequestFailure::request(
            error,
            state,
            Some(terminal(&rejected)),
        ))
    }
}

/// The registered instance bound to a reported one: the discovery binding with the same
/// index, else the explicit HOST:PORT instance with the same ADB port.
fn bound_alias(
    registry: &BTreeMap<InstanceId, RegisteredInstance>,
    reported: &ProviderDiscoveredInstance,
) -> Option<String> {
    let discovered_index = |instance: &RegisteredInstance| {
        instance
            .adb_endpoint
            .as_ref()
            .and_then(ResolvedInstanceEndpoint::discovered_binding)
            .map(|binding| binding.instance_index())
    };
    registry
        .values()
        .find(|instance| discovered_index(instance) == Some(reported.instance_index))
        .or_else(|| {
            let port = reported.adb_port?;
            registry.values().find(|instance| {
                discovered_index(instance).is_none()
                    && instance.bound_adb_endpoint().is_some_and(|endpoint| {
                        !endpoint.serial_configured() && endpoint.port() == port
                    })
            })
        })
        .map(|instance| instance.instance_alias.clone())
}

/// Maps the provider refusal onto the host error. `instance_discovery_unavailable` is
/// `invalid_request` (denied) when the provider has no discovery surface and
/// `backend_operation_failed` (failed) for a tool spawn, exit, JSON or timeout failure;
/// `mumu_manager_version_unsupported` is `backend_operation_failed`. The device error goes to
/// `native_detail` only; the primary detail is a controlled template (no paths, no
/// provider-supplied token besides the typed stage).
fn discovery_error(failure: &InstanceDiscoveryFailure) -> RuntimeHostError {
    let diagnostic = failure.error.diagnostic();
    let stage = diagnostic.map_or("instance_discovery.failed", |diagnostic| diagnostic.stage());
    let category = diagnostic.map_or("protocol", |diagnostic| diagnostic.category().as_str());
    let (code, runtime_code) = match (failure.code, stage) {
        ("mumu_manager_version_unsupported", _) => (
            "mumu_manager_version_unsupported",
            RuntimeErrorCode::BackendOperationFailed,
        ),
        (_, "instance_discovery.unsupported") => (
            "instance_discovery_unavailable",
            RuntimeErrorCode::InvalidRequest,
        ),
        _ => (
            "instance_discovery_unavailable",
            RuntimeErrorCode::BackendOperationFailed,
        ),
    };
    RuntimeHostError::request(code, DISCOVERY_OPERATION, runtime_code)
        .with_diagnostic_detail(DiagnosticDetailDraft::new(
            category,
            stage,
            "execution_backend_provider",
            DISCOVERY_OPERATION,
            format!("{code}: stage={stage}"),
            Sensitivity::Sensitive,
        ))
        .with_native_detail(failure.error.to_string())
}
