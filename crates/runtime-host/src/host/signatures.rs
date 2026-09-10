// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_artifact_store::verify_projected_read_only;
use actingcommand_contract::{
    DiagnosticSignatureDefinition, LedgerPayloadDraft, LedgerSignatureEvent,
    RuntimeSignatureMatchRequest, SignatureRegistrationRef,
};
use actingcommand_ledger::signatures::{
    SignatureCatalog, SignaturePrefix, registration_ref, replay_signatures,
};
use actingcommand_ledger::{GlobalLedgerError, GlobalLedgerEvidenceConfig};

impl HostShared {
    fn current_signature_catalog(&self) -> Result<SignatureCatalog, RequestFailure> {
        let through = self
            .ledger
            .latest_sequence()
            .map_err(signature_ledger_error)?;
        let prefix =
            SignaturePrefix::from_live(&self.ledger, through).map_err(signature_ledger_error)?;
        Ok(SignatureCatalog::from_prefix(&prefix))
    }

    pub(super) fn register_signature(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        definition: &DiagnosticSignatureDefinition,
    ) -> Result<OperationSuccess, RequestFailure> {
        let _gate = lock(&self.signature_write_gate, "register_diagnostic_signature")?;
        let catalog = self.current_signature_catalog()?;
        catalog
            .validate_registration(definition)
            .map_err(signature_ledger_error)?;
        let event = self.append_event(
            EventSeverity::Info,
            EventSource::Lab,
            OriginModule::GlobalLedger,
            EventActor::Lab,
            validated.event_links(None, None, None),
            LedgerPayloadDraft::signature(
                LedgerSignatureEvent::Registered {
                    definition: definition.clone(),
                },
                AuditInput::new(),
            ),
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&event)),
            result: RuntimeResult::SignatureRegistered {
                registration: registration_ref(&event, definition),
            },
        })
    }

    pub(super) fn retire_signature(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        registration: &SignatureRegistrationRef,
    ) -> Result<OperationSuccess, RequestFailure> {
        let _gate = lock(&self.signature_write_gate, "retire_diagnostic_signature")?;
        let catalog = self.current_signature_catalog()?;
        catalog
            .validate_retirement(registration)
            .map_err(signature_ledger_error)?;
        let event = self.append_event(
            EventSeverity::Info,
            EventSource::Lab,
            OriginModule::GlobalLedger,
            EventActor::Lab,
            validated.event_links(None, None, None),
            LedgerPayloadDraft::signature(
                LedgerSignatureEvent::Retired {
                    registration: registration.clone(),
                },
                AuditInput::new(),
            ),
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&event)),
            result: RuntimeResult::SignatureRetired {
                registration: registration.clone(),
            },
        })
    }

    pub(super) fn match_signatures(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        request: &RuntimeSignatureMatchRequest,
    ) -> Result<OperationSuccess, RequestFailure> {
        let catalog_prefix = SignaturePrefix::from_live(&self.ledger, request.catalog_through)
            .map_err(signature_ledger_error)?;
        let catalog = SignatureCatalog::from_prefix(&catalog_prefix);
        let root = Path::new(&request.input_state_root);
        let snapshot =
            GlobalLedger::open_evidence(GlobalLedgerEvidenceConfig::new(root), |reference| {
                verify_projected_read_only(root, reference).ok()
            })
            .map_err(|error| signature_request_error(error.code()))?;
        let input = SignaturePrefix::from_evidence(&snapshot, request.input_through)
            .map_err(|error| signature_request_error(error.code()))?;
        let page =
            replay_signatures(&input, &catalog, &request.page).map_err(signature_ledger_error)?;
        let event = self.append_event(
            if page.evidence_complete() {
                EventSeverity::Info
            } else {
                EventSeverity::Warning
            },
            EventSource::Lab,
            OriginModule::GlobalLedger,
            EventActor::Lab,
            validated.event_links(None, None, None),
            LedgerPayloadDraft::signature(
                LedgerSignatureEvent::Matched {
                    page: Box::new(page.clone()),
                },
                AuditInput::new(),
            ),
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&event)),
            result: RuntimeResult::SignaturesMatched {
                page: Box::new(page),
            },
        })
    }
}

fn signature_ledger_error(error: GlobalLedgerError) -> RequestFailure {
    if error.is_fatal() {
        RequestFailure::poison_without_terminal(ledger_error("diagnostic_signatures"))
    } else {
        signature_request_error(error.code())
    }
}

fn signature_request_error(code: &'static str) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(
            code,
            "diagnostic_signatures",
            RuntimeErrorCode::InvalidRequest,
        ),
        RuntimeReceiptState::Denied,
        None,
    )
}
