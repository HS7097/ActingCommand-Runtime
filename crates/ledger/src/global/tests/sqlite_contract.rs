// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #109 S2: the existing bounded store corpus, using the candidate owner.
use super::*;
use actingcommand_runtime_database::RuntimeDatabase;

pub(in crate::global) fn database(root: &Path) -> Arc<RuntimeDatabase> {
    Arc::new(
        RuntimeDatabase::open_with_initializer::<GlobalLedgerError>(
            root,
            b"sqlite-ledger-contract-seed",
            |_| Ok(()),
            |_| Ok(()),
        )
        .expect("candidate physical owner"),
    )
}

pub(in crate::global) fn open(config: GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger> {
    let database = database(&config.root);
    GlobalLedger::open_sqlite_candidate(config, database)
}

#[test]
fn sqlite_runs_the_existing_seven_store_contracts() {
    let held = std::cell::RefCell::new(None);
    store_contract::query_filters_by_sequence_and_all_typed_correlation_ids(
        |config| {
            let database = match held.borrow().as_ref() {
                Some(database) => Arc::clone(database),
                None => database(&config.root),
            };
            *held.borrow_mut() = Some(Arc::clone(&database));
            GlobalLedger::open_sqlite_candidate(config, database)
        },
        |config| {
            GlobalLedger::open_sqlite_candidate_read_only(
                config,
                Arc::clone(held.borrow().as_ref().expect("same database owner")),
                |_| None,
            )
            .map(Box::new)
            .map(store_contract::ContractReadOnly::Sqlite)
        },
    );
    store_contract::query_pages_are_bounded_ordered_and_pinned_to_the_requested_snapshot(open);
    store_contract::indexes_rebuild_after_reopen(open);
    store_contract::scheduled_settlement_continuation_is_one_shot_and_idempotent(open);
    store_contract::subscription_reports_terminal_writer_failure(open);
    store_contract::scheduling_projection_rejects_a_second_same_run_terminal_with_another_request(
        open,
    );
    store_contract::signature_catalog_rebuilds_versions_and_binds_paginated_multiple_matches(open);
}

#[test]
fn sqlite_owner_and_read_only_snapshot_preserve_live_writer_and_bounds() {
    let root = TempDir::new().expect("root");
    let database = database(root.path());
    let ledger = GlobalLedger::open_sqlite_candidate(config(&root, "owner"), Arc::clone(&database))
        .expect("writer");
    let first = ledger.append(event("first")).expect("first");
    let before = database
        .connection("test snapshot")
        .expect("connection")
        .total_changes();
    let snapshot = GlobalLedger::open_sqlite_candidate_read_only(
        GlobalLedgerReadOnlyConfig::new(root.path()),
        Arc::clone(&database),
        |_| None,
    )
    .expect("snapshot");
    assert_eq!(snapshot.events(), std::slice::from_ref(&first));
    assert_eq!(snapshot.latest_sequence(), 1);
    assert_eq!(
        database
            .connection("test snapshot")
            .expect("connection")
            .total_changes(),
        before
    );
    let conflict =
        GlobalLedger::open_sqlite_candidate(config(&root, "other"), Arc::clone(&database))
            .expect_err("same owner cannot have two ledgers");
    assert_eq!(conflict.code(), "writer_conflict");
    assert!(conflict.is_fatal());
    let bounded = GlobalLedger::open_sqlite_candidate_read_only(
        GlobalLedgerReadOnlyConfig::new(root.path()).with_budget(
            1,
            1,
            Instant::now() + Duration::from_secs(2),
        ),
        Arc::clone(&database),
        |_| None,
    );
    assert_eq!(
        bounded.err().expect("bounded read").code(),
        "ledger_read_budget_exceeded"
    );
    ledger
        .append(event("second"))
        .expect("writer remains usable");
    assert_eq!(snapshot.events(), &[first]);
    ledger.close().expect("close");
    let reopened = GlobalLedger::open_sqlite_candidate(config(&root, "next-owner"), database)
        .expect("ownership released");
    assert_eq!(reopened.latest_sequence().expect("position"), 2);
    reopened.close().expect("close reopened");
}
