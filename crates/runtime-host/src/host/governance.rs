// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
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
        let work = approvals
            .prepare_decision(decision)
            .map_err(RequestFailure::poison_without_terminal)?;
        let fact_gate = lock(&self.fact_write_gate, "append_approval_transaction")
            .map_err(RequestFailure::poison_without_terminal)?;
        if self.lifecycle_append_failed.load(Ordering::Acquire) {
            return Err(RequestFailure::poison_without_terminal(ledger_error(
                "append_approval_transaction",
            )));
        }
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
                    self.lifecycle_append_failed.store(true, Ordering::Release);
                    RequestFailure::poison_without_terminal(error)
                } else {
                    RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                }
            })?;
        let observed = self
            .observe_device_diagnostics_under_fact_gate(&persisted)
            .and_then(|()| self.synchronize_fact_store_under_gate());
        drop(fact_gate);
        observed
            .and_then(|()| self.observe_pipeline_event(&persisted))
            .map_err(|error| {
                self.lifecycle_append_failed.store(true, Ordering::Release);
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
                "approval_read_ids_exceeded",
                "read_latest_approval_decisions",
                RuntimeErrorCode::InvalidRequest,
            ));
        }
        if let Some(error) = self.fatal.current()? {
            return Err(error);
        }
        // Recovery rewrites derived State rows, so it shares the gate of every other recovery.
        let _gate = lock(
            &self.governance_write_gate,
            "read_latest_approval_decisions",
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
        let lineage = lock(self.policy()?, "read_policy_catalog_lineage")?
            .lineage()
            .clone();
        let approvals = self.recover_approval_projection("read_superseded_catalog_approvals")?;
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
                "approval_read_ids_exceeded",
                "plan_policy_catalog_approvals",
                RuntimeErrorCode::InvalidRequest,
            ));
        }
        let approvals = self.recover_approval_projection("plan_policy_catalog_approvals")?;
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
        operation: &'static str,
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
                "invalid_governance_identity_card",
                None,
            ));
        }
        if !valid_governance_declaration_origin(request.actor(), request.source()) {
            return Err(governance_identity_denied(
                "invalid_governance_origin",
                None,
            ));
        }
        let _gate = lock(&self.governance_write_gate, "declare_governance_identity")
            .map_err(RequestFailure::poison_without_terminal)?;
        let instance_id = match card.instance.as_deref() {
            Some(alias) => lock(
                &self.registered_instances,
                "resolve_governance_identity_instance",
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
        } else if lock(&self.governance_connections, "read_governance_connection")?
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
            return Err(governance_identity_denied(
                code.as_str(),
                Some(terminal(&persisted)),
            ));
        }
        if !lock(
            &self.governance_connections,
            "declare_governance_connection",
        )?
        .insert(connection_id)
        {
            return Err(RequestFailure::poison(
                RuntimeHostError::fatal(
                    "governance_connection_state_conflict",
                    "declare_governance_identity",
                    RuntimeErrorCode::RuntimeFatal,
                ),
                Some(terminal(&persisted)),
            ));
        }
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&persisted)),
            result: RuntimeResult::GovernanceIdentityAccepted,
        })
    }
}

fn governance_identity_denied(
    code: &'static str,
    terminal: Option<TerminalEvent>,
) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(
            code,
            "declare_governance_identity",
            RuntimeErrorCode::InvalidRequest,
        ),
        RuntimeReceiptState::Denied,
        terminal,
    )
}
