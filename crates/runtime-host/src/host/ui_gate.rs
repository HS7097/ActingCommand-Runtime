// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-6S3 (`contracts/lab-two-entries.md` §2.3 step 5, §2.6, §3; register
//! "Slice LAB-6S3"): the UI list on the `ui` channel. On a connection bound to `ui` the one gate
//! (`process_request_observed`, after the request's own checks and before any work begins) takes
//! a handshake kind and a kind of the UI list ([`ui_allows`]), a `ConfigureSettings` only when
//! every change's key has the console tier `daily` or `advanced` and is settable; it refuses every
//! other request there, Denied `channel_request_refused` with `channel` `ui`, `operation` the kind
//! and, for a setting, `config_key` the first key refused. However the connection came to be on
//! `ui` (its `BindChannel`, or the binding its first request's envelope made) the rule is the
//! same. Every other connection passes as before: `lab`, `cli`, `mcp` and an unbound connection
//! are judged by nothing here. A refused request that G14 copies (a write, or a read that records)
//! is recorded `command.rejected` with its channel; a refused pure read is only in its receipt.

use super::*;
use actingcommand_contract::{ChannelBinding, RuntimeChannel, RuntimeOperationKind, ui_allows};
use actingcommand_runtime_config::{UiTier, find_row, ui_tier};

/// A request the gate refused on a `ui` connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiRefusal {
    /// The binding of the connection the request came on, the record's channel.
    pub(crate) binding: ChannelBinding,
    /// The refused request's kind.
    pub(crate) kind: RuntimeOperationKind,
    /// For a `ConfigureSettings` of a kind the list holds: the first key, in the request's
    /// order, that the console may not change.
    pub(crate) config_key: Option<String>,
}

/// The gate's decision on the request `operation` of a connection bound to `channel` (§2.3 steps
/// 1 and 5): `None` takes it, `Some` refuses it. Only a connection bound to `ui` is judged.
pub(crate) fn ui_refusal(
    operation: &RuntimeOperation,
    channel: Option<ChannelBinding>,
) -> Option<UiRefusal> {
    let binding = channel.filter(|binding| binding.channel == RuntimeChannel::Ui)?;
    let kind = operation.kind();
    if !ui_takes_kind(kind) {
        return Some(UiRefusal {
            binding,
            kind,
            config_key: None,
        });
    }
    let RuntimeOperation::ConfigureSettings { changes, .. } = operation else {
        return None;
    };
    changes
        .iter()
        .find(|change| !ui_may_set(&change.key))
        .map(|change| UiRefusal {
            binding,
            kind,
            config_key: Some(change.key.clone()),
        })
}

/// Whether the `ui` channel takes a request of `kind` (§2.3 steps 1 and 5): a handshake kind
/// (`Health`, `BindChannel`) or a kind of the UI list. The identity card, which the request
/// table still files with the handshake, is no kind of the list: `ui` refuses it.
pub(crate) const fn ui_takes_kind(kind: RuntimeOperationKind) -> bool {
    matches!(
        kind,
        RuntimeOperationKind::Health | RuntimeOperationKind::BindChannel
    ) || ui_allows(kind)
}

/// Whether the `ui` channel may change the configuration key `key`, as `config.parameters` names
/// it (§2.3 step 5): the key's console tier (`ui_tier`, a per-instance key by its member) is
/// `daily` or `advanced`, and its key-table row is settable. Settable is the row's static
/// property, not whether the Runtime's state allows the change now: the checks behind the gate
/// answer that with their own codes. A key the key table does not have has no tier: refused. A
/// change that clears the key is judged as one that sets it.
pub(crate) fn ui_may_set(key: &str) -> bool {
    matches!(ui_tier(key), UiTier::Daily | UiTier::Advanced)
        && find_row(key).is_some_and(|(row, _)| row.settable.is_settable())
}

impl UiRefusal {
    /// The refusal as the request's error: `channel_request_refused` with `channel` `ui`,
    /// `operation` the registered location of the kind ([`kind_location`]) and, for a setting,
    /// `config_key`. A kind whose location is not registered is an internal invariant breach.
    pub(crate) fn error(&self) -> RuntimeHostResult<RuntimeHostError> {
        let operation = Location::relayed(kind_location(self.kind)).ok_or_else(receipt_error)?;
        let channel = self.binding.channel;
        Ok(RuntimeHostError::request(
            HostCode::ChannelRequestRefused,
            operation,
            RuntimeErrorCode::InvalidRequest,
        )
        .with_outcome_values(|outcome| {
            let outcome = outcome.channel(channel);
            match &self.config_key {
                Some(key) => outcome.config_key(key.clone()),
                None => outcome,
            }
        }))
    }
}

impl HostShared {
    /// The refusal of `request` by the gate on a `ui` connection (§2.6): Denied with the refusal's
    /// error. A request G14 copies is first recorded `command.rejected` with the connection's
    /// channel and the refusal as its outcome, the record its terminal; a pure read records
    /// nothing. A refused append stops the Runtime, as every refused record does.
    pub(super) fn refuse_on_ui(
        &self,
        request: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        refusal: &UiRefusal,
    ) -> RequestFailure {
        let error = match refusal.error() {
            Ok(error) => error,
            Err(error) => return RequestFailure::poison_without_terminal(error),
        };
        if !request.operation().interface_class().g14_copied {
            return RequestFailure::request(error, RuntimeReceiptState::Denied, None);
        }
        let rejected = self.append_event(
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            validated.event_links(None, None, None),
            CommandPayloadDraft::rejected_on_channel(
                EventAction::RuntimeAction,
                EffectDisposition::NotPerformed,
                error.outcome(),
                refusal.binding,
                AuditInput::new(),
            ),
        );
        match rejected {
            Ok(rejected) => RequestFailure::request(
                error,
                RuntimeReceiptState::Denied,
                Some(terminal(&rejected)),
            ),
            Err(failure) => failure,
        }
    }
}

/// The registered location that names a request of `kind` in an outcome's `operation`: the
/// kind's spelling on the wire where that spelling is a registered location, otherwise the name
/// the runtime client or the contract gives that request (`runtime_input`: a location is never
/// one word, so `input` cannot be one).
pub(crate) const fn kind_location(kind: RuntimeOperationKind) -> &'static str {
    use RuntimeOperationKind as Kind;
    match kind {
        Kind::Health => "runtime_health",
        Kind::RequestShutdown => "request_runtime_shutdown",
        Kind::InstallTransition => "install_transition",
        Kind::Status => "runtime_status",
        Kind::ProjectInterface => "runtime_project_interface",
        Kind::ProjectPolicyInputIdentity => "project_policy_input_identity",
        Kind::MonitorStatus => "runtime_monitor_status",
        Kind::RuntimeFactSnapshot => "runtime_fact_snapshot",
        Kind::InstanceFactSnapshot => "runtime_instance_fact_snapshot",
        Kind::PerformanceStatus => "runtime_performance_status",
        Kind::FactRefreshStatus => "runtime_fact_refresh_status",
        Kind::ScheduleExplanation => "runtime_schedule_explanation",
        Kind::StartReport => "runtime_start_report",
        Kind::ConfigureMonitor => "runtime_monitor_configure",
        Kind::ClearMonitor => "runtime_monitor_clear",
        Kind::AcquireLease => "acquire_lease",
        Kind::QueueLease => "queue_lease",
        Kind::PollQueuedLease => "poll_queued_lease",
        Kind::CancelQueuedLease => "cancel_queued_lease",
        Kind::CancelContainedTask => "cancel_contained_task",
        Kind::RenewLease => "renew_lease",
        Kind::ReleaseLease => "release_lease",
        Kind::ObserveReadonly => "observe_readonly",
        Kind::RecognizeArtifact => "recognize_artifact",
        Kind::ObserveContainedPage => "observe_contained_page",
        Kind::RunContainedLabOperation => "run_contained_lab_operation",
        Kind::CaptureSequence => "capture_sequence",
        Kind::SafeReset => "safe_reset",
        Kind::ApplicationLifecycle => "application_lifecycle",
        Kind::ControlEmulatorInstance => "control_emulator_instance",
        Kind::DiscoverInstances => "discover_instances",
        Kind::PauseScheduling => "pause_scheduling",
        Kind::ResumeScheduling => "resume_scheduling",
        Kind::SelfCheckInstance => "self_check_instance",
        Kind::DrillRecoveryLadder => "runtime_recovery_ladder_drill",
        Kind::RunContainedTask => "run_contained_task",
        Kind::Input => "runtime_input",
        Kind::PublishFact => "publish_fact",
        Kind::PublishFacts => "publish_facts",
        Kind::QueryEvents => "query_events",
        Kind::ReadMaterial => "read_runtime_material",
        Kind::SubscribeEvents => "subscribe_events",
        Kind::RegisterDiagnosticSignature => "register_diagnostic_signature",
        Kind::MatchDiagnosticSignatures => "match_diagnostic_signatures",
        Kind::RetireDiagnosticSignature => "retire_diagnostic_signature",
        Kind::DebugPackage => "debug_package",
        Kind::ExportEvidence => "export_evidence",
        Kind::RecordAuthoringEvent => "record_resource_authoring_event",
        Kind::RecordDebugEvent => "record_runtime_debug_event",
        Kind::ReleaseLabPin => "release_lab_pin",
        Kind::RecordClientAction => "record_client_action",
        Kind::DeclareGovernanceIdentity => "declare_governance_identity",
        Kind::RecordApprovalDecision => "record_approval_decision",
        Kind::StartAgentSession => "start_agent_session",
        Kind::ResumeAgentSession => "resume_agent_session",
        Kind::AgentSessionStatus => "agent_session_status",
        Kind::RecordAgentResponse => "record_agent_response",
        Kind::PrepareStrategicReport => "prepare_strategic_report",
        Kind::ProjectPolicyForward => "project_policy_forward",
        Kind::AssessPredictiveMaintenance => "assess_predictive_maintenance",
        Kind::CompileProposal => "compile_proposal",
        Kind::PromoteProposal => "promote_proposal",
        Kind::ApplyResourceTargets => "apply_resource_targets",
        Kind::ResourceTargetView => "resource_target_view",
        Kind::ConfigureSettings => "configure_settings",
        Kind::BindChannel => "bind_channel",
        Kind::UpgradeState => "upgrade_state",
        Kind::UpgradeStateStatus => "upgrade_state_status",
        Kind::LoadPack => "load_pack",
        Kind::UnloadPack => "unload_pack",
        Kind::RunInstanceTask => "run_instance_task",
        Kind::StopInstanceRun => "stop_instance_run",
        Kind::EditPackSchedule => "edit_pack_schedule",
        Kind::EditResourceTarget => "edit_resource_target",
        Kind::RoutineSummary => "routine_summary",
        Kind::PackScheduleView => "pack_schedule_view",
        Kind::AcquirePlayerHold => "acquire_player_hold",
        Kind::ReleasePlayerHold => "release_player_hold",
        Kind::AgentWakeTest => "agent_wake_test",
    }
}
