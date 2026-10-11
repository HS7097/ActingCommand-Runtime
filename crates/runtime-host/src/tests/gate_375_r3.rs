// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #375 R375-3 v1.2, test plan A-4 G8 (the `quick_check` line): the `check-config`
//! catalog preview opens the state database without the whole-file `PRAGMA quick_check(1)`.
//! One page that no preview statement reads is damaged: the preview still returns the plan of
//! the undamaged root, while the open `ledger-maintenance verify` uses refuses the file.

use super::*;
use actingcommand_runtime_database::RuntimeDatabase;
use std::io::{Seek, SeekFrom};

/// A secondary index of `ledger_links`. The preview reads that table only by its `sequence`
/// primary key, so no preview statement reads a page of this index.
const UNREAD_INDEX: &str = "ledger_links_recognition";

fn preview(root: &Path, catalog: &CatalogSources) -> RuntimeHostResult<CatalogPreview> {
    preview_policy_catalog_transition(&CatalogPreviewRequest {
        state_root: root,
        catalog: Some(catalog),
        transition: None,
        approval_ids: &[],
    })
}

/// Overwrites byte 0 of `index`'s root page (the b-tree page type) with `0x00`, after the WAL
/// is checkpointed into the main file. The page number and size are read from the file.
fn damage_index_root_page(root: &Path, index: &str) {
    let database =
        RuntimeDatabase::open_existing(root, false).expect("open the state database read-write");
    let (database_path, offset) = {
        let connection = database
            .connection("damage one index page")
            .expect("state database connection");
        let busy = connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("checkpoint the WAL");
        assert_eq!(busy, 0, "the checkpoint must not be blocked");
        let page_size = connection
            .query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))
            .expect("page size");
        let root_page = connection
            .query_row(
                "SELECT rootpage FROM sqlite_schema WHERE type = 'index' AND name = ?1",
                [index],
                |row| row.get::<_, i64>(0),
            )
            .expect("root page of the index");
        assert!(root_page > 1, "an index root page follows the schema page");
        let page_size = u64::try_from(page_size).expect("page size is positive");
        let root_page = u64::try_from(root_page).expect("root page is positive");
        (
            database.database_path().to_path_buf(),
            (root_page - 1) * page_size,
        )
    };
    drop(database);
    let mut file = OpenOptions::new()
        .write(true)
        .open(&database_path)
        .expect("open the database file");
    file.seek(SeekFrom::Start(offset))
        .expect("seek to the index root page");
    file.write_all(&[0x00]).expect("overwrite the page type");
    file.sync_all().expect("sync the database file");
}

#[test]
fn gate_check_config_preview_runs_no_whole_file_quick_check() {
    let root = TempDir::new().expect("tempdir");
    let catalog = policy_sources(1);
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let generation = host
        .activate_policy_catalog(&catalog)
        .expect("activate catalog");
    host.close().expect("close host");

    let undamaged = preview(root.path(), &catalog).expect("preview of the undamaged root");
    damage_index_root_page(root.path(), UNREAD_INDEX);

    // (b) The control: the open `ledger-maintenance verify` uses runs the whole-file check and
    // refuses the damaged file.
    match RuntimeDatabase::open_existing(root.path(), true) {
        Ok(_) => panic!("the checked open must refuse a file with a damaged page"),
        Err(error) => {
            assert_eq!(error.code(), "state_database_corrupt");
            assert_eq!(error.operation(), "open_existing_database");
        }
    }

    // (a) The required state: the preview never reads the damaged page and runs no whole-file
    // check, so it returns the undamaged root's plan.
    let damaged = match preview(root.path(), &catalog) {
        Ok(preview) => preview,
        Err(error) => panic!(
            "the preview must not run the whole-file check: {} / {} ({error:?})",
            error.code(),
            error.operation()
        ),
    };
    assert_eq!(damaged.active(), Some(&generation));
    assert_eq!(damaged.active(), undamaged.active());
    assert_eq!(damaged.plan(), undamaged.plan());
    assert_eq!(damaged.approvals(), undamaged.approvals());
}
