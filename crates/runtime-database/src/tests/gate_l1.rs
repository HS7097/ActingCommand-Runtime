// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan v2.2, L-1 G7 (inventory-ledger key 3) with the fake I2 sync-restore hook:
//! a connection whose `FULL` level could not be restored is refused to every later user
//! (`contracts/runtime-database.md`; `contracts/ledger-store.md`, "Durability classes").

use crate::{
    CommitSync, DatabaseLocation, FULL_SYNC_READBACK_HOOK, RuntimeDatabase, RuntimeDatabaseError,
};
use tempfile::TempDir;

#[test]
fn gate_failed_sync_restore_refuses_every_later_user() {
    let root = TempDir::new().expect("root");
    let database = RuntimeDatabase::open_with_initializer::<RuntimeDatabaseError>(
        root.path(),
        b"gate-l1-sync-restore-seed",
        |_| Ok(()),
        |_| Ok(()),
    )
    .expect("database");
    let guard = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("connection");
    database
        .set_commit_sync(&guard, CommitSync::Normal)
        .expect("relaxed for one observational commit");

    FULL_SYNC_READBACK_HOOK.set(Some(1));
    let restore = database
        .set_commit_sync(&guard, CommitSync::Full)
        .expect_err("a restore that reads back NORMAL fails");
    assert_eq!(restore.code().as_str(), "state_database_sync_config_failed");
    assert_eq!(restore.operation().as_str(), "set_commit_sync");
    assert_eq!(restore.detail(), Some("readback=1"));
    drop(guard);

    let refused = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect_err("connection refuses a later user");
    assert_eq!(refused.code().as_str(), "state_database_sync_relaxed");
    let refused = database
        .try_connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect_err("try_connection refuses a later user");
    assert_eq!(refused.code().as_str(), "state_database_sync_relaxed");
}
