// SPDX-License-Identifier: AGPL-3.0-only

//! Board #391 item 1: a registered instance alias with upper-case letters is an alias like any
//! other. Through `actingctl mcp-serve`, `ac_pause` and `ac_resume` on such an instance are
//! accepted, and the ledger holds the `client.action` record of each with that alias
//! byte-for-byte. The Runtime is the c4 fake child process (`tests/support/c4_runtime.rs`); the
//! MCP server is the real binary over pipes. Every wait returns as soon as its message arrives;
//! `GATE_WAIT` only bounds a broken run.

use crate::c4_runtime::{RuntimeChild, write_sealed_frame};
use crate::held_runtime::GATE_WAIT;
use actingcommand_contract::{
    ClientActionKind, ClientPayload, EventActor, EventPayload, EventQuery, EventSource, EventType,
    ProjectionPayload, ProjectionProfile,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use tempfile::TempDir;

/// The registered alias of the fake instance: neutral, with upper-case letters.
const ALIAS: &str = "Neutral.Instance";

#[test]
fn gate_mcp_pause_and_resume_record_the_client_action_of_an_upper_case_alias() {
    let root = TempDir::new().expect("tempdir");
    write_sealed_frame(&root.path().join("sealed.png"));
    let mut runtime =
        RuntimeChild::spawn_for_instance(root.path(), "c4_runtime_child_process", ALIAS);
    runtime.wait_ready(root.path());
    let mut mcp = McpServer::spawn(root.path());
    mcp.initialize();

    let pause = mcp.call_tool(2, "ac_pause", json!({"instance": ALIAS}));
    let paused = accepted(&pause, "ac_pause");
    assert_eq!(
        paused["paused"]["scope"],
        json!({"kind": "instance", "instance_alias": ALIAS}),
        "ac_pause answer: {pause}"
    );
    let revision = paused["paused"]["revision"]
        .as_u64()
        .filter(|revision| *revision >= 1)
        .unwrap_or_else(|| panic!("ac_pause answer carries no pause revision: {pause}"));
    let owner_epoch = paused["owner_epoch"]
        .as_str()
        .unwrap_or_else(|| panic!("ac_pause answer carries no owner_epoch: {pause}"))
        .to_owned();

    let resume = mcp.call_tool(
        3,
        "ac_resume",
        json!({
            "instance": ALIAS,
            "expected_owner_epoch": owner_epoch,
            "expected_revision": revision,
        }),
    );
    let resumed = accepted(&resume, "ac_resume");
    assert!(resumed["resumed"].is_object(), "ac_resume answer: {resume}");

    let ledger = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect a ledger client");
    let events = ledger
        .query_events(
            EventQuery {
                event_type: Some(EventType::ClientAction),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("query the client.action events");
    drop(ledger);
    let records = events
        .iter()
        .filter_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Client(ClientPayload::Action(action)) => Some(action.record()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    for control in ["ac_pause", "ac_resume"] {
        let recorded = records
            .iter()
            .filter(|record| record.surface_id() == "mcp" && record.control_id() == control)
            .collect::<Vec<_>>();
        assert_eq!(
            recorded.len(),
            1,
            "client.action records of {control}: {records:?}"
        );
        assert_eq!(
            recorded[0].kind(),
            ClientActionKind::Command,
            "client.action record of {control}: {records:?}"
        );
        assert_eq!(
            recorded[0].instance_alias(),
            Some(ALIAS),
            "client.action record of {control}: {records:?}"
        );
    }

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

/// The tool result of an accepted call: `isError` false and `structuredContent.ok` true.
#[track_caller]
fn accepted<'a>(answer: &'a Value, tool: &str) -> &'a Value {
    let result = &answer["result"];
    assert!(
        result["isError"] == json!(false) && result["structuredContent"]["ok"] == json!(true),
        "{tool} was not accepted: {answer}"
    );
    &result["structuredContent"]["result"]
}

/// `actingctl mcp-serve` on one state root over pipes. Dropped, it is killed if still running.
struct McpServer {
    child: Child,
    input: Option<ChildStdin>,
    lines: Receiver<Result<String, String>>,
}

impl McpServer {
    fn spawn(state_root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_actingctl"))
            .arg("mcp-serve")
            .arg("--state-root")
            .arg(state_root)
            .args(["--tier", "observer,operator"])
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
            child,
            input: Some(input),
            lines,
        }
    }

    fn send(&mut self, message: &Value) {
        let input = self.input.as_mut().expect("mcp-serve stdin is open");
        writeln!(input, "{message}").expect("write a message to mcp-serve");
        input.flush().expect("flush mcp-serve stdin");
    }

    /// Sends one request and returns its answer; notifications in between are skipped.
    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = match self.lines.recv_timeout(GATE_WAIT) {
                Ok(Ok(line)) => line,
                Ok(Err(error)) => panic!("cannot read mcp-serve stdout: {error}"),
                Err(error) => panic!("mcp-serve gave no answer to {method} (id {id}): {error}"),
            };
            let message = serde_json::from_str::<Value>(&line).unwrap_or_else(|error| {
                panic!("mcp-serve wrote a line that is not JSON ({error}): {line}")
            });
            match message.get("id") {
                None => {}
                Some(answered) if *answered == json!(id) => return message,
                Some(_) => panic!(
                    "mcp-serve answered another request while {method} (id {id}) was open: {message}"
                ),
            }
        }
    }

    fn initialize(&mut self) {
        let answer = self.request(
            1,
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "gate-test-client", "version": "0.0.0"},
            }),
        );
        assert!(answer.get("result").is_some(), "initialize: {answer}");
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    }

    fn call_tool(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        )
    }

    fn close_input(&mut self) {
        drop(self.input.take());
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        drop(self.input.take());
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
