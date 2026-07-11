// SPDX-License-Identifier: AGPL-3.0-only

#[path = "../../../tests/support/c4_runtime.rs"]
mod support;

use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn actinglab_binary() -> &'static str {
    env!("CARGO_BIN_EXE_actinglab")
}

fn run_actinglab(
    config_path: &Path,
    runtime_root: &Path,
    local_app_data: &Path,
    legacy_state: &Path,
    args: &[&str],
) -> Output {
    Command::new(actinglab_binary())
        .args(args)
        .env("ACTINGLAB_CONFIG_PATH", config_path)
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", runtime_root)
        .env("ACTINGLAB_SESSION_STATE_DIR", legacy_state)
        .env("LOCALAPPDATA", local_app_data)
        .output()
        .expect("run ActingLab")
}

fn json_output(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "ActingLab did not return JSON: {error}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_retired(output: Output) {
    assert_eq!(output.status.code(), Some(6));
    let envelope = json_output(&output);
    assert_eq!(envelope["ok"], false);
    assert_eq!(
        envelope["error"]["code"],
        "legacy_session_authority_retired"
    );
    assert!(envelope["data"].is_null());
}

#[test]
fn session_closeout_runtime_child_process() {
    support::run_child_if_requested();
}

#[test]
fn retired_session_commands_and_selectors_create_no_legacy_state() {
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("actinglab.json");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let legacy_state = root.path().join("legacy-session");
    fs::write(&config_path, "{}").expect("write config");

    let retired_commands: &[&[&str]] = &[
        &["--json", "session", "daemon"],
        &["--json", "session", "queue"],
        &["--json", "session", "request", "status"],
        &["--json", "session", "journal"],
        &["--json", "session", "events"],
        &["--json", "session", "response"],
        &["--json", "session", "request-state"],
        &["--json", "session", "lease"],
        &["--json", "monitor"],
    ];
    for command in retired_commands {
        assert_retired(run_actinglab(
            &config_path,
            &runtime_root,
            &local_app_data,
            &legacy_state,
            command,
        ));
    }

    let retired_selectors: &[&[&str]] = &[
        &["--json", "session", "status", "--via-daemon"],
        &["--json", "session", "monitor-policy", "status", "--local"],
        &[
            "--json",
            "session",
            "stream",
            "--state-dir",
            legacy_state.to_str().expect("legacy state path"),
        ],
    ];
    for command in retired_selectors {
        assert_retired(run_actinglab(
            &config_path,
            &runtime_root,
            &local_app_data,
            &legacy_state,
            command,
        ));
    }

    assert!(!legacy_state.exists());
    assert!(
        !local_app_data
            .join("ActingCommand/actinglab/session")
            .exists()
    );
}

#[test]
fn runtime_clients_reconnect_without_restoring_session_file_authority() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().to_path_buf();
    let local_app_data = root.path().join("local-app-data");
    let legacy_state = root.path().join("legacy-session");
    let config_path = root.path().join("actinglab.json");
    let frame = root.path().join("sealed.png");
    fs::write(&config_path, "{}").expect("write config");
    support::write_sealed_frame(&frame);
    let mut runtime =
        support::RuntimeChild::spawn(root.path(), "session_closeout_runtime_child_process");
    runtime.wait_ready(root.path());

    let first = run_actinglab(
        &config_path,
        &runtime_root,
        &local_app_data,
        &legacy_state,
        &["--json", "session", "status"],
    );
    assert!(
        first.status.success(),
        "first status failed: stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let first = json_output(&first);
    assert_eq!(first["data"]["running"], true);
    let runtime_pid = first["data"]["info"]["pid"].clone();

    let stream = run_actinglab(
        &config_path,
        &runtime_root,
        &local_app_data,
        &legacy_state,
        &[
            "--json",
            "--instance",
            "ak.cn",
            "session",
            "stream",
            "--max-frames",
            "2",
        ],
    );
    assert!(stream.status.success());
    let stream = json_output(&stream);
    assert_eq!(stream["data"]["frames"].as_array().map(Vec::len), Some(2));
    runtime.assert_alive();

    let reconnected = run_actinglab(
        &config_path,
        &runtime_root,
        &local_app_data,
        &legacy_state,
        &["--json", "session", "status"],
    );
    assert!(reconnected.status.success());
    let reconnected = json_output(&reconnected);
    assert_eq!(reconnected["data"]["info"]["pid"], runtime_pid);
    assert!(!legacy_state.exists());

    runtime.stop_clean();
    assert_eq!(
        support::backend_events(root.path()),
        ["capture_open", "capture", "capture", "capture_close"]
    );
    let unavailable = run_actinglab(
        &config_path,
        &runtime_root,
        &local_app_data,
        &legacy_state,
        &["--json", "session", "status"],
    );
    assert_eq!(unavailable.status.code(), Some(5));
    let unavailable = json_output(&unavailable);
    assert_eq!(unavailable["error"]["code"], "runtime_not_running");
    assert!(unavailable["data"].is_null());
    assert!(!legacy_state.exists());
}
