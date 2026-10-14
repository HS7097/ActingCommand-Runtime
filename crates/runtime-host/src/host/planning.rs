// SPDX-License-Identifier: AGPL-3.0-only

use super::agent_control::append_agent_wake;
use super::*;
use crate::codes::HostCode;
use crate::codes::HostLocation;
use actingcommand_contract::codes::{ContractLocation, RuntimeContractCode, SharedCode};
use actingcommand_contract::outcome::{Location, Outcome};

/// Every `ArtifactVerified` reference read so far, in ledger order (Workflow #317 item C).
/// The first read after start reads every verified event; each later read starts after the
/// last verified event already seen.
#[derive(Default)]
pub(super) struct VerifiedArtifactIndex {
    through: u64,
    references: Vec<(u64, ProjectedArtifactReference)>,
}

impl HostShared {
    /// Brings the verified artifact index up to the ledger tail and hands `read` every
    /// verified reference with its event sequence, in ledger order.
    fn read_verified_artifacts<T>(
        &self,
        operation: impl Into<Location>,
        read: impl FnOnce(&[(u64, ProjectedArtifactReference)]) -> T,
    ) -> RuntimeHostResult<T> {
        let operation: Location = operation.into();
        let mut index = lock(&self.verified_artifacts, operation)?;
        let events = self
            .ledger
            .query(EventQuery {
                event_type: Some(EventType::ArtifactVerified),
                from_sequence: (index.through != 0).then_some(index.through + 1),
                ..EventQuery::default()
            })
            .map_err(|_| ledger_error(operation))?;
        for event in &events {
            index.references.extend(
                event
                    .artifacts()
                    .iter()
                    .map(|artifact| (event.sequence(), artifact.project(true))),
            );
            index.through = event.sequence();
        }
        Ok(read(&index.references))
    }

    pub(super) fn record_policy_planning_signal(
        &self,
        signal: PolicyPlanningSignalEventData,
    ) -> RuntimeHostResult<()> {
        let mut committed_signal = None;
        let result: RuntimeHostResult<()> = (|| {
            let mut policy = lock(self.policy()?, HostLocation::RecordPolicyPlanningSignal)?;
            policy.validate_planning_signal(&signal)?;
            if let Some(existing) = policy.planning_signal(&signal.signal_id)? {
                return if existing == signal {
                    Ok(())
                } else {
                    Err(RuntimeHostError::fatal(
                        SharedCode::PolicyPlanningSignalIdentityConflict,
                        HostLocation::RecordPolicyPlanningSignal,
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                };
            }
            let (work, staged_quota) = policy.prepare_planning_signal(&signal)?;
            let fact_gate = lock(
                &self.fact_write_gate,
                HostLocation::AppendPlanningTransaction,
            )?;
            let links = self.events.system_links()?;
            let draft = self.events.draft(
                EventSeverity::Info,
                EventSource::Scheduler,
                OriginModule::Policy,
                EventActor::Scheduler,
                links.clone(),
                PolicyPayloadDraft::planning_signal_observed(signal.clone(), AuditInput::new()),
            )?;
            let draft = self.events.sanitize(draft)?;
            let attempt_event_id = *draft.event_id();
            let persisted = self
                .ledger
                .append_transaction(draft, Box::new(work))
                .map_err(|error| {
                    let error = crate::policy_host::planning_transaction_error(error);
                    let context = error.clone().with_native_detail(format!(
                        "event_id={attempt_event_id:?}; attempted_sequence={:?}",
                        staged_quota.attempted_sequence()
                    ));
                    error.with_related_failure("planning_attempt", &context)
                })?;
            committed_signal = Some((*persisted.event_id(), persisted.sequence()));
            policy.publish_planning_signal(staged_quota);
            drop(policy);
            let observed = self
                .observe_device_diagnostics_under_fact_gate(&persisted)
                .and_then(|()| self.synchronize_fact_store_under_gate());
            drop(fact_gate);
            observed.and_then(|()| self.observe_pipeline_event(&persisted))?;
            let Some(config) = &self.agent_dispatcher_config else {
                return Ok(());
            };
            let kind = match signal.kind {
                actingcommand_contract::PolicyPlanningSignalKind::TimelineReached => {
                    AgentWakeKind::TimelineReached
                }
                actingcommand_contract::PolicyPlanningSignalKind::DriftPredicted => {
                    AgentWakeKind::DriftPredicted
                }
                _ => return Ok(()),
            };
            let instance_id = lock(
                &self.registered_instances,
                HostLocation::ResolveAgentWakeInstance,
            )?
            .values()
            .find(|instance| instance.instance_alias == signal.instance_id)
            .map(|instance| instance.instance_id)
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    HostCode::AgentWakeInstanceUnknown,
                    HostLocation::RecordPolicyPlanningSignal,
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            let _gate = lock(&self.agent_write_gate, HostLocation::RecordAgentWake)?;
            let mut agent = lock(&self.agent_dispatcher, HostLocation::RecordAgentWake)?;
            if !agent.has_wake_for_trigger(persisted.event_id()) {
                append_agent_wake(
                    &mut agent,
                    &self.ledger,
                    &self.events,
                    config,
                    &persisted,
                    instance_id,
                    kind,
                )?;
            }
            Ok(())
        })()
        .map_err(|error| {
            if let Some((event_id, sequence)) = committed_signal {
                let context = error.clone().with_native_detail(format!(
                    "planning_fact_committed=true; event_id={event_id:?}; sequence={sequence}"
                ));
                let error = error
                    .into_fatal()
                    .with_related_failure("committed_planning_fact", &context);
                let _ = error.diagnostics().recorded_event().set(event_id);
                error
            } else {
                error
            }
        });
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone()).map_err(|secondary| {
                error
                    .clone()
                    .with_related_failure("fatal_mark", &secondary)
                    .with_recording_from(error)
            })?;
        }
        result
    }

    pub(super) fn prepare_strategic_report_ipc(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        report: &RuntimePlanningDocument,
        evidence: &[ProjectedArtifactReference],
    ) -> Result<OperationSuccess, RequestFailure> {
        let report: StrategicReport = decode_planning_document(
            report,
            RuntimePlanningDocumentKind::StrategicReport,
            ContractLocation::PrepareStrategicReport,
        )?;
        let links = validated.event_links(None, None, None);
        let entered = self
            .append_lifecycle_observed(RuntimeLifecyclePhase::StrategicReportEntered, links.clone())
            .map_err(RequestFailure::poison_without_terminal)?;
        let result = self
            .prepare_strategic_report_with(&report, evidence, |_, transport_plan| {
                Ok(transport_plan)
            })
            .map(|plan| OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: None,
                result: RuntimeResult::StrategicPlanPrepared {
                    plan: Box::new(plan),
                },
                failure_outcome: None,
            })
            .map_err(planning_request_failure);
        self.record_planning_result(
            &result,
            links,
            RuntimeLifecycleFailureStage::StrategicReport,
            entered,
            RuntimeLifecyclePhase::StrategicReportReturned {
                entered_event_id: entered,
            },
        )?;
        result
    }

    pub(super) fn project_policy_forward_ipc(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        request: &RuntimeForwardProjectionRequest,
    ) -> Result<OperationSuccess, RequestFailure> {
        let facts: EvaluationFacts = decode_planning_document(
            request.facts(),
            RuntimePlanningDocumentKind::EvaluationFacts,
            ContractLocation::ProjectPolicyForward,
        )?;
        let resources: EvaluationResources = decode_planning_document(
            request.resources(),
            RuntimePlanningDocumentKind::EvaluationResources,
            ContractLocation::ProjectPolicyForward,
        )?;
        let time: EvaluationTime = decode_planning_document(
            request.time(),
            RuntimePlanningDocumentKind::EvaluationTime,
            ContractLocation::ProjectPolicyForward,
        )?;
        let config: ForwardProjectionConfig = decode_planning_document(
            request.config(),
            RuntimePlanningDocumentKind::ForwardProjectionConfig,
            ContractLocation::ProjectPolicyForward,
        )?;
        let links = validated.event_links(None, None, None);
        let entered = self
            .append_lifecycle_observed(RuntimeLifecyclePhase::PolicyForwardEntered, links.clone())
            .map_err(RequestFailure::poison_without_terminal)?;
        let result = (|| {
            let projection = self
                .project_policy_forward(&facts, &resources, time, request.seed(), config)
                .map_err(planning_request_failure)?;
            let projection = encode_planning_document(
                RuntimePlanningDocumentKind::ForwardProjection,
                &projection,
                ContractLocation::ProjectPolicyForward,
            )
            .map_err(planning_request_failure)?;
            Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: None,
                result: RuntimeResult::PolicyForwardProjected {
                    projection: Box::new(projection),
                },
                failure_outcome: None,
            })
        })();
        self.record_planning_result(
            &result,
            links,
            RuntimeLifecycleFailureStage::PolicyForward,
            entered,
            RuntimeLifecyclePhase::PolicyForwardReturned {
                entered_event_id: entered,
            },
        )?;
        result
    }

    fn record_planning_result(
        &self,
        result: &Result<OperationSuccess, RequestFailure>,
        links: EventLinksDraft,
        stage: RuntimeLifecycleFailureStage,
        entered_event_id: EventId,
        returned: RuntimeLifecyclePhase,
    ) -> Result<(), RequestFailure> {
        match result {
            Ok(_) => self.append_lifecycle_observed(returned, links).map(|_| ()),
            Err(failure) => self.append_lifecycle_failure(
                stage,
                RuntimeLifecycleFailure::Host(&failure.error),
                links,
                Some(entered_event_id),
            ),
        }
        .map_err(RequestFailure::poison_without_terminal)
    }

    pub(super) fn assess_predictive_maintenance_ipc(
        &self,
        query: &RuntimeMaintenanceQuery,
    ) -> Result<OperationSuccess, RequestFailure> {
        let trend_policy: MaintenanceTrendPolicy = decode_planning_document(
            query.trend_policy(),
            RuntimePlanningDocumentKind::MaintenanceTrendPolicy,
            ContractLocation::AssessPredictiveMaintenance,
        )?;
        let query = MaintenanceLedgerQuery::new(
            query.instance_id(),
            query.task_id(),
            query.fact_scope().clone(),
            query.fact_key(),
            query.as_of_ledger_position(),
            query.as_of_unix_ms(),
            trend_policy,
        )
        .map_err(planning_request_failure)?;
        let assessment = self
            .assess_and_publish_predictive_maintenance(&query)
            .map_err(planning_request_failure)?;
        let assessment = encode_planning_document(
            RuntimePlanningDocumentKind::MaintenanceAssessmentV2,
            &assessment,
            ContractLocation::AssessPredictiveMaintenance,
        )
        .map_err(planning_request_failure)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::PredictiveMaintenanceAssessed {
                assessment: Box::new(assessment),
            },
            failure_outcome: None,
        })
    }

    pub(super) fn compile_proposal(
        &self,
        proposal: &CatalogProposal,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.verify_proposal_reports(proposal)?;
        let prepared = {
            let policy = lock(self.policy()?, ContractLocation::CompileProposal)?;
            let generation = policy.active_generation().ok_or_else(|| {
                proposal_request_failure(RuntimeHostError::request(
                    HostCode::ProposalBaseCatalogUnavailable,
                    ContractLocation::CompileProposal,
                    RuntimeErrorCode::InvalidRequest,
                ))
            })?;
            let sources = policy.active_sources().ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    HostCode::ProposalBaseSourcesUnavailable,
                    ContractLocation::CompileProposal,
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
            prepare_proposal(&generation, &sources, proposal).map_err(proposal_request_failure)?
        };
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::ProposalEvaluated {
                preview: prepared.preview().clone(),
            },
            failure_outcome: None,
        })
    }

    pub(super) fn promote_proposal(
        &self,
        proposal: &CatalogProposal,
    ) -> Result<OperationSuccess, RequestFailure> {
        let _gate = lock(&self.proposal_write_gate, ContractLocation::PromoteProposal)
            .map_err(RequestFailure::poison_without_terminal)?;
        self.verify_proposal_reports(proposal)?;
        let (prepared, current) = {
            let policy = lock(self.policy()?, HostLocation::PrepareProposalPromotion)?;
            let base = policy
                .load_generation(proposal.base_catalog_hash())
                .map_err(proposal_request_failure)?;
            let prepared = prepare_proposal(base.generation(), base.sources(), proposal)
                .map_err(proposal_request_failure)?;
            (prepared, policy.active_generation())
        };
        let (preview, sources) = prepared.into_ready().map_err(proposal_request_failure)?;
        let target_hash = preview.target_catalog_hash().ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::ProposalTargetHashMissing,
                ContractLocation::PromoteProposal,
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let current = current.ok_or_else(|| {
            proposal_request_failure(RuntimeHostError::request(
                HostCode::ProposalActiveCatalogUnavailable,
                ContractLocation::PromoteProposal,
                RuntimeErrorCode::InvalidRequest,
            ))
        })?;
        if current.catalog_hash() != proposal.base_catalog_hash()
            && current.catalog_hash() != target_hash
        {
            return Err(proposal_request_failure(RuntimeHostError::request(
                HostCode::ProposalActiveCatalogChanged,
                ContractLocation::PromoteProposal,
                RuntimeErrorCode::InvalidRequest,
            )));
        }
        let approvals = {
            let _approval_gate = lock(
                &self.governance_write_gate,
                HostLocation::RecoverProposalApprovals,
            )
            .map_err(RequestFailure::poison_without_terminal)?;
            ApprovalProjection::recover(&self.ledger, Arc::clone(&self.state))
                .map_err(RequestFailure::poison_without_terminal)?
        };
        let mut approval_fact_ids = approvals.active_for_plan(
            preview.proposal_id(),
            target_hash,
            preview.target_catalog_version(),
        );
        if approval_fact_ids.is_empty() {
            return Err(proposal_request_failure(RuntimeHostError::request(
                HostCode::ProposalApprovalMissing,
                ContractLocation::PromoteProposal,
                RuntimeErrorCode::InvalidRequest,
            )));
        }
        if preview.class() == ProposalClass::A {
            let template_approvals = approvals.active_for_catalog(
                proposal.base_catalog_hash(),
                proposal.base_catalog_version(),
            );
            if template_approvals.is_empty() {
                return Err(proposal_request_failure(RuntimeHostError::request(
                    HostCode::ProposalTemplateApprovalMissing,
                    ContractLocation::PromoteProposal,
                    RuntimeErrorCode::InvalidRequest,
                )));
            }
            approval_fact_ids.extend(template_approvals);
        }
        let promotion =
            ProposalPromotion::new(preview.clone(), approval_fact_ids.into_iter().collect())
                .map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        HostCode::ProposalPromotionInvalid,
                        ContractLocation::PromoteProposal,
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
        let authorization =
            CatalogPromotionAuthorization::new(proposal, &promotion).map_err(|_| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    HostCode::ProposalAuthorizationInvalid,
                    ContractLocation::PromoteProposal,
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
        if current.catalog_hash() != target_hash {
            let generation = self
                .activate_policy_catalog_with_authorization(&sources, Some(authorization))
                .map_err(proposal_request_failure)?;
            if generation.catalog_hash() != target_hash
                || generation.catalog_version() != preview.target_catalog_version()
            {
                return Err(RequestFailure::poison_without_terminal(
                    RuntimeHostError::fatal(
                        HostCode::ProposalActivationMismatch,
                        ContractLocation::PromoteProposal,
                        RuntimeErrorCode::RuntimeFatal,
                    ),
                ));
            }
        }
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::ProposalPromoted { promotion },
            failure_outcome: None,
        })
    }

    fn verify_proposal_reports(&self, proposal: &CatalogProposal) -> Result<(), RequestFailure> {
        let verified = self
            .read_verified_artifacts(HostLocation::VerifyProposalReports, |verified| {
                proposal
                    .report_refs()
                    .iter()
                    .map(|reference| proposal_report_is_verified(verified, reference))
                    .collect::<Vec<_>>()
            })
            .map_err(RequestFailure::poison_without_terminal)?;
        for (reference, verified) in proposal.report_refs().iter().zip(verified) {
            if !verified {
                return Err(proposal_request_failure(RuntimeHostError::request(
                    HostCode::ProposalReportUnverified,
                    HostLocation::VerifyProposalReports,
                    RuntimeErrorCode::InvalidRequest,
                )));
            }
            // Workflow #375 R5b: a report file that cannot be read (deleted by hand, say)
            // refuses the request; it never stops the Runtime.
            if read_projected_verified(self.artifacts.root(), reference).is_err() {
                let artifact_id =
                    artifact_id_text(reference).map_err(RequestFailure::poison_without_terminal)?;
                return Err(proposal_request_failure(
                    RuntimeHostError::request(
                        HostCode::ProposalReportUnavailable,
                        HostLocation::VerifyProposalReports,
                        RuntimeErrorCode::InvalidRequest,
                    )
                    .with_outcome_values(|outcome| outcome.artifact_id(artifact_id)),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn prepare_strategic_report(
        &self,
        report: &StrategicReport,
        evidence: &[ProjectedArtifactReference],
    ) -> RuntimeHostResult<StrategicPlanPreparation> {
        self.prepare_strategic_report_with(report, evidence, |prepared, _| Ok(prepared.clone()))
    }

    fn prepare_strategic_report_with<T>(
        &self,
        report: &StrategicReport,
        evidence: &[ProjectedArtifactReference],
        finalize: impl FnOnce(
            &StrategicPlanPreparation,
            RuntimeStrategicPlanResult,
        ) -> RuntimeHostResult<T>,
    ) -> RuntimeHostResult<T> {
        let result = (|| {
            let _gate = lock(
                &self.proposal_write_gate,
                ContractLocation::PrepareStrategicReport,
            )?;
            report.validate().map_err(|_| {
                RuntimeHostError::request(
                    SharedCode::StrategicReportInvalid,
                    ContractLocation::PrepareStrategicReport,
                    RuntimeErrorCode::InvalidRequest,
                )
            })?;
            self.verify_strategic_evidence(report, evidence)?;
            let (facts, resources) = {
                let _outcome_gate = lock(
                    &self.policy_outcome_gate,
                    HostLocation::SnapshotStrategicOutcomeState,
                )?;
                let mut policy = lock(self.policy()?, HostLocation::ReadStrategicOutcomeKeys)?;
                let outcome_keys = policy.outcome_key_snapshot()?;
                let _fact_gate = lock(&self.fact_write_gate, HostLocation::ProjectStrategicFacts)?;
                self.project_authoritative_policy_inputs_under_gate(
                    &mut policy,
                    ContractLocation::PrepareStrategicReport,
                    &outcome_keys,
                    Some(report.as_of_ledger_position()),
                )?
            };
            if report.as_of_ledger_position() > facts.ledger_position {
                return Err(RuntimeHostError::request(
                    HostCode::StrategicReportPositionUnavailable,
                    ContractLocation::PrepareStrategicReport,
                    RuntimeErrorCode::InvalidRequest,
                ));
            }
            let loaded = lock(self.policy()?, ContractLocation::PrepareStrategicReport)?
                .active_loaded()
                .ok_or_else(|| {
                    RuntimeHostError::request(
                        HostCode::StrategicCatalogUnavailable,
                        ContractLocation::PrepareStrategicReport,
                        RuntimeErrorCode::InvalidRequest,
                    )
                })?;
            let projection =
                project_strategic_report(loaded.compiled(), report, &facts, &resources).map_err(
                    |error| {
                        RuntimeHostError::request(
                            error.code(),
                            ContractLocation::PrepareStrategicReport,
                            RuntimeErrorCode::InvalidRequest,
                        )
                    },
                )?;
            let projection_document = encode_planning_document(
                RuntimePlanningDocumentKind::StrategicProjection,
                &projection,
                ContractLocation::PrepareStrategicReport,
            )?;
            let bytes = report.canonical_bytes().map_err(|_| {
                RuntimeHostError::fatal(
                    HostCode::StrategicReportEncodeFailed,
                    ContractLocation::PrepareStrategicReport,
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            let (report_reference, prepared_artifact) =
                self.prepare_or_reuse_strategic_report(&bytes)?;
            let proposal =
                build_strategy_proposal(&projection, loaded.sources(), report_reference.clone())?;
            let preview = proposal
                .as_ref()
                .map(|proposal| {
                    prepare_proposal(loaded.generation(), loaded.sources(), proposal)
                        .and_then(|prepared| prepared.into_ready().map(|(preview, _)| preview))
                })
                .transpose()?;
            let preparation =
                StrategicPlanPreparation::new(report_reference, projection, proposal, preview)?;
            let transport_plan = RuntimeStrategicPlanResult::new(
                preparation.report().clone(),
                projection_document,
                preparation.proposal().cloned(),
                preparation.preview().cloned(),
            )
            .map_err(|error| {
                planning_result_contract_error(error, ContractLocation::PrepareStrategicReport)
            })?;
            let output = finalize(&preparation, transport_plan)?;
            if let Some(prepared_artifact) = prepared_artifact {
                self.commit_strategic_report(prepared_artifact, &bytes, preparation.report())?;
            }
            self.record_strategic_planning_signals(report, preparation.projection())?;
            Ok(output)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    fn record_strategic_planning_signals(
        &self,
        report: &StrategicReport,
        projection: &StrategicProjection,
    ) -> RuntimeHostResult<()> {
        let mut signals = Vec::new();
        for instance in &projection.instances {
            if instance.band == StrategicBand::InfeasibleBestEffort {
                signals.push((
                    instance.goal_id.as_str(),
                    instance.instance_id.as_str(),
                    actingcommand_contract::PolicyPlanningSignalKind::FeasibilityRed,
                    HostCode::StrategicFeasibilityRed,
                ));
            }
            if instance.shortfall.is_some_and(|shortfall| shortfall > 0)
                && instance.deadline_unix_ms <= report.as_of_unix_ms()
            {
                signals.push((
                    instance.goal_id.as_str(),
                    instance.instance_id.as_str(),
                    actingcommand_contract::PolicyPlanningSignalKind::TimelineReached,
                    HostCode::StrategicTimelineReached,
                ));
            }
        }
        signals.sort_by(|left, right| (left.0, left.1, left.2).cmp(&(right.0, right.1, right.2)));
        for (goal_id, instance_id, kind, code) in signals {
            let identity =
                serde_json::to_vec(&(report.report_id(), goal_id, instance_id, kind.as_str()))
                    .map_err(|_| {
                        RuntimeHostError::fatal(
                            HostCode::StrategicSignalIdentityEncodeFailed,
                            ContractLocation::PrepareStrategicReport,
                            RuntimeErrorCode::RuntimeFatal,
                        )
                    })?;
            self.record_policy_planning_signal(PolicyPlanningSignalEventData {
                signal_id: format!("signal:strategic:{:x}", Sha256::digest(identity)),
                instance_id: instance_id.to_owned(),
                task_id: None,
                kind,
                outcome: Outcome::new(code),
                observed_at_unix_ms: report.as_of_unix_ms(),
                detection_budget: None,
            })?;
        }
        Ok(())
    }

    fn verify_strategic_evidence(
        &self,
        report: &StrategicReport,
        evidence: &[ProjectedArtifactReference],
    ) -> RuntimeHostResult<()> {
        let verified_sequences =
            self.read_verified_artifacts(HostLocation::VerifyStrategicEvidence, |verified| {
                evidence
                    .iter()
                    .map(|reference| proposal_report_verified_sequence(verified, reference))
                    .collect::<Vec<_>>()
            })?;
        let mut pointers = Vec::with_capacity(evidence.len());
        for (reference, verified_sequence) in evidence.iter().zip(verified_sequences) {
            reference.validate().map_err(|_| {
                RuntimeHostError::request(
                    HostCode::StrategicEvidenceInvalid,
                    HostLocation::VerifyStrategicEvidence,
                    RuntimeErrorCode::InvalidRequest,
                )
            })?;
            if reference.object_key().is_none()
                || reference.redaction_state() == ArtifactRedactionState::Pending
                || verified_sequence.is_none()
                || verified_sequence
                    .is_some_and(|sequence| sequence > report.as_of_ledger_position())
            {
                return Err(RuntimeHostError::request(
                    HostCode::StrategicEvidenceUnverified,
                    HostLocation::VerifyStrategicEvidence,
                    RuntimeErrorCode::InvalidRequest,
                ));
            }
            // Workflow #375 R5b: an evidence frame that cannot be read (deleted by hand, say)
            // refuses the request; it never stops the Runtime.
            if read_projected_verified(self.artifacts.root(), reference).is_err() {
                let artifact_id = artifact_id_text(reference)?;
                return Err(RuntimeHostError::request(
                    HostCode::StrategicEvidenceUnavailable,
                    HostLocation::VerifyStrategicEvidence,
                    RuntimeErrorCode::InvalidRequest,
                )
                .with_outcome_values(|outcome| outcome.artifact_id(artifact_id)));
            }
            pointers.push(strategic_evidence_pointer(reference)?);
        }
        pointers.sort();
        if pointers != report.evidence() {
            return Err(RuntimeHostError::request(
                HostCode::StrategicEvidenceMismatch,
                HostLocation::VerifyStrategicEvidence,
                RuntimeErrorCode::InvalidRequest,
            ));
        }
        Ok(())
    }

    fn prepare_or_reuse_strategic_report(
        &self,
        bytes: &[u8],
    ) -> RuntimeHostResult<(ProjectedArtifactReference, Option<PreparedArtifact>)> {
        let sha256 = format!("sha256:{:x}", Sha256::digest(bytes));
        let matching =
            self.read_verified_artifacts(HostLocation::FindStrategicReport, |verified| {
                verified
                    .iter()
                    .filter(|(_, reference)| {
                        reference.kind() == ArtifactKind::StrategyReport
                            && reference.sha256() == sha256
                    })
                    .map(|(_, reference)| reference.clone())
                    .collect::<Vec<_>>()
            })?;
        let mut existing = Vec::new();
        for reference in matching {
            existing.push((artifact_id_text(&reference)?, reference));
        }
        existing.sort_by(|left, right| left.0.cmp(&right.0));
        if let Some((artifact_id, reference)) = existing.into_iter().next() {
            // Workflow #375 R5b: a stored report that cannot be read (deleted by hand, say)
            // refuses the request; it never stops the Runtime.
            let Ok(stored) = read_projected_verified(self.artifacts.root(), &reference) else {
                return Err(RuntimeHostError::request(
                    HostCode::ProposalReportUnavailable,
                    HostLocation::ReadStrategicReport,
                    RuntimeErrorCode::InvalidRequest,
                )
                .with_outcome_values(|outcome| outcome.artifact_id(artifact_id)));
            };
            if stored != bytes {
                return Err(RuntimeHostError::fatal(
                    HostCode::StrategicReportIdentityConflict,
                    HostLocation::ReadStrategicReport,
                    RuntimeErrorCode::RuntimeFatal,
                ));
            }
            return Ok((reference, None));
        }
        let context = ArtifactWriteContext::new(
            ArtifactLinksDraft::default(),
            self.events.system_links()?,
            unix_ms_now()?,
        );
        let prepared = self
            .artifacts
            .prepare(ArtifactWriteRequest::new(
                ArtifactKind::StrategyReport,
                bytes,
                context,
                ArtifactIssuePolicy::new(
                    ArtifactProducer::ArtifactStore,
                    RetentionClass::Adaptive,
                    ArtifactRedactionState::Applied,
                ),
            ))
            .map_err(RuntimeHostError::artifact)?;
        Ok((prepared.reference().project(true), Some(prepared)))
    }

    fn commit_strategic_report(
        &self,
        prepared: PreparedArtifact,
        bytes: &[u8],
        expected: &ProjectedArtifactReference,
    ) -> RuntimeHostResult<()> {
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.ledger,
            events: &self.events,
        };
        let stored = self
            .artifacts
            .commit_prepared(prepared, bytes, &mut sink)
            .map_err(RuntimeHostError::artifact)?;
        let actual = stored.reference().project(true);
        if &actual != expected {
            return Err(RuntimeHostError::fatal(
                HostCode::StrategicReportIdentityConflict,
                HostLocation::StoreStrategicReport,
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn store_test_report(
        &self,
        bytes: &[u8],
    ) -> RuntimeHostResult<ProjectedArtifactReference> {
        let context = ArtifactWriteContext::new(
            ArtifactLinksDraft::default(),
            self.events.system_links()?,
            unix_ms_now()?,
        );
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.ledger,
            events: &self.events,
        };
        self.artifacts
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::TextReport,
                    bytes,
                    context,
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::ArtifactStore,
                        RetentionClass::Adaptive,
                        ArtifactRedactionState::NotRequired,
                    ),
                ),
                &mut sink,
            )
            .map(|stored| stored.reference().project(true))
            .map_err(RuntimeHostError::artifact)
    }
}

fn decode_planning_document<T>(
    document: &RuntimePlanningDocument,
    kind: RuntimePlanningDocumentKind,
    operation: impl Into<Location>,
) -> Result<T, RequestFailure>
where
    T: serde::de::DeserializeOwned,
{
    let operation: Location = operation.into();
    document.decode(kind).map_err(|_| {
        RequestFailure::request(
            RuntimeHostError::request(
                HostCode::PlanningDocumentInvalid,
                operation,
                RuntimeErrorCode::InvalidRequest,
            ),
            RuntimeReceiptState::Denied,
            None,
        )
    })
}

fn encode_planning_document<T>(
    kind: RuntimePlanningDocumentKind,
    value: &T,
    operation: impl Into<Location>,
) -> RuntimeHostResult<RuntimePlanningDocument>
where
    T: serde::Serialize,
{
    let operation: Location = operation.into();
    RuntimePlanningDocument::encode(kind, value)
        .map_err(|error| planning_result_contract_error(error, operation))
}

fn planning_result_contract_error(
    error: RuntimeContractError,
    operation: impl Into<Location>,
) -> RuntimeHostError {
    let operation: Location = operation.into();
    match error.code() {
        code if code == RuntimeContractCode::PlanningDocumentSizeInvalid.code()
            || code == RuntimeContractCode::PlanningResponseSizeInvalid.code() =>
        {
            RuntimeHostError::request(error.code(), operation, RuntimeErrorCode::InvalidRequest)
        }
        _ => RuntimeHostError::fatal(
            SharedCode::PlanningResultEncodeFailed,
            operation,
            RuntimeErrorCode::RuntimeFatal,
        ),
    }
}

pub(super) fn planning_request_failure(error: RuntimeHostError) -> RequestFailure {
    if error.is_fatal() {
        RequestFailure::poison_without_terminal(error)
    } else {
        RequestFailure::request(error, RuntimeReceiptState::Denied, None)
    }
}

fn proposal_request_failure(error: RuntimeHostError) -> RequestFailure {
    if error.is_fatal() {
        RequestFailure::poison_without_terminal(error)
    } else {
        RequestFailure::request(error, RuntimeReceiptState::Denied, None)
    }
}

fn proposal_report_is_verified(
    verified: &[(u64, ProjectedArtifactReference)],
    reference: &ProjectedArtifactReference,
) -> bool {
    proposal_report_verified_sequence(verified, reference).is_some()
}

fn proposal_report_verified_sequence(
    verified: &[(u64, ProjectedArtifactReference)],
    reference: &ProjectedArtifactReference,
) -> Option<u64> {
    verified
        .iter()
        .find_map(|(sequence, artifact)| (artifact == reference).then_some(*sequence))
}

fn strategic_evidence_pointer(
    reference: &ProjectedArtifactReference,
) -> RuntimeHostResult<StrategicEvidencePointer> {
    Ok(StrategicEvidencePointer {
        artifact_id: artifact_id_text(reference)?,
        sha256: reference.sha256.clone(),
    })
}

fn artifact_id_text(reference: &ProjectedArtifactReference) -> RuntimeHostResult<String> {
    serde_json::to_value(reference.artifact_id)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::ArtifactIdentityEncodeFailed,
                ContractLocation::PrepareStrategicReport,
                RuntimeErrorCode::RuntimeFatal,
            )
        })
}
