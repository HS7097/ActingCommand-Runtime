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
