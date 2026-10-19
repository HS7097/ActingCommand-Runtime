// SPDX-License-Identifier: AGPL-3.0-only

use super::facts::FactPublicationPurpose;
use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::ipc::{FrameRead, FrameStart, read_started_frame, wait_frame_start, write_frame};
use actingcommand_contract::outcome::{Code, Location};
use actingcommand_contract::{ChannelBinding, InterfaceBlock, RuntimeChannel};
use actingcommand_scheduler::codes::SchedulerCode;
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
        timing: Option<&mut ConnectionTiming>,
        material: Option<MaterialReadContext>,
    ) -> RuntimeHostResult<RuntimeReceipt> {
        // Workflow #389 U-2 (model section 3.1): the door knows the connection's channel, tags
        // the request and its receipt with it and hands the request on. Until U-6 it marks and
        // never refuses, by channel or by Lab option (section 4.2).
        let channel = self.door_channel(request, connection_id)?;
        let receipt =
            self.process_request_at_door(request, channel, connection_id, timing, material)?;
        // A BindChannel's receipt carries the binding it made.
        let channel = if matches!(request.operation(), RuntimeOperation::BindChannel { .. }) {
            self.connection_channel(connection_id)?
        } else {
            channel
        };
        Ok(receipt.with_channel(channel))
    }

    /// The channel the door tags `request` with: its connection's binding. Until U-6 a
    /// connection that has not bound is bound at its first request other than the handshake,
    /// from that request's envelope source (model section 3.2, transition): `Ui` → `ui`, `Lab` →
    /// `lab`, `Cli` and `Adapter` → `cli`. The recorded value is always one of the four; the
    /// handshake of a connection not bound yet has none.
    fn door_channel(
        &self,
        request: &RuntimeRequest,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<Option<ChannelBinding>> {
        let mut channels = lock(
            &self.connection_channels,
            HostLocation::ReadConnectionChannel,
        )?;
        if let Some(binding) = channels.get(&connection_id) {
            return Ok(Some(*binding));
        }
        if request.operation().kind().interface_class().block == InterfaceBlock::Handshake {
            return Ok(None);
        }
        let channel = match request.source() {
            EventSource::Ui => RuntimeChannel::Ui,
            EventSource::Lab => RuntimeChannel::Lab,
            EventSource::Cli | EventSource::Adapter => RuntimeChannel::Cli,
            // The envelope check refuses every other source before any handler runs.
            EventSource::Runtime
            | EventSource::Scheduler
            | EventSource::Device
            | EventSource::System => return Ok(None),
        };
        let binding = ChannelBinding::new(channel);
        channels.insert(connection_id, binding);
        Ok(Some(binding))
    }

    /// The channel `connection_id` is bound to, if it is (Workflow #389 U-2).
    pub(super) fn connection_channel(
        &self,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<Option<ChannelBinding>> {
        Ok(lock(
            &self.connection_channels,
            HostLocation::ReadConnectionChannel,
        )?
        .get(&connection_id)
        .copied())
    }

    fn process_request_at_door(
        &self,
        request: &RuntimeRequest,
        channel: Option<ChannelBinding>,
        connection_id: ConnectionId,
        mut timing: Option<&mut ConnectionTiming>,
        material: Option<MaterialReadContext>,
    ) -> RuntimeHostResult<RuntimeReceipt> {
        // Workflow #378 A6a (model section 2.3): a latched host answers every later request by
        // the latching condition, under `runtime_halted` unless the condition is fatal.
        if let Some(error) = self.fatal.answer()? {
            return runtime_failure_receipt(
                request,
                RuntimeReceiptState::Failed,
                None,
                error.receipt_outcome(),
            );
        }
        let validated = match request.validate() {
            Ok(validated) => validated.with_channel(channel),
            Err(error) => {
                if let RuntimeOperation::InstallTransition { action, .. } = request.operation() {
                    let failure = self.invalid_install_request(action, error.code())?;
                    return runtime_failure_receipt(
                        request,
                        failure.state,
                        failure.terminal,
                        failure.error.receipt_outcome(),
                    );
                }
                // Workflow #378 A4 (model section 7.2, L3): the denied receipt carries its
                // refusal, the contract's validation code, over `invalid_request`.
                return runtime_failure_receipt(
                    request,
                    RuntimeReceiptState::Denied,
                    None,
                    RuntimeErrorProjection::new(RuntimeErrorCode::InvalidRequest, false)
                        .receipt_outcome(Some(error.code()), None),
                );
            }
        };
        let _work = if Self::request_is_lifecycle_control(request.operation()) {
            None
        } else {
            match self.begin_request(request.operation(), connection_id)? {
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
                #[cfg(test)]
                {
                    lock(
                        &self.failure_outcomes_for_test,
                        crate::codes::test_spellings::TestLocation::RecordFailureDetailForTest,
                    )?
                    .insert(request.request_id(), failure.error.outcome());
                }
                let outcome = failure.error.receipt_outcome();
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
                    return runtime_failure_receipt(
                        request,
                        failure.state,
                        failure.terminal,
                        outcome,
                    )?
                    .with_resource_declaration(rejection, rejected)
                    .map_err(|_| receipt_error());
                }
                // Workflow #308 RT-S1a: a refused resource target document is answered with
                // its field position and records nothing.
                if let Some(rejection) = failure.error.resource_targets_rejection().cloned() {
                    return runtime_failure_receipt(
                        request,
                        failure.state,
                        failure.terminal,
                        outcome,
                    )?
                    .with_resource_targets_rejection(rejection)
                    .map_err(|_| receipt_error());
                }
                runtime_failure_receipt(request, failure.state, failure.terminal, outcome)
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
                        HostLocation::MaterialReadContextMissing,
                    ))
                })?,
            ),
            RuntimeOperation::RequestShutdown { target } => {
                self.request_shutdown(validated, *target)
            }
            RuntimeOperation::InstallTransition { target, action } => {
                self.install_transition(validated, *target, action, connection_id)
            }
            RuntimeOperation::Health => Ok(OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: None,
                result: RuntimeResult::Health {
                    owner_epoch: self.owner_epoch,
                },
                failure_outcome: None,
            }),
            RuntimeOperation::Status => self.control_plane_status(),
            RuntimeOperation::DiscoverInstances => self.discover_instances(validated),
            RuntimeOperation::ProjectInterface { request } => self.project_interface(request),
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
            RuntimeOperation::MonitorStatus => self.monitor_status(),
            // Workflow #385 P-m (register §1.61 W15): the shadow measurement's status, a general
            // read from memory that records nothing.
            RuntimeOperation::PerformanceStatus => self.performance_status_read(),
            // Workflow #384 D2-S5 (register §1.68): the fact refresh's waiting keys, a general
            // read from memory that records nothing.
            RuntimeOperation::FactRefreshStatus => self.fact_refresh_status_read(request),
            // Workflow #386 SR-6b, MCP-19a (register §1.143): the latest evaluation's not-run
            // explanation, a general read from the policy host's memory that records nothing.
            RuntimeOperation::ScheduleExplanation {
                instance,
                candidates,
            } => self.schedule_explanation_read(request, instance.as_deref(), *candidates),
            RuntimeOperation::RuntimeFactSnapshot => {
                let snapshot = self.runtime_fact_snapshot().map_err(|error| {
                    if error.is_fatal() {
                        RequestFailure::poison_without_terminal(error)
                    } else {
                        RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                    }
                })?;
                let result = RuntimeResult::RuntimeFactSnapshot { snapshot };
                // Workflow #308 5d-1: a store whose reply exceeds one IPC frame is refused
                // here instead of failing the frame write; paged reads are a follow-up.
                let reply_bytes = RuntimeReceipt::success(
                    request,
                    RuntimeReceiptState::Completed,
                    None,
                    result.clone(),
                )
                .ok()
                .and_then(|receipt| serde_json::to_vec(&receipt).ok())
                .ok_or_else(|| RequestFailure::poison_without_terminal(receipt_error()))?
                .len();
                if reply_bytes > self.maximum_frame_bytes {
                    return Err(RequestFailure::request(
                        RuntimeHostError::request(
                            HostCode::RuntimeFactSnapshotTooLargeForFrame,
                            HostLocation::ReadRuntimeFactSnapshot,
                            RuntimeErrorCode::InvalidRequest,
                        )
                        .with_native_detail(format!(
                            "reply is {reply_bytes} bytes; one frame carries at most {} bytes",
                            self.maximum_frame_bytes
                        )),
                        RuntimeReceiptState::Denied,
                        None,
                    ));
                }
                Ok(OperationSuccess {
                    state: RuntimeReceiptState::Completed,
                    terminal: None,
                    result,
                    failure_outcome: None,
                })
            }
            // Workflow #389 MCP-4 (register W2): one instance's facts, a general read that
            // records nothing.
            RuntimeOperation::InstanceFactSnapshot { instance } => {
                self.instance_fact_snapshot_read(request, instance)
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
                    kind: ClaimKind::ClientLease,
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
                let reset = self.safe_reset(
                    request,
                    validated,
                    instance_alias,
                    *holder_id,
                    connection_id,
                );
                // Workflow #191 ps2: the reset this connection owed after a cancelled contained
                // run has finished; an instance pause may now hand the device back.
                match self.settle_client_reset(instance_alias, connection_id) {
                    Ok(()) => reset,
                    Err(error) => Err(match reset {
                        Ok(_) => RequestFailure::poison_without_terminal(error),
                        Err(failure) => failure.replace_with_poison(error),
                    }),
                }
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
                self.control_emulator_instance(
                    request,
                    validated,
                    instance_alias,
                    *action,
                    connection_id,
                )
            }
            // Workflow #191 ps1: an instance pause applies to physical instances only.
            RuntimeOperation::PauseScheduling {
                scope,
                pause_reason,
                drain_timeout_ms,
            } => {
                if let Some(instance_alias) = scope.instance_alias() {
                    self.require_physical_instance_alias(instance_alias)?;
                }
                self.pause_scheduling(validated, scope, pause_reason, *drain_timeout_ms)
            }
            RuntimeOperation::ResumeScheduling { scope, expected } => {
                if let Some(instance_alias) = scope.instance_alias() {
                    self.require_physical_instance_alias(instance_alias)?;
                }
                self.resume_scheduling(request, validated, scope, expected.as_ref(), connection_id)
            }
            // Workflow #317 sc3 (d): a manual reconnect and self-check of a physical instance.
            RuntimeOperation::SelfCheckInstance { instance_alias } => {
                self.require_physical_instance_alias(instance_alias)?;
                self.self_check_instance(request, validated, instance_alias, connection_id)
            }
            // Workflow #390 DRILL-1: a person's drill of a physical instance's ladder.
            RuntimeOperation::DrillRecoveryLadder {
                instance_alias,
                mode,
            } => {
                self.require_physical_instance_alias(instance_alias)?;
                self.drill_recovery_ladder(request, validated, instance_alias, *mode)
            }
            RuntimeOperation::RunContainedTask {
                instance_alias,
                holder_id,
                request: task_request,
            } => {
                self.require_physical_instance_alias(instance_alias)?;
                // Red proof M9 (never merge): a waiting draw holds the instance.
                if lock(self.policy()?, HostLocation::ReadPolicyOutcomeKeys)?
                    .random_delay_waiting(instance_alias)
                {
                    return Err(RuntimeHostError::request(
                        HostCode::DispatchRandomDelayWaiting,
                        HostLocation::AdmitPolicyDispatch,
                        RuntimeErrorCode::InvalidRequest,
                    )
                    .into());
                }
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
                        FactPublicationPurpose::Ordinary,
                    )
                    .map(|publication| publication.event_id)
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
                    failure_outcome: None,
                })
            }
            RuntimeOperation::PublishFacts { observation } => {
                let event_id = self
                    .publish_facts(
                        observation.clone(),
                        Some(validated),
                        FactPublicationPurpose::Ordinary,
                    )
                    .map(|publication| publication.event_id)
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
                    failure_outcome: None,
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
            RuntimeOperation::ApplyResourceTargets { document_json } => {
                self.apply_resource_targets(validated, document_json)
            }
            RuntimeOperation::ResourceTargetView { instance_alias } => {
                self.resource_target_view(instance_alias)
            }
            // Workflow #387 D1b: a general instruction of the three counters.
            RuntimeOperation::ConfigureSettings { changes } => {
                self.configure_settings(request, validated, changes)
            }
            // Workflow #389 U-1: the contract has these shapes and later slices add the
            // behaviour (LoadPack and UnloadPack U-3, UpgradeState and UpgradeStateStatus U-4).
            // Until then each is refused and records nothing.
            // Workflow #389 U-2.
            RuntimeOperation::BindChannel { channel, via } => self.bind_channel(
                connection_id,
                ChannelBinding {
                    channel: *channel,
                    via: *via,
                },
            ),
            RuntimeOperation::LoadPack { .. } => Err(not_implemented(HostLocation::LoadPack)),
            RuntimeOperation::UnloadPack { .. } => Err(not_implemented(HostLocation::UnloadPack)),
            RuntimeOperation::UpgradeState { .. } => {
                Err(not_implemented(HostLocation::UpgradeState))
            }
            RuntimeOperation::UpgradeStateStatus { .. } => {
                Err(not_implemented(HostLocation::UpgradeStateStatus))
            }
        }
    }
}

/// A request kind whose behaviour a later slice adds: Denied with the shared code
/// `not_implemented` (Workflow #389 U-1).
fn not_implemented(operation: HostLocation) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(
            actingcommand_contract::codes::SharedCode::NotImplemented,
            operation,
            RuntimeErrorCode::InvalidRequest,
        ),
        RuntimeReceiptState::Denied,
        None,
    )
}

pub(super) struct RequestFailure {
    pub(super) state: RuntimeReceiptState,
    pub(super) terminal: Option<TerminalEvent>,
    pub(super) error: Box<RuntimeHostError>,
    pub(super) poison_runtime: bool,
    pub(super) task_failure: Option<TaskFailureEvidence>,
}

/// The original task failure of a run, kept while a cleanup failure replaces its error: its
/// outcome is the run's terminal outcome (ledger revision 3), and `severe` is the verdict where
/// it arose that it must pause the task (its error was fatal there; the terminal's
/// `failure_severe`, which the policy class follows; Workflow #378 A3 ruling 9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TaskFailureEvidence {
    pub(super) outcome: actingcommand_contract::outcome::Outcome,
    pub(super) severe: bool,
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
                RuntimeReceipt::material_read(request, self.terminal, result, self.failure_outcome)
            }
            RuntimeResult::ContainedLabOperation { operation } => {
                RuntimeReceipt::contained_lab_operation(
                    request,
                    self.terminal.ok_or_else(receipt_error)?,
                    operation,
                    self.failure_outcome,
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
                actingcommand_contract::codes::SharedCode::CaptureBackendOperationFailed,
                crate::codes::HostLocation::RunContainedTaskCapture,
                RuntimeErrorCode::CaptureFailed,
            )),
            poison_runtime: false,
            task_failure: Some(TaskFailureEvidence {
                outcome: actingcommand_contract::outcome::Outcome::new(
                    actingcommand_contract::codes::SharedCode::CaptureBackendOperationFailed,
                )
                .operation(crate::codes::HostLocation::RunContainedTaskCapture),
                severe: false,
            }),
        };

        let escalated = original.replace_with_poison(RuntimeHostError::fatal(
            crate::codes::test_spellings::TestCode::LeaseCleanupFailed,
            crate::codes::HostLocation::ReleaseFailedPolicyRun,
            RuntimeErrorCode::RuntimeHalted,
        ));

        assert_eq!(escalated.error.code(), "lease_cleanup_failed");
        assert!(escalated.poison_runtime);
        assert_eq!(
            escalated.task_failure,
            Some(TaskFailureEvidence {
                outcome: actingcommand_contract::outcome::Outcome::new(
                    actingcommand_contract::codes::SharedCode::CaptureBackendOperationFailed,
                )
                .operation(crate::codes::HostLocation::RunContainedTaskCapture),
                severe: false,
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
            transfer_after: error.code() == SchedulerCode::LeaseTransferNotSafe.as_str(),
            task_failure: None,
            error,
        }
    }

    pub(super) fn backend(error: RuntimeHostError) -> Self {
        let task_failure = Some(Box::new(TaskFailureEvidence {
            outcome: error.outcome(),
            severe: error.is_fatal(),
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
    /// The stage's registered location, the stage of the connection failure's diagnostic
    /// detail (Workflow #378 A2-P; it was a dotted `runtime.ipc.*` token).
    pub(super) const fn location(self) -> HostLocation {
        match self {
            Self::RequestRead => HostLocation::RuntimeIpcRequestRead,
            Self::RequestDecode => HostLocation::RuntimeIpcRequestDecode,
            Self::RequestCache => HostLocation::RuntimeIpcRequestCache,
            Self::Dispatch => HostLocation::RuntimeIpcDispatch,
            Self::ReceiptBuild => HostLocation::RuntimeIpcReceiptBuild,
            Self::ReceiptWrite => HostLocation::RuntimeIpcReceiptWrite,
            Self::ConnectionPanic => HostLocation::RuntimeIpcConnectionPanic,
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

/// A connection's failure context. Ledger revision 3 records its failure as the record's
/// outcome; the former connection serial, decode flag and timing summary were Runtime text and
/// are not recorded (the timings are still taken per request).
pub(super) struct ConnectionFailureContext {
    stage: Option<ConnectionFailureStage>,
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
    maximum_frame_bytes: usize,
    io_timeout: Duration,
) -> RuntimeHostResult<()> {
    let mut context = ConnectionFailureContext {
        stage: None,
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
                HostCode::RuntimeConnectionPanicked,
                HostLocation::ServeRuntimeConnection,
                RuntimeErrorCode::RuntimeHalted,
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
    // Every read of a started frame is bounded by io_timeout; the idle wait before a frame is a
    // non-blocking poll (ipc::wait_frame_start), so no receive timeout expires on an idle socket.
    stream
        .set_read_timeout(Some(io_timeout))
        .map_err(|_| protocol_error(HostLocation::SetReadTimeout))?;
    stream
        .set_write_timeout(Some(io_timeout))
        .map_err(|_| protocol_error(HostLocation::SetWriteTimeout))?;
    stream
        .set_nodelay(true)
        .map_err(|_| protocol_error(HostLocation::SetTcpNodelay))?;
    let mut cache = RequestCache::default();
    while !shared.fatal.is_shutdown_requested() {
        context.stage = Some(ConnectionFailureStage::RequestRead);
        context.links = EventLinksDraft::default();
        context.timing = ConnectionTiming::default();
        context.timing.receive.begin();
        let received = match wait_frame_start(stream, || shared.fatal.is_shutdown_requested()) {
            Ok(FrameStart::Ready) => {
                // Time the frame itself, not the idle wait before it.
                context.timing.receive.begin();
                read_started_frame(stream, maximum_frame_bytes)
            }
            Ok(FrameStart::Stopped) => Ok(FrameRead::Idle),
            Ok(FrameStart::Closed) => Ok(FrameRead::Closed),
            Err(error) => Err(error),
        };
        context.timing.receive.finish(received.is_ok());
        let frame = match received {
            Ok(FrameRead::Data(frame)) => frame,
            // Only after FrameStart::Stopped: the loop condition then ends the connection.
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
                return Err(protocol_error(HostLocation::RuntimeRequestDecodeFailed));
            }
        };
        let material_context = MaterialReadContext::for_request(&request, maximum_frame_bytes)?;
        // Idle sockets hold no admission. A decoded request remains in flight through its reply.
        let _work = shared.begin_request(request.operation(), connection_id)?;
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
        // Workflow #378 A4 (model section 7.1): the receipt leaves with the detail its request
        // asks for, filtered once, here; the request cache keeps the full receipt.
        let receipt = receipt.map(|receipt| receipt.for_request_detail(&request));
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
                if error.operation() == HostLocation::BuildRuntimeReceipt.as_str() {
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
            return Err(protocol_error(HostLocation::RuntimeRequestIdReused));
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
                    HostCode::PolicyTaskRequestOriginMismatch,
                    HostLocation::AppendScheduledRequestLifecycle,
                    RuntimeErrorCode::RuntimeHalted,
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
        self.append_scoped_client_command_intent(
            original,
            request,
            Some(instance_id),
            action,
            run_links,
        )
    }

    /// [`Self::append_client_command_intent`] of a request that may concern no instance
    /// (Workflow #387 D1b: `ConfigureSettings` is Runtime-wide).
    pub(super) fn append_scoped_client_command_intent(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_id: Option<InstanceId>,
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
                        HostCode::C4ClientSourceUnsupported,
                        HostLocation::AppendRequestLifecycle,
                        RuntimeErrorCode::InvalidRequest,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                ));
            }
        };
        let mut links = self.events.request_links(request, instance_id, None, None);
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
        // Workflow #389 U-2 (model section 5.2): the `command.received` the C4 path writes
        // carries the channel the door tagged the request with.
        self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            CommandPayloadDraft::received_on_channel(action, request.channel(), AuditInput::new()),
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
                HostCode::C4ClientSourceUnsupported,
                HostLocation::AppendRequestLifecycle,
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

/// A denied or failed receipt carrying its refusal or failure as `outcome` (Workflow #378 A4).
fn runtime_failure_receipt(
    request: &RuntimeRequest,
    state: RuntimeReceiptState,
    terminal: Option<TerminalEvent>,
    outcome: actingcommand_contract::outcome::Outcome,
) -> RuntimeHostResult<RuntimeReceipt> {
    RuntimeReceipt::failure(request, state, terminal, outcome).map_err(|_| receipt_error())
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

pub(super) fn policy_id_error(operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::fatal(
        HostCode::PolicyIdentifierIssueFailed,
        operation,
        RuntimeErrorCode::RuntimeHalted,
    )
}

pub(super) fn policy_contract_error(operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::fatal(
        HostCode::PolicyRuntimeContractInvalid,
        operation,
        RuntimeErrorCode::RuntimeHalted,
    )
}

pub(super) fn policy_admission_request(
    code: impl Into<Code>,
    operation: impl Into<Location>,
) -> RuntimeHostError {
    RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest)
}

pub(super) fn policy_admission_fatal(
    code: impl Into<Code>,
    operation: impl Into<Location>,
) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeHalted)
}

pub(super) fn critical_execution_error<E>(error: &CriticalExecutionError<E>) -> RuntimeHostError {
    match error {
        CriticalExecutionError::IntentAppend(_) => ledger_error(HostLocation::AppendCriticalIntent),
        CriticalExecutionError::OutcomeUndurable { .. } => {
            ledger_error(HostLocation::AppendCriticalOutcome)
        }
        CriticalExecutionError::Action { .. } => RuntimeHostError::fatal(
            HostCode::CriticalActionMappingInvalid,
            HostLocation::MapCriticalResult,
            RuntimeErrorCode::RuntimeHalted,
        ),
    }
}

pub(super) fn critical_plan_error() -> RuntimeHostError {
    RuntimeHostError::fatal(
        HostCode::CriticalEventPlanInvalid,
        HostLocation::BuildCriticalEvent,
        RuntimeErrorCode::RuntimeHalted,
    )
}

pub(super) fn client_fact_conflict(
    code: impl Into<Code>,
    operation: impl Into<Location>,
) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest),
        RuntimeReceiptState::Denied,
        None,
    )
}

/// The host could not build a receipt: its own code `runtime_receipt_build_failed` (Workflow #378
/// A2-P; it was the runtime client's `runtime_receipt_invalid`, a receipt the client cannot
/// read, one spelling with two meanings, M378:90).
pub(super) fn receipt_error() -> RuntimeHostError {
    RuntimeHostError::fatal(
        HostCode::RuntimeReceiptBuildFailed,
        HostLocation::BuildRuntimeReceipt,
        RuntimeErrorCode::RuntimeHalted,
    )
}

pub(super) fn protocol_error(operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::request(
        HostCode::RuntimeProtocolInvalid,
        operation,
        RuntimeErrorCode::ProtocolInvalid,
    )
}
