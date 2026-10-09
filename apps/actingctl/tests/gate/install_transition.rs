// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 A, test plan A-2 G2b (R1′ + R2): while a held start prepares and its ledger
//! writer is stopped, `actingctl install-transition` answers a Query before the writer goes on.
//! The writer is stopped by the stall gate (LED-I3), not by a timed pause.

use crate::held_runtime::{HELD_TRANSITION_ID, HeldStart};
use actingcommand_ledger::WriterStallGate;
use actingcommand_runtime_host::{PreparationCheckpoint, PreparationTestAction};
use serde_json::Value;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn gate_install_transition_query_answers_while_the_ledger_writer_is_stalled_in_preparing() {
    let root = TempDir::new().expect("tempdir");
    let start = HeldStart::spawn(root.path());
    start.reached(PreparationCheckpoint::Held);
    let installer = start.release(root.path());
    start.reached(PreparationCheckpoint::Preparing);

    let gate = WriterStallGate::arm(root.path()).expect("arm the writer stall gate");
    let query = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .arg("install-transition")
        .arg("--state-root")
        .arg(root.path())
        .arg("--action-json")
        .arg(format!(
            r#"{{"action":"query","transition_id":"{HELD_TRANSITION_ID}"}}"#
        ))
        .output()
        .expect("run actingctl install-transition");
    // The writer goes on only after the run has ended.
    gate.release();
    start.go_on(PreparationTestAction::Continue);
    let released = start.finish().expect("the released start finishes");
    drop(installer);
    released.close().expect("close the released Runtime");

    assert!(
        query.status.success(),
        "actingctl install-transition query: {}; stdout: {}; stderr: {}",
        query.status,
        String::from_utf8_lossy(&query.stdout),
        String::from_utf8_lossy(&query.stderr)
    );
    let output: Value = serde_json::from_slice(&query.stdout).expect("actingctl JSON");
    assert_eq!(
        output["receipt"]["result"]["status"]["phase"], "preparing",
        "{output}"
    );
}
