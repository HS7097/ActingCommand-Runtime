// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::HostLocation;
use crate::events::RuntimeEvents;
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    EventActor, EventLinksDraft, EventSeverity, EventSource, OriginModule, OwnerEpoch,
    ProviderBackend, ProviderNativeFailure, ProviderPayloadDraft, ProviderStartupObservation,
    ProviderStartupRecord, ProviderStartupStage, RuntimeErrorCode,
};
use actingcommand_ledger::GlobalLedger;

/// A synchronous, startup-only view of the Host's existing ledger authority.
pub struct ProviderStartup<'a> {
    pub(crate) ledger: &'a GlobalLedger,
    pub(crate) events: &'a RuntimeEvents,
    pub(crate) owner_epoch: OwnerEpoch,
    pub(crate) links: EventLinksDraft,
    pub(crate) preparation_boundary: &'a dyn Fn() -> RuntimeHostResult<()>,
}

impl ProviderStartup<'_> {
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
