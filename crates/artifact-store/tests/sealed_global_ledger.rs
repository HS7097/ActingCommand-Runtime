// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_artifact_store::{
    ArtifactEventSink, ArtifactStoreError, ArtifactStoreResult, ArtifactWriteContext,
    CapturePipeline, CapturePipelineConfig, CapturePipelineSummary, EvidenceExportDocuments,
    EvidenceExportIdentity, EvidenceExportRequest, EvidenceExporter, EvidenceJsonDocument,
    EvidencePackage, FrameStoreConfig, FrameStoreFrameInput, MemorySample, MemorySampleSource,
    PackageVerification, RecognitionState, verify_evidence_archive,
};
use actingcommand_contract::{
    ArtifactKind, ArtifactLinksDraft, ArtifactRedactionState, AuditInput, CapturePolicyReason,
    DiagnosticCode, EffectDisposition, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventQuery, EventSeverity, EventSource, EventType, EvidenceCompleteness,
    IdentifierIssuer, IssuedCorrelationId, IssuedFrameId, IssuedRunId, OriginModule,
    PinnedFrameReason, ProjectedEvent, ProjectionProfile, RetentionClass, SanitizationError,
    SecretField, SecretFingerprinter, Sha256Fingerprint, TaskOutcome, TaskPayloadDraft,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerConfig};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
struct SealedIdentity {
    run: IssuedRunId,
    correlation: IssuedCorrelationId,
}

struct GlobalLedgerSink<'a> {
    ledger: &'a GlobalLedger,
    fail_next: Option<EventType>,
}

impl<'a> GlobalLedgerSink<'a> {
    fn new(ledger: &'a GlobalLedger) -> Self {
        Self {
            ledger,
            fail_next: None,
        }
    }

    fn fail_next(&mut self, event_type: EventType) {
        self.fail_next = Some(event_type);
    }
}

impl ArtifactEventSink for GlobalLedgerSink<'_> {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        let sanitized = draft.sanitize(&TestFingerprinter).map_err(|error| {
            ArtifactStoreError::fatal(
                "event_sanitize_failed",
                "append_sealed_event",
                error.to_string(),
            )
        })?;
        if self.fail_next == Some(sanitized.event_type()) {
            self.fail_next = None;
            return Err(ArtifactStoreError::fatal(
                "injected_global_ledger_failure",
                "append_sealed_event",
                "injected required global-ledger append failure",
            ));
        }
        self.ledger.append(sanitized).map(|_| ()).map_err(|error| {
            ArtifactStoreError::fatal(
                "global_ledger_append_failed",
                "append_sealed_event",
                format!("{} during {}", error.code(), error.operation()),
            )
        })
    }
}

struct TestFingerprinter;

impl SecretFingerprinter for TestFingerprinter {
    fn fingerprint(
        &self,
        _field: SecretField,
        original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        Sha256Fingerprint::new(format!("sha256:{}", "a".repeat(64)), original)
    }
}

#[test]
fn sealed_pipeline_round_trips_through_real_global_ledger_and_verified_export() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ledger = open_ledger(temp.path(), "c2-sealed-success");
    let identity = sealed_identity();
    let artifact_root = temp.path().join("artifacts");
    let mut sink = GlobalLedgerSink::new(&ledger);
    let summary = run_capture_pipeline(&artifact_root, temp.path(), identity, &mut sink);
    append_terminal(&mut sink, identity, TaskOutcome::Success);
    let pre_export_events = project_correlation(&ledger, identity);
    let terminal = terminal_event(&pre_export_events, TaskOutcome::Success);
    let output = temp.path().join("sealed-evidence.zip");
    let request = export_request(
        output.clone(),
        identity,
        TaskOutcome::Success,
        summary.clone(),
        pre_export_events.clone(),
        terminal,
    );
    let mut exporter = EvidenceExporter::open(&artifact_root).expect("exporter");

    let receipt = exporter.export(request, &mut sink).expect("sealed export");
    let verified = verify_evidence_archive(receipt.output_path(), receipt.zip_sha256())
        .expect("verify sealed evidence");
    let all_events = project_correlation(&ledger, identity);

    assert_eq!(summary.counts.captured, 3);
    assert_eq!(summary.counts.deduplicated, 1);
    assert_eq!(summary.counts.dropped, 0);
    assert_eq!(summary.counts.persisted, 2);
    assert_eq!(
        summary.evidence_completeness,
        EvidenceCompleteness::Complete
    );
    assert_eq!(verified.manifest.screenshot_counts.captured, 3);
    assert_eq!(verified.manifest.screenshot_counts.deduplicated, 1);
    assert_eq!(verified.manifest.screenshot_counts.dropped, 0);
    assert_eq!(verified.manifest.screenshot_counts.persisted, 2);
    assert_eq!(verified.manifest.pinned_count, 1);
    assert_eq!(
        verified
            .manifest
            .pinned_reason_counts
            .get(&PinnedFrameReason::Terminal),
        Some(&1)
    );
    assert!(verified.manifest.missing_pinned.is_empty());
    assert_eq!(
        verified.manifest.ledger_sequence_start,
        pre_export_events.first().expect("first event").sequence
    );
    assert_eq!(
        verified.manifest.ledger_sequence_end,
        pre_export_events.last().expect("last event").sequence
    );
    assert_eq!(
        verified.manifest.terminal_receipt.event_type,
        EventType::TaskCompleted
    );
    assert_eq!(receipt.archive().sha256(), receipt.zip_sha256());
    assert_eq!(receipt.archive().byte_count(), receipt.zip_byte_count());
    assert_eq!(sha256_file(&output), receipt.zip_sha256());

    for screenshot in &verified.manifest.screenshots {
        let source = summary
            .frames
            .iter()
            .find(|frame| frame.frame_index == screenshot.frame_index)
            .expect("source frame");
        assert_eq!(screenshot.artifact.sha256, source.artifact.sha256());
        assert_eq!(screenshot.artifact.byte_count, source.artifact.byte_count());
    }

    for event_type in [
        EventType::CapturePolicyChanged,
        EventType::CapturePressureChanged,
        EventType::CaptureDedupWindow,
        EventType::ArtifactCreated,
        EventType::ArtifactVerified,
        EventType::ArtifactExportCompleted,
    ] {
        assert!(
            all_events
                .iter()
                .any(|event| event.event_type == event_type),
            "missing {event_type:?} in correlated projection"
        );
        assert!(!query_type(&ledger, identity, event_type).is_empty());
    }
    let completed = all_events
        .iter()
        .find(|event| event.event_type == EventType::ArtifactExportCompleted)
        .expect("completed export event");
    assert_eq!(completed.artifacts.len(), 1);
    assert_eq!(completed.artifacts[0].sha256, receipt.zip_sha256());
    assert_eq!(completed.artifacts[0].kind, ArtifactKind::EvidenceArchive);

    ledger.close().expect("close ledger");
}

#[test]
fn required_global_ledger_failure_cannot_return_success_or_leave_archive() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ledger = open_ledger(temp.path(), "c2-sealed-event-failure");
    let identity = sealed_identity();
    let artifact_root = temp.path().join("artifacts");
    let mut sink = GlobalLedgerSink::new(&ledger);
    let summary = run_capture_pipeline(&artifact_root, temp.path(), identity, &mut sink);
    append_terminal(&mut sink, identity, TaskOutcome::Failure);
    let events = project_correlation(&ledger, identity);
    let terminal = terminal_event(&events, TaskOutcome::Failure);
    let output = temp.path().join("event-failure.zip");
    let request = export_request(
        output.clone(),
        identity,
        TaskOutcome::Failure,
        summary,
        events,
        terminal,
    );
    sink.fail_next(EventType::ArtifactExportCompleted);
    let mut exporter = EvidenceExporter::open(&artifact_root).expect("exporter");

    let error = exporter
        .export(request, &mut sink)
        .expect_err("required ledger failure");
    let all_events = project_correlation(&ledger, identity);

    assert_eq!(error.code(), "injected_global_ledger_failure");
    assert!(!output.exists());
    assert!(
        all_events
            .iter()
            .all(|event| event.event_type != EventType::ArtifactExportCompleted)
    );
    assert!(
        all_events
            .iter()
            .any(|event| event.event_type == EventType::ArtifactExportFailed)
    );
    for event in all_events
        .iter()
        .filter(|event| event.event_type == EventType::ArtifactCreated)
    {
        for artifact in &event.artifacts {
            if artifact.kind == ArtifactKind::EvidenceArchive {
                let object_key = artifact.object_key.as_ref().expect("forensic object key");
                assert!(!artifact_root.join(object_key).exists());
            }
        }
    }

    ledger.close().expect("close ledger");
}

#[test]
fn corrupted_frame_artifact_cannot_publish_archive_and_is_ledger_visible() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ledger = open_ledger(temp.path(), "c2-sealed-artifact-failure");
    let identity = sealed_identity();
    let artifact_root = temp.path().join("artifacts");
    let mut sink = GlobalLedgerSink::new(&ledger);
    let summary = run_capture_pipeline(&artifact_root, temp.path(), identity, &mut sink);
    append_terminal(&mut sink, identity, TaskOutcome::Failure);
    let events = project_correlation(&ledger, identity);
    let terminal = terminal_event(&events, TaskOutcome::Failure);
    let source = summary.frames.first().expect("source frame");
    fs::write(
        artifact_root.join(source.artifact.object_key()),
        b"corrupted artifact",
    )
    .expect("corrupt source");
    let output = temp.path().join("artifact-failure.zip");
    let request = export_request(
        output.clone(),
        identity,
        TaskOutcome::Failure,
        summary,
        events,
        terminal,
    );
    let mut exporter = EvidenceExporter::open(&artifact_root).expect("exporter");

    let error = exporter
        .export(request, &mut sink)
        .expect_err("corrupt artifact failure");
    let all_events = project_correlation(&ledger, identity);

    assert_eq!(error.code(), "artifact_hash_mismatch");
    assert!(!output.exists());
    assert!(
        all_events
            .iter()
            .all(|event| event.event_type != EventType::ArtifactExportCompleted)
    );
    assert!(
        all_events
            .iter()
            .any(|event| event.event_type == EventType::ArtifactExportFailed)
    );

    ledger.close().expect("close ledger");
}

fn open_ledger(root: &Path, owner: &str) -> GlobalLedger {
    GlobalLedger::open(GlobalLedgerConfig::new(root.join("ledger"), owner)).expect("open ledger")
}

fn sealed_identity() -> SealedIdentity {
    let identifiers = IdentifierIssuer::new().expect("identifiers");
    SealedIdentity {
        run: identifiers.mint_run_id().expect("run"),
        correlation: identifiers.mint_correlation_id().expect("correlation"),
    }
}

fn run_capture_pipeline(
    artifact_root: &Path,
    temp_root: &Path,
    identity: SealedIdentity,
    sink: &mut GlobalLedgerSink<'_>,
) -> CapturePipelineSummary {
    let mut pipeline = CapturePipeline::open(
        artifact_root,
        temp_root.join("frames"),
        pipeline_config(),
        write_context(identity, None, 1_752_147_200_000),
        sink,
    )
    .expect("pipeline");
    pipeline
        .record_frame(
            frame_input(1, 17, None),
            write_context(identity, Some(frame_id()), 1_752_147_200_100),
            sink,
        )
        .expect("first frame");
    pipeline
        .record_frame(
            frame_input(2, 17, None),
            write_context(identity, Some(frame_id()), 1_752_147_200_200),
            sink,
        )
        .expect("deduplicated frame");
    pipeline
        .record_frame(
            frame_input(3, 29, Some(PinnedFrameReason::Terminal)),
            write_context(identity, Some(frame_id()), 1_752_147_200_300),
            sink,
        )
        .expect("pinned frame");
    pipeline.finish(sink).expect("finish pipeline")
}

fn pipeline_config() -> CapturePipelineConfig {
    let mut frame_store = FrameStoreConfig::default();
    frame_store.similarity_threshold = 0.95;
    frame_store.tier1_ratio = 0.50;
    frame_store.tier2_ratio = 0.70;
    frame_store.tier3_ratio = 0.90;
    frame_store.hysteresis_ratio = 0.10;
    frame_store.max_mem_bytes = Some(7_000);
    frame_store.os_reserve_bytes = 0;
    frame_store.flush_workspace_reserve_bytes = 1;
    let frame_store = frame_store.with_memory_source(MemorySampleSource::fixed(MemorySample {
        total_bytes: 7_000,
        available_bytes: 7_000,
    }));
    CapturePipelineConfig {
        frame_store,
        cadence_ms: 300,
        retention_class: RetentionClass::DebugFull,
        policy_reason: CapturePolicyReason::Default,
        redaction_state: ArtifactRedactionState::NotRequired,
    }
}

fn frame_input(
    frame_index: usize,
    value: u8,
    pinned_reason: Option<PinnedFrameReason>,
) -> FrameStoreFrameInput {
    let frame = Frame::from_pixels(
        16,
        16,
        vec![value; 16 * 16 * 3],
        PixelFormat::Rgb8,
        CaptureBackendName::AdbScreencap,
    )
    .expect("frame");
    FrameStoreFrameInput {
        frame_index,
        file_name: format!("frame-{frame_index}.png"),
        label: "steady".to_string(),
        recognition_state: RecognitionState::Matched {
            page_id: "sealed/home".to_string(),
        },
        pinned_reason,
        frame,
    }
}

fn frame_id() -> IssuedFrameId {
    IdentifierIssuer::new()
        .expect("identifiers")
        .mint_frame_id()
        .expect("frame id")
}

fn write_context(
    identity: SealedIdentity,
    frame: Option<IssuedFrameId>,
    timestamp_unix_ms: u64,
) -> ArtifactWriteContext {
    let mut artifact_links = ArtifactLinksDraft::default()
        .with_run_id(identity.run)
        .with_correlation_id(identity.correlation);
    let mut event_links = EventLinksDraft::default()
        .with_run_id(identity.run)
        .with_correlation_id(identity.correlation);
    if let Some(frame) = frame {
        artifact_links = artifact_links.with_frame_id(frame);
        event_links = event_links.with_frame_id(frame);
    }
    ArtifactWriteContext::new(artifact_links, event_links, timestamp_unix_ms)
}

fn append_terminal(
    sink: &mut GlobalLedgerSink<'_>,
    identity: SealedIdentity,
    outcome: TaskOutcome,
) {
    let identifiers = IdentifierIssuer::new().expect("identifiers");
    let payload = match outcome {
        TaskOutcome::Success => TaskPayloadDraft::completed(
            EventAction::CriticalTest,
            EffectDisposition::Performed,
            AuditInput::new(),
        ),
        TaskOutcome::Failure => TaskPayloadDraft::failed(
            EventAction::CriticalTest,
            DiagnosticCode::RuntimeDiagnostic,
            EffectDisposition::NotPerformed,
            AuditInput::new(),
        ),
        TaskOutcome::Cancelled => TaskPayloadDraft::cancelled(
            EventAction::CriticalTest,
            EffectDisposition::NotPerformed,
            AuditInput::new(),
        ),
    };
    sink.append(EventDraft::new(
        identifiers.mint_event_id().expect("event"),
        1_752_147_200_900,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::System,
            OriginModule::ProcessTest,
            EventActor::System,
        ),
        EventLinksDraft::default()
            .with_run_id(identity.run)
            .with_correlation_id(identity.correlation),
        payload.into(),
    ))
    .expect("append terminal");
}

fn project_correlation(ledger: &GlobalLedger, identity: SealedIdentity) -> Vec<ProjectedEvent> {
    ledger
        .project(
            EventQuery {
                correlation_id: Some(*identity.correlation.transport()),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("project correlation")
}

fn query_type(
    ledger: &GlobalLedger,
    identity: SealedIdentity,
    event_type: EventType,
) -> Vec<actingcommand_ledger::PersistedEvent> {
    ledger
        .query(EventQuery {
            event_type: Some(event_type),
            correlation_id: Some(*identity.correlation.transport()),
            ..EventQuery::default()
        })
        .expect("query event type")
}

fn terminal_event(events: &[ProjectedEvent], outcome: TaskOutcome) -> ProjectedEvent {
    let event_type = match outcome {
        TaskOutcome::Success => EventType::TaskCompleted,
        TaskOutcome::Failure => EventType::TaskFailed,
        TaskOutcome::Cancelled => EventType::TaskCancelled,
    };
    events
        .iter()
        .find(|event| event.event_type == event_type)
        .cloned()
        .expect("terminal event")
}

fn export_request(
    output_path: PathBuf,
    identity: SealedIdentity,
    outcome: TaskOutcome,
    pipeline: CapturePipelineSummary,
    events: Vec<ProjectedEvent>,
    terminal_receipt: ProjectedEvent,
) -> EvidenceExportRequest {
    #[derive(Serialize)]
    struct Document<'a> {
        status: &'a str,
    }

    EvidenceExportRequest {
        output_path,
        identity: EvidenceExportIdentity {
            run_id: *identity.run.transport(),
            correlation_id: *identity.correlation.transport(),
            package: EvidencePackage::new(
                "sealed-package.zip",
                "c".repeat(64),
                PackageVerification::Passed,
            )
            .expect("package"),
            task_outcome: outcome,
            terminal_receipt,
            projection_profile: ProjectionProfile::Forensic,
            retention_class: RetentionClass::DebugFull,
            archive_redaction_state: ArtifactRedactionState::NotRequired,
        },
        events,
        pipeline,
        documents: EvidenceExportDocuments::new(
            EvidenceJsonDocument::from_serializable(&Document { status: "result" })
                .expect("result"),
            EvidenceJsonDocument::from_serializable(&Document {
                status: "diagnostics",
            })
            .expect("diagnostics"),
            "sealed GlobalLedger evidence",
        )
        .expect("documents"),
        archive_context: write_context(identity, None, 1_752_147_201_000),
    }
}

fn sha256_file(path: &Path) -> String {
    let digest = Sha256::digest(fs::read(path).expect("read archive"));
    format!("sha256:{digest:x}")
}
