// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::ChannelBinding;
use actingcommand_contract::codes::RuntimeContractCode;
use actingcommand_contract::outcome::Location;
use actingcommand_policy::MAX_APPROVAL_REFS;

impl HostShared {
    /// Workflow #389 U-2 (model section 3.2, "actingd's own connections"): actingd's start-up
    /// transcription of the approvals its configuration holds, in process instead of over a
    /// (User, Ui) connection with an identity card. It writes the same approval record through
    /// the same joint capture (G15), under the governance write gate and after the same replay
    /// of the approval projection; the record's provenance stays (Governance, User, Ui), which
    /// the projection requires of every approval record. The record links a request and a
    /// correlation the host mints, since no client request carries it.
    pub(super) fn transcribe_configured_approval(
        &self,
        decision: &ApprovalDecisionRecord,
    ) -> RuntimeHostResult<()> {
        if let Some(error) = self.fatal.answer()? {
            return Err(error);
        }
        match self.transcribe_under_gate(decision) {
            // Workflow #389 MCP-9a (register A5): `config.catalog` follows the decision, once the
            // governance write gate is released.
            Ok(_) => self.record_config_catalog(),
            Err(failure) => {
                if failure.poison_runtime {
                    self.fatal.mark((*failure.error).clone())?;
                }
                Err(*failure.error)
            }
        }
    }

    fn transcribe_under_gate(
        &self,
        decision: &ApprovalDecisionRecord,
    ) -> Result<PersistedEvent, RequestFailure> {
        let _gate = lock(
            &self.governance_write_gate,
            HostLocation::RecordConfiguredApproval,
        )
        .map_err(RequestFailure::poison_without_terminal)?;
        let approvals = ApprovalProjection::recover(&self.ledger, Arc::clone(&self.state))
            .map_err(RequestFailure::poison_without_terminal)?;
        let links = self
            .events
            .minted_request_links()
            .map_err(RequestFailure::poison_without_terminal)?;
        self.append_approval_decision(&approvals, links, decision)
    }

    /// The approval write of `decision` under the governance write gate the caller holds, after
    /// `approvals` was replayed: its transition is checked, then the record and its state work
    /// are appended as one joint capture (G15) with the provenance (Governance, User, Ui).
    fn append_approval_decision(
        &self,
        approvals: &ApprovalProjection,
        links: EventLinksDraft,
        decision: &ApprovalDecisionRecord,
    ) -> Result<PersistedEvent, RequestFailure> {
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
        let draft = self
            .events
            .draft(
                EventSeverity::Info,
                EventSource::Ui,
                OriginModule::Governance,
                EventActor::User,
                links,
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
        Ok(persisted)
    }

    /// Workflow #389 U-2 (model section 3.2): binds the connection to `binding`, once. A second
    /// `BindChannel` on the connection is refused with `channel_binding_exists`, naming the
    /// channel it keeps; nothing is recorded either way (the handshake is not copied by G14).
    /// Workflow #434 LAB-6S1a: the binding keeps the initiator a `lab` connection declared, the
    /// answer echoes it, and the entry leaves with its connection; nothing reads it but the
    /// receipts of the connection. `None` is a request the shape check refused before any handler
    /// runs (`invalid_bind_channel`), refused the same way here and leaving the connection
    /// unbound.
    pub(super) fn bind_channel(
        &self,
        connection_id: ConnectionId,
        binding: Option<ChannelBinding>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let Some(binding) = binding else {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    RuntimeContractCode::InvalidBindChannel,
                    HostLocation::BindChannel,
                    RuntimeErrorCode::InvalidRequest,
                ),
                RuntimeReceiptState::Denied,
                None,
            ));
        };
        let mut channels = lock(&self.connection_channels, HostLocation::BindChannel)?;
        if let Some(bound) = channels.get(&connection_id) {
            let kept = bound.channel;
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::ChannelBindingExists,
                    HostLocation::BindChannel,
                    RuntimeErrorCode::InvalidRequest,
                )
                .with_outcome_values(|outcome| outcome.channel(kept)),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        channels.insert(connection_id, binding);
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::ChannelBound {
                channel: binding.channel,
                via: binding.via,
                initiator: binding.initiator,
            },
            failure_outcome: None,
        })
    }

    /// Workflow #389 U-2: a closed connection's binding leaves with it.
    pub(super) fn forget_connection_channel(
        &self,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<()> {
        lock(
            &self.connection_channels,
            HostLocation::CleanupConnectionChannel,
        )?
        .remove(&connection_id);
        Ok(())
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
        if let Some(error) = self.fatal.answer()? {
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
        if let Some(error) = self.fatal.answer()? {
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
}
