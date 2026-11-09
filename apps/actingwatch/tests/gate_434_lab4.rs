// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-4 (register §1.287, "Slice LAB-4"; contracts/lab-two-entries.md §7, §8.1;
// contracts/runtime-watchdog.md, "Launcher"): `actingwatch_tick_in_process`. One tick runs in
// actingwatch.exe's own process, with the decisions the crate's tick makes: the program, run as
// the task runs it (no argument, in the root), ticks a root that holds no actingctl.exe, so no
// child could have ticked it, and it decides, records and logs what `crates/watchdog`'s task tick
// decides, records and logs on a twin root.
//
// The roots are single-location roots whose state root holds no owner journal: row 3,
// `watchdog_never_started`, decided before any probe of the machine.

#![cfg(windows)]

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

#[test]
fn actingwatch_tick_in_process() {
    let temp = TempDir::new().expect("tempdir");
    let root = single_root(&temp, "program");
    let twin = single_root(&temp, "crate");
    let program = root.join("tools").join("actingwatch.exe");
    fs::copy(env!("CARGO_BIN_EXE_actingwatch"), &program).expect("place actingwatch.exe");

    // The task's action: the program without arguments, in the root.
    let status = Command::new(&program)
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run actingwatch.exe");
    // The crate's task tick on the twin root.
    let decided =
        actingcommand_watchdog::tick_for_test(&actingcommand_watchdog::start::system(), &twin);

    assert_eq!(decided.code().as_str(), "watchdog_never_started");
    assert_eq!(
        status.code(),
        Some(0),
        "the exit of a never-started root's tick"
    );
    assert!(
        !root.join("runtime").join("actingctl.exe").exists(),
        "the root holds no fixed entry that could have run the tick"
    );
    let (state, twin_state) = (state(&root), state(&twin));
    assert_eq!(
        state["last_decision"],
        json!({ "code": "watchdog_never_started" }),
        "{state}"
    );
    assert_eq!(state["last_decision"], twin_state["last_decision"]);
    assert_eq!(state["starts"], twin_state["starts"]);
    // A task tick records its end, which opens the gap grace of the next one.
    assert!(state["last_task_tick_end_unix_ms"].is_u64(), "{state}");
    assert!(
        twin_state["last_task_tick_end_unix_ms"].is_u64(),
        "{twin_state}"
    );
    let (log, twin_log) = (decision_lines(&root), decision_lines(&twin));
    assert_eq!(log, ["INFO watchdog_never_started"]);
    assert_eq!(log, twin_log);
}

/// `<temp>\<name>`: `runtime\BUILD-MANIFEST.json`, `actingd.config.json` naming the absolute
/// state root `<temp>\<name>-state`, which holds no owner journal, and empty `tools\` and
/// `watchdog\` folders; no `runtime\actingctl.exe` and no `install\`.
fn single_root(temp: &TempDir, name: &str) -> PathBuf {
    let root = temp.path().join(name);
    let state = temp.path().join(format!("{name}-state"));
    for directory in [
        root.join("runtime"),
        root.join("tools"),
        root.join("watchdog"),
        state.clone(),
    ] {
        fs::create_dir_all(directory).expect("create a fixture directory");
    }
    fs::write(
        root.join("runtime").join("BUILD-MANIFEST.json"),
        br#"{"fixture":"actingwatch-tick"}"#,
    )
    .expect("write the build manifest");
    let config = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": state,
        "instances": []
    }))
    .expect("the config JSON");
    fs::write(root.join("actingd.config.json"), config).expect("write the config");
    root
}

/// `<root>\watchdog\state.json`.
fn state(root: &Path) -> Value {
    let bytes = fs::read(root.join("watchdog").join("state.json")).expect("state.json");
    serde_json::from_slice(&bytes).expect("state.json is JSON")
}

/// The lines of `<root>\watchdog\watchdog.log` without their time: `<LEVEL> <code> …`.
fn decision_lines(root: &Path) -> Vec<String> {
    fs::read_to_string(root.join("watchdog").join("watchdog.log"))
        .expect("watchdog.log")
        .lines()
        .map(|line| line.splitn(3, ' ').nth(2).unwrap_or(line).to_owned())
        .collect()
}
