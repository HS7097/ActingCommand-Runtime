// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::{Code, Location};
use actingcommand_contract::{
    GovernanceIdentityCard, GovernanceIdentityRefusal, GovernanceIdentityVerdict, GovernancePeer,
    valid_governance_declaration_origin,
};
use actingcommand_policy::MAX_APPROVAL_REFS;

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
                HostLocation::AuthorizeApprovalConnection,
            )?
            .contains(&connection_id)
        {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::GovernanceAuthorityRequired,
                    ContractLocation::RecordApprovalDecision,
                    RuntimeErrorCode::InvalidRequest,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        let _gate = lock(
            &self.governance_write_gate,
            ContractLocation::RecordApprovalDecision,
        )
        .map_err(RequestFailure::poison_without_terminal)?;
        let approvals = ApprovalProjection::recover(&self.ledger, Arc::clone(&self.state))
            .map_err(RequestFailure::poison_without_terminal)?;
        if let Some(existing) = self.client_fact_replay(request, EventType::ApprovalDecision)? {
            let EventPayload::Approval(ApprovalPayload::Decision(payload)) = existing.payload()
            else {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        HostCode::ApprovalReplayPayloadMismatch,
                        ContractLocation::RecordApprovalDecision,
                        RuntimeErrorCode::RuntimeFatal,
                    ),
                ));
            };
            if payload.decision() != decision {
                return Err(client_fact_conflict(
                    HostCode::ApprovalReplayConflict,
                    ContractLocation::RecordApprovalDecision,
                ));
            }
            return Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: Some(terminal(&existing)),
                result: RuntimeResult::ApprovalDecisionRecorded {
                    approval_id: decision.approval_id().to_owned(),
                    disposition: decision.disposition(),
                },
                failure_outcome: None,
            });
        }
        approvals
            .validate_transition(decision)
            .map_err(|error| RequestFailure::request(error, RuntimeReceiptState::Denied, None))?;
        let work = approvals
            .prepare_decision(decision)
            .map_err(RequestFailure::poison_without_terminal)?;
        let fact_gate = lock(
            &self.fact_write_gate,
            HostLocation::AppendApprovalTransaction,
        )
        .map_err(RequestFailure::poison_without_terminal)?;
        let links = validated.event_links(None, None, None);
        let draft = self
            .events
            .draft(
                EventSeverity::Info,
                EventSource::Ui,
                OriginModule::Governance,
                EventActor::User,
                links.clone(),
                ApprovalPayloadDraft::decision(decision.clone(), AuditInput::new()),
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        let draft = self
            .events
            .sanitize(draft)
            .map_err(RequestFailure::poison_without_terminal)?;
        let persisted = self
            .ledger
            .append_transaction(draft, work)
            .map_err(|error| {
                let error = crate::approval::approval_transaction_error(error);
                if error.is_fatal() {
                    RequestFailure::poison_without_terminal(error)
                } else {
                    RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                }
            })?;
        // Workflow #383 AO-7: a committed approval decision wakes the policy driver.
        self.policy_wakes.raise(persisted.event_type());
        let observed = self
            .observe_device_diagnostics_under_fact_gate(&persisted)
            .and_then(|()| self.synchronize_fact_store_under_gate());
        drop(fact_gate);
        observed
            .and_then(|()| self.observe_pipeline_event(&persisted))
            .map_err(|error| {
                let _ = error
                    .diagnostics()
                    .recorded_event()
                    .set(*persisted.event_id());
                RequestFailure::poison(error, Some(terminal(&persisted)))
            })?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&persisted)),
            result: RuntimeResult::ApprovalDecisionRecorded {
                approval_id: decision.approval_id().to_owned(),
                disposition: decision.disposition(),
            },
            failure_outcome: None,
        })
    }

    /// Workflow #191 D2: the latest decision of each requested approval id, read from the
    /// complete ledger-verified approval projection (the same recovery an approval write and
    /// policy admission run) under the governance write gate. An id absent from the map has
    /// never been decided; every failure is an `Err`, never an absence. Appends nothing and
    /// releases the gate on return, before any caller records a decision.
    pub(super) fn latest_approval_decisions(
        &self,
        approval_ids: &[String],
    ) -> RuntimeHostResult<BTreeMap<String, ApprovalDecisionRecord>> {
        if approval_ids.len() > MAX_APPROVAL_REFS {
            return Err(RuntimeHostError::request(
                HostCode::ApprovalReadIdsExceeded,
                HostLocation::ReadLatestApprovalDecisions,
                RuntimeErrorCode::InvalidRequest,
            ));
        }
        if let Some(error) = self.fatal.current()? {
            return Err(error);
        }
        // Recovery rewrites derived State rows, so it shares the gate of every other recovery.
        let _gate = lock(
            &self.governance_write_gate,
            HostLocation::ReadLatestApprovalDecisions,
        )?;
        let approvals = match ApprovalProjection::recover(&self.ledger, Arc::clone(&self.state)) {
            Ok(approvals) => approvals,
            Err(error) => {
                if error.is_fatal() {
                    self.fatal.mark(error.clone())?;
                }
                return Err(error);
            }
        };
        let mut latest = BTreeMap::new();
        for approval_id in approval_ids {
            if let Some(decision) = approvals.latest_decision(approval_id)? {
                latest.insert(approval_id.clone(), decision);
            }
        }
        Ok(latest)
    }

    /// Workflow #330 H2, #361 A: the active catalog approvals `generation` supersedes (another
    /// catalog id that was active, an older version, or the same version under another hash),
    /// read from the complete ledger-verified approval projection under the governance write
    /// gate, exactly as `latest_approval_decisions` reads. Appends nothing and releases the
    /// gate on return, before the policy driver records any revocation.
    pub(super) fn superseded_catalog_approvals(
        &self,
        generation: &CatalogGeneration,
    ) -> RuntimeHostResult<Vec<ApprovalDecisionRecord>> {
        let lineage = lock(self.policy()?, HostLocation::ReadPolicyCatalogLineage)?
            .lineage()
            .clone();
        let approvals =
            self.recover_approval_projection(HostLocation::ReadSupersededCatalogApprovals)?;
        Ok(approvals.superseded_catalog_approvals(&lineage, generation))
    }

    /// Workflow #361 C2: the approvals the policy driver records for a planned `generation`
    /// (`catalog_plan::plan_catalog_approvals`), read from the same complete ledger-verified
    /// projection. Appends nothing.
    pub(super) fn plan_catalog_approvals(
        &self,
        lineage: &crate::catalog_plan::CatalogLineage,
        generation: &CatalogGeneration,
        approval_ids: &[String],
    ) -> RuntimeHostResult<crate::catalog_plan::CatalogApprovalPlan> {
        if approval_ids.len() > MAX_APPROVAL_REFS {
            return Err(RuntimeHostError::request(
                HostCode::ApprovalReadIdsExceeded,
                HostLocation::PlanPolicyCatalogApprovals,
                RuntimeErrorCode::InvalidRequest,
            ));
        }
        let approvals =
            self.recover_approval_projection(HostLocation::PlanPolicyCatalogApprovals)?;
        crate::catalog_plan::plan_catalog_approvals(
            Some(&approvals),
            lineage,
            generation,
            approval_ids,
        )
    }

    /// The complete ledger-verified approval projection, recovered under the governance write
    /// gate (recovery rewrites derived State rows); a fatal failure poisons the Runtime.
    fn recover_approval_projection(
        &self,
        operation: impl Into<Location>,
    ) -> RuntimeHostResult<ApprovalProjection> {
        if let Some(error) = self.fatal.current()? {
            return Err(error);
        }
        let _gate = lock(&self.governance_write_gate, operation)?;
        match ApprovalProjection::recover(&self.ledger, Arc::clone(&self.state)) {
            Ok(approvals) => Ok(approvals),
            Err(error) => {
                if error.is_fatal() {
                    self.fatal.mark(error.clone())?;
                }
                Err(error)
            }
        }
    }

    /// Workflow #318 cfg4: verifies one declarative governance identity card and records it
    /// as `governance.identity_declared` with the request's actor and source. Order: card,
    /// origin (both already refused by the request envelope; repeated so this entry trusts
    /// no caller), then the policy's `allowed_clients`, then the card's instance alias, then
    /// one card per connection. A refusal is appended before its Denied receipt returns; an
    /// append failure poisons the Runtime exactly as an approval append does.
    pub(super) fn declare_governance_identity(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        connection_id: ConnectionId,
        card: &GovernanceIdentityCard,
    ) -> Result<OperationSuccess, RequestFailure> {
        if card.validate().is_err() {
            return Err(governance_identity_denied(
                SharedCode::InvalidGovernanceIdentityCard,
                None,
            ));
        }
        if !valid_governance_declaration_origin(request.actor(), request.source()) {
            return Err(governance_identity_denied(
                SharedCode::InvalidGovernanceOrigin,
                None,
            ));
        }
        let _gate = lock(
            &self.governance_write_gate,
            ContractLocation::DeclareGovernanceIdentity,
        )
        .map_err(RequestFailure::poison_without_terminal)?;
        let instance_id = match card.instance.as_deref() {
            Some(alias) => lock(
                &self.registered_instances,
                HostLocation::ResolveGovernanceIdentityInstance,
            )?
            .values()
            .find(|instance| instance.instance_alias == alias)
            .map(RegisteredInstance::instance_id),
            None => None,
        };
        let client_allowed = self
            .governance_policy
            .allowed_clients
            .as_ref()
            .is_none_or(|allowed| allowed.contains(&card.client));
        let verdict = if !client_allowed {
            GovernanceIdentityVerdict::Refused {
                code: GovernanceIdentityRefusal::ClientNotAllowed,
            }
        } else if card.instance.is_some() && instance_id.is_none() {
            GovernanceIdentityVerdict::Refused {
                code: GovernanceIdentityRefusal::InstanceUnknown,
            }
        } else if lock(
            &self.governance_connections,
            HostLocation::ReadGovernanceConnection,
        )?
        .contains(&connection_id)
        {
            GovernanceIdentityVerdict::Refused {
                code: GovernanceIdentityRefusal::AlreadyDeclared,
            }
        } else {
            GovernanceIdentityVerdict::Accepted
        };
        let severity = match verdict {
            GovernanceIdentityVerdict::Accepted => EventSeverity::Info,
            GovernanceIdentityVerdict::Refused { .. } => EventSeverity::Warning,
        };
        let persisted = self.append_event(
            severity,
            request.source(),
            OriginModule::Governance,
            request.actor(),
            validated.event_links(instance_id, None, None),
            ClientPayloadDraft::governance_identity_declared(
                card.clone(),
                GovernancePeer::Loopback,
                verdict,
                AuditInput::new(),
            ),
        )?;
        if let GovernanceIdentityVerdict::Refused { code } = verdict {
            return Err(governance_identity_denied(code, Some(terminal(&persisted))));
        }
        if !lock(
            &self.governance_connections,
            HostLocation::DeclareGovernanceConnection,
        )?
        .insert(connection_id)
        {
            return Err(RequestFailure::poison(
                RuntimeHostError::fatal(
                    HostCode::GovernanceConnectionStateConflict,
                    ContractLocation::DeclareGovernanceIdentity,
                    RuntimeErrorCode::RuntimeFatal,
                ),
                Some(terminal(&persisted)),
            ));
        }
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&persisted)),
            result: RuntimeResult::GovernanceIdentityAccepted,
            failure_outcome: None,
        })
    }
}

fn governance_identity_denied(
    code: impl Into<Code>,
    terminal: Option<TerminalEvent>,
) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(
            code,
            ContractLocation::DeclareGovernanceIdentity,
            RuntimeErrorCode::InvalidRequest,
        ),
        RuntimeReceiptState::Denied,
        terminal,
    )
}
