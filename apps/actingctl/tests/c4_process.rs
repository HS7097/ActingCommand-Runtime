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
            "--sealed-test",
            "--sealed-frame",
            frame.to_str().expect("sealed frame"),
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
    assert!(support::backend_events(root.path()).is_empty());
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
        ["open", "reset", "close"]
    );
    runtime.assert_alive();
    runtime.stop_clean();
}
