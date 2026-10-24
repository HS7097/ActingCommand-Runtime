// SPDX-License-Identifier: AGPL-3.0-only

// LN-1 (Workflow #422, LEAN-L model §5.1 and §7.2): the synthetic ledger the scale harness starts
// the Runtime on. Its output opens and authenticates: the writer's formal open reads and
// authenticates every row (hash, keyed tag, predecessor, links and index columns) and restores
// exactly the records written. It is the production mix with its index rows, and the same spec
// writes the same records.

use super::*;
use crate::global::{LedgerMaintenance, LedgerOpenTiming};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use std::sync::Arc;
use tempfile::TempDir;

// The State integrity key and the writer's owner id: neutral fixture words.
const STATE_KEY: &[u8] = b"ln1-synthetic-fixture";
const OWNER: &str = "ln1-synthetic-fixture";
// Enough records for several runs, written in batches that do not divide them.
const RECORDS: u64 = 3_000;
const BATCH: usize = 700;

// The record types of the production mix (module documentation of `crate::synthetic`).
const MIX: [&str; 21] = [
    "command.received",
    "command.validated",
    "task.requested",
    "task.started",
    "capture.requested",
    "capture.completed",
    "task.evidence_indexed",
    "recognition.requested",
    "task.recognition_started",
    "recognition.completed",
    "task.recognition_completed",
    "task.step_started",
    "task.effect_intent",
    "input.intent",
    "input.committed",
    "task.effect_completed",
    "task.step_finished",
    "fact.published",
    "runtime.fact_recorded",
    "task.terminal_intent",
    "task.completed",
];

fn instance() -> SyntheticInstance {
    SyntheticInstance {
        alias: "fixture-instance-a".to_owned(),
        instance_id: serde_json::from_str("\"instance_000000000000000000000000000000a1\"")
            .expect("a fixture instance id"),
    }
}

fn spec() -> SyntheticLedgerSpec {
    SyntheticLedgerSpec {
        batch_records: BATCH,
        ..SyntheticLedgerSpec::new(RECORDS, vec![instance()])
    }
}

// A closed formal root as acsetup leaves it: the State database, the formal empty ledger, and
// one writer open and close, whose owner metadata every later open reads.
fn formal_root() -> TempDir {
    let directory = tempfile::tempdir().expect("state root");
    let database = Arc::new(
        RuntimeStateStore::open_database(directory.path(), STATE_KEY).expect("State database"),
    );
    let limits = MaintenanceLimits::default();
    let maintenance = LedgerMaintenance::acquire(
        directory.path(),
        true,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner");
    maintenance
        .initialize_empty(&database)
        .expect("formal ledger");
    maintenance
        .open_writer(Arc::clone(&database), OWNER.into())
        .expect("formal writer")
        .close()
        .expect("close the formal writer");
    directory
}

fn generate(root: &TempDir) -> SyntheticLedgerReport {
    let database = RuntimeDatabase::open_existing(root.path(), false).expect("the root's database");
    write_synthetic_ledger(&database, &spec()).expect("the synthetic ledger")
}

fn index_rows(database: &RuntimeDatabase, role: &str) -> i64 {
    database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("connection")
        .query_row(
            "SELECT count(*) FROM ledger_index WHERE role=?1",
            [role],
            |row| row.get(0),
        )
        .expect("index rows")
}

#[test]
fn ln1_synthetic_ledger_opens_authenticates_and_repeats() {
    let first = formal_root();
    let second = formal_root();
    let report = generate(&first);
    let again = generate(&second);

    // Exactly the requested history, in consecutive sequences and batches.
    assert_eq!(report.records, RECORDS);
    assert_eq!(report.head_sequence, report.first_sequence + RECORDS - 1);
    assert_eq!(report.record_types.values().sum::<u64>(), RECORDS);
    assert_eq!(report.batches, RECORDS.div_ceil(BATCH as u64));
    assert!(report.runs > 1 && report.frames > report.runs, "{report:?}");
    // The production mix.
    for record_type in MIX {
        assert!(
            report
                .record_types
                .get(record_type)
                .is_some_and(|count| *count > 0),
            "{record_type} is in the mix: {:?}",
            report.record_types
        );
    }
    assert_eq!(
        report.record_types.len(),
        MIX.len(),
        "{:?}",
        report.record_types
    );
    // Deterministic: the same spec from the same head writes the same records.
    assert_eq!(
        (
            again.first_sequence,
            &again.content_sha256,
            &again.record_types,
            again.canonical_bytes
        ),
        (
            report.first_sequence,
            &report.content_sha256,
            &report.record_types,
            report.canonical_bytes
        )
    );

    // The writer's formal open authenticates every row and restores every record.
    let database =
        Arc::new(RuntimeDatabase::open_existing(first.path(), false).expect("the root's database"));
    let limits = MaintenanceLimits::default();
    let maintenance = LedgerMaintenance::acquire(
        first.path(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner");
    let mut timing = LedgerOpenTiming::default();
    let ledger = maintenance
        .open_writer_timed(Arc::clone(&database), OWNER.into(), None, &mut timing)
        .expect("the formal open authenticates the synthetic ledger");
    assert_eq!(timing.events, report.head_sequence);
    assert_eq!(
        ledger.latest_sequence().expect("the head"),
        report.head_sequence
    );
    ledger.close().expect("close the writer");

    // The index table: the tagged keys' rows and the request ring; every intent closed by its
    // result.
    assert!(index_rows(&database, "state") > 0);
    assert!(index_rows(&database, "request") > 0);
    assert_eq!(index_rows(&database, "open"), 0);
}
