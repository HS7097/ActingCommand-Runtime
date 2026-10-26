// SPDX-License-Identifier: AGPL-3.0-only

// #422 LN-7c F6 (rd-1026/ln7c/final-query.md §1.1, §1.3, §7 F6, §8; ruling 70: frame retention
// by ledger query):
// - capture frames by ingestion time, `ledger_artifacts_created`, a plain partial index: a new
//   root has it from `schema.sql`, a root an older build wrote gets it at its first writer open,
//   and a later open leaves it as it is;
// - the plan check for ledger tests holds a statement's plan on an empty database to the §1.3
//   rule, so a statement doctored to read the whole history is rejected.

use super::plan_check::check_plan;
use super::*;
use crate::global::LedgerMaintenance;
use crate::{SyntheticInstance, SyntheticLedgerSpec, write_synthetic_ledger};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits};
use actingcommand_runtime_state::RuntimeStateStore;
use rusqlite::OptionalExtension;
use std::path::Path;
use std::sync::MutexGuard;

// The State key, the writer's owner id and the fixture instance: neutral fixture words.
const STATE_KEY: &[u8] = b"ln7c-f6-index-fixture";
const OWNER: &str = "ln7c-f6-index-fixture";
const INSTANCE_ID: &str = "\"instance_000000000000000000000000000000a1\"";
// A short history with a stored capture frame every fourth frame of a run.
const RECORDS: u64 = 1_000;
const STORED_FRAME_EVERY: u64 = 4;
// The index as SQLite keeps it (final-query §1.1).
const INDEX_SQL: &str = "CREATE INDEX ledger_artifacts_created ON ledger_artifacts(created_at_unix_ms, sequence) WHERE kind = 'capture.frame'";

// The database's one connection; held only while a statement runs, never across an open.
fn connection(database: &RuntimeDatabase) -> MutexGuard<'_, Connection> {
    database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("connection")
}

// The index's row in the schema (its table, root page and definition); `None` without it.
fn index_row(database: &RuntimeDatabase) -> Option<(String, i64, String)> {
    connection(database)
        .query_row(
            "SELECT tbl_name, rootpage, sql FROM sqlite_schema \
             WHERE type = 'index' AND name = 'ledger_artifacts_created'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .expect("the index's schema row")
}

// The capture-frame rows read through the index, and those read from the table without it.
fn frame_rows(database: &RuntimeDatabase) -> (i64, i64) {
    let connection = connection(database);
    let indexed = connection
        .query_row(
            "SELECT count(*) FROM ledger_artifacts INDEXED BY ledger_artifacts_created \
             WHERE kind = 'capture.frame' AND created_at_unix_ms >= ?1",
            [i64::MIN],
            |row| row.get(0),
        )
        .expect("the frames through the index");
    let stored = connection
        .query_row(
            "SELECT count(*) FROM ledger_artifacts NOT INDEXED WHERE kind = 'capture.frame'",
            [],
            |row| row.get(0),
        )
        .expect("the frames in the table");
    (indexed, stored)
}

// One offline writer open of the closed formal root at `root`, and its close.
fn open_and_close(root: &Path, database: &Arc<RuntimeDatabase>) {
    let limits = MaintenanceLimits::default();
    LedgerMaintenance::acquire(root, false, limits, limits.deadline().expect("deadline"))
        .expect("maintenance owner")
        .open_writer(Arc::clone(database), OWNER.into())
        .expect("the writer opens")
        .close()
        .expect("close the writer");
}

#[test]
fn ln7c_f6_a_root_without_the_frame_time_index_gets_it_at_its_first_writer_open() {
    let dir = tempfile::tempdir().expect("state root");
    let database =
        Arc::new(RuntimeStateStore::open_database(dir.path(), STATE_KEY).expect("State database"));
    let limits = MaintenanceLimits::default();
    let maintenance = LedgerMaintenance::acquire(
        dir.path(),
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
    // A new root has the index from `schema.sql`.
    assert_eq!(
        index_row(&database).map(|(table, _, sql)| (table, sql)),
        Some(("ledger_artifacts".to_owned(), INDEX_SQL.to_owned())),
        "a new root's index"
    );
    let mut spec = SyntheticLedgerSpec::new(
        RECORDS,
        vec![SyntheticInstance {
            alias: "fixture-instance-a".to_owned(),
            instance_id: serde_json::from_str(INSTANCE_ID).expect("a fixture instance id"),
        }],
    );
    spec.mix.frame_artifact_every_frames = STORED_FRAME_EVERY;
    let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
    assert!(generated.stored_frames > 0, "{generated:?}");

    // The root as a build without the index left it.
    connection(&database)
        .execute_batch("DROP INDEX ledger_artifacts_created")
        .expect("a root without the index");
    assert_eq!(index_row(&database), None, "the index is gone");

    // The first writer open creates it, with the definition a new root has, over every frame.
    open_and_close(dir.path(), &database);
    let created = index_row(&database).expect("the writer open creates the index");
    assert_eq!(
        (created.0.as_str(), created.2.as_str()),
        ("ledger_artifacts", INDEX_SQL),
        "the created index"
    );
    let (indexed, stored) = frame_rows(&database);
    assert!(
        stored > 0 && indexed == stored,
        "{indexed} frame rows through the index, {stored} in the table"
    );

    // A second open leaves it as it is: the same row, at the same root page (not rebuilt).
    open_and_close(dir.path(), &database);
    assert_eq!(
        index_row(&database),
        Some(created),
        "the second open changed the index"
    );
}

#[test]
fn ln7c_f6_the_plan_check_rejects_a_doctored_statement_that_reads_the_whole_history() {
    // Capture frames created in a time window, through the frame time index: bounded.
    let window = "SELECT a.artifact_id FROM ledger_artifacts a INDEXED BY ledger_artifacts_created \
                  WHERE a.kind = 'capture.frame' AND a.created_at_unix_ms > ?1 \
                  AND a.created_at_unix_ms <= ?2 ORDER BY a.created_at_unix_ms, a.sequence";
    let bounded = check_plan(window);
    assert!(
        bounded.violations.is_empty()
            && bounded
                .plan
                .iter()
                .any(|step| step.contains("USING INDEX ledger_artifacts_created (")),
        "{bounded:?}"
    );

    // Doctored statements, each with the rule it breaks.
    for (doctored, rule) in [
        // The same window without its index: a scan of the table under its alias.
        (
            window.replace("INDEXED BY ledger_artifacts_created", "NOT INDEXED"),
            "a full scan of a ledger table or index",
        ),
        // A scan under the table's own name.
        (
            "SELECT artifact_id FROM ledger_artifacts WHERE byte_count > ?1".to_owned(),
            "a full scan of a ledger table or index",
        ),
        // A walk of a whole index.
        (
            "SELECT l.run_id FROM ledger_links l ORDER BY l.run_id".to_owned(),
            "a full scan of a ledger table or index",
        ),
        // An automatic index, built by reading the whole table.
        (
            "SELECT a.sequence, b.sequence FROM ledger_artifacts a \
             CROSS JOIN ledger_artifacts b ON b.producer = a.producer"
                .to_owned(),
            "an automatic index on a ledger table",
        ),
        // Every record of a type, not from a sequence on.
        (
            "SELECT e.canonical_record FROM ledger_events e \
             INDEXED BY ledger_view_index_type_v1 WHERE e.event_type = ?1"
                .to_owned(),
            "the event type index without a sequence bound",
        ),
        // A history-wide index.
        (
            "SELECT e.sequence FROM ledger_events e WHERE +e.event_type = ?1 AND e.severity = ?2"
                .to_owned(),
            "a history-wide index",
        ),
    ] {
        let check = check_plan(&doctored);
        assert!(
            check
                .violations
                .iter()
                .any(|violation| violation.starts_with(rule)),
            "{doctored}: not rejected for {rule}: {check:?}"
        );
    }
}
