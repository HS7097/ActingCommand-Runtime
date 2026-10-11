// SPDX-License-Identifier: AGPL-3.0-only

//! Fixture I5 of the test plan (Workflow #382 MCP-T1, from board #391 item 1): the real
//! `actingctl mcp-serve` binary over pipes. A row sends raw lines, JSON messages or batch
//! arrays and reads stdout one message at a time; `finish` closes stdin, reads stdout to its
//! end and returns the exit status. Every read returns as soon as its message arrives;
//! `GATE_WAIT` only bounds a broken run.

use crate::held_runtime::GATE_WAIT;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;

/// The tool result of an accepted call: `isError` false and `structuredContent.ok` true.
#[track_caller]
pub fn accepted<'a>(answer: &'a Value, tool: &str) -> &'a Value {
    let result = &answer["result"];
    assert!(
        result["isError"] == json!(false) && result["structuredContent"]["ok"] == json!(true),
        "{tool} was not accepted: {answer}"
    );
    &result["structuredContent"]["result"]
}

/// `actingctl mcp-serve` on one state root over pipes. Dropped, it is killed if still running.
pub struct McpServer {
    child: Option<Child>,
    input: Option<ChildStdin>,
    lines: Receiver<Result<String, String>>,
}

impl McpServer {
    /// With the observer and operator tiers (#391 item 1).
    pub fn spawn(state_root: &Path) -> Self {
        Self::spawn_with_tier(state_root, Some("observer,operator"))
    }

    /// With `--tier <tier>` when one is given; without it, the default tier set.
    pub fn spawn_with_tier(state_root: &Path, tier: Option<&str>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_actingctl"));
        command.arg("mcp-serve").arg("--state-root").arg(state_root);
        if let Some(tier) = tier {
            command.args(["--tier", tier]);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn actingctl mcp-serve");
        let input = child.stdin.take().expect("mcp-serve stdin");
        let output = child.stdout.take().expect("mcp-serve stdout");
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let failed = line.is_err();
                if sender
                    .send(line.map_err(|error| error.to_string()))
                    .is_err()
                    || failed
                {
                    return;
                }
            }
        });
        Self {
            child: Some(child),
            input: Some(input),
            lines,
        }
    }

    /// Writes one line as it is, newline-terminated.
    pub fn send_line(&mut self, line: &str) {
        let input = self.input.as_mut().expect("mcp-serve stdin is open");
        writeln!(input, "{line}").expect("write a line to mcp-serve");
        input.flush().expect("flush mcp-serve stdin");
    }

    /// Writes one JSON message, or a batch array, as one line.
    pub fn send(&mut self, message: &Value) {
        self.send_line(&message.to_string());
    }

    /// The next message on stdout (an object, or a batch answer's array).
    pub fn recv(&mut self) -> Value {
        match self.next_message() {
            Some(message) => message,
            None => panic!("mcp-serve stdout ended before the next message"),
        }
    }

    /// The next message, or `None` once stdout ended.
    fn next_message(&mut self) -> Option<Value> {
        let line = match self.lines.recv_timeout(GATE_WAIT) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => panic!("cannot read mcp-serve stdout: {error}"),
            Err(RecvTimeoutError::Disconnected) => return None,
            Err(RecvTimeoutError::Timeout) => panic!("mcp-serve wrote nothing within GATE_WAIT"),
        };
        Some(
            serde_json::from_str::<Value>(&line).unwrap_or_else(|error| {
                panic!("mcp-serve wrote a line that is not JSON ({error}): {line}")
            }),
        )
    }

    /// Sends one request and returns its answer; notifications in between are skipped.
    pub fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let message = self.recv();
            match message.get("id") {
                None => {}
                Some(answered) if *answered == json!(id) => return message,
                Some(_) => panic!(
                    "mcp-serve answered another request while {method} (id {id}) was open: {message}"
                ),
            }
        }
    }

    /// A legacy `initialize` with protocol 2025-11-25, then `notifications/initialized`.
    pub fn initialize(&mut self) {
        let answer = self.initialize_version(1, "2025-11-25");
        assert!(answer.get("result").is_some(), "initialize: {answer}");
    }

    /// A legacy `initialize` asking for `version`, then `notifications/initialized`; returns
    /// the answer to the `initialize`.
    pub fn initialize_version(&mut self, id: u64, version: &str) -> Value {
        let answer = self.request(
            id,
            "initialize",
            json!({
                "protocolVersion": version,
                "capabilities": {},
                "clientInfo": {"name": "gate-test-client", "version": "0.0.0"},
            }),
        );
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        answer
    }

    pub fn call_tool(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        )
    }

    pub fn close_input(&mut self) {
        drop(self.input.take());
    }

    /// Closes stdin, reads stdout to its end and returns every message still unread, then the
    /// exit status of the process.
    pub fn finish(mut self) -> (Vec<Value>, ExitStatus) {
        self.close_input();
        let mut messages = Vec::new();
        while let Some(message) = self.next_message() {
            messages.push(message);
        }
        let mut child = self.child.take().expect("mcp-serve child");
        let (sender, exited) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(child.wait().map_err(|error| error.to_string()));
        });
        let status = match exited.recv_timeout(GATE_WAIT) {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => panic!("cannot wait for mcp-serve: {error}"),
            Err(error) => panic!("mcp-serve did not exit after its stdout ended: {error}"),
        };
        (messages, status)
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        drop(self.input.take());
        let Some(mut child) = self.child.take() else {
            return;
        };
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
    }
}
