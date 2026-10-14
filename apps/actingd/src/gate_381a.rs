// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #381 A, test plan A-2 G3b (R3′): the last daemon lines of a held start that a
// latched Runtime failure stopped keep the stop's own code on top and name the latched cause.
// Workflow #378 A6a: the stop is the terminal `OUTCOME actingd:` line, the latched cause its
// `caused_by` link (it was the `FATAL actingd:` line's `cause=` text).

#[allow(dead_code)]
#[path = "../../../tests/support/held_runtime.rs"]
mod held_runtime;

use super::{ActingdError, daemon_lines};
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
        operation: actingcommand_runtime_host::HostLocation::AppendRuntimeEvent.location(),
    });
    let stopped = start.finish().err().expect("the held start stops");
    drop(installer);

    let error = ActingdError::runtime(stopped);
    let lines = daemon_lines(&error.outcome(), Some(&error.to_string()));
    let mut lines = lines.lines();
    // The developer line keeps the stop's code and names the latched cause.
    assert_eq!(
        lines.next(),
        Some(
            "ERROR actingd: install_startup_stopped runtime host error install_startup_stopped \
             during install_transition cause=ledger_failure cause_operation=append_runtime_event"
        )
    );
    // The OUTCOME line: the stop on top, the latched condition as its `caused_by` link.
    let envelope: serde_json::Value = serde_json::from_str(
        lines
            .next()
            .and_then(|line| line.strip_prefix("OUTCOME actingd: "))
            .expect("the OUTCOME line"),
    )
    .expect("the envelope");
    assert_eq!(envelope["code"], "install_startup_stopped");
    assert_eq!(envelope["category"], "error");
    assert_eq!(envelope["values"]["operation"], "install_transition");
    let latched = envelope["causes"]
        .as_array()
        .and_then(|causes| {
            causes
                .iter()
                .find(|link| link["relation"] == "caused_by" && link["code"] == "ledger_failure")
        })
        .expect("the latched cause as a caused_by link");
    assert_eq!(latched["values"]["operation"], "append_runtime_event");
    assert_eq!(lines.next(), None);
}
