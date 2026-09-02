// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    AuditInput, CommandPayloadDraft, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventQuery, EventSeverity, EventSource, IdentifierIssuer, IssuedCorrelationId,
    IssuedEventId, OriginModule, SanitizedEventDraft,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

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
    assert_eq!(snapshot.events(), &[first]);
    assert!(snapshot.corrupt_tail().is_none());
    assert!(snapshot.repairs().is_empty());
    if let Some(owner) = snapshot.writer_metadata() {
        assert_eq!(owner.owner_id(), "active-writer");
        assert!(owner.active());
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
    assert_eq!(tail.code, "corrupt_segment");
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
    assert_eq!(tail.code, "corrupt_segment");
    assert_eq!(tail.segment_index, 1);
    assert_eq!(tail.byte_offset, byte_offset);
    assert_eq!(tail.dangling_byte_count, suffix.len() as u64);
    assert_eq!(tail.tail_sha256, sha256(suffix));
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
    assert_eq!(tail.code, "corrupt_segment");
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
