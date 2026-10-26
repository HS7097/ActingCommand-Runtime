// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-7c V (`rd-1026/ln7c/final-query.md` §7 row V; brief `rd-1026/brief-ln7c-v.md`):
//! `ledger-maintenance verify` runs the frame cleaner's statements W, L and K against the full view
//! over the same root, listings and clock. The root is the case fixture of T-eq (`gate_ln7.rs`):
//! twenty live frames of every class, F5 and F6, and E4's run-less Warning naming W1.
//! - A matching root reports no difference: the check compares every live frame and the kept frame
//!   of a ledger record (a kept file this ledger does not know is left out), and accounts W1's E4,
//!   both on the ledger's own call and through `RuntimeHost::maintain_ledger` (`verify`).
//! - A root whose F5 row was removed consistently (R1's `reading` row deleted, and the frame index's
//!   own pointer reader made to miss it, so `verify`'s check of the index rows passes: a valid root
//!   with a wrong row) is reported: `ledger_index_mismatch` of `verify_frame_query`, R1's
//!   `artifact_id`, `field` `/class`.

use super::gate_ln7::{CHUNK_FRAMES, DAY_MS, EVIDENCE_MAX_AGE_MS, SWITCHES, T0, corpus};
use super::missing_material::{formal_root, material_store};
use crate::global::read_work::DEFAULT_READ_CHECK_VM_STEPS;
use crate::global::sqlite::frame_refs::LedgerFrameRefsOmitGate;
use crate::global::{
    FrameQueryCheck, FrameQueryCheckSettings, LedgerMaintenance, LedgerOpenTiming,
};
use actingcommand_contract::{ArtifactId, IdentifierIssuer};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_host::{
    LedgerMaintenanceOperation, LedgerMaintenanceRequest, RuntimeHost, RuntimeHostConfig,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The check's clock: two days after `T0`, as T-eq's, past every frame's settle age.
const NOW: u64 = T0 + 2 * DAY_MS;
/// The fingerprint salt of `RuntimeHost::maintain_ledger`, as `missing_material.rs` passes it.
const SALT: &[u8] = b"runtime-host-test-salt";

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

/// `ledger-maintenance verify`'s two steps on the ledger's side: the complete verification
/// (records, chain and frame index rows), then the frame query check.
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

#[test]
fn ln7c_v_a_matching_root_reports_no_difference() {
    let (root, database, live) = fixture();
    // L2, a Lab frame, moved into `kept\` as the cleaner names it; beside it a kept file whose
    // artifact this ledger does not know.
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

    // The same through `ledger-maintenance verify` at the host's clock and keys.
    let receipt = RuntimeHost::maintain_ledger(
        RuntimeHostConfig::new(root.path(), SALT),
        LedgerMaintenanceRequest {
            operation: LedgerMaintenanceOperation::Verify,
            backup: None,
            target: None,
            artifact_root: None,
            limits: MaintenanceLimits::default(),
        },
    )
    .expect("verify passes a matching root");
    assert_eq!(receipt.status, "verified-sqlite");
    let checked = receipt
        .frame_query
        .expect("verify checked the frame queries");
    assert_eq!(
        (checked.live_frames, checked.kept_frames, checked.accounted),
        (compared, 1, 1)
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
