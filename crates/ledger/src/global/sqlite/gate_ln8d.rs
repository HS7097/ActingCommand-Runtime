// SPDX-License-Identifier: AGPL-3.0-only

// #422 LN-8d (brief rd-1026/brief-open-stall.md; the v0.12.14 start on the production ledger,
// 2026-10-26): a history page of a link set is the first `rows` linked records after its cursor,
// and SQLite's work for it does not depend on how many linked records follow the page. v0.12.14
// joined the link table and ordered every linked record after the cursor in a temporary B-tree
// to return `rows` of them (`USE TEMP B-TREE FOR ORDER BY` at the statement's top), so a read of
// n linked records in pages of `rows` visited about n² / (2·rows) records: the start's read of the
// production frame store's runs (395 644 records in pages of 256) read about 720 GB.
//
// On the synthetic ledger (LN-1), for each link dimension with every identifier of the history:
// the statement's plan sorts nothing at its top and reads the event rows by their key; the first
// page and a page with only two pages of linked records after it return the linked records in
// ledger order, and the first takes at most twice the work of the late one.

use super::*;
use crate::global::LedgerMaintenance;
use crate::{SyntheticInstance, SyntheticLedgerSpec, write_synthetic_ledger};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits};
use actingcommand_runtime_state::RuntimeStateStore;
use rusqlite::StatementStatus;
use tempfile::TempDir;

// The State key, the writer's owner id and the fixture instance: neutral fixture words.
const STATE_KEY: &[u8] = b"ln8d-page-fixture";
const OWNER: &str = "ln8d-page-fixture";
const INSTANCE_ID: &str = "\"instance_000000000000000000000000000000a1\"";
// Some thirteen runs; a page of 16 rows.
const RECORDS: u64 = 6_000;
const ROWS: usize = 16;
// The work both pages share whatever follows them (the statement's setup).
const SLACK_STEPS: i32 = 2_000;

// A closed formal root holding `RECORDS` synthetic records.
fn synthetic_root() -> (TempDir, Arc<RuntimeDatabase>, u64) {
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
    let spec = SyntheticLedgerSpec::new(
        RECORDS,
        vec![SyntheticInstance {
            alias: "fixture-instance-a".to_owned(),
            instance_id: serde_json::from_str(INSTANCE_ID).expect("a fixture instance id"),
        }],
    );
    let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
    (dir, database, generated.head_sequence)
}

// The link set of every identifier of `column` in the history.
fn every(connection: &Connection, column: &str) -> LinkSet {
    let mut statement = connection
        .prepare(&format!(
            "SELECT DISTINCT {column} FROM ledger_links WHERE {column} IS NOT NULL"
        ))
        .expect("the identifiers' statement");
    let texts = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("the identifiers")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("read the identifiers");
    let mut set = LinkSet::default();
    for text in &texts {
        match column {
            "run_id" => {
                set.runs.insert(id_from_text(text).expect("a run id"));
            }
            "lease_id" => {
                set.leases.insert(id_from_text(text).expect("a lease id"));
            }
            "frame_id" => {
                set.frames.insert(id_from_text(text).expect("a frame id"));
            }
            "request_id" => {
                set.requests
                    .insert(id_from_text(text).expect("a request id"));
            }
            "correlation_id" => {
                set.correlations
                    .insert(id_from_text(text).expect("a correlation id"));
            }
            other => panic!("no link dimension {other}"),
        }
    }
    set
}

// The sequences of the records linked in `column`, in ledger order.
fn linked(connection: &Connection, column: &str) -> Vec<u64> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT sequence FROM ledger_links WHERE {column} IS NOT NULL ORDER BY sequence"
        ))
        .expect("the linked records' statement");
    statement
        .query_map([], |row| row.get::<_, i64>(0))
        .expect("the linked records")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("read the linked records")
        .into_iter()
        .map(decode)
        .collect()
}

// One history page of `selection` after `after`: the sequences it returns and the SQLite
// virtual-machine steps it took.
fn page(
    connection: &Connection,
    selection: &HistorySelection<'_>,
    after: u64,
    head: u64,
) -> (Vec<u64>, i32) {
    let (sql, values) =
        history_statement(selection, None, (after, head), ROWS).expect("the page's statement");
    let mut statement = connection.prepare(&sql).expect("prepare the page");
    let sequences = statement
        .query_map(params_from_iter(values.iter()), |row| row.get::<_, i64>(0))
        .expect("the page")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("read the page")
        .into_iter()
        .map(decode)
        .collect();
    (sequences, statement.get_status(StatementStatus::VmStep))
}

// The steps of the page statement's plan at its top (no parent), with the statement.
fn top_of_plan(
    connection: &Connection,
    selection: &HistorySelection<'_>,
    head: u64,
) -> (Vec<String>, String) {
    let (sql, values) =
        history_statement(selection, None, (0, head), ROWS).expect("the page's statement");
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("prepare the plan");
    let top = statement
        .query_map(params_from_iter(values.iter()), |row| {
            Ok((row.get::<_, i64>(1)?, row.get::<_, String>(3)?))
        })
        .expect("the plan")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("read the plan")
        .into_iter()
        .filter(|(parent, _)| *parent == 0)
        .map(|(_, detail)| detail)
        .collect();
    (top, sql)
}

#[test]
fn ln8d_a_link_set_page_reads_its_page_whatever_follows_it() {
    let (_root, database, head) = synthetic_root();
    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("connection");
    let query = EventQuery::default();
    for column in [
        "run_id",
        "lease_id",
        "frame_id",
        "request_id",
        "correlation_id",
    ] {
        let set = every(&connection, column);
        let selection = HistorySelection::linked(&query, &set);
        let linked = linked(&connection, column);
        let n = linked.len();
        assert!(n >= 16 * ROWS, "{column}: {n} linked records");

        // The plan sorts nothing at its top and reads the event rows by their key.
        let (top, sql) = top_of_plan(&connection, &selection, head);
        assert!(
            top.iter().all(|step| !step.starts_with("USE TEMP B-TREE")),
            "{column}: the page sorts at its top: {top:?}\n{sql}"
        );
        assert!(
            top.iter()
                .any(|step| step == "SEARCH e USING INTEGER PRIMARY KEY (rowid=?)"),
            "{column}: the event rows are not read by their key: {top:?}\n{sql}"
        );

        // The first page, with all but one page of the linked records after it, and a page with
        // one page of them after it: each returns the next linked records in ledger order.
        let (first, first_steps) = page(&connection, &selection, 0, head);
        assert_eq!(first, linked[..ROWS], "{column}: the first page");
        let late_after = linked[n - 2 * ROWS - 1];
        let (late, late_steps) = page(&connection, &selection, late_after, head);
        assert_eq!(
            late,
            linked[n - 2 * ROWS..n - ROWS],
            "{column}: the page after {late_after}"
        );
        // Their work is about the same: it does not follow the linked records after the page.
        assert!(
            first_steps <= 2 * late_steps + SLACK_STEPS,
            "{column}: the first page took {first_steps} VM steps with {} linked records after \
             it, the late page {late_steps} with {ROWS}",
            n - ROWS
        );
    }
}
