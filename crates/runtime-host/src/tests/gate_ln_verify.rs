// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-VERIFY (brief rd-1031/brief-ln-verify.md, tests 2 to 5; `contracts/ledger-maintenance.md`,
//! "`verify` of a formal root"): through `RuntimeHost::maintain_ledger`, on a real closed root whose
//! tail (`ledger_observer.open_verify_tail_records`, from the configuration manifest as actingd
//! gives it) is shorter than its history.
//! - The default refuses a damaged head, a damaged tail record and a gap in the sequences, with the
//!   code, operation, category and note link the complete read gives.
//! - The default passes a damaged old record and a damaged page no statement of it reads; `--full`
//!   refuses both, and `--frame-query` still does the complete read.
//! - The default and `--full` report the same `ledger`; the default leaves every file as it was but
//!   the owner journal.
//! - `--full` with another operation is refused before the root is touched, as `--frame-query` is.

use super::gate_375_r3::{UNREAD_INDEX, damage_index_root_page};
use super::*;
use crate::{
    LedgerMaintenanceFailure, LedgerMaintenanceOperation, LedgerMaintenanceReceipt,
    LedgerMaintenanceRequest,
};
use actingcommand_contract::RuntimeConfigManifest;
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};

/// The tail of these roots.
const TAIL: u64 = 3;
/// The instance facts a root holds; the first is older than the tail.
const FACTS: u64 = 8;
/// The owner journal in a state root (`contracts/owner-journal.md`).
const OWNER_JOURNAL: &str = "owner.lock";

fn fact(index: u64) -> FactRecord {
    FactRecord {
        scope: FactScope::Server {
            server_id: "fixture-server".to_owned(),
        },
        key: format!("env.ln_verify_fixture_{index}"),
        content: FactContent::Inline {
            value: ContractFactValue::Boolean(true),
        },
        observed_at_unix_ms: 1_000,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 1_000,
        source_detector: "detector.fixture".to_owned(),
        // One observation per fact: a second record under a snapshot id that names an active
        // record is refused (`fact_observation_version_conflict`).
        source_snapshot_id: format!("snapshot:ln-verify-{index}"),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "a".repeat(64),
        invalidate_on: Vec::new(),
    }
}

/// A closed root that a Runtime started on and wrote `FACTS` instance facts to.
fn closed_root() -> TempDir {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    for index in 0..FACTS {
        host.publish_fact(fact(index)).expect("publish a fact");
    }
    host.close().expect("close the runtime host");
    root
}

/// The host's configuration with `TAIL` as `ledger_observer.open_verify_tail_records`.
fn tail_config(root: &TempDir) -> RuntimeHostConfig {
    let (_, defaults) =
        actingcommand_runtime_config::DefaultsFile::shipped().expect("the shipped defaults");
    let file = serde_json::json!({"ledger_observer": {"open_verify_tail_records": TAIL}});
    let mut parameters = actingcommand_runtime_config::render_parameters(
        &actingcommand_runtime_config::mechanism_rows(),
        &[],
        &defaults,
        &file,
        &|_| None,
    )
    .expect("every key resolves");
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    config(root).with_config_manifest(RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    })
}

type Answer = Result<LedgerMaintenanceReceipt, LedgerMaintenanceFailure>;

/// `ledger-maintenance verify` of `root`, with these flags.
fn verify(root: &TempDir, full: bool, frame_query: bool) -> Answer {
    RuntimeHost::maintain_ledger(
        tail_config(root),
        LedgerMaintenanceRequest {
            operation: LedgerMaintenanceOperation::Verify,
            backup: None,
            target: None,
            artifact_root: None,
            limits: MaintenanceLimits::default(),
            frame_query,
            full,
        },
    )
}

/// What a caller reads of a failure: its code, its operation and its category.
fn shape(failure: &LedgerMaintenanceFailure) -> (String, Option<String>, String) {
    let outcome = failure.outcome();
    (
        outcome.code().as_str().to_owned(),
        outcome
            .value("operation")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        outcome.category().as_str().to_owned(),
    )
}

/// The newest record's and the first fact's sequence, as the ledger tables store them.
fn stored_sequences(root: &TempDir) -> (i64, i64) {
    let database = RuntimeDatabase::open_existing(root.path(), true).expect("state database");
    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("state database connection");
    let head: i64 = connection
        .query_row("SELECT MAX(sequence) FROM ledger_events", [], |row| {
            row.get(0)
        })
        .expect("the newest record");
    let first_fact: i64 = connection
        .query_row(
            "SELECT sequence FROM ledger_events WHERE instr(canonical_record, 'env.ln_verify_fixture_0') > 0 ORDER BY sequence LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("the first fact's row");
    assert!(
        first_fact + i64::try_from(TAIL).expect("the tail") < head,
        "the first fact is older than the tail"
    );
    (head, first_fact)
}

fn damage(root: &TempDir, sql: &str) {
    let database = RuntimeDatabase::open_existing(root.path(), false).expect("state database");
    database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("state database connection")
        .execute_batch(sql)
        .expect("damage the fixture");
}

/// The statements that take the row `stored` out of the ledger tables.
fn delete_row(stored: i64) -> String {
    format!(
        "PRAGMA foreign_keys=OFF; DELETE FROM ledger_links WHERE sequence={stored}; DELETE FROM ledger_artifacts WHERE sequence={stored}; DELETE FROM ledger_events WHERE sequence={stored}"
    )
}

/// Every file under `root` with its bytes, but SQLite's own `-wal` and `-shm` sidecars.
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).expect("a directory of the root") {
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
            let bytes = fs::read(&path).expect("a file of the root");
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

/// One damage the default refuses: its label, its SQL for the root's head and first fact, the code
/// and operation it gives, whether the complete read gives the same, and whether the read was under
/// way (the note link of its phases: the keyed meta row is checked before it, as it always was).
struct Damage {
    label: &'static str,
    sql: fn(i64, i64) -> String,
    code: &'static str,
    operation: &'static str,
    same_as_full: bool,
    measured: bool,
}

#[test]
fn ln_verify_t2_the_default_refuses_a_damaged_head_a_damaged_tail_record_and_a_gap() {
    let cases = [
        Damage {
            label: "the keyed meta row",
            sql: |_, _| "UPDATE ledger_meta SET integrity_tag='ln-verify-invalid-tag'".to_owned(),
            code: "ledger_meta_mismatch",
            operation: "verify_sqlite_head",
            same_as_full: true,
            measured: false,
        },
        Damage {
            label: "the head's record",
            sql: |head, _| {
                format!(
                    "UPDATE ledger_events SET canonical_record=CAST('ln-verify-invalid-json' AS BLOB) WHERE sequence={head}"
                )
            },
            code: "corrupt_ledger_record",
            operation: "parse_sqlite_record",
            same_as_full: true,
            measured: true,
        },
        Damage {
            label: "the head row",
            sql: |head, _| delete_row(head),
            code: "ledger_meta_mismatch",
            operation: "verify_sqlite_head",
            same_as_full: true,
            measured: true,
        },
        Damage {
            label: "a tail record's tag",
            sql: |head, _| {
                format!(
                    "UPDATE ledger_events SET integrity_tag='ln-verify-invalid-tag' WHERE sequence={}",
                    head - 1
                )
            },
            code: "ledger_record_mismatch",
            operation: "verify_sqlite_record",
            same_as_full: true,
            measured: true,
        },
        Damage {
            label: "a gap older than the tail",
            sql: |_, first_fact| delete_row(first_fact),
            code: "sequence_discontinuity",
            operation: "recover_sequence",
            same_as_full: false,
            measured: true,
        },
    ];
    for Damage {
        label,
        sql,
        code,
        operation,
        same_as_full,
        measured,
    } in cases
    {
        let root = closed_root();
        let (head, first_fact) = stored_sequences(&root);
        damage(&root, &sql(head, first_fact));
        let refused = verify(&root, false, false).expect_err(label);
        let (actual_code, actual_operation, category) = shape(&refused);
        assert_eq!(
            (actual_code.as_str(), actual_operation.as_deref()),
            (code, Some(operation)),
            "{label}"
        );
        let outcome = refused.outcome();
        assert_eq!(
            outcome.causes().iter().any(|link| {
                link.relation().as_str() == "note"
                    && link.outcome().code().as_str() == "ledger_open_measured"
            }),
            measured,
            "{label}: {outcome}"
        );
        let full = verify(&root, true, false).expect_err(label);
        if same_as_full {
            assert_eq!(
                shape(&full),
                (actual_code, actual_operation, category),
                "{label}"
            );
        }
    }
}

#[test]
fn ln_verify_t3_the_default_passes_old_damage_that_full_and_frame_query_refuse() {
    // A damaged old record.
    let root = closed_root();
    let (_, first_fact) = stored_sequences(&root);
    damage(
        &root,
        &format!(
            "UPDATE ledger_events SET integrity_tag='ln-verify-invalid-tag' WHERE sequence={first_fact}"
        ),
    );
    let passed =
        verify(&root, false, false).expect("the default reads no record older than its tail");
    assert_eq!(passed.status, "verified-sqlite-bounded");
    assert!(passed.frame_query.is_none());
    let complete = (
        "ledger_record_mismatch".to_owned(),
        Some("verify_sqlite_record".to_owned()),
    );
    for (flags, full, frame_query) in [("--full", true, false), ("--frame-query", false, true)] {
        let refused = verify(&root, full, frame_query).expect_err(flags);
        let (code, operation, _) = shape(&refused);
        assert_eq!((code, operation), complete, "{flags}");
    }

    // A damaged page no statement of the default reads: the whole-file check is `--full`'s alone.
    let root = closed_root();
    damage_index_root_page(root.path(), UNREAD_INDEX);
    let passed = verify(&root, false, false).expect("the default runs no whole-file check");
    assert_eq!(passed.status, "verified-sqlite-bounded");
    let refused = verify(&root, true, false).expect_err("--full runs the whole-file check");
    let (code, operation, _) = shape(&refused);
    assert_eq!(
        (code.as_str(), operation.as_deref()),
        ("state_database_corrupt", Some("open_existing_database"))
    );
}

#[test]
fn ln_verify_t4_the_default_changes_nothing_and_reports_the_ledger_of_the_complete_read() {
    let root = closed_root();
    let mut before = files(root.path());
    let bounded = verify(&root, false, false).expect("the default passes a sound root");
    assert_eq!(bounded.status, "verified-sqlite-bounded");
    assert!(!bounded.activated);

    let mut after = files(root.path());
    let journal = Path::new(OWNER_JOURNAL);
    let (journal_before, journal_after) = (
        before.remove(journal).expect("the owner journal"),
        after.remove(journal).expect("the owner journal"),
    );
    assert!(
        journal_after.starts_with(&journal_before),
        "the owner journal is only appended to"
    );
    assert_eq!(after, before, "no other file changes or appears");

    let complete = verify(&root, true, false).expect("--full passes a sound root");
    assert_eq!(complete.status, "verified-sqlite");
    assert_eq!(
        serde_json::to_value(&bounded.ledger).expect("the ledger status"),
        serde_json::to_value(&complete.ledger).expect("the ledger status"),
        "the same head, head hash and migration"
    );
}

#[test]
fn ln_verify_t5_full_with_another_operation_is_refused_before_the_root_is_touched() {
    let root = closed_root();
    let destinations = TempDir::new().expect("maintenance destinations");
    let backup = destinations.path().join("backup");
    let before = files(root.path());
    let refused = RuntimeHost::maintain_ledger(
        tail_config(&root),
        LedgerMaintenanceRequest {
            operation: LedgerMaintenanceOperation::Backup,
            backup: Some(backup.clone()),
            target: None,
            artifact_root: None,
            limits: MaintenanceLimits::default(),
            frame_query: false,
            full: true,
        },
    )
    .expect_err("--full is an option of verify");
    let (code, operation, _) = shape(&refused);
    assert_eq!(
        (code.as_str(), operation.as_deref()),
        (
            "maintenance_arguments_invalid",
            Some("validate_ledger_maintenance")
        )
    );
    assert_eq!(files(root.path()), before, "nothing changes");
    assert!(!backup.exists());
}
