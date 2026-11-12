// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #421 MCP-ENTRY, the process test: the real entry binary with an in-tree fake child
//! (`actingcommand-mcp-entry-fake-child`, copied to `<root>\runtime\actingctl.exe` of a temporary
//! root) pipes a session both ways and switches once when the install's manifest names another
//! build: the client gets `notifications/tools/list_changed` and the next `tools/list` comes from
//! the new child.

use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const OLD: &str = "1111111111111111111111111111111111111111";
const NEW: &str = "2222222222222222222222222222222222222222";
const WAIT: Duration = Duration::from_secs(60);

fn write_manifest(runtime: &Path, commit: &str, note: &str) {
    let manifest = json!({"commit_sha": commit, "note": note});
    fs::write(runtime.join("BUILD-MANIFEST.json"), manifest.to_string()).expect("manifest");
}

fn send(stdin: &mut ChildStdin, message: &Value) {
    writeln!(stdin, "{message}").expect("write to the entry");
    stdin.flush().expect("flush");
}

/// The next message from the entry that `wanted` accepts; others are skipped.
fn next(lines: &Receiver<Value>, wanted: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let message = lines
            .recv_timeout(left)
            .expect("the entry answers within the wait");
        if wanted(&message) {
            return message;
        }
    }
}

fn status_of(root: &Path, pid: u32) -> Value {
    let path = root
        .join("mcp-entry")
        .join("state")
        .join(format!("entry-{pid}.json"));
    fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

/// Waits until the status file says `ready`.
fn wait_status(root: &Path, pid: u32, ready: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let status = status_of(root, pid);
        if ready(&status) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "the status never got there: {status}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn mcp_entry_process_pipes_a_session_and_switches_once() {
    let root = tempfile::tempdir().expect("temporary root");
    let runtime = root.path().join("runtime");
    fs::create_dir_all(&runtime).expect("runtime directory");
    fs::copy(
        env!("CARGO_BIN_EXE_actingcommand-mcp-entry-fake-child"),
        runtime.join("actingctl.exe"),
    )
    .expect("the fake child as actingctl.exe");
    write_manifest(&runtime, OLD, "first");

    let mut entry = Command::new(env!("CARGO_BIN_EXE_actingcommand-mcp-entry"))
        .arg("--root")
        .arg(root.path())
        .args([
            "--client",
            "claude",
            "--",
            "mcp-serve",
            "--tier",
            "observer",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("the entry starts");
    let pid = entry.id();
    let mut stdin = entry.stdin.take().expect("stdin");
    let stdout = entry.stdout.take().expect("stdout");
    let (sender, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                return;
            };
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if sender.send(message).is_err() {
                return;
            }
        }
    });

    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "process-test", "version": "1"}}}),
    );
    let init = next(&lines, |message| message["id"] == 1);
    assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], true);
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    );
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    );
    let first = next(&lines, |message| message["id"] == 2);
    assert_eq!(first["result"]["tools"][0]["name"], "fake_11111111");
    let before = wait_status(root.path(), pid, |status| status["child"]["build"] == OLD);
    let first_child = before["child"]["pid"].clone();

    write_manifest(&runtime, NEW, "second, a longer note");
    next(&lines, |message| {
        message["method"] == "notifications/tools/list_changed"
    });
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}),
    );
    let second = next(&lines, |message| message["id"] == 3);
    assert_eq!(second["result"]["tools"][0]["name"], "fake_22222222");
    let after = wait_status(root.path(), pid, |status| {
        status["notice"]["tools_list_after"] == true
    });
    assert_eq!(after["child"]["build"], NEW);
    assert_ne!(after["child"]["pid"], first_child);
    assert_eq!(after["previous_children"][0]["pid"], first_child);
    assert_eq!(after["last_switch"]["to_build"], NEW);

    drop(stdin);
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(status) = entry.try_wait().expect("the entry's state") {
            assert!(status.success(), "the entry ends cleanly: {status}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the entry ends after stdin closes"
        );
        thread::sleep(Duration::from_millis(20));
    }
}
