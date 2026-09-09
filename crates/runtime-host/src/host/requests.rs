// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

impl HostShared {
    pub(super) fn process_request(
        &self,
        request: &RuntimeRequest,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<RuntimeReceipt> {
        if let Some(error) = self.fatal.current()? {
            return runtime_error_receipt(
                request,
                RuntimeReceiptState::Failed,
                None,
                error.projection().clone(),
            );
        }
        let validated = match request.validate() {
            Ok(validated) => validated,
            Err(_) => {
                return runtime_error_receipt(
                    request,
                    RuntimeReceiptState::Denied,
                    None,
                    RuntimeErrorProjection::new(RuntimeErrorCode::InvalidRequest, false),
                );
            }
        };
        let _work = if matches!(
            request.operation(),
            RuntimeOperation::RequestShutdown { .. }
        ) {
            None
        } else {
            match self.begin_work()? {
                Some(work) => Some(work),
                None => {
                    return runtime_error_receipt(
                        request,
                        RuntimeReceiptState::Denied,
                        None,
                        RuntimeErrorProjection::new(RuntimeErrorCode::RuntimeUnavailable, false),
                    );
                }
            }
        };
        match self.process_validated(request, &validated, connection_id) {
            Ok(success) => success.into_receipt(request),
            Err(failure) => {
                if failure.poison_runtime {
                    self.fatal.mark((*failure.error).clone())?;
                }
                runtime_error_receipt(
                    request,
                    failure.state,
                    failure.terminal,
                    failure.error.projection().clone(),
                )
            }
        }
    }

    fn process_validated(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        match request.operation() {
            RuntimeOperation::RequestShutdown { target } => {
                self.request_shutdown(validated, *target)
            }
            RuntimeOperation::Health => Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: None,
                result: RuntimeResult::Health {
                    owner_epoch: self.owner_epoch,
                },
            }),
            RuntimeOperation::Status => self.control_plane_status(validated),
            RuntimeOperation::ProjectInterface { request } => {
                self.project_interface(validated, request)
            }
            RuntimeOperation::ProjectPolicyInputIdentity {
                as_of_ledger_position,
            } => self.project_policy_input_identity(*as_of_ledger_position),
            RuntimeOperation::MonitorStatus => self.monitor_status(validated),
            RuntimeOperation::ConfigureMonitor {
                instance_alias,
                policy,
            } => {
                self.require_physical_instance_alias(instance_alias)?;
                self.configure_monitor(request, validated, instance_alias, policy.clone())
            }
            RuntimeOperation::ClearMonitor { instance_alias } => {
                self.require_physical_instance_alias(instance_alias)?;
                self.clear_monitor(request, validated, instance_alias)
            }
            RuntimeOperation::AcquireLease {
                instance_alias,
                holder_id,
            } => {
                self.require_physical_instance_alias(instance_alias)?;
                self.acquire_lease(RuntimeLeaseAcquisition {
                    request: validated,
                    request_id: request.request_id(),
                    instance_alias,
                    holder_id: *holder_id,
                    connection_id,
                    run_links: None,
                    lease_ttl_ms: None,
                })
            }
            RuntimeOperation::QueueLease {
                instance_alias,
                holder_id,
                policy,
            } => {
                self.require_physical_instance_alias(instance_alias)?;
                self.queue_lease(
                    request,
                    validated,
                    instance_alias,
                    *holder_id,
                    *policy,
                    connection_id,
                )
            }
            RuntimeOperation::PollQueuedLease { queued_request_id } => {
                self.poll_queued_lease(validated, *queued_request_id, connection_id)
            }
            RuntimeOperation::CancelQueuedLease { queued_request_id } => {
                self.cancel_queued_lease(validated, *queued_request_id, connection_id)
            }
            RuntimeOperation::CancelContainedTask { task_request_id } => {
                self.cancel_contained_task(*task_request_id)
            }
            RuntimeOperation::RenewLease { token } => {
                self.require_physical_instance_id(token.instance_id())?;
                self.renew_lease(validated, request.request_id(), token, connection_id)
            }
            RuntimeOperation::ReleaseLease { token } => {
                self.require_physical_instance_id(token.instance_id())?;
                self.release_lease(validated, request.request_id(), token, connection_id, None)
            }
            RuntimeOperation::ObserveReadonly { instance_alias } => {
                self.require_physical_instance_alias(instance_alias)?;
                self.observe_readonly(request, validated, instance_alias)
            }
            RuntimeOperation::RecognizeArtifact {
                request: recognition,
            } => self.recognize_artifact(validated, recognition),
            RuntimeOperation::ObserveContainedPage {
                instance_alias,
                request: observation,
            } => {
                self.require_physical_instance_alias(instance_alias)?;
                self.observe_contained_page(request, validated, instance_alias, observation)
            }
            RuntimeOperation::RunContainedLabOperation {
                instance_alias,
                holder_id,
                request: operation,
            } => {
                self.require_physical_instance_alias(instance_alias)?;
                self.run_contained_lab_operation(
                    request,
                    validated,
                    instance_alias,
                    *holder_id,
                    operation,
                    connection_id,
                )
            }
            RuntimeOperation::CaptureSequence {
                instance_alias,
                spec,
            } => {
                self.require_physical_instance_alias(instance_alias)?;
                self.capture_sequence(request, validated, instance_alias, *spec)
            }
            RuntimeOperation::SafeReset {
                instance_alias,
                holder_id,
            } => {
                self.require_physical_instance_alias(instance_alias)?;
                self.safe_reset(
                    request,
                    validated,
                    instance_alias,
                    *holder_id,
                    connection_id,
                )
            }
            RuntimeOperation::ApplicationLifecycle {
                instance_alias,
                holder_id,
                action,
            } => {
                self.require_physical_instance_alias(instance_alias)?;
                self.application_lifecycle(
                    request,
                    validated,
                    instance_alias,
                    *holder_id,
                    *action,
                    connection_id,
                )
            }
            RuntimeOperation::RunContainedTask {
                instance_alias,
                holder_id,
                request: task_request,
            } => {
                self.require_physical_instance_alias(instance_alias)?;
                self.run_contained_task(
                    request,
                    validated,
                    instance_alias,
                    *holder_id,
                    task_request,
                    connection_id,
                )
            }
            RuntimeOperation::Input { token, action } => self
                .input(
                    validated,
                    token,
                    action,
                    connection_id,
                    ExecutionBackendProvenance::PhysicalDevice,
                    RuntimeInputContext::default(),
                )
                .map(|(success, _)| success),
            RuntimeOperation::PublishFact { record } => {
                let event_id = self
                    .publish_facts(
                        actingcommand_contract::FactObservation {
                            records: vec![record.clone()],
                        },
                        Some(validated),
                    )
                    .map_err(|error| {
                        if error.is_fatal() {
                            RequestFailure::poison_without_terminal(error)
                        } else {
                            RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                        }
                    })?;
                Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: None,
                    result: RuntimeResult::FactPublished { event_id },
                })
            }
            RuntimeOperation::PublishFacts { observation } => {
                let event_id = self
                    .publish_facts(observation.clone(), Some(validated))
                    .map_err(|error| {
                        if error.is_fatal() {
                            RequestFailure::poison_without_terminal(error)
                        } else {
                            RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                        }
                    })?;
                Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: None,
                    result: RuntimeResult::FactPublished { event_id },
                })
            }
            RuntimeOperation::QueryEvents {
                query,
                profile,
                page,
            } => self.query_events(query, *profile, page),
            RuntimeOperation::SubscribeEvents { request } => self.subscribe_events(request),
            RuntimeOperation::RegisterDiagnosticSignature { definition } => {
                self.register_signature(validated, definition)
            }
            RuntimeOperation::MatchDiagnosticSignatures { request } => {
                self.match_signatures(validated, request)
            }
            RuntimeOperation::RetireDiagnosticSignature { registration } => {
                self.retire_signature(validated, registration)
            }
            RuntimeOperation::DebugPackage { request } => self.debug_package(validated, request),
            RuntimeOperation::ExportEvidence { request } => {
                self.export_evidence(validated, request)
            }
            RuntimeOperation::RecordAuthoringEvent { event } => {
                self.record_authoring_event(validated, event)
            }
            RuntimeOperation::RecordDebugEvent { event } => {
                self.record_debug_event(validated, event)
            }
            RuntimeOperation::RecordClientAction { action } => {
                self.record_client_action(request, validated, action)
            }
            RuntimeOperation::AuthenticateGovernance { capability } => {
                self.authenticate_governance(request, connection_id, capability)
            }
            RuntimeOperation::RecordApprovalDecision { decision } => {
                self.record_approval_decision(request, validated, decision, connection_id)
            }
            RuntimeOperation::StartAgentSession { wake_id } => {
                self.start_agent_session(request, validated, *wake_id)
            }
            RuntimeOperation::ResumeAgentSession { session_id } => {
                self.resume_agent_session(request, validated, *session_id)
            }
            RuntimeOperation::AgentSessionStatus { session_id } => {
                self.agent_session_status(*session_id)
            }
            RuntimeOperation::RecordAgentResponse { response } => {
                self.record_agent_response(request, validated, response)
            }
            RuntimeOperation::PrepareStrategicReport { request } => {
                self.prepare_strategic_report_ipc(validated, request.report(), request.evidence())
            }
            RuntimeOperation::ProjectPolicyForward { request } => {
                self.project_policy_forward_ipc(validated, request)
            }
            RuntimeOperation::AssessPredictiveMaintenance { query } => {
                self.assess_predictive_maintenance_ipc(query)
            }
            RuntimeOperation::CompileProposal { proposal } => self.compile_proposal(proposal),
            RuntimeOperation::PromoteProposal { proposal } => self.promote_proposal(proposal),
        }
    }
}
