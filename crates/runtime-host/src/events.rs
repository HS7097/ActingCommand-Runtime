// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeHostError, RuntimeHostResult, time::unix_ms_now};
use actingcommand_contract::{
    ActionId, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventPayloadDraft,
    EventSeverity, EventSource, IdentifierIssuer, InstanceId, LeaseId, LeaseToken, OriginModule,
    RuntimeErrorCode, RuntimeOperation, RuntimeRequest, SanitizedEventDraft,
    ValidatedRuntimeRequest,
};
use actingcommand_ledger::Sha256SecretFingerprinter;

pub(crate) struct RuntimeEvents {
    issuer: IdentifierIssuer,
    fingerprinter: Sha256SecretFingerprinter,
}

impl RuntimeEvents {
    pub(crate) fn new(secret_fingerprint_salt: &[u8]) -> RuntimeHostResult<Self> {
        let issuer = IdentifierIssuer::new().map_err(|_| {
            RuntimeHostError::fatal(
                "event_issuer_failed",
                "initialize_runtime_events",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let fingerprinter =
            Sha256SecretFingerprinter::new(secret_fingerprint_salt).map_err(|_| {
                RuntimeHostError::fatal(
                    "event_fingerprinter_failed",
                    "initialize_runtime_events",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
        Ok(Self {
            issuer,
            fingerprinter,
        })
    }

    pub(crate) const fn issuer(&self) -> &IdentifierIssuer {
        &self.issuer
    }

    pub(crate) const fn fingerprinter(&self) -> &Sha256SecretFingerprinter {
        &self.fingerprinter
    }

    pub(crate) fn action_id(&self) -> RuntimeHostResult<ActionId> {
        self.issuer
            .mint_action_id()
            .map(|value| *value.transport())
            .map_err(|_| {
                RuntimeHostError::fatal(
                    "action_id_issue_failed",
                    "build_runtime_event",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })
    }

    pub(crate) fn request_links(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: Option<InstanceId>,
        lease_id: Option<LeaseId>,
        action_id: Option<ActionId>,
    ) -> EventLinksDraft {
        request.event_links(instance_id, lease_id, action_id)
    }

    pub(crate) fn synthetic_links(
        &self,
        token: &LeaseToken,
        action_id: ActionId,
    ) -> RuntimeHostResult<EventLinksDraft> {
        let request_id = self.issuer.mint_request_id().map_err(|_| id_error())?;
        let correlation_id = self.issuer.mint_correlation_id().map_err(|_| id_error())?;
        let request = RuntimeRequest::new(
            request_id,
            correlation_id,
            None,
            EventActor::Agent,
            EventSource::Adapter,
            unix_ms_now()?,
            RuntimeOperation::ReleaseLease {
                token: token.clone(),
            },
        )
        .map_err(|_| {
            RuntimeHostError::fatal(
                "synthetic_request_invalid",
                "build_runtime_event",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let validated = request.validate().map_err(|_| {
            RuntimeHostError::fatal(
                "synthetic_request_invalid",
                "build_runtime_event",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        Ok(validated.event_links(
            Some(token.instance_id()),
            Some(token.lease_id()),
            Some(action_id),
        ))
    }

    pub(crate) fn draft(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        payload: impl Into<EventPayloadDraft>,
    ) -> RuntimeHostResult<EventDraft> {
        let event_id = self.issuer.mint_event_id().map_err(|_| id_error())?;
        Ok(EventDraft::new(
            event_id,
            unix_ms_now()?,
            severity,
            EventOrigin::new(source, module, actor),
            links,
            payload.into(),
        ))
    }

    pub(crate) fn sanitize(&self, draft: EventDraft) -> RuntimeHostResult<SanitizedEventDraft> {
        draft.sanitize(&self.fingerprinter).map_err(|_| {
            RuntimeHostError::fatal(
                "event_sanitization_failed",
                "sanitize_runtime_event",
                RuntimeErrorCode::LedgerFailure,
            )
        })
    }
}

fn id_error() -> RuntimeHostError {
    RuntimeHostError::fatal(
        "event_id_issue_failed",
        "build_runtime_event",
        RuntimeErrorCode::RuntimeFatal,
    )
}
