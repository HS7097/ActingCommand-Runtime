// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::HostLocation;
use crate::events::RuntimeEvents;
use crate::{RuntimeClock, RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    EventActor, EventLinksDraft, EventSeverity, EventSource, OriginModule, OwnerEpoch,
    ProviderBackend, ProviderNativeFailure, ProviderPayloadDraft, ProviderStartupObservation,
    ProviderStartupRecord, ProviderStartupStage, RuntimeErrorCode,
};
use actingcommand_ledger::GlobalLedger;
use actingcommand_runtime_config::StartupDiscoveryConfig;
use std::time::Duration;

/// A synchronous, startup-only view of the Host's existing ledger authority.
pub struct ProviderStartup<'a> {
    pub(crate) ledger: &'a GlobalLedger,
    pub(crate) events: &'a RuntimeEvents,
    pub(crate) owner_epoch: OwnerEpoch,
    pub(crate) links: EventLinksDraft,
    pub(crate) preparation_boundary: &'a dyn Fn() -> RuntimeHostResult<()>,
    /// Workflow #390 DISC-1: the Runtime clock, whose waits a test clock counts instead.
    pub(crate) clock: &'a dyn RuntimeClock,
    /// Workflow #390 DISC-1: the `startup_discovery` section in effect.
    pub(crate) startup_discovery: StartupDiscoveryConfig,
}

impl ProviderStartup<'_> {
    /// Workflow #390 DISC-1: how often the startup discovery is tried while it is unavailable
    /// and the wait between two attempts (`startup_discovery.*`, the configuration in effect,
    /// or the defaults file's for a host started without a manifest).
    pub const fn startup_discovery(&self) -> StartupDiscoveryConfig {
        self.startup_discovery
    }

    /// Workflow #390 DISC-1: a wait of the provider startup on the Runtime clock (a test clock
    /// moves itself forward by `duration` instead), then the install preparation boundary, so a
    /// held start whose release ran out ends under its own code.
    pub fn pause(&mut self, duration: Duration) -> RuntimeHostResult<()> {
        self.clock.pause(duration);
        (self.preparation_boundary)()
    }

    pub fn record(
        &mut self,
        backend: ProviderBackend,
        observation: ProviderStartupObservation,
    ) -> RuntimeHostResult<()> {
        if matches!(observation, ProviderStartupObservation::Started { .. }) {
            (self.preparation_boundary)()?;
        }
        let severity = if matches!(observation, ProviderStartupObservation::Failed { .. }) {
            EventSeverity::Fatal
        } else {
            EventSeverity::Info
        };
        let draft = self.events.draft(
            severity,
            EventSource::Runtime,
            OriginModule::Provider,
            EventActor::Runtime,
            self.links.clone(),
            ProviderPayloadDraft::observed(ProviderStartupRecord {
                owner_epoch: self.owner_epoch,
                backend,
                observation,
            }),
        )?;
        self.ledger
            .append(self.events.sanitize(draft)?)
            .map(|_| ())
            .map_err(|_| {
                RuntimeHostError::fatal(
                    RuntimeErrorCode::LedgerFailure,
                    HostLocation::AppendProviderStartup,
                    RuntimeErrorCode::RuntimeHalted,
                )
            })
    }

    pub fn failed(
        &mut self,
        backend: ProviderBackend,
        stage: ProviderStartupStage,
        code: &'static str,
        failure: ProviderNativeFailure,
    ) -> RuntimeHostError {
        match self.record(
            backend,
            ProviderStartupObservation::Failed { stage, failure },
        ) {
            Ok(()) => RuntimeHostError::relayed_fatal(
                code,
                HostLocation::AssembleProvider.as_str(),
                RuntimeErrorCode::RuntimeHalted,
            ),
            Err(error) => error,
        }
    }
}
