// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5G5a, the second generation's process test, copied from the first's
//! (Workflow #421 MCP-ENTRY): the real entry binary with an in-tree fake
//! (`actingcommand-mcp-entry-g2-fake-child`, copied to `<root>\tools\actinglab-mcp.exe` and
//! `<root>\tools\actinglab.exe` of a temporary root) says its generation, pipes a session both
//! ways, gives the server the words after `--` and not `--client`, writes a status file of
//! generation 2, and switches once when the install's manifest names another build: the client
//! gets `notifications/tools/list_changed` and the next `tools/list` comes from the new child.
//!
//! #398 DB-3 (register §1.475): the entry, run with its output through a pipe, starts its server,
//! which starts a long-lived grandchild; the reader of the entry's output sees the pipe's end when
//! the entry ends, while the grandchild still runs.

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
fn mcp_entry_g2_process_pipes_a_session_and_switches_once() {
    let generation = Command::new(env!("CARGO_BIN_EXE_actingcommand-mcp-entry-g2"))
        .arg("--generation")
        .output()
        .expect("--generation runs");
    assert!(generation.status.success());
    assert_eq!(String::from_utf8_lossy(&generation.stdout).trim(), "2");

    let root = tempfile::tempdir().expect("temporary root");
    let runtime = root.path().join("runtime");
    let tools = root.path().join("tools");
    fs::create_dir_all(&runtime).expect("runtime directory");
    fs::create_dir_all(&tools).expect("tools directory");
    let fake = env!("CARGO_BIN_EXE_actingcommand-mcp-entry-g2-fake-child");
    fs::copy(fake, tools.join("actinglab-mcp.exe")).expect("the fake as actinglab-mcp.exe");
    fs::copy(fake, tools.join("actinglab.exe")).expect("the fake as actinglab.exe");
    write_manifest(&runtime, OLD, "first");

    let mut entry = Command::new(env!("CARGO_BIN_EXE_actingcommand-mcp-entry-g2"))
        .arg("--root")
        .arg(root.path())
        .args(["--client", "claude", "--", "--list", "core"])
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

    // The server got the words after `--` and nothing of the entry's own.
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "fake_11111111", "arguments": {}}}),
    );
    let call = next(&lines, |message| message["id"] == 3);
    let served = call["result"]["content"][0]["text"]
        .as_str()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .expect("the fake server's own answer");
    assert_eq!(served["args"], json!(["--list", "core"]));

    let before = wait_status(root.path(), pid, |status| status["child"]["build"] == OLD);
    assert_eq!(before["entry_generation"], 2);
    assert_eq!(before["entry_protocol"], 2);
    assert_eq!(before["client"]["configured"], "claude");
    let first_child = before["child"]["pid"].clone();
    assert_eq!(served["pid"], first_child);

    write_manifest(&runtime, NEW, "second, a longer note");
    next(&lines, |message| {
        message["method"] == "notifications/tools/list_changed"
    });
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 4, "method": "tools/list"}),
    );
    let second = next(&lines, |message| message["id"] == 4);
    assert_eq!(second["result"]["tools"][0]["name"], "fake_22222222");
    let after = wait_status(root.path(), pid, |status| {
        status["notice"]["tools_list_after"] == true
    });
    assert_eq!(after["child"]["build"], NEW);
    assert_ne!(after["child"]["pid"], first_child);
    assert_eq!(after["previous_children"][0]["pid"], first_child);
    assert_eq!(after["last_switch"]["to_build"], NEW);
    assert_eq!(after["entry_generation"], 2);

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

/// #398 DB-3 (register §1.475): the entry, run as a client runs it with its stdin and stdout
/// through pipes, starts its server (the fake as `actinglab-mcp.exe`), and the server, on a call,
/// starts a grandchild that lingers until the test releases it, its standard handles null and
/// every inheritable handle of the server its own. Once the client closed its input and the entry
/// ended, the reader of the entry's stdout sees that pipe's end, and the grandchild still runs.
/// Before DB-3 the entry's stdout was inheritable: the server, then the grandchild, received a
/// copy, and the reader saw the end only when the grandchild ended (after its 120 s at most,
/// past the wait here). No clock is read; the waits are the channel's, bounded by [`WAIT`].
#[cfg(windows)]
#[test]
fn db3_the_entrys_output_ends_with_the_entry_while_a_grandchild_lives() {
    use actingcommand_host_metrics::{ProcessProbe, probe_process};
    use std::sync::mpsc::RecvTimeoutError;

    let root = tempfile::tempdir().expect("temporary root");
    let runtime = root.path().join("runtime");
    let tools = root.path().join("tools");
    fs::create_dir_all(&runtime).expect("runtime directory");
    fs::create_dir_all(&tools).expect("tools directory");
    let fake = env!("CARGO_BIN_EXE_actingcommand-mcp-entry-g2-fake-child");
    fs::copy(fake, tools.join("actinglab-mcp.exe")).expect("the fake as actinglab-mcp.exe");
    fs::copy(fake, tools.join("actinglab.exe")).expect("the fake as actinglab.exe");
    write_manifest(&runtime, OLD, "first");
    // The grandchild runs the fake from where cargo built it, so that nothing of it is left in
    // the temporary root; it ends once this file exists or the root is gone.
    let release = root.path().join("release");

    let mut entry = Command::new(env!("CARGO_BIN_EXE_actingcommand-mcp-entry-g2"))
        .arg("--root")
        .arg(root.path())
        .arg("--")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("the entry starts");
    let mut stdin = entry.stdin.take().expect("stdin");
    let stdout = entry.stdout.take().expect("stdout");
    // The reader of the entry's stdout: each message, until the pipe's end, where it drops the
    // sender.
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
    let answer = |id: u64| loop {
        let message = lines
            .recv_timeout(WAIT)
            .expect("the entry answers within the wait");
        if message["id"] == id {
            break message;
        }
    };

    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "process-test", "version": "1"}}}),
    );
    answer(1);
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    );
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "fake_11111111", "arguments": {
                "linger": {"program": fake, "release": release}}}}),
    );
    let call = answer(2);
    let served = call["result"]["content"][0]["text"]
        .as_str()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .expect("the fake server's own answer");
    let lingerer = served["lingerer"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .unwrap_or_else(|| panic!("the server started the grandchild: {served}"));

    // The client closes its input; the entry ends its server and itself.
    drop(stdin);
    let status = entry.wait().expect("the entry ends");
    assert!(status.success(), "the entry ends cleanly: {status}");
    // The end of the entry's stdout reaches its reader now, not when the grandchild ends.
    loop {
        match lines.recv_timeout(WAIT) {
            Ok(_) => {}
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                panic!("the entry ended but its stdout did not: a descendant still holds the pipe")
            }
        }
    }
    assert!(
        matches!(probe_process(lingerer), ProcessProbe::Running { .. }),
        "the grandchild {lingerer} still runs when the entry's output has ended"
    );
    fs::write(&release, b"released").expect("release the grandchild");
}
