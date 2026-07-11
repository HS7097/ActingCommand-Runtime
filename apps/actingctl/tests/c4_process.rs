// SPDX-License-Identifier: AGPL-3.0-only

#[path = "../../../tests/support/c4_runtime.rs"]
mod support;

use serde_json::Value;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn c4_runtime_child_process() {
    support::run_child_if_requested();
}

#[test]
fn actingctl_observe_and_reset_leave_runtime_alive_and_share_projection_shape() {
    let root = TempDir::new().expect("tempdir");
    let frame = root.path().join("sealed.png");
    support::write_sealed_frame(&frame);
    let mut runtime = support::RuntimeChild::spawn(root.path(), "c4_runtime_child_process");
    runtime.wait_ready(root.path());

    let observe = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args([
            "observe",
            "--state-root",
            root.path().to_str().expect("state root"),
            "--instance",
            "ak.cn",
        ])
        .output()
        .expect("run actingctl observe");
    assert!(
        observe.status.success(),
        "actingctl observe failed: {}",
        String::from_utf8_lossy(&observe.stderr)
    );
    let observe_json: Value = serde_json::from_slice(&observe.stdout).expect("observe JSON");
    assert_eq!(
        observe_json["receipt"]["result"]["kind"],
        "readonly_observation_completed"
    );
    assert!(observe_json["events"].as_array().is_some_and(|events| {
        events
            .iter()
            .any(|event| event["event_type"] == "recognition.completed")
    }));
    assert_eq!(
        support::backend_events(root.path()),
        ["capture_open", "capture"]
    );
    runtime.assert_alive();

    let reset = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args([
            "reset",
            "--state-root",
            root.path().to_str().expect("state root"),
            "--instance",
            "ak.cn",
        ])
        .output()
        .expect("run actingctl reset");
    assert!(
        reset.status.success(),
        "actingctl reset failed: {}",
        String::from_utf8_lossy(&reset.stderr)
    );
    let reset_json: Value = serde_json::from_slice(&reset.stdout).expect("reset JSON");
    assert_eq!(
        reset_json["receipt"]["result"]["kind"],
        "safe_reset_completed"
    );
    assert!(reset_json["events"].is_array());
    assert_eq!(
        support::backend_events(root.path()),
        ["capture_open", "capture", "open", "reset"]
    );
    runtime.assert_alive();
    runtime.stop_clean();
    assert_eq!(
        support::backend_events(root.path()),
        [
            "capture_open",
            "capture",
            "open",
            "reset",
            "capture_close",
            "close"
        ]
    );
}

#[test]
fn actingctl_status_monitor_and_stream_are_runtime_backed() {
    let root = TempDir::new().expect("tempdir");
    let frame = root.path().join("sealed.png");
    support::write_sealed_frame(&frame);
    let mut runtime = support::RuntimeChild::spawn(root.path(), "c4_runtime_child_process");
    runtime.wait_ready(root.path());
    let binary = env!("CARGO_BIN_EXE_actingctl");
    let state_root = root.path().to_str().expect("state root");

    let status = run_json(binary, ["status", "--state-root", state_root]);
    assert_eq!(status["instances"][0]["instance_alias"], "ak.cn");

    let monitor_status = run_json(binary, ["monitor-status", "--state-root", state_root]);
    assert_eq!(monitor_status["instances"][0]["instance_alias"], "ak.cn");
    assert!(monitor_status["instances"][0].get("policy").is_none());

    let configured = run_json(
        binary,
        [
            "monitor-set",
            "--state-root",
            state_root,
            "--instance",
            "ak.cn",
            "--interval-ms",
            "60000",
            "--expect",
            "home",
        ],
    );
    assert_eq!(configured["instance_alias"], "ak.cn");
    assert_eq!(configured["policy"]["expected_page"], "home");

    let cleared = run_json(
        binary,
        [
            "monitor-clear",
            "--state-root",
            state_root,
            "--instance",
            "ak.cn",
        ],
    );
    assert_eq!(cleared["instance_alias"], "ak.cn");
    assert!(cleared.get("policy").is_none());

    let stream = run_json(
        binary,
        [
            "stream",
            "--state-root",
            state_root,
            "--instance",
            "ak.cn",
            "--max-frames",
            "2",
            "--interval-ms",
            "1",
        ],
    );
    assert_eq!(
        stream["receipt"]["result"]["kind"],
        "capture_sequence_completed"
    );
    assert_eq!(
        stream["receipt"]["result"]["sequence"]["observations"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    runtime.assert_alive();
    runtime.stop_clean();
}

fn run_json<const N: usize>(binary: &str, arguments: [&str; N]) -> Value {
    let output = Command::new(binary)
        .args(arguments)
        .output()
        .expect("run actingctl");
    assert!(
        output.status.success(),
        "actingctl failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("actingctl JSON")
}
