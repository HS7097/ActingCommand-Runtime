// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 A, test plan A-2 G3b (R3′): the FATAL line of a held start that a latched
//! Runtime failure stopped keeps the stop's own code on top and names the latched cause.

#[allow(dead_code)]
#[path = "../../../tests/support/held_runtime.rs"]
mod held_runtime;

use super::{ActingdError, fatal_line};
use actingcommand_runtime_host::{PreparationCheckpoint, PreparationTestAction};
use held_runtime::HeldStart;

#[test]
fn gate_the_fatal_line_of_a_held_start_stopped_by_a_latched_failure_names_the_cause() {
    let root = tempfile::TempDir::new().expect("tempdir");
    let start = HeldStart::spawn(root.path());
    start.reached(PreparationCheckpoint::Held);
    let installer = start.release(root.path());
    start.reached(PreparationCheckpoint::Preparing);
    start.go_on(PreparationTestAction::LatchLedgerFailure {
        operation: "append_runtime_event",
    });
    let stopped = start.finish().err().expect("the held start stops");
    drop(installer);

    assert_eq!(
        fatal_line(&ActingdError::runtime(stopped)),
        "FATAL actingd: runtime host error install_startup_stopped during install_transition \
         cause=ledger_failure cause_operation=append_runtime_event"
    );
}
