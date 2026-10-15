// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 A-4 G8 with the L-3 read counters (LED-I5; apps key 10, inventory-apps.md gap G8):
// the two ledger reads of #375 that check-config and `ledger-maintenance verify` make, counted on
// the opening thread. The requirements:
// - rd-1008/v0115/spec-375-r3.md §0 and Q4 (req-basis-ha.md A-4 G8): the check-config preview
//   reads only the records of its types plus the head (`GlobalLedger::open_selected`, which
//   runtime-host's catalog preview calls with its types); so what it reads does not grow with
//   the records of other types: a root with ten times the unrelated history reads the same rows.
// - spec-375-r3 Q4: `ledger-maintenance verify` remains the complete row check: each event row and
//   each link row is read once (`LedgerMaintenance::verify_formal`, runtime-host's verify of a
//   formal root); contracts/ledger-store.md (Workflow #375 R5a): it reads no artifact material,
//   so a root whose frames were deleted by hand reads the same rows.
// The cold `quick_check` row of G8 is runtime-host's `gate_375_r3` (apps key 10 already).
//
// The roots are real formal SQLite roots with their ArtifactStore, built by the fixture of
// `missing_material.rs`; each writer closes before the read, so the root's head is its last
// committed record.

use super::missing_material::{capture, formal_root, material_store};
use crate::global::{GlobalLedger, LedgerMaintenance, LedgerOpenTiming, Sha256SecretFingerprinter};
use crate::{LedgerReadCounter, LedgerReadCounts, LedgerTableReads};
use actingcommand_contract::{
    AuditInput, CommandPayloadDraft, EffectDisposition, EventAction, EventActor, EventDraft,
    EventLinksDraft, EventOrigin, EventQuery, EventSeverity, EventSource, EventType,
    IdentifierIssuer, OriginModule, SanitizedEventDraft,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

const SALT: &[u8] = b"ledger-a4-g8-fixture";
// The time of the fixture's records.
const AT_MS: u64 = 1_752_147_200_000;
// The records of the selected type, and the unrelated records of the smaller root.
const SELECTED: u64 = 4;
const UNRELATED: u64 = 40;
// The verified root: its records, and the captured frames among them.
const RECORDS: u64 = 32;
const FRAMES: usize = 2;
const FRAME_BYTES: usize = 64 * 1024;

// G8, the preview's read: SELECTED records of the selected type, then UNRELATED or ten times as
// many records of another type. The selected open returns exactly the selected records and reads
// the same rows from both roots, repeats no row and opens no material.
#[test]
fn gate_382_a4_g8_a_selected_open_reads_the_same_rows_with_ten_times_the_unrelated_history() {
    let smaller = selection_root(UNRELATED);
    let larger = selection_root(10 * UNRELATED);

    let read_smaller = counted_selected_open(&smaller);
    let read_larger = counted_selected_open(&larger);

    for counts in [read_smaller, read_larger] {
        assert_eq!(counts.events.repeated, 0, "no event row twice: {counts:?}");
        assert_eq!(counts.links.repeated, 0, "no link row twice: {counts:?}");
        assert_eq!(counts.material_files_opened, 0, "no material: {counts:?}");
    }
    assert_eq!(
        read_larger, read_smaller,
        "ten times the unrelated history changes no read"
    );
}

// G8, verify: the complete check of a formal root reads each event row and each link row once
// and opens no material; with its frames deleted by hand it reads the same rows.
#[test]
fn gate_382_a4_g8_verify_reads_each_row_once_and_no_material() {
    let root = verified_root();
    let with_material = counted_verify(&root);
    assert_one_pass(&root, with_material);

    for material in &root.materials {
        std::fs::remove_file(material).expect("hand deletion");
    }
    let without_material = counted_verify(&root);
    assert_one_pass(&root, without_material);
    assert_eq!(
        without_material, with_material,
        "deleted material changes no read"
    );
}

// --- the preview's read ---

// A closed formal root with SELECTED validated commands, then `unrelated` received commands.
struct SelectionRoot {
    dir: TempDir,
    head: u64,
}

fn selection_root(unrelated: u64) -> SelectionRoot {
    let dir = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(dir.path());
    let ids = IdentifierIssuer::new().expect("issuer");
    for _ in 0..SELECTED {
        ledger.append(validated(&ids)).expect("selected record");
    }
    for _ in 0..unrelated {
        ledger.append(received(&ids)).expect("unrelated record");
    }
    let head = ledger.latest_sequence().expect("head");
    ledger.close().expect("close the writer");
    SelectionRoot { dir, head }
}

// The selected open of the validated commands, counted on this thread.
fn counted_selected_open(root: &SelectionRoot) -> LedgerReadCounts {
    let limits = MaintenanceLimits::default();
    let counter = LedgerReadCounter::start();
    let selection = GlobalLedger::open_selected(
        root.dir.path(),
        &[EventType::CommandValidated],
        limits.deadline().expect("deadline"),
    )
    .expect("the selected open");
    let counts = counter.counts();
    drop(counter);
    assert!(selection.is_complete());
    assert_eq!(selection.head_sequence(), root.head);
    let selected = selection
        .query(&EventQuery {
            event_type: Some(EventType::CommandValidated),
            ..EventQuery::default()
        })
        .expect("query the selected type");
    assert_eq!(
        selected.len() as u64,
        SELECTED,
        "exactly the selected records"
    );
    counts
}

fn salt() -> Sha256SecretFingerprinter {
    Sha256SecretFingerprinter::new(SALT).expect("fingerprint salt")
}

fn command(ids: &IdentifierIssuer, payload: CommandPayloadDraft) -> SanitizedEventDraft {
    EventDraft::new(
        ids.mint_event_id().expect("event id"),
        AT_MS,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
        EventLinksDraft::default(),
        payload.into(),
    )
    .sanitize(&salt())
    .expect("a command draft")
}

// A record of the selected type (`command.validated`).
fn validated(ids: &IdentifierIssuer) -> SanitizedEventDraft {
    command(
        ids,
        CommandPayloadDraft::validated(
            EventAction::RuntimeStart,
            EffectDisposition::NotPerformed,
            AuditInput::new(),
        ),
    )
}

// An unrelated record (`command.received`).
fn received(ids: &IdentifierIssuer) -> SanitizedEventDraft {
    command(
        ids,
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()),
    )
}

// --- verify ---

// A closed formal root of RECORDS records, FRAMES of them with a captured frame.
struct VerifiedRoot {
    _dir: TempDir,
    database: Arc<RuntimeDatabase>,
    records: u64,
    artifact_rows: u64,
    materials: Vec<PathBuf>,
}

fn verified_root() -> VerifiedRoot {
    let dir = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(dir.path());
    let artifacts = material_store(dir.path());
    let ids = IdentifierIssuer::new().expect("issuer");
    let mut materials = Vec::with_capacity(FRAMES);
    for index in 0..FRAMES {
        let mut bytes = format!("captured frame {index} of the fixture ").into_bytes();
        bytes.resize(FRAME_BYTES, b'.');
        materials.push(capture(&ledger, &artifacts, &bytes).material);
        ledger.append(received(&ids)).expect("plain record");
    }
    let head = ledger.latest_sequence().expect("head");
    assert!(
        head <= RECORDS,
        "the frames fit in {RECORDS} records: {head}"
    );
    for _ in head..RECORDS {
        ledger.append(received(&ids)).expect("plain record");
    }
    let history = ledger.query(EventQuery::default()).expect("history");
    assert_eq!(
        history.len() as u64,
        RECORDS,
        "the root holds {RECORDS} records"
    );
    let artifact_rows = history
        .iter()
        .map(|event| event.artifacts().len() as u64)
        .sum::<u64>();
    assert!(artifact_rows > 0, "some records carry a frame");
    ledger.close().expect("close the writer");
    VerifiedRoot {
        _dir: dir,
        database,
        records: RECORDS,
        artifact_rows,
        materials,
    }
}

// The verify of the formal root as `ledger-maintenance verify` runs it, counted on this thread.
fn counted_verify(root: &VerifiedRoot) -> LedgerReadCounts {
    let limits = MaintenanceLimits::default();
    let maintenance = LedgerMaintenance::acquire(
        root.database.root(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner");
    let mut timing = LedgerOpenTiming::default();
    let counter = LedgerReadCounter::start();
    let (events, _status) = maintenance
        .verify_formal(&root.database, &mut timing)
        .expect("the formal root verifies");
    let counts = counter.counts();
    drop(counter);
    maintenance.close().expect("close the maintenance owner");
    assert_eq!(
        events.len() as u64,
        root.records,
        "every record is verified"
    );
    counts
}

// One pass: each event row and each link row exactly once, no artifact row twice, no material.
fn assert_one_pass(root: &VerifiedRoot, counts: LedgerReadCounts) {
    let once = LedgerTableReads {
        rows: root.records,
        repeated: 0,
    };
    assert_eq!(counts.events, once, "each event row once: {counts:?}");
    assert_eq!(counts.links, once, "each link row once: {counts:?}");
    assert_eq!(
        counts.artifacts.repeated, 0,
        "no artifact row twice: {counts:?}"
    );
    assert!(
        counts.artifacts.rows <= root.artifact_rows,
        "at most the {} artifact rows: {counts:?}",
        root.artifact_rows
    );
    assert_eq!(
        counts.material_files_opened, 0,
        "verify opens no material: {counts:?}"
    );
}
