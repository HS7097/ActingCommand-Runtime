// SPDX-License-Identifier: AGPL-3.0-only

// #422 LN-VERIFY (brief rd-1031/brief-ln-verify.md, tests 1 and 4; `contracts/ledger-maintenance.md`,
// "`verify` of a formal root"): `ledger-maintenance verify` without flags on a formal root does what
// the start's open does to authenticate the ledger, and nothing more.
// - On the ledger's call, under the read counter (LN-7): the same rows on 3 000 and on 12 000
//   records: the meta row, the aggregates and the newest `open_verify_tail_records` (256 here)
//   records, and no older record.
// - Through `RuntimeHost::maintain_ledger`: a root with an eviction intent without its outcome
//   passes, every file stays as it was but the owner journal, so the eviction is not completed;
//   `--full` still refuses that root.
// The damage, the failure shape and `--frame-query` are `gate_ln_verify.rs` of runtime-host, whose
// host reads a tail from a configuration manifest. No production data: synthetic ledgers and fixture
// records only.

use super::missing_material::{admit, capture, formal_root, material_store};
use crate::global::{LedgerMaintenance, LedgerOpenTiming, LedgerStorageStatus};
use crate::{LedgerReadCounter, SyntheticInstance, SyntheticLedgerSpec, write_synthetic_ledger};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_host::{
    LedgerMaintenanceOperation, LedgerMaintenanceRequest, RuntimeHost, RuntimeHostConfig,
};
use actingcommand_runtime_state::RuntimeStateStore;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// The State key (a bootstrap seed of at least 16 bytes), the writer's owner id and the fixture
// instance: neutral fixture words.
const STATE_KEY: &[u8] = b"ledger-ln-verify-fixture";
const OWNER: &str = "ledger-ln-verify-fixture";
const INSTANCE_ID: &str = "\"instance_000000000000000000000000000000d1\"";
// Two synthetic histories of the same shape, one four times the other.
const SMALL_RECORDS: u64 = 3_000;
const LARGE_RECORDS: u64 = 12_000;
// The tail on the synthetic roots, shorter than both histories.
const TAIL: usize = 256;
// The fingerprint salt of `RuntimeHost::maintain_ledger`, as `missing_material.rs` passes it.
const SALT: &[u8] = b"runtime-host-test-salt";
// The owner journal in a state root (`contracts/owner-journal.md`).
const OWNER_JOURNAL: &str = "owner.lock";

/// A closed formal root holding `records` synthetic records (LN-1).
fn synthetic_root(records: u64) -> tempfile::TempDir {
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
        records,
        vec![SyntheticInstance {
            alias: "fixture-instance-d".to_owned(),
            instance_id: serde_json::from_str(INSTANCE_ID).expect("a fixture instance id"),
        }],
    );
    let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
    assert_eq!(generated.records, records, "{generated:?}");
    dir
}

/// `ledger-maintenance verify` of `root` through the host, with `--full` or without it.
fn host_verify(
    root: &Path,
    full: bool,
) -> Result<
    actingcommand_runtime_host::LedgerMaintenanceReceipt,
    actingcommand_runtime_host::LedgerMaintenanceFailure,
> {
    RuntimeHost::maintain_ledger(
        RuntimeHostConfig::new(root, SALT),
        LedgerMaintenanceRequest {
            operation: LedgerMaintenanceOperation::Verify,
            backup: None,
            target: None,
            artifact_root: None,
            limits: MaintenanceLimits::default(),
            frame_query: false,
            full,
        },
    )
}

/// Every file under `root` with its bytes, but SQLite's own `-wal` and `-shm` sidecars.
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(&directory).expect("a directory of the root") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                directories.push(path);
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if name.ends_with("-wal") || name.ends_with("-shm") {
                continue;
            }
            let bytes = std::fs::read(&path).expect("a file of the root");
            files.insert(
                path.strip_prefix(root)
                    .expect("a path under the root")
                    .to_path_buf(),
                bytes,
            );
        }
    }
    files
}

// Test 1: the default reads the meta row, the aggregates and the tail, the same rows on both
// histories, and no record older than its tail, however long the history.
#[test]
fn ln_verify_t1_the_default_reads_the_same_rows_at_both_scales_and_no_record_older_than_its_tail() {
    let mut read = Vec::new();
    for records in [SMALL_RECORDS, LARGE_RECORDS] {
        let root = synthetic_root(records);
        let database =
            RuntimeDatabase::open_existing_unchecked(root.path()).expect("the root's database");
        let limits = MaintenanceLimits::default();
        let maintenance = LedgerMaintenance::acquire(
            root.path(),
            false,
            limits,
            limits.deadline().expect("deadline"),
        )
        .expect("maintenance owner");
        let mut timing = LedgerOpenTiming::default();
        let counter = LedgerReadCounter::start();
        let status = maintenance
            .verify_formal_bounded(&database, TAIL, &mut timing)
            .expect("the bounded check passes");
        let counts = counter.counts();
        let sequences = counter.sequences_read();
        drop(counter);
        maintenance.close().expect("close the maintenance owner");
        let LedgerStorageStatus::Ready { head_sequence, .. } = status else {
            panic!("a formal root is ready: {status:?}");
        };
        assert!(
            head_sequence >= records,
            "the history and the writer's own records"
        );
        let tail = (head_sequence - TAIL as u64 + 1..=head_sequence).collect::<BTreeSet<_>>();
        assert_eq!(
            sequences.events, tail,
            "{records} records: the event rows of the tail and no other"
        );
        assert_eq!(
            sequences.links, tail,
            "{records} records: the link rows of the tail and no other"
        );
        assert_eq!(counts.events.repeated, 0, "{counts:?}");
        assert_eq!(counts.links.repeated, 0, "{counts:?}");
        assert_eq!(counts.material_files_opened, 0, "{counts:?}");
        assert_eq!(timing.events, TAIL as u64, "{timing:?}");
        read.push((
            counts.events.rows,
            counts.links.rows,
            counts.unidentified,
            counts.other_rows,
        ));
    }
    assert_eq!(
        read[0], read[1],
        "the rows the default read on {SMALL_RECORDS} and on {LARGE_RECORDS} records"
    );
}

// Test 4 (the pending eviction): an intent without its outcome does not fail the default, the
// default does not complete it (no file but the owner journal changes, the frame is still there),
// and `--full` still refuses the root, with the count.
#[test]
fn ln_verify_t4_the_default_passes_a_pending_eviction_and_does_not_complete_it() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let pending = capture(&ledger, &artifacts, b"frame of an unfinished eviction");
    // A hard stop between the intent and the unlink leaves the intent without its outcome.
    drop(admit(&ledger, &artifacts, &pending));
    ledger.close().expect("close with a pending intent");
    drop(artifacts);
    drop(database);

    let mut before = files(root.path());
    let verified = host_verify(root.path(), false).expect("the default passes a pending intent");
    assert_eq!(verified.status, "verified-sqlite-bounded");
    assert!(!verified.activated);

    let mut after = files(root.path());
    let journal = Path::new(OWNER_JOURNAL);
    // This root had no owner before the pass: the journal starts here.
    let (journal_before, journal_after) = (
        before.remove(journal).unwrap_or_default(),
        after.remove(journal).expect("the owner journal"),
    );
    assert!(
        journal_after.starts_with(&journal_before),
        "the owner journal is only appended to"
    );
    assert_eq!(after, before, "no other file changes or appears");
    assert!(pending.material.exists(), "nothing was read or removed");

    let refused = host_verify(root.path(), true).expect_err("--full refuses a pending intent");
    let outcome = refused.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "maintenance_artifact_material_unavailable"
    );
    assert_eq!(
        outcome.value("count"),
        Some(&serde_json::json!(1)),
        "{outcome}"
    );
}
