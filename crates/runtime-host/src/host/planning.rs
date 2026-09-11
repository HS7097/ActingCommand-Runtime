// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

impl HostShared {
    pub(super) fn prepare_strategic_report_ipc(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        report: &RuntimePlanningDocument,
        evidence: &[ProjectedArtifactReference],
    ) -> Result<OperationSuccess, RequestFailure> {
        let report: StrategicReport = decode_planning_document(
            report,
            RuntimePlanningDocumentKind::StrategicReport,
            "prepare_strategic_report",
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
            "project_policy_forward",
        )?;
        let resources: EvaluationResources = decode_planning_document(
            request.resources(),
            RuntimePlanningDocumentKind::EvaluationResources,
            "project_policy_forward",
        )?;
        let time: EvaluationTime = decode_planning_document(
            request.time(),
            RuntimePlanningDocumentKind::EvaluationTime,
            "project_policy_forward",
        )?;
        let config: ForwardProjectionConfig = decode_planning_document(
            request.config(),
            RuntimePlanningDocumentKind::ForwardProjectionConfig,
            "project_policy_forward",
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
                "project_policy_forward",
            )
            .map_err(planning_request_failure)?;
            Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: None,
                result: RuntimeResult::PolicyForwardProjected {
                    projection: Box::new(projection),
                },
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
            "assess_predictive_maintenance",
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
            "assess_predictive_maintenance",
        )
        .map_err(planning_request_failure)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::PredictiveMaintenanceAssessed {
                assessment: Box::new(assessment),
            },
        })
    }

    pub(super) fn compile_proposal(
        &self,
        proposal: &CatalogProposal,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.verify_proposal_reports(proposal)?;
        let prepared = {
            let policy = lock(&self.policy, "compile_proposal")?;
            let generation = policy.active_generation().ok_or_else(|| {
                proposal_request_failure(RuntimeHostError::request(
                    "proposal_base_catalog_unavailable",
                    "compile_proposal",
                    RuntimeErrorCode::InvalidRequest,
                ))
            })?;
            let sources = policy.active_sources().ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "proposal_base_sources_unavailable",
                    "compile_proposal",
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
        })
    }

    pub(super) fn promote_proposal(
        &self,
        proposal: &CatalogProposal,
    ) -> Result<OperationSuccess, RequestFailure> {
        let _gate = lock(&self.proposal_write_gate, "promote_proposal")
            .map_err(RequestFailure::poison_without_terminal)?;
        self.verify_proposal_reports(proposal)?;
        let (prepared, current) = {
            let policy = lock(&self.policy, "prepare_proposal_promotion")?;
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
                "proposal_target_hash_missing",
                "promote_proposal",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let current = current.ok_or_else(|| {
            proposal_request_failure(RuntimeHostError::request(
                "proposal_active_catalog_unavailable",
                "promote_proposal",
                RuntimeErrorCode::InvalidRequest,
            ))
        })?;
        if current.catalog_hash() != proposal.base_catalog_hash()
            && current.catalog_hash() != target_hash
        {
            return Err(proposal_request_failure(RuntimeHostError::request(
                "proposal_active_catalog_changed",
                "promote_proposal",
                RuntimeErrorCode::InvalidRequest,
            )));
        }
        let approvals = {
            let _approval_gate = lock(&self.governance_write_gate, "recover_proposal_approvals")
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
                "proposal_approval_missing",
                "promote_proposal",
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
                    "proposal_template_approval_missing",
                    "promote_proposal",
                    RuntimeErrorCode::InvalidRequest,
                )));
            }
            approval_fact_ids.extend(template_approvals);
        }
        let promotion =
            ProposalPromotion::new(preview.clone(), approval_fact_ids.into_iter().collect())
                .map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "proposal_promotion_invalid",
                        "promote_proposal",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
        let authorization =
            CatalogPromotionAuthorization::new(proposal, &promotion).map_err(|_| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "proposal_authorization_invalid",
                    "promote_proposal",
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
                        "proposal_activation_mismatch",
                        "promote_proposal",
                        RuntimeErrorCode::RuntimeFatal,
                    ),
                ));
            }
        }
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::ProposalPromoted { promotion },
        })
    }

    fn verify_proposal_reports(&self, proposal: &CatalogProposal) -> Result<(), RequestFailure> {
        let verified_events = self
            .ledger
            .query(EventQuery {
                event_type: Some(EventType::ArtifactVerified),
                ..EventQuery::default()
            })
            .map_err(|_| {
                RequestFailure::poison_without_terminal(ledger_error("verify_proposal_reports"))
            })?;
        for reference in proposal.report_refs() {
            if !proposal_report_is_verified(&verified_events, reference) {
                return Err(proposal_request_failure(RuntimeHostError::request(
                    "proposal_report_unverified",
                    "verify_proposal_reports",
                    RuntimeErrorCode::InvalidRequest,
                )));
            }
            read_projected_verified(self.artifacts.root(), reference).map_err(|_| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "proposal_report_unavailable",
                    "verify_proposal_reports",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })?;
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
            let _gate = lock(&self.proposal_write_gate, "prepare_strategic_report")?;
            report.validate().map_err(|_| {
                RuntimeHostError::request(
                    "strategic_report_invalid",
                    "prepare_strategic_report",
                    RuntimeErrorCode::InvalidRequest,
                )
            })?;
            self.verify_strategic_evidence(report, evidence)?;
            let (facts, resources) = {
                let _outcome_gate = lock(
                    &self.policy_outcome_gate,
                    "snapshot_strategic_outcome_state",
                )?;
                let outcome_keys =
                    lock(&self.policy, "read_strategic_outcome_keys")?.outcome_key_snapshot()?;
                let _fact_gate = lock(&self.fact_write_gate, "project_strategic_facts")?;
                self.project_authoritative_policy_inputs_under_gate(
                    "prepare_strategic_report",
                    &outcome_keys,
                    Some(report.as_of_ledger_position()),
                )?
            };
            if report.as_of_ledger_position() > facts.ledger_position {
                return Err(RuntimeHostError::request(
                    "strategic_report_position_unavailable",
                    "prepare_strategic_report",
                    RuntimeErrorCode::InvalidRequest,
                ));
            }
            let loaded = lock(&self.policy, "prepare_strategic_report")?
                .active_loaded()
                .ok_or_else(|| {
                    RuntimeHostError::request(
                        "strategic_catalog_unavailable",
                        "prepare_strategic_report",
                        RuntimeErrorCode::InvalidRequest,
                    )
                })?;
            let projection =
                project_strategic_report(loaded.compiled(), report, &facts, &resources).map_err(
                    |error| {
                        RuntimeHostError::request(
                            error.code(),
                            "prepare_strategic_report",
                            RuntimeErrorCode::InvalidRequest,
                        )
                    },
                )?;
            let projection_document = encode_planning_document(
                RuntimePlanningDocumentKind::StrategicProjection,
                &projection,
                "prepare_strategic_report",
            )?;
            let bytes = report.canonical_bytes().map_err(|_| {
                RuntimeHostError::fatal(
                    "strategic_report_encode_failed",
                    "prepare_strategic_report",
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
            .map_err(|error| planning_result_contract_error(error, "prepare_strategic_report"))?;
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
                    "strategic.feasibility_red",
                ));
            }
            if instance.shortfall.is_some_and(|shortfall| shortfall > 0)
                && instance.deadline_unix_ms <= report.as_of_unix_ms()
            {
                signals.push((
                    instance.goal_id.as_str(),
                    instance.instance_id.as_str(),
                    actingcommand_contract::PolicyPlanningSignalKind::TimelineReached,
                    "strategic.timeline_reached",
                ));
            }
        }
        signals.sort_by(|left, right| (left.0, left.1, left.2).cmp(&(right.0, right.1, right.2)));
        for (goal_id, instance_id, kind, fact_code) in signals {
            let identity =
                serde_json::to_vec(&(report.report_id(), goal_id, instance_id, kind.as_str()))
                    .map_err(|_| {
                        RuntimeHostError::fatal(
                            "strategic_signal_identity_encode_failed",
                            "prepare_strategic_report",
                            RuntimeErrorCode::RuntimeFatal,
                        )
                    })?;
            self.record_policy_planning_signal(PolicyPlanningSignalEventData {
                signal_id: format!("signal:strategic:{:x}", Sha256::digest(identity)),
                instance_id: instance_id.to_owned(),
                task_id: None,
                kind,
                fact_code: fact_code.to_owned(),
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
        let verified_events = self
            .ledger
            .query(EventQuery {
                event_type: Some(EventType::ArtifactVerified),
                ..EventQuery::default()
            })
            .map_err(|_| ledger_error("verify_strategic_evidence"))?;
        let mut pointers = Vec::with_capacity(evidence.len());
        for reference in evidence {
            reference.validate().map_err(|_| {
                RuntimeHostError::request(
                    "strategic_evidence_invalid",
                    "verify_strategic_evidence",
                    RuntimeErrorCode::InvalidRequest,
                )
            })?;
            let verified_sequence = proposal_report_verified_sequence(&verified_events, reference);
            if reference.object_key().is_none()
                || reference.redaction_state() == ArtifactRedactionState::Pending
                || verified_sequence.is_none()
                || verified_sequence
                    .is_some_and(|sequence| sequence > report.as_of_ledger_position())
            {
                return Err(RuntimeHostError::request(
                    "strategic_evidence_unverified",
                    "verify_strategic_evidence",
                    RuntimeErrorCode::InvalidRequest,
                ));
            }
            read_projected_verified(self.artifacts.root(), reference).map_err(|_| {
                RuntimeHostError::fatal(
                    "strategic_evidence_unavailable",
                    "verify_strategic_evidence",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            pointers.push(strategic_evidence_pointer(reference)?);
        }
        pointers.sort();
        if pointers != report.evidence() {
            return Err(RuntimeHostError::request(
                "strategic_evidence_mismatch",
                "verify_strategic_evidence",
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
        let events = self
            .ledger
            .query(EventQuery {
                event_type: Some(EventType::ArtifactVerified),
                ..EventQuery::default()
            })
            .map_err(|_| ledger_error("find_strategic_report"))?;
        let mut existing = Vec::new();
        for reference in events
            .iter()
            .flat_map(PersistedEvent::artifacts)
            .filter(|reference| {
                reference.kind() == ArtifactKind::StrategyReport && reference.sha256() == sha256
            })
        {
            let reference = reference.project(true);
            existing.push((artifact_id_text(&reference)?, reference));
        }
        existing.sort_by(|left, right| left.0.cmp(&right.0));
        if let Some((_, reference)) = existing.into_iter().next() {
            let stored = read_projected_verified(self.artifacts.root(), &reference)
                .map_err(RuntimeHostError::artifact)?;
            if stored != bytes {
                return Err(RuntimeHostError::fatal(
                    "strategic_report_identity_conflict",
                    "read_strategic_report",
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
                "strategic_report_identity_conflict",
                "store_strategic_report",
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
    operation: &'static str,
) -> Result<T, RequestFailure>
where
    T: serde::de::DeserializeOwned,
{
    document.decode(kind).map_err(|_| {
        RequestFailure::request(
            RuntimeHostError::request(
                "planning_document_invalid",
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
    operation: &'static str,
) -> RuntimeHostResult<RuntimePlanningDocument>
where
    T: serde::Serialize,
{
    RuntimePlanningDocument::encode(kind, value)
        .map_err(|error| planning_result_contract_error(error, operation))
}

fn planning_result_contract_error(
    error: RuntimeContractError,
    operation: &'static str,
) -> RuntimeHostError {
    match error.code() {
        "planning_document_size_invalid" | "planning_response_size_invalid" => {
            RuntimeHostError::request(error.code(), operation, RuntimeErrorCode::InvalidRequest)
        }
        _ => RuntimeHostError::fatal(
            "planning_result_encode_failed",
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
    events: &[PersistedEvent],
    reference: &ProjectedArtifactReference,
) -> bool {
    proposal_report_verified_sequence(events, reference).is_some()
}

fn proposal_report_verified_sequence(
    events: &[PersistedEvent],
    reference: &ProjectedArtifactReference,
) -> Option<u64> {
    events.iter().find_map(|event| {
        event
            .artifacts()
            .iter()
            .any(|artifact| artifact.project(true) == *reference)
            .then_some(event.sequence())
    })
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
                "artifact_identity_encode_failed",
                "prepare_strategic_report",
                RuntimeErrorCode::RuntimeFatal,
            )
        })
}
