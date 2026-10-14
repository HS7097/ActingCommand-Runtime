// SPDX-License-Identifier: AGPL-3.0-only

//! On-demand instance discovery (`RuntimeOperation::DiscoverInstances`, slice #316 a2).
//!
//! One explicit User+Ui or Cli request re-runs the provider's instance discovery once and
//! reports every instance it found, each with the alias of the registered instance bound to
//! it. No lease, admission guard, device session or binding is touched. The answer appends no
//! ledger record (Workflow #383 AO-4q): its `source` is the sampled state source (the ledger's
//! committed head and the sampling interval). A refusal appends `command.rejected` (the receipt
//! terminal) plus one `runtime.failed` whose native detail keeps the provider's device error.

use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::{InstanceDiscoveryFailure, ProviderDiscoveredInstance};
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::{
    RuntimeDiscoveredInstance, RuntimeInstanceDiscovery, RuntimeObservedState,
};

const DISCOVERY_OPERATION: ContractLocation = ContractLocation::DiscoverInstances;

impl HostShared {
    pub(super) fn discover_instances(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let (state, source) = self.observe_runtime_state(|| {
            let discovery = match self.execution()?.discover_instances() {
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
                HostLocation::ReadInstanceDiscoveryRegistry,
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
                    SharedCode::InstanceDiscoveryUnavailable,
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
                HostLocation::InstanceDiscoveryObservationKind,
            )));
        };
        let discovery = RuntimeInstanceDiscovery::new(provider_version, source, instances)
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error(
                    HostLocation::InstanceDiscoverySource,
                ))
            })?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::InstancesDiscovered { discovery },
            failure_outcome: None,
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
    let stage = diagnostic.map_or(
        HostLocation::InstanceDiscoveryFailed.location(),
        |diagnostic| diagnostic.stage(),
    );
    let category = diagnostic.map_or("protocol", |diagnostic| diagnostic.category().as_str());
    let (code, runtime_code) = match failure.code {
        relayed if relayed == HostCode::MumuManagerVersionUnsupported.as_str() => (
            HostCode::MumuManagerVersionUnsupported.code(),
            RuntimeErrorCode::BackendOperationFailed,
        ),
        _ if stage == ContractLocation::InstanceDiscoveryUnsupported.location() => (
            SharedCode::InstanceDiscoveryUnavailable.code(),
            RuntimeErrorCode::InvalidRequest,
        ),
        _ => (
            SharedCode::InstanceDiscoveryUnavailable.code(),
            RuntimeErrorCode::BackendOperationFailed,
        ),
    };
    RuntimeHostError::request(code, DISCOVERY_OPERATION, runtime_code)
        .with_diagnostic_detail(DiagnosticDetailDraft::new(
            category,
            stage,
            "execution_backend_provider",
            DISCOVERY_OPERATION.as_str(),
        ))
        .with_native_detail(failure.error.to_string())
}
