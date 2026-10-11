// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan v2.2, L-1 G13 (inventory-ledger key 33): maintenance limits, disjoint backup
//! paths and relative destinations (`contracts/ledger-maintenance.md`).

use actingcommand_runtime_database::{
    MaintenanceLimits, RuntimeDatabase, RuntimeDatabaseError, require_disjoint,
    resolve_maintenance_destination,
};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn gate_maintenance_limits_accept_defaults_and_caps_and_refuse_zero_and_over_cap() {
    let defaults = MaintenanceLimits::default();
    assert_eq!(defaults.max_bytes, 512 * 1024 * 1024);
    assert_eq!(defaults.max_files, 16_384);
    assert_eq!(defaults.max_events, 200_000);
    assert_eq!(defaults.timeout_seconds, 120);
    assert_eq!(defaults.pages_per_step, 128);
    assert!(defaults.deadline().is_ok(), "the defaults are valid");

    type Set = fn(&mut MaintenanceLimits, u64);
    let fields: [(&str, u64, Set); 5] = [
        ("max_bytes", 4 * 1024 * 1024 * 1024, |limits, value| {
            limits.max_bytes = value;
        }),
        ("max_files", 65_536, |limits, value| {
            limits.max_files = usize::try_from(value).expect("entries");
        }),
        ("max_events", 1_000_000, |limits, value| {
            limits.max_events = usize::try_from(value).expect("events");
        }),
        ("timeout_seconds", 600, |limits, value| {
            limits.timeout_seconds = value;
        }),
        ("pages_per_step", 1_024, |limits, value| {
            limits.pages_per_step = i32::try_from(value).expect("pages");
        }),
    ];
    for (field, cap, set) in fields {
        for (value, valid) in [(0, false), (cap, true), (cap + 1, false)] {
            let mut limits = MaintenanceLimits::default();
            set(&mut limits, value);
            assert_eq!(
                limits.deadline().is_ok(),
                valid,
                "{field} = {value}: accepted must be {valid}"
            );
        }
    }
}

#[test]
fn gate_maintenance_backup_path_must_be_disjoint_from_the_source() {
    let root = TempDir::new().expect("root");
    let source = root.path().join("source");
    fs::create_dir(&source).expect("source directory");

    assert!(
        require_disjoint(&source, &source).is_err(),
        "the target is the source"
    );
    assert!(
        require_disjoint(&source, &source.join("backup")).is_err(),
        "the target is inside the source"
    );
    assert!(
        require_disjoint(&source, root.path()).is_err(),
        "the source is inside the target"
    );
    let sibling = require_disjoint(&source, &root.path().join("backup"));
    assert!(sibling.is_ok(), "a sibling target: {sibling:?}");
}

#[test]
fn gate_relative_maintenance_destination_resolves_against_the_working_directory() {
    let name = "gate-l1-relative-maintenance-destination";
    assert!(
        !Path::new(name).exists(),
        "{name} must not exist in the working directory"
    );
    let expected = std::env::current_dir()
        .expect("working directory")
        .canonicalize()
        .expect("canonical working directory")
        .join(name);

    let resolved = resolve_maintenance_destination(Path::new(name)).expect("resolved");

    assert_eq!(resolved, expected);
}

/// One-off probe (L-1 G14, to be reverted): does a held WAL read snapshot on the R4b reader make
/// `backup` meet Busy? The panic prints the outcome on CI.
#[test]
fn probe_backup_busy_reachability() {
    #[allow(dead_code)] // Read only through Debug when the open fails.
    #[derive(Debug)]
    struct ProbeError(String);
    impl From<RuntimeDatabaseError> for ProbeError {
        fn from(error: RuntimeDatabaseError) -> Self {
            Self(error.to_string())
        }
    }

    let root = TempDir::new().expect("root");
    let database = RuntimeDatabase::open_with_initializer::<ProbeError>(
        &root.path().join("source"),
        b"probe-backup-busy-seed",
        |_| Ok(()),
        |connection| {
            connection
                .execute_batch(
                    "CREATE TABLE probe (value INTEGER NOT NULL); INSERT INTO probe VALUES (1);",
                )
                .map_err(|error| ProbeError(error.to_string()))
        },
    )
    .expect("database");
    let reader = database
        .read_snapshot_connection("probe")
        .expect("snapshot reader");
    reader.execute_batch("BEGIN").expect("begin");
    let value: i64 = reader
        .query_row("SELECT value FROM probe", [], |row| row.get(0))
        .expect("snapshot read");

    let limits = MaintenanceLimits::default();
    let destination = TempDir::new().expect("destination root");
    let outcome = match database.backup(
        &destination.path().join("backup"),
        &[],
        &[],
        serde_json::json!({}),
        limits,
        limits.deadline().expect("deadline"),
    ) {
        Ok(backup) => format!("Ok warnings={:?}", backup.warnings),
        Err(error) => format!("Err code={} warnings={:?}", error.code(), error.warnings()),
    };
    panic!("probe: snapshot_value={value} {outcome}");
}
