// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 CLI-COMP (register §1.318; contracts/lab-two-entries.md §9.3, "Completion"; the
// coordinator's rulings of 2026-11-11, not Alice's words): `actinglab __complete`, the callback
// the completion scripts call, run as a process the way they run it, prints its candidates on
// stdout only and exits 0: with a scripted Runtime that answers the read that resolves an
// instance (its aliases), with one that does not answer within the wait, with none at all, and
// for lines it cannot read (nothing). The candidates themselves are the bin's gate
// `gate_434_cli_comp`. Fakes only: the runtime client's scripted Runtime on loopback in a
// temporary state root, neutral aliases; no device, no sleep.

use actingcommand_contract::{
    IdentifierIssuer, RuntimeControlPlaneStatus, RuntimeInstanceStatus, RuntimeOperation,
    RuntimeResult,
};
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedRuntime,
};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

/// The callback's line `args` (the words after `__complete`), its Runtime found through the state
/// root `state_root`, as the CLI module's rows find it.
fn callback(state_root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .arg("__complete")
        .args(args)
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", state_root)
        .env_remove("ACTINGCOMMAND_INITIATOR")
        .output()
        .expect("the actinglab child process")
}

/// The process exited 0, printed `stdout` and wrote nothing on stderr.
fn assert_quiet(output: &Output, stdout: &str, args: &[&str]) {
    assert_eq!(output.status.code(), Some(0), "{args:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), stdout, "{args:?}");
    assert!(
        output.stderr.is_empty(),
        "{args:?} wrote on stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn completion_callback_prints_on_stdout_only_and_exits_zero() {
    let temp = TempDir::new().expect("a temporary folder");
    let state = temp.path().join("state");
    fs::create_dir_all(&state).expect("the state root");
    let runtime = ScriptedRuntime::start(&state, |owner_epoch| {
        vec![
            ScriptedConnection::new().request(move |request| {
                assert!(matches!(request.operation(), RuntimeOperation::Status));
                let issuer = IdentifierIssuer::new().expect("identifier issuer");
                let instances = ["neutral.a", "neutral.b"]
                    .into_iter()
                    .map(|alias| {
                        let instance_id = *issuer
                            .mint_instance_id()
                            .expect("an instance id")
                            .transport();
                        RuntimeInstanceStatus::new(
                            alias,
                            instance_id,
                            false,
                            0,
                            false,
                            false,
                            false,
                        )
                        .expect("an instance's status")
                    })
                    .collect();
                let status = RuntimeControlPlaneStatus::new(owner_epoch, instances)
                    .expect("the scripted status");
                ScriptedAnswer::completed(request, RuntimeResult::Status { status })
            }),
            ScriptedConnection::new().request(|_| ScriptedAnswer::StallUntilClosed),
        ]
    });

    // A Runtime that answers: its aliases, one per line.
    let answered = [
        "--wait-ms",
        "60000",
        "2",
        ":actinglab",
        ":pause",
        ":neutral.",
    ];
    assert_quiet(
        &callback(&state, &answered),
        "neutral.a\nneutral.b\n",
        &answered,
    );
    // One that does not answer within the wait: nothing (this program runs from no installation,
    // so there is no configuration to read either), and no error.
    let stalled = ["--wait-ms", "2000", "2", ":actinglab", ":pause", ":"];
    assert_quiet(&callback(&state, &stalled), "", &stalled);
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "one connection per read");
    for connection in &record.connections {
        assert_eq!(
            connection.count(|operation| matches!(operation, RuntimeOperation::Status)),
            1,
            "one status read after the handshake"
        );
    }

    // No Runtime at all, and lines the callback cannot read: nothing, and no error.
    let nowhere = temp.path().join("nowhere");
    fs::create_dir_all(&nowhere).expect("a state root without a Runtime");
    let lines: [&[&str]; 6] = [
        &["2", ":actinglab", ":pause", ":"],
        &["9", ":actinglab"],
        &["2", ":actinglab", ":frobnicate", ":"],
        &[],
        &["--frobnicate"],
        &["1", ":actinglab", ":%FF"],
    ];
    for args in lines {
        assert_quiet(&callback(&nowhere, args), "", args);
    }
}
