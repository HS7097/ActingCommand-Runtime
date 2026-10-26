// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 L-3 G9 with its read counters LED-I5 (ledger key 6, inventory gap G9: the formal
// open over N and 4N records reads each event and link row once and opens no artifact file; a
// root with ten times the material gives the same read count). The requirements:
// - `contracts/ledger-store.md` (LedgerStore): the formal SQLite constructor authenticates the
//   keyed meta row, then reads and verifies the history in one pass, reading no further than the
//   authenticated head; open time grows linearly with the committed history. The constructor runs
//   on the opening thread.
// - `contracts/ledger-store.md` (Workflow #375 R5a): the formal openings, the startup writer open
//   included, never open, hash or stat a referenced artifact file.
// - Test plan v2.2 row L-3: I5 read counters, G9 #375 linear.
//
// Records the open itself appends: none. Each writer closes explicitly before the reopen, so no
// stale-owner `ledger.recovered` is due (`contracts/ledger-store.md`: the writer's own stale-owner
// recovery report) and no `tail_unconfirmed` gap (`contracts/ledger-observer.md` §10: only when
// the previous owner was not closed cleanly); the reopen is the offline writer without a failure
// plan, which imports no dump (§9.4, `open_writer_timed`). So the head after the reopen is the
// head the root closed with, which `counted_open` checks, and the rows to read are exactly the
// committed records: one `ledger_events` row and one `ledger_links` row per record, one
// `ledger_artifacts` row per artifact reference.
//
// #422 LN-7 (LEAN-L §3.5, `contracts/ledger-store.md`): until LN-8 drops the full read, the open
// also reads its retention state by type and by index besides the pass. The pass is counted by
// the set of sequences it saw (each event and link row, none past the head); the extra reads are
// the rows read again and the rows read without an identity (a predecessor's stored hash). They
// follow the live frames, not the records: the owner records (here each capture's two
// quiescences) and what the classes of the frames present in the store read. A root whose frames
// were all deleted by hand reads again only its owner records, none of its frames' records.
//
// The counters are per thread: the whole formal open runs on the opening thread before the writer
// thread receives the store, so the writer's command window (`LedgerWriterPeak`) never sees it.
// The roots are real formal SQLite roots with their ArtifactStore, built by the fixture of
// `missing_material.rs`; some records carry a captured frame.

use super::missing_material::{capture, formal_root, material_store};
use crate::global::{LedgerMaintenance, Sha256SecretFingerprinter};
use crate::{LedgerReadCounter, LedgerReadCounts, LedgerSequencesRead};
use actingcommand_contract::{
    AuditInput, CommandPayloadDraft, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventQuery, EventSeverity, EventSource, EventType, IdentifierIssuer, OriginModule,
    SanitizedEventDraft,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

// The reopening writer's owner id and the fingerprint salt: neutral fixture words.
const OWNER: &str = "ledger-l3-g9";
const SALT: &[u8] = b"ledger-l3-g9-fixture";
// The time of the plain records.
const AT_MS: u64 = 1_752_147_200_000;
// N: the records of the smaller root, and the captured frames among them.
const RECORDS: u64 = 32;
const FRAMES: usize = 2;
// The size of one captured frame.
const FRAME_BYTES: usize = 64 * 1024;

// A closed formal root.
struct Root {
    _dir: TempDir,
    database: Arc<RuntimeDatabase>,
    // The committed records (the head sequence).
    records: u64,
    // The artifact references of the committed records (`ledger_artifacts` rows).
    artifact_rows: u64,
    // The owner records among them (`runtime.lifecycle_observed`: each capture's quiescences).
    owner_records: u64,
    // The material files of the captured frames.
    materials: Vec<PathBuf>,
}

fn salt() -> Sha256SecretFingerprinter {
    Sha256SecretFingerprinter::new(SALT).expect("fingerprint salt")
}

// A plain record without artifacts (`command.received`).
fn plain(ids: &IdentifierIssuer) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        AT_MS,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
        EventLinksDraft::default(),
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
    )
    .sanitize(&salt())
    .expect("a plain draft")
}

// Distinct frame bytes of `len` bytes.
fn frame(index: usize, len: usize) -> Vec<u8> {
    let mut bytes = format!("captured frame {index} of the fixture ").into_bytes();
    bytes.resize(len, b'.');
    bytes
}

// A formal root with `frames` captured frames of `frame_bytes` bytes, each followed by a plain
// record, then plain records up to `records`; closed by its writer.
fn build(frames: usize, frame_bytes: usize, records: u64) -> Root {
    let dir = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(dir.path());
    let artifacts = material_store(dir.path());
    let ids = IdentifierIssuer::new().expect("issuer");
    let mut materials = Vec::with_capacity(frames);
    for index in 0..frames {
        let captured = capture(&ledger, &artifacts, &frame(index, frame_bytes));
        materials.push(captured.material);
        ledger.append(plain(&ids)).expect("plain record");
    }
    let head = ledger.latest_sequence().expect("head");
    assert!(
        head <= records,
        "the frames fit in {records} records: {head}"
    );
    for _ in head..records {
        ledger.append(plain(&ids)).expect("plain record");
    }
    let history = ledger.query(EventQuery::default()).expect("history");
    assert_eq!(
        history.len() as u64,
        records,
        "the root holds {records} records"
    );
    let artifact_rows = history
        .iter()
        .map(|event| event.artifacts().len() as u64)
        .sum::<u64>();
    assert!(artifact_rows > 0, "some records carry a frame");
    let owner_records = history
        .iter()
        .filter(|event| event.event_type() == EventType::RuntimeLifecycleObserved)
        .count() as u64;
    ledger.close().expect("close the writer");
    Root {
        _dir: dir,
        database,
        records,
        artifact_rows,
        owner_records,
        materials,
    }
}

// What one open read on its thread: the counts, and the sequences of the event and link rows.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenReads {
    counts: LedgerReadCounts,
    sequences: LedgerSequencesRead,
}

// Reopens the root's formal writer as a start does and returns what the open read on this
// thread; checks that the open appended no record, then closes the writer.
fn counted_open(root: &Root) -> OpenReads {
    let limits = MaintenanceLimits::default();
    let counter = LedgerReadCounter::start();
    let ledger = LedgerMaintenance::acquire(
        root.database.root(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner")
    .open_writer(Arc::clone(&root.database), OWNER.into())
    .expect("the formal writer opens");
    let counts = counter.counts();
    let sequences = counter.sequences_read();
    drop(counter);
    assert_eq!(
        ledger.latest_sequence().expect("head"),
        root.records,
        "the open appends no record"
    );
    ledger.close().expect("close the reopened writer");
    OpenReads { counts, sequences }
}

// One pass: the event rows and the link rows seen are exactly those of the committed records, so
// each was read and none past the head; no artifact row beyond the committed ones; no material.
fn assert_one_pass(root: &Root, reads: &OpenReads) {
    let counts = reads.counts;
    let committed = (1..=root.records).collect::<BTreeSet<_>>();
    assert_eq!(
        reads.sequences.events, committed,
        "each event row, none past the head: {counts:?}"
    );
    assert_eq!(
        reads.sequences.links, committed,
        "each link row, none past the head: {counts:?}"
    );
    assert!(
        counts.artifacts.rows - counts.artifacts.repeated - counts.unidentified.artifacts
            <= root.artifact_rows,
        "at most the {} artifact rows: {counts:?}",
        root.artifact_rows
    );
    assert_eq!(
        counts.material_files_opened, 0,
        "the formal open opens no material: {counts:?}"
    );
}

// #422 LN-7: what the open read besides its pass, by table (events, links, artifacts): the rows
// read again and the rows read without an identity.
fn extra(counts: &LedgerReadCounts) -> [u64; 3] {
    [
        counts.events.repeated + counts.unidentified.events,
        counts.links.repeated + counts.unidentified.links,
        counts.artifacts.repeated + counts.unidentified.artifacts,
    ]
}

#[test]
fn gate_l3_g9_formal_open_reads_each_row_once_and_scales_linearly() {
    let at_n = build(FRAMES, FRAME_BYTES, RECORDS);
    let at_4n = build(4 * FRAMES, FRAME_BYTES, 4 * RECORDS);
    let read_n = counted_open(&at_n);
    let read_4n = counted_open(&at_4n);
    assert_one_pass(&at_n, &read_n);
    assert_one_pass(&at_4n, &read_4n);
    assert_eq!(
        (
            read_4n.sequences.events.len(),
            read_4n.sequences.links.len()
        ),
        (
            4 * read_n.sequences.events.len(),
            4 * read_n.sequences.links.len()
        ),
        "four times the records, four times the event and link rows of the pass"
    );
    // #422 LN-7: the extra reads follow the live frames, not the records: four times the records
    // with the same live frames read the same extra rows.
    let same_frames = build(FRAMES, FRAME_BYTES, 4 * RECORDS);
    let read_same_frames = counted_open(&same_frames);
    assert_one_pass(&same_frames, &read_same_frames);
    assert_eq!(
        extra(&read_same_frames.counts),
        extra(&read_n.counts),
        "four times the records with the same live frames, the same extra reads: {:?} {:?}",
        read_same_frames.counts,
        read_n.counts
    );
}

#[test]
fn gate_l3_g9_ten_times_larger_material_reads_the_same_rows() {
    let plain_root = build(FRAMES, FRAME_BYTES, RECORDS);
    let tenfold = build(FRAMES, 10 * FRAME_BYTES, RECORDS);
    assert_eq!(
        tenfold.artifact_rows, plain_root.artifact_rows,
        "the same records"
    );
    let read_plain = counted_open(&plain_root);
    let read_tenfold = counted_open(&tenfold);
    assert_one_pass(&plain_root, &read_plain);
    assert_eq!(
        read_tenfold, read_plain,
        "the material size changes no read"
    );
}

// #422 LN-7: an open of a root without live frames reads again its owner records (by type), each
// with at most one predecessor's hash, and nothing else: none of its frames' records.
fn assert_no_frame_record_read_again(root: &Root, counts: &LedgerReadCounts) {
    assert_eq!(
        (
            counts.events.repeated,
            counts.links.repeated,
            counts.artifacts.repeated
        ),
        (root.owner_records, root.owner_records, 0),
        "only the {} owner records are read again: {counts:?}",
        root.owner_records
    );
    assert!(
        counts.unidentified.events <= root.owner_records
            && counts.unidentified.links == 0
            && counts.unidentified.artifacts == 0,
        "at most one predecessor's hash per owner record: {counts:?}"
    );
}

#[test]
fn gate_l3_g9_material_deleted_by_hand_reads_the_same_rows() {
    let root = build(FRAMES, FRAME_BYTES, RECORDS);
    let with_material = counted_open(&root);
    for material in &root.materials {
        std::fs::remove_file(material).expect("hand deletion");
    }
    let without_material = counted_open(&root);
    assert_one_pass(&root, &without_material);
    assert_eq!(
        (
            &without_material.sequences,
            without_material.counts.material_files_opened
        ),
        (
            &with_material.sequences,
            with_material.counts.material_files_opened
        ),
        "missing material changes no row of the pass and opens no material"
    );
    // #422 LN-7: a frame whose file is gone is no live frame, so the open reads none of its records
    // again; with four times the frames deleted, still none.
    let fourfold = build(4 * FRAMES, FRAME_BYTES, 4 * RECORDS);
    for material in &fourfold.materials {
        std::fs::remove_file(material).expect("hand deletion");
    }
    let fourfold_without_material = counted_open(&fourfold);
    assert_one_pass(&fourfold, &fourfold_without_material);
    assert_no_frame_record_read_again(&root, &without_material.counts);
    assert_no_frame_record_read_again(&fourfold, &fourfold_without_material.counts);
}
