use super::*;
use crate::codes::ContractCode;
use crate::outcome::{CodeStr, Outcome};
use crate::{
    AgentCapabilityContract, AgentResponseDisposition, AgentSessionBudget, AgentSessionEventData,
    AgentSessionResponse, AgentSessionStatus, AgentWakeData, AgentWakeKind, AgentWakeTrigger,
    ApprovalDecisionRecord, ApprovalDisposition, ApprovalTarget, ClientActionKind,
    ClientActionRecord, ClientActionValue, FactContent, FactInvalidationEventData, FactRecord,
    FactScope, FactTtlPolicy, FactTtlSource, FactValue, InputAction, MonitorDecision,
    MonitorDiagnosis, MonitorDisposition, MonitorObservation, MonitorRecoveryCoordinationReason,
    MonitorRecoveryKind, PerformanceContext, PerformanceControlEventData, PerformanceControlLevel,
    PerformanceControlReason, PerformanceDeadlineDisposition, PerformanceMetric,
    PerformanceMonitorHealth, PerformanceMonitorStateEventData, PerformancePressureEventData,
    PerformancePressureKind, PerformancePressureRecord, PerformancePressureSeverity,
    PerformancePressureValue, PerformanceStutterEventData, PerformanceSummaryEventData,
    ReleaseResourceVersion, ReleaseTransitionData, ReleaseTransitionKind, RuntimeReleaseSet,
    StateMigrationData, StateRecoveryAction, StateTransitionStatus, StateValidationResult,
};
use sha2::{Digest, Sha256};
use std::sync::Mutex;

/// A code as a client sends it (a `CodeStr` is built only by deserialising).
fn received_code(spelling: &str) -> CodeStr {
    serde_json::from_value(serde_json::Value::String(spelling.to_owned())).expect("a received code")
}

struct SpyFingerprinter {
    seen: Mutex<Vec<(SecretField, String)>>,
}

impl SpyFingerprinter {
    fn new() -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
        }
    }

    fn seen(&self) -> Vec<(SecretField, String)> {
        self.seen.lock().expect("spy lock").clone()
    }
}

impl SecretFingerprinter for SpyFingerprinter {
    fn fingerprint(
        &self,
        field: SecretField,
        original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        self.seen
            .lock()
            .expect("spy lock")
            .push((field, original.to_string()));
        Sha256Fingerprint::new(format!("sha256:{}", "a".repeat(64)), original)
    }
}

struct MaliciousEchoFingerprinter;

impl SecretFingerprinter for MaliciousEchoFingerprinter {
    fn fingerprint(
        &self,
        _field: SecretField,
        original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        let candidate = if original.starts_with("sha256:") {
            original.to_string()
        } else {
            format!("sha256:{original}")
        };
        Sha256Fingerprint::new(candidate, "different-original")
    }
}

fn identifier_issuer() -> IdentifierIssuer {
    IdentifierIssuer::new().expect("identifier issuer")
}

fn origin() -> EventOrigin {
    EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User)
}

fn links(issuer: &IdentifierIssuer) -> EventLinksDraft {
    EventLinksDraft::default()
        .with_instance_id(issuer.mint_instance_id().expect("instance id"))
        .with_request_id(issuer.mint_request_id().expect("request id"))
        .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"))
        .with_causation_id(issuer.mint_causation_id().expect("causation id"))
        .with_task_id(issuer.mint_task_id().expect("task id"))
        .with_run_id(issuer.mint_run_id().expect("run id"))
        .with_lease_id(issuer.mint_lease_id().expect("lease id"))
        .with_frame_id(issuer.mint_frame_id().expect("frame id"))
        .with_action_id(issuer.mint_action_id().expect("action id"))
        .with_recognition_id(issuer.mint_recognition_id().expect("recognition id"))
}

fn artifact_links(issuer: &IdentifierIssuer) -> ArtifactLinksDraft {
    ArtifactLinksDraft::default()
        .with_run_id(issuer.mint_run_id().expect("run id"))
        .with_frame_id(issuer.mint_frame_id().expect("frame id"))
        .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"))
}

fn artifact(bytes: &[u8]) -> StoreIssuedArtifact {
    let links_issuer = identifier_issuer();
    super::artifact::issue_pending_for_tests(
        ArtifactKind::CaptureFrame,
        artifact_links(&links_issuer),
        bytes,
        1_752_147_200_000,
    )
    .expect("store-issued artifact")
}

fn audit_all() -> AuditInput {
    AuditInput::new()
        .with_account("account-secret-c1@example.invalid")
        .with_authentication("authentication-secret-c1")
        .with_machine_path(r"C:\Users\Alice\private\runtime.json")
        .with_device_endpoint("127.0.0.1:16384")
}

fn all_payload_drafts(mut input: impl FnMut() -> AuditInput) -> Vec<EventPayloadDraft> {
    let action = EventAction::RuntimeAction;
    let diagnostic = DiagnosticCode::RuntimeDiagnostic;
    let effect = EffectDisposition::Performed;
    let ids = identifier_issuer();
    let from_holder = *ids.mint_holder_id().expect("from holder").transport();
    let to_holder = *ids.mint_holder_id().expect("to holder").transport();
    let from_lease = *ids.mint_lease_id().expect("from lease").transport();
    let to_lease = *ids.mint_lease_id().expect("to lease").transport();
    let queued_request = *ids.mint_request_id().expect("queued request").transport();
    let monitor_observation =
        MonitorObservation::new(MonitorDiagnosis::Healthy, "home", Some("home".to_string()))
            .expect("monitor observation");
    let monitor_decision =
        MonitorDecision::new(MonitorDiagnosis::Healthy, MonitorDisposition::Healthy, None)
            .expect("monitor decision");
    let policy_data = PolicyDispatchEventData {
        decision_id: "decision:fixture-a".to_owned(),
        task_id: "task:fixture-a".to_owned(),
        instance_id: "instance:fixture-a".to_owned(),
        operation_id: "operation:fixture-a".to_owned(),
        package_digest: format!("sha256:{}", "c".repeat(64)).into(),
        procedure_binding_digest: format!("sha256:{}", "d".repeat(64)),
        reason_chain_id: "reason:fixture-a".to_owned(),
        reasons: vec![PolicyReasonRecord {
            code: "eligible".to_owned(),
        }],
        catalog_hash: format!("sha256:{}", "b".repeat(64)),
        catalog_version: 1,
        input_ledger_position: 1,
        fact_snapshot_id: "snapshot:fixture-a".to_owned(),
        approval_fact_ids: vec!["approval:fixture-a".to_owned()],
        urgency_milli: 500,
    };
    let policy_admission = PolicyAdmissionRecord {
        activity: PolicyActivitySample {
            profile_id: "activity:fixture-a".to_owned(),
            local_day: 20_000,
            window_id: "activity:fixture-a:20000:0".to_owned(),
            admitted_at_unix_ms: 1_752_147_200_000,
            seed: 7,
            interval_ms: 60_000,
            next_eligible_unix_ms: 1_752_147_260_000,
        },
        budget: PolicyBudgetReceipt {
            task_daily_used: 1,
            task_daily_limit: 24,
            task_window_used: 1,
            task_window_limit: 4,
            task_runtime_reserved_ms: 60_000,
            task_runtime_limit_ms: 300_000,
            activity_daily_used: 1,
            activity_daily_limit: 24,
            activity_window_used: 1,
            activity_window_limit: 4,
            activity_runtime_reserved_ms: 60_000,
            activity_runtime_limit_ms: 7_200_000,
        },
    };
    let policy_execution = PolicyExecutionEventData {
        decision_id: "decision:fixture-a".to_owned(),
        task_id: "task:fixture-a".to_owned(),
        instance_id: "instance:fixture-a".to_owned(),
        observed_at_unix_ms: 1_752_147_201_000,
        outcome: PolicyExecutionOutcome::Succeeded { runtime_ms: 1_000 },
    };
    let policy_signal = PolicyPlanningSignalEventData {
        signal_id: "signal:fixture-a".to_owned(),
        instance_id: "instance:fixture-a".to_owned(),
        task_id: Some("task:fixture-a".to_owned()),
        kind: PolicyPlanningSignalKind::GoalMissed,
        fact_code: "goal.primary.missed".to_owned(),
        observed_at_unix_ms: 1_752_147_201_000,
        detection_budget: None,
    };
    let catalog_data = CatalogTransitionEventData {
        catalog_id: "catalog:fixture-a".to_owned(),
        catalog_version: 2,
        catalog_hash: format!("sha256:{}", "c".repeat(64)),
        previous_catalog_hash: Some(format!("sha256:{}", "b".repeat(64))),
        promotion: None,
    };
    let migration = StateMigrationData::new(
        format!("migration:{}", "d".repeat(64)),
        "policy.catalog.active",
        "pointer.v0",
        "pointer.v1",
        format!("sha256:{}", "e".repeat(64)),
        StateValidationResult::Passed,
        StateRecoveryAction::ImportedLegacy,
    )
    .expect("state migration");
    let release = RuntimeReleaseSet::new(
        "1.0.0",
        format!("sha256:{}", "d".repeat(64)),
        "1.0.0",
        format!("sha256:{}", "e".repeat(64)),
        vec![
            ReleaseResourceVersion::new(
                "project-neutral",
                "1.0.0",
                format!("sha256:{}", "f".repeat(64)),
            )
            .expect("release resource"),
        ],
    )
    .expect("release set");
    let release_transition = ReleaseTransitionData::new(
        format!("release-transition:{}", "a".repeat(64)),
        ReleaseTransitionKind::Activate,
        None,
        release.release_id(),
        1,
        release.manifest_sha256(),
        StateTransitionStatus::new(StateValidationResult::Passed, StateRecoveryAction::None),
    )
    .expect("release transition");
    let rollback_transition = ReleaseTransitionData::new(
        format!("release-transition:{}", "b".repeat(64)),
        ReleaseTransitionKind::Rollback,
        Some(format!("release:{}", "c".repeat(64))),
        release.release_id(),
        2,
        release.manifest_sha256(),
        StateTransitionStatus::new(StateValidationResult::Passed, StateRecoveryAction::None),
    )
    .expect("rollback transition");
    let agent_wake = AgentWakeData::new(
        *ids.mint_agent_wake_id().expect("agent wake id").transport(),
        *ids.mint_correlation_id()
            .expect("agent correlation id")
            .transport(),
        AgentWakeTrigger::new(
            *ids.mint_instance_id()
                .expect("agent instance id")
                .transport(),
            AgentWakeKind::DriftPredicted,
            *ids.mint_event_id().expect("agent trigger id").transport(),
            1,
            1_752_147_201_000,
        )
        .expect("agent wake trigger"),
        AgentSessionBudget::new(2, 60_000).expect("agent budget"),
        AgentCapabilityContract::read_only(2).expect("agent capabilities"),
    )
    .expect("agent wake");
    let agent_session = AgentSessionStatus::started(
        *ids.mint_agent_session_id()
            .expect("agent session id")
            .transport(),
        &agent_wake,
        1_752_147_202_000,
    )
    .expect("agent session");
    let agent_resumed = agent_session
        .resumed(1_752_147_203_000)
        .expect("agent resumed");
    let retry_response = AgentSessionResponse::new(
        agent_session.session_id(),
        AgentResponseDisposition::RetryableFailure,
        received_code("transient_failure"),
        1_752_147_204_000,
    )
    .expect("agent retry response");
    let agent_retry = agent_session
        .retry_or_escalate(retry_response.observed_at_unix_ms())
        .expect("agent retry");
    let completed_response = AgentSessionResponse::new(
        agent_session.session_id(),
        AgentResponseDisposition::Completed,
        received_code("completed"),
        1_752_147_205_000,
    )
    .expect("agent completed response");
    let agent_completed = agent_session
        .completed(completed_response.observed_at_unix_ms())
        .expect("agent completed");
    let human_response = AgentSessionResponse::new(
        agent_session.session_id(),
        AgentResponseDisposition::NeedsHuman,
        received_code("human_required"),
        1_752_147_206_000,
    )
    .expect("agent human response");
    let agent_escalated = agent_session
        .escalated(human_response.observed_at_unix_ms())
        .expect("agent escalated");
    let performance_pressure_started = PerformancePressureEventData {
        observed_at_unix_ms: 1_752_147_201_000,
        pressure: PerformancePressureRecord {
            kind: PerformancePressureKind::Cpu,
            severity: PerformancePressureSeverity::High,
            started_at_unix_ms: 1_752_147_201_000,
            last_observed_at_unix_ms: 1_752_147_201_000,
            peak: PerformancePressureValue::Utilization {
                basis_points: 9_500,
            },
        },
    };
    let performance_pressure_ended = PerformancePressureEventData {
        observed_at_unix_ms: 1_752_147_201_000,
        pressure: PerformancePressureRecord {
            started_at_unix_ms: 1_752_147_200_000,
            ..performance_pressure_started.pressure.clone()
        },
    };
    let performance_context = PerformanceContext {
        window_start_unix_ms: 1_752_147_171_000,
        window_end_unix_ms: 1_752_147_201_000,
        health: PerformanceMonitorHealth::Healthy,
        sample_count: 2,
        unavailable_metrics: Vec::new(),
        pressures: vec![performance_pressure_ended.pressure.clone()],
        max_cpu_basis_points: Some(9_500),
        max_ram_basis_points: Some(4_000),
        disk_queue_depth_p95_milli: Some(500),
        disk_latency_p95_micros: Some(1_000),
        max_gpu_basis_points: Some(3_000),
        max_frame_gap_ms: Some(1_500),
        max_capture_latency_ms: Some(120),
        max_recognition_latency_ms: Some(80),
        max_action_effect_latency_ms: Some(250),
        max_touch_response_us: Some(4_200),
        max_capture_acquire_us: Some(38_000),
        related_event_ids: Vec::new(),
    };
    let fact_record = FactRecord {
        scope: FactScope::Instance {
            instance_id: "instance:fixture-a".to_owned(),
        },
        key: "env.theme".to_owned(),
        content: FactContent::Inline {
            value: FactValue::String("Neutral".to_owned()),
        },
        observed_at_unix_ms: 1_752_147_201_000,
        expires_at_unix_ms: Some(1_752_147_261_000),
        ttl_policy: Some(FactTtlPolicy {
            minimum_ms: 1_000,
            maximum_ms: 120_000,
            source: FactTtlSource::DetectorContract,
        }),
        confidence_milli: 900,
        source_detector: "detector.theme".to_owned(),
        source_snapshot_id: "snapshot:fixture-a".to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "d".repeat(64),
        invalidate_on: vec![EventType::RuntimeTakeover],
    };
    let fact_invalidation = FactInvalidationEventData {
        scope: fact_record.scope.clone(),
        key: fact_record.key.clone(),
        source_snapshot_id: fact_record.source_snapshot_id.clone(),
        invalidated_at_unix_ms: 1_752_147_202_000,
        invalidated_by_event_id: *ids.mint_event_id().expect("trigger event").transport(),
        invalidated_by_event_type: EventType::RuntimeTakeover,
    };
    let client_action = ClientActionRecord::new(
        "settings",
        "profile_mode",
        ClientActionKind::Input,
        Some("instance-a".to_owned()),
        Some(ClientActionValue::PathSafeString("balanced".to_owned())),
    )
    .expect("client action");
    let approval = ApprovalDecisionRecord::new(
        "approval:fixture-a",
        ApprovalDisposition::Approved,
        ApprovalTarget::Catalog {
            catalog_hash: format!("sha256:{}", "d".repeat(64)),
            catalog_version: 1,
        },
        crate::ApprovalReasonCode::new("user_confirmed").expect("approval reason"),
    )
    .expect("approval");
    let capture_artifact = artifact(b"capture-summary-frame");
    let capture_projected = capture_artifact.reference().project(true);
    let capture_summary = CaptureSummaryRecord::new(
        1,
        0,
        0,
        1,
        EvidenceCompleteness::Complete,
        vec![CapturePersistedEvidence::new(0, capture_projected.clone()).expect("capture frame")],
        vec![
            CapturePinnedEvidence::new(
                Some(0),
                PinnedFrameReason::Terminal,
                Some(capture_projected),
            )
            .expect("capture pin"),
        ],
    )
    .expect("capture summary");

    vec![
        MonitorPayloadDraft::requested(input()).into(),
        MonitorPayloadDraft::started(input()).into(),
        MonitorPayloadDraft::completed(effect, monitor_observation, monitor_decision, input())
            .into(),
        MonitorPayloadDraft::failed(diagnostic, effect, input()).into(),
        MonitorPayloadDraft::recovery_admitted(MonitorRecoveryKind::WakeStandby, input()).into(),
        MonitorPayloadDraft::recovery_deferred(
            MonitorRecoveryKind::ReturnToExpectedPage,
            MonitorRecoveryCoordinationReason::ActiveLease,
            input(),
        )
        .into(),
        PerformancePayloadDraft::pressure_started(performance_pressure_started, input()).into(),
        PerformancePayloadDraft::pressure_ended(performance_pressure_ended, input()).into(),
        PerformancePayloadDraft::stutter_detected(
            PerformanceStutterEventData {
                instance_id: "instance:fixture-a".to_owned(),
                observed_at_unix_ms: 1_752_147_201_000,
                frame_gap_ms: 1_500,
                capture_latency_ms: Some(120),
                recognition_latency_ms: Some(80),
                action_effect_latency_ms: Some(250),
            },
            input(),
        )
        .into(),
        PerformancePayloadDraft::summary(
            PerformanceSummaryEventData {
                context: performance_context,
                foreground: None,
                owned_processes: Vec::new(),
                third_party_high_load: Vec::new(),
                ledger_commits: None,
                capacity: None,
            },
            input(),
        )
        .into(),
        PerformancePayloadDraft::monitor_degraded(
            PerformanceMonitorStateEventData {
                observed_at_unix_ms: 1_752_147_201_000,
                health: PerformanceMonitorHealth::Degraded,
                outcome: Some(Outcome::relayed("performance_counter_failed")),
                consecutive_failures: 1,
                terminal: false,
                unavailable_metrics: vec![PerformanceMetric::Gpu],
            },
            input(),
        )
        .into(),
        PerformancePayloadDraft::monitor_recovered(
            PerformanceMonitorStateEventData {
                observed_at_unix_ms: 1_752_147_202_000,
                health: PerformanceMonitorHealth::Healthy,
                outcome: None,
                consecutive_failures: 0,
                terminal: false,
                unavailable_metrics: Vec::new(),
            },
            input(),
        )
        .into(),
        PerformancePayloadDraft::balance_changed(
            PerformanceControlEventData {
                observed_at_unix_ms: 1_752_147_203_000,
                instance_id: None,
                previous_level: PerformanceControlLevel::Normal,
                level: PerformanceControlLevel::DispatchPaused,
                reason: PerformanceControlReason::ThirdPartyContention,
                host_responsiveness_basis_points: Some(9_000),
                third_party_pressure_basis_points: Some(3_000),
                recovery: false,
                deadline_disposition: None,
                arbitration: None,
            },
            input(),
        )
        .into(),
        FactPayloadDraft::published(fact_record, input()).into(),
        FactPayloadDraft::invalidated(fact_invalidation, input()).into(),
        ApprovalPayloadDraft::decision(approval, input()).into(),
        CommandPayloadDraft::received(action, input()).into(),
        CommandPayloadDraft::validated(action, effect, input()).into(),
        CommandPayloadDraft::rejected(action, diagnostic, effect, input()).into(),
        SchedulerPayloadDraft::admitted(action, input()).into(),
        SchedulerPayloadDraft::queued(action, crate::LeasePriority::High, 1, 100, true, input())
            .into(),
        SchedulerPayloadDraft::denied(action, diagnostic, input()).into(),
        SchedulerPayloadDraft::preempted(
            action,
            from_holder,
            from_lease,
            queued_request,
            crate::LeasePriority::High,
            true,
            input(),
        )
        .into(),
        PolicyPayloadDraft::dispatch_intent(policy_data.clone(), input()).into(),
        PolicyPayloadDraft::dispatch_admitted(
            policy_data.clone(),
            policy_admission.clone(),
            input(),
        )
        .into(),
        PolicyPayloadDraft::dispatch_rejected_with_reason(
            policy_data.clone(),
            EffectDisposition::NotPerformed,
            PolicyDispatchRejection {
                outcome: Outcome::relayed("policy_budget_exhausted"),
                budget: Some(PolicyBudgetDenial {
                    dimension: PolicyBudgetDimension::TaskDaily,
                    used: 1,
                    requested: 1,
                    limit: 1,
                }),
                next_eligible_unix_ms: Some(1_752_192_000_000),
            },
            input(),
        )
        .into(),
        PolicyPayloadDraft::execution_recorded(policy_execution, input()).into(),
        PolicyPayloadDraft::planning_signal_observed(policy_signal, input()).into(),
        PolicyPayloadDraft::dispatch_completed(policy_data, policy_admission, input()).into(),
        CatalogPayloadDraft::transition_intent(
            EventAction::CatalogActivate,
            catalog_data.clone(),
            input(),
        )
        .into(),
        CatalogPayloadDraft::activated(catalog_data.clone(), input()).into(),
        CatalogPayloadDraft::transition_failed(
            EventAction::CatalogActivate,
            catalog_data.clone(),
            EffectDisposition::Indeterminate,
            input(),
        )
        .into(),
        CatalogPayloadDraft::rolled_back(catalog_data, input()).into(),
        StatePayloadDraft::migrated(migration, input()).into(),
        ReleasePayloadDraft::staged(release, input()).into(),
        ReleasePayloadDraft::transition_intent(release_transition.clone(), input()).into(),
        ReleasePayloadDraft::activated(release_transition.clone(), input()).into(),
        ReleasePayloadDraft::rolled_back(rollback_transition, input()).into(),
        ReleasePayloadDraft::transition_failed(
            release_transition,
            EffectDisposition::Indeterminate,
            input(),
        )
        .into(),
        AgentPayloadDraft::wake_requested(agent_wake, input()).into(),
        AgentPayloadDraft::session_started(
            AgentSessionEventData::new(agent_session, None).expect("agent start event"),
            input(),
        )
        .into(),
        AgentPayloadDraft::session_resumed(
            AgentSessionEventData::new(agent_resumed, None).expect("agent resume event"),
            input(),
        )
        .into(),
        AgentPayloadDraft::response_recorded(
            AgentSessionEventData::new(agent_retry, Some(retry_response))
                .expect("agent retry event"),
            input(),
        )
        .into(),
        AgentPayloadDraft::session_completed(
            AgentSessionEventData::new(agent_completed, Some(completed_response))
                .expect("agent completed event"),
            input(),
        )
        .into(),
        AgentPayloadDraft::session_escalated(
            AgentSessionEventData::new(agent_escalated, Some(human_response))
                .expect("agent escalation event"),
            input(),
        )
        .into(),
        LeasePayloadDraft::requested(action, input()).into(),
        LeasePayloadDraft::granted(action, effect, input()).into(),
        LeasePayloadDraft::transferred(
            action,
            effect,
            from_holder,
            from_lease,
            to_holder,
            to_lease,
            queued_request,
            crate::LeasePriority::High,
            input(),
        )
        .into(),
        LeasePayloadDraft::released(action, effect, input()).into(),
        LeasePayloadDraft::expired(action, effect, input()).into(),
        LeasePayloadDraft::transition_intent(action, input()).into(),
        LeasePayloadDraft::transition_failed(action, diagnostic, effect, input()).into(),
        TaskPayloadDraft::requested(action, input()).into(),
        TaskPayloadDraft::started(action, input()).into(),
        TaskPayloadDraft::step_started(action, input()).into(),
        TaskPayloadDraft::step_finished(action, input()).into(),
        TaskPayloadDraft::completed(action, effect, input()).into(),
        TaskPayloadDraft::failed(action, diagnostic, effect, input()).into(),
        TaskPayloadDraft::cancelled(action, effect, input()).into(),
        TaskPayloadDraft::terminal_intent(action, input()).into(),
        TaskPayloadDraft::terminal_commit_failed(action, diagnostic, effect, input()).into(),
        TaskPayloadDraft::semantic(TaskSemanticFact::RunStarted, input()).into(),
        InputPayloadDraft::intent(action, input()).into(),
        InputPayloadDraft::committed(action, effect, input()).into(),
        InputPayloadDraft::completed(action, input()).into(),
        InputPayloadDraft::failed(action, diagnostic, effect, input()).into(),
        CapturePayloadDraft::requested(EventAction::CaptureObserve, input()).into(),
        CapturePayloadDraft::completed(EventAction::CaptureObserve, effect, 1280, 720, input())
            .into(),
        CapturePayloadDraft::failed(
            EventAction::CaptureObserve,
            DiagnosticCode::CaptureFailed,
            effect,
            input(),
        )
        .into(),
        CapturePayloadDraft::pressure_changed(
            CapturePressureState::Tier2Flush,
            1_000,
            750,
            input(),
        )
        .into(),
        CapturePayloadDraft::dedup_window(3, 900, input()).into(),
        CapturePayloadDraft::policy_changed(
            300,
            RetentionClass::DebugFull,
            CapturePolicyReason::Default,
            input(),
        )
        .into(),
        CapturePayloadDraft::summary_committed(capture_summary, input()).into(),
        RecognitionPayloadDraft::requested(EventAction::RecognitionObserve, input()).into(),
        RecognitionPayloadDraft::completed(
            EventAction::RecognitionObserve,
            effect,
            1280,
            720,
            RecognitionVerdict::FrameDecoded,
            input(),
        )
        .into(),
        RecognitionPayloadDraft::failed(
            EventAction::RecognitionObserve,
            DiagnosticCode::RecognitionFailed,
            effect,
            input(),
        )
        .into(),
        ArtifactPayloadDraft::created(input()).into(),
        ArtifactPayloadDraft::verified(input()).into(),
        ArtifactPayloadDraft::store_failed(DiagnosticCode::ArtifactWriteFailed, input()).into(),
        ArtifactPayloadDraft::verification_failed(DiagnosticCode::ArtifactVerifyFailed, input())
            .into(),
        ArtifactPayloadDraft::export_completed(
            TaskOutcome::Success,
            EvidenceCompleteness::Complete,
            2,
            input(),
        )
        .into(),
        ArtifactPayloadDraft::export_failed(
            DiagnosticCode::ArtifactExportFailed,
            TaskOutcome::Failure,
            EvidenceCompleteness::Failed,
            1,
            input(),
        )
        .into(),
        ResourceAuthoringPayloadDraft::event(
            ResourceAuthoringPhase::AuthoringStarted,
            "draft-a",
            "resource-root",
            "b".repeat(64),
            vec!["operations/task-a/task.json".to_string()],
            None,
            input(),
        )
        .into(),
        ResourceAuthoringPayloadDraft::event(
            ResourceAuthoringPhase::DraftBuilt,
            "draft-a",
            "resource-root",
            "b".repeat(64),
            vec!["operations/task-a/task.json".to_string()],
            None,
            input(),
        )
        .into(),
        ResourceAuthoringPayloadDraft::event(
            ResourceAuthoringPhase::ValidationCompleted,
            "draft-a",
            "resource-root",
            "b".repeat(64),
            vec!["operations/task-a/task.json".to_string()],
            None,
            input(),
        )
        .into(),
        ResourceAuthoringPayloadDraft::event(
            ResourceAuthoringPhase::PromoteIntent,
            "draft-a",
            "resource-root",
            "b".repeat(64),
            vec!["operations/task-a/task.json".to_string()],
            None,
            input(),
        )
        .into(),
        ResourceAuthoringPayloadDraft::event(
            ResourceAuthoringPhase::Promoted,
            "draft-a",
            "resource-root",
            "b".repeat(64),
            vec!["operations/task-a/task.json".to_string()],
            None,
            input(),
        )
        .into(),
        ResourceAuthoringPayloadDraft::event(
            ResourceAuthoringPhase::PromoteFailed,
            "draft-a",
            "resource-root",
            "b".repeat(64),
            vec!["operations/task-a/task.json".to_string()],
            Some(Outcome::relayed("authoring_validation_failed")),
            input(),
        )
        .into(),
        ClientPayloadDraft::action(client_action, input()).into(),
        ClientPayloadDraft::ui_action(action, input()).into(),
        ClientPayloadDraft::cli_command(action, input()).into(),
        ClientPayloadDraft::lab_request(action, input()).into(),
        LedgerPayloadDraft::recovered(RecoveryReason::StaleOwner, Some(1), 64, input()).into(),
    ]
}

#[test]
fn monitor_outcome_is_typed_and_public_projection_preserves_the_decision() {
    let observation = MonitorObservation::new(
        MonitorDiagnosis::UnexpectedPage,
        "home",
        Some("campaign".to_string()),
    )
    .expect("observation");
    let decision = MonitorDecision::new(
        MonitorDiagnosis::UnexpectedPage,
        MonitorDisposition::RecoveryRequested,
        Some(MonitorRecoveryKind::ReturnToExpectedPage),
    )
    .expect("decision");
    let sanitized = sanitize(
        MonitorPayloadDraft::completed(
            EffectDisposition::Performed,
            observation,
            decision,
            AuditInput::new(),
        )
        .into(),
        9_000,
    );

    assert_eq!(sanitized.event_type(), EventType::MonitorProbeCompleted);
    assert_eq!(sanitized.payload().family(), EventFamily::Monitor);
    let projected =
        serde_json::to_value(sanitized.payload().public_projection()).expect("monitor projection");
    assert_eq!(projected["family"], "monitor");
    assert_eq!(projected["payload"]["monitor_diagnosis"], "unexpected_page");
    assert_eq!(
        projected["payload"]["monitor_disposition"],
        "recovery_requested"
    );
    assert_eq!(
        projected["payload"]["monitor_recovery"],
        "return_to_expected_page"
    );
}

#[test]
fn monitor_recovery_coordination_is_typed_and_never_claims_an_effect() {
    let sanitized = sanitize(
        MonitorPayloadDraft::recovery_deferred(
            MonitorRecoveryKind::WakeStandby,
            MonitorRecoveryCoordinationReason::TakeoverCooldown,
            AuditInput::new(),
        )
        .into(),
        9_100,
    );

    assert_eq!(sanitized.event_type(), EventType::MonitorRecoveryDeferred);
    assert_eq!(
        sanitized.payload().effect_disposition(),
        Some(EffectDisposition::NotPerformed)
    );
    let projected = serde_json::to_value(sanitized.payload().public_projection())
        .expect("monitor recovery projection");
    assert_eq!(projected["payload"]["monitor_recovery"], "wake_standby");
    assert_eq!(
        projected["payload"]["monitor_recovery_coordination_reason"],
        "takeover_cooldown"
    );
}

fn sanitize_with(
    payload: EventPayloadDraft,
    index: u64,
    fingerprinter: &dyn SecretFingerprinter,
) -> SanitizedEventDraft {
    let issuer = identifier_issuer();
    EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        1_752_147_200_000 + index,
        EventSeverity::Info,
        origin(),
        links(&issuer),
        payload,
    )
    .sanitize(fingerprinter)
    .expect("sanitize event")
}

fn sanitize(payload: EventPayloadDraft, index: u64) -> SanitizedEventDraft {
    sanitize_with(payload, index, &SpyFingerprinter::new())
}

fn sanitize_error(payload: EventPayloadDraft) -> SanitizationError {
    let issuer = identifier_issuer();
    match EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        1_752_147_200_000,
        EventSeverity::Info,
        origin(),
        links(&issuer),
        payload,
    )
    .sanitize(&SpyFingerprinter::new())
    {
        Ok(_) => panic!("event must be rejected"),
        Err(error) => error,
    }
}

#[test]
fn input_intent_execution_plan_is_bounded_and_backward_compatible() {
    let exact_events = vec![
        InputExecutionPlanEvent::Down { x: 0, y: 200 },
        InputExecutionPlanEvent::Move {
            x: 75,
            y: 200,
            delay_before_ms: 100,
        },
        InputExecutionPlanEvent::Move {
            x: 100,
            y: 200,
            delay_before_ms: 100,
        },
        InputExecutionPlanEvent::Hold { duration_ms: 150 },
        InputExecutionPlanEvent::Move {
            x: 100,
            y: 125,
            delay_before_ms: 100,
        },
        InputExecutionPlanEvent::Move {
            x: 100,
            y: 100,
            delay_before_ms: 100,
        },
        InputExecutionPlanEvent::Up,
    ];
    let plan = InputExecutionPlanRecord::new(exact_events.clone()).expect("exact Maa 2/0 plan");
    let event = sanitize(
        InputPayloadDraft::intent_with_execution_plan(
            EventAction::InputSwipe,
            plan,
            AuditInput::new(),
        )
        .into(),
        90,
    );

    assert_eq!(event.schema_version(), GLOBAL_EVENT_SCHEMA_VERSION);
    assert_eq!(event.payload().sensitivity(), Sensitivity::Internal);
    let EventPayload::Input(InputPayload::Intent(intent)) = event.payload() else {
        panic!("typed input intent")
    };
    let recorded = intent.execution_plan().expect("execution plan");
    assert_eq!(recorded.version(), INPUT_EXECUTION_PLAN_VERSION);
    assert_eq!(recorded.profile(), INPUT_EXECUTION_PLAN_PROFILE_MAA_2_0);
    assert_eq!(recorded.declared_sensitivity(), Sensitivity::Internal);
    assert_eq!(recorded.events(), exact_events);
    assert!(matches!(
        recorded.events()[1],
        InputExecutionPlanEvent::Move { x: 75, .. }
    ));

    let encoded = serde_json::to_string(event.payload()).expect("serialize plan payload");
    let decoded: EventPayload = serde_json::from_str(&encoded).expect("deserialize plan payload");
    decoded.validate().expect("validate plan payload");
    let valid_value = serde_json::to_value(event.payload()).expect("plan payload value");
    let mut invalid_profile = valid_value.clone();
    invalid_profile["payload"]["data"]["execution_plan"]["profile"] =
        serde_json::json!("t_squared");
    serde_json::from_value::<EventPayload>(invalid_profile)
        .expect("closed typed payload")
        .validate()
        .expect_err("unknown profile rejected");
    let mut invalid_sensitivity = valid_value;
    invalid_sensitivity["payload"]["data"]["execution_plan"]["declared_sensitivity"] =
        serde_json::json!("public");
    serde_json::from_value::<EventPayload>(invalid_sensitivity)
        .expect("closed typed payload")
        .validate()
        .expect_err("non-internal plan rejected");

    let old_payload = sanitize(
        InputPayloadDraft::intent(EventAction::InputSwipe, AuditInput::new()).into(),
        91,
    );
    let old_json =
        serde_json::to_string(old_payload.payload()).expect("serialize old input intent");
    assert!(!old_json.contains("execution_plan"));
    let old_round_trip: EventPayload =
        serde_json::from_str(&old_json).expect("deserialize old input intent");
    let EventPayload::Input(InputPayload::Intent(old_intent)) = old_round_trip else {
        panic!("old typed input intent")
    };
    assert!(old_intent.execution_plan().is_none());

    let mut maximum = Vec::with_capacity(MAX_INPUT_EXECUTION_PLAN_EVENTS);
    maximum.push(InputExecutionPlanEvent::Down { x: 0, y: 200 });
    for index in 0..60 {
        maximum.push(InputExecutionPlanEvent::Move {
            x: 100,
            y: 200,
            delay_before_ms: if index == 59 { 23 } else { 3 },
        });
    }
    maximum.push(InputExecutionPlanEvent::Hold { duration_ms: 150 });
    for index in 0..60 {
        maximum.push(InputExecutionPlanEvent::Move {
            x: 100,
            y: 100,
            delay_before_ms: if index == 59 { 23 } else { 3 },
        });
    }
    maximum.push(InputExecutionPlanEvent::Up);
    assert_eq!(maximum.len(), MAX_INPUT_EXECUTION_PLAN_EVENTS);
    InputExecutionPlanRecord::new(maximum.clone()).expect("maximum bounded plan");
    maximum.insert(
        maximum.len() - 1,
        InputExecutionPlanEvent::Move {
            x: 100,
            y: 100,
            delay_before_ms: 1,
        },
    );
    assert!(InputExecutionPlanRecord::new(maximum).is_err());

    assert!(
        serde_json::from_value::<InputExecutionPlanEvent>(serde_json::json!({
            "kind": "unknown"
        }))
        .is_err()
    );
    let invalid_order = vec![
        InputExecutionPlanEvent::Down { x: 0, y: 200 },
        InputExecutionPlanEvent::Hold { duration_ms: 150 },
        InputExecutionPlanEvent::Move {
            x: 100,
            y: 200,
            delay_before_ms: 200,
        },
        InputExecutionPlanEvent::Move {
            x: 100,
            y: 100,
            delay_before_ms: 200,
        },
        InputExecutionPlanEvent::Up,
    ];
    assert!(InputExecutionPlanRecord::new(invalid_order).is_err());
}

#[test]
fn diagnostic_detail_is_bounded_sensitive_and_backward_compatible() {
    // Ledger revision 3 (Workflow #378 A3): the detail keeps only where the failure arose; the
    // failure is the payload's outcome, and native text rides on its foreign link with the
    // text's declared sensitivity.
    let failed = |detail| {
        InputPayloadDraft::failed_with_detail(
            EventAction::InputTap,
            DiagnosticCode::InputFailed,
            EffectDisposition::NotPerformed,
            Outcome::new(DiagnosticCode::InputFailed.code())
                .caused_by(LifecycleNativeDetail::new("bounded native failure", false).outcome()),
            detail,
            AuditInput::new(),
        )
        .into()
    };
    let detail = DiagnosticDetailDraft::new(
        "native",
        "device_registry.input.operation",
        "adb_shell_input",
        "tap",
    );
    let sanitized = sanitize(failed(detail), 92);

    assert_eq!(sanitized.payload().sensitivity(), Sensitivity::Sensitive);
    let EventPayload::Input(InputPayload::Failed(outcome)) = sanitized.payload() else {
        panic!("typed input failure")
    };
    let detail = outcome.detail().expect("diagnostic detail");
    assert_eq!(detail.category(), "native");
    assert_eq!(detail.stage(), "device_registry.input.operation");
    assert_eq!(detail.backend(), "adb_shell_input");
    assert_eq!(detail.operation(), "tap");
    assert_eq!(
        outcome.outcome().code(),
        &DiagnosticCode::InputFailed.code()
    );
    let [native] = outcome.outcome().causes() else {
        panic!("one native link")
    };
    assert_eq!(
        native.outcome().code(),
        &ContractCode::ForeignOtherError.code()
    );
    assert_eq!(
        native.outcome().value("raw_text"),
        Some(&serde_json::json!("bounded native failure"))
    );

    let stored = serde_json::to_value(sanitized.payload()).expect("detail payload JSON");
    assert_eq!(
        stored["payload"]["data"]["detail"],
        serde_json::json!({
            "category": "native",
            "stage": "device_registry.input.operation",
            "backend": "adb_shell_input",
            "operation": "tap",
        })
    );
    assert!(stored["payload"]["data"]["detail"].get("message").is_none());
    let public = serde_json::to_string(&sanitized.payload().public_projection())
        .expect("public detail projection");
    assert!(!public.contains("bounded native failure"));

    let old = sanitize(
        InputPayloadDraft::failed(
            EventAction::InputTap,
            DiagnosticCode::InputFailed,
            EffectDisposition::NotPerformed,
            AuditInput::new(),
        )
        .into(),
        93,
    );
    let old_json = serde_json::to_value(old.payload()).expect("old failure JSON");
    assert!(old_json["payload"]["data"].get("detail").is_none());
    let old_round_trip: EventPayload =
        serde_json::from_value(old_json).expect("failure without detail remains readable");
    let EventPayload::Input(InputPayload::Failed(old_outcome)) = old_round_trip else {
        panic!("typed input failure without detail")
    };
    assert!(old_outcome.detail().is_none());

    let exact_token = "a".repeat(256);
    let exact_stage = format!("{}.b", "a".repeat(254));
    sanitize(
        failed(DiagnosticDetailDraft::new(
            exact_token.clone(),
            exact_stage,
            exact_token.clone(),
            exact_token,
        )),
        94,
    );

    let assert_rejected = |detail, expected_code| {
        assert_eq!(
            sanitize_error(failed(detail)).code().as_str(),
            expected_code
        );
    };
    for detail in [
        DiagnosticDetailDraft::new("", "device.input", "adb", "tap"),
        DiagnosticDetailDraft::new("Native", "device.input", "adb", "tap"),
        DiagnosticDetailDraft::new("native", "device.input", "a".repeat(257), "tap"),
    ] {
        assert_rejected(detail, "invalid_diagnostic_detail_token");
    }
    for stage in ["device..input", "Device.input", "a.b.c.d.e"] {
        assert_rejected(
            DiagnosticDetailDraft::new("native", stage, "adb", "tap"),
            "invalid_diagnostic_detail_stage",
        );
    }

    let mut invalid_stored = stored;
    invalid_stored["payload"]["data"]["detail"]["stage"] = serde_json::json!("Device.input");
    let invalid_stored: EventPayload =
        serde_json::from_value(invalid_stored).expect("typed stored detail");
    assert_eq!(
        invalid_stored
            .validate()
            .expect_err("stored detail validation")
            .code()
            .as_str(),
        "invalid_diagnostic_detail_stage"
    );
}

#[test]
fn producer_cannot_select_redaction_policy() {
    let sanitized = sanitize(
        CommandPayloadDraft::received(EventAction::RuntimeStart, audit_all()).into(),
        1,
    );
    let json = serde_json::to_string(&sanitized).expect("serialize sanitized event");

    assert!(!json.contains("redaction_policy"));
    assert!(!json.contains("\"policy\""));
}

#[test]
fn machine_path_audit_retains_basename_and_full_path_sha256_without_parent_path() {
    let cases = [
        (
            r#"C:\Users\Alice\private\報告 "final" #1.json"#,
            r#"報告 "final" #1.json"#,
            "Users",
        ),
        (
            "/srv/private/reports/runtime state.json",
            "runtime state.json",
            "reports",
        ),
    ];

    for (index, (machine_path, basename, parent_fragment)) in cases.into_iter().enumerate() {
        let sanitized = sanitize(
            CommandPayloadDraft::received(
                EventAction::RuntimeStart,
                AuditInput::new().with_machine_path(machine_path),
            )
            .into(),
            index as u64 + 1,
        );
        let payload = serde_json::to_value(sanitized.payload()).expect("payload value");
        let stored = payload["payload"]["data"]["audit"]["machine_path"]
            .as_str()
            .expect("stored machine path audit");
        let expected = format!(
            "basename:{}|sha256:{:x}",
            serde_json::to_string(basename).expect("canonical basename JSON"),
            Sha256::digest(machine_path.as_bytes())
        );

        assert_eq!(stored, expected);
        assert!(!stored.contains(machine_path));
        assert!(!stored.contains(parent_fragment));
        assert!(!stored.contains("[redacted]"));
        assert!(!format!("{:?}", sanitized.payload()).contains(machine_path));
    }

    for invalid in [
        String::new(),
        "C:\\private\\".to_string(),
        "/private/".to_string(),
        ".".to_string(),
        "..".to_string(),
        "/private/.".to_string(),
        "C:\\private\\..".to_string(),
        format!("/private/{}", "x".repeat(256)),
        "/private/control\u{0001}".to_string(),
    ] {
        let error = sanitize_error(
            CommandPayloadDraft::received(
                EventAction::RuntimeStart,
                AuditInput::new().with_machine_path(invalid),
            )
            .into(),
        );
        assert_eq!(error.code().as_str(), "invalid_machine_path");
    }

    let sanitized = sanitize(
        CommandPayloadDraft::received(
            EventAction::RuntimeStart,
            AuditInput::new().with_machine_path("parent/legacy.json"),
        )
        .into(),
        3,
    );
    let mut legacy = serde_json::to_value(sanitized.payload()).expect("legacy payload value");
    legacy["payload"]["data"]["audit"]["machine_path"] = serde_json::json!("[redacted]");
    let legacy = serde_json::from_value::<EventPayload>(legacy).expect("decode legacy audit");
    legacy.validate().expect("validate legacy redacted audit");

    let mut noncanonical =
        serde_json::to_value(sanitized.payload()).expect("noncanonical payload value");
    noncanonical["payload"]["data"]["audit"]["machine_path"] =
        serde_json::json!("basename:\"legacy.json\"|sha256:ABCDEF");
    let noncanonical =
        serde_json::from_value::<EventPayload>(noncanonical).expect("decode typed audit");
    assert_eq!(
        noncanonical
            .validate()
            .expect_err("reject noncanonical stored audit")
            .code()
            .as_str(),
        "invalid_sanitized_payload"
    );
}

#[test]
fn all_runtime_secret_classes_follow_schema_owned_policy_independently() {
    fn collect_machine_paths<'a>(value: &'a serde_json::Value, found: &mut Vec<&'a str>) {
        match value {
            serde_json::Value::Object(fields) => {
                for (key, value) in fields {
                    if key == "machine_path"
                        && let Some(value) = value.as_str()
                    {
                        found.push(value);
                    }
                    collect_machine_paths(value, found);
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    collect_machine_paths(value, found);
                }
            }
            _ => {}
        }
    }

    type AuditBuilder = fn(&str) -> AuditInput;
    const MACHINE_PATH: &str = r"C:\Users\Alice\private\runtime.json";
    let machine_basename = "runtime.json";
    let machine_audit = format!(
        "basename:{}|sha256:{:x}",
        serde_json::to_string(machine_basename).expect("canonical basename JSON"),
        Sha256::digest(MACHINE_PATH.as_bytes())
    );
    let cases: [(&str, AuditBuilder); 4] = [
        ("account-secret-c1@example.invalid", |value| {
            AuditInput::new().with_account(value)
        }),
        ("authentication-secret-c1", |value| {
            AuditInput::new().with_authentication(value)
        }),
        (MACHINE_PATH, |value| {
            AuditInput::new().with_machine_path(value)
        }),
        ("127.0.0.1:16384", |value| {
            AuditInput::new().with_device_endpoint(value)
        }),
    ];

    for (secret, input) in cases {
        for (index, payload) in all_payload_drafts(|| input(secret)).into_iter().enumerate() {
            let spy = SpyFingerprinter::new();
            let sanitized = sanitize_with(payload, index as u64 + 1, &spy);
            let json = serde_json::to_string(&sanitized).expect("serialize sanitized event");
            let debug = format!("{sanitized:?}");

            assert!(!json.contains(secret), "secret leaked: {secret}");
            assert!(!debug.contains(secret), "debug leaked: {secret}");
            if secret == "account-secret-c1@example.invalid" {
                assert_eq!(
                    spy.seen(),
                    vec![(SecretField::AccountIdentity, secret.to_string())]
                );
                assert!(json.contains(&format!("sha256:{}", "a".repeat(64))));
            } else {
                assert!(spy.seen().is_empty());
            }
            if secret == MACHINE_PATH {
                let full =
                    serde_json::to_value(sanitized.payload()).expect("full sanitized payload");
                let mut stored_machine_paths = Vec::new();
                collect_machine_paths(&full, &mut stored_machine_paths);
                assert_eq!(stored_machine_paths, [machine_audit.as_str()]);
                let public = serde_json::to_string(&sanitized.payload().public_projection())
                    .expect("public payload");
                assert!(!public.contains(machine_basename));
                assert!(!public.contains(&machine_audit));
            }
        }
    }
}

#[test]
fn hash_shaped_original_cannot_survive_as_fingerprint() {
    for original in ["a".repeat(64), format!("sha256:{}", "a".repeat(64))] {
        let issuer = identifier_issuer();
        let draft = EventDraft::new(
            issuer.mint_event_id().expect("event id"),
            1_752_147_200_000,
            EventSeverity::Info,
            origin(),
            EventLinksDraft::default(),
            CommandPayloadDraft::received(
                EventAction::RuntimeStart,
                AuditInput::new().with_account(original.clone()),
            )
            .into(),
        );

        let error = draft
            .sanitize(&MaliciousEchoFingerprinter)
            .expect_err("hash echo must fail");
        assert_eq!(error.code().as_str(), "invalid_fingerprint");
        assert!(!error.to_string().contains(&original));
    }
}

#[test]
fn runtime_values_cannot_enter_origin_action_or_diagnostic_codes() {
    for runtime in [
        "token-secret-7d141b7b",
        "account-secret-valid-code",
        Box::leak(String::from("leaked-runtime-token").into_boxed_str()),
    ] {
        let json = serde_json::to_string(runtime).expect("runtime string");
        assert!(serde_json::from_str::<OriginModule>(&json).is_err());
        assert!(serde_json::from_str::<EventAction>(&json).is_err());
        assert!(serde_json::from_str::<DiagnosticCode>(&json).is_err());
        assert!(serde_json::from_str::<RecoveryReason>(&json).is_err());
    }

    assert_eq!(EventAction::RuntimeStart.to_string(), "runtime.start");
    assert_eq!(format!("{:?}", EventAction::RuntimeStart), "RuntimeStart");
    assert_eq!(OriginModule::Actingctl.to_string(), "actingctl");
    assert_eq!(
        DiagnosticCode::RuntimeDiagnostic.to_string(),
        "runtime_diagnostic"
    );
}

#[test]
fn canonical_transport_ids_cannot_be_promoted_to_producer_capabilities() {
    let canonical = format!("evt_{}", "ab".repeat(16));
    let transport: EventId =
        serde_json::from_str(&format!("\"{canonical}\"")).expect("transport event id");
    assert!(!format!("{transport:?}").contains(&canonical));

    let issuer = identifier_issuer();
    let issued = issuer.mint_event_id().expect("issued event id");
    let serialized = serde_json::to_string(issued.transport()).expect("serialize issued id");
    assert_ne!(serialized, format!("\"{canonical}\""));
    assert!(!format!("{issued:?}").contains(&canonical));
}

#[test]
fn every_action_and_diagnostic_slot_rejects_runtime_code_mutations() {
    for (index, payload) in all_payload_drafts(AuditInput::new).into_iter().enumerate() {
        let sanitized = sanitize(payload, index as u64 + 1);
        let value = serde_json::to_value(sanitized.payload()).expect("payload value");
        if value["payload"]["data"].get("action").is_some() {
            let mut action_mutation = value.clone();
            action_mutation["payload"]["data"]["action"] =
                serde_json::json!("token-secret-valid-code");
            assert!(serde_json::from_value::<EventPayload>(action_mutation).is_err());
        }
        if value["payload"]["data"].get("diagnostic_code").is_some() {
            let mut diagnostic_mutation = value;
            diagnostic_mutation["payload"]["data"]["diagnostic_code"] =
                serde_json::json!("account-secret-valid-code");
            assert!(serde_json::from_value::<EventPayload>(diagnostic_mutation).is_err());
        }
    }
}

#[test]
fn artifact_store_owns_complete_v3_metadata_and_event_sensitivity() {
    let issued = artifact(b"capture bytes");
    let reference = issued.reference();

    assert_eq!(reference.kind(), ArtifactKind::CaptureFrame);
    assert!(reference.run_id().is_some());
    assert!(reference.frame_id().is_some());
    assert!(reference.correlation_id().is_some());
    assert!(reference.object_key().starts_with("artifacts/"));
    assert_eq!(reference.media_type(), ArtifactMediaType::ImagePng);
    assert_eq!(reference.byte_count(), 13);
    assert_eq!(reference.created_at_unix_ms(), 1_752_147_200_000);
    assert_eq!(reference.producer(), ArtifactProducer::CaptureStore);
    assert_eq!(reference.retention_class(), RetentionClass::Adaptive);
    assert_eq!(reference.redaction_state(), ArtifactRedactionState::Pending);
    assert_eq!(reference.sensitivity(), Sensitivity::Secret);

    let payload = CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new());
    let issuer = identifier_issuer();
    let sanitized = EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        1_752_147_200_000,
        EventSeverity::Info,
        origin(),
        EventLinksDraft::default(),
        payload.into(),
    )
    .with_artifacts(vec![issued])
    .sanitize(&SpyFingerprinter::new())
    .expect("sanitize artifact event");

    assert_eq!(sanitized.sensitivity(), Sensitivity::Secret);
}

#[test]
fn artifact_secret_classes_cannot_survive_any_metadata_or_diagnostic_surface() {
    for secret in [
        "token-secret-artifact",
        "account-secret-artifact@example.invalid",
        r"C:\Users\Alice\private\artifact.png",
        "127.0.0.1:16384",
        &format!("sha256:{}", "d".repeat(64)),
    ] {
        let issued = artifact(secret.as_bytes());
        let json = serde_json::to_string(issued.reference()).expect("artifact JSON");
        let debug = format!("{:?}", issued.reference());
        assert!(!json.contains(secret), "artifact metadata leaked {secret}");
        assert!(!debug.contains(secret), "artifact debug leaked {secret}");
        // ARTIFACT-PERSIST-v2: native text belongs only to the sensitive failure fact.
        let failure = ArtifactFailureDraft {
            artifact_id: *issued.reference().artifact_id(),
            stage: ArtifactFailureStage::BeforePublication,
            primary: ArtifactFailureCause {
                step: Outcome::new(crate::codes::SharedCode::ArtifactWriteFailed)
                    .operation(crate::codes::ContractLocation::ExportEvidence),
                native_detail: LifecycleNativeDetail::new(secret, false),
                raw_os_error: None,
            },
            secondary: Vec::new(),
            omitted_secondary_count: 0,
            capacity: None,
        };
        let payload = sanitize(
            ArtifactPayloadDraft::persistence_failed(failure.clone(), AuditInput::new()).into(),
            1,
        );
        assert_eq!(payload.payload().sensitivity(), Sensitivity::Sensitive);
        let json = serde_json::to_value(payload.payload()).expect("private failure");
        let roundtrip: EventPayload =
            serde_json::from_value(json.clone()).expect("failure round trip");
        roundtrip.validate().expect("valid typed failure");
        assert_eq!(
            roundtrip.artifact_failure(),
            Some(&ArtifactFailureRecord {
                artifact_id: failure.artifact_id,
                stage: failure.stage,
                omitted_secondary_count: 0,
                capacity: None,
            })
        );
        assert_eq!(roundtrip.outcome(), Some(&failure.outcome()));
        assert!(!format!("{roundtrip:?}").contains(secret));
        assert!(
            !serde_json::to_string(&roundtrip.public_projection())
                .expect("public failure")
                .contains(secret)
        );

        let mut wrong_owner = json;
        wrong_owner["family"] = serde_json::json!("recognition");
        wrong_owner["payload"]["kind"] = serde_json::json!("failed");
        let wrong_owner: EventPayload =
            serde_json::from_value(wrong_owner).expect("diagnostic wire shape");
        assert_eq!(
            wrong_owner
                .validate()
                .expect_err("failure owner")
                .code()
                .as_str(),
            "invalid_artifact_failure_owner"
        );
    }
}

#[test]
fn artifact_wire_shape_has_no_store_authorization() {
    let issued = artifact(b"capture bytes");
    let value = serde_json::to_value(issued.reference()).expect("artifact value");
    let mut keys = value
        .as_object()
        .expect("artifact object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();

    assert!(
        value.get("store_authorization").is_none(),
        "artifact wire shape must not claim provenance with store_authorization"
    );
    assert_eq!(
        keys,
        [
            "artifact_id",
            "byte_count",
            "correlation_id",
            "created_at_unix_ms",
            "frame_id",
            "kind",
            "media_type",
            "object_key",
            "producer",
            "redaction_state",
            "retention_class",
            "run_id",
            "sha256",
        ]
    );
}

#[test]
fn artifact_projection_controls_object_key_and_rejects_unknown_fields() {
    let issued = artifact(b"trusted stored bytes");
    let hidden = issued.reference().project(false);
    let visible = issued.reference().project(true);

    assert_eq!(hidden.object_key, None);
    assert_eq!(
        visible.object_key.as_deref(),
        Some(issued.reference().object_key())
    );

    let mut value = serde_json::to_value(visible).expect("artifact projection value");
    value["smuggled"] = serde_json::json!("token-secret-projection-artifact");
    let error = serde_json::from_value::<ProjectedArtifactReference>(value)
        .expect_err("unknown artifact projection field");
    assert!(!error.to_string().contains("token-secret"));
}

#[test]
fn tagged_payload_and_projection_layers_reject_unknown_fields() {
    let sanitized = sanitize(
        ClientPayloadDraft::cli_command(EventAction::RuntimeStatus, AuditInput::new()).into(),
        1,
    );
    let payload = serde_json::to_value(sanitized.payload()).expect("payload value");

    let mut event_layer = payload.clone();
    event_layer["smuggled"] = serde_json::json!("token-secret-event");
    let error = serde_json::from_value::<EventPayload>(event_layer)
        .expect_err("unknown event payload field");
    assert!(!error.to_string().contains("token-secret"));

    let mut family_layer = payload.clone();
    family_layer["payload"]["smuggled"] = serde_json::json!("token-secret-family");
    let error = serde_json::from_value::<EventPayload>(family_layer)
        .expect_err("unknown family payload field");
    assert!(!error.to_string().contains("token-secret"));

    let mut detail_layer = payload.clone();
    detail_layer["payload"]["data"]["smuggled"] = serde_json::json!("token-secret-detail");
    let error = serde_json::from_value::<EventPayload>(detail_layer)
        .expect_err("unknown payload detail field");
    assert!(!error.to_string().contains("token-secret"));

    let mut audit_layer = payload.clone();
    audit_layer["payload"]["data"]["audit"]["smuggled"] = serde_json::json!("token-secret-audit");
    let error =
        serde_json::from_value::<EventPayload>(audit_layer).expect_err("unknown audit field");
    assert!(!error.to_string().contains("token-secret"));

    let mut projection_layer = serde_json::json!({
        "detail": "full",
        "payload": payload,
    });
    projection_layer["smuggled"] = serde_json::json!("token-secret-projection");
    let error = serde_json::from_value::<ProjectionPayload>(projection_layer)
        .expect_err("unknown projection payload field");
    assert!(!error.to_string().contains("token-secret"));

    let public_projection =
        ProjectionPayload::Public(Box::new(sanitized.payload().public_projection()));
    let mut public_family_layer =
        serde_json::to_value(&public_projection).expect("public projection value");
    public_family_layer["payload"]["smuggled"] = serde_json::json!("token-secret-public-family");
    let error = serde_json::from_value::<ProjectionPayload>(public_family_layer)
        .expect_err("unknown public family field");
    assert!(!error.to_string().contains("token-secret"));

    let mut public_detail_layer =
        serde_json::to_value(&public_projection).expect("public projection value");
    public_detail_layer["payload"]["payload"]["smuggled"] =
        serde_json::json!("token-secret-public-detail");
    let error = serde_json::from_value::<ProjectionPayload>(public_detail_layer)
        .expect_err("unknown public payload detail field");
    assert!(!error.to_string().contains("token-secret"));
}

#[test]
fn event_v2_round_trips_every_c1_payload_variant() {
    let payloads = all_payload_drafts(AuditInput::new);
    assert_eq!(payloads.len(), 93);

    // Timing fields: round trip, legacy wire without them, and family scope.
    let measured_input = sanitize(
        InputPayloadDraft::committed_with_touch_response(
            EventAction::RuntimeAction,
            EffectDisposition::Performed,
            Some(4_200),
            AuditInput::new(),
        )
        .into(),
        1,
    );
    let measured_capture = sanitize(
        CapturePayloadDraft::completed_with_capture_acquire(
            EventAction::CaptureObserve,
            EffectDisposition::Performed,
            1280,
            720,
            Some(38_000),
            AuditInput::new(),
        )
        .into(),
        2,
    );
    let input_json = serde_json::to_value(measured_input.payload()).expect("input JSON");
    let capture_json = serde_json::to_value(measured_capture.payload()).expect("capture JSON");
    assert_eq!(input_json["payload"]["data"]["touch_response_us"], 4_200);
    assert_eq!(
        capture_json["payload"]["data"]["capture_acquire_us"],
        38_000
    );
    let EventPayload::Input(InputPayload::Committed(committed)) =
        serde_json::from_value::<EventPayload>(input_json.clone()).expect("input round trip")
    else {
        panic!("expected input.committed")
    };
    assert_eq!(committed.touch_response_us(), Some(4_200));
    let EventPayload::Capture(CapturePayload::Completed(completed)) =
        serde_json::from_value::<EventPayload>(capture_json.clone()).expect("capture round trip")
    else {
        panic!("expected capture.completed")
    };
    assert_eq!(completed.capture_acquire_us(), Some(38_000));

    let mut legacy_input = input_json.clone();
    legacy_input["payload"]["data"]
        .as_object_mut()
        .expect("input data")
        .remove("touch_response_us");
    let legacy_input =
        serde_json::from_value::<EventPayload>(legacy_input).expect("legacy input wire");
    legacy_input.validate().expect("legacy input validates");
    let EventPayload::Input(InputPayload::Committed(committed)) = legacy_input else {
        panic!("expected legacy input.committed")
    };
    assert_eq!(committed.touch_response_us(), None);
    let mut legacy_capture = capture_json.clone();
    legacy_capture["payload"]["data"]
        .as_object_mut()
        .expect("capture data")
        .remove("capture_acquire_us");
    let legacy_capture =
        serde_json::from_value::<EventPayload>(legacy_capture).expect("legacy capture wire");
    legacy_capture.validate().expect("legacy capture validates");
    let EventPayload::Capture(CapturePayload::Completed(completed)) = legacy_capture else {
        panic!("expected legacy capture.completed")
    };
    assert_eq!(completed.capture_acquire_us(), None);

    let mut wrong_input_owner = input_json;
    wrong_input_owner["family"] = serde_json::json!("task");
    wrong_input_owner["payload"]["kind"] = serde_json::json!("completed");
    let wrong_input_owner =
        serde_json::from_value::<EventPayload>(wrong_input_owner).expect("outcome wire shape");
    assert_eq!(
        wrong_input_owner
            .validate()
            .expect_err("touch response scope")
            .code()
            .as_str(),
        "invalid_touch_response_scope"
    );
    let mut wrong_capture_owner = capture_json;
    wrong_capture_owner["family"] = serde_json::json!("recognition");
    let wrong_capture_owner = serde_json::from_value::<EventPayload>(wrong_capture_owner)
        .expect("observation result wire shape");
    assert_eq!(
        wrong_capture_owner
            .validate()
            .expect_err("capture acquire scope")
            .code()
            .as_str(),
        "invalid_capture_acquire_scope"
    );

    for (index, payload) in payloads.into_iter().enumerate() {
        let sanitized = sanitize(payload, index as u64 + 1);
        assert_eq!(sanitized.schema_version(), GLOBAL_EVENT_SCHEMA_VERSION);
        assert_eq!(sanitized.event_type(), sanitized.payload().event_type());

        let json = serde_json::to_string(sanitized.payload()).expect("serialize typed payload");
        let round_trip: EventPayload =
            serde_json::from_str(&json).expect("deserialize typed payload");
        assert_eq!(round_trip, *sanitized.payload());
    }
}

#[test]
fn fact_public_projection_exposes_identity_without_inline_value() {
    let event = sanitize(
        FactPayloadDraft::published(
            FactRecord {
                scope: FactScope::Instance {
                    instance_id: "instance:fixture-a".to_owned(),
                },
                key: "env.theme".to_owned(),
                content: FactContent::Inline {
                    value: FactValue::String("private-inline-value".to_owned()),
                },
                observed_at_unix_ms: 1_752_147_201_000,
                expires_at_unix_ms: None,
                ttl_policy: None,
                confidence_milli: 900,
                source_detector: "detector.theme".to_owned(),
                source_snapshot_id: "snapshot:fixture-a".to_owned(),
                schema_version: "fact.v1".to_owned(),
                resource_bundle_hash: "d".repeat(64),
                invalidate_on: Vec::new(),
            },
            AuditInput::new(),
        )
        .into(),
        1,
    );
    assert_eq!(event.payload().sensitivity(), Sensitivity::Internal);
    let projection =
        serde_json::to_value(event.payload().public_projection()).expect("fact public projection");
    assert_eq!(projection["family"], "fact");
    assert_eq!(projection["payload"]["fact_key"], "env.theme");
    assert_eq!(
        projection["payload"]["fact_source_snapshot_id"],
        "snapshot:fixture-a"
    );
    assert!(!projection.to_string().contains("private-inline-value"));
}

#[test]
fn artifact_backed_fact_inherits_redaction_sensitivity() {
    let artifact = artifact(b"private-fact-evidence").reference().project(true);
    let object_key = artifact
        .object_key()
        .expect("durable artifact object key")
        .to_owned();
    let event = sanitize(
        FactPayloadDraft::published(
            FactRecord {
                scope: FactScope::Game {
                    game_id: "game:fixture-a".to_owned(),
                },
                key: "health.evidence".to_owned(),
                content: FactContent::Artifact { artifact },
                observed_at_unix_ms: 1_752_147_201_000,
                expires_at_unix_ms: None,
                ttl_policy: None,
                confidence_milli: 1_000,
                source_detector: "detector.health".to_owned(),
                source_snapshot_id: "snapshot:health".to_owned(),
                schema_version: "fact.v1".to_owned(),
                resource_bundle_hash: "d".repeat(64),
                invalidate_on: Vec::new(),
            },
            AuditInput::new(),
        )
        .into(),
        1,
    );
    assert_eq!(event.payload().sensitivity(), Sensitivity::Secret);
    let projection =
        serde_json::to_value(event.payload().public_projection()).expect("fact projection");
    assert!(!projection.to_string().contains(&object_key));
}

#[test]
fn client_and_approval_public_projections_keep_values_private() {
    let secret_hash = format!("sha256:{}", "e".repeat(64));
    let client = sanitize(
        ClientPayloadDraft::action(
            ClientActionRecord::new(
                "settings",
                "account_token",
                ClientActionKind::Input,
                None,
                Some(ClientActionValue::Redacted {
                    sha256: secret_hash.clone(),
                    byte_count: 24,
                }),
            )
            .expect("client action"),
            AuditInput::new(),
        )
        .into(),
        1,
    );
    let projection =
        serde_json::to_value(client.payload().public_projection()).expect("client projection");
    assert_eq!(projection["payload"]["client_surface_id"], "settings");
    assert_eq!(projection["payload"]["client_action_kind"], "input");
    assert!(!projection.to_string().contains(&secret_hash));

    let approval = sanitize(
        ApprovalPayloadDraft::decision(
            ApprovalDecisionRecord::new(
                "approval:fixture-a",
                ApprovalDisposition::Pinned,
                ApprovalTarget::Decision {
                    decision_id: "decision:fixture-a".to_owned(),
                    catalog_hash: format!("sha256:{}", "a".repeat(64)),
                    catalog_version: 3,
                },
                crate::ApprovalReasonCode::new("user_confirmed").expect("approval reason"),
            )
            .expect("approval"),
            AuditInput::new(),
        )
        .into(),
        2,
    );
    let projection =
        serde_json::to_value(approval.payload().public_projection()).expect("approval projection");
    assert_eq!(projection["family"], "approval");
    assert_eq!(projection["payload"]["approval_id"], "approval:fixture-a");
    assert_eq!(projection["payload"]["approval_target_kind"], "decision");
    assert!(!projection.to_string().contains("user_confirmed"));
}

#[test]
fn policy_failure_payload_rejects_impossible_retry_combinations() {
    for (alias, valid) in [
        (" Instance Ω ".to_owned(), true),
        (" ".to_owned(), true),
        ("é".repeat(128), true),
        ("é".repeat(129), false),
        ("node\n".to_owned(), false),
    ] {
        for payload in [
            PolicyPayloadDraft::execution_recorded(
                PolicyExecutionEventData {
                    decision_id: "decision:fixture-a".to_owned(),
                    task_id: "task:fixture-a".to_owned(),
                    instance_id: alias.clone(),
                    observed_at_unix_ms: 1_752_147_201_000,
                    outcome: PolicyExecutionOutcome::Succeeded { runtime_ms: 1 },
                },
                AuditInput::new(),
            ),
            PolicyPayloadDraft::planning_signal_observed(
                PolicyPlanningSignalEventData {
                    signal_id: "signal:fixture-a".to_owned(),
                    instance_id: alias.clone(),
                    task_id: None,
                    kind: PolicyPlanningSignalKind::GoalMissed,
                    fact_code: "goal.fixture.missed".to_owned(),
                    observed_at_unix_ms: 1_752_147_201_000,
                    detection_budget: None,
                },
                AuditInput::new(),
            ),
        ] {
            if valid {
                let event = sanitize(payload.into(), 1);
                let json = serde_json::to_value(event.payload()).unwrap();
                assert_eq!(json["payload"]["data"]["instance_id"], alias);
            } else {
                assert_eq!(
                    sanitize_error(payload.into()).code().as_str(),
                    "invalid_policy_token"
                );
            }
        }
    }
    let base = PolicyFailureRecord {
        error_code: "transient.capture".to_owned(),
        reported_success: false,
        original_class: PolicyFailureClass::Recoverable,
        effective_class: PolicyFailureClass::Recoverable,
        consecutive_same_error: 1,
        escalation_streak: 1,
        performance_tax_exempt: false,
        retry_attempt: 1,
        disposition: PolicyFailureDisposition::RetryScheduled,
        retry_at_unix_ms: Some(1_752_147_201_100),
        runtime_ms: 1_000,
        sensitive: false,
        perf_context: Box::new(PerformanceContext::unavailable(1_752_147_201_000)),
    };
    let event = |failure| {
        PolicyPayloadDraft::execution_recorded(
            PolicyExecutionEventData {
                decision_id: "decision:fixture-a".to_owned(),
                task_id: "task:fixture-a".to_owned(),
                instance_id: "instance:fixture-a".to_owned(),
                observed_at_unix_ms: 1_752_147_201_000,
                outcome: PolicyExecutionOutcome::Failed { failure },
            },
            AuditInput::new(),
        )
        .into()
    };

    let mut severe_retry = base.clone();
    severe_retry.effective_class = PolicyFailureClass::Severe;
    assert_eq!(
        sanitize_error(event(severe_retry)).code().as_str(),
        "invalid_policy_failure_record"
    );

    let mut hidden_retry_attempt = base.clone();
    hidden_retry_attempt.disposition = PolicyFailureDisposition::Continue;
    hidden_retry_attempt.retry_at_unix_ms = None;
    assert_eq!(
        sanitize_error(event(hidden_retry_attempt)).code().as_str(),
        "invalid_policy_failure_record"
    );

    let mut unsupported_exemption = base.clone();
    unsupported_exemption.performance_tax_exempt = true;
    unsupported_exemption.escalation_streak = 0;
    assert_eq!(
        sanitize_error(event(unsupported_exemption)).code().as_str(),
        "invalid_policy_failure_record"
    );

    let mut past_retry = base;
    past_retry.retry_at_unix_ms = Some(1_752_147_201_000);
    assert_eq!(
        sanitize_error(event(past_retry)).code().as_str(),
        "invalid_policy_failure_record"
    );
}

#[test]
fn performance_payload_rejects_fake_health_and_invalid_stutter() {
    // Instance alias copied by the policy admission deadline gate.
    let alias = format!(" {} ", "é".repeat(127));
    let admitted_alias = sanitize(
        PerformancePayloadDraft::balance_changed(
            PerformanceControlEventData {
                observed_at_unix_ms: 1_752_147_201_000,
                instance_id: Some(alias.clone()),
                previous_level: PerformanceControlLevel::DispatchPaused,
                level: PerformanceControlLevel::DispatchPaused,
                reason: PerformanceControlReason::DeadlineConflict,
                host_responsiveness_basis_points: None,
                third_party_pressure_basis_points: None,
                recovery: false,
                deadline_disposition: Some(PerformanceDeadlineDisposition::CapacityFailure),
                arbitration: None,
            },
            AuditInput::new(),
        )
        .into(),
        1,
    );
    assert_eq!(
        serde_json::to_value(admitted_alias.payload()).unwrap()["payload"]["data"]["instance_id"],
        alias
    );
    let fake_health: EventPayloadDraft = PerformancePayloadDraft::monitor_degraded(
        PerformanceMonitorStateEventData {
            observed_at_unix_ms: 1_752_147_201_000,
            health: PerformanceMonitorHealth::Degraded,
            outcome: None,
            consecutive_failures: 1,
            terminal: false,
            unavailable_metrics: vec![PerformanceMetric::Gpu],
        },
        AuditInput::new(),
    )
    .into();
    assert_eq!(
        sanitize_error(fake_health).code().as_str(),
        "invalid_performance_monitor_state"
    );

    let invalid_stutter: EventPayloadDraft = PerformancePayloadDraft::stutter_detected(
        PerformanceStutterEventData {
            instance_id: "instance:fixture-a".to_owned(),
            observed_at_unix_ms: 1_752_147_201_000,
            frame_gap_ms: 0,
            capture_latency_ms: None,
            recognition_latency_ms: None,
            action_effect_latency_ms: None,
        },
        AuditInput::new(),
    )
    .into();
    assert_eq!(
        sanitize_error(invalid_stutter).code().as_str(),
        "invalid_performance_stutter"
    );

    let invalid_control: EventPayloadDraft = PerformancePayloadDraft::balance_changed(
        PerformanceControlEventData {
            observed_at_unix_ms: 1_752_147_201_000,
            instance_id: Some("instance:fixture-a".to_owned()),
            previous_level: PerformanceControlLevel::DispatchPaused,
            level: PerformanceControlLevel::DispatchPaused,
            reason: PerformanceControlReason::Recovery,
            host_responsiveness_basis_points: Some(10_000),
            third_party_pressure_basis_points: Some(0),
            recovery: true,
            deadline_disposition: Some(PerformanceDeadlineDisposition::Throttled),
            arbitration: None,
        },
        AuditInput::new(),
    )
    .into();
    assert_eq!(
        sanitize_error(invalid_control).code().as_str(),
        "invalid_performance_control_transition"
    );
}

#[test]
fn legacy_policy_failure_defaults_preserve_streak_and_explicitly_mark_context_unavailable() {
    let failure = PolicyFailureRecord {
        error_code: "transient.capture".to_owned(),
        reported_success: false,
        original_class: PolicyFailureClass::Recoverable,
        effective_class: PolicyFailureClass::Recoverable,
        consecutive_same_error: 3,
        escalation_streak: 3,
        performance_tax_exempt: false,
        retry_attempt: 0,
        disposition: PolicyFailureDisposition::Continue,
        retry_at_unix_ms: None,
        runtime_ms: 1_000,
        sensitive: false,
        perf_context: Box::new(PerformanceContext::unavailable(1_752_147_201_000)),
    };
    let mut legacy = serde_json::to_value(&failure).expect("failure JSON");
    let object = legacy.as_object_mut().expect("failure object");
    object.remove("escalation_streak");
    object.remove("performance_tax_exempt");
    object.remove("perf_context");
    let recovered: PolicyFailureRecord = serde_json::from_value(legacy).expect("legacy failure");
    assert_eq!(recovered.escalation_streak, 3);
    assert!(!recovered.performance_tax_exempt);
    assert_eq!(
        recovered.perf_context.health,
        PerformanceMonitorHealth::Unavailable
    );

    let mut measured = PerformanceContext::unavailable(1_752_147_201_000);
    measured.max_touch_response_us = Some(4_200);
    measured.max_capture_acquire_us = Some(38_000);
    let measured_json = serde_json::to_value(&measured).expect("context JSON");
    assert_eq!(measured_json["max_touch_response_us"], 4_200);
    assert_eq!(measured_json["max_capture_acquire_us"], 38_000);
    let round_trip: PerformanceContext =
        serde_json::from_value(measured_json.clone()).expect("context round trip");
    assert_eq!(round_trip, measured);
    let mut legacy_context = measured_json;
    let object = legacy_context.as_object_mut().expect("context object");
    object.remove("max_touch_response_us");
    object.remove("max_capture_acquire_us");
    let legacy_context: PerformanceContext =
        serde_json::from_value(legacy_context).expect("legacy context without timing maxima");
    assert_eq!(legacy_context.max_touch_response_us, None);
    assert_eq!(legacy_context.max_capture_acquire_us, None);

    let mut explicit_null = serde_json::to_value(&failure).expect("failure JSON");
    explicit_null["escalation_streak"] = serde_json::Value::Null;
    serde_json::from_value::<PolicyFailureRecord>(explicit_null)
        .expect_err("explicit null streak must not use the legacy default");
}

#[test]
fn task_semantic_payload_preserves_recognition_basis_and_v3_schema() {
    let event = sanitize(
        TaskPayloadDraft::semantic(
            TaskSemanticFact::RecognitionCompleted {
                candidate_pages: vec!["home".to_string(), "campaign".to_string()],
                matched_page: Some("campaign".to_string()),
                frame_width: 1280,
                frame_height: 720,
                targets: Vec::new(),
            },
            AuditInput::new(),
        )
        .into(),
        1,
    );

    assert_eq!(event.event_type(), EventType::TaskRecognitionCompleted);
    assert_eq!(event.payload_schema(), TASK_PAYLOAD_SCHEMA);
    assert_eq!(event.payload().family(), EventFamily::Task);
    let projection = serde_json::to_value(event.payload().public_projection())
        .expect("task semantic projection");
    assert_eq!(projection["family"], "task");
    assert_eq!(
        projection["payload"]["task_semantic_fact"]["kind"],
        "recognition_completed"
    );
    assert_eq!(
        projection["payload"]["task_semantic_fact"]["candidate_pages"],
        serde_json::json!(["home", "campaign"])
    );
    assert_eq!(
        projection["payload"]["task_semantic_fact"]["matched_page"],
        "campaign"
    );
}

#[test]
fn task_semantic_effect_intent_redacts_text_and_key_before_persistence() {
    let identifiers = IdentifierIssuer::new().unwrap();
    let step = identifiers.mint_action_id().unwrap();
    let frame = identifiers.mint_frame_id().unwrap();
    for (index, action) in [
        InputAction::Text {
            text: "authentication-secret-task-text".to_string(),
        },
        InputAction::Key {
            key: "authentication-secret-task-key".to_string(),
        },
    ]
    .into_iter()
    .enumerate()
    {
        let event = sanitize(
            TaskPayloadDraft::semantic(
                TaskSemanticFact::EffectIntent {
                    frame_extent: None,
                    step_index: index as u32,
                    operation_label: "input".to_string(),
                    action: action.clone(),
                },
                AuditInput::new(),
            )
            .into(),
            index as u64 + 1,
        );
        let durable = serde_json::to_string(event.payload()).expect("semantic payload JSON");

        assert!(!durable.contains("authentication-secret-task"));
        assert!(durable.contains("[redacted]"));
        let intent = sanitize(
            InputPayloadDraft::intent_with_provenance(
                action.clone(),
                None,
                Some(*step.transport()),
                Some(*frame.transport()),
                AuditInput::new(),
            )
            .into(),
            index as u64 + 3,
        );
        let persisted = serde_json::to_string(intent.payload()).unwrap();
        assert!(!persisted.contains("authentication-secret-task"));
        assert!(persisted.contains("[redacted]"));
        assert!(
            serde_json::to_string(&action)
                .unwrap()
                .contains("authentication-secret-task")
        );
        let decoded: EventPayload = serde_json::from_str(&persisted).unwrap();
        let EventPayload::Input(InputPayload::Intent(payload)) = decoded else {
            panic!("typed input intent");
        };
        let provenance = payload.provenance().unwrap();
        assert_eq!(provenance.source_step_action_id, Some(*step.transport()));
        assert_eq!(provenance.before_frame_id, Some(*frame.transport()));
        let EventPayload::Task(TaskPayload::Semantic(semantic)) = event.payload() else {
            panic!("typed semantic intent");
        };
        let TaskSemanticFact::EffectIntent { action, .. } = semantic.fact() else {
            panic!("semantic input action");
        };
        assert_eq!(&provenance.input_action, action);
    }
}

#[test]
fn task_semantic_effect_intent_sampling_round_trips_and_legacy_payload_decodes() {
    let sampling = InputSamplingEvidence::new(
        77,
        vec![InputSamplingRegion::new(10, 20, 30, 40).expect("sampling region")],
    )
    .expect("sampling evidence");
    let sampled = sanitize(
        TaskPayloadDraft::semantic_with_sampling(
            TaskSemanticFact::EffectIntent {
                frame_extent: None,
                step_index: 3,
                operation_label: "collect".to_string(),
                action: InputAction::Tap { x: 11, y: 22 },
            },
            sampling,
            AuditInput::new(),
        )
        .into(),
        1,
    );
    let sampled_json = serde_json::to_value(sampled.payload()).expect("sampled payload JSON");
    let recovered: EventPayload =
        serde_json::from_value(sampled_json).expect("sampled payload round trip");
    let EventPayload::Task(TaskPayload::Semantic(payload)) = recovered else {
        panic!("expected task semantic payload")
    };
    let sampling = payload.sampling().expect("sampling metadata");
    assert_eq!(
        sampling.algorithm(),
        InputSamplingAlgorithm::Xorshift64UniformRectV1
    );
    assert_eq!(sampling.action_seed(), 77);
    assert_eq!(sampling.source_regions()[0].x(), 10);
    assert_eq!(sampling.source_regions()[0].height(), 40);

    let legacy = sanitize(
        TaskPayloadDraft::semantic(
            TaskSemanticFact::EffectIntent {
                frame_extent: None,
                step_index: 4,
                operation_label: "legacy".to_string(),
                action: InputAction::Tap { x: 1, y: 2 },
            },
            AuditInput::new(),
        )
        .into(),
        2,
    );
    let legacy_json = serde_json::to_value(legacy.payload()).expect("legacy payload JSON");
    assert!(!legacy_json.to_string().contains("sampling"));
    let recovered: EventPayload =
        serde_json::from_value(legacy_json).expect("legacy payload without sampling");
    let EventPayload::Task(TaskPayload::Semantic(payload)) = recovered else {
        panic!("expected legacy task semantic payload")
    };
    assert!(payload.sampling().is_none());
}

#[test]
fn task_semantic_effect_intent_sampling_rejects_invalid_evidence() {
    let invalid_region: InputSamplingEvidence = serde_json::from_value(serde_json::json!({
        "algorithm": "xorshift64_uniform_rect_v1",
        "action_seed": 1,
        "source_regions": [{"x": 0, "y": 0, "width": 0, "height": 1}]
    }))
    .expect("invalid region shape remains syntactically typed");
    let mismatch = InputSamplingEvidence::new(
        2,
        vec![InputSamplingRegion::new(10, 10, 2, 2).expect("sampling region")],
    )
    .expect("sampling evidence");

    for (fact, sampling) in [
        (
            TaskSemanticFact::EffectIntent {
                frame_extent: None,
                step_index: 1,
                operation_label: "invalid-region".to_string(),
                action: InputAction::Tap { x: 0, y: 0 },
            },
            invalid_region,
        ),
        (
            TaskSemanticFact::EffectIntent {
                frame_extent: None,
                step_index: 2,
                operation_label: "mismatch".to_string(),
                action: InputAction::Tap { x: 0, y: 0 },
            },
            mismatch.clone(),
        ),
        (TaskSemanticFact::RunStarted, mismatch),
    ] {
        let issuer = identifier_issuer();
        EventDraft::new(
            issuer.mint_event_id().expect("event id"),
            1_752_147_200_000,
            EventSeverity::Info,
            origin(),
            links(&issuer),
            TaskPayloadDraft::semantic_with_sampling(fact, sampling, AuditInput::new()).into(),
        )
        .sanitize(&SpyFingerprinter::new())
        .expect_err("invalid sampling evidence");
    }

    serde_json::from_value::<InputSamplingEvidence>(serde_json::json!({
        "algorithm": "unknown_algorithm",
        "action_seed": 1,
        "source_regions": [{"x": 0, "y": 0, "width": 1, "height": 1}]
    }))
    .expect_err("unknown sampling algorithm");
}

#[test]
fn task_semantic_payload_rejects_invalid_facts() {
    let invalid = [
        TaskSemanticFact::PackageAdmitted {
            package_label: "package".to_string(),
            task_label: "task".to_string(),
            package_sha256: "not-a-sha256".into(),
            response_deadline_monotonic_ms: Some(60_000),
        },
        TaskSemanticFact::RecognitionStarted {
            candidate_pages: Vec::new(),
            frame_width: 1280,
            frame_height: 720,
        },
        TaskSemanticFact::RecognitionCompleted {
            candidate_pages: vec!["home".to_string()],
            matched_page: Some("campaign".to_string()),
            frame_width: 1280,
            frame_height: 720,
            targets: Vec::new(),
        },
        TaskSemanticFact::TerminalCommitted {
            task_result: TaskOutcome::Success,
            final_page: Some("home".to_string()),
            executed_steps: Some(1),
            outcome: Some(Outcome::relayed("must_be_absent")),
            failure_severe: false,
            scheduling_disposition: None,
            task_timing: None,
        },
        TaskSemanticFact::TerminalCommitted {
            task_result: TaskOutcome::Success,
            final_page: Some("home".to_string()),
            executed_steps: None,
            outcome: None,
            failure_severe: false,
            scheduling_disposition: None,
            task_timing: None,
        },
    ];

    for fact in invalid {
        let issuer = identifier_issuer();
        EventDraft::new(
            issuer.mint_event_id().expect("event id"),
            1_752_147_200_000,
            EventSeverity::Info,
            origin(),
            links(&issuer),
            TaskPayloadDraft::semantic(fact, AuditInput::new()).into(),
        )
        .sanitize(&SpyFingerprinter::new())
        .expect_err("invalid task semantic fact");
    }
}

#[test]
fn scheduling_outcome_declaration_is_finite_unique_and_identifier_closed() {
    let valid: SchedulingOutcomeDeclaration = serde_json::from_value(serde_json::json!({
        "designated_operation": "collect",
        "mappings": [
            {
                "outcome_key": "effect-applied",
                "effect": "designated_effect_completed",
                "terminal_pages": ["terminal"]
            },
            {
                "outcome_key": "no-effect",
                "effect": "no_designated_effect",
                "terminal_pages": ["terminal"]
            }
        ]
    }))
    .expect("valid declaration shape");
    valid.validate().expect("valid declaration");

    for invalid in [
        serde_json::json!({"mappings": []}),
        serde_json::json!({
            "mappings": [
                {
                    "outcome_key": "duplicate",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": "duplicate",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["alternate"]
                }
            ]
        }),
        serde_json::json!({
            "mappings": [
                {
                    "outcome_key": "first",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": "second",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }
            ]
        }),
        serde_json::json!({
            "mappings": [{
                "outcome_key": "contains whitespace",
                "effect": "no_designated_effect",
                "terminal_pages": ["terminal"]
            }]
        }),
        serde_json::json!({
            "mappings": [{
                "outcome_key": "effect-applied",
                "effect": "designated_effect_completed",
                "terminal_pages": ["terminal"]
            }]
        }),
    ] {
        let declaration: SchedulingOutcomeDeclaration =
            serde_json::from_value(invalid).expect("closed declaration shape");
        declaration
            .validate()
            .expect_err("invalid scheduling declaration");
    }
}

#[test]
fn resource_authoring_payloads_preserve_phase_and_publish_only_path_count() {
    let phases = [
        (
            ResourceAuthoringPhase::AuthoringStarted,
            EventType::ResourceAuthoringStarted,
            None,
        ),
        (
            ResourceAuthoringPhase::DraftBuilt,
            EventType::ResourceDraftBuilt,
            None,
        ),
        (
            ResourceAuthoringPhase::ValidationCompleted,
            EventType::ResourceValidationCompleted,
            None,
        ),
        (
            ResourceAuthoringPhase::PromoteIntent,
            EventType::ResourcePromoteIntent,
            None,
        ),
        (
            ResourceAuthoringPhase::Promoted,
            EventType::ResourcePromoted,
            None,
        ),
        (
            ResourceAuthoringPhase::PromoteFailed,
            EventType::ResourcePromoteFailed,
            Some(Outcome::relayed("authoring_validation_failed")),
        ),
    ];

    for (index, (phase, expected_type, failure_code)) in phases.into_iter().enumerate() {
        let event = sanitize(
            ResourceAuthoringPayloadDraft::event(
                phase,
                "draft-a",
                "resource-root",
                "b".repeat(64),
                vec![
                    "operations/task-a/task.json".to_string(),
                    "operations/task-a/frame.png".to_string(),
                ],
                failure_code,
                AuditInput::new().with_machine_path(r"C:\private\resource-root"),
            )
            .into(),
            index as u64 + 1,
        );
        assert_eq!(event.event_type(), expected_type);
        assert_eq!(event.payload().family(), EventFamily::ResourceAuthoring);

        let projection = serde_json::to_value(event.payload().public_projection())
            .expect("authoring projection");
        assert_eq!(projection["family"], "resource_authoring");
        assert_eq!(projection["payload"]["authoring_phase"], phase.as_str());
        assert_eq!(projection["payload"]["draft_id"], "draft-a");
        assert_eq!(projection["payload"]["target_label"], "resource-root");
        assert_eq!(projection["payload"]["target_fingerprint"], "b".repeat(64));
        assert_eq!(projection["payload"]["changed_path_count"], 2);
        let encoded = serde_json::to_string(&projection).expect("projection JSON");
        assert!(!encoded.contains("operations/task-a"));
        assert!(!encoded.contains("C:\\private"));
    }
}

#[test]
fn resource_authoring_payload_rejects_invalid_identity_paths_and_failure_semantics() {
    let invalid = [
        ResourceAuthoringPayloadDraft::event(
            ResourceAuthoringPhase::AuthoringStarted,
            "",
            "resource-root",
            "b".repeat(64),
            vec!["operations/task-a/task.json".to_string()],
            None,
            AuditInput::new(),
        ),
        ResourceAuthoringPayloadDraft::event(
            ResourceAuthoringPhase::AuthoringStarted,
            "draft-a",
            "resource-root",
            "B".repeat(64),
            vec!["operations/task-a/task.json".to_string()],
            None,
            AuditInput::new(),
        ),
        ResourceAuthoringPayloadDraft::event(
            ResourceAuthoringPhase::AuthoringStarted,
            "draft-a",
            "resource-root",
            "b".repeat(64),
            vec!["../outside.json".to_string()],
            None,
            AuditInput::new(),
        ),
        ResourceAuthoringPayloadDraft::event(
            ResourceAuthoringPhase::PromoteFailed,
            "draft-a",
            "resource-root",
            "b".repeat(64),
            vec!["operations/task-a/task.json".to_string()],
            None,
            AuditInput::new(),
        ),
        ResourceAuthoringPayloadDraft::event(
            ResourceAuthoringPhase::Promoted,
            "draft-a",
            "resource-root",
            "b".repeat(64),
            vec!["operations/task-a/task.json".to_string()],
            Some(Outcome::relayed("unexpected_failure")),
            AuditInput::new(),
        ),
    ];

    for payload in invalid {
        let issuer = identifier_issuer();
        let error = EventDraft::new(
            issuer.mint_event_id().expect("event id"),
            1_752_147_200_000,
            EventSeverity::Info,
            origin(),
            links(&issuer),
            payload.into(),
        )
        .sanitize(&SpyFingerprinter::new())
        .expect_err("invalid resource authoring payload");
        assert!(
            error.code().as_str().starts_with("invalid_")
                || error.code().as_str().starts_with("missing_")
                || error.code().as_str().starts_with("unexpected_")
        );
    }
}

#[test]
fn capture_and_recognition_public_projection_preserve_typed_observation() {
    let capture = sanitize(
        CapturePayloadDraft::completed(
            EventAction::CaptureObserve,
            EffectDisposition::Performed,
            1280,
            720,
            AuditInput::new(),
        )
        .into(),
        1,
    );
    let recognition = sanitize(
        RecognitionPayloadDraft::completed(
            EventAction::RecognitionObserve,
            EffectDisposition::Performed,
            1280,
            720,
            RecognitionVerdict::FrameDecoded,
            AuditInput::new(),
        )
        .into(),
        2,
    );

    for event in [&capture, &recognition] {
        let projection = event.payload().public_projection();
        let json = serde_json::to_value(projection).expect("public projection");
        assert_eq!(json["payload"]["frame_width"], 1280);
        assert_eq!(json["payload"]["frame_height"], 720);
    }
    let projection = recognition.payload().public_projection();
    let json = serde_json::to_value(projection).expect("recognition projection");
    assert_eq!(json["payload"]["recognition_verdict"], "frame_decoded");
}

#[test]
fn artifact_sha256_matches_known_vector() {
    let issued = artifact(b"abc");
    assert_eq!(
        issued.reference().sha256(),
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn c2_artifact_issuer_preserves_store_owned_metadata() {
    let identifiers = identifier_issuer();
    let issuer = ArtifactStoreIssuer::new().expect("artifact issuer");
    let issued = issuer
        .issue(
            ArtifactKind::EvidenceArchive,
            artifact_links(&identifiers),
            b"evidence archive bytes",
            1_752_147_200_001,
            ArtifactIssuePolicy::new(
                ArtifactProducer::EvidenceExporter,
                RetentionClass::DebugFull,
                ArtifactRedactionState::Applied,
            ),
        )
        .expect("issued artifact");

    let reference = issued.reference();
    assert_eq!(reference.kind(), ArtifactKind::EvidenceArchive);
    assert_eq!(reference.media_type(), ArtifactMediaType::ApplicationZip);
    assert_eq!(reference.producer(), ArtifactProducer::EvidenceExporter);
    assert_eq!(reference.retention_class(), RetentionClass::DebugFull);
    assert_eq!(reference.redaction_state(), ArtifactRedactionState::Applied);
    assert!(reference.object_key().ends_with(".zip"));
    reference.validate().expect("valid reference");
}

#[test]
fn c2_capture_pipeline_and_export_projection_preserve_typed_facts() {
    let pressure = sanitize(
        CapturePayloadDraft::pressure_changed(
            CapturePressureState::Tier3Paused,
            10_000,
            9_500,
            AuditInput::new(),
        )
        .into(),
        1,
    );
    let dedup = sanitize(
        CapturePayloadDraft::dedup_window(4, 1_200, AuditInput::new()).into(),
        2,
    );
    let policy = sanitize(
        CapturePayloadDraft::policy_changed(
            300,
            RetentionClass::DebugFull,
            CapturePolicyReason::RequestOverride,
            AuditInput::new(),
        )
        .into(),
        3,
    );
    let export = sanitize(
        ArtifactPayloadDraft::export_completed(
            TaskOutcome::Cancelled,
            EvidenceCompleteness::Partial,
            5,
            AuditInput::new(),
        )
        .into(),
        4,
    );

    let pressure =
        serde_json::to_value(pressure.payload().public_projection()).expect("pressure projection");
    assert_eq!(
        pressure["payload"]["capture_pressure_state"],
        "tier3_paused"
    );
    assert_eq!(pressure["payload"]["memory_budget_bytes"], 10_000);
    assert_eq!(pressure["payload"]["resident_bytes"], 9_500);

    let dedup =
        serde_json::to_value(dedup.payload().public_projection()).expect("dedup projection");
    assert_eq!(dedup["payload"]["duplicate_count"], 4);
    assert_eq!(dedup["payload"]["duration_ms"], 1_200);

    let policy =
        serde_json::to_value(policy.payload().public_projection()).expect("policy projection");
    assert_eq!(policy["payload"]["cadence_ms"], 300);
    assert_eq!(policy["payload"]["retention_class"], "debug_full");
    assert_eq!(
        policy["payload"]["capture_policy_reason"],
        "request_override"
    );

    let export =
        serde_json::to_value(export.payload().public_projection()).expect("export projection");
    assert_eq!(export["family"], "artifact");
    assert_eq!(export["payload"]["task_outcome"], "cancelled");
    assert_eq!(export["payload"]["evidence_completeness"], "partial");
    assert_eq!(export["payload"]["artifact_count"], 5);
}

#[test]
fn capture_summary_contract_is_strict_bounded_and_preserves_pin_pairs() {
    let stored = artifact(b"capture-summary-contract");
    let projected = stored.reference().project(true);
    let frames =
        vec![CapturePersistedEvidence::new(0, projected.clone()).expect("persisted frame")];
    let pins = vec![
        CapturePinnedEvidence::new(
            Some(0),
            PinnedFrameReason::PreInput,
            Some(projected.clone()),
        )
        .expect("pre-input pin"),
        CapturePinnedEvidence::new(
            Some(0),
            PinnedFrameReason::Terminal,
            Some(projected.clone()),
        )
        .expect("terminal pin"),
    ];
    let record = CaptureSummaryRecord::new(
        1,
        0,
        0,
        1,
        EvidenceCompleteness::Complete,
        frames.clone(),
        pins.clone(),
    )
    .expect("valid capture summary");
    let event = sanitize(
        CapturePayloadDraft::summary_committed(record.clone(), AuditInput::new()).into(),
        12_001,
    );
    assert_eq!(event.event_type(), EventType::CaptureSummaryCommitted);
    assert_eq!(event.payload().action(), EventAction::CaptureSummaryCommit);
    assert_eq!(event.payload().sensitivity(), Sensitivity::Internal);
    let encoded = serde_json::to_value(event.payload()).expect("capture summary JSON");
    let decoded: EventPayload =
        serde_json::from_value(encoded.clone()).expect("capture summary round trip");
    assert_eq!(decoded, *event.payload());
    let EventPayload::Capture(CapturePayload::SummaryCommitted(payload)) = decoded else {
        panic!("unexpected capture summary payload");
    };
    assert_eq!(payload.summary(), &record);
    assert_eq!(payload.summary().pinned(), pins);

    let mut unknown = encoded;
    unknown["payload"]["data"]["summary"]["unexpected"] = serde_json::json!(true);
    serde_json::from_value::<EventPayload>(unknown)
        .expect_err("capture summary must reject unknown fields");

    let duplicate_pin = CaptureSummaryRecord::new(
        1,
        0,
        0,
        1,
        EvidenceCompleteness::Complete,
        frames.clone(),
        vec![pins[0].clone(), pins[0].clone()],
    )
    .expect_err("duplicate frame/reason pair must fail");
    assert_eq!(
        duplicate_pin.code().as_str(),
        "capture_summary_pin_conflict"
    );

    let invalid_counts = CaptureSummaryRecord::new(
        1,
        1,
        0,
        1,
        EvidenceCompleteness::Complete,
        frames.clone(),
        Vec::new(),
    )
    .expect_err("capture accounting above captured must fail");
    assert_eq!(
        invalid_counts.code().as_str(),
        "capture_summary_count_mismatch"
    );

    let missing_pin = CaptureSummaryRecord::new(
        1,
        0,
        0,
        1,
        EvidenceCompleteness::Failed,
        frames,
        vec![
            CapturePinnedEvidence::new(Some(0), PinnedFrameReason::Terminal, None)
                .expect("missing terminal pin"),
        ],
    )
    .expect("missing required pin is a valid failed summary");
    assert_eq!(
        missing_pin.evidence_completeness(),
        EvidenceCompleteness::Failed
    );

    let wrong_completeness = CaptureSummaryRecord::new(
        MAX_CAPTURE_SUMMARY_COUNT + 1,
        0,
        0,
        0,
        EvidenceCompleteness::Failed,
        Vec::new(),
        Vec::new(),
    )
    .expect_err("summary count bound must fail");
    assert_eq!(
        wrong_completeness.code().as_str(),
        "capture_summary_bound_exceeded"
    );
}

#[test]
fn c2_payloads_reject_invalid_numeric_and_unknown_closed_values() {
    for payload in [
        CapturePayloadDraft::pressure_changed(
            CapturePressureState::Tier1Dedup,
            0,
            0,
            AuditInput::new(),
        )
        .into(),
        CapturePayloadDraft::dedup_window(0, 300, AuditInput::new()).into(),
        CapturePayloadDraft::policy_changed(
            0,
            RetentionClass::Adaptive,
            CapturePolicyReason::Default,
            AuditInput::new(),
        )
        .into(),
        ArtifactPayloadDraft::export_completed(
            TaskOutcome::Success,
            EvidenceCompleteness::Complete,
            0,
            AuditInput::new(),
        )
        .into(),
    ] {
        let issuer = identifier_issuer();
        let error = EventDraft::new(
            issuer.mint_event_id().expect("event id"),
            1_752_147_200_000,
            EventSeverity::Info,
            origin(),
            links(&issuer),
            payload,
        )
        .sanitize(&SpyFingerprinter::new())
        .expect_err("invalid C2 payload");
        assert!(error.code().as_str().starts_with("invalid_"));
    }

    let value = serde_json::json!({
        "family": "capture",
        "payload": {
            "kind": "pressure_changed",
            "data": {
                "action": "capture.pressure",
                "state": "tier4_impossible",
                "memory_budget_bytes": 1000,
                "resident_bytes": 900,
                "audit": {
                    "account_fingerprint": null,
                    "authentication_fingerprint": null,
                    "machine_path_present": false,
                    "device_endpoint_present": false
                }
            }
        }
    });
    let error =
        serde_json::from_value::<EventPayload>(value).expect_err("unknown pressure state rejected");
    assert!(!error.to_string().contains("tier4_impossible"));
}

#[test]
fn c3b_queue_preemption_and_transfer_facts_are_typed_and_strict() {
    let ids = identifier_issuer();
    let from_holder = *ids.mint_holder_id().expect("from holder").transport();
    let to_holder = *ids.mint_holder_id().expect("to holder").transport();
    let from_lease = *ids.mint_lease_id().expect("from lease").transport();
    let to_lease = *ids.mint_lease_id().expect("to lease").transport();
    let queued_request = *ids.mint_request_id().expect("queued request").transport();

    let queued = sanitize(
        SchedulerPayloadDraft::queued(
            EventAction::ScheduleAdmit,
            crate::LeasePriority::High,
            2,
            500,
            true,
            AuditInput::new(),
        )
        .into(),
        10_001,
    );
    let queued_value = serde_json::to_value(queued.payload()).expect("queued payload");
    assert_eq!(queued_value["payload"]["kind"], "queued");
    assert_eq!(queued_value["payload"]["data"]["priority"], "high");
    assert_eq!(queued_value["payload"]["data"]["position"], 2);
    assert_eq!(queued_value["payload"]["data"]["preempt_requested"], true);

    let preempted = sanitize(
        SchedulerPayloadDraft::preempted(
            EventAction::ScheduleAdmit,
            from_holder,
            from_lease,
            queued_request,
            crate::LeasePriority::High,
            true,
            AuditInput::new(),
        )
        .into(),
        10_002,
    );
    let preempted_value = serde_json::to_value(preempted.payload()).expect("preempt payload");
    assert_eq!(preempted_value["payload"]["kind"], "preempted");
    assert_eq!(
        preempted_value["payload"]["data"]["deferred_by_destructive_step"],
        true
    );

    let transferred = sanitize(
        LeasePayloadDraft::transferred(
            EventAction::LeaseAcquire,
            EffectDisposition::Performed,
            from_holder,
            from_lease,
            to_holder,
            to_lease,
            queued_request,
            crate::LeasePriority::High,
            AuditInput::new(),
        )
        .into(),
        10_003,
    );
    let transferred_value = serde_json::to_value(transferred.payload()).expect("transfer payload");
    assert_eq!(transferred_value["payload"]["kind"], "transferred");
    assert_eq!(
        transferred_value["payload"]["data"]["from_lease_id"],
        serde_json::to_value(from_lease).expect("from lease JSON")
    );
    assert_eq!(
        transferred_value["payload"]["data"]["to_lease_id"],
        serde_json::to_value(to_lease).expect("to lease JSON")
    );

    let invalid_queue = SchedulerPayloadDraft::queued(
        EventAction::ScheduleAdmit,
        crate::LeasePriority::Normal,
        0,
        500,
        false,
        AuditInput::new(),
    );
    let error = EventDraft::new(
        ids.mint_event_id().expect("event id"),
        1_752_147_200_000,
        EventSeverity::Info,
        origin(),
        links(&ids),
        invalid_queue.into(),
    )
    .sanitize(&SpyFingerprinter::new())
    .expect_err("zero queue position");
    assert_eq!(error.code().as_str(), "invalid_scheduler_queue");

    let invalid_transfer = LeasePayloadDraft::transferred(
        EventAction::LeaseAcquire,
        EffectDisposition::Performed,
        from_holder,
        from_lease,
        to_holder,
        from_lease,
        queued_request,
        crate::LeasePriority::High,
        AuditInput::new(),
    );
    let error = EventDraft::new(
        ids.mint_event_id().expect("event id"),
        1_752_147_200_000,
        EventSeverity::Info,
        origin(),
        links(&ids),
        invalid_transfer.into(),
    )
    .sanitize(&SpyFingerprinter::new())
    .expect_err("same lease transfer");
    assert_eq!(error.code().as_str(), "invalid_lease_transfer");
}

// Workflow #378 A3 (ledger revision 3, model section 8.1): every record shape A3 touched round
// trips with its `outcome`, every serde member named `outcome` is the envelope, no legacy
// member survives, and a record's severity is its category.

/// The `outcome` members of a serialized record: each must be an envelope (`{"code":..`).
fn assert_outcome_members_are_envelopes(json: &str) {
    for (index, _) in json.match_indices("\"outcome\":") {
        let rest = &json[index + "\"outcome\":".len()..];
        assert!(
            rest.starts_with("{\"code\":"),
            "a member named outcome that is not the envelope: {}",
            &json[index..(index + 80).min(json.len())]
        );
    }
}

/// Members ledger revision 3 removed from the records A3 converted.
const REVISION_2_MEMBERS: [&str; 5] = [
    "\"failure_code\"",
    "\"native_detail\"",
    "\"cleanup_cause\"",
    "\"message\"",
    "\"fatal\"",
];

/// Members no record of any family keeps (ledger revision 3): persisted text and the former
/// cleanup cause record.
const REVISION_2_TEXT_MEMBERS: [&str; 3] =
    ["\"native_detail\"", "\"cleanup_cause\"", "\"message\""];

/// A selection record whose one actionable candidate was selected, with `confirmation`
/// (acceptance CL-L6: the confirmation's `guard_failed` and `capture_failed` outcomes).
fn revision_3_selection(confirmation: TaskSelectionConfirmation) -> EventPayloadDraft {
    let candidate_id = "slots/main#00".to_owned();
    let projection = crate::CandidateProjection::new(
        "list_page",
        "slots/main",
        crate::CandidateLayoutKind::FixedSlots,
        crate::CandidateFrame {
            width: 1280,
            height: 720,
        },
        vec![crate::ProjectedCandidate {
            id: candidate_id.clone(),
            instance_index: 0,
            actionable: true,
            rect: crate::CandidateRect {
                x: 214,
                y: 204,
                width: 176,
                height: 90,
            },
            click: crate::CandidateRect {
                x: 224,
                y: 214,
                width: 156,
                height: 60,
            },
            features: crate::CandidateFeatureMap::from([(
                "marked".to_owned(),
                crate::CandidateFeature::Boolean {
                    value: true,
                    confidence: Some(940),
                },
            )]),
        }],
    )
    .expect("candidate projection");
    TaskPayloadDraft::semantic(
        TaskSemanticFact::SelectionEvaluated {
            step_index: 1,
            operation_label: "select".to_owned(),
            selection: Box::new(TaskSelectionRecord {
                layout_id: "slots/main".to_owned(),
                page_id: "list_page".to_owned(),
                projection,
                policy: TaskSelectionPolicy {
                    path: "policy.json".to_owned(),
                    package_sha256: "c".repeat(64),
                    policy_sha256: format!("sha256:{}", "d".repeat(64)),
                    policy_id: "policy-a".to_owned(),
                },
                fact_snapshot_id: "snapshot-a".to_owned(),
                input_ledger_position: 1,
                now_unix_ms: 1_752_147_200_000,
                input_sha256: format!("sha256:{}", "e".repeat(64)),
                selection_result: TaskSelectionOutcome::Selected { count: 1 },
                outcome_key: "selected".to_owned(),
                selected: vec![candidate_id.clone()],
                verdicts: vec![TaskSelectionVerdict {
                    candidate_id,
                    status: TaskSelectionCandidateStatus::Ranked,
                    score_milli: Some(1_000),
                    rank: Some(1),
                    gates: Vec::new(),
                    terms: Vec::new(),
                    reasons: Vec::new(),
                }],
                reasons: vec![PolicyReasonRecord {
                    code: "best_score".to_owned(),
                }],
                confirmation,
            }),
        },
        AuditInput::new(),
    )
    .into()
}

fn revision_3_records() -> Vec<(EventPayloadDraft, EventType, Option<crate::outcome::Code>)> {
    let issuer = identifier_issuer();
    let epoch = *issuer.mint_owner_epoch().expect("epoch").transport();
    let detail = || {
        DiagnosticDetailDraft::new(
            "runtime_connection",
            "runtime.ipc.receipt_write",
            "local_ipc",
            "runtime_local_ipc",
        )
    };
    let failure = Outcome::new(crate::RuntimeErrorCode::BackendOperationFailed)
        .operation(crate::codes::ContractLocation::ControlInstance);
    let package: crate::PackageRef = format!("sha256:{}", "c".repeat(64)).into();
    vec![
        (
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::Indeterminate,
                failure.clone(),
                detail(),
                AuditInput::new(),
            )
            .into(),
            EventType::RuntimeFailed,
            Some(crate::RuntimeErrorCode::BackendOperationFailed.code()),
        ),
        (
            RuntimePayloadDraft::failed_with_lifecycle(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::Indeterminate,
                detail(),
                RuntimeLifecycleFailureDraft::new(epoch, "runtime_host", failure.clone())
                    .with_raw_os_error(Some(5))
                    .with_runtime_code(Some(crate::RuntimeErrorCode::RuntimeFatal)),
                AuditInput::new(),
            )
            .into(),
            EventType::RuntimeFailed,
            Some(crate::RuntimeErrorCode::BackendOperationFailed.code()),
        ),
        (
            InputPayloadDraft::failed_with_causes(
                EventAction::InputTap,
                DiagnosticCode::InputFailed,
                EffectDisposition::NotPerformed,
                failure.clone(),
                Some(detail()),
                Some(CleanupCauseDraft::new(
                    crate::RuntimeErrorCode::BackendOpenFailed,
                    CleanupCauseSeverity::Transient,
                    Some(detail()),
                )),
                AuditInput::new(),
            )
            .into(),
            EventType::InputFailed,
            Some(crate::RuntimeErrorCode::BackendOperationFailed.code()),
        ),
        (
            InputPayloadDraft::failed(
                EventAction::InputTap,
                DiagnosticCode::InputFailed,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            )
            .into(),
            EventType::InputFailed,
            Some(DiagnosticCode::InputFailed.code()),
        ),
        (
            TaskPayloadDraft::semantic(
                TaskSemanticFact::TerminalCommitted {
                    task_result: TaskOutcome::Failure,
                    final_page: None,
                    executed_steps: Some(2),
                    outcome: Some(failure.clone()),
                    // Ruling 9: the failure point's verdict round-trips with the record.
                    failure_severe: true,
                    scheduling_disposition: None,
                    task_timing: None,
                },
                AuditInput::new(),
            )
            .into(),
            EventType::TaskFailed,
            Some(crate::RuntimeErrorCode::BackendOperationFailed.code()),
        ),
        (
            TaskPayloadDraft::semantic(
                TaskSemanticFact::TerminalCommitted {
                    task_result: TaskOutcome::Cancelled,
                    final_page: None,
                    executed_steps: None,
                    outcome: Some(Outcome::new(
                        crate::RuntimeErrorCode::ContainedTaskCancelled,
                    )),
                    failure_severe: false,
                    scheduling_disposition: None,
                    task_timing: None,
                },
                AuditInput::new(),
            )
            .into(),
            EventType::TaskCancelled,
            Some(crate::RuntimeErrorCode::ContainedTaskCancelled.code()),
        ),
        (
            TaskPayloadDraft::semantic(
                TaskSemanticFact::TerminalCommitted {
                    task_result: TaskOutcome::Success,
                    final_page: Some("home".to_owned()),
                    executed_steps: Some(1),
                    outcome: None,
                    failure_severe: false,
                    scheduling_disposition: None,
                    task_timing: None,
                },
                AuditInput::new(),
            )
            .into(),
            EventType::TaskCompleted,
            None,
        ),
        (
            TaskPayloadDraft::semantic(
                TaskSemanticFact::Finalizing {
                    task_result: TaskOutcome::Failure,
                },
                AuditInput::new(),
            )
            .into(),
            EventType::TaskTerminalIntent,
            None,
        ),
        (
            TaskPayloadDraft::semantic(
                TaskSemanticFact::EntryRecoveryFailed {
                    package_sha256: package.clone(),
                    outcome: failure.clone(),
                    role: None,
                },
                AuditInput::new(),
            )
            .into(),
            EventType::TaskEntryPreflight,
            Some(crate::RuntimeErrorCode::BackendOperationFailed.code()),
        ),
        (
            TaskPayloadDraft::semantic(
                TaskSemanticFact::EntryTargetDisposition {
                    disposition: TaskEntryTargetDisposition::FailClosed,
                    outcome: Some(failure.clone()),
                },
                AuditInput::new(),
            )
            .into(),
            EventType::TaskEntryPreflight,
            Some(crate::RuntimeErrorCode::BackendOperationFailed.code()),
        ),
        (
            TaskPayloadDraft::semantic(
                TaskSemanticFact::TerminalRejected {
                    committed_outcome: TaskOutcome::Success,
                    attempted_outcome: TaskOutcome::Failure,
                    outcome: Outcome::new(crate::RuntimeErrorCode::ContainedTaskCancelled),
                },
                AuditInput::new(),
            )
            .into(),
            EventType::TaskTerminalRejected,
            Some(crate::RuntimeErrorCode::ContainedTaskCancelled.code()),
        ),
        (
            RuntimePayloadDraft::lifecycle_observed(
                epoch,
                RuntimeLifecyclePhase::RecoveryRungFinished {
                    rung: RecoveryRung::ReturnHome,
                    rung_result: RecoveryRungOutcome::Failed,
                    run_id: None,
                    outcome: Some(failure.clone()),
                    environment: None,
                },
                AuditInput::new(),
            )
            .into(),
            EventType::RuntimeLifecycleObserved,
            Some(crate::RuntimeErrorCode::BackendOperationFailed.code()),
        ),
        (
            RuntimePayloadDraft::lifecycle_observed(
                epoch,
                RuntimeLifecyclePhase::RecoveryLadderFinished {
                    ladder_result: RecoveryLadderOutcome::Exhausted,
                    rungs_tried: 3,
                    outcome: Some(failure.clone()),
                },
                AuditInput::new(),
            )
            .into(),
            EventType::RuntimeLifecycleObserved,
            Some(crate::RuntimeErrorCode::BackendOperationFailed.code()),
        ),
        (
            ProviderPayloadDraft::observed(ProviderStartupRecord {
                owner_epoch: epoch,
                backend: ProviderBackend::Configured,
                observation: ProviderStartupObservation::Failed {
                    stage: ProviderStartupStage::RegistryBinding,
                    failure: ProviderNativeFailure {
                        module: "provider_registry".to_owned(),
                        outcome: failure.clone().caused_by(
                            LifecycleNativeDetail::new("native registry refusal", false).outcome(),
                        ),
                    },
                },
            })
            .into(),
            EventType::ProviderStartupObserved,
            Some(crate::RuntimeErrorCode::BackendOperationFailed.code()),
        ),
        (
            ProviderPayloadDraft::observed(ProviderStartupRecord {
                owner_epoch: epoch,
                backend: ProviderBackend::Configured,
                observation: ProviderStartupObservation::ModelFolderInvalid {
                    outcome: Outcome::relayed("vision_model_folder_invalid")
                        .path("C:/fixture/vision/models/neutral.partial")
                        .model_name("neutral.partial"),
                },
            })
            .into(),
            EventType::ProviderStartupObserved,
            crate::outcome::Code::relayed("vision_model_folder_invalid"),
        ),
        // Acceptance CL-L6: a revision-3 round trip of every E3 shape that no other test
        // round-trips (the PR body lists where the others are covered).
        (
            RuntimePayloadDraft::lifecycle_observed(
                epoch,
                RuntimeLifecyclePhase::RecoveryRungFinished {
                    rung: RecoveryRung::EmulatorRestart,
                    rung_result: RecoveryRungOutcome::Skipped,
                    run_id: None,
                    outcome: Some(Outcome::relayed(
                        "recovery_rung_skipped_no_emulator_control",
                    )),
                    environment: None,
                },
                AuditInput::new(),
            )
            .into(),
            EventType::RuntimeLifecycleObserved,
            crate::outcome::Code::relayed("recovery_rung_skipped_no_emulator_control"),
        ),
        (
            RuntimePayloadDraft::lifecycle_observed(
                epoch,
                RuntimeLifecyclePhase::InstancePreparationFinished {
                    stage: RecoveryTriggerStage::ConnectionPreparation,
                    capture: crate::BackendObservationStatus::Passed,
                    input: crate::BackendObservationStatus::Failed,
                    outcome: Some(Outcome::new(crate::RuntimeErrorCode::BackendOpenFailed)),
                },
                AuditInput::new(),
            )
            .into(),
            EventType::RuntimeLifecycleObserved,
            Some(crate::RuntimeErrorCode::BackendOpenFailed.code()),
        ),
        (
            revision_3_selection(TaskSelectionConfirmation::GuardFailed {
                outcome: failure.clone(),
            }),
            EventType::TaskSelectionEvaluated,
            Some(crate::RuntimeErrorCode::BackendOperationFailed.code()),
        ),
        (
            revision_3_selection(TaskSelectionConfirmation::CaptureFailed {
                outcome: Outcome::relayed("selection_confirmation_capture_failed"),
            }),
            EventType::TaskSelectionEvaluated,
            crate::outcome::Code::relayed("selection_confirmation_capture_failed"),
        ),
        // A volume whose free space is unknown carries its OS failure as `cause`.
        (
            PerformancePayloadDraft::summary(
                PerformanceSummaryEventData {
                    context: PerformanceContext::unavailable(1_752_147_201_000),
                    foreground: None,
                    owned_processes: Vec::new(),
                    third_party_high_load: Vec::new(),
                    ledger_commits: None,
                    capacity: Some(crate::PerformanceCapacitySample {
                        owner_epoch: epoch,
                        observed_at_unix_ms: 1_752_147_201_000,
                        observed_at_monotonic_ms: 1,
                        freshness_ms: 5_000,
                        thresholds: crate::CapacityThresholds::default(),
                        volumes: vec![crate::CapacityVolumeSample {
                            volume_id: None,
                            purposes: vec![
                                crate::CapacityPurpose::Installation,
                                crate::CapacityPurpose::State,
                                crate::CapacityPurpose::Artifact,
                                crate::CapacityPurpose::ArtifactStaging,
                            ],
                            available_bytes: None,
                            state: crate::CapacityState::Unknown,
                            cause: Some(
                                LifecycleNativeDetail::new("volume query refused", false)
                                    .os_outcome(5)
                                    .operation(crate::codes::ContractLocation::ControlInstance),
                            ),
                        }],
                    }),
                },
                AuditInput::new(),
            )
            .into(),
            EventType::PerformanceSummary,
            None,
        ),
        // An artifact failure with a secondary step: the `secondary` link.
        (
            ArtifactPayloadDraft::persistence_failed(
                ArtifactFailureDraft {
                    artifact_id: *artifact(b"revision-3 failure").reference().artifact_id(),
                    stage: ArtifactFailureStage::BeforePublication,
                    primary: ArtifactFailureCause {
                        step: Outcome::new(crate::codes::SharedCode::ArtifactWriteFailed)
                            .operation(crate::codes::ContractLocation::ExportEvidence),
                        native_detail: LifecycleNativeDetail::new("primary native text", false),
                        raw_os_error: Some(5),
                    },
                    secondary: vec![ArtifactFailureCause {
                        step: Outcome::relayed("artifact_cleanup_failed"),
                        native_detail: LifecycleNativeDetail::new("cleanup native text", false),
                        raw_os_error: None,
                    }],
                    omitted_secondary_count: 0,
                    capacity: None,
                },
                AuditInput::new(),
            )
            .into(),
            EventType::ArtifactStoreFailed,
            Some(crate::codes::SharedCode::ArtifactWriteFailed.code()),
        ),
        // The `config.subsystems` row (register E8): a runtime fact, its code a member.
        (
            RuntimePayloadDraft::fact_recorded(
                {
                    let [subsystems, _] = crate::RuntimeConfigManifest {
                        subsystems: vec![crate::ConfigSubsystem {
                            name: "frame_retention".to_owned(),
                            enabled: true,
                            outcome: Outcome::relayed("config_subsystem_configured"),
                        }],
                        parameters: Vec::new(),
                    }
                    .to_fact_records(1_752_147_200_000, OriginModule::Runtime);
                    subsystems
                },
                AuditInput::new(),
            )
            .into(),
            EventType::RuntimeFactRecorded,
            None,
        ),
        (
            SchedulerPayloadDraft::denied(
                EventAction::RuntimeAction,
                DiagnosticCode::LeaseBusy,
                AuditInput::new(),
            )
            .into(),
            EventType::SchedulerDenied,
            Some(DiagnosticCode::LeaseBusy.code()),
        ),
    ]
}

/// Workflow #378 A3 (acceptance CL-L6): a geometry recheck's two failures
/// (`TaskGeometryFailure`) round-trip with their outcomes. A geometry record needs the Runtime's
/// origin and a frame link only when it names a frame, so it is not one of
/// [`revision_3_records`]; its failures are not the record's outcome.
#[test]
fn revision_3_geometry_failures_round_trip() {
    let issuer = identifier_issuer();
    let failure = Outcome::new(crate::RuntimeErrorCode::BackendOperationFailed)
        .operation(crate::codes::ContractLocation::ControlInstance);
    let sanitized = EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        1_752_147_200_001,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default()
            .with_instance_id(issuer.mint_instance_id().expect("instance id"))
            .with_request_id(issuer.mint_request_id().expect("request id"))
            .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"))
            .with_task_id(issuer.mint_task_id().expect("task id"))
            .with_run_id(issuer.mint_run_id().expect("run id")),
        TaskPayloadDraft::semantic(
            TaskSemanticFact::GeometryObserved {
                observation: Box::new(TaskGeometryObservation {
                    phase: TaskGeometryPhase::Recheck,
                    frame: None,
                    observation: None,
                    conclusion: TaskGeometryConclusion::Unavailable,
                    trigger: Some(TaskGeometryRecheckTrigger::RecognitionFailed),
                    original_failure: Some(TaskGeometryFailure {
                        outcome: Outcome::relayed("recognition_failed"),
                        event_id: None,
                    }),
                    unavailable: Some(TaskGeometryFailure {
                        outcome: failure.clone(),
                        event_id: None,
                    }),
                }),
            },
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&SpyFingerprinter::new())
    .expect("sanitize the geometry record");
    let json = serde_json::to_string(sanitized.payload()).expect("record JSON");
    assert_outcome_members_are_envelopes(&json);
    let round_trip: EventPayload = serde_json::from_str(&json).expect("record round trip");
    round_trip
        .validate()
        .expect("round-tripped record validates");
    assert_eq!(round_trip, *sanitized.payload());
    let EventPayload::Task(TaskPayload::Semantic(semantic)) = &round_trip else {
        panic!("task semantic record")
    };
    let TaskSemanticFact::GeometryObserved { observation } = semantic.fact() else {
        panic!("geometry record")
    };
    assert_eq!(
        observation
            .original_failure
            .as_ref()
            .map(|failure| failure.outcome.code().as_str()),
        Some("recognition_failed")
    );
    assert_eq!(
        observation
            .unavailable
            .as_ref()
            .map(|failure| &failure.outcome),
        Some(&failure)
    );
}

#[test]
fn revision_3_records_carry_their_outcome_and_round_trip() {
    for (index, (draft, event_type, code)) in revision_3_records().into_iter().enumerate() {
        let sanitized = sanitize(draft, index as u64 + 1);
        assert_eq!(sanitized.schema_version(), "actingcommand.event.v3");
        assert_eq!(sanitized.event_type(), event_type);
        assert_eq!(
            sanitized
                .payload()
                .outcome()
                .map(|outcome| outcome.code().clone()),
            code.map(CodeStr::from),
            "{event_type:?}"
        );
        let json = serde_json::to_string(sanitized.payload()).expect("record JSON");
        assert_outcome_members_are_envelopes(&json);
        for member in REVISION_2_MEMBERS {
            assert!(!json.contains(member), "{event_type:?} keeps {member}");
        }
        let round_trip: EventPayload = serde_json::from_str(&json).expect("record round trip");
        round_trip
            .validate()
            .expect("round-tripped record validates");
        assert_eq!(round_trip, *sanitized.payload());
        let public = serde_json::to_string(&round_trip.public_projection()).expect("public");
        // Codes mode drops the foreign text but keeps the vocabulary token
        // `raw_text_sensitivity` (model section 4.3), so the member itself is checked.
        assert!(
            !public.contains("\"raw_text\""),
            "{event_type:?} public raw text"
        );
    }
}

#[test]
fn revision_3_lifecycle_failure_links_its_os_error_and_keeps_the_runtime_code() {
    let (draft, _, _) = revision_3_records().swap_remove(1);
    let sanitized = sanitize(draft, 1);
    let EventPayload::Runtime(RuntimePayload::Failed(failed)) = sanitized.payload() else {
        panic!("runtime failure")
    };
    assert_eq!(
        failed
            .lifecycle_failure()
            .map(RuntimeLifecycleFailureRecord::source),
        Some("runtime_host")
    );
    let outcome = failed.outcome();
    assert_eq!(
        outcome.value("operation"),
        Some(&serde_json::json!("control_instance"))
    );
    // Acceptance CL-M3: the receipt's Runtime code is the outcome's value `runtime_code`.
    assert_eq!(
        outcome.value("runtime_code"),
        Some(&serde_json::json!("runtime_fatal"))
    );
    let [os_error] = outcome.causes() else {
        panic!("the OS error link")
    };
    assert_eq!(os_error.relation().as_str(), "caused_by");
    assert_eq!(
        os_error.outcome().code(),
        &ContractCode::ForeignOsError.code()
    );
    assert_eq!(
        os_error.outcome().value("os_error"),
        Some(&serde_json::json!(5))
    );
    assert_eq!(
        os_error.outcome().value("raw_source"),
        Some(&serde_json::json!("os"))
    );
}

#[test]
fn revision_3_renamed_members_keep_their_results_beside_the_envelope() {
    let issuer = identifier_issuer();
    let epoch = *issuer.mint_owner_epoch().expect("epoch").transport();
    let cases: [(EventPayloadDraft, &str); 4] = [
        (
            TaskPayloadDraft::semantic(
                TaskSemanticFact::Finalizing {
                    task_result: TaskOutcome::Success,
                },
                AuditInput::new(),
            )
            .into(),
            "\"task_result\":\"success\"",
        ),
        (
            RuntimePayloadDraft::lifecycle_observed(
                epoch,
                RuntimeLifecyclePhase::RecoveryLadderFinished {
                    ladder_result: RecoveryLadderOutcome::Recovered,
                    rungs_tried: 1,
                    outcome: None,
                },
                AuditInput::new(),
            )
            .into(),
            "\"ladder_result\":\"recovered\"",
        ),
        (
            RuntimePayloadDraft::lifecycle_observed(
                epoch,
                RuntimeLifecyclePhase::RecoveryRungFinished {
                    rung: RecoveryRung::ReturnHome,
                    rung_result: RecoveryRungOutcome::Skipped,
                    run_id: None,
                    outcome: Some(Outcome::new(crate::RuntimeErrorCode::BackendOpenFailed)),
                    environment: None,
                },
                AuditInput::new(),
            )
            .into(),
            "\"rung_result\":\"skipped\"",
        ),
        (
            TaskPayloadDraft::semantic(
                TaskSemanticFact::TerminalCommitted {
                    task_result: TaskOutcome::Failure,
                    final_page: None,
                    executed_steps: None,
                    outcome: Some(Outcome::new(
                        crate::RuntimeErrorCode::BackendOperationFailed,
                    )),
                    failure_severe: false,
                    scheduling_disposition: None,
                    task_timing: None,
                },
                AuditInput::new(),
            )
            .into(),
            "\"task_result\":\"failure\"",
        ),
    ];
    for (index, (draft, member)) in cases.into_iter().enumerate() {
        let sanitized = sanitize(draft, index as u64 + 1);
        let json = serde_json::to_string(sanitized.payload()).expect("record JSON");
        assert!(json.contains(member), "missing {member}");
        assert_outcome_members_are_envelopes(&json);
    }
}

/// Workflow #378 A3 (acceptance CL-L3): every event type resolves its category from the
/// embedded catalog, with no fallback. The tokens are the catalog's `event_type` vocabulary,
/// which the guard (G7) keeps equal to the `EventType` enum, so this covers every variant.
#[test]
fn every_event_type_resolves_its_catalog_category() {
    let tokens = crate::outcome::catalog_json()
        .expect("embedded catalog")
        .pointer("/vocabularies/event_type/tokens")
        .and_then(serde_json::Value::as_object)
        .expect("event_type vocabulary");
    assert!(!tokens.is_empty());
    for (token, entry) in tokens {
        let event_type: EventType = serde_json::from_value(serde_json::json!(token))
            .unwrap_or_else(|error| panic!("{token} is no EventType: {error}"));
        assert_eq!(
            serde_json::to_value(event_type).expect("event type token"),
            serde_json::json!(token)
        );
        let category: crate::outcome::Category =
            serde_json::from_value(entry.get("category").cloned().expect("a category"))
                .unwrap_or_else(|error| panic!("{token}: {error}"));
        assert_eq!(event_type.category(), category, "{token}");
    }
}

#[test]
fn record_severity_is_the_outcome_category_else_the_event_type_category() {
    // The categories the catalog gives these codes and event types (vocabulary `event_type`):
    // task.completed is success, task.terminal_intent info, lease.expired warning,
    // backend_operation_failed and input_failed error.
    let expected = [
        (
            TaskPayloadDraft::semantic(
                TaskSemanticFact::TerminalCommitted {
                    task_result: TaskOutcome::Success,
                    final_page: Some("home".to_owned()),
                    executed_steps: Some(1),
                    outcome: None,
                    failure_severe: false,
                    scheduling_disposition: None,
                    task_timing: None,
                },
                AuditInput::new(),
            )
            .into(),
            EventSeverity::Success,
        ),
        (
            TaskPayloadDraft::semantic(
                TaskSemanticFact::Finalizing {
                    task_result: TaskOutcome::Failure,
                },
                AuditInput::new(),
            )
            .into(),
            EventSeverity::Info,
        ),
        (
            LeasePayloadDraft::expired(
                EventAction::RuntimeAction,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
            EventSeverity::Warning,
        ),
        (
            InputPayloadDraft::failed(
                EventAction::InputTap,
                DiagnosticCode::InputFailed,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            )
            .into(),
            EventSeverity::Error,
        ),
    ];
    for (index, (draft, severity)) in expected.into_iter().enumerate() {
        // The producer asks for `fatal`; only the category decides.
        let issuer = identifier_issuer();
        let sanitized = EventDraft::new(
            issuer.mint_event_id().expect("event id"),
            1_752_147_200_000 + index as u64,
            EventSeverity::Fatal,
            origin(),
            links(&issuer),
            draft,
        )
        .sanitize(&SpyFingerprinter::new())
        .expect("sanitize record");
        assert_eq!(sanitized.severity(), severity);
    }

    for (index, (draft, _, _)) in revision_3_records().into_iter().enumerate() {
        let sanitized = sanitize(draft, index as u64 + 1);
        let category = sanitized.payload().outcome().map_or_else(
            || sanitized.event_type().category(),
            crate::outcome::Outcome::category,
        );
        assert_eq!(sanitized.severity(), EventSeverity::from_category(category));
    }

    let issuer = identifier_issuer();
    let debug = EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        1_752_147_200_000,
        EventSeverity::Debug,
        origin(),
        links(&issuer),
        TaskPayloadDraft::semantic(TaskSemanticFact::RunStarted, AuditInput::new()).into(),
    )
    .sanitize(&SpyFingerprinter::new())
    .expect("debug record");
    assert_eq!(debug.severity(), EventSeverity::Debug);
}

/// Ledger revision 3 (#378 A3): a recovery ladder's end carries why exactly when the climb ended
/// exhausted, and that outcome's category is the record's severity, so an exhausted end keeps
/// review L6's error or warning (#666).
#[test]
fn revision_3_ladder_end_carries_an_outcome_exactly_when_exhausted() {
    let issuer = identifier_issuer();
    let epoch = *issuer.mint_owner_epoch().expect("epoch").transport();
    let end = |ladder_result, outcome| -> EventPayloadDraft {
        RuntimePayloadDraft::lifecycle_observed(
            epoch,
            RuntimeLifecyclePhase::RecoveryLadderFinished {
                ladder_result,
                rungs_tried: 2,
                outcome,
            },
            AuditInput::new(),
        )
        .into()
    };
    let error = Outcome::new(crate::RuntimeErrorCode::BackendOperationFailed)
        .operation(crate::codes::ContractLocation::ControlInstance);
    let warning = Outcome::new(ContractCode::PanicCaught)
        .thread("worker")
        .boundary(crate::codes::ContractLocation::ControlInstance)
        .raw_source(crate::outcome::RawSource::Panic)
        .raw_text("panicked");
    for (index, (outcome, severity)) in [
        (error.clone(), EventSeverity::Error),
        (warning, EventSeverity::Warning),
    ]
    .into_iter()
    .enumerate()
    {
        let code = outcome.code().clone();
        let exhausted = sanitize(
            end(RecoveryLadderOutcome::Exhausted, Some(outcome)),
            index as u64 + 1,
        );
        assert_eq!(exhausted.severity(), severity);
        assert_eq!(
            exhausted.payload().outcome().map(Outcome::code),
            Some(&code)
        );
    }
    assert_eq!(
        sanitize(end(RecoveryLadderOutcome::Recovered, None), 3).severity(),
        EventSeverity::Info
    );
    sanitize_error(end(RecoveryLadderOutcome::Exhausted, None));
    sanitize_error(end(RecoveryLadderOutcome::Recovered, Some(error)));
}

/// Ledger revision 3 (#378 A3), the A2-R bridge: a relayed spelling becomes its registered code
/// with the catalog's category and carries no values, so a code's required values are not
/// demanded of a relay; an unregistered spelling stays visible as `outcome_relay_unregistered`
/// with the spelling as `raw_code` when it is a token.
#[test]
fn revision_3_relayed_spellings_resolve_through_the_catalog() {
    let relayed = Outcome::relayed("panic_caught");
    assert_eq!(
        relayed.code(),
        &CodeStr::from(ContractCode::PanicCaught.code())
    );
    assert_eq!(
        relayed.category(),
        ContractCode::PanicCaught.code().category()
    );
    // `panic_caught` requires thread, boundary, raw_source and raw_text; the relay has none.
    let json = serde_json::to_value(&relayed).expect("a relayed outcome serialises");
    assert_eq!(json["code"], "panic_caught");

    let unregistered = Outcome::relayed("no_such_code_anywhere");
    assert_eq!(
        unregistered.code(),
        &CodeStr::from(ContractCode::OutcomeRelayUnregistered.code())
    );
    assert_eq!(
        unregistered
            .value("raw_code")
            .and_then(serde_json::Value::as_str),
        Some("no_such_code_anywhere")
    );
    let prose = Outcome::relayed("Not a token");
    assert_eq!(
        prose.code(),
        &CodeStr::from(ContractCode::OutcomeRelayUnregistered.code())
    );
    assert!(prose.value("raw_code").is_none());
}

#[test]
fn every_payload_variant_keeps_outcome_for_the_envelope_only() {
    for (index, payload) in all_payload_drafts(AuditInput::new).into_iter().enumerate() {
        let sanitized = sanitize(payload, index as u64 + 1);
        let json = serde_json::to_string(sanitized.payload()).expect("serialize typed payload");
        assert_outcome_members_are_envelopes(&json);
        for member in REVISION_2_TEXT_MEMBERS {
            assert!(
                !json.contains(member),
                "{:?} keeps {member}",
                sanitized.event_type()
            );
        }
    }
}
