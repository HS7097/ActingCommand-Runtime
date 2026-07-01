// SPDX-License-Identifier: AGPL-3.0-only

use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const SESSION_CRASH_INJECTION_ENV: &str = "ACTINGLAB_TEST_SESSION_CRASH_POINT";

fn actinglab_binary() -> &'static str {
    env!("CARGO_BIN_EXE_actinglab")
}

fn run_json(args: &[&str]) -> Value {
    let output = Command::new(actinglab_binary())
        .args(args)
        .output()
        .expect("actinglab command should run");
    assert!(
        output.status.success(),
        "actinglab failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("actinglab stdout should be JSON")
}

fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn wait_for<F>(timeout: Duration, mut predicate: F) -> bool
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

fn request_path(state_dir: &Path, request_id: &str) -> std::path::PathBuf {
    state_dir
        .join("requests")
        .join(format!("{request_id}.json"))
}

fn running_path(state_dir: &Path, request_id: &str) -> std::path::PathBuf {
    state_dir.join("running").join(format!("{request_id}.json"))
}

fn response_path(state_dir: &Path, request_id: &str) -> std::path::PathBuf {
    state_dir
        .join("responses")
        .join(format!("{request_id}.json"))
}

fn journal_path(state_dir: &Path) -> std::path::PathBuf {
    state_dir.join("request-journal.jsonl")
}

fn write_status_request(state_dir: &Path, request_id: &str) {
    fs::create_dir_all(state_dir.join("requests")).unwrap();
    let request = serde_json::json!({
        "request_id": request_id,
        "command": "queue",
        "global": {
            "instance": null,
            "game": null,
            "server": null,
            "resource_root": null,
            "capture_backend": null,
            "dry_run": true
        },
        "args": [],
        "created_at_unix_ms": 1
    });
    fs::write(
        request_path(state_dir, request_id),
        serde_json::to_vec_pretty(&request).unwrap(),
    )
    .unwrap();
}

fn spawn_daemon(state_dir: &Path, crash_point: Option<&str>) -> Child {
    let mut command = Command::new(actinglab_binary());
    command
        .args([
            "--json",
            "session",
            "daemon",
            "--state-dir",
            state_dir.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(point) = crash_point {
        command.env(SESSION_CRASH_INJECTION_ENV, point);
    }
    command.spawn().expect("session daemon should start")
}

fn wait_for_child_exit(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        thread::sleep(Duration::from_millis(50));
    }
    stop_child(child);
    panic!("session daemon did not exit after crash injection");
}

fn journal_line_count(state_dir: &Path, request_id: &str) -> usize {
    let Ok(text) = fs::read_to_string(journal_path(state_dir)) else {
        return 0;
    };
    text.lines()
        .filter(|line| line.contains(&format!("\"request_id\":\"{request_id}\"")))
        .count()
}

fn readiness_data(response: &Value) -> &Value {
    response
        .pointer("/data/response")
        .or_else(|| response.get("data"))
        .expect("readiness data")
}

#[test]
fn session_daemon_crash_points_recover_without_duplicate_execution() {
    for crash_point in [
        "after_response_write",
        "after_journal_append",
        "after_request_remove",
    ] {
        let temp = TempDir::new().unwrap();
        let state_dir = temp.path();
        let request_id = format!("crash-{crash_point}");
        write_status_request(state_dir, &request_id);

        let mut crashing = spawn_daemon(state_dir, Some(crash_point));
        let status = wait_for_child_exit(&mut crashing);
        assert!(
            !status.success(),
            "crash injection {crash_point} should terminate the daemon"
        );
        assert!(
            response_path(state_dir, &request_id).exists(),
            "response must survive crash point {crash_point}"
        );

        let mut recovering = spawn_daemon(state_dir, None);
        let recovered = wait_for(Duration::from_secs(5), || {
            !request_path(state_dir, &request_id).exists()
                && !running_path(state_dir, &request_id).exists()
                && response_path(state_dir, &request_id).exists()
                && journal_line_count(state_dir, &request_id) == 1
        });
        stop_child(&mut recovering);

        assert!(
            recovered,
            "daemon restart must recover request state after crash point {crash_point}"
        );
    }
}

#[test]
fn session_daemon_non_graceful_death_makes_readiness_not_ready() {
    let temp = TempDir::new().unwrap();
    let state_dir = temp.path().to_str().unwrap();
    let mut child = Command::new(actinglab_binary())
        .args(["--json", "session", "daemon", "--state-dir", state_dir])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("session daemon should start");

    let ready = (0..50).any(|_| {
        let response = run_json(&["--json", "session", "readiness", "--state-dir", state_dir]);
        if readiness_data(&response)
            .get("ready")
            .and_then(Value::as_bool)
            == Some(true)
        {
            true
        } else {
            thread::sleep(Duration::from_millis(100));
            false
        }
    });
    if !ready {
        stop_child(&mut child);
    }
    assert!(ready, "session daemon should become ready before kill");

    stop_child(&mut child);

    let not_ready = (0..50).any(|_| {
        let response = run_json(&["--json", "session", "readiness", "--state-dir", state_dir]);
        let data = readiness_data(&response);
        let ready = data.get("ready").and_then(Value::as_bool) == Some(true);
        let status = data.pointer("/daemon/status").and_then(Value::as_str);
        if !ready && status != Some("alive") {
            true
        } else {
            thread::sleep(Duration::from_millis(100));
            false
        }
    });
    assert!(
        not_ready,
        "readiness must reject residual daemon state after non-graceful death"
    );
}

#[test]
fn session_daemon_no_wait_request_returns_after_ack() {
    let temp = TempDir::new().unwrap();
    let state_dir = temp.path();
    let state_dir_text = state_dir.to_str().unwrap();
    let mut child = spawn_daemon(state_dir, None);

    let ready = wait_for(Duration::from_secs(5), || {
        let response = run_json(&[
            "--json",
            "session",
            "readiness",
            "--state-dir",
            state_dir_text,
        ]);
        readiness_data(&response)
            .get("ready")
            .and_then(Value::as_bool)
            == Some(true)
    });
    if !ready {
        stop_child(&mut child);
    }
    assert!(ready, "session daemon should become ready before request");

    let response = run_json(&[
        "--json",
        "session",
        "request",
        "status",
        "--no-wait",
        "--request-ack-timeout-ms",
        "2000",
        "--state-dir",
        state_dir_text,
    ]);
    let data = response.get("data").expect("request data");
    assert_eq!(data.get("status").and_then(Value::as_str), Some("queued"));
    assert_eq!(
        data.get("waited_for_response").and_then(Value::as_bool),
        Some(false)
    );
    let ack_status = data
        .pointer("/acknowledgement/status")
        .and_then(Value::as_str)
        .expect("ack status");
    assert!(
        ack_status == "running" || ack_status == "response_available",
        "unexpected ack status {ack_status}"
    );
    let request_id = data
        .get("request_id")
        .and_then(Value::as_str)
        .expect("request id");
    let completed = wait_for(Duration::from_secs(5), || {
        !request_path(state_dir, request_id).exists()
            && !running_path(state_dir, request_id).exists()
            && response_path(state_dir, request_id).exists()
    });
    stop_child(&mut child);

    assert!(completed, "daemon should finish acked no-wait request");
}
