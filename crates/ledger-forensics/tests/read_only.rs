// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_artifact_store::{
    ArtifactEventSink, ArtifactStoreError, ArtifactStoreResult, ArtifactWriteContext,
    CapturePipelineCounts, CapturePipelineSummary, EvidenceExportDocuments, EvidenceExportIdentity,
    EvidenceExportRequest, EvidenceExporter, EvidenceJsonDocument, EvidencePackage,
    PackageVerification, capture_summary_record, verify_evidence_archive,
};
use actingcommand_contract::{
    ArtifactLinksDraft, ArtifactRedactionState, AuditInput, CapturePayloadDraft,
    CommandPayloadDraft, DiagnosticCode, EffectDisposition, EventAction, EventActor, EventDraft,
    EventLinksDraft, EventOrigin, EventPayloadDraft, EventQuery, EventSeverity, EventSource,
    EventType, EvidenceCompleteness, IdentifierIssuer, IssuedEventId, OriginModule,
    ProjectionProfile, RetentionClass, SanitizedEventDraft, TaskOutcome, TaskPayloadDraft,
};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerConfig, Sha256SecretFingerprinter};
use actingcommand_ledger_forensics::{
    ForensicCommand, ForensicEventFilter, ForensicEventsRequest, ForensicOutput,
    ForensicReplayRequest, ForensicReport, ForensicRequest, WriterObservationReport, replay, run,
};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[test]
fn forensic_snapshot_commands_are_read_only_and_deterministic() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_root = temp.path();
    let ledger_root = state_root.join("ledger");
    let identifiers = IdentifierIssuer::new().expect("identifiers");
    let request_id = identifiers.mint_request_id().expect("request id");
    let request_text = serde_json::to_value(request_id.transport())
        .expect("request id JSON")
        .as_str()
        .expect("request id string")
        .to_owned();

    let writer = GlobalLedger::open(GlobalLedgerConfig::new(&ledger_root, "repair-source"))
        .expect("open repair source");
    writer
        .append(event(
            EventLinksDraft::default().with_request_id(request_id),
        ))
        .expect("append first request event");
    writer
        .append(event(
            EventLinksDraft::default().with_request_id(request_id),
        ))
        .expect("append second request event");
    writer.close().expect("close repair source");
    append_bytes(&latest_segment(&ledger_root), b"{\"repair\":true");

    let active_writer = GlobalLedger::open(GlobalLedgerConfig::new(&ledger_root, "active-writer"))
        .expect("recover and hold writer");
    let live_tail = b"{\"partial\":true";
    append_bytes(&latest_segment(&ledger_root), live_tail);
    let before = tree_bytes(state_root);

    let commands = [
        ForensicCommand::Open,
        ForensicCommand::Events,
        ForensicCommand::Chain {
            request_id: request_text.clone(),
        },
        ForensicCommand::Tail,
        ForensicCommand::Repairs,
        ForensicCommand::Export,
    ];
    let mut outputs = Vec::new();
    for command in commands {
        let first = run(ForensicRequest::new(state_root, command.clone()))
            .expect("first deterministic forensic report");
        let second = run(ForensicRequest::new(state_root, command))
            .expect("second deterministic forensic report");
        assert_eq!(first, second);
        outputs.push(first);
    }

    assert_eq!(tree_bytes(state_root), before);
    match &outputs[0] {
        ForensicOutput::Machine(ForensicReport::Open(report)) => {
            assert!(report.event_count >= 2);
            assert!(report.repair_count >= 1);
            assert!(report.corrupt_tail.is_some());
            match &report.writer {
                WriterObservationReport::Locked { byte_count } => assert!(*byte_count > 0),
                WriterObservationReport::Readable { active, .. } => assert!(*active),
                WriterObservationReport::Absent => panic!("active writer observation was absent"),
            }
        }
        output => panic!("unexpected open output: {output:?}"),
    }
    match &outputs[1] {
        ForensicOutput::Machine(ForensicReport::Events(report)) => {
            assert!(report.events.len() >= 2);
            assert!(report.events.len() <= report.limit);
        }
        output => panic!("unexpected events output: {output:?}"),
    }
    match &outputs[2] {
        ForensicOutput::Machine(ForensicReport::Chain(report)) => {
            assert_eq!(report.request_id, request_text);
            assert_eq!(report.events.len(), 2);
            assert!(report.events.iter().all(|event| {
                event.links().request_id().is_some_and(|value| {
                    serde_json::to_value(value)
                        .ok()
                        .as_ref()
                        .and_then(|json| json.as_str())
                        == Some(request_text.as_str())
                })
            }));
        }
        output => panic!("unexpected chain output: {output:?}"),
    }
    match &outputs[3] {
        ForensicOutput::Machine(ForensicReport::Tail(report)) => {
            let tail = report.corrupt_tail.as_ref().expect("live corrupt tail");
            assert_eq!(tail.code, "corrupt_segment");
            assert_eq!(tail.dangling_byte_count, live_tail.len() as u64);
        }
        output => panic!("unexpected tail output: {output:?}"),
    }
    match &outputs[4] {
        ForensicOutput::Machine(ForensicReport::Repairs(report)) => {
            assert!(!report.repairs.is_empty());
            assert!(report.repairs.iter().all(|repair| repair.completed));
        }
        output => panic!("unexpected repairs output: {output:?}"),
    }
    match &outputs[5] {
        ForensicOutput::Human(report) => {
            assert!(report.starts_with("ActingCommand ledger forensic export\n"));
            assert!(report.contains("corrupt_tail: corrupt_segment"));
            assert!(report.contains("repairs:"));
            assert!(report.contains("events:"));
        }
        output => panic!("unexpected export output: {output:?}"),
    }

    active_writer.close().expect("close active writer");
}

#[test]
fn filters_events_by_persisted_fields_with_stable_cursor() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_root = temp.path();
    let ledger_root = state_root.join("ledger");
    let identifiers = IdentifierIssuer::new().expect("identifiers");
    let correlation_a = identifiers.mint_correlation_id().expect("correlation a");
    let correlation_b = identifiers.mint_correlation_id().expect("correlation b");
    let correlation_a_text = serde_json::to_value(correlation_a.transport())
        .expect("correlation a JSON")
        .as_str()
        .expect("correlation a string")
        .to_owned();
    let correlation_b_text = serde_json::to_value(correlation_b.transport())
        .expect("correlation b JSON")
        .as_str()
        .expect("correlation b string")
        .to_owned();
    let writer = GlobalLedger::open(GlobalLedgerConfig::new(&ledger_root, "filter-source"))
        .expect("open filter source");
    let fixtures: Vec<(
        EventSeverity,
        EventSource,
        OriginModule,
        EventActor,
        EventLinksDraft,
        EventPayloadDraft,
    )> = vec![
        (
            EventSeverity::Info,
            EventSource::Cli,
            OriginModule::Actingctl,
            EventActor::User,
            EventLinksDraft::default().with_correlation_id(correlation_a),
            CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
        ),
        (
            EventSeverity::Warning,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            EventLinksDraft::default().with_correlation_id(correlation_a),
            CommandPayloadDraft::rejected(
                EventAction::RuntimeStart,
                DiagnosticCode::CommandRejected,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            )
            .into(),
        ),
        (
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            EventLinksDraft::default().with_correlation_id(correlation_a),
            CommandPayloadDraft::rejected(
                EventAction::RuntimeStart,
                DiagnosticCode::CommandRejected,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            )
            .into(),
        ),
        (
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            EventLinksDraft::default().with_correlation_id(correlation_b),
            CommandPayloadDraft::rejected(
                EventAction::RuntimeStart,
                DiagnosticCode::CommandRejected,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            )
            .into(),
        ),
        (
            EventSeverity::Error,
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
            EventLinksDraft::default().with_correlation_id(correlation_a),
            CommandPayloadDraft::rejected(
                EventAction::RuntimeStart,
                DiagnosticCode::LeaseBusy,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            )
            .into(),
        ),
        (
            EventSeverity::Fatal,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            EventLinksDraft::default().with_correlation_id(correlation_a),
            CommandPayloadDraft::rejected(
                EventAction::RuntimeStart,
                DiagnosticCode::CommandRejected,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            )
            .into(),
        ),
    ];
    for (index, (severity, source, module, actor, links, payload)) in
        fixtures.into_iter().enumerate()
    {
        let draft = EventDraft::new(
            identifiers.mint_event_id().expect("event id"),
            1_752_147_200_000 + index as u64,
            severity,
            EventOrigin::new(source, module, actor),
            links,
            payload,
        )
        .sanitize(&Sha256SecretFingerprinter::new(b"forensic-filter-salt").expect("fingerprinter"))
        .expect("sanitize filter event");
        writer.append(draft).expect("append filter event");
    }
    writer.close().expect("close filter source");

    let page = |filter: ForensicEventFilter, after, through, limit| {
        let options = ForensicEventsRequest::new(filter, after, through, limit)
            .expect("valid events request");
        match run(ForensicRequest::events(state_root, options)).expect("events report") {
            ForensicOutput::Machine(ForensicReport::Events(report)) => report,
            output => panic!("unexpected events output: {output:?}"),
        }
    };
    let filter = |origin: Option<&str>,
                  diagnostic: Option<&str>,
                  severity: Option<&str>,
                  correlation: Option<&str>| {
        ForensicEventFilter::new(
            origin.map(str::to_owned),
            diagnostic.map(str::to_owned),
            severity.map(str::to_owned),
            correlation.map(str::to_owned),
        )
        .expect("valid event filter")
    };

    assert_eq!(
        page(filter(Some("runtime"), None, None, None), 0, Some(6), 10)
            .events
            .iter()
            .map(|event| event.sequence())
            .collect::<Vec<_>>(),
        vec![2, 3, 4, 6]
    );
    assert_eq!(
        page(
            filter(None, Some("command.rejected"), None, None),
            0,
            Some(6),
            10,
        )
        .events
        .iter()
        .map(|event| event.sequence())
        .collect::<Vec<_>>(),
        vec![2, 3, 4, 6]
    );
    assert_eq!(
        page(filter(None, None, Some("error"), None), 0, Some(6), 10)
            .events
            .iter()
            .map(|event| event.sequence())
            .collect::<Vec<_>>(),
        vec![3, 4, 5]
    );
    assert_eq!(
        page(
            filter(None, None, None, Some(&correlation_b_text)),
            0,
            Some(6),
            10,
        )
        .events
        .iter()
        .map(|event| event.sequence())
        .collect::<Vec<_>>(),
        vec![4]
    );
    assert_eq!(
        page(
            filter(
                Some("runtime"),
                Some("command.rejected"),
                Some("error"),
                Some(&correlation_a_text),
            ),
            0,
            Some(6),
            10,
        )
        .events
        .iter()
        .map(|event| event.sequence())
        .collect::<Vec<_>>(),
        vec![3]
    );

    let pagination_filter = filter(Some("runtime"), Some("command.rejected"), None, None);
    let first = page(pagination_filter.clone(), 0, None, 2);
    assert_eq!(first.after_sequence, 0);
    assert_eq!(first.through_sequence, 6);
    assert_eq!(first.limit, 2);
    assert_eq!(
        first
            .events
            .iter()
            .map(|event| event.sequence())
            .collect::<Vec<_>>(),
        vec![2, 3]
    );
    assert_eq!(first.next_after_sequence, Some(3));
    assert_eq!(first.filter, pagination_filter);

    let writer = GlobalLedger::open(GlobalLedgerConfig::new(&ledger_root, "late-filter-source"))
        .expect("open late source");
    writer
        .append(
            EventDraft::new(
                identifiers.mint_event_id().expect("late event id"),
                1_752_147_200_100,
                EventSeverity::Error,
                EventOrigin::new(
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                ),
                EventLinksDraft::default().with_correlation_id(correlation_a),
                CommandPayloadDraft::rejected(
                    EventAction::RuntimeStart,
                    DiagnosticCode::CommandRejected,
                    EffectDisposition::NotPerformed,
                    AuditInput::new(),
                )
                .into(),
            )
            .sanitize(
                &Sha256SecretFingerprinter::new(b"forensic-filter-salt").expect("fingerprinter"),
            )
            .expect("sanitize late event"),
        )
        .expect("append late event");
    writer.close().expect("close late source");

    let second = page(pagination_filter, 3, Some(6), 2);
    assert_eq!(
        second
            .events
            .iter()
            .map(|event| event.sequence())
            .collect::<Vec<_>>(),
        vec![4, 6]
    );
    assert_eq!(second.next_after_sequence, None);
    assert!(
        page(filter(None, None, Some("debug"), None), 6, Some(6), 1)
            .events
            .is_empty()
    );
    assert!(ForensicEventsRequest::new(ForensicEventFilter::default(), 0, None, 0).is_err());
    assert!(
        ForensicEventsRequest::new(
            ForensicEventFilter::default(),
            0,
            None,
            actingcommand_ledger_forensics::MAX_FORENSIC_EVENTS + 1,
        )
        .is_err()
    );
}

#[test]
fn replays_a_sealed_archive_through_the_canonical_verifier() {
    struct LedgerSink<'a> {
        ledger: &'a GlobalLedger,
    }

    impl ArtifactEventSink for LedgerSink<'_> {
        fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
            let event = draft
                .sanitize(
                    &Sha256SecretFingerprinter::new(b"forensic-replay-sink")
                        .expect("fingerprinter"),
                )
                .map_err(|error| {
                    ArtifactStoreError::fatal(
                        "event_sanitize_failed",
                        "append_replay_fixture_event",
                        error.to_string(),
                    )
                })?;
            self.ledger.append(event).map(|_| ()).map_err(|error| {
                ArtifactStoreError::fatal(
                    error.code(),
                    "append_replay_fixture_event",
                    error.to_string(),
                )
            })
        }
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let identifiers = IdentifierIssuer::new().expect("identifiers");
    let run_id = identifiers.mint_run_id().expect("run id");
    let correlation_id = identifiers.mint_correlation_id().expect("correlation id");
    let links = EventLinksDraft::default()
        .with_run_id(run_id)
        .with_correlation_id(correlation_id);
    let pipeline = CapturePipelineSummary {
        counts: CapturePipelineCounts {
            captured: 0,
            deduplicated: 0,
            dropped: 0,
            persisted: 0,
        },
        evidence_completeness: EvidenceCompleteness::Complete,
        pinned: Vec::new(),
        frames: Vec::new(),
    };
    let ledger = GlobalLedger::open(GlobalLedgerConfig::new(
        temp.path().join("fixture-ledger"),
        "replay-fixture",
    ))
    .expect("open fixture ledger");
    ledger
        .append(
            EventDraft::new(
                identifiers.mint_event_id().expect("summary event id"),
                1_752_147_200_000,
                EventSeverity::Info,
                EventOrigin::new(
                    EventSource::Runtime,
                    OriginModule::CapturePipeline,
                    EventActor::Runtime,
                ),
                links.clone(),
                CapturePayloadDraft::summary_committed(
                    capture_summary_record(&pipeline).expect("capture summary"),
                    AuditInput::new(),
                )
                .into(),
            )
            .sanitize(
                &Sha256SecretFingerprinter::new(b"forensic-replay-fixture").expect("fingerprinter"),
            )
            .expect("sanitize summary"),
        )
        .expect("append summary");
    ledger
        .append(
            EventDraft::new(
                identifiers.mint_event_id().expect("terminal event id"),
                1_752_147_200_100,
                EventSeverity::Info,
                EventOrigin::new(
                    EventSource::System,
                    OriginModule::ProcessTest,
                    EventActor::System,
                ),
                links.clone(),
                TaskPayloadDraft::completed(
                    EventAction::CriticalTest,
                    EffectDisposition::Performed,
                    AuditInput::new(),
                )
                .into(),
            )
            .sanitize(
                &Sha256SecretFingerprinter::new(b"forensic-replay-fixture").expect("fingerprinter"),
            )
            .expect("sanitize terminal"),
        )
        .expect("append terminal");
    let events = ledger
        .project(
            EventQuery {
                correlation_id: Some(*correlation_id.transport()),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("project fixture events");
    let terminal_receipt = events
        .iter()
        .find(|event| event.event_type == EventType::TaskCompleted)
        .cloned()
        .expect("terminal receipt");
    let archive = temp.path().join("sealed-evidence.zip");
    let artifact_root = temp.path().join("artifacts");
    let documents = EvidenceExportDocuments::new(
        EvidenceJsonDocument::from_serializable(&serde_json::json!({ "status": "result" }))
            .expect("result document"),
        EvidenceJsonDocument::from_serializable(&serde_json::json!({ "status": "diagnostics" }))
            .expect("diagnostics document"),
        "forensic replay fixture",
    )
    .expect("documents");
    let request = EvidenceExportRequest {
        output_path: archive.clone(),
        identity: EvidenceExportIdentity {
            run_id: *run_id.transport(),
            correlation_id: *correlation_id.transport(),
            package: EvidencePackage::new(
                "sealed-package.zip",
                "b".repeat(64),
                PackageVerification::Passed,
            )
            .expect("package"),
            task_outcome: TaskOutcome::Success,
            terminal_receipt,
            projection_profile: ProjectionProfile::Forensic,
            retention_class: RetentionClass::DebugFull,
            archive_redaction_state: ArtifactRedactionState::NotRequired,
        },
        events,
        source_capture_summary_sequence: 1,
        pipeline,
        documents,
        archive_context: ArtifactWriteContext::new(
            ArtifactLinksDraft::default()
                .with_run_id(run_id)
                .with_correlation_id(correlation_id),
            links,
            1_752_147_200_200,
        ),
    };
    let mut exporter = EvidenceExporter::open(&artifact_root).expect("exporter");
    let receipt = exporter
        .export(request, &mut LedgerSink { ledger: &ledger })
        .expect("sealed export");
    ledger.close().expect("close fixture ledger");
    let expected =
        verify_evidence_archive(&archive, receipt.zip_sha256()).expect("canonical verification");
    let archive_bytes = fs::read(&archive).expect("archive bytes");
    let before = tree_bytes(temp.path());

    let first = replay(ForensicReplayRequest::new(
        &archive,
        receipt.zip_sha256().to_owned(),
    ))
    .expect("first replay");
    let second = replay(ForensicReplayRequest::new(
        &archive,
        receipt.zip_sha256().to_owned(),
    ))
    .expect("second replay");
    assert_eq!(first, second);
    match first {
        ForensicOutput::Machine(ForensicReport::Replay(report)) => {
            assert_eq!(
                report.verifier,
                "actingcommand_artifact_store::verify_evidence_archive"
            );
            assert_eq!(report.zip_byte_count, expected.zip_byte_count);
            assert_eq!(report.zip_sha256, expected.zip_sha256);
            assert_eq!(report.manifest_sha256, expected.manifest_sha256);
            assert_eq!(report.manifest, expected.manifest);
        }
        output => panic!("unexpected replay output: {output:?}"),
    }
    assert_eq!(
        fs::read(&archive).expect("archive after replay"),
        archive_bytes
    );
    assert_eq!(tree_bytes(temp.path()), before);

    let mismatch = replay(ForensicReplayRequest::new(&archive, "0".repeat(64)))
        .expect_err("external hash mismatch");
    assert_eq!(mismatch.code(), "evidence_archive_hash_mismatch");
    assert_eq!(
        fs::read(&archive).expect("archive after mismatch"),
        archive_bytes
    );
    assert_eq!(tree_bytes(temp.path()), before);

    let corrupt = temp.path().join("corrupt-evidence.zip");
    let mut corrupt_bytes = archive_bytes.clone();
    corrupt_bytes[0] ^= 0xff;
    fs::write(&corrupt, &corrupt_bytes).expect("write corrupt archive");
    let corrupt_before = tree_bytes(temp.path());
    assert!(
        replay(ForensicReplayRequest::new(
            &corrupt,
            receipt.zip_sha256().to_owned(),
        ))
        .is_err()
    );
    assert_eq!(tree_bytes(temp.path()), corrupt_before);
}

#[test]
fn performance_pages_preserve_typed_facts_and_read_only_boundaries() {
    use actingcommand_contract::{
        PerformanceControlEventData, PerformanceControlLevel, PerformanceControlReason,
        PerformancePayloadDraft, PerformanceStutterEventData,
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let state_root = temp.path();
    let ledger_root = state_root.join("ledger");
    let identifiers = IdentifierIssuer::new().expect("identifiers");
    let request_id = identifiers.mint_request_id().expect("request id");
    let writer = GlobalLedger::open(GlobalLedgerConfig::new(&ledger_root, "performance-fixture"))
        .expect("open ledger");
    let mut facts = vec![
        writer
            .append(event(EventLinksDraft::default()))
            .expect("unrelated fact"),
    ];
    let control = PerformanceControlEventData {
        observed_at_unix_ms: 1_752_147_201_000,
        instance_id: None,
        previous_level: PerformanceControlLevel::Normal,
        level: PerformanceControlLevel::Normal,
        reason: PerformanceControlReason::ClockJump,
        host_responsiveness_basis_points: None,
        third_party_pressure_basis_points: None,
        recovery: false,
        deadline_disposition: None,
    };
    let stutter = PerformanceStutterEventData {
        instance_id: "instance:fixture-a".to_owned(),
        observed_at_unix_ms: 1_752_147_202_000,
        frame_gap_ms: 1_500,
        capture_latency_ms: None,
        recognition_latency_ms: None,
        action_effect_latency_ms: None,
    };
    let payloads: [EventPayloadDraft; 5] = [
        PerformancePayloadDraft::stutter_detected(stutter.clone(), AuditInput::new()).into(),
        PerformancePayloadDraft::balance_changed(
            PerformanceControlEventData {
                level: PerformanceControlLevel::DispatchPaused,
                reason: PerformanceControlReason::ThirdPartyContention,
                third_party_pressure_basis_points: Some(3_000),
                ..control.clone()
            },
            AuditInput::new(),
        )
        .into(),
        PerformancePayloadDraft::balance_changed(control.clone(), AuditInput::new()).into(),
        PerformancePayloadDraft::stutter_detected(
            PerformanceStutterEventData {
                capture_latency_ms: Some(120),
                recognition_latency_ms: Some(80),
                action_effect_latency_ms: Some(250),
                ..stutter
            },
            AuditInput::new(),
        )
        .into(),
        PerformancePayloadDraft::balance_changed(
            PerformanceControlEventData {
                instance_id: Some("instance:fixture-a".to_owned()),
                host_responsiveness_basis_points: Some(9_000),
                third_party_pressure_basis_points: Some(3_000),
                ..control
            },
            AuditInput::new(),
        )
        .into(),
    ];
    for payload in payloads {
        let draft = EventDraft::new(
            identifiers.mint_event_id().expect("event id"),
            1_752_147_203_000,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::PerformanceMonitor,
                EventActor::Runtime,
            ),
            EventLinksDraft::default().with_request_id(request_id),
            payload,
        )
        .sanitize(
            &Sha256SecretFingerprinter::new(b"performance-fixture-salt").expect("fingerprinter"),
        )
        .expect("sanitize performance fact");
        facts.push(writer.append(draft).expect("append performance fact"));
    }
    let before = tree_bytes(state_root);
    let first = run(ForensicRequest::performance(
        state_root,
        ForensicEventsRequest::new(ForensicEventFilter::default(), 0, None, 1).expect("first page"),
    ))
    .expect("first report");
    let ForensicOutput::Machine(ForensicReport::Performance(first)) = first else {
        panic!("expected performance report");
    };
    assert_eq!(tree_bytes(state_root), before);
    assert!(first.rows.is_empty());
    assert_eq!(first.scanned_event_count, 1);
    assert_eq!(first.next_after_sequence, Some(facts[0].sequence()));
    assert_eq!((first.stutter_count, first.clock_jump_count), (0, 0));
    assert!(first.has_more);
    assert!(!first.window_complete);
    let through = first.through_sequence;
    assert_eq!(through, facts[5].sequence());

    writer
        .append(event(EventLinksDraft::default()))
        .expect("later fact outside frozen range");
    let before = tree_bytes(state_root);
    let mut after = first
        .next_after_sequence
        .expect("empty matching page advances");
    let mut rows = Vec::new();
    for fact in &facts[1..] {
        let output = run(ForensicRequest::performance(
            state_root,
            ForensicEventsRequest::new(ForensicEventFilter::default(), after, Some(through), 1)
                .expect("bounded page"),
        ))
        .expect("bounded report");
        let ForensicOutput::Machine(ForensicReport::Performance(page)) = output else {
            panic!("expected performance report");
        };
        assert_eq!(page.after_sequence, after);
        assert_eq!(page.through_sequence, through);
        assert_eq!(page.scanned_event_count, 1);
        assert_eq!(page.scanned_through_sequence, fact.sequence());
        assert_eq!(page.stutter_count + page.clock_jump_count, page.rows.len());
        assert_eq!(page.has_more, fact.sequence() < through);
        assert_eq!(
            page.next_after_sequence,
            (fact.sequence() < through).then_some(fact.sequence())
        );
        assert_eq!(page.window_complete, fact.sequence() == through);
        assert!(page.corrupt_tail.is_none());
        assert!(page.gaps.is_empty());
        after = page.scanned_through_sequence;
        rows.extend(page.rows);
    }
    assert_eq!(tree_bytes(state_root), before);
    assert_eq!(rows.len(), 4);
    for (row, fact) in rows
        .iter()
        .zip([&facts[1], &facts[3], &facts[4], &facts[5]])
    {
        assert_eq!(&row.event, fact);
        assert!(row.thread_identity.is_none());
    }
    let json = serde_json::to_value(&rows).expect("rows JSON");
    assert_eq!(json[0]["observation"]["kind"], "stutter");
    assert_eq!(json[0]["observation"]["frame_gap_ms"], 1_500);
    for field in [
        "capture_latency_ms",
        "recognition_latency_ms",
        "action_effect_latency_ms",
    ] {
        assert_eq!(
            json[0]["observation"].get(field),
            Some(&serde_json::Value::Null)
        );
    }
    assert_eq!(json[2]["observation"]["capture_latency_ms"], 120);
    assert_eq!(json[2]["observation"]["recognition_latency_ms"], 80);
    assert_eq!(json[2]["observation"]["action_effect_latency_ms"], 250);
    assert_eq!(json[1]["observation"]["kind"], "clock_jump");
    for field in [
        "magnitude_ms",
        "instance_id",
        "host_responsiveness_basis_points",
        "third_party_pressure_basis_points",
    ] {
        assert_eq!(
            json[1]["observation"].get(field),
            Some(&serde_json::Value::Null)
        );
    }
    assert_eq!(json[3]["observation"]["instance_id"], "instance:fixture-a");
    assert_eq!(
        json[3]["observation"]["host_responsiveness_basis_points"],
        9_000
    );
    assert_eq!(
        json[3]["observation"]["third_party_pressure_basis_points"],
        3_000
    );
    assert_eq!(
        json[3]["observation"].get("magnitude_ms"),
        Some(&serde_json::Value::Null)
    );
    assert_eq!(
        json[3].get("thread_identity"),
        Some(&serde_json::Value::Null)
    );

    writer.close().expect("close fixture writer");
    let segment = latest_segment(&ledger_root);
    let contents = fs::read_to_string(&segment).expect("segment text");
    let mut unknown: serde_json::Value =
        serde_json::from_str(contents.lines().last().expect("stored fact")).expect("stored JSON");
    unknown["event"]["unexpected_metric"] = serde_json::json!(42);
    let mut unknown_bytes = serde_json::to_vec(&unknown).expect("unknown field JSON");
    unknown_bytes.push(b'\n');
    append_bytes(&segment, &unknown_bytes);
    let corrupt_before = tree_bytes(state_root);
    for (after, requested_through, expected_count) in [(0, through, 6), (through, through + 2, 1)] {
        let output = run(ForensicRequest::performance(
            state_root,
            ForensicEventsRequest::new(
                ForensicEventFilter::default(),
                after,
                Some(requested_through),
                1_024,
            )
            .expect("tail page"),
        ))
        .expect("corrupt prefix report");
        let ForensicOutput::Machine(ForensicReport::Performance(page)) = output else {
            panic!("expected performance report");
        };
        assert_eq!(page.scanned_event_count, expected_count);
        assert!(!page.has_more);
        assert_eq!(page.next_after_sequence, None);
        assert!(!page.window_complete);
        assert!(page.gaps.contains(&"corrupt_tail"));
        let tail = page
            .corrupt_tail
            .expect("unknown field is visible corruption");
        assert_eq!(tail.code, "corrupt_segment");
        assert_eq!(tail.dangling_byte_count, unknown_bytes.len() as u64);
        assert_eq!(tail.byte_offset, contents.len() as u64);
        assert!(tail.tail_sha256.starts_with("sha256:"));
        assert_eq!(tail.tail_sha256.len(), 71);
        if after == 0 {
            assert_eq!((page.stutter_count, page.clock_jump_count), (2, 2));
            assert_eq!(page.rows, rows);
        } else {
            assert!(page.rows.is_empty());
            assert!(page.gaps.contains(&"through_sequence_unavailable"));
        }
    }
    assert_eq!(tree_bytes(state_root), corrupt_before);
    for (after, through, limit) in [(0, Some(6), 0), (0, None, 1_025), (7, Some(6), 1)] {
        assert!(
            ForensicEventsRequest::new(ForensicEventFilter::default(), after, through, limit)
                .is_err()
        );
    }
    for options in [
        ForensicEventsRequest::new(ForensicEventFilter::default(), u64::MAX, None, 1)
            .expect("deferred through"),
        ForensicEventsRequest::new(
            ForensicEventFilter::new(Some("runtime".to_owned()), None, None, None).expect("filter"),
            0,
            None,
            1,
        )
        .expect("filtered page"),
    ] {
        assert_eq!(
            run(ForensicRequest::performance(state_root, options))
                .expect_err("invalid performance page")
                .code(),
            "invalid_event_page"
        );
    }
    assert_eq!(tree_bytes(state_root), corrupt_before);
}

// Specification: #257-B8-STABILITY-READ-v1, leaf projection and bounded failures.
#[test]
fn stability_projection_preserves_facts_provenance_and_bounded_failures() {
    use actingcommand_artifact_store::{ArtifactStore, ArtifactWriteRequest};
    use actingcommand_contract::{ArtifactIssuePolicy, ArtifactKind, ArtifactProducer};
    use actingcommand_ledger_forensics::MAX_STABILITY_ARTIFACT_BYTES;
    struct Sink<'a>(&'a GlobalLedger);
    impl ArtifactEventSink for Sink<'_> {
        fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
            self.0
                .append(
                    draft
                        .sanitize(&Sha256SecretFingerprinter::new(b"stability-spec").expect("salt"))
                        .expect("sanitize"),
                )
                .map(|_| ())
                .map_err(|e| ArtifactStoreError::fatal(e.code(), "append_spec", "append failed"))
        }
    }
    let ids = IdentifierIssuer::new().expect("ids");
    let task = ids.mint_task_id().expect("task");
    let run_id = ids.mint_run_id().expect("run");
    let action = ids.mint_action_id().expect("action");
    let previous = ids.mint_frame_id().expect("previous");
    let current = ids.mint_frame_id().expect("current");
    let fact = serde_json::json!({
        "schema_version": "actingcommand.runtime.contained-task-stability-comparison.v1",
        "task_id": task.transport(), "run_id": run_id.transport(), "action_id": action.transport(),
        "step_index": 3, "operation_label": "neutral-step", "previous_frame_id": previous.transport(),
        "current_frame_id": current.transport(), "region": {"x": 7, "y": 11, "width": 23, "height": 29},
        "comparison_mode": "exact_pixels_v1", "comparison_parameters": {}, "result": "unchanged",
        "prior_consecutive_unchanged": 1, "new_consecutive_unchanged": 2,
        "consecutive_unchanged_threshold": 2, "terminal_reason": "consecutive_unchanged_threshold_reached"
    });
    let mut cases = vec![("valid".to_owned(), fact.clone(), None)];
    let mut changed = fact.clone();
    changed["result"] = "changed".into();
    changed["new_consecutive_unchanged"] = 0.into();
    changed["terminal_reason"] = serde_json::Value::Null;
    cases.push(("changed".to_owned(), changed, None));
    for field in fact
        .as_object()
        .expect("object")
        .keys()
        .filter(|key| *key != "schema_version")
    {
        let mut missing = fact.clone();
        missing.as_object_mut().expect("object").remove(field);
        cases.push((
            format!("missing-{field}"),
            missing,
            Some("stability_fields_invalid"),
        ));
    }
    for (name, field, value, code) in [
        (
            "version",
            "schema_version",
            serde_json::json!("actingcommand.runtime.contained-task-stability-comparison.v9"),
            "stability_schema_unsupported",
        ),
        (
            "schema",
            "schema_version",
            serde_json::Value::Null,
            "diagnostic_schema_unavailable",
        ),
        (
            "mode",
            "comparison_mode",
            serde_json::json!("unsupported"),
            "stability_fields_invalid",
        ),
        (
            "parameters",
            "comparison_parameters",
            serde_json::json!({"secret": "PRIVATE_DIAGNOSTIC"}),
            "stability_fields_invalid",
        ),
        (
            "links",
            "run_id",
            serde_json::json!(ids.mint_run_id().expect("other run").transport()),
            "stability_source_links_mismatch",
        ),
    ] {
        let mut value_fact = fact.clone();
        value_fact[field] = value;
        cases.push((name.to_owned(), value_fact, Some(code)));
    }
    cases.push(("unrelated".to_owned(), serde_json::json!({"schema_version":"another.diagnostic.v1", "private":"PRIVATE_DIAGNOSTIC"}), None));
    for (name, code) in [
        ("missing", "artifact_read_failed"),
        ("directory", "artifact_read_failed"),
        ("hash", "artifact_hash_mismatch"),
        ("declared-large", "stability_artifact_too_large"),
        ("stored-large", "stability_artifact_too_large"),
        ("redaction", "artifact_redaction_pending"),
        ("invalid-json", "diagnostic_json_invalid"),
    ] {
        cases.push((name.to_owned(), fact.clone(), Some(code)));
    }
    for (name, expected, error) in cases {
        let temp = tempfile::tempdir().expect("state");
        let root = temp.path();
        let ledger = GlobalLedger::open(GlobalLedgerConfig::new(
            root.join("ledger"),
            "stability-spec",
        ))
        .expect("ledger");
        let prefix = ledger
            .append(event(EventLinksDraft::default()))
            .expect("prefix");
        let store = ArtifactStore::open(root).expect("store");
        let mut bytes = serde_json::to_vec(&expected).expect("JSON");
        if name == "declared-large" {
            bytes.resize(MAX_STABILITY_ARTIFACT_BYTES as usize + 1, b' ');
        }
        if name == "invalid-json" {
            bytes = b"{PRIVATE_DIAGNOSTIC".to_vec();
        }
        let stored = store
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::DiagnosticJson,
                    &bytes,
                    ArtifactWriteContext::new(
                        ArtifactLinksDraft::default()
                            .with_run_id(run_id)
                            .with_frame_id(current),
                        EventLinksDraft::default()
                            .with_task_id(task)
                            .with_run_id(run_id)
                            .with_action_id(action)
                            .with_frame_id(current),
                        1_752_147_200_100,
                    ),
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::CapturePipeline,
                        RetentionClass::DebugFull,
                        if name == "redaction" {
                            ArtifactRedactionState::Pending
                        } else {
                            ArtifactRedactionState::NotRequired
                        },
                    ),
                ),
                &mut Sink(&ledger),
            )
            .expect("artifact");
        ledger.close().expect("close");
        if name == "missing" || name == "directory" {
            fs::remove_file(stored.path()).expect("remove fixture object");
        }
        if name == "directory" {
            fs::create_dir(stored.path()).expect("directory fixture");
        }
        if name == "hash" {
            bytes[0] ^= 1;
            fs::write(stored.path(), &bytes).expect("corrupt fixture");
        }
        if name == "stored-large" {
            fs::write(
                stored.path(),
                vec![b' '; MAX_STABILITY_ARTIFACT_BYTES as usize + 1],
            )
            .expect("oversized fixture");
        }
        let before = tree_bytes(root);
        let ForensicOutput::Machine(ForensicReport::Stability(report)) = run(
            ForensicRequest::stability(root, ForensicEventsRequest::default()),
        )
        .expect("report") else {
            panic!("stability report");
        };
        assert_eq!(tree_bytes(root), before, "{name}");
        let serialized = serde_json::to_string(&report).expect("output");
        assert!(!serialized.contains("PRIVATE_DIAGNOSTIC"), "{name}");
        assert_eq!(report.artifact_byte_limit, MAX_STABILITY_ARTIFACT_BYTES);
        if let Some(code) = error {
            assert_eq!(report.failures[0].code, code, "{name}");
            assert!(!report.window_complete, "{name}");
            assert!(report.rows.is_empty(), "{name}");
            assert_eq!(
                report.failures[0].artifact,
                stored.reference().project(true)
            );
            if [
                "missing",
                "directory",
                "hash",
                "declared-large",
                "stored-large",
            ]
            .contains(&name.as_str())
            {
                assert!(report.corrupt_tail.is_some(), "{name}");
                assert_eq!(report.scanned_through_sequence, prefix.sequence());
                assert_eq!(report.failures[0].source_sequence, None);
            } else {
                assert_eq!(
                    report.failures[0].source_sequence,
                    Some(prefix.sequence() + 1)
                );
            }
        } else if name == "unrelated" {
            assert_eq!(report.matched_count, 0);
            assert_eq!(report.scanned_diagnostic_count, 2);
            assert!(report.rows.is_empty());
            assert!(report.window_complete);
        } else {
            assert!(report.failures.is_empty(), "{name}");
            assert_eq!(report.matched_count, 2);
            assert_eq!(report.rows.len(), 2);
            for (offset, row) in report.rows.iter().enumerate() {
                assert_eq!(
                    serde_json::to_value(&row.comparison).expect("comparison"),
                    expected
                );
                assert_eq!(row.event.sequence(), prefix.sequence() + offset as u64 + 1);
                assert_eq!(row.artifact, stored.reference().project(true));
                assert_eq!(row.event.links().action_id(), Some(action.transport()));
            }
            assert!(report.window_complete);
        }
    }
}

fn event(links: EventLinksDraft) -> SanitizedEventDraft {
    EventDraft::new(
        event_id(),
        1_752_147_200_000,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
        links,
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"forensic-test-salt").expect("fingerprinter"))
    .expect("sanitize")
}

fn event_id() -> IssuedEventId {
    IdentifierIssuer::new()
        .expect("identifiers")
        .mint_event_id()
        .expect("event id")
}

fn latest_segment(root: &Path) -> PathBuf {
    let mut paths = fs::read_dir(root.join("segments"))
        .expect("read segments")
        .map(|entry| entry.expect("segment entry").path())
        .collect::<Vec<_>>();
    paths.sort();
    paths.pop().expect("latest segment")
}

fn append_bytes(path: &Path, bytes: &[u8]) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open segment")
        .write_all(bytes)
        .expect("append bytes");
}

#[derive(Debug, PartialEq, Eq)]
enum TreeObject {
    Bytes(Vec<u8>),
    Locked(u64),
}

fn tree_bytes(root: &Path) -> BTreeMap<PathBuf, TreeObject> {
    fn collect(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, TreeObject>) {
        let mut children = fs::read_dir(path)
            .expect("read tree")
            .map(|entry| entry.expect("tree entry").path())
            .collect::<Vec<_>>();
        children.sort();
        for child in children {
            if child.is_dir() {
                collect(root, &child, entries);
            } else {
                let relative = child
                    .strip_prefix(root)
                    .expect("relative path")
                    .to_path_buf();
                let object = match fs::read(&child) {
                    Ok(bytes) => TreeObject::Bytes(bytes),
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            || error.raw_os_error() == Some(33) =>
                    {
                        TreeObject::Locked(fs::metadata(&child).expect("locked metadata").len())
                    }
                    Err(error) => panic!("read tree file {}: {error}", child.display()),
                };
                entries.insert(relative, object);
            }
        }
    }

    let mut entries = BTreeMap::new();
    collect(root, root, &mut entries);
    entries
}
