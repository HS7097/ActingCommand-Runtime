use super::*;
use crate::{
    FactContent, FactInvalidationEventData, FactRecord, FactScope, FactValue, InputAction,
    MonitorDecision, MonitorDiagnosis, MonitorDisposition, MonitorObservation,
    MonitorRecoveryCoordinationReason, MonitorRecoveryKind, PerformanceContext,
    PerformanceControlEventData, PerformanceControlLevel, PerformanceControlReason,
    PerformanceDeadlineDisposition, PerformanceMetric, PerformanceMonitorHealth,
    PerformanceMonitorStateEventData, PerformancePressureEventData, PerformancePressureKind,
    PerformancePressureRecord, PerformancePressureSeverity, PerformancePressureValue,
    PerformanceStutterEventData, PerformanceSummaryEventData,
};
use std::sync::Mutex;

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
        reason_chain_id: "reason:fixture-a".to_owned(),
        reasons: vec![PolicyReasonRecord {
            code: "eligible".to_owned(),
            detail: "neutral fixture is eligible".to_owned(),
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
    };
    let catalog_data = CatalogTransitionEventData {
        catalog_id: "catalog:fixture-a".to_owned(),
        catalog_version: 2,
        catalog_hash: format!("sha256:{}", "c".repeat(64)),
        previous_catalog_hash: Some(format!("sha256:{}", "b".repeat(64))),
    };
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
            },
            input(),
        )
        .into(),
        PerformancePayloadDraft::monitor_degraded(
            PerformanceMonitorStateEventData {
                observed_at_unix_ms: 1_752_147_201_000,
                health: PerformanceMonitorHealth::Degraded,
                failure_code: Some("performance_counter_failed".to_owned()),
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
                failure_code: None,
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
            },
            input(),
        )
        .into(),
        FactPayloadDraft::published(fact_record, input()).into(),
        FactPayloadDraft::invalidated(fact_invalidation, input()).into(),
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
        PolicyPayloadDraft::dispatch_rejected(
            policy_data.clone(),
            EffectDisposition::NotPerformed,
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
            Some("authoring_validation_failed".to_string()),
            input(),
        )
        .into(),
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
fn all_runtime_secret_classes_follow_schema_owned_policy_independently() {
    type AuditBuilder = fn(&str) -> AuditInput;
    let cases: [(&str, AuditBuilder); 4] = [
        ("account-secret-c1@example.invalid", |value| {
            AuditInput::new().with_account(value)
        }),
        ("authentication-secret-c1", |value| {
            AuditInput::new().with_authentication(value)
        }),
        (r"C:\Users\Alice\private\runtime.json", |value| {
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
        assert_eq!(error.code(), "invalid_fingerprint");
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
        "runtime.diagnostic"
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
    assert_eq!(payloads.len(), 78);

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
fn policy_failure_payload_rejects_impossible_retry_combinations() {
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
        sanitize_error(event(severe_retry)).code(),
        "invalid_policy_failure_record"
    );

    let mut hidden_retry_attempt = base.clone();
    hidden_retry_attempt.disposition = PolicyFailureDisposition::Continue;
    hidden_retry_attempt.retry_at_unix_ms = None;
    assert_eq!(
        sanitize_error(event(hidden_retry_attempt)).code(),
        "invalid_policy_failure_record"
    );

    let mut unsupported_exemption = base.clone();
    unsupported_exemption.performance_tax_exempt = true;
    unsupported_exemption.escalation_streak = 0;
    assert_eq!(
        sanitize_error(event(unsupported_exemption)).code(),
        "invalid_policy_failure_record"
    );

    let mut past_retry = base;
    past_retry.retry_at_unix_ms = Some(1_752_147_201_000);
    assert_eq!(
        sanitize_error(event(past_retry)).code(),
        "invalid_policy_failure_record"
    );
}

#[test]
fn performance_payload_rejects_fake_health_and_invalid_stutter() {
    let fake_health: EventPayloadDraft = PerformancePayloadDraft::monitor_degraded(
        PerformanceMonitorStateEventData {
            observed_at_unix_ms: 1_752_147_201_000,
            health: PerformanceMonitorHealth::Degraded,
            failure_code: None,
            consecutive_failures: 1,
            terminal: false,
            unavailable_metrics: vec![PerformanceMetric::Gpu],
        },
        AuditInput::new(),
    )
    .into();
    assert_eq!(
        sanitize_error(fake_health).code(),
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
        sanitize_error(invalid_stutter).code(),
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
        },
        AuditInput::new(),
    )
    .into();
    assert_eq!(
        sanitize_error(invalid_control).code(),
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
                    step_index: index as u32,
                    operation_label: "input".to_string(),
                    action,
                },
                AuditInput::new(),
            )
            .into(),
            index as u64 + 1,
        );
        let durable = serde_json::to_string(event.payload()).expect("semantic payload JSON");

        assert!(!durable.contains("authentication-secret-task"));
        assert!(durable.contains("[redacted]"));
    }
}

#[test]
fn task_semantic_payload_rejects_invalid_facts() {
    let invalid = [
        TaskSemanticFact::PackageAdmitted {
            package_label: "package".to_string(),
            task_label: "task".to_string(),
            package_sha256: "not-a-sha256".to_string(),
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
        },
        TaskSemanticFact::TerminalCommitted {
            outcome: TaskOutcome::Success,
            final_page: Some("home".to_string()),
            executed_steps: 1,
            failure_code: Some("must_be_absent".to_string()),
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
            Some("authoring_validation_failed".to_string()),
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
            Some("unexpected_failure".to_string()),
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
            error.code().starts_with("invalid_")
                || error.code().starts_with("missing_")
                || error.code().starts_with("unexpected_")
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
        assert!(error.code().starts_with("invalid_"));
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
    assert_eq!(error.code(), "invalid_scheduler_queue");

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
    assert_eq!(error.code(), "invalid_lease_transfer");
}
