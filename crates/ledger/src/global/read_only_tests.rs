// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    AuditInput, CommandPayloadDraft, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventQuery, EventSeverity, EventSource, EventType, IdentifierIssuer,
    IssuedCorrelationId, IssuedEventId, OriginModule, SanitizedEventDraft,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

// The first bytes of a record line whose write a crash cut short.
const TORN_TAIL: &[u8] = br#"{"line_type":"event","event":{"schema_version""#;

// A repair record as the read-only open reports it: schema version, repair id, completed,
// segment index, original length, repaired length, tail hash and quarantine key.
type RepairView = (String, String, bool, u64, u64, u64, String, String);

// Specification criterion 5: https://github.com/HS7097/ActingCommand-Workflow/issues/257#issuecomment-5552006104
#[test]
fn b3_storage_snapshot_keeps_physical_tail_and_unverified_segments() {
    let temp = tempfile::tempdir().expect("root");
    let root = temp.path();
    let ledger = GlobalLedger::open(
        GlobalLedgerConfig::new(root, "neutral-storage").with_segment_max_bytes(128),
    )
    .expect("ledger");
    let empty = GlobalLedger::open_read_only(GlobalLedgerReadOnlyConfig::new(root), |_| None)
        .expect("empty physical segment");
    assert_eq!(empty.storage_snapshot().segment_count, 1);
    assert_eq!(empty.storage_snapshot().observed_bytes, 0);
    assert_eq!(empty.latest_sequence(), 0);
    let first = ledger
        .append(event(EventLinksDraft::default()))
        .expect("first");
    ledger
        .append(event(EventLinksDraft::default()))
        .expect("second");
    ledger
        .append(event(EventLinksDraft::default()))
        .expect("third");
    ledger.close().expect("close writer");
    // Specification: native-reader byte, event and deadline budgets of the segment reader.
    let before_budget_reads = tree_bytes(root);
    for (bytes, events, deadline) in [
        (
            0,
            100,
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        ),
        (
            1024 * 1024,
            0,
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        ),
        (1024 * 1024, 100, std::time::Instant::now()),
    ] {
        let result = GlobalLedger::open_read_only(
            GlobalLedgerReadOnlyConfig::new(root).with_budget(bytes, events, deadline),
            |_| None,
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("exhausted budget accepted"),
        };
        assert_eq!(error.code(), "ledger_read_budget_exceeded");
    }
    assert_eq!(tree_bytes(root), before_budget_reads);
    let paths = segment_paths(root);
    assert_eq!(paths.len(), 3);
    let valid_prefix_bytes = fs::metadata(&paths[0])
        .expect("first physical length")
        .len();
    append_bytes(&paths[0], b"not-json\n");
    let before = tree_bytes(root);
    let source_hashes: Vec<_> = paths
        .iter()
        .map(|path| sha256(&fs::read(path).expect("source")))
        .collect();
    let total_bytes: u64 = paths
        .iter()
        .map(|path| fs::metadata(path).expect("physical size").len())
        .sum();
    let snapshot = GlobalLedger::open_read_only(GlobalLedgerReadOnlyConfig::new(root), |_| None)
        .expect("physical snapshot with bad prefix tail");
    assert_eq!(snapshot.events(), &[first]);
    assert_eq!(
        snapshot
            .corrupt_tail()
            .expect("bad first segment")
            .segment_index,
        1
    );
    assert_eq!(snapshot.listed_through_segment(), Some(3));
    let physical = snapshot.storage_snapshot();
    assert_eq!(physical.segment_count, 3);
    assert_eq!(physical.observed_bytes, total_bytes);
    assert_eq!(physical.read_bytes, total_bytes);
    assert_eq!(physical.verified_prefix_bytes, valid_prefix_bytes);
    assert!(physical.read_complete);
    assert!(!physical.atomic);
    assert_eq!(tree_bytes(root), before);
    assert_eq!(
        paths
            .iter()
            .map(|path| sha256(&fs::read(path).expect("preserved source")))
            .collect::<Vec<_>>(),
        source_hashes
    );
}

#[test]
fn open_read_only_is_byte_identical_and_does_not_contend_with_writer() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("ledger");
    let ledger =
        GlobalLedger::open(GlobalLedgerConfig::new(&root, "active-writer")).expect("open writer");
    let first = ledger
        .append(event(EventLinksDraft::default()))
        .expect("append");
    let before = tree_bytes(&root);

    let snapshot = GlobalLedger::open_read_only(GlobalLedgerReadOnlyConfig::new(&root), |_| None)
        .expect("open read-only snapshot");

    assert_eq!(tree_bytes(&root), before);
    assert_eq!(snapshot.events(), std::slice::from_ref(&first));
    let metadata = GlobalLedger::open_metadata(super::GlobalLedgerEvidenceConfig::new(temp.path()))
        .expect("metadata does not contend with writer");
    let page = metadata
        .project_view_page(
            &EventQuery::default(),
            ProjectionProfile::Lab,
            &actingcommand_contract::RuntimeEventQueryPageRequest::default(),
        )
        .expect("metadata page");
    assert_eq!(page.events()[0].event_id, *first.event_id());
    assert_eq!(metadata.latest_sequence(), first.sequence());
    assert!(metadata.read_complete());
    assert_eq!(metadata.writer_metadata(), snapshot.writer_metadata());
    assert_eq!(tree_bytes(&root), before);
    assert!(snapshot.corrupt_tail().is_none());
    assert!(snapshot.repairs().is_empty());
    #[cfg(windows)]
    match snapshot.writer_metadata() {
        GlobalLedgerWriterMetadataObservation::Locked { byte_count } => {
            assert!(*byte_count > 0);
        }
        observation => panic!("expected locked writer metadata, got {observation:?}"),
    }
    #[cfg(not(windows))]
    match snapshot.writer_metadata() {
        GlobalLedgerWriterMetadataObservation::Readable(owner) => {
            assert_eq!(owner.owner_id(), "active-writer");
            assert!(owner.active());
        }
        observation => panic!("expected readable writer metadata, got {observation:?}"),
    }
    ledger
        .append(event(EventLinksDraft::default()))
        .expect("writer remains available");
    ledger.close().expect("close writer");
}

#[test]
fn open_read_only_reports_last_segment_dangling_tail_at_last_complete_newline() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("ledger");
    let ledger =
        GlobalLedger::open(GlobalLedgerConfig::new(&root, "tail-writer")).expect("open writer");
    let event = ledger
        .append(event(EventLinksDraft::default()))
        .expect("append");
    ledger.close().expect("close writer");
    let segment = segment_paths(&root).pop().expect("segment");
    let byte_offset = fs::metadata(&segment).expect("segment metadata").len();
    let suffix = b"{\"partial\":true";
    append_bytes(&segment, suffix);
    let before = tree_bytes(&root);

    let snapshot = GlobalLedger::open_read_only(GlobalLedgerReadOnlyConfig::new(&root), |_| None)
        .expect("open read-only snapshot");

    assert_eq!(tree_bytes(&root), before);
    assert_eq!(snapshot.events(), &[event]);
    let tail = snapshot.corrupt_tail().expect("corrupt tail");
    assert_eq!(tail.code.as_str(), "corrupt_segment");
    assert_eq!(tail.segment_index, 1);
    assert_eq!(tail.byte_offset, byte_offset);
    assert_eq!(tail.dangling_byte_count, suffix.len() as u64);
    assert_eq!(tail.tail_sha256, sha256(suffix));
}

#[test]
fn open_read_only_reports_complete_corruption_with_readable_prefix() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("ledger");
    let ledger =
        GlobalLedger::open(GlobalLedgerConfig::new(&root, "corrupt-writer")).expect("open writer");
    let event = ledger
        .append(event(EventLinksDraft::default()))
        .expect("append");
    ledger.close().expect("close writer");
    let segment = segment_paths(&root).pop().expect("segment");
    let byte_offset = fs::metadata(&segment).expect("segment metadata").len();
    let suffix = b"{not-json}\n{\"later\":true}\n";
    append_bytes(&segment, suffix);
    let before = tree_bytes(&root);

    let snapshot = GlobalLedger::open_read_only(GlobalLedgerReadOnlyConfig::new(&root), |_| None)
        .expect("open read-only snapshot");

    assert_eq!(tree_bytes(&root), before);
    assert_eq!(snapshot.events(), &[event]);
    let tail = snapshot.corrupt_tail().expect("corrupt tail");
    assert_eq!(tail.code.as_str(), "corrupt_segment");
    assert_eq!(tail.segment_index, 1);
    assert_eq!(tail.byte_offset, byte_offset);
    assert_eq!(tail.dangling_byte_count, suffix.len() as u64);
    assert_eq!(tail.tail_sha256, sha256(suffix));
    let metadata = GlobalLedger::open_metadata(super::GlobalLedgerEvidenceConfig::new(temp.path()))
        .expect("metadata retains the verified prefix");
    assert!(!metadata.read_complete());
    assert_eq!(metadata.corrupt_tail(), snapshot.corrupt_tail());
    assert_eq!(metadata.latest_sequence(), snapshot.latest_sequence());
    let page = metadata
        .project_view_page(
            &EventQuery::default(),
            ProjectionProfile::Lab,
            &actingcommand_contract::RuntimeEventQueryPageRequest::default(),
        )
        .expect("incomplete metadata page");
    assert!(!page.read_scope().unwrap().read_complete);
    assert!(
        page.read_scope()
            .unwrap()
            .limits
            .contains(&actingcommand_contract::LedgerPageLimit::SourceIncomplete)
    );
    assert_eq!(page.events().len(), 1);
    assert_eq!(tree_bytes(&root), before);
}

#[test]
fn open_read_only_stops_on_non_final_tail_before_later_segments() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("ledger");
    let ledger = GlobalLedger::open(
        GlobalLedgerConfig::new(&root, "rotating-writer").with_segment_max_bytes(128),
    )
    .expect("open writer");
    let events = (0..3)
        .map(|_| {
            ledger
                .append(event(EventLinksDraft::default()))
                .expect("append")
        })
        .collect::<Vec<_>>();
    ledger.close().expect("close writer");
    let segments = segment_paths(&root);
    assert_eq!(
        segments.len(),
        3,
        "test setup requires one event per segment"
    );
    let first_len = fs::metadata(&segments[0])
        .expect("first segment metadata")
        .len();
    let suffix = b"partial-non-final-record";
    append_bytes(&segments[0], suffix);
    let before = tree_bytes(&root);

    let snapshot = GlobalLedger::open_read_only(GlobalLedgerReadOnlyConfig::new(&root), |_| None)
        .expect("open read-only snapshot");

    assert_eq!(tree_bytes(&root), before);
    assert_eq!(snapshot.events(), &events[..1]);
    assert_eq!(snapshot.latest_sequence(), events[0].sequence());
    assert_eq!(snapshot.listed_through_segment(), Some(3));
    let tail = snapshot.corrupt_tail().expect("corrupt tail");
    assert_eq!(tail.code.as_str(), "corrupt_segment");
    assert_eq!(tail.segment_index, 1);
    assert_eq!(tail.byte_offset, first_len);
    assert_eq!(tail.dangling_byte_count, suffix.len() as u64);
    assert_eq!(tail.tail_sha256, sha256(suffix));
}

#[test]
fn open_read_only_query_matches_event_indexes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("ledger");
    let ledger =
        GlobalLedger::open(GlobalLedgerConfig::new(&root, "query-writer")).expect("open writer");
    let correlation = identifiers().mint_correlation_id().expect("correlation id");
    ledger
        .append(event(
            EventLinksDraft::default().with_correlation_id(correlation),
        ))
        .expect("append matching event");
    ledger
        .append(event(EventLinksDraft::default()))
        .expect("append unmatched event");
    ledger
        .append(event(
            EventLinksDraft::default().with_correlation_id(correlation),
        ))
        .expect("append second matching event");
    let through = ledger.latest_sequence().expect("latest sequence");
    let expected = ledger
        .query(correlation_query(correlation))
        .expect("writer query");
    let expected_page = ledger
        .query_page(correlation_query(correlation), 0, through, 1)
        .expect("writer query page");

    let snapshot = GlobalLedger::open_read_only(GlobalLedgerReadOnlyConfig::new(&root), |_| None)
        .expect("open read-only snapshot");

    assert_eq!(snapshot.query(&correlation_query(correlation)), expected);
    assert_eq!(
        snapshot
            .query_page(&correlation_query(correlation), 0, through, 1)
            .expect("read-only query page"),
        expected_page
    );
    assert_eq!(snapshot.latest_sequence(), through);
    ledger.close().expect("close writer");
}

// Workflow #382 L-4 (test plan §4 #17, LED-R2): the legacy Segment parser cases that the retired
// writer-open tests of global/tests.rs refused, read the way migration and forensics read a
// Segment root. The read-only open changes no byte of the root, reports the events before the
// first bad record, and reports that record's segment, offset and the bytes from there on
// (contracts/ledger-store.md, "Recovery, artifacts and forensic consumers": the Segment reader
// reports the verified prefix and any corrupt tail, a complete bad record and a bad non-final
// segment are corruption, and file paths, secret fields and forged values keep their
// non-disclosure rules). A complete corrupt line, a dangling final tail and a non-final torn
// record are the tests above.
#[test]
fn open_read_only_reports_blank_lines_duplicate_keys_and_empty_segments() {
    let temp = tempfile::tempdir().expect("tempdir");

    let blank = temp.path().join("blank-line");
    let ledger =
        GlobalLedger::open(GlobalLedgerConfig::new(&blank, "blank-writer")).expect("open writer");
    let first = ledger
        .append(event(EventLinksDraft::default()))
        .expect("append");
    ledger.close().expect("close writer");
    let segment = segment_paths(&blank).pop().expect("segment");
    let prefix_len = fs::metadata(&segment).expect("segment metadata").len();
    append_bytes(&segment, b"\n");
    assert_legacy_report(
        "blank complete line",
        &blank,
        std::slice::from_ref(&first),
        1,
        Some((1, prefix_len, b"\n".as_slice())),
    );

    let duplicate = temp.path().join("duplicate-key");
    let ledger = GlobalLedger::open(GlobalLedgerConfig::new(&duplicate, "duplicate-writer"))
        .expect("open writer");
    let first = ledger
        .append(event(EventLinksDraft::default()))
        .expect("append first");
    ledger
        .append(event(EventLinksDraft::default()))
        .expect("append second");
    ledger.close().expect("close writer");
    let segment = segment_paths(&duplicate).pop().expect("segment");
    let text = fs::read_to_string(&segment).expect("segment text");
    let first_len = text.find('\n').expect("first record") + 1;
    let hidden = r"C:\hidden\duplicate-subject";
    let encoded = serde_json::to_string(hidden).expect("encode the hidden value");
    let forged = text[first_len..].replacen(
        r#""action":"runtime.start""#,
        &format!(r#""action":{encoded},"action":"runtime.start""#),
        1,
    );
    assert_ne!(forged.as_str(), &text[first_len..]);
    fs::write(&segment, format!("{}{forged}", &text[..first_len]))
        .expect("write the duplicate-key record");
    let snapshot = assert_legacy_report(
        "duplicate JSON key",
        &duplicate,
        std::slice::from_ref(&first),
        1,
        Some((1, first_len as u64, forged.as_bytes())),
    );
    assert!(
        !format!("{:?}", snapshot.corrupt_tail()).contains(hidden),
        "the report discloses the hidden value"
    );

    let empty_non_final = temp.path().join("empty-non-final");
    fs::create_dir_all(empty_non_final.join("segments")).expect("segments");
    fs::write(empty_non_final.join("segments/segment-000001.jsonl"), b"")
        .expect("empty first segment");
    fs::write(empty_non_final.join("segments/segment-000002.jsonl"), b"")
        .expect("empty final segment");
    assert_legacy_report(
        "empty non-final segment",
        &empty_non_final,
        &[],
        2,
        Some((1, 0, b"".as_slice())),
    );

    let sole_empty = temp.path().join("sole-empty-final");
    fs::create_dir_all(sole_empty.join("segments")).expect("segments");
    fs::write(sole_empty.join("segments/segment-000001.jsonl"), b"").expect("empty final segment");
    let snapshot = assert_legacy_report("sole final empty segment", &sole_empty, &[], 1, None);
    assert_eq!(snapshot.latest_sequence(), 0);
}

// Workflow #382 L-4 (test plan §4 #16, LED-R1): the Segment tail repair is written only by the
// cfg(test) writer since 09-13, while migration and forensics still read Segment roots
// read-only. A crash during that repair leaves one of the rows below on disk. Every row is
// built from the files of one repair the cfg(test) writer ran to completion, with no child
// process, and the read-only open reports it as it stands: no byte of the root changes, the
// events are the verified prefix, the dangling tail is reported while it is still on disk, and
// the repair record keeps the completed flag its journal gives it (contracts/ledger-store.md,
// "Recovery, artifacts and forensic consumers": the Segment reader reports the verified prefix
// and any corrupt tail, and `GlobalLedgerReadOnly` opens without writer ownership or mutation
// with tail and repair observations that describe the backend and are never fabricated;
// contracts/ledger-maintenance.md: the read-only parser and the repair journal stay for
// maintenance and forensics, and an import requires every repair completed).
// The same crash semantics on the formal SQLite store, where the evidence table of
// ledger-store.md maps global/recovery_tests.rs to the commit and recovery boundary, are L-3 G5
// (global/tests/gate_l3_g5.rs: an uncertain COMMIT stops the writer with its readback
// classification, and a reopen sees exactly the classified state).
#[test]
fn open_read_only_reports_each_repair_journal_state_a_crash_can_leave() {
    struct Stage<'a> {
        label: &'static str,
        segment: &'a [u8],
        journal: Option<&'a [u8]>,
        quarantined: bool,
        events: &'a [PersistedEvent],
        dangling_tail: bool,
        completed: Option<bool>,
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let repaired = temp.path().join("repaired");
    let ledger =
        GlobalLedger::open(GlobalLedgerConfig::new(&repaired, "seed-writer")).expect("open writer");
    let seeded = ledger
        .append(event(EventLinksDraft::default()))
        .expect("append");
    ledger.close().expect("close writer");
    let segment = segment_paths(&repaired).pop().expect("segment");
    let segment_name = segment.file_name().expect("segment name").to_owned();
    let repaired_len = fs::metadata(&segment).expect("segment metadata").len();
    append_bytes(&segment, TORN_TAIL);
    let torn = fs::read(&segment).expect("torn segment");
    let truncated = &torn[..usize::try_from(repaired_len).expect("prefix length")];
    let ledger = GlobalLedger::open(GlobalLedgerConfig::new(&repaired, "repair-writer"))
        .expect("the writer repairs the dangling tail");
    let recovered_events = ledger
        .query(EventQuery::default())
        .expect("repaired events");
    ledger.close().expect("close repair writer");
    let recovered_segment = fs::read(&segment).expect("repaired segment");
    let journal = fs::read(repaired.join("repair-journal.jsonl")).expect("repair journal");
    let prepared_len = journal
        .iter()
        .position(|byte| *byte == b'\n')
        .expect("prepared record")
        + 1;
    let prepared_journal = &journal[..prepared_len];
    let prepared: serde_json::Value =
        serde_json::from_slice(&journal[..prepared_len - 1]).expect("prepared record");
    let completed_record: serde_json::Value =
        serde_json::from_slice(journal[prepared_len..].trim_ascii_end()).expect("completed record");
    let quarantine_key = prepared["quarantine_key"]
        .as_str()
        .expect("quarantine key")
        .to_owned();
    // The completed repair the rows are derived from: one recovery fact after the seeded
    // event, the truncated prefix kept, the tail quarantined, one prepared and one completed
    // record of the same repair.
    assert_eq!(recovered_events.len(), 2);
    assert_eq!(recovered_events[0], seeded);
    assert_eq!(recovered_events[1].event_type(), EventType::LedgerRecovered);
    assert!(matches!(
        recovered_events[1].payload(),
        actingcommand_contract::EventPayload::Ledger(
            actingcommand_contract::LedgerPayload::Recovered(payload)
        ) if payload.reason() == actingcommand_contract::RecoveryReason::TruncatedFinalTail
    ));
    assert!(recovered_segment.starts_with(truncated));
    assert_eq!(
        fs::read(repaired.join(&quarantine_key)).expect("quarantine copy"),
        TORN_TAIL
    );
    assert_eq!(prepared["state"], "prepared");
    assert_eq!(completed_record["state"], "completed");
    assert_eq!(completed_record["repair_id"], prepared["repair_id"]);
    let expected_repair = |completed: bool| -> RepairView {
        (
            prepared["schema_version"]
                .as_str()
                .expect("schema version")
                .to_owned(),
            prepared["repair_id"]
                .as_str()
                .expect("repair id")
                .to_owned(),
            completed,
            1,
            torn.len() as u64,
            repaired_len,
            sha256(TORN_TAIL),
            quarantine_key.clone(),
        )
    };

    let seeded_only = std::slice::from_ref(&seeded);
    let stages = [
        Stage {
            label: "dangling tail before any repair",
            segment: torn.as_slice(),
            journal: None,
            quarantined: false,
            events: seeded_only,
            dangling_tail: true,
            completed: None,
        },
        Stage {
            label: "after prepare",
            segment: torn.as_slice(),
            journal: Some(prepared_journal),
            quarantined: false,
            events: seeded_only,
            dangling_tail: true,
            completed: Some(false),
        },
        Stage {
            label: "after quarantine",
            segment: torn.as_slice(),
            journal: Some(prepared_journal),
            quarantined: true,
            events: seeded_only,
            dangling_tail: true,
            completed: Some(false),
        },
        Stage {
            label: "after truncate",
            segment: truncated,
            journal: Some(prepared_journal),
            quarantined: true,
            events: seeded_only,
            dangling_tail: false,
            completed: Some(false),
        },
        Stage {
            label: "after the recovery fact",
            segment: recovered_segment.as_slice(),
            journal: Some(prepared_journal),
            quarantined: true,
            events: recovered_events.as_slice(),
            dangling_tail: false,
            completed: Some(false),
        },
        Stage {
            label: "after completion",
            segment: recovered_segment.as_slice(),
            journal: Some(journal.as_slice()),
            quarantined: true,
            events: recovered_events.as_slice(),
            dangling_tail: false,
            completed: Some(true),
        },
    ];
    for (index, stage) in stages.iter().enumerate() {
        let root = temp.path().join(format!("stage-{index}"));
        fs::create_dir_all(root.join("segments")).expect("stage segments");
        fs::write(root.join("segments").join(&segment_name), stage.segment).expect("stage segment");
        if let Some(journal) = stage.journal {
            fs::write(root.join("repair-journal.jsonl"), journal).expect("stage repair journal");
        }
        if stage.quarantined {
            let copy = root.join(&quarantine_key);
            fs::create_dir_all(copy.parent().expect("quarantine directory"))
                .expect("stage quarantine directory");
            fs::write(copy, TORN_TAIL).expect("stage quarantine copy");
        }
        let snapshot = assert_legacy_report(
            stage.label,
            &root,
            stage.events,
            1,
            stage.dangling_tail.then_some((1, repaired_len, TORN_TAIL)),
        );
        assert_eq!(
            snapshot
                .repairs()
                .iter()
                .map(repair_view)
                .collect::<Vec<_>>(),
            stage
                .completed
                .map(expected_repair)
                .into_iter()
                .collect::<Vec<_>>(),
            "{}: repair records",
            stage.label
        );
    }
}

fn identifiers() -> IdentifierIssuer {
    IdentifierIssuer::new().expect("identifier issuer")
}

fn event_id() -> IssuedEventId {
    identifiers().mint_event_id().expect("event id")
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
    .sanitize(&Sha256SecretFingerprinter::new(b"read-only-test-salt").expect("fingerprinter"))
    .expect("sanitize")
}

fn correlation_query(correlation: IssuedCorrelationId) -> EventQuery {
    EventQuery {
        correlation_id: Some(*correlation.transport()),
        ..EventQuery::default()
    }
}

fn segment_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(root.join("segments"))
        .expect("read segments")
        .map(|entry| entry.expect("segment entry").path())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn append_bytes(path: &Path, bytes: &[u8]) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open segment for test corruption")
        .write_all(bytes)
        .expect("append test corruption");
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

// Opens `root` read-only and asserts its report: no byte of the root changes, the events are the
// verified prefix, the listing ends at `listed_through_segment`, and the corrupt tail is
// `corrupt_segment` at (segment index, byte offset) with the bytes from there on, or absent.
fn assert_legacy_report(
    label: &str,
    root: &Path,
    events: &[PersistedEvent],
    listed_through_segment: u64,
    tail: Option<(u64, u64, &[u8])>,
) -> GlobalLedgerReadOnly {
    let before = tree_bytes(root);
    let snapshot =
        GlobalLedger::open_read_only(GlobalLedgerReadOnlyConfig::new(root), |_| None).expect(label);
    assert_eq!(
        tree_bytes(root),
        before,
        "{label}: the read-only open changed the root"
    );
    assert_eq!(snapshot.events(), events, "{label}: verified prefix");
    assert_eq!(
        snapshot.listed_through_segment(),
        Some(listed_through_segment),
        "{label}: listed segments"
    );
    assert_eq!(
        snapshot.corrupt_tail().map(|actual| (
            actual.code.as_str(),
            actual.segment_index,
            actual.byte_offset,
            actual.dangling_byte_count,
            actual.tail_sha256.clone()
        )),
        tail.map(|(segment_index, byte_offset, dangling)| (
            "corrupt_segment",
            segment_index,
            byte_offset,
            dangling.len() as u64,
            sha256(dangling)
        )),
        "{label}: corrupt tail"
    );
    snapshot
}

fn repair_view(record: &GlobalLedgerRepairRecord) -> RepairView {
    (
        record.schema_version().to_owned(),
        record.repair_id().to_owned(),
        record.completed(),
        record.segment_index(),
        record.original_len(),
        record.repaired_len(),
        record.tail_sha256().to_owned(),
        record.quarantine_key().to_owned(),
    )
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
                        TreeObject::Locked(
                            fs::metadata(&child).expect("locked file metadata").len(),
                        )
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
