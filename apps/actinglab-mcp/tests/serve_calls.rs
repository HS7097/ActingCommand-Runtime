// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5G2b (register §1.287, "Slice LAB-5G2b"; contracts/lab-two-entries.md §9.3):
//! the real `actinglab-mcp` over pipes serves the calls of the core rows in its own process (the
//! rescue rows are the slice's second part). The Runtime is the runtime client's scripted one
//! (`test_support`) on loopback in a temporary state root, which the server finds through
//! `ACTINGCOMMAND_RUNTIME_STATE_ROOT`, as actinglab does; `TMP` and `TEMP` name a temporary
//! folder, so its exports stay there. No device, no clock, no sleep; `GATE_WAIT` only bounds a
//! broken run.
//!
//! - `mcp_stdio_one_row_per_kind`: a core read (`discover`) on the server's kept connection and a
//!   core write (`pause`) on a connection of its own, each refused by the scripted Runtime: each
//!   answer's text is the line the command line's code (`lab_ops::core_ops`) gives for the same
//!   parameters, the ids each run issues masked, `structuredContent` repeats it and `isError` is
//!   true exactly when that line exits non-zero. The write's connection carries today's
//!   provenance before the write, the read's nothing of it.
//! - `mcp_refusals_send_nothing`: a member the row does not declare and a parameter the front
//!   gives itself (`unknown`), a member of the wrong type (`wrong_type`), a required one missing
//!   (`required`) and a relative path (`not_absolute`), each `arguments_invalid`, and a string
//!   that begins with `--`, alone or in a list, `actingctl_usage_invalid` `not_recognised` as
//!   `serve` refuses it: each with the argument, `isError` true, and the scripted Runtime receives
//!   no connection at all.
//! - `mcp_write_provenance_as_todays_server`: today's server (`actingctl mcp-serve`, the program
//!   the same build puts beside this one) and this one each pause scheduling once on a scripted
//!   Runtime of their own: both connections carry the same requests, their issued ids and times
//!   left out (`Health`, `BindChannel` on the MCP channel, the identity card `actingctl-mcp`, the
//!   client action of `ac_pause` on surface `mcp`, then `PauseScheduling`), each with the envelope
//!   (Cli, Cli).

use actingcommand_contract::{
    EventActor, EventSource, IdentifierIssuer, RuntimeErrorCode, RuntimeErrorProjection,
    RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
    TerminalEvent,
};
use actingcommand_lab_ops::cli_line::Exit;
use actingcommand_lab_ops::core_ops::{self, Caller, Place};
use actingcommand_lab_ops::events::CursorBinding;
use actingcommand_lab_ops::front::{self, Connection, Front};
use actingcommand_lab_ops::table;
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedConnectionRecord, ScriptedRecord, ScriptedRuntime,
};
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

/// Bounds a broken run only.
const GATE_WAIT: Duration = Duration::from_secs(120);
const PROGRAM: &str = env!("CARGO_BIN_EXE_actinglab-mcp");
/// The variable that names the state root, as actinglab and this server read it.
const STATE_ROOT_VARIABLE: &str = "ACTINGCOMMAND_RUNTIME_STATE_ROOT";
/// The pause's reason, which both servers send.
const REASON: &str = "gate.provenance";

/// A server over pipes. Dropped, it is killed if still running.
struct Server {
    child: Option<Child>,
    input: Option<ChildStdin>,
    lines: Receiver<Result<String, String>>,
}

impl Server {
    fn start(mut command: Command) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn the server");
        let input = child.stdin.take().expect("the server's stdin");
        let output = child.stdout.take().expect("the server's stdout");
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

    fn send(&mut self, message: &Value) {
        let input = self.input.as_mut().expect("the server's stdin is open");
        writeln!(input, "{message}").expect("write a line to the server");
        input.flush().expect("flush the server's stdin");
    }

    /// The next message, or `None` once stdout ended.
    fn next_message(&mut self) -> Option<Value> {
        let line = match self.lines.recv_timeout(GATE_WAIT) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => panic!("cannot read the server's stdout: {error}"),
            Err(RecvTimeoutError::Disconnected) => return None,
            Err(RecvTimeoutError::Timeout) => panic!("the server wrote nothing within GATE_WAIT"),
        };
        Some(serde_json::from_str(&line).unwrap_or_else(|error| {
            panic!("the server wrote a line that is not JSON ({error}): {line}")
        }))
    }

    /// Sends one request and returns its answer, the next message.
    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let answer = self
            .next_message()
            .unwrap_or_else(|| panic!("{method}: stdout ended before its answer"));
        assert_eq!(
            answer["id"],
            json!(id),
            "{method}: not its answer: {answer}"
        );
        answer
    }

    /// A legacy `initialize` of 2025-11-25, then `notifications/initialized`.
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
        assert_eq!(
            answer["result"]["protocolVersion"], "2025-11-25",
            "{answer}"
        );
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    }

    fn call_tool(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        )
    }

    /// Closes stdin, reads stdout to its end and returns the exit status; its connections close
    /// with it.
    fn finish(mut self) -> ExitStatus {
        drop(self.input.take());
        while self.next_message().is_some() {}
        let mut child = self.child.take().expect("the server's process");
        let (sender, exited) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(child.wait().map_err(|error| error.to_string()));
        });
        match exited.recv_timeout(GATE_WAIT) {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => panic!("cannot wait for the server: {error}"),
            Err(error) => panic!("the server did not exit after its stdout ended: {error}"),
        }
    }
}

impl Drop for Server {
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

/// `actinglab-mcp` at `program`, its exports under `temp`, its state root `state_root` when
/// given (else none named).
fn actinglab_mcp(program: &Path, state_root: Option<&Path>, temp: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .env("TMP", temp)
        .env("TEMP", temp)
        .env_remove("ACTINGCOMMAND_DETAIL")
        .env_remove("ACTINGCOMMAND_INITIATOR");
    if let Some(state_root) = state_root {
        command.env(STATE_ROOT_VARIABLE, state_root);
    } else {
        command.env_remove(STATE_ROOT_VARIABLE);
    }
    command
}

/// Today's server, `actingctl mcp-serve`, which the same build puts beside this program.
fn todays_server(state_root: &Path) -> Command {
    let program =
        Path::new(PROGRAM).with_file_name(format!("actingctl{}", std::env::consts::EXE_SUFFIX));
    assert!(
        program.is_file(),
        "today's server is built beside actinglab-mcp: {}",
        program.display()
    );
    let mut command = Command::new(program);
    command
        .arg("mcp-serve")
        .arg("--state-root")
        .arg(state_root)
        .args(["--tier", "operator"])
        .env_remove("ACTINGCOMMAND_DETAIL");
    command
}

/// A refusal of the Runtime: denied, `lease_busy`.
fn denied(request: &RuntimeRequest) -> ScriptedAnswer {
    ScriptedAnswer::failed(
        request,
        RuntimeReceiptState::Denied,
        RuntimeErrorProjection::new(RuntimeErrorCode::LeaseBusy, false).receipt_outcome(None, None),
    )
}

/// The refusal of the one request the script expects, of the kind `kind` names.
fn denied_one(kind: &'static str) -> impl FnOnce(&RuntimeRequest) -> ScriptedAnswer + Send {
    move |request| {
        assert_eq!(
            operation_of(request),
            kind,
            "the scripted Runtime expects {kind}"
        );
        denied(request)
    }
}

/// The name of a request's operation, as it is sent.
fn operation_of(request: &RuntimeRequest) -> String {
    serde_json::to_value(request).expect("a request encodes")["operation"]["operation"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// The connection of one write with today's provenance: the identity card accepted, the client
/// action recorded, the pause refused.
fn write_connection() -> ScriptedConnection {
    ScriptedConnection::new()
        .request(|request| match request.operation() {
            RuntimeOperation::DeclareGovernanceIdentity { .. } => {
                ScriptedAnswer::completed(request, RuntimeResult::GovernanceIdentityAccepted)
            }
            other => panic!("the identity card comes first, not {other:?}"),
        })
        .request(|request| match request.operation() {
            RuntimeOperation::RecordClientAction { .. } => {
                let event_id = *IdentifierIssuer::new()
                    .expect("identifiers")
                    .mint_event_id()
                    .expect("an event id")
                    .transport();
                let receipt = RuntimeReceipt::success(
                    request,
                    RuntimeReceiptState::Completed,
                    Some(TerminalEvent {
                        sequence: 1,
                        event_id,
                    }),
                    RuntimeResult::ClientActionRecorded,
                )
                .expect("the client action's receipt");
                ScriptedAnswer::Receipt(Box::new(receipt))
            }
            other => panic!("the client action comes second, not {other:?}"),
        })
        .request(denied_one("pause_scheduling"))
}

/// A request as sent, without what each connection issues for itself: its ids and its time.
fn sent(request: &RuntimeRequest) -> Value {
    let mut value = serde_json::to_value(request).expect("a request encodes");
    let members = value.as_object_mut().expect("a request is an object");
    for issued in [
        "request_id",
        "correlation_id",
        "causation_id",
        "submitted_at_unix_ms",
    ] {
        members.remove(issued);
    }
    value
}

/// `value` with the ids each run issues for itself masked, wherever they are.
fn masked(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(name, member)| {
                    let member = if matches!(
                        name.as_str(),
                        "request_id" | "correlation_id" | "causation_id" | "req_id"
                    ) {
                        json!("<issued>")
                    } else {
                        masked(member)
                    };
                    (name.clone(), member)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(masked).collect()),
        other => other.clone(),
    }
}

/// The line and the exit code of `exit`.
fn line_and_code(exit: Exit) -> (Value, u8) {
    (exit.line.expect("the line encodes"), exit.code)
}

/// What the command line's code answers for the core row `name` with `params`, on a connection of
/// its own to the Runtime of `state_root` (the command line passes no budget).
fn command_line_line(state_root: &Path, name: &str, params: Value) -> (Value, u8) {
    let table = table::operation_table().expect("the operation table reads");
    let operation = table
        .operation(name)
        .unwrap_or_else(|| panic!("the table has no row {name}"));
    let Value::Object(params) = params else {
        panic!("parameters are an object");
    };
    let client = RuntimeClient::connect(front::config(Front::Mcp, state_root))
        .expect("connect to the scripted Runtime");
    let place = || Place {
        state_root: Ok(state_root.to_path_buf()),
        root: None,
    };
    let caller = Caller {
        place: &place,
        unlocated: "",
        connection: Connection::Kept(&client),
        budget: None,
        cursor: CursorBinding::fresh(),
    };
    line_and_code(core_ops::answer(operation, &params, &caller))
}

/// A `tools/call` answer that serves the line `expected` with its exit code: the text is the
/// line, the ids masked; `structuredContent` repeats the text; `isError` is the non-zero exit.
#[track_caller]
fn assert_served_as(answer: &Value, expected: &(Value, u8)) {
    let result = &answer["result"];
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("an answer without its text: {answer}"));
    let line = serde_json::from_str::<Value>(text)
        .unwrap_or_else(|error| panic!("the text is no JSON ({error}): {answer}"));
    assert_eq!(
        masked(&line),
        masked(&expected.0),
        "the line the command line's code gives"
    );
    assert_eq!(
        result["structuredContent"], line,
        "structuredContent repeats it"
    );
    assert_eq!(
        result["isError"],
        json!(expected.1 != 0),
        "isError is the command line's non-zero exit"
    );
}

/// The one scripted connection of `record`.
fn only_connection(record: ScriptedRecord) -> ScriptedConnectionRecord {
    assert!(
        record.unscripted.is_empty(),
        "a connection beyond the script"
    );
    let mut connections = record.connections;
    assert_eq!(connections.len(), 1, "one connection");
    connections.remove(0)
}

/// The operations a connection received, in order.
fn operations(connection: &ScriptedConnectionRecord) -> Vec<String> {
    connection.requests.iter().map(operation_of).collect()
}

#[test]
fn mcp_stdio_one_row_per_kind() {
    let state = TempDir::new().expect("tempdir");
    let temp = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(state.path(), |_| {
        vec![
            // The server's kept connection: the core read.
            ScriptedConnection::new().request(denied_one("discover_instances")),
            // The server's write connection: today's provenance, then the core write.
            write_connection(),
            // The command line's code: the same read and the same write, each on its own.
            ScriptedConnection::new().request(denied_one("discover_instances")),
            ScriptedConnection::new().request(denied_one("pause_scheduling")),
        ]
    });
    let mut server = Server::start(actinglab_mcp(
        Path::new(PROGRAM),
        Some(state.path()),
        temp.path(),
    ));
    server.initialize();
    let read = server.call_tool(2, "ac_discover", json!({}));
    let write = server.call_tool(3, "ac_pause", json!({"reason": REASON, "drain": "60s"}));
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");

    assert_served_as(
        &read,
        &command_line_line(state.path(), "discover", json!({})),
    );
    assert_served_as(
        &write,
        &command_line_line(
            state.path(),
            "pause",
            json!({"reason": REASON, "drain": "60s"}),
        ),
    );
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the four");
    let [kept, written, ..] = record.connections.as_slice() else {
        panic!("{} connections, not four", record.connections.len());
    };
    assert_eq!(
        operations(kept),
        ["health", "bind_channel", "discover_instances"],
        "the read: on the kept connection, no provenance"
    );
    assert_eq!(
        operations(written),
        [
            "health",
            "bind_channel",
            "declare_governance_identity",
            "record_client_action",
            "pause_scheduling",
        ],
        "the write: today's provenance first, on a connection of its own"
    );
    let action = sent(&written.requests[3]);
    assert_eq!(
        (
            &action["operation"]["action"]["surface_id"],
            &action["operation"]["action"]["control_id"],
        ),
        (&json!("mcp"), &json!("ac_pause")),
        "{action}"
    );
}

#[test]
fn mcp_refusals_send_nothing() {
    let state = TempDir::new().expect("tempdir");
    let temp = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(state.path(), |_| Vec::new());
    let mut server = Server::start(actinglab_mcp(
        Path::new(PROGRAM),
        Some(state.path()),
        temp.path(),
    ));
    server.initialize();
    // Each case: the tool, its arguments, and the refusal's code, argument and problem. A value
    // that begins with `--` is refused as `serve` refuses it (actingctl_usage_invalid).
    const ARGUMENTS_INVALID: &str = "arguments_invalid";
    const USAGE_INVALID: &str = "actingctl_usage_invalid";
    let cases = [
        (
            "ac_pause",
            json!({"reason": REASON, "neutral_undeclared": true}),
            ARGUMENTS_INVALID,
            "neutral_undeclared",
            "unknown",
        ),
        (
            "ac_frame",
            json!({"instance": "neutral.frame", "out": "C:\\neutral"}),
            ARGUMENTS_INVALID,
            "out",
            "unknown",
        ),
        (
            "ac_pause",
            json!({"reason": 7}),
            ARGUMENTS_INVALID,
            "reason",
            "wrong_type",
        ),
        (
            "ac_pause",
            json!({}),
            ARGUMENTS_INVALID,
            "reason",
            "required",
        ),
        (
            "ac_facts_publish",
            json!({"file": "relative\\observation.json"}),
            ARGUMENTS_INVALID,
            "file",
            "not_absolute",
        ),
        (
            "ac_pause",
            json!({"reason": "--drain"}),
            USAGE_INVALID,
            "reason",
            "not_recognised",
        ),
        (
            "ac_config_set",
            json!({"changes": ["neutral.key=1", "--neutral=1"]}),
            USAGE_INVALID,
            "changes",
            "not_recognised",
        ),
    ];
    for (id, (tool, arguments, code, argument, problem)) in (2_u64..).zip(cases) {
        let answer = server.call_tool(id, tool, arguments);
        let result = &answer["result"];
        assert_eq!(result["isError"], json!(true), "{tool}: {answer}");
        let line = serde_json::from_str::<Value>(
            result["content"][0]["text"]
                .as_str()
                .unwrap_or_else(|| panic!("{tool}: no text: {answer}")),
        )
        .unwrap_or_else(|error| panic!("{tool}: the text is no JSON ({error}): {answer}"));
        let outcome = &line["outcome"];
        assert_eq!(
            (
                &outcome["code"],
                &outcome["values"]["argument"],
                &outcome["values"]["argument_problem"],
            ),
            (&json!(code), &json!(argument), &json!(problem)),
            "{tool}: {line}"
        );
    }
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");
    let record = runtime.finish();
    assert!(
        record.connections.is_empty() && record.unscripted.is_empty(),
        "nothing reached the Runtime: {record:?}"
    );
}

#[test]
fn mcp_write_provenance_as_todays_server() {
    let todays_state = TempDir::new().expect("tempdir");
    let state = TempDir::new().expect("tempdir");
    let temp = TempDir::new().expect("tempdir");
    let todays_runtime = ScriptedRuntime::start(todays_state.path(), |_| vec![write_connection()]);
    let runtime = ScriptedRuntime::start(state.path(), |_| vec![write_connection()]);

    // Today's server: `ac_pause` with its own arguments for the same pause.
    let mut todays = Server::start(todays_server(todays_state.path()));
    todays.initialize();
    let answer = todays.call_tool(
        2,
        "ac_pause",
        json!({"reason": REASON, "drain_timeout_s": 60}),
    );
    assert_eq!(
        answer["result"]["isError"],
        json!(true),
        "refused: {answer}"
    );
    let _ = todays.finish();

    // This server: the row's parameters.
    let mut server = Server::start(actinglab_mcp(
        Path::new(PROGRAM),
        Some(state.path()),
        temp.path(),
    ));
    server.initialize();
    let answer = server.call_tool(2, "ac_pause", json!({"reason": REASON, "drain": "60s"}));
    assert_eq!(
        answer["result"]["isError"],
        json!(true),
        "refused: {answer}"
    );
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");

    let todays_connection = only_connection(todays_runtime.finish());
    let connection = only_connection(runtime.finish());
    let todays_sent = todays_connection
        .requests
        .iter()
        .map(sent)
        .collect::<Vec<_>>();
    assert_eq!(
        connection.requests.iter().map(sent).collect::<Vec<_>>(),
        todays_sent,
        "the new server sends what today's server sends for the same write"
    );
    assert_eq!(
        operations(&connection),
        [
            "health",
            "bind_channel",
            "declare_governance_identity",
            "record_client_action",
            "pause_scheduling",
        ]
    );
    let RuntimeOperation::BindChannel { channel, via } = connection.requests[1].operation() else {
        panic!("the second request is no BindChannel");
    };
    assert_eq!((*channel, *via), (RuntimeChannel::Mcp, None));
    assert_eq!(
        todays_sent[2]["operation"]["card"]["client"],
        json!("actingctl-mcp"),
        "the identity card"
    );
    for request in &todays_sent {
        assert_eq!(
            (&request["actor"], &request["source"]),
            (&json!(EventActor::Cli), &json!(EventSource::Cli)),
            "{request}"
        );
    }
}
