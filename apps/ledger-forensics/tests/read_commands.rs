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
    EventLinksDraft, EventOrigin, EventQuery, EventSeverity, EventSource, EventType,
    EvidenceCompleteness, IdentifierIssuer, OriginModule, ProjectionProfile, RetentionClass,
    TaskOutcome, TaskPayloadDraft,
};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerConfig, Sha256SecretFingerprinter};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

// Specification criterion 6: https://github.com/HS7097/ActingCommand-Workflow/issues/257#issuecomment-5552006104
#[test]
fn b3_actingledger_projects_resource_samples_and_unknowns() {
    use actingcommand_contract::{
        PerformanceContext, PerformanceLedgerSample, PerformanceLedgerWindow,
        PerformanceMonitorHealth, PerformancePayloadDraft, PerformanceProcessOwnership,
        PerformanceProcessSummary, PerformanceSummaryEventData,
    };
    let temp = tempfile::tempdir().expect("state root");
    let root = temp.path();
    let ledger = GlobalLedger::open(GlobalLedgerConfig::new(
        root.join("ledger"),
        "neutral-resource-cli",
    ))
    .expect("ledger");
    let ids = IdentifierIssuer::new().expect("ids");
    let append = |payload: actingcommand_contract::EventPayloadDraft| {
        ledger
            .append(
                EventDraft::new(
                    ids.mint_event_id().expect("event id"),
                    1_000,
                    EventSeverity::Info,
                    EventOrigin::new(
                        EventSource::Runtime,
                        OriginModule::PerformanceMonitor,
                        EventActor::Runtime,
                    ),
                    EventLinksDraft::default(),
                    payload,
                )
                .sanitize(
                    &Sha256SecretFingerprinter::new(b"neutral-resource-cli")
                        .expect("fingerprinter"),
                )
                .expect("sanitize"),
            )
            .expect("append")
    };
    let unrelated =
        append(CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into());
    let mut context = PerformanceContext::unavailable(1_000);
    context.sample_count = 1;
    context.health = PerformanceMonitorHealth::Partial;
    let mut summary = PerformanceSummaryEventData {
        context,
        foreground: None,
        owned_processes: vec![PerformanceProcessSummary {
            pid: 7,
            process_name: "neutral-process".to_owned(),
            ownership: PerformanceProcessOwnership::Runtime,
            cpu_basis_points: 100,
            working_set_bytes: 100,
            peak_working_set_bytes: Some(900),
            process_created_at_windows_100ns: Some(11),
            io_bytes_per_second: 0,
        }],
        third_party_high_load: Vec::new(),
        ledger_commits: Some(PerformanceLedgerSample::Available {
            window: PerformanceLedgerWindow {
                writer_id: *ids.mint_correlation_id().expect("writer id").transport(),
                start_monotonic_ns: 0,
                end_monotonic_ns: 1_000_000_000,
                first_sequence: Some(unrelated.sequence()),
                last_sequence: Some(unrelated.sequence()),
                successful_commits: 1,
                write_sync_total_ns: 20,
                writer_lifetime_write_sync_max_ns: 20,
                commits_per_second_milli: 1_000,
            },
        }),
    };
    let current =
        append(PerformancePayloadDraft::summary(summary.clone(), AuditInput::new()).into());
    summary.ledger_commits = None;
    summary.owned_processes[0].peak_working_set_bytes = None;
    summary.owned_processes[0].process_created_at_windows_100ns = None;
    let legacy = append(PerformancePayloadDraft::summary(summary, AuditInput::new()).into());
    assert!(
        !serde_json::to_string(&legacy)
            .expect("old shape")
            .contains("ledger_commits")
    );
    ledger.close().expect("close fixture");
    let ledger_root = root.join("ledger");
    let files: Vec<_> = fs::read_dir(&ledger_root)
        .expect("ledger files")
        .map(|entry| entry.expect("ledger entry").path())
        .filter(|path| path.is_file())
        .chain(
            fs::read_dir(ledger_root.join("segments"))
                .expect("segment files")
                .map(|entry| entry.expect("segment entry").path()),
        )
        .collect();
    let before: Vec<_> = files
        .iter()
        .map(|path| fs::read(path).expect("source bytes"))
        .collect();
    let binary = env!("CARGO_BIN_EXE_actingledger");
    let open = invoke(binary, root, &["open".to_owned()]);
    assert!(open.status.success(), "{open:?}");
    let open: serde_json::Value = serde_json::from_slice(&open.stdout).expect("open JSON");
    let storage = &open["data"]["storage_snapshot"];
    assert_eq!(storage["segment_count"], 1);
    assert!(storage["observed_bytes"].as_u64().expect("observed bytes") > 0);
    assert_eq!(storage["read_bytes"], storage["observed_bytes"]);
    assert_eq!(storage["verified_prefix_bytes"], storage["read_bytes"]);
    assert_eq!(storage["atomic"], false);
    let first = invoke(
        binary,
        root,
        &[
            "export".to_owned(),
            "--performance".to_owned(),
            "--limit".to_owned(),
            "1".to_owned(),
        ],
    );
    assert!(first.status.success(), "{first:?}");
    let first: serde_json::Value = serde_json::from_slice(&first.stdout).expect("first page");
    assert_eq!(first["data"]["rows"], serde_json::json!([]));
    assert_eq!(first["data"]["next_after_sequence"], unrelated.sequence());
    let page = invoke(
        binary,
        root,
        &[
            "export".to_owned(),
            "--performance".to_owned(),
            "--after".to_owned(),
            unrelated.sequence().to_string(),
            "--through".to_owned(),
            legacy.sequence().to_string(),
            "--limit".to_owned(),
            "2".to_owned(),
        ],
    );
    assert!(page.status.success(), "{page:?}");
    assert!(page.stderr.is_empty());
    let page: serde_json::Value = serde_json::from_slice(&page.stdout).expect("resource page");
    let data = &page["data"];
    assert_eq!(data["summary_count"], 2);
    assert_eq!(data["scanned_event_count"], 2);
    assert_eq!(data["next_after_sequence"], serde_json::Value::Null);
    assert_eq!(data["window_complete"], true);
    let rows = &data["rows"];
    assert_eq!(
        rows[0]["event"],
        serde_json::to_value(current).expect("source event")
    );
    assert_eq!(rows[0]["observation"]["kind"], "resource_sample");
    assert_eq!(
        rows[0]["observation"]["ledger_commits"]["window"]["commits_per_second_milli"],
        1_000
    );
    assert_eq!(
        rows[0]["observation"]["owned_processes"][0]["peak_working_set_bytes"],
        900
    );
    assert_eq!(
        rows[0]["observation"]["owned_processes"][0]["process_created_at_windows_100ns"],
        11
    );
    assert_eq!(
        rows[1]["event"],
        serde_json::to_value(legacy).expect("legacy source event")
    );
    assert_eq!(
        rows[1]["observation"].get("ledger_commits"),
        Some(&serde_json::Value::Null)
    );
    for field in ["peak_working_set_bytes", "process_created_at_windows_100ns"] {
        assert_eq!(
            rows[1]["observation"]["owned_processes"][0].get(field),
            Some(&serde_json::Value::Null)
        );
    }
    let ordinary = invoke(binary, root, &["export".to_owned()]);
    assert!(ordinary.status.success(), "{ordinary:?}");
    assert!(
        std::str::from_utf8(&ordinary.stdout)
            .expect("human export")
            .contains("storage_snapshot:")
    );
    let after: Vec<_> = files
        .iter()
        .map(|path| fs::read(path).expect("preserved source bytes"))
        .collect();
    assert_eq!(after, before);
}

#[test]
fn actingledger_read_commands_are_thin_and_fail_loud() {
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
    let writer = GlobalLedger::open(GlobalLedgerConfig::new(&ledger_root, "cli-fixture"))
        .expect("open ledger");
    let event_id = identifiers.mint_event_id().expect("event id");
    let event = EventDraft::new(
        event_id,
        1_752_147_200_000,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
        EventLinksDraft::default().with_request_id(request_id),
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"actingledger-test-salt").expect("fingerprinter"))
    .expect("sanitize");
    writer.append(event).expect("append event");
    writer.close().expect("close writer");

    let binary = env!("CARGO_BIN_EXE_actingledger");
    let commands = [
        vec!["open".to_owned()],
        vec!["events".to_owned()],
        vec!["chain".to_owned(), "--req".to_owned(), request_text],
        vec!["tail".to_owned()],
        vec!["repairs".to_owned()],
        vec!["export".to_owned()],
    ];
    for (index, command) in commands.iter().enumerate() {
        let first = invoke(binary, state_root, command);
        let second = invoke(binary, state_root, command);
        assert!(first.status.success(), "valid command failed: {first:?}");
        assert_eq!(first.stdout, second.stdout);
        assert!(first.stderr.is_empty());
        if index < 5 {
            let value: serde_json::Value =
                serde_json::from_slice(&first.stdout).expect("machine JSON");
            assert_eq!(value["command"], command[0]);
        } else {
            assert!(
                std::str::from_utf8(&first.stdout)
                    .expect("UTF-8 export")
                    .starts_with("ActingCommand ledger forensic export\n")
            );
        }
    }

    for invalid in [
        Vec::<&str>::new(),
        vec!["--state-root"],
        vec![
            "--state-root",
            state_root.to_str().expect("state root"),
            "unknown",
        ],
        vec![
            "--state-root",
            state_root.to_str().expect("state root"),
            "chain",
        ],
        vec![
            "--state-root",
            state_root.to_str().expect("state root"),
            "events",
            "--module",
            "runtime_host",
        ],
    ] {
        let output = Command::new(binary)
            .args(invalid)
            .output()
            .expect("run invalid command");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }

    let args = [
        OsString::from("--state-root"),
        state_root.as_os_str().to_owned(),
        OsString::from("open"),
    ];
    let error = actingledger::run(args, &mut FailingWriter).expect_err("output failure");
    assert_eq!(error.code(), "output_failed");

    let main_source = include_str!("../src/main.rs");
    assert_eq!(main_source.matches("actingledger::run_env()").count(), 1);
    for forbidden in ["GlobalLedger", "EventQuery", "serde_json"] {
        assert!(!main_source.contains(forbidden));
    }
    let cli_source = include_str!("../src/lib.rs");
    assert_eq!(
        cli_source
            .matches("actingcommand_ledger_forensics::run(request)")
            .count(),
        1
    );
    for forbidden in ["GlobalLedger", "EventQuery", "PersistedEvent"] {
        assert!(!cli_source.contains(forbidden));
    }
}

#[test]
fn events_cli_parses_bounded_filters_and_reports_next_cursor() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_root = temp.path();
    let ledger_root = state_root.join("ledger");
    let identifiers = IdentifierIssuer::new().expect("identifiers");
    let correlation_id = identifiers.mint_correlation_id().expect("correlation id");
    let correlation_text = serde_json::to_value(correlation_id.transport())
        .expect("correlation JSON")
        .as_str()
        .expect("correlation string")
        .to_owned();
    let writer = GlobalLedger::open(GlobalLedgerConfig::new(&ledger_root, "cli-filter-fixture"))
        .expect("open ledger");
    for index in 0..2 {
        let event = EventDraft::new(
            identifiers.mint_event_id().expect("event id"),
            1_752_147_200_000 + index,
            EventSeverity::Error,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            EventLinksDraft::default().with_correlation_id(correlation_id),
            CommandPayloadDraft::rejected(
                EventAction::RuntimeStart,
                DiagnosticCode::CommandRejected,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            )
            .into(),
        )
        .sanitize(
            &Sha256SecretFingerprinter::new(b"actingledger-filter-test-salt")
                .expect("fingerprinter"),
        )
        .expect("sanitize");
        writer.append(event).expect("append event");
    }
    writer.close().expect("close writer");

    let binary = env!("CARGO_BIN_EXE_actingledger");
    let command = vec![
        "events".to_owned(),
        "--after".to_owned(),
        "0".to_owned(),
        "--through".to_owned(),
        "2".to_owned(),
        "--limit".to_owned(),
        "1".to_owned(),
        "--origin-module".to_owned(),
        "runtime".to_owned(),
        "--diagnostic-code".to_owned(),
        "command.rejected".to_owned(),
        "--severity".to_owned(),
        "error".to_owned(),
        "--correlation-id".to_owned(),
        correlation_text.clone(),
    ];
    let first = invoke(binary, state_root, &command);
    assert!(first.status.success(), "filtered events failed: {first:?}");
    assert!(first.stderr.is_empty());
    let first: serde_json::Value = serde_json::from_slice(&first.stdout).expect("events JSON");
    assert_eq!(first["command"], "events");
    assert_eq!(first["data"]["filter"]["origin_module"], "runtime");
    assert_eq!(
        first["data"]["filter"]["diagnostic_code"],
        "command.rejected"
    );
    assert_eq!(first["data"]["filter"]["severity"], "error");
    assert_eq!(first["data"]["filter"]["correlation_id"], correlation_text);
    assert_eq!(first["data"]["after_sequence"], 0);
    assert_eq!(first["data"]["through_sequence"], 2);
    assert_eq!(first["data"]["limit"], 1);
    assert_eq!(first["data"]["events"][0]["sequence"], 1);
    assert_eq!(first["data"]["next_after_sequence"], 1);

    let continuation = invoke(
        binary,
        state_root,
        &[
            "events".to_owned(),
            "--after".to_owned(),
            "1".to_owned(),
            "--through".to_owned(),
            "2".to_owned(),
            "--limit".to_owned(),
            "1".to_owned(),
            "--origin-module".to_owned(),
            "runtime".to_owned(),
            "--diagnostic-code".to_owned(),
            "command.rejected".to_owned(),
            "--severity".to_owned(),
            "error".to_owned(),
            "--correlation-id".to_owned(),
            correlation_text,
        ],
    );
    assert!(
        continuation.status.success(),
        "filtered continuation failed: {continuation:?}"
    );
    let continuation: serde_json::Value =
        serde_json::from_slice(&continuation.stdout).expect("continuation JSON");
    assert_eq!(continuation["data"]["events"][0]["sequence"], 2);
    assert!(continuation["data"].get("next_after_sequence").is_none());

    let missing_root = state_root.join("does-not-exist");
    let invalid_commands = [
        vec!["events", "--limit", "1", "--limit", "2"],
        vec!["events", "--unknown", "value"],
        vec!["events", "--limit"],
        vec!["events", "--severity", "urgent"],
        vec!["events", "--correlation-id", "correlation_not_hex"],
        vec!["events", "--after", "not-a-number"],
        vec!["events", "--limit", "0"],
        vec!["events", "--limit", "1025"],
        vec!["events", "--after", "3", "--through", "2"],
    ];
    for command in invalid_commands {
        let command = command.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let output = invoke(binary, &missing_root, &command);
        assert!(
            !output.status.success(),
            "invalid command passed: {command:?}"
        );
        assert!(output.stdout.is_empty());
        assert!(
            std::str::from_utf8(&output.stderr)
                .expect("UTF-8 stderr")
                .contains("invalid_arguments"),
            "unexpected invalid-command error: {output:?}"
        );
    }
}

#[test]
fn replay_cli_requires_the_external_receipt_and_reports_verified_manifest() {
    struct LedgerSink<'a> {
        ledger: &'a GlobalLedger,
    }

    impl ArtifactEventSink for LedgerSink<'_> {
        fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
            let event = draft
                .sanitize(
                    &Sha256SecretFingerprinter::new(b"replay-cli-sink").expect("fingerprinter"),
                )
                .map_err(|error| {
                    ArtifactStoreError::fatal(
                        "event_sanitize_failed",
                        "append_replay_cli_fixture_event",
                        error.to_string(),
                    )
                })?;
            self.ledger.append(event).map(|_| ()).map_err(|error| {
                ArtifactStoreError::fatal(
                    error.code(),
                    "append_replay_cli_fixture_event",
                    error.to_string(),
                )
            })
        }
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn collect(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            let mut entries = fs::read_dir(path)
                .expect("read snapshot directory")
                .map(|entry| entry.expect("snapshot entry").path())
                .collect::<Vec<_>>();
            entries.sort();
            for entry in entries {
                if entry.is_dir() {
                    collect(root, &entry, files);
                } else {
                    files.insert(
                        entry
                            .strip_prefix(root)
                            .expect("relative path")
                            .to_path_buf(),
                        fs::read(&entry).expect("snapshot bytes"),
                    );
                }
            }
        }

        let mut files = BTreeMap::new();
        collect(root, root, &mut files);
        files
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
        "replay-cli-fixture",
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
                &Sha256SecretFingerprinter::new(b"replay-cli-fixture").expect("fingerprinter"),
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
                &Sha256SecretFingerprinter::new(b"replay-cli-fixture").expect("fingerprinter"),
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
        documents: EvidenceExportDocuments::new(
            EvidenceJsonDocument::from_serializable(&serde_json::json!({ "status": "result" }))
                .expect("result document"),
            EvidenceJsonDocument::from_serializable(
                &serde_json::json!({ "status": "diagnostics" }),
            )
            .expect("diagnostics document"),
            "forensic replay CLI fixture",
        )
        .expect("documents"),
        archive_context: ArtifactWriteContext::new(
            ArtifactLinksDraft::default()
                .with_run_id(run_id)
                .with_correlation_id(correlation_id),
            links,
            1_752_147_200_200,
        ),
    };
    let mut exporter = EvidenceExporter::open(temp.path().join("artifacts")).expect("exporter");
    let receipt = exporter
        .export(request, &mut LedgerSink { ledger: &ledger })
        .expect("sealed export");
    ledger.close().expect("close fixture ledger");
    let expected =
        verify_evidence_archive(&archive, receipt.zip_sha256()).expect("canonical verification");
    let archive_bytes = fs::read(&archive).expect("archive bytes");
    let before = snapshot(temp.path());
    let binary = env!("CARGO_BIN_EXE_actingledger");

    let invoke_replay = || {
        Command::new(binary)
            .arg("replay")
            .arg("--zip")
            .arg(&archive)
            .arg("--expected-sha256")
            .arg(receipt.zip_sha256())
            .output()
            .expect("run replay")
    };
    let first = invoke_replay();
    let second = invoke_replay();
    assert!(first.status.success(), "replay failed: {first:?}");
    assert_eq!(first.stdout, second.stdout);
    assert!(first.stderr.is_empty());
    assert!(second.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&first.stdout).expect("replay JSON");
    assert_eq!(report["command"], "replay");
    assert_eq!(
        report["data"]["verifier"],
        "actingcommand_artifact_store::verify_evidence_archive"
    );
    assert_eq!(report["data"]["zip_byte_count"], expected.zip_byte_count);
    assert_eq!(report["data"]["zip_sha256"], expected.zip_sha256);
    assert_eq!(report["data"]["manifest_sha256"], expected.manifest_sha256);
    assert_eq!(
        report["data"]["manifest"],
        serde_json::to_value(&expected.manifest).expect("manifest JSON")
    );
    assert_eq!(
        fs::read(&archive).expect("archive after replay"),
        archive_bytes
    );
    assert_eq!(snapshot(temp.path()), before);

    let invalid_commands = [
        vec!["replay"],
        vec!["replay", "--zip", archive.to_str().expect("archive path")],
        vec![
            "replay",
            "--zip",
            archive.to_str().expect("archive path"),
            "--zip",
            archive.to_str().expect("archive path"),
            "--expected-sha256",
            receipt.zip_sha256(),
        ],
        vec![
            "replay",
            "--zip",
            archive.to_str().expect("archive path"),
            "--expected-sha256",
            receipt.zip_sha256(),
            "--expected-sha256",
            receipt.zip_sha256(),
        ],
        vec!["replay", "--unknown", "value"],
        vec![
            "replay",
            "--state-root",
            temp.path().to_str().expect("temp path"),
            "--zip",
            archive.to_str().expect("archive path"),
            "--expected-sha256",
            receipt.zip_sha256(),
        ],
    ];
    for command in invalid_commands {
        let output = Command::new(binary)
            .args(command)
            .output()
            .expect("run invalid replay");
        assert!(
            !output.status.success(),
            "invalid replay passed: {output:?}"
        );
        assert!(output.stdout.is_empty());
        assert!(
            std::str::from_utf8(&output.stderr)
                .expect("UTF-8 stderr")
                .contains("invalid_arguments")
        );
    }

    let missing = temp.path().join("missing.zip");
    let malformed = Command::new(binary)
        .arg("replay")
        .arg("--zip")
        .arg(&missing)
        .arg("--expected-sha256")
        .arg("not-a-sha256")
        .output()
        .expect("run malformed replay");
    assert!(!malformed.status.success());
    assert!(malformed.stdout.is_empty());
    let malformed_error = std::str::from_utf8(&malformed.stderr).expect("malformed stderr");
    assert!(malformed_error.contains("sha256_invalid"));
    assert!(!malformed_error.contains("evidence_archive_read_failed"));

    let absent = Command::new(binary)
        .arg("replay")
        .arg("--zip")
        .arg(&missing)
        .arg("--expected-sha256")
        .arg(receipt.zip_sha256())
        .output()
        .expect("run absent replay");
    assert!(!absent.status.success());
    assert!(absent.stdout.is_empty());
    assert!(
        std::str::from_utf8(&absent.stderr)
            .expect("absent stderr")
            .contains("evidence_archive_read_failed")
    );

    let mismatch = Command::new(binary)
        .arg("replay")
        .arg("--zip")
        .arg(&archive)
        .arg("--expected-sha256")
        .arg("0".repeat(64))
        .output()
        .expect("run mismatched replay");
    assert!(!mismatch.status.success());
    assert!(mismatch.stdout.is_empty());
    assert!(
        std::str::from_utf8(&mismatch.stderr)
            .expect("mismatch stderr")
            .contains("evidence_archive_hash_mismatch")
    );

    let corrupt = temp.path().join("corrupt.zip");
    fs::write(&corrupt, b"not a ZIP archive").expect("write corrupt archive");
    let corrupt_output = Command::new(binary)
        .arg("replay")
        .arg("--zip")
        .arg(&corrupt)
        .arg("--expected-sha256")
        .arg("0".repeat(64))
        .output()
        .expect("run corrupt replay");
    assert!(!corrupt_output.status.success());
    assert!(corrupt_output.stdout.is_empty());
    assert!(
        std::str::from_utf8(&corrupt_output.stderr)
            .expect("corrupt stderr")
            .contains("evidence_archive_invalid")
    );
    assert_eq!(
        fs::read(&archive).expect("final archive bytes"),
        archive_bytes
    );
    assert!(!temp.path().join("sealed-evidence").exists());

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;

        let invalid_utf8 = OsString::from_wide(&[0xd800]);
        let error = actingledger::run(
            [
                OsString::from("replay"),
                OsString::from("--zip"),
                invalid_utf8,
                OsString::from("--expected-sha256"),
                OsString::from(receipt.zip_sha256()),
            ],
            &mut Vec::new(),
        )
        .expect_err("non-UTF-8 replay path");
        assert_eq!(error.code(), "invalid_arguments");
    }
}

#[test]
fn performance_export_is_explicit_bounded_and_preserves_ordinary_export() {
    use actingcommand_contract::{
        EventPayloadDraft, PerformanceControlEventData, PerformanceControlLevel,
        PerformanceControlReason, PerformancePayloadDraft, PerformanceStutterEventData,
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let state_root = temp.path();
    let identifiers = IdentifierIssuer::new().expect("identifiers");
    let request_id = identifiers.mint_request_id().expect("request id");
    let writer = GlobalLedger::open(GlobalLedgerConfig::new(
        state_root.join("ledger"),
        "performance-cli-fixture",
    ))
    .expect("open fixture ledger");
    let payloads: [EventPayloadDraft; 3] = [
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
        PerformancePayloadDraft::stutter_detected(
            PerformanceStutterEventData {
                instance_id: "instance:fixture-a".to_owned(),
                observed_at_unix_ms: 1_752_147_201_000,
                frame_gap_ms: 1_500,
                capture_latency_ms: Some(120),
                recognition_latency_ms: None,
                action_effect_latency_ms: Some(250),
            },
            AuditInput::new(),
        )
        .into(),
        PerformancePayloadDraft::balance_changed(
            PerformanceControlEventData {
                observed_at_unix_ms: 1_752_147_202_000,
                instance_id: None,
                previous_level: PerformanceControlLevel::Normal,
                level: PerformanceControlLevel::Normal,
                reason: PerformanceControlReason::ClockJump,
                host_responsiveness_basis_points: None,
                third_party_pressure_basis_points: None,
                recovery: false,
                deadline_disposition: None,
            },
            AuditInput::new(),
        )
        .into(),
    ];
    let mut facts = Vec::new();
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
        .sanitize(&Sha256SecretFingerprinter::new(b"performance-cli-salt").expect("fingerprinter"))
        .expect("sanitize fixture event");
        facts.push(writer.append(draft).expect("append fixture event"));
    }
    writer.close().expect("close fixture writer");
    let binary = env!("CARGO_BIN_EXE_actingledger");
    let ordinary = invoke(binary, state_root, &["export".to_owned()]);
    assert!(ordinary.status.success(), "ordinary export: {ordinary:?}");
    assert!(ordinary.stderr.is_empty());
    assert!(
        std::str::from_utf8(&ordinary.stdout)
            .expect("ordinary text")
            .starts_with("ActingCommand ledger forensic export\n")
    );

    let first = invoke(
        binary,
        state_root,
        &[
            "export".to_owned(),
            "--performance".to_owned(),
            "--limit".to_owned(),
            "1".to_owned(),
        ],
    );
    assert!(first.status.success(), "first performance page: {first:?}");
    assert!(first.stderr.is_empty());
    let first: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("machine performance JSON");
    assert_eq!(first["command"], "performance");
    assert_eq!(first["data"]["scanned_event_count"], 1);
    assert_eq!(first["data"]["rows"], serde_json::json!([]));
    assert_eq!(first["data"]["has_more"], true);
    assert_eq!(first["data"]["window_complete"], false);
    assert_eq!(first["data"]["through_sequence"], facts[2].sequence());
    assert_eq!(first["data"]["next_after_sequence"], facts[0].sequence());
    let next = invoke(
        binary,
        state_root,
        &[
            "export".to_owned(),
            "--performance".to_owned(),
            "--after".to_owned(),
            first["data"]["next_after_sequence"].to_string(),
            "--through".to_owned(),
            first["data"]["through_sequence"].to_string(),
            "--limit".to_owned(),
            "2".to_owned(),
        ],
    );
    assert!(next.status.success(), "next performance page: {next:?}");
    assert!(next.stderr.is_empty());
    let next: serde_json::Value = serde_json::from_slice(&next.stdout).expect("next machine JSON");
    let data = &next["data"];
    assert_eq!(data["scanned_event_count"], 2);
    assert_eq!(data["scanned_through_sequence"], facts[2].sequence());
    assert_eq!(data["stutter_count"], 1);
    assert_eq!(data["clock_jump_count"], 1);
    assert_eq!(data["has_more"], false);
    assert_eq!(data["window_complete"], true);
    assert_eq!(
        data.get("next_after_sequence"),
        Some(&serde_json::Value::Null)
    );
    assert_eq!(data.get("corrupt_tail"), Some(&serde_json::Value::Null));
    assert_eq!(data["gaps"], serde_json::json!([]));
    assert_eq!(
        data["rows"][0]["event"],
        serde_json::to_value(&facts[1]).expect("stutter fact JSON")
    );
    assert_eq!(
        data["rows"][1]["event"],
        serde_json::to_value(&facts[2]).expect("clock fact JSON")
    );
    assert_eq!(data["rows"][0]["observation"]["frame_gap_ms"], 1_500);
    assert_eq!(data["rows"][0]["observation"]["capture_latency_ms"], 120);
    assert_eq!(
        data["rows"][0]["observation"].get("recognition_latency_ms"),
        Some(&serde_json::Value::Null)
    );
    assert_eq!(
        data["rows"][1]["observation"].get("magnitude_ms"),
        Some(&serde_json::Value::Null)
    );
    for row in data["rows"].as_array().expect("rows array") {
        assert_eq!(row.get("thread_identity"), Some(&serde_json::Value::Null));
    }

    for arguments in [
        vec!["export", "--limit", "1"],
        vec!["export", "--performance", "--performance"],
        vec!["export", "--performance", "--limit"],
        vec!["export", "--performance", "--limit", "0"],
        vec!["export", "--performance", "--limit", "1025"],
        vec!["export", "--performance", "--limit", "1", "--limit", "2"],
        vec!["export", "--performance", "--after", "4", "--through", "3"],
        vec!["export", "--performance", "--after", "18446744073709551615"],
        vec!["export", "--performance", "--through", "invalid"],
        vec!["export", "--performance", "--origin-module", "runtime"],
        vec!["export", "--performance", "--severity", "info"],
        vec!["events", "--performance"],
        vec!["performance"],
    ] {
        let arguments = arguments.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let output = invoke(binary, state_root, &arguments);
        assert!(
            !output.status.success(),
            "invalid command accepted: {arguments:?}"
        );
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
    let after = invoke(binary, state_root, &["export".to_owned()]);
    assert!(after.status.success(), "final ordinary export: {after:?}");
    assert!(after.stderr.is_empty());
    assert_eq!(after.stdout, ordinary.stdout);
}

// Specification: #257-B8-STABILITY-READ-v1, official CLI pagination and source preservation.
#[test]
fn stability_cli_pages_errors_and_source_files_are_explicit() {
    use actingcommand_artifact_store::{ArtifactStore, ArtifactWriteRequest};
    use actingcommand_contract::{ArtifactIssuePolicy, ArtifactKind, ArtifactProducer};
    struct Sink<'a>(&'a GlobalLedger);
    impl ArtifactEventSink for Sink<'_> {
        fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
            self.0
                .append(
                    draft
                        .sanitize(&Sha256SecretFingerprinter::new(b"stability-cli").expect("salt"))
                        .expect("sanitize"),
                )
                .map(|_| ())
                .map_err(|e| ArtifactStoreError::fatal(e.code(), "append_spec", "append failed"))
        }
    }
    let temp = tempfile::tempdir().expect("state");
    let root = temp.path();
    let ids = IdentifierIssuer::new().expect("ids");
    let task = ids.mint_task_id().expect("task");
    let run_id = ids.mint_run_id().expect("run");
    let action = ids.mint_action_id().expect("action");
    let previous = ids.mint_frame_id().expect("previous");
    let current = ids.mint_frame_id().expect("current");
    let ledger = GlobalLedger::open(GlobalLedgerConfig::new(
        root.join("ledger"),
        "stability-cli",
    ))
    .expect("ledger");
    let unrelated = || {
        EventDraft::new(
            ids.mint_event_id().expect("event"),
            1_752_147_200_001,
            EventSeverity::Info,
            EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
            EventLinksDraft::default(),
            CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
        )
        .sanitize(&Sha256SecretFingerprinter::new(b"stability-cli").expect("salt"))
        .expect("sanitize")
    };
    let first = ledger.append(unrelated()).expect("unrelated prefix");
    let fact = serde_json::json!({
        "schema_version": "actingcommand.runtime.contained-task-stability-comparison.v1",
        "task_id": task.transport(), "run_id": run_id.transport(), "action_id": action.transport(),
        "step_index": 1, "operation_label": "neutral", "previous_frame_id": previous.transport(),
        "current_frame_id": current.transport(), "region": {"x": 1, "y": 2, "width": 3, "height": 4},
        "comparison_mode": "exact_pixels_v1", "comparison_parameters": {}, "result": "changed",
        "prior_consecutive_unchanged": 1, "new_consecutive_unchanged": 0,
        "consecutive_unchanged_threshold": 2, "terminal_reason": null
    });
    let store = ArtifactStore::open(root).expect("store");
    let mut references = Vec::new();
    for schema in [
        "actingcommand.runtime.contained-task-stability-comparison.v1",
        "actingcommand.runtime.contained-task-stability-comparison.v99",
    ] {
        let mut input = fact.clone();
        input["schema_version"] = schema.into();
        let stored = store
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::DiagnosticJson,
                    &serde_json::to_vec(&input).expect("JSON"),
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
                        ArtifactRedactionState::NotRequired,
                    ),
                ),
                &mut Sink(&ledger),
            )
            .expect("artifact");
        references.push(stored.reference().project(true));
    }
    let excluded = ledger.append(unrelated()).expect("outside frozen interval");
    let through = excluded.sequence() - 1;
    ledger.close().expect("close");
    let snapshot = || {
        let mut files = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(path) = pending.pop() {
            for entry in fs::read_dir(path).expect("tree") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    files.insert(path.clone(), fs::read(path).expect("source bytes"));
                }
            }
        }
        files
    };
    let before = snapshot();
    let binary = env!("CARGO_BIN_EXE_actingledger");
    let mut after = 0;
    let mut emitted = Vec::new();
    loop {
        let output = invoke(
            binary,
            root,
            &[
                "export".into(),
                "--stability".into(),
                "--after".into(),
                after.to_string(),
                "--through".into(),
                through.to_string(),
                "--limit".into(),
                "1".into(),
            ],
        );
        let json: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("structured report even on failure");
        assert_eq!(json["command"], "stability");
        let page = &json["data"];
        assert_eq!(page["scanned_event_count"], 1);
        assert_eq!(page["through_sequence"], through);
        assert_eq!(page["scanned_through_sequence"], after + 1);
        if after == 0 {
            assert_eq!(page["matched_count"], 0);
            assert_eq!(page["rows"], serde_json::json!([]));
            assert_eq!(page["next_after_sequence"], first.sequence());
        } else if after < 3 {
            assert_eq!(page["rows"][0]["comparison"], fact);
            assert_eq!(
                page["rows"][0]["artifact"],
                serde_json::to_value(&references[0]).expect("reference")
            );
            emitted.push(
                page["rows"][0]["event"]["sequence"]
                    .as_u64()
                    .expect("sequence"),
            );
        } else {
            assert_eq!(page["failures"][0]["code"], "stability_schema_unsupported");
            assert_eq!(page["failures"][0]["source_sequence"], after + 1);
            assert_eq!(page["window_complete"], false);
        }
        assert_eq!(output.status.success(), after < 3, "{output:?}");
        if after < 3 {
            assert!(output.stderr.is_empty());
        } else {
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("stability_export_incomplete")
            );
        }
        assert_eq!(snapshot(), before);
        if page["has_more"] == false {
            assert!(page["next_after_sequence"].is_null());
            break;
        }
        after = page["next_after_sequence"].as_u64().expect("cursor");
    }
    assert_eq!(emitted, vec![first.sequence() + 1, first.sequence() + 2]);
    for invalid in [
        vec!["export", "--stability", "--performance"],
        vec!["export", "--stability", "--limit", "0"],
        vec!["export", "--stability", "--limit", "1025"],
        vec!["export", "--stability", "--after", "4", "--through", "3"],
        vec!["export", "--stability", "--severity", "info"],
        vec!["events", "--stability"],
    ] {
        let output = invoke(
            binary,
            root,
            &invalid.into_iter().map(str::to_owned).collect::<Vec<_>>(),
        );
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("invalid_arguments"));
    }
    assert_eq!(snapshot(), before);
    let segment = fs::read_dir(root.join("ledger/segments"))
        .expect("segments")
        .map(|entry| entry.expect("segment").path())
        .max()
        .expect("segment");
    fs::OpenOptions::new()
        .append(true)
        .open(segment)
        .expect("append fixture tail")
        .write_all(b"{\"partial\":")
        .expect("partial tail");
    let corrupt_before = snapshot();
    let output = invoke(
        binary,
        root,
        &[
            "export".into(),
            "--stability".into(),
            "--through".into(),
            "1".into(),
        ],
    );
    assert!(!output.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("bad tail report");
    assert!(report["data"]["corrupt_tail"].is_object());
    assert_eq!(report["data"]["window_complete"], false);
    assert_eq!(snapshot(), corrupt_before);
}

fn invoke(binary: &str, state_root: &std::path::Path, command: &[String]) -> std::process::Output {
    Command::new(binary)
        .arg("--state-root")
        .arg(state_root)
        .args(command)
        .output()
        .expect("run actingledger")
}

struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed output"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
