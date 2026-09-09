// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

impl HostShared {
    pub(super) fn record_approval_decision(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        decision: &ApprovalDecisionRecord,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        if request.actor() != EventActor::User
            || request.source() != EventSource::Ui
            || !lock(
                &self.governance_connections,
                "authorize_approval_connection",
            )?
            .contains(&connection_id)
        {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    "governance_authority_required",
                    "record_approval_decision",
                    RuntimeErrorCode::InvalidRequest,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        let _gate = lock(&self.governance_write_gate, "record_approval_decision")
            .map_err(RequestFailure::poison_without_terminal)?;
        let approvals = ApprovalProjection::recover(&self.ledger, Arc::clone(&self.state))
            .map_err(RequestFailure::poison_without_terminal)?;
        if let Some(existing) = self.client_fact_replay(request, EventType::ApprovalDecision)? {
            let EventPayload::Approval(ApprovalPayload::Decision(payload)) = existing.payload()
            else {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        "approval_replay_payload_mismatch",
                        "record_approval_decision",
                        RuntimeErrorCode::RuntimeFatal,
                    ),
                ));
            };
            if payload.decision() != decision {
                return Err(client_fact_conflict(
                    "approval_replay_conflict",
                    "record_approval_decision",
                ));
            }
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: Some(terminal(&existing)),
                result: RuntimeResult::ApprovalDecisionRecorded {
                    approval_id: decision.approval_id().to_owned(),
                    disposition: decision.disposition(),
                },
            });
        }
        approvals
            .validate_transition(decision)
            .map_err(|error| RequestFailure::request(error, RuntimeReceiptState::Denied, None))?;
        let persisted = self.append_event(
            EventSeverity::Info,
            EventSource::Ui,
            OriginModule::Governance,
            EventActor::User,
            validated.event_links(None, None, None),
            ApprovalPayloadDraft::decision(decision.clone(), AuditInput::new()),
        )?;
        ApprovalProjection::recover(&self.ledger, Arc::clone(&self.state))
            .map_err(RequestFailure::poison_without_terminal)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&persisted)),
            result: RuntimeResult::ApprovalDecisionRecorded {
                approval_id: decision.approval_id().to_owned(),
                disposition: decision.disposition(),
            },
        })
    }

    pub(super) fn authenticate_governance(
        &self,
        request: &RuntimeRequest,
        connection_id: ConnectionId,
        capability: &str,
    ) -> Result<OperationSuccess, RequestFailure> {
        if request.actor() != EventActor::User || request.source() != EventSource::Ui {
            return Err(governance_authentication_denied(
                "governance_origin_untrusted",
            ));
        }
        let expected = self.governance_capability_sha256.ok_or_else(|| {
            governance_authentication_denied("governance_authentication_unavailable")
        })?;
        let actual: [u8; 32] = Sha256::digest(capability.as_bytes()).into();
        if !constant_time_digest_eq(&expected, &actual) {
            return Err(governance_authentication_denied(
                "governance_authentication_failed",
            ));
        }
        lock(
            &self.governance_connections,
            "authenticate_governance_connection",
        )?
        .insert(connection_id);
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::GovernanceAuthenticated,
        })
    }
}

fn governance_authentication_denied(code: &'static str) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(
            code,
            "authenticate_governance",
            RuntimeErrorCode::InvalidRequest,
        ),
        RuntimeReceiptState::Denied,
        None,
    )
}

fn constant_time_digest_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}
