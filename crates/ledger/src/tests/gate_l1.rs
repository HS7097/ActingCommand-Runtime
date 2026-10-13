// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan v2.2, L-1 G7 (inventory-ledger key 3): the durability class that
//! `contracts/ledger-store.md` ("Durability classes") gives each item it names, fixed by the
//! Ledger from the payload alone. Every payload is built with the contract's public draft
//! constructors and sanitized like an append.

use crate::Sha256SecretFingerprinter;
use crate::durability::commit_sync;
use actingcommand_artifact_store::{ArtifactStore, ArtifactWriteContext, ArtifactWriteRequest};
use actingcommand_contract::{
    AdbRecoveryPath, AdbRecoveryPhase, AdbRecoveryStep, AdbTargetRecovery, AdbTransportState,
    ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactPayloadDraft, ArtifactPinReason,
    ArtifactPinRecord, ArtifactProducer, ArtifactRedactionState, ArtifactRetentionFact,
    ArtifactRetentionIdentity, AuditInput, BackendOpenEntry, BackendOpenReport, CandidateFrame,
    CandidateLayoutKind, CandidateProjection, CapturePayloadDraft, CapturePolicyReason,
    CapturePressureState, DeviceSelfCheck, DeviceSelfCheckEntry, DeviceSelfCheckStatus,
    DiagnosticCode, EffectDisposition, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventPayloadDraft, EventSeverity, EventSource, EventType,
    FRAME_RETENTION_POLICY_VERSION, FactValue, IdentifierIssuer, LifecycleNativeDetail,
    MonitorDecision, MonitorDiagnosis, MonitorDisposition, MonitorObservation, MonitorPayloadDraft,
    OriginModule, OwnerEpoch, PerformanceContext, PerformanceControlEventData,
    PerformanceControlLevel, PerformanceControlReason, PerformanceMetric, PerformanceMonitorHealth,
    PerformanceMonitorStateEventData, PerformancePayloadDraft, PerformancePressureEventData,
    PerformancePressureKind, PerformancePressureRecord, PerformancePressureSeverity,
    PerformancePressureValue, PerformanceStutterEventData, PerformanceSummaryEventData,
    PolicyReasonRecord, ProviderBackend, ProviderPayloadDraft, ProviderStartupObservation,
    ProviderStartupRecord, RecognitionPayloadDraft, RecognitionVerdict, RetentionClass,
    RuntimeFactInvalidation, RuntimeFactInvalidationReason, RuntimeFactRecord, RuntimeFactScope,
    RuntimePayloadDraft, SanitizedEventDraft, StdioFact, TaskGeometryConclusion,
    TaskGeometryFailure, TaskGeometryObservation, TaskGeometryPhase, TaskGeometryRecheckTrigger,
    TaskPayloadDraft, TaskSelectionConfirmation, TaskSelectionOutcome, TaskSelectionPolicy,
    TaskSelectionRecord, TaskSemanticFact, TerminalEvent, VendorStdioFacts,
};
use actingcommand_runtime_database::CommitSync;
use tempfile::TempDir;

/// TCLOCK: every timestamp is a fixed constant.
const AT_MS: u64 = 1_752_147_200_000;

/// One item of the contract's table: its name there, a payload, its event type and its class.
type Row = (&'static str, EventPayloadDraft, EventType, CommitSync);

fn sanitized(
    ids: &IdentifierIssuer,
    item: &str,
    payload: EventPayloadDraft,
) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        AT_MS,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default()
            .with_instance_id(ids.mint_instance_id().expect("instance id"))
            .with_request_id(ids.mint_request_id().expect("request id"))
            .with_correlation_id(ids.mint_correlation_id().expect("correlation id"))
            .with_task_id(ids.mint_task_id().expect("task id"))
            .with_run_id(ids.mint_run_id().expect("run id")),
        payload,
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"gate-l1-durability").expect("fingerprinter"))
    .unwrap_or_else(|error| panic!("{item}: the draft does not sanitize: {error:?}"))
}

fn pressure() -> PerformancePressureRecord {
    PerformancePressureRecord {
        kind: PerformancePressureKind::Cpu,
        severity: PerformancePressureSeverity::High,
        started_at_unix_ms: AT_MS,
        last_observed_at_unix_ms: AT_MS + 1_000,
        peak: PerformancePressureValue::Utilization {
            basis_points: 9_500,
        },
    }
}

fn summary() -> PerformanceSummaryEventData {
    PerformanceSummaryEventData {
        context: PerformanceContext {
            window_start_unix_ms: AT_MS - 29_000,
            window_end_unix_ms: AT_MS + 1_000,
            health: PerformanceMonitorHealth::Healthy,
            sample_count: 2,
            unavailable_metrics: Vec::new(),
            pressures: vec![pressure()],
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
        },
        foreground: None,
        owned_processes: Vec::new(),
        third_party_high_load: Vec::new(),
        ledger_commits: None,
        capacity: None,
    }
}

fn geometry_unavailable() -> TaskSemanticFact {
    TaskSemanticFact::GeometryObserved {
        observation: Box::new(TaskGeometryObservation {
            phase: TaskGeometryPhase::Recheck,
            frame: None,
            observation: None,
            conclusion: TaskGeometryConclusion::Unavailable,
            trigger: Some(TaskGeometryRecheckTrigger::RecognitionFailed),
            original_failure: Some(TaskGeometryFailure {
                outcome: actingcommand_contract::outcome::Outcome::relayed("recognition_failed"),
                event_id: None,
            }),
            unavailable: Some(TaskGeometryFailure {
                outcome: actingcommand_contract::outcome::Outcome::relayed("capture_failed"),
                event_id: None,
            }),
        }),
    }
}

fn selection_evaluated() -> TaskSemanticFact {
    let projection = CandidateProjection::new(
        "page-a",
        "layout-a",
        CandidateLayoutKind::FixedSlots,
        CandidateFrame {
            width: 1280,
            height: 720,
        },
        Vec::new(),
    )
    .expect("candidate projection");
    TaskSemanticFact::SelectionEvaluated {
        step_index: 1,
        operation_label: "select".to_owned(),
        selection: Box::new(TaskSelectionRecord {
            layout_id: "layout-a".to_owned(),
            page_id: "page-a".to_owned(),
            projection,
            policy: TaskSelectionPolicy {
                path: "policy.json".to_owned(),
                package_sha256: "c".repeat(64),
                policy_sha256: format!("sha256:{}", "d".repeat(64)),
                policy_id: "policy-a".to_owned(),
            },
            fact_snapshot_id: "snapshot-a".to_owned(),
            input_ledger_position: 1,
            now_unix_ms: AT_MS,
            input_sha256: format!("sha256:{}", "e".repeat(64)),
            selection_result: TaskSelectionOutcome::Empty,
            outcome_key: "empty".to_owned(),
            selected: Vec::new(),
            verdicts: Vec::new(),
            reasons: vec![PolicyReasonRecord(
                actingcommand_contract::outcome::Outcome::relayed("selection_empty"),
            )],
            confirmation: TaskSelectionConfirmation::NotAttempted,
        }),
    }
}

/// A frame pin of a capture frame whose reference the ArtifactStore reserved; nothing is
/// published.
fn pin_recorded(ids: &IdentifierIssuer, owner: OwnerEpoch) -> EventPayloadDraft {
    let root = TempDir::new().expect("artifact root");
    let store = ArtifactStore::open(root.path()).expect("artifact store");
    let correlation = ids.mint_correlation_id().expect("correlation id");
    let prepared = store
        .prepare(ArtifactWriteRequest::new(
            ArtifactKind::CaptureFrame,
            b"gate-l1-frame",
            ArtifactWriteContext::new(
                ArtifactLinksDraft::default()
                    .with_frame_id(ids.mint_frame_id().expect("frame id"))
                    .with_correlation_id(correlation),
                EventLinksDraft::default(),
                AT_MS,
            ),
            ArtifactIssuePolicy::new(
                ArtifactProducer::CaptureStore,
                RetentionClass::DebugFull,
                ArtifactRedactionState::Pending,
            ),
        ))
        .expect("reserved frame reference");
    ArtifactPayloadDraft::retention(
        ArtifactRetentionFact::PinRecorded(ArtifactPinRecord {
            identity: ArtifactRetentionIdentity {
                artifact: prepared.reference().project(true),
                owner_epoch: owner,
                instance_id: *ids.mint_instance_id().expect("instance id").transport(),
                request_id: *ids.mint_request_id().expect("request id").transport(),
                correlation_id: *correlation.transport(),
                run_id: None,
                lease_id: None,
                policy_version: FRAME_RETENTION_POLICY_VERSION,
            },
            reason: ArtifactPinReason::Explicit,
            trigger: TerminalEvent {
                sequence: 1,
                event_id: *ids.mint_event_id().expect("trigger id").transport(),
            },
        }),
        AuditInput::new(),
    )
    .into()
}

fn fact_recorded(scope: RuntimeFactScope, key: &str, value: FactValue) -> EventPayloadDraft {
    RuntimePayloadDraft::fact_recorded(
        RuntimeFactRecord {
            scope,
            key: key.to_owned(),
            value,
            observed_at_unix_ms: AT_MS,
            source: OriginModule::Runtime,
            ttl_ms: None,
        },
        AuditInput::new(),
    )
    .into()
}

fn fact_invalidated(
    scope: RuntimeFactScope,
    key: &str,
    reason: RuntimeFactInvalidationReason,
) -> EventPayloadDraft {
    RuntimePayloadDraft::fact_invalidated(
        RuntimeFactInvalidation {
            scope,
            key: key.to_owned(),
            reason,
            at_unix_ms: AT_MS,
        },
        AuditInput::new(),
    )
    .into()
}

/// `ledger-store.md` "Observational:": each item the list names.
fn observational(ids: &IdentifierIssuer, owner: OwnerEpoch) -> Vec<Row> {
    use CommitSync::Normal;
    let instance = || RuntimeFactScope::Instance {
        instance_id: *ids.mint_instance_id().expect("instance id").transport(),
    };
    let mut backend_open = BackendOpenReport::unobserved(BackendOpenEntry::Capture);
    backend_open.session_generation = 1;
    vec![
        (
            "provider.startup_observed",
            ProviderPayloadDraft::observed(ProviderStartupRecord {
                owner_epoch: owner,
                backend: ProviderBackend::Configured,
                observation: ProviderStartupObservation::Ready,
            })
            .into(),
            EventType::ProviderStartupObserved,
            Normal,
        ),
        (
            "monitor.probe_requested",
            MonitorPayloadDraft::requested(AuditInput::new()).into(),
            EventType::MonitorProbeRequested,
            Normal,
        ),
        (
            "monitor.probe_started",
            MonitorPayloadDraft::started(AuditInput::new()).into(),
            EventType::MonitorProbeStarted,
            Normal,
        ),
        (
            "monitor.probe_completed",
            MonitorPayloadDraft::completed(
                EffectDisposition::Performed,
                MonitorObservation::new(MonitorDiagnosis::Healthy, "home", Some("home".to_owned()))
                    .expect("monitor observation"),
                MonitorDecision::new(MonitorDiagnosis::Healthy, MonitorDisposition::Healthy, None)
                    .expect("monitor decision"),
                AuditInput::new(),
            )
            .into(),
            EventType::MonitorProbeCompleted,
            Normal,
        ),
        (
            "monitor.probe_failed",
            MonitorPayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
            EventType::MonitorProbeFailed,
            Normal,
        ),
        (
            "perf.pressure_started",
            PerformancePayloadDraft::pressure_started(
                PerformancePressureEventData {
                    observed_at_unix_ms: AT_MS,
                    pressure: PerformancePressureRecord {
                        last_observed_at_unix_ms: AT_MS,
                        ..pressure()
                    },
                },
                AuditInput::new(),
            )
            .into(),
            EventType::PerformancePressureStarted,
            Normal,
        ),
        (
            "perf.pressure_ended",
            PerformancePayloadDraft::pressure_ended(
                PerformancePressureEventData {
                    observed_at_unix_ms: AT_MS + 1_000,
                    pressure: pressure(),
                },
                AuditInput::new(),
            )
            .into(),
            EventType::PerformancePressureEnded,
            Normal,
        ),
        (
            "perf.stutter_detected",
            PerformancePayloadDraft::stutter_detected(
                PerformanceStutterEventData {
                    instance_id: "instance-a".to_owned(),
                    observed_at_unix_ms: AT_MS,
                    frame_gap_ms: 1_500,
                    capture_latency_ms: Some(120),
                    recognition_latency_ms: Some(80),
                    action_effect_latency_ms: Some(250),
                },
                AuditInput::new(),
            )
            .into(),
            EventType::PerformanceStutterDetected,
            Normal,
        ),
        (
            "perf.summary",
            PerformancePayloadDraft::summary(summary(), AuditInput::new()).into(),
            EventType::PerformanceSummary,
            Normal,
        ),
        (
            "perf.monitor_degraded",
            PerformancePayloadDraft::monitor_degraded(
                PerformanceMonitorStateEventData {
                    observed_at_unix_ms: AT_MS,
                    health: PerformanceMonitorHealth::Degraded,
                    outcome: Some(actingcommand_contract::outcome::Outcome::relayed(
                        "performance_pdh_open_failed",
                    )),
                    consecutive_failures: 1,
                    terminal: false,
                    unavailable_metrics: vec![PerformanceMetric::Gpu],
                },
                AuditInput::new(),
            )
            .into(),
            EventType::PerformanceMonitorDegraded,
            Normal,
        ),
        (
            "perf.monitor_recovered",
            PerformancePayloadDraft::monitor_recovered(
                PerformanceMonitorStateEventData {
                    observed_at_unix_ms: AT_MS + 1_000,
                    health: PerformanceMonitorHealth::Healthy,
                    outcome: None,
                    consecutive_failures: 0,
                    terminal: false,
                    unavailable_metrics: Vec::new(),
                },
                AuditInput::new(),
            )
            .into(),
            EventType::PerformanceMonitorRecovered,
            Normal,
        ),
        (
            "task.evidence_indexed",
            TaskPayloadDraft::semantic(
                TaskSemanticFact::EvidenceIndexed {
                    frame_width: 1280,
                    frame_height: 720,
                },
                AuditInput::new(),
            )
            .into(),
            EventType::TaskEvidenceIndexed,
            Normal,
        ),
        (
            "task.geometry_observed",
            TaskPayloadDraft::semantic(geometry_unavailable(), AuditInput::new()).into(),
            EventType::TaskGeometryObserved,
            Normal,
        ),
        (
            "task.recognition_started",
            TaskPayloadDraft::semantic(
                TaskSemanticFact::RecognitionStarted {
                    candidate_pages: vec!["page-a".to_owned()],
                    frame_width: 1280,
                    frame_height: 720,
                },
                AuditInput::new(),
            )
            .into(),
            EventType::TaskRecognitionStarted,
            Normal,
        ),
        (
            "task.recognition_completed",
            TaskPayloadDraft::semantic(
                TaskSemanticFact::RecognitionCompleted {
                    candidate_pages: vec!["page-a".to_owned()],
                    matched_page: Some("page-a".to_owned()),
                    frame_width: 1280,
                    frame_height: 720,
                    targets: Vec::new(),
                },
                AuditInput::new(),
            )
            .into(),
            EventType::TaskRecognitionCompleted,
            Normal,
        ),
        (
            "capture.requested",
            CapturePayloadDraft::requested(EventAction::CaptureObserve, AuditInput::new()).into(),
            EventType::CaptureRequested,
            Normal,
        ),
        (
            "capture.completed",
            CapturePayloadDraft::completed(
                EventAction::CaptureObserve,
                EffectDisposition::Performed,
                1280,
                720,
                AuditInput::new(),
            )
            .into(),
            EventType::CaptureCompleted,
            Normal,
        ),
        (
            "capture.failed",
            CapturePayloadDraft::failed(
                EventAction::CaptureObserve,
                DiagnosticCode::CaptureFailed,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
            EventType::CaptureFailed,
            Normal,
        ),
        (
            "capture.pressure_changed",
            CapturePayloadDraft::pressure_changed(
                CapturePressureState::Tier2Flush,
                1_000,
                750,
                AuditInput::new(),
            )
            .into(),
            EventType::CapturePressureChanged,
            Normal,
        ),
        (
            "capture.dedup_window",
            CapturePayloadDraft::dedup_window(3, 900, AuditInput::new()).into(),
            EventType::CaptureDedupWindow,
            Normal,
        ),
        (
            "capture.policy_changed",
            CapturePayloadDraft::policy_changed(
                300,
                RetentionClass::DebugFull,
                CapturePolicyReason::Default,
                AuditInput::new(),
            )
            .into(),
            EventType::CapturePolicyChanged,
            Normal,
        ),
        (
            "recognition.requested",
            RecognitionPayloadDraft::requested(EventAction::RecognitionObserve, AuditInput::new())
                .into(),
            EventType::RecognitionRequested,
            Normal,
        ),
        (
            "recognition.completed",
            RecognitionPayloadDraft::completed(
                EventAction::RecognitionObserve,
                EffectDisposition::Performed,
                1280,
                720,
                RecognitionVerdict::FrameDecoded,
                AuditInput::new(),
            )
            .into(),
            EventType::RecognitionCompleted,
            Normal,
        ),
        (
            "recognition.failed",
            RecognitionPayloadDraft::failed(
                EventAction::RecognitionObserve,
                DiagnosticCode::RecognitionFailed,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
            EventType::RecognitionFailed,
            Normal,
        ),
        (
            "artifact.verified",
            ArtifactPayloadDraft::verified(AuditInput::new()).into(),
            EventType::ArtifactVerified,
            Normal,
        ),
        (
            "artifact.pin_recorded",
            pin_recorded(ids, owner),
            EventType::ArtifactPinRecorded,
            Normal,
        ),
        (
            "runtime.fact_recorded with an instance scope",
            fact_recorded(instance(), "device.connected", FactValue::Boolean(true)),
            EventType::RuntimeFactRecorded,
            Normal,
        ),
        (
            "runtime.fact_invalidated with an instance scope",
            fact_invalidated(
                instance(),
                "device.connected",
                RuntimeFactInvalidationReason::DeviceClosed,
            ),
            EventType::RuntimeFactInvalidated,
            Normal,
        ),
        (
            "runtime.lifecycle_observed carrying device.self_check",
            RuntimePayloadDraft::device_self_check(DeviceSelfCheck {
                instance_alias: "instance-a".to_owned(),
                entry: DeviceSelfCheckEntry::Input,
                status: DeviceSelfCheckStatus::Unknown,
                selected: None,
                capture: None,
                touch: None,
                failure_code: None,
                generation: 1,
            })
            .into(),
            EventType::RuntimeLifecycleObserved,
            Normal,
        ),
        (
            "runtime.lifecycle_observed phase backend_open_observed",
            RuntimePayloadDraft::backend_open_observed(owner, backend_open).into(),
            EventType::RuntimeLifecycleObserved,
            Normal,
        ),
        (
            "runtime.lifecycle_observed phase adb_target_recovery",
            RuntimePayloadDraft::adb_target_recovery(
                owner,
                AdbTargetRecovery {
                    endpoint: LifecycleNativeDetail::new("127.0.0.1:5555", false),
                    initial_error: LifecycleNativeDetail::new("device offline", false),
                    path: AdbRecoveryPath::Connect,
                    budget_ms: 1_000,
                    steps: vec![AdbRecoveryStep {
                        phase: AdbRecoveryPhase::InitialState,
                        attempt: 1,
                        elapsed_ms: 10,
                        command: None,
                        error: None,
                    }],
                    final_state: AdbTransportState::Device,
                    recovered: true,
                    dropped_count: 0,
                },
            )
            .into(),
            EventType::RuntimeLifecycleObserved,
            Normal,
        ),
    ]
}

/// `ledger-store.md` "Everything else is durable, including …": each item it names.
fn durable(ids: &IdentifierIssuer, owner: OwnerEpoch) -> Vec<Row> {
    use CommitSync::Full;
    vec![
        (
            "perf.balance_changed",
            PerformancePayloadDraft::balance_changed(
                PerformanceControlEventData {
                    observed_at_unix_ms: AT_MS,
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
                AuditInput::new(),
            )
            .into(),
            EventType::PerformanceBalanceChanged,
            Full,
        ),
        (
            "artifact.created",
            ArtifactPayloadDraft::created(AuditInput::new()).into(),
            EventType::ArtifactCreated,
            Full,
        ),
        (
            "runtime.fact_recorded with the Runtime scope",
            fact_recorded(
                RuntimeFactScope::Runtime,
                "config.subsystems",
                FactValue::RecordList(Vec::new()),
            ),
            EventType::RuntimeFactRecorded,
            Full,
        ),
        (
            "runtime.fact_invalidated with the Runtime scope",
            fact_invalidated(
                RuntimeFactScope::Runtime,
                "config.subsystems",
                RuntimeFactInvalidationReason::Operator,
            ),
            EventType::RuntimeFactInvalidated,
            Full,
        ),
        (
            "runtime.lifecycle_observed phase vendor_stdio_close",
            RuntimePayloadDraft::vendor_stdio_close(
                owner,
                Some(*ids.mint_instance_id().expect("instance id").transport()),
                VendorStdioFacts {
                    process_id: 1,
                    process_created_filetime: StdioFact::Known(1),
                    started_filetime: 1,
                    steps: Vec::new(),
                    dropped_count: 0,
                    paths: Vec::new(),
                    restart_manager: None,
                },
            )
            .into(),
            EventType::RuntimeLifecycleObserved,
            Full,
        ),
        (
            "task.selection_evaluated",
            TaskPayloadDraft::semantic(selection_evaluated(), AuditInput::new()).into(),
            EventType::TaskSelectionEvaluated,
            Full,
        ),
    ]
}

#[test]
fn gate_durability_classes_match_the_contract_table() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let owner = *ids.mint_owner_epoch().expect("owner epoch").transport();
    let rows = observational(&ids, owner)
        .into_iter()
        .chain(durable(&ids, owner))
        .collect::<Vec<_>>();
    for (item, payload, event_type, class) in rows {
        let draft = sanitized(&ids, item, payload);
        assert_eq!(draft.event_type(), event_type, "{item}: event type");
        assert_eq!(
            commit_sync(draft.payload()),
            class,
            "{item}: durability class"
        );
    }
}
