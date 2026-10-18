// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 A4 (v0.12.1, gate ledger 49): on an SQLite root the offline `actingledger events`
// page and `export --performance` read only their range (`GlobalLedger::open_range`), not the
// whole ledger. The requirements (brief A4 and the coordinator's ruling (a) of 2026-10-18):
// - the range opening authenticates the meta row, the contiguity of the sequences, the head row
//   and the rows after `--after` until one more than `--limit` match, and nothing else;
// - without `--through` the bound is the head of its read transaction, whatever a writer appends
//   meanwhile;
// - the report is the one the whole opening gives on a valid ledger, eviction proofs included,
//   also with `--through` below an eviction;
// - a segment root is read whole, as before.
// The roots are real formal SQLite roots with their ArtifactStore, built by the fixture of
// `missing_material.rs`, or a segment root; each writer closes before the comparing reads.

use super::missing_material::{Capture, capture, formal_root, material_store};
use super::*;
use crate::global::{ArtifactEvictionAdmission, ArtifactEvictionPermit, GlobalLedgerRangeRequest};
use crate::{LedgerReadCounter, LedgerTableReads};
use actingcommand_artifact_store::{ArtifactStore, try_artifact_delete_guard};
use actingcommand_ledger_forensics::{
    ForensicEventFilter, ForensicEventsRequest, ForensicOutput, ForensicRequest, run,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The history of the range test: a few thousand events.
const HISTORY: usize = 2_000;
/// The events the writer appends while the range read holds its head.
const APPENDED: u64 = 3;

/// The range request of an `events` page of `limit` events.
fn range_request(after: u64, through: Option<u64>, limit: usize) -> GlobalLedgerRangeRequest {
    GlobalLedgerRangeRequest {
        query: EventQuery::default(),
        after_sequence: after,
        through_sequence: through,
        matches: limit + 1,
        eviction_proofs: true,
    }
}

/// The `actingledger events` report of the root, as JSON.
fn events_report(root: &Path, after: u64, through: Option<u64>, limit: usize) -> Value {
    let request = ForensicEventsRequest::new(ForensicEventFilter::default(), after, through, limit)
        .expect("events request");
    let ForensicOutput::Machine(report) =
        run(ForensicRequest::events(root, request)).expect("events report")
    else {
        panic!("a machine report");
    };
    serde_json::to_value(&report).expect("report JSON")
}

/// The `events` page the whole opening gives: the first `limit` events after `after` through
/// the bound (the head without `through`), the eviction proofs of their artifacts by artifact,
/// and the cursor when one more event follows within the bound.
fn whole_page(root: &Path, after: u64, through: Option<u64>, limit: usize) -> Value {
    let whole = GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(root).sqlite_material_not_read(),
        |_| None,
    )
    .expect("the whole opening");
    let through = through.unwrap_or_else(|| whole.latest_sequence());
    let events = whole
        .query_page(&EventQuery::default(), after, through, limit + 1)
        .expect("the whole page");
    page(after, through, limit, events)
}

/// The same page from the ledger's own range opening. Its chunks are small in tests, so the
/// page, the eviction rows after it and the head row are read apart.
fn ranged_page(root: &Path, after: u64, through: Option<u64>, limit: usize) -> Value {
    let range = GlobalLedger::open_range(root, &range_request(after, through, limit))
        .expect("the range opening")
        .expect("an SQLite root");
    let through = through.unwrap_or_else(|| range.head_sequence());
    page(after, through, limit, range.into_events())
}

/// The report of at most one more than `limit` events.
fn page(after: u64, through: u64, limit: usize, mut events: Vec<PersistedEvent>) -> Value {
    let next = (events.len() > limit).then(|| events[limit - 1].sequence());
    events.truncate(limit);
    let proofs = events
        .iter()
        .flat_map(PersistedEvent::artifact_evictions)
        .map(|proof| (proof.identity.artifact.artifact_id, proof.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut data = serde_json::json!({
        "filter": ForensicEventFilter::default(),
        "after_sequence": after,
        "through_sequence": through,
        "limit": limit,
        "events": events,
        "artifact_evictions": proofs.into_values().collect::<Vec<_>>(),
    });
    if let Some(next) = next {
        data["next_after_sequence"] = serde_json::json!(next);
    }
    serde_json::json!({ "command": "events", "data": data })
}

fn sequences(events: &[PersistedEvent]) -> Vec<u64> {
    events.iter().map(PersistedEvent::sequence).collect()
}

/// The eviction intent of a closed frame, committed by the writer.
fn evict(
    ledger: &GlobalLedger,
    artifacts: &ArtifactStore,
    frame: &Capture,
) -> Box<ArtifactEvictionPermit> {
    let guard = try_artifact_delete_guard(artifacts.root(), &frame.reference)
        .expect("delete guard")
        .expect("the material guard is free");
    match ledger.admit_artifact_eviction(guard).expect("admission") {
        ArtifactEvictionAdmission::Committed(permit) => permit,
        ArtifactEvictionAdmission::Deferred => panic!("the closed frame is admitted"),
    }
}

// Requirement: a formal SQLite root with a few thousand events; `events --after <head-5>
// --limit 3` returns the whole opening's report and authenticates only the meta row, the head row
// and the rows after head-5. Observable: the read counter of the range opening on this thread.
#[test]
fn a4_events_reads_only_its_range_and_the_head() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    for index in 0..HISTORY {
        ledger
            .append(event(&format!("history-{index}")))
            .expect("history");
    }
    let head = ledger.latest_sequence().expect("head");
    ledger.close().expect("close the writer");
    drop(database);
    let after = head - 5;

    assert_eq!(
        events_report(root.path(), after, None, 3),
        whole_page(root.path(), after, None, 3),
        "the report is the whole opening's"
    );
    // `export --performance` reads its range too: its page is the whole opening's (no
    // performance row among these events).
    let performance = run(ForensicRequest::performance(
        root.path(),
        ForensicEventsRequest::new(ForensicEventFilter::default(), after, None, 3)
            .expect("performance request"),
    ))
    .expect("performance report");
    let ForensicOutput::Machine(performance) = performance else {
        panic!("a machine report");
    };
    assert_eq!(
        serde_json::to_value(&performance).expect("performance JSON"),
        serde_json::json!({
            "command": "performance",
            "data": {
                "after_sequence": after,
                "through_sequence": head,
                "limit": 3,
                "snapshot_latest_sequence": head,
                "scanned_event_count": 3,
                "scanned_through_sequence": after + 3,
                "stutter_count": 0,
                "clock_jump_count": 0,
                "summary_count": 0,
                "rows": [],
                "next_after_sequence": after + 3,
                "has_more": true,
                "window_complete": false,
                "corrupt_tail": null,
                "gaps": [],
            }
        })
    );

    let counter = LedgerReadCounter::start();
    let range = GlobalLedger::open_range(root.path(), &range_request(after, None, 3))
        .expect("the range opening")
        .expect("an SQLite root");
    let counts = counter.counts();
    drop(counter);
    assert_eq!(range.head_sequence(), head);
    assert_eq!(
        sequences(range.events()),
        (after + 1..=after + 4).collect::<Vec<_>>()
    );
    // The four rows after head-5 that the page needs and the head row, each with its link row
    // and no artifact row; besides them only the contiguity row (`COUNT`, `MIN`, `MAX`) and the
    // stored hash of row head-5, the first row's predecessor.
    assert_eq!(
        counts.links,
        LedgerTableReads {
            rows: 5,
            repeated: 0
        },
        "{counts:?}"
    );
    assert_eq!(
        counts.events,
        LedgerTableReads {
            rows: 7,
            repeated: 0
        },
        "{counts:?}"
    );
    assert_eq!(counts.artifacts.rows, 0, "{counts:?}");
    assert_eq!(counts.material_files_opened, 0, "{counts:?}");
}

// Requirement: without `--through` the page ends at the head of its read transaction while a
// second thread appends during the read.
#[test]
fn a4_events_without_through_ends_at_the_head_at_start_while_a_writer_appends() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    for index in 0..8 {
        ledger
            .append(event(&format!("before-{index}")))
            .expect("history");
    }
    let head = ledger.latest_sequence().expect("head at start");
    let after = head - 2;
    let (start, started) = mpsc::channel::<()>();
    let (appended, committed) = mpsc::channel::<u64>();
    let later = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&later);
    let writer = &ledger;
    let range = thread::scope(|scope| {
        let _appender = scope.spawn(move || {
            // No signal: the read ended without its hook.
            if started.recv().is_err() {
                return;
            }
            for index in 0..APPENDED {
                writer
                    .append(event(&format!("during-{index}")))
                    .expect("append during the read");
            }
            appended
                .send(writer.latest_sequence().expect("head after the appends"))
                .expect("report the appends");
        });
        crate::read_count::on_range_head(move || {
            start.send(()).expect("start the appends");
            observed.store(
                committed.recv().expect("the appends committed"),
                Ordering::SeqCst,
            );
        });
        let range = GlobalLedger::open_range(root.path(), &range_request(after, None, 64));
        let pending = crate::read_count::clear_range_head();
        (range, pending)
    });
    let (range, pending) = range;
    assert!(!pending, "the appends ran inside the read");
    let range = range.expect("the range opening").expect("an SQLite root");
    assert_eq!(later.load(Ordering::SeqCst), head + APPENDED);
    assert_eq!(range.head_sequence(), head);
    assert_eq!(sequences(range.events()), vec![head - 1, head]);
    ledger.close().expect("close the writer");
    drop(database);
    let whole = GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(root.path()).sqlite_material_not_read(),
        |_| None,
    )
    .expect("the whole opening");
    assert_eq!(whole.latest_sequence(), head + APPENDED);
    assert_eq!(
        range.events(),
        whole
            .query_page(&EventQuery::default(), after, head, 64)
            .expect("the whole page through the head at start")
            .as_slice()
    );
}

// Requirement: a segment root is read whole, as before: the range opening does not apply to it
// and the report is the whole opening's.
#[test]
fn a4_events_on_a_segment_root_reads_it_whole_as_before() {
    let root = TempDir::new().expect("root");
    let ledger = GlobalLedger::open(GlobalLedgerConfig::new(
        root.path().join("ledger"),
        "segment",
    ))
    .expect("segment writer");
    for index in 0..8 {
        ledger
            .append(event(&format!("segment-{index}")))
            .expect("append");
    }
    let head = ledger.latest_sequence().expect("head");
    ledger.close().expect("close the segment writer");
    assert!(
        GlobalLedger::open_range(root.path(), &range_request(head - 3, None, 2))
            .expect("no SQLite ledger")
            .is_none()
    );
    assert_eq!(
        events_report(root.path(), head - 3, None, 2),
        whole_page(root.path(), head - 3, None, 2)
    );
}

// Requirement (ruling (a), item 3): on a valid root with evictions the range opening gives the
// report the whole opening gives, eviction proofs included, also with `--through` below an
// eviction; every event carries the same proofs and availability.
#[test]
fn a4_events_lists_the_eviction_proofs_of_its_page_as_the_whole_opening_does() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let evicted = capture(&ledger, &artifacts, b"frame evicted with its proof");
    ledger
        .finish_artifact_eviction(evict(&ledger, &artifacts, &evicted))
        .expect("recorded deletion");
    for index in 0..4 {
        ledger
            .append(event(&format!("between-{index}")))
            .expect("history");
    }
    let kept = capture(&ledger, &artifacts, b"frame without an eviction");
    for index in 0..4 {
        ledger
            .append(event(&format!("after-{index}")))
            .expect("history");
    }
    let pending = capture(&ledger, &artifacts, b"frame of a pending eviction");
    // The intent is the head; its outcome never comes.
    drop(evict(&ledger, &artifacts, &pending));
    let head = ledger.latest_sequence().expect("head");
    ledger.close().expect("close with a pending intent");
    drop(database);

    let all = whole_page(root.path(), 0, None, 512);
    assert_eq!(
        all["data"]["artifact_evictions"].as_array().map(Vec::len),
        Some(2),
        "the deleted and the pending frame have proofs: {all}"
    );
    for (after, through, limit) in [
        (0, None, 512),
        // `--through` below the eviction still lists its proof.
        (0, Some(evicted.verified.sequence), 512),
        (evicted.verified.sequence - 1, None, 1),
        (kept.verified.sequence - 1, None, 1),
        (pending.verified.sequence - 1, None, 2),
    ] {
        let whole = whole_page(root.path(), after, through, limit);
        assert_eq!(
            events_report(root.path(), after, through, limit),
            whole,
            "report: after {after}, through {through:?}, limit {limit}"
        );
        assert_eq!(
            ranged_page(root.path(), after, through, limit),
            whole,
            "range opening: after {after}, through {through:?}, limit {limit}"
        );
    }

    let range = GlobalLedger::open_range(root.path(), &range_request(0, None, 512))
        .expect("the range opening")
        .expect("an SQLite root");
    let whole = GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(root.path()).sqlite_material_not_read(),
        |_| None,
    )
    .expect("the whole opening");
    assert_eq!(range.head_sequence(), head);
    assert_eq!(range.events(), whole.events());
    for (ranged, replayed) in range.events().iter().zip(whole.events()) {
        assert_eq!(
            ranged.artifact_evictions(),
            replayed.artifact_evictions(),
            "proofs of event {}",
            ranged.sequence()
        );
        assert_eq!(
            ranged
                .artifacts()
                .iter()
                .map(|artifact| artifact.availability().clone())
                .collect::<Vec<_>>(),
            replayed
                .artifacts()
                .iter()
                .map(|artifact| artifact.availability().clone())
                .collect::<Vec<_>>(),
            "availability of event {}",
            ranged.sequence()
        );
    }
}
