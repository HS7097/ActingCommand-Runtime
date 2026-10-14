// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 L-5b (ledger gate 44): the failed-run (K/T) evidence of an eviction intent is
// retired. A formal root whose sealed eviction intent carries `failed_run` evidence in place of
// its `success` source (an intent the retired K/T admission sealed) is refused when it is
// opened: replay does not decode that record (`corrupt_ledger_record` at
// `decode_sqlite_record`) and the writer does not open. The same root with the success-sourced
// intent the writer sealed opens.

use super::missing_material::{capture, formal_root, material_store};
use crate::global::{
    ArtifactEvictionAdmission, GlobalLedger, GlobalLedgerResult, LedgerMaintenance,
    LedgerOpenTiming,
};
use actingcommand_artifact_store::try_artifact_delete_guard;
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};
use serde_json::{Value, json};
use std::sync::Arc;

const DAY_MS: u64 = 86_400_000;
/// The stored eviction intent's event type, as the `ledger_events` table indexes it.
const INTENT_ROW: &str = "event_type='artifact.eviction_intent'";

/// The writer of the existing formal root, opened as a start opens it.
fn reopen(database: &Arc<RuntimeDatabase>) -> GlobalLedgerResult<GlobalLedger> {
    let limits = MaintenanceLimits::default();
    LedgerMaintenance::acquire(
        database.root(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner")
    .open_writer_timed(
        Arc::clone(database),
        "l5b-reopen".into(),
        &mut LedgerOpenTiming::default(),
    )
}

#[test]
fn gate_replay_refuses_an_eviction_intent_carrying_failed_run_evidence() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let sealed = capture(&ledger, &artifacts, b"frame of a sealed eviction intent");
    // The writer seals a success-sourced intent; a stop before the unlink leaves it pending.
    let guard = try_artifact_delete_guard(artifacts.root(), &sealed.reference)
        .expect("delete guard")
        .expect("material guard is free");
    match ledger.admit_artifact_eviction(guard).expect("admission") {
        ArtifactEvictionAdmission::Committed(permit) => drop(permit),
        ArtifactEvictionAdmission::Deferred => panic!("the closed capture is admitted"),
    }
    ledger.close().expect("close with a pending intent");

    // The success-sourced intent replays.
    reopen(&database)
        .expect("a root with a success-sourced intent opens")
        .close()
        .expect("close the reopened writer");

    // The same intent as the retired K/T admission sealed it: `failed_run` evidence over the
    // elapsed-time basis, with the defaults K = 3 and T = 7, in place of the success.
    {
        let connection = database
            .connection(DatabaseLocation::OpenExistingDatabase.location())
            .expect("fixture connection");
        let record: Vec<u8> = connection
            .query_row(
                &format!("SELECT canonical_record FROM ledger_events WHERE {INTENT_ROW}"),
                [],
                |row| row.get(0),
            )
            .expect("the stored intent");
        let mut stored: Value = serde_json::from_slice(&record).expect("stored record JSON");
        let at = stored["timestamp_unix_ms"].as_u64().expect("record time");
        let intent = stored
            .pointer_mut("/payload/payload/data/record/data")
            .and_then(Value::as_object_mut)
            .expect("the stored intent's members");
        let success = intent
            .remove("success")
            .expect("the intent names its success");
        intent.insert(
            "failed_run".to_owned(),
            json!({
                "policy": {"successor_successes": 3, "retention_days": 7},
                "terminal": success,
                "task_result": "failure",
                "terminal_unix_ms": at - 7 * DAY_MS,
                "evaluated_at_unix_ms": at,
                "basis": {"kind": "elapsed_time"},
            }),
        );
        let rewritten = serde_json::to_vec(&stored).expect("record JSON");
        assert_eq!(
            connection
                .execute(
                    &format!("UPDATE ledger_events SET canonical_record=?1 WHERE {INTENT_ROW}"),
                    rusqlite::params![rewritten],
                )
                .expect("rewrite the stored intent"),
            1
        );
    }

    let refused = reopen(&database)
        .err()
        .expect("a root whose intent carries failed-run evidence is refused");
    assert_eq!(
        (refused.code(), refused.operation()),
        ("corrupt_ledger_record", "decode_sqlite_record")
    );
}
