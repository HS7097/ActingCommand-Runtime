// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::ipc::{FrameRead, read_frame, write_frame};
use std::net::TcpStream;
use std::panic::{AssertUnwindSafe, catch_unwind};

impl HostShared {
    #[cfg(test)]
    pub(super) fn process_request(
        &self,
        request: &RuntimeRequest,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<RuntimeReceipt> {
        self.process_request_observed(
            request,
            connection_id,
            None,
            MaterialReadContext::for_request(request, DEFAULT_RUNTIME_MAX_FRAME_BYTES)?,
        )
    }

    pub(super) fn process_request_observed(
        &self,
        request: &RuntimeRequest,
        connection_id: ConnectionId,
        mut timing: Option<&mut ConnectionTiming>,
        material: Option<MaterialReadContext>,
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
        if let Some(timing) = timing.as_deref_mut() {
            timing.validated_dispatch.begin();
        }
        let dispatched = self.process_validated(
            request,
            &validated,
            connection_id,
            timing.as_deref_mut(),
            material,
        );
        if let Some(timing) = timing {
            timing.validated_dispatch.finish(dispatched.is_ok());
        }
        match dispatched {
            Ok(success) => success.into_receipt(request),
            Err(mut failure) => {
                if failure.poison_runtime {
                    self.fatal.mark((*failure.error).clone())?;
                }
                let projection = failure
                    .error
                    .projection()
                    .clone()
                    .with_host_failure(failure.error.code(), failure.error.operation());
                if let Some(rejection) = failure.error.resource_declaration().cloned() {
                    let rejected = if let Some(event) =
                        failure.error.lifecycle.resource_declaration_event
                    {
                        event
                    } else {
                        let links = validated.event_links(None, None, None);
                        let event = self.append_event_raw(
                            EventSeverity::Warning,
                            EventSource::Runtime,
                            OriginModule::Runtime,
                            EventActor::Runtime,
                            links.clone(),
                            RuntimePayloadDraft::resource_declaration_rejected(rejection.clone()),
                        )?;
                        self.record_required_failure(&failure.error, &event, links)?;
                        terminal(&event)
                    };
                    failure.terminal.get_or_insert(rejected);
                    return runtime_error_receipt(
                        request,
                        failure.state,
                        failure.terminal,
                        projection,
                    )?
                    .with_resource_declaration(rejection, rejected)
                    .map_err(|_| receipt_error());
                }
                runtime_error_receipt(request, failure.state, failure.terminal, projection)
            }
        }
    }

    fn process_validated(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        connection_id: ConnectionId,
        timing: Option<&mut ConnectionTiming>,
        material: Option<MaterialReadContext>,
    ) -> Result<OperationSuccess, RequestFailure> {
        match request.operation() {
            RuntimeOperation::ReadMaterial { request } => self.read_material(
                validated,
                request,
                material.ok_or_else(|| {
                    RequestFailure::poison_without_terminal(protocol_error(
                        "material_read_context_missing",
                    ))
                })?,
            ),
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
            RuntimeOperation::DiscoverInstances => self.discover_instances(validated),
            RuntimeOperation::ProjectInterface { request } => {
                self.project_interface(validated, request)
            }
            RuntimeOperation::ProjectPolicyInputIdentity {
                as_of_ledger_position,
            } => {
                if let Some(timing) = timing {
                    timing.policy_identity_projection.begin();
                    let projected = self.project_policy_input_identity(*as_of_ledger_position);
                    timing.policy_identity_projection.finish(projected.is_ok());
                    projected
                } else {
                    self.project_policy_input_identity(*as_of_ledger_position)
                }
            }
            RuntimeOperation::MonitorStatus => self.monitor_status(validated),
            RuntimeOperation::RuntimeFactSnapshot => {
                let snapshot = self.runtime_fact_snapshot().map_err(|error| {
                    if error.is_fatal() {
                        RequestFailure::poison_without_terminal(error)
                    } else {
                        RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                    }
                })?;
                Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: None,
                    result: RuntimeResult::RuntimeFactSnapshot { snapshot },
                })
            }
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
            RuntimeOperation::ControlEmulatorInstance {
                instance_alias,
                action,
            } => {
                self.require_physical_instance_alias(instance_alias)?;
                self.control_emulator_instance(request, validated, instance_alias, *action)
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
            RuntimeOperation::Input {
                token,
                action,
                frame,
            } => self
                .input(
                    validated,
                    token,
                    action,
                    connection_id,
                    ExecutionBackendProvenance::PhysicalDevice,
                    RuntimeInputContext {
                        input_frame: *frame,
                        ..RuntimeInputContext::default()
                    },
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
            RuntimeOperation::ReleaseLabPin { target } => self.release_lab_pin(validated, *target),
            RuntimeOperation::RecordClientAction { action } => {
                self.record_client_action(request, validated, action)
            }
            RuntimeOperation::DeclareGovernanceIdentity { card } => {
                self.declare_governance_identity(request, validated, connection_id, card)
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

pub(super) struct RequestFailure {
    pub(super) state: RuntimeReceiptState,
    pub(super) terminal: Option<TerminalEvent>,
    pub(super) error: Box<RuntimeHostError>,
    pub(super) poison_runtime: bool,
    pub(super) task_failure: Option<TaskFailureEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TaskFailureEvidence {
    pub(super) code: &'static str,
    pub(super) severity: EventSeverity,
}

pub(super) struct ActionFailure {
    pub(super) error: RuntimeHostError,
    pub(super) diagnostic: DiagnosticCode,
    pub(super) effect: EffectDisposition,
    pub(super) poison_runtime: bool,
    pub(super) release_after: bool,
    pub(super) destructive_started: bool,
    pub(super) transfer_after: bool,
    pub(super) task_failure: Option<Box<TaskFailureEvidence>>,
}

impl RuntimeRunLinks {
    pub(super) const fn new(task_id: IssuedTaskId, run_id: IssuedRunId) -> Self {
        Self { task_id, run_id }
    }

    pub(super) fn apply(self, links: EventLinksDraft) -> EventLinksDraft {
        links.with_task_id(self.task_id).with_run_id(self.run_id)
    }
}

impl OperationSuccess {
    fn into_receipt(self, request: &RuntimeRequest) -> RuntimeHostResult<RuntimeReceipt> {
        match self.result {
            RuntimeResult::MaterialRead { result } => {
                RuntimeReceipt::material_read(request, self.terminal, result)
            }
            RuntimeResult::ContainedLabOperation { operation } => {
                RuntimeReceipt::contained_lab_operation(
                    request,
                    self.terminal.ok_or_else(receipt_error)?,
                    operation,
                )
            }
            result => RuntimeReceipt::success(request, self.state, self.terminal, result),
        }
        .map_err(|_| receipt_error())
    }
}

impl RequestFailure {
    pub(super) fn request(
        error: RuntimeHostError,
        state: RuntimeReceiptState,
        terminal: Option<TerminalEvent>,
    ) -> Self {
        Self {
            state,
            terminal,
            error: Box::new(error),
            poison_runtime: false,
            task_failure: None,
        }
    }

    pub(super) fn poison(error: RuntimeHostError, terminal: Option<TerminalEvent>) -> Self {
        Self {
            state: RuntimeReceiptState::Failed,
            terminal,
            error: Box::new(error.into_fatal()),
            poison_runtime: true,
            task_failure: None,
        }
    }

    pub(super) fn poison_without_terminal(error: RuntimeHostError) -> Self {
        Self::poison(error, None)
    }

    pub(super) fn replace_with_poison(self, error: RuntimeHostError) -> Self {
        let mut error = if self.error.has_ppocr_diagnostics() {
            self.error.as_ref().clone().with_complete_failure(
                crate::error::RuntimeFailureRelation::DiagnosticArchive,
                error,
            )
        } else if self.error.lifecycle.capacity.is_some() {
            error.with_related_failure("prior_capacity_admission", &self.error)
        } else {
            error
        };
        if error.lifecycle.task_timing.is_none() {
            error.lifecycle.task_timing = self.error.lifecycle.task_timing.clone();
        }
        Self {
            state: RuntimeReceiptState::Failed,
            terminal: self.terminal,
            error: Box::new(error.into_fatal()),
            poison_runtime: true,
            task_failure: self.task_failure,
        }
    }
}

#[cfg(test)]
mod request_failure_tests {
    use super::*;

    #[test]
    fn cleanup_escalation_preserves_the_original_task_failure_classification() {
        let original = RequestFailure {
            state: RuntimeReceiptState::Failed,
            terminal: None,
            error: Box::new(RuntimeHostError::request(
                "capture_backend_operation_failed",
                "run_contained_task_capture",
                RuntimeErrorCode::CaptureFailed,
            )),
            poison_runtime: false,
            task_failure: Some(TaskFailureEvidence {
                code: "capture_backend_operation_failed",
                severity: EventSeverity::Warning,
            }),
        };

        let escalated = original.replace_with_poison(RuntimeHostError::fatal(
            "lease_cleanup_failed",
            "release_failed_policy_run",
            RuntimeErrorCode::RuntimeFatal,
        ));

        assert_eq!(escalated.error.code(), "lease_cleanup_failed");
        assert!(escalated.poison_runtime);
        assert_eq!(
            escalated.task_failure,
            Some(TaskFailureEvidence {
                code: "capture_backend_operation_failed",
                severity: EventSeverity::Warning,
            })
        );
    }
}

impl From<RuntimeHostError> for RequestFailure {
    fn from(error: RuntimeHostError) -> Self {
        Self::poison_without_terminal(error)
    }
}

impl ActionFailure {
    pub(super) fn scheduler(error: RuntimeHostError) -> Self {
        Self {
            diagnostic: diagnostic_for_projection(error.projection()),
            effect: EffectDisposition::NotPerformed,
            poison_runtime: error.is_fatal(),
            release_after: false,
            destructive_started: false,
            transfer_after: error.code() == "lease_transfer_not_safe",
            task_failure: None,
            error,
        }
    }

    pub(super) fn backend(error: RuntimeHostError) -> Self {
        let task_failure = Some(Box::new(TaskFailureEvidence {
            code: error.code(),
            severity: if error.is_fatal() {
                EventSeverity::Fatal
            } else {
                EventSeverity::Warning
            },
        }));
        Self {
            diagnostic: DiagnosticCode::BackendOperationFailed,
            effect: EffectDisposition::Indeterminate,
            poison_runtime: false,
            release_after: true,
            destructive_started: true,
            transfer_after: false,
            task_failure,
            error,
        }
    }

    pub(super) fn poison(error: RuntimeHostError) -> Self {
        let error = error.into_fatal();
        Self {
            diagnostic: DiagnosticCode::RuntimeDiagnostic,
            effect: EffectDisposition::Indeterminate,
            poison_runtime: true,
            release_after: false,
            destructive_started: false,
            transfer_after: false,
            task_failure: None,
            error,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum ConnectionFailureStage {
    RequestRead,
    RequestDecode,
    RequestCache,
    Dispatch,
    ReceiptBuild,
    ReceiptWrite,
    ConnectionPanic,
}

impl ConnectionFailureStage {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::RequestRead => "runtime.ipc.request_read",
            Self::RequestDecode => "runtime.ipc.request_decode",
            Self::RequestCache => "runtime.ipc.request_cache",
            Self::Dispatch => "runtime.ipc.dispatch",
            Self::ReceiptBuild => "runtime.ipc.receipt_build",
            Self::ReceiptWrite => "runtime.ipc.receipt_write",
            Self::ConnectionPanic => "runtime.ipc.connection_panic",
        }
    }

    pub(super) const fn effect(self) -> EffectDisposition {
        match self {
            Self::RequestRead | Self::RequestDecode | Self::RequestCache => {
                EffectDisposition::NotPerformed
            }
            Self::Dispatch | Self::ReceiptBuild | Self::ReceiptWrite | Self::ConnectionPanic => {
                EffectDisposition::Indeterminate
            }
        }
    }
}

pub(super) struct ConnectionFailureContext {
    pub(super) connection_serial: u64,
    stage: Option<ConnectionFailureStage>,
    pub(super) request_decoded: bool,
    pub(super) links: EventLinksDraft,
    pub(super) timing: ConnectionTiming,
}

#[derive(Default, serde::Serialize)]
pub(super) struct ConnectionTiming {
    receive: ConnectionCallTiming,
    validated_dispatch: ConnectionCallTiming,
    policy_identity_projection: ConnectionCallTiming,
    receipt_write: ConnectionCallTiming,
}

#[derive(serde::Serialize)]
struct ConnectionCallTiming {
    status: TaskTimingObservationState,
    elapsed_us: Option<ObservedMicroseconds>,
    result: Option<TaskTimingResult>,
    #[serde(skip)]
    started: Option<Instant>,
}

impl Default for ConnectionCallTiming {
    fn default() -> Self {
        Self {
            status: TaskTimingObservationState::Unobserved,
            elapsed_us: None,
            result: None,
            started: None,
        }
    }
}

impl ConnectionCallTiming {
    fn begin(&mut self) {
        self.started = Some(Instant::now());
        self.status = TaskTimingObservationState::Incomplete {
            reason: TimingObservationIssue::CallIncomplete,
        };
    }

    fn finish(&mut self, succeeded: bool) {
        let ended = Instant::now();
        self.result = Some(if succeeded {
            TaskTimingResult::Ok
        } else {
            TaskTimingResult::Err
        });
        if let Some(started) = self.started {
            let elapsed = actingcommand_execution_kernel::observe_instant_span(started, ended);
            self.status = match elapsed {
                ObservedMicroseconds::Measured { .. } => TaskTimingObservationState::Observed,
                ObservedMicroseconds::Unavailable { reason } => {
                    TaskTimingObservationState::Incomplete { reason }
                }
            };
            self.elapsed_us = Some(elapsed);
        }
    }
}

pub(super) fn connection_boundary(
    mut stream: TcpStream,
    shared: Arc<HostShared>,
    connection_id: ConnectionId,
    connection_serial: u64,
    maximum_frame_bytes: usize,
    io_timeout: Duration,
) -> RuntimeHostResult<()> {
    let mut context = ConnectionFailureContext {
        connection_serial,
        stage: None,
        request_decoded: false,
        links: EventLinksDraft::default(),
        timing: ConnectionTiming::default(),
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        connection_loop(
            &mut stream,
            &shared,
            connection_id,
            maximum_frame_bytes,
            io_timeout,
            &mut context,
        )
    }));
    let mut failure = match result {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(_) => {
            context.stage = Some(ConnectionFailureStage::ConnectionPanic);
            Some(RuntimeHostError::fatal(
                "runtime_connection_panicked",
                "serve_runtime_connection",
                RuntimeErrorCode::RuntimeFatal,
            ))
        }
    };
    if let (Some(error), Some(stage)) = (&failure, context.stage)
        && let Err(append_error) = shared.append_connection_failure(&context, stage, error)
    {
        record_failure(&mut failure, Err(append_error));
    }
    drop(stream);
    let reason = if shared.fatal.is_shutdown_requested() {
        LeaseReleaseReason::HostShutdown
    } else {
        LeaseReleaseReason::Disconnect
    };
    let cleanup = (|| {
        let _work = shared.work_guard()?;
        shared.cleanup_connection(connection_id, reason)
    })();
    #[cfg(feature = "test-observation")]
    crate::test_observation::emit_connection(
        crate::test_observation::HostTestObservationPoint::ConnectionCleanupResult,
        if cleanup.is_ok() {
            crate::test_observation::HostTestObservationOutcome::Success
        } else {
            crate::test_observation::HostTestObservationOutcome::Error
        },
    );
    shared.record_lifecycle_result(
        RuntimeLifecycleFailureStage::ConnectionCleanup,
        &mut failure,
        cleanup,
    );
    if let Some(error) = failure {
        if error.is_fatal() {
            match shared.fatal.mark(error.clone()) {
                Ok(()) => Err(error),
                Err(mark) => Err(error.with_complete_failure(
                    crate::error::RuntimeFailureRelation::LifecycleRecord,
                    mark,
                )),
            }
        } else {
            Ok(())
        }
    } else {
        Ok(())
    }
}

fn connection_loop(
    stream: &mut TcpStream,
    shared: &HostShared,
    connection_id: ConnectionId,
    maximum_frame_bytes: usize,
    io_timeout: Duration,
    context: &mut ConnectionFailureContext,
) -> RuntimeHostResult<()> {
    stream
        .set_read_timeout(Some(io_timeout))
        .map_err(|_| protocol_error("set_read_timeout"))?;
    stream
        .set_write_timeout(Some(io_timeout))
        .map_err(|_| protocol_error("set_write_timeout"))?;
    stream
        .set_nodelay(true)
        .map_err(|_| protocol_error("set_tcp_nodelay"))?;
    let mut cache = RequestCache::default();
    while !shared.fatal.is_shutdown_requested() {
        context.stage = Some(ConnectionFailureStage::RequestRead);
        context.request_decoded = false;
        context.links = EventLinksDraft::default();
        context.timing = ConnectionTiming::default();
        context.timing.receive.begin();
        let received = read_frame(stream, maximum_frame_bytes);
        context.timing.receive.finish(received.is_ok());
        let frame = match received {
            Ok(FrameRead::Data(frame)) => frame,
            Ok(FrameRead::Idle) => continue,
            Ok(FrameRead::Closed) => {
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_connection(
                    crate::test_observation::HostTestObservationPoint::ConnectionExit,
                    crate::test_observation::HostTestObservationOutcome::Closed,
                );
                return Ok(());
            }
            Err(error) => {
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_connection(
                    crate::test_observation::HostTestObservationPoint::ConnectionExit,
                    crate::test_observation::HostTestObservationOutcome::Error,
                );
                return Err(error);
            }
        };
        context.stage = Some(ConnectionFailureStage::RequestDecode);
        let request = match serde_json::from_slice::<RuntimeRequest>(&frame) {
            Ok(request) => request,
            Err(_) => {
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_connection(
                    crate::test_observation::HostTestObservationPoint::ConnectionExit,
                    crate::test_observation::HostTestObservationOutcome::Error,
                );
                return Err(protocol_error("runtime_request_decode_failed"));
            }
        };
        context.request_decoded = true;
        let material_context = MaterialReadContext::for_request(&request, maximum_frame_bytes)?;
        // Idle sockets hold no admission. A decoded request remains in flight through its reply.
        let _work = if matches!(
            request.operation(),
            RuntimeOperation::RequestShutdown { .. }
        ) {
            None
        } else {
            shared.begin_work()?
        };
        if let Ok(validated) = request.validate() {
            context.links = validated.event_links(None, None, None);
        }
        #[cfg(feature = "test-observation")]
        crate::test_observation::emit_request(
            crate::test_observation::HostTestObservationPoint::FrameReceived,
            crate::test_observation::HostTestObservationOutcome::Success,
            &request,
        );
        #[cfg(feature = "test-observation")]
        crate::test_observation::emit_request(
            crate::test_observation::HostTestObservationPoint::DispatchStart,
            crate::test_observation::HostTestObservationOutcome::Started,
            &request,
        );
        context.stage = Some(ConnectionFailureStage::RequestCache);
        let receipt = match cache.get(&request) {
            Ok(_) if material_context.is_some() => {
                context.stage = Some(ConnectionFailureStage::Dispatch);
                shared.process_request_observed(
                    &request,
                    connection_id,
                    Some(&mut context.timing),
                    material_context,
                )
            }
            Ok(Some(receipt)) => Ok(receipt),
            Ok(None) => {
                context.stage = Some(ConnectionFailureStage::Dispatch);
                shared
                    .process_request_observed(
                        &request,
                        connection_id,
                        Some(&mut context.timing),
                        None,
                    )
                    .inspect(|receipt| {
                        cache.insert(request.clone(), receipt.clone());
                    })
            }
            Err(error) => Err(error),
        };
        let receipt = match receipt {
            Ok(receipt) => {
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_receipt(
                    crate::test_observation::HostTestObservationPoint::DispatchResult,
                    crate::test_observation::HostTestObservationOutcome::Success,
                    &request,
                    &receipt,
                );
                receipt
            }
            Err(error) => {
                if error.operation() == "build_runtime_receipt" {
                    context.stage = Some(ConnectionFailureStage::ReceiptBuild);
                }
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_request(
                    crate::test_observation::HostTestObservationPoint::DispatchResult,
                    crate::test_observation::HostTestObservationOutcome::Error,
                    &request,
                );
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_connection(
                    crate::test_observation::HostTestObservationPoint::ConnectionExit,
                    crate::test_observation::HostTestObservationOutcome::Error,
                );
                return Err(error);
            }
        };
        let (receipt, material_body) = if let Some(material) = material_context {
            let (receipt, body) = shared.prepare_material_reply(&request, receipt, material)?;
            (receipt, Some(body))
        } else {
            (receipt, None)
        };
        #[cfg(feature = "test-observation")]
        crate::test_observation::emit_receipt(
            crate::test_observation::HostTestObservationPoint::ReceiptWriteStart,
            crate::test_observation::HostTestObservationOutcome::Started,
            &request,
            &receipt,
        );
        context.stage = Some(ConnectionFailureStage::ReceiptWrite);
        context.timing.receipt_write.begin();
        let written = match (&material_body, material_context) {
            (Some(body), Some(material)) => {
                crate::ipc::write_encoded_frame(stream, body, material.max_reply_bytes)
            }
            _ => write_frame(stream, &receipt, maximum_frame_bytes),
        };
        context.timing.receipt_write.finish(written.is_ok());
        // Slice #316-B4: a stuck-recovery ladder triggered by this request is admitted only
        // now that its receipt was written (or the write failed); it never runs here.
        shared.release_parked_recovery_ladder(request.request_id())?;
        match written {
            Ok(()) => {
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_receipt(
                    crate::test_observation::HostTestObservationPoint::ReceiptWriteResult,
                    crate::test_observation::HostTestObservationOutcome::Success,
                    &request,
                    &receipt,
                );
            }
            Err(error) => {
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_receipt(
                    crate::test_observation::HostTestObservationPoint::ReceiptWriteResult,
                    crate::test_observation::HostTestObservationOutcome::Error,
                    &request,
                    &receipt,
                );
                #[cfg(feature = "test-observation")]
                crate::test_observation::emit_connection(
                    crate::test_observation::HostTestObservationPoint::ConnectionExit,
                    crate::test_observation::HostTestObservationOutcome::Error,
                );
                return Err(error);
            }
        }
    }
    #[cfg(feature = "test-observation")]
    crate::test_observation::emit_connection(
        crate::test_observation::HostTestObservationPoint::ConnectionExit,
        crate::test_observation::HostTestObservationOutcome::Shutdown,
    );
    Ok(())
}

#[derive(Default)]
struct RequestCache {
    entries: BTreeMap<RequestId, (RuntimeRequest, RuntimeReceipt)>,
    order: VecDeque<RequestId>,
}

impl RequestCache {
    fn get(&self, request: &RuntimeRequest) -> RuntimeHostResult<Option<RuntimeReceipt>> {
        let Some((original, receipt)) = self.entries.get(&request.request_id()) else {
            return Ok(None);
        };
        if original != request {
            return Err(protocol_error("runtime_request_id_reused"));
        }
        Ok(Some(receipt.clone()))
    }

    fn insert(&mut self, request: RuntimeRequest, receipt: RuntimeReceipt) {
        let request_id = request.request_id();
        self.entries.insert(request_id, (request, receipt));
        self.order.push_back(request_id);
        while self.order.len() > MAX_REQUEST_CACHE_ENTRIES {
            if let Some(expired) = self.order.pop_front() {
                self.entries.remove(&expired);
            }
        }
    }
}

impl HostShared {
    pub(super) fn append_request_lifecycle(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: InstanceId,
        action: EventAction,
        run_links: Option<RuntimeRunLinks>,
    ) -> Result<(), RequestFailure> {
        let links =
            self.append_client_command_intent(original, request, instance_id, action, run_links)?;
        self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            CommandPayloadDraft::validated(
                action,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            ),
        )?;
        Ok(())
    }

    pub(super) fn append_scheduled_request_lifecycle(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: InstanceId,
        run_links: RuntimeRunLinks,
        execution_provenance: ExecutionBackendProvenance,
    ) -> Result<(), RequestFailure> {
        let expected_origin = scheduled_request_transport_origin(execution_provenance);
        if (original.actor(), original.source()) != expected_origin {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    "policy_task_request_origin_mismatch",
                    "append_scheduled_request_lifecycle",
                    RuntimeErrorCode::RuntimeFatal,
                ),
            ));
        }
        if execution_provenance == ExecutionBackendProvenance::FixtureSimulation {
            return self.append_request_lifecycle(
                original,
                request,
                instance_id,
                EventAction::RuntimeTaskRun,
                Some(run_links),
            );
        }
        let links =
            run_links.apply(
                self.events
                    .request_links(request, Some(instance_id), None, None),
            );
        self.append_event(
            EventSeverity::Info,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            links.clone(),
            CommandPayloadDraft::received(EventAction::RuntimeTaskRun, AuditInput::new()),
        )?;
        self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            CommandPayloadDraft::validated(
                EventAction::RuntimeTaskRun,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            ),
        )?;
        Ok(())
    }

    pub(super) fn append_client_command_intent(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: InstanceId,
        action: EventAction,
        run_links: Option<RuntimeRunLinks>,
    ) -> Result<EventLinksDraft, RequestFailure> {
        self.validate_c4_client_source(original)?;
        let (source, module, payload) = match original.source() {
            EventSource::Cli => (
                EventSource::Cli,
                OriginModule::Actingctl,
                ClientPayloadDraft::cli_command(action, AuditInput::new()),
            ),
            EventSource::Lab => (
                EventSource::Lab,
                OriginModule::Actinglab,
                ClientPayloadDraft::lab_request(action, AuditInput::new()),
            ),
            EventSource::Ui => (
                EventSource::Ui,
                OriginModule::Runtime,
                ClientPayloadDraft::ui_action(action, AuditInput::new()),
            ),
            EventSource::Adapter
            | EventSource::Runtime
            | EventSource::Scheduler
            | EventSource::Device
            | EventSource::System => {
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        "c4_client_source_unsupported",
                        "append_request_lifecycle",
                        RuntimeErrorCode::InvalidRequest,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                ));
            }
        };
        let mut links = self
            .events
            .request_links(request, Some(instance_id), None, None);
        if let Some(run_links) = run_links {
            links = run_links.apply(links);
        }
        self.append_event(
            EventSeverity::Info,
            source,
            module,
            original.actor(),
            links.clone(),
            payload,
        )?;
        self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            CommandPayloadDraft::received(action, AuditInput::new()),
        )?;
        Ok(links)
    }

    pub(super) fn validate_c4_client_source(
        &self,
        request: &RuntimeRequest,
    ) -> Result<(), RequestFailure> {
        if matches!(
            request.source(),
            EventSource::Cli | EventSource::Lab | EventSource::Ui
        ) {
            return Ok(());
        }
        Err(RequestFailure::request(
            RuntimeHostError::request(
                "c4_client_source_unsupported",
                "append_request_lifecycle",
                RuntimeErrorCode::InvalidRequest,
            ),
            RuntimeReceiptState::Denied,
            None,
        ))
    }
}

fn runtime_error_receipt(
    request: &RuntimeRequest,
    state: RuntimeReceiptState,
    terminal: Option<TerminalEvent>,
    error: RuntimeErrorProjection,
) -> RuntimeHostResult<RuntimeReceipt> {
    RuntimeReceipt::error(request, state, terminal, error).map_err(|_| receipt_error())
}

pub(super) fn terminal(event: &PersistedEvent) -> TerminalEvent {
    TerminalEvent {
        sequence: event.sequence(),
        event_id: *event.event_id(),
    }
}

pub(super) fn diagnostic_for_projection(projection: &RuntimeErrorProjection) -> DiagnosticCode {
    match projection.code {
        RuntimeErrorCode::LeaseBusy => DiagnosticCode::LeaseBusy,
        RuntimeErrorCode::LeaseCooldown => DiagnosticCode::LeaseCooldown,
        RuntimeErrorCode::LeaseExpired => DiagnosticCode::LeaseExpired,
        RuntimeErrorCode::BackendOpenFailed => DiagnosticCode::BackendOpenFailed,
        RuntimeErrorCode::BackendOperationFailed => DiagnosticCode::BackendOperationFailed,
        _ => DiagnosticCode::LeaseFencingDenied,
    }
}

pub(super) fn policy_id_error(operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        "policy_identifier_issue_failed",
        operation,
        RuntimeErrorCode::RuntimeFatal,
    )
}

pub(super) fn policy_contract_error(operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        "policy_runtime_contract_invalid",
        operation,
        RuntimeErrorCode::RuntimeFatal,
    )
}

pub(super) fn policy_admission_request(
    code: &'static str,
    operation: &'static str,
) -> RuntimeHostError {
    RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest)
}

pub(super) fn policy_admission_fatal(
    code: &'static str,
    operation: &'static str,
) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeFatal)
}

pub(super) fn critical_execution_error<E>(error: &CriticalExecutionError<E>) -> RuntimeHostError {
    match error {
        CriticalExecutionError::IntentAppend(_) => ledger_error("append_critical_intent"),
        CriticalExecutionError::OutcomeUndurable { .. } => ledger_error("append_critical_outcome"),
        CriticalExecutionError::Action { .. } => RuntimeHostError::fatal(
            "critical_action_mapping_invalid",
            "map_critical_result",
            RuntimeErrorCode::RuntimeFatal,
        ),
    }
}

pub(super) fn critical_plan_error() -> RuntimeHostError {
    RuntimeHostError::fatal(
        "critical_event_plan_invalid",
        "build_critical_event",
        RuntimeErrorCode::RuntimeFatal,
    )
}

pub(super) fn client_fact_conflict(code: &'static str, operation: &'static str) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest),
        RuntimeReceiptState::Denied,
        None,
    )
}

pub(super) fn receipt_error() -> RuntimeHostError {
    RuntimeHostError::fatal(
        "runtime_receipt_invalid",
        "build_runtime_receipt",
        RuntimeErrorCode::RuntimeFatal,
    )
}

pub(super) fn protocol_error(operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(
        "runtime_protocol_invalid",
        operation,
        RuntimeErrorCode::ProtocolInvalid,
    )
}
