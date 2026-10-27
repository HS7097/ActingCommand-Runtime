// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-7c V (`rd-1026/ln7c/final-query.md` §7 row V; briefs `rd-1026/brief-ln7c-v.md` and
//! `brief-ln7c-v-fix1.md`): `ledger-maintenance verify --frame-query` runs the frame cleaner's
//! statements W, L and K against the full view over the same root, listings and clock; the check is
//! opt-in. The root is the case fixture of T-eq (`gate_ln7.rs`): twenty live frames of every class,
//! F5 and F6, and E4's run-less Warning naming W1.
//! - A matching root reports no difference: the check compares every live frame and the kept frame
//!   of a ledger record (a kept file this ledger does not know is left out), and accounts W1's E4,
//!   both on the ledger's own call (what the flag runs) and through `RuntimeHost::maintain_ledger`
//!   (`verify` with the flag).
//! - A root whose F5 row was removed consistently (R1's `reading` row deleted, and the frame index's
//!   own pointer reader made to miss it, so `verify`'s check of the index rows passes: a valid root
//!   with a wrong row) is reported, on the ledger's call that the flag runs: `ledger_index_mismatch`
//!   of `verify_frame_query`, R1's `artifact_id`, `field` `/class`.
//! - Without the flag, `verify` of the same matching root answers as before the check existed: the
//!   receipt of the complete read alone, and no count for actingd's stderr line.
//! - Fix round 1b (Fail Loud): the flag is never ignored. With an operation other than `verify`,
//!   or on a root that is not formal (a Segment root), it is refused with
//!   `maintenance_arguments_invalid`, and nothing is written.

use super::gate_ln7::{CHUNK_FRAMES, DAY_MS, EVIDENCE_MAX_AGE_MS, SWITCHES, T0, corpus};
use super::missing_material::{formal_root, material_store};
use crate::global::read_work::DEFAULT_READ_CHECK_VM_STEPS;
use crate::global::sqlite::frame_refs::LedgerFrameRefsOmitGate;
use crate::global::{
    FrameQueryCheck, FrameQueryCheckSettings, GlobalLedger, GlobalLedgerConfig, LedgerMaintenance,
    LedgerOpenTiming,
};
use actingcommand_contract::{ArtifactId, IdentifierIssuer};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_host::{
    LedgerMaintenanceOperation, LedgerMaintenanceRequest, RuntimeHost, RuntimeHostConfig,
};
use actingcommand_runtime_state::RuntimeStateStore;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The check's clock: two days after `T0`, as T-eq's, past every frame's settle age.
const NOW: u64 = T0 + 2 * DAY_MS;
/// The fingerprint salt of `RuntimeHost::maintain_ledger`, as `missing_material.rs` passes it.
const SALT: &[u8] = b"runtime-host-test-salt";
/// The owner journal in a state root (`contracts/owner-journal.md`).
const OWNER_JOURNAL: &str = "owner.lock";

fn text<T: serde::Serialize>(id: &T) -> String {
    match serde_json::to_value(id) {
        Ok(serde_json::Value::String(text)) => text,
        other => panic!("identifier text: {other:?}"),
    }
}

/// What the check runs with: T-eq's switches and the keys' defaults, at `NOW`.
fn settings() -> FrameQueryCheckSettings {
    FrameQueryCheckSettings {
        now_unix_ms: NOW,
        switches: SWITCHES,
        evidence_max_age_days: EVIDENCE_MAX_AGE_MS / DAY_MS,
        chunk_frames: CHUNK_FRAMES,
        check_vm_steps: DEFAULT_READ_CHECK_VM_STEPS,
    }
}

/// The case fixture on a closed formal root: the root, its database and the corpus's frames.
fn fixture() -> (
    tempfile::TempDir,
    Arc<RuntimeDatabase>,
    super::gate_ln7::Live,
) {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let live = corpus(&ledger, &artifacts);
    ledger.close().expect("close the writer");
    drop(artifacts);
    (root, database, live)
}

/// `ledger-maintenance verify --frame-query`'s two steps on the ledger's side: the complete
/// verification (records, chain and frame index rows), then the frame query check.
fn verify(
    root: &Path,
    database: &Arc<RuntimeDatabase>,
) -> crate::global::GlobalLedgerResult<FrameQueryCheck> {
    let limits = MaintenanceLimits::default();
    let maintenance =
        LedgerMaintenance::acquire(root, false, limits, limits.deadline().expect("deadline"))
            .expect("maintenance owner");
    let (events, _) = maintenance
        .verify_formal(database, &mut LedgerOpenTiming::default())
        .expect("the complete verification passes: records, chain and frame index rows agree");
    let checked = maintenance.verify_frame_queries(database, &events, settings());
    maintenance.close().expect("close the maintenance owner");
    checked
}

/// The live frame file of `artifact`: `artifacts\<shard>\<artifact>.png`.
fn live_file(root: &Path, artifact: ArtifactId) -> PathBuf {
    let name = format!("{}.png", text(&artifact));
    std::fs::read_dir(root.join("artifacts"))
        .expect("the frame store")
        .map(|shard| shard.expect("a shard").path().join(&name))
        .find(|path| path.is_file())
        .expect("the frame's file")
}

/// What `verify` answered of a formal root before the check existed: the complete read's ledger
/// status, as JSON (the host's copy of the ledger crate is another type).
fn complete_read_status(root: &Path, database: &Arc<RuntimeDatabase>) -> serde_json::Value {
    let limits = MaintenanceLimits::default();
    let maintenance =
        LedgerMaintenance::acquire(root, false, limits, limits.deadline().expect("deadline"))
            .expect("maintenance owner");
    let (_, ledger) = maintenance
        .verify_formal(database, &mut LedgerOpenTiming::default())
        .expect("the complete verification passes");
    maintenance.close().expect("close the maintenance owner");
    serde_json::to_value(ledger).expect("the ledger status")
}

type HostAnswer = Result<
    actingcommand_runtime_host::LedgerMaintenanceReceipt,
    actingcommand_runtime_host::LedgerMaintenanceFailure,
>;

/// `ledger-maintenance <operation>` of `root` through the host, at its clock and keys.
fn host_maintain(
    root: &Path,
    operation: LedgerMaintenanceOperation,
    backup: Option<PathBuf>,
    target: Option<PathBuf>,
    frame_query: bool,
) -> HostAnswer {
    RuntimeHost::maintain_ledger(
        RuntimeHostConfig::new(root, SALT),
        LedgerMaintenanceRequest {
            operation,
            backup,
            target,
            artifact_root: None,
            limits: MaintenanceLimits::default(),
            frame_query,
        },
    )
}

/// `ledger-maintenance verify` of `root` through the host, with or without `--frame-query`.
fn host_verify(root: &Path, frame_query: bool) -> HostAnswer {
    host_maintain(
        root,
        LedgerMaintenanceOperation::Verify,
        None,
        None,
        frame_query,
    )
}

/// A refusal's code and operation.
fn refusal(answer: HostAnswer) -> (String, Option<String>) {
    let failure = answer.expect_err("the request is refused");
    let outcome = failure.outcome();
    (
        outcome.code().as_str().to_owned(),
        outcome
            .value("operation")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    )
}

/// The refusal of `--frame-query` where the check cannot run (fix round 1b).
fn arguments_invalid() -> (String, Option<String>) {
    (
        "maintenance_arguments_invalid".to_owned(),
        Some("validate_ledger_maintenance".to_owned()),
    )
}

/// A Segment root (not formal): the State material and a closed Segment ledger, as a root before
/// its cutover.
fn segment_root() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("root");
    drop(RuntimeStateStore::open(root.path(), SALT).expect("the State material"));
    GlobalLedger::open(GlobalLedgerConfig::new(
        root.path().join("ledger"),
        "ln7c-v-segment",
    ))
    .expect("a Segment ledger")
    .close()
    .expect("close the Segment ledger");
    root
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

/// The matching root: the case fixture with L2, a Lab frame, moved into `kept\` as the cleaner
/// names it, and beside it a kept file whose artifact this ledger does not know.
fn matching_root() -> (
    tempfile::TempDir,
    Arc<RuntimeDatabase>,
    super::gate_ln7::Live,
) {
    let (root, database, live) = fixture();
    let leaf = root
        .path()
        .join(actingcommand_artifact_store::KEPT_DIRECTORY)
        .join("2026-01-01")
        .join("lab-fixture");
    std::fs::create_dir_all(&leaf).expect("a kept leaf");
    std::fs::rename(
        live_file(root.path(), live.l2),
        leaf.join(format!("000330-000_{}.png", text(&live.l2))),
    )
    .expect("move L2 into kept");
    let unknown = *IdentifierIssuer::new()
        .expect("issuer")
        .mint_artifact_id()
        .expect("artifact")
        .transport();
    std::fs::write(
        leaf.join(format!("000331-000_{}.png", text(&unknown))),
        b"a kept file of another ledger",
    )
    .expect("an unknown kept file");
    (root, database, live)
}

#[test]
fn ln7c_v_a_matching_root_reports_no_difference() {
    let (root, database, live) = matching_root();

    // Every live frame but L2 compared, the kept L2 compared, W1's E4 accounted, nothing reported.
    let compared = live.all.len() as u64 - 1;
    assert_eq!(
        verify(root.path(), &database).expect("a matching root reports no difference"),
        FrameQueryCheck {
            live_frames: compared,
            kept_frames: 1,
            accounted: 1,
        }
    );
    drop(database);

    // The same through `ledger-maintenance verify --frame-query` at the host's clock and keys.
    let receipt = host_verify(root.path(), true).expect("verify passes a matching root");
    assert_eq!(receipt.status, "verified-sqlite");
    let checked = receipt
        .frame_query
        .expect("verify --frame-query checked the frame queries");
    assert_eq!(
        (checked.live_frames, checked.kept_frames, checked.accounted),
        (compared, 1, 1)
    );
}

#[test]
fn ln7c_v_without_the_flag_verify_answers_the_complete_read_alone() {
    let (root, database, _) = matching_root();
    let ledger = complete_read_status(root.path(), &database);
    drop(database);

    // `verify` without `--frame-query`: the receipt of the complete read alone, and no count, so
    // actingd prints no frame query line.
    let receipt = host_verify(root.path(), false).expect("verify passes a matching root");
    assert!(
        receipt.frame_query.is_none(),
        "no frame query count without the flag: {:?}",
        receipt.frame_query
    );
    assert_eq!(
        serde_json::to_value(&receipt).expect("the receipt"),
        serde_json::json!({
            "schema_version": "actingcommand.ledger-maintenance.v2",
            "status": "verified-sqlite",
            "ledger": ledger,
            "backup_id": null,
            "warnings": [],
            "activated": false,
        })
    );
}

#[test]
fn ln7c_v_a_consistently_removed_frame_index_row_is_reported() {
    let (root, database, live) = fixture();
    // R1's `reading` row, its only resource reading, goes; the frame index's pointer reader misses
    // it from now on, so the index is consistent with every record as `verify` checks it.
    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("the shared connection");
    let readings: i64 = connection
        .query_row(
            "SELECT count(*) FROM ledger_frame_refs WHERE pointer='reading'",
            [],
            |row| row.get(0),
        )
        .expect("the reading rows");
    assert_eq!(readings, 1, "R1's reading row is the corpus's only one");
    let target: String = connection
        .query_row(
            "SELECT target_id FROM ledger_frame_refs WHERE pointer='reading'",
            [],
            |row| row.get(0),
        )
        .expect("R1's reading row");
    connection
        .execute("DELETE FROM ledger_frame_refs WHERE pointer='reading'", [])
        .expect("remove R1's reading row");
    drop(connection);
    let _missed = LedgerFrameRefsOmitGate::arm(target);

    let refused = verify(root.path(), &database).expect_err("the wrong row is reported");
    assert_eq!(
        (refused.code(), refused.operation()),
        ("ledger_index_mismatch", "verify_frame_query")
    );
    let outcome = refused
        .outcome_values()
        .expect("the finding's outcome with its values");
    assert_eq!(
        outcome.value("artifact_id"),
        Some(&serde_json::json!(text(&live.r1)))
    );
    // Statement L classes R1 default (removed at 1 day) where the view classes it resource.
    assert_eq!(outcome.value("field"), Some(&serde_json::json!("/class")));
    let detail = refused.detail().unwrap_or_default();
    assert!(
        detail.contains("phase=verify_frame_query")
            && detail.contains("statement=L")
            && detail.contains("query=Default reference=Resource")
            && detail.contains("differences=1"),
        "{detail}"
    );
}

/// Fix round 1b (a): `--frame-query` with an operation other than `verify` is refused as an
/// argument before the root is touched: no file of the root changes and none appears (not even
/// the owner journal), and no destination is created.
#[test]
fn ln7c_v_frame_query_with_another_operation_is_refused_before_the_root_is_touched() {
    let root = segment_root();
    let destinations = tempfile::tempdir().expect("maintenance destinations");
    let backup = destinations.path().join("backup");
    let restored = destinations.path().join("restored");
    let before = files(root.path());
    for (operation, target) in [
        (LedgerMaintenanceOperation::DryRun, None),
        (LedgerMaintenanceOperation::Import, None),
        (LedgerMaintenanceOperation::Backup, None),
        (LedgerMaintenanceOperation::Restore, Some(restored.clone())),
    ] {
        assert_eq!(
            refusal(host_maintain(
                root.path(),
                operation,
                Some(backup.clone()),
                target,
                true,
            )),
            arguments_invalid(),
            "{operation:?} --frame-query"
        );
        assert_eq!(
            files(root.path()),
            before,
            "{operation:?} --frame-query touches nothing"
        );
        assert!(!backup.exists() && !restored.exists(), "{operation:?}");
    }
}

/// Fix round 1b (b): `--frame-query` on a root that is not formal (a Segment root) is refused as
/// an argument, never ignored; without the flag the same root verifies. The refusal writes
/// nothing: every file is unchanged but the owner journal, which only gains the records every
/// maintenance pass appends (SQLite's own `-wal` and `-shm` sidecars are not compared).
#[test]
fn ln7c_v_frame_query_on_a_non_formal_root_is_refused() {
    let root = segment_root();
    let verified = host_verify(root.path(), false).expect("verify passes the Segment root");
    assert_eq!(verified.status, "verified-segment");
    assert!(verified.frame_query.is_none());

    let mut before = files(root.path());
    assert_eq!(refusal(host_verify(root.path(), true)), arguments_invalid());
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
}
