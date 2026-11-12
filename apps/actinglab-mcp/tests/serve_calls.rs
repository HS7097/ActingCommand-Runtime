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
//!
//! Workflow #434 LAB-5G3 (register §1.287, "Slice LAB-5G3"), each server with a short call budget
//! (`--call-budget`), so that no test waits for today's 25 s:
//! - `mcp_jobs_handles_entry_status`: a read the scripted Runtime holds past the budget answers its
//!   job's handle (`mcp_job_started`, `running`), counted by `actingcommand/entryStatus` while it
//!   runs; `ac_run_show` of the handle answers the job's line, the command line's, and then it
//!   counts no more. A read whose line is far larger than the budget (a pack schedule of made-up
//!   packages) answers `output_budget_exceeded` with its job's handle and the bytes it takes (the
//!   work order's point 10); the finished job counts, unread, until `ac_run_show` of its handle
//!   answers its summary (the line's own outcome, the command line's, and its members' names).
//!   A handle this process never issued is `handle_unknown`. `ac_run_start` answers at once with
//!   `submitting` and a handle that is the request id the Runtime then receives, and its job counts
//!   while it submits.
//! - `mcp_run_show_and_run_stop_wait`: `ac_run_show` of a run not found reads it once without a
//!   wait and once more a second later with one; `ac_run_stop` answers its job's handle after its
//!   `wait_s`, and `ac_run_show` of the handle then the job's line; a wait past the budget, below
//!   0 or of another type is refused before anything is sent.

use actingcommand_contract::{
    EventActor, EventSource, FactScope, IdentifierIssuer, OwnerEpoch, PackSchedulePackage,
    PackScheduleScopeVersion, PackScheduleSource, PackScheduleView, RuntimeControlPlaneStatus,
    RuntimeErrorCode, RuntimeErrorProjection, RuntimeEventQueryPage, RuntimeInstanceStatus,
    RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
    TerminalEvent,
};
use actingcommand_lab_ops::cli_line::Exit;
use actingcommand_lab_ops::core_ops::{self, Caller, Place};
use actingcommand_lab_ops::events::CursorBinding;
use actingcommand_lab_ops::front::{self, Connection, Front};
use actingcommand_lab_ops::table;
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedConnectionRecord, ScriptedGate, ScriptedRecord,
    ScriptedRuntime,
};
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient};
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};
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

// ---- Workflow #434 LAB-5G3: jobs, handles, waits and cursors taken back

/// The made-up instance the scripted status lists.
const INSTANCE: &str = "neutral.a";
/// The made-up packages of a pack schedule view: some 70 bytes of JSON each, so the line takes
/// some 140 KB as text and as much again as `structuredContent`, far past the 24 KiB budget.
const PACKAGES: usize = 2000;

/// `command` with the call budget `seconds` (the start option `--call-budget`).
fn with_call_budget(mut command: Command, seconds: u64) -> Command {
    command.arg("--call-budget").arg(seconds.to_string());
    command
}

/// The answer of `actingcommand/entryStatus`.
fn entry_status(server: &mut Server, id: u64) -> Value {
    server.request(id, "actingcommand/entryStatus", json!({}))["result"].clone()
}

/// The line a `tools/call` answer carries as its text, which `structuredContent` repeats.
#[track_caller]
fn line_of(answer: &Value) -> Value {
    let result = &answer["result"];
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("an answer without its text: {answer}"));
    let line = serde_json::from_str::<Value>(text)
        .unwrap_or_else(|error| panic!("the text is no JSON ({error}): {answer}"));
    assert_eq!(
        result["structuredContent"], line,
        "structuredContent repeats the line"
    );
    line
}

/// The handle of an answer that names a job going on: `{handle, job_phase}` beside
/// `mcp_job_started`, no error.
#[track_caller]
fn handle_of(answer: &Value, phase: &str) -> String {
    let line = line_of(answer);
    assert_eq!(answer["result"]["isError"], json!(false), "{answer}");
    assert_eq!(line["outcome"]["code"], "mcp_job_started", "{line}");
    assert_eq!(line["job_phase"], phase, "{line}");
    line["handle"]
        .as_str()
        .unwrap_or_else(|| panic!("no handle: {line}"))
        .to_owned()
}

/// The code of the outcome a refused answer's line carries, and the answer is an error.
#[track_caller]
fn refused_code(answer: &Value) -> Value {
    assert_eq!(answer["result"]["isError"], json!(true), "{answer}");
    line_of(answer)["outcome"]["code"].clone()
}

/// `Health` on a kept connection before it is used again, answered in `owner_epoch`.
fn healthy(owner_epoch: OwnerEpoch) -> impl FnOnce(&RuntimeRequest) -> ScriptedAnswer + Send {
    move |request| {
        assert!(
            matches!(request.operation(), RuntimeOperation::Health),
            "a kept connection is checked with Health first, not {:?}",
            request.operation()
        );
        ScriptedAnswer::completed(request, RuntimeResult::Health { owner_epoch })
    }
}

/// The Runtime's status in `owner_epoch`, listing the made-up instances `aliases`.
fn status_listing(
    owner_epoch: OwnerEpoch,
    aliases: &'static [&'static str],
) -> impl FnOnce(&RuntimeRequest) -> ScriptedAnswer + Send {
    move |request| {
        assert!(
            matches!(request.operation(), RuntimeOperation::Status),
            "a status read, not {:?}",
            request.operation()
        );
        let issuer = IdentifierIssuer::new().expect("identifiers");
        let instances = aliases
            .iter()
            .copied()
            .map(|alias| {
                let instance_id = *issuer
                    .mint_instance_id()
                    .expect("an instance id")
                    .transport();
                RuntimeInstanceStatus::new(alias, instance_id, false, 0, false, false, false)
                    .expect("an instance's status")
            })
            .collect();
        let status =
            RuntimeControlPlaneStatus::new(owner_epoch, instances).expect("the scripted status");
        ScriptedAnswer::completed(request, RuntimeResult::Status { status })
    }
}

/// `INSTANCE`'s pack schedule view of `PACKAGES` made-up packages.
fn large_pack_schedule(request: &RuntimeRequest) -> ScriptedAnswer {
    assert!(
        matches!(
            request.operation(),
            RuntimeOperation::PackScheduleView { .. }
        ),
        "a pack schedule read, not {:?}",
        request.operation()
    );
    let packages = (0..PACKAGES)
        .map(|index| PackSchedulePackage {
            package_id: format!("neutral.package.{index:04}"),
            schedulable: true,
            source: PackScheduleSource::None,
            row: None,
            next_change_unix_ms: None,
            below: None,
        })
        .collect();
    let view = PackScheduleView {
        instance_alias: INSTANCE.to_owned(),
        scopes: vec![PackScheduleScopeVersion {
            scope: FactScope::Instance {
                instance_id: INSTANCE.to_owned(),
            },
            version: 0,
        }],
        packages,
    };
    ScriptedAnswer::completed(
        request,
        RuntimeResult::PackScheduleViewed {
            view: Box::new(view),
        },
    )
}

/// The client action of today's provenance, recorded.
fn recorded_action(request: &RuntimeRequest) -> ScriptedAnswer {
    assert!(
        matches!(
            request.operation(),
            RuntimeOperation::RecordClientAction { .. }
        ),
        "the client action comes second, not {:?}",
        request.operation()
    );
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

/// A write's own connection, today's provenance first: the identity card accepted, the client
/// action recorded.
fn provenance() -> ScriptedConnection {
    ScriptedConnection::new()
        .request(|request| match request.operation() {
            RuntimeOperation::DeclareGovernanceIdentity { .. } => {
                ScriptedAnswer::completed(request, RuntimeResult::GovernanceIdentityAccepted)
            }
            other => panic!("the identity card comes first, not {other:?}"),
        })
        .request(recorded_action)
}

/// An event query of a ledger that holds nothing.
fn empty_page(request: &RuntimeRequest) -> ScriptedAnswer {
    let RuntimeOperation::QueryEvents { page, .. } = request.operation() else {
        panic!("an event query, not {:?}", request.operation());
    };
    let page = RuntimeEventQueryPage::new(Vec::new(), 0, page.limit(), false, None)
        .expect("an empty page");
    ScriptedAnswer::completed(request, RuntimeResult::EventPage { page })
}

/// How many event queries a connection received.
fn queries(connection: &ScriptedConnectionRecord) -> usize {
    connection.count(|operation| matches!(operation, RuntimeOperation::QueryEvents { .. }))
}

/// A request id the Runtime never issued, as text.
fn made_up_request_id() -> String {
    let issuer = IdentifierIssuer::new().expect("identifiers");
    let request_id = *issuer.mint_request_id().expect("a request id").transport();
    json!(request_id)
        .as_str()
        .expect("a request id is text")
        .to_owned()
}

#[test]
fn mcp_jobs_handles_entry_status() {
    let state = TempDir::new().expect("tempdir");
    let temp = TempDir::new().expect("tempdir");
    let package = temp.path().join("neutral-package.json");
    fs::write(&package, b"{}").expect("write a made-up package");
    let package = package
        .to_str()
        .expect("the temporary folder is UTF-8")
        .to_owned();
    // `hold` keeps the first read's answer back until the test lets it go, `written` says it went;
    // `submit` keeps the run's receipt back.
    let (hold, written, submit) = (
        ScriptedGate::new(),
        ScriptedGate::new(),
        ScriptedGate::new(),
    );
    let runtime = ScriptedRuntime::start(state.path(), |owner_epoch| {
        let held = hold.clone();
        let submitted = submit.clone();
        vec![
            // The server's kept connection: the held read; a pack schedule far past the budget;
            // the instance of the run.
            ScriptedConnection::new()
                .request(move |request| {
                    assert_eq!(operation_of(request), "discover_instances");
                    denied(request).after(&held)
                })
                .open(&written)
                .request(healthy(owner_epoch))
                .request(status_listing(owner_epoch, &[INSTANCE]))
                .request(large_pack_schedule)
                .request(healthy(owner_epoch))
                .request(status_listing(owner_epoch, &[INSTANCE])),
            // The run's own connection: today's provenance, the instance again, the run held and
            // refused.
            provenance()
                .request(status_listing(owner_epoch, &[INSTANCE]))
                .request(move |request| {
                    assert!(
                        matches!(
                            request.operation(),
                            RuntimeOperation::RunContainedTask { .. }
                        ),
                        "the run, not {:?}",
                        request.operation()
                    );
                    denied(request).after(&submitted)
                })
                .ledger(Vec::new()),
            // The command line's code: the same read and the same pack schedule.
            ScriptedConnection::new().request(denied_one("discover_instances")),
            ScriptedConnection::new()
                .request(status_listing(owner_epoch, &[INSTANCE]))
                .request(large_pack_schedule),
        ]
    });
    let mut server = Server::start(with_call_budget(
        actinglab_mcp(Path::new(PROGRAM), Some(state.path()), temp.path()),
        2,
    ));
    server.initialize();

    // A call past the budget answers its job's handle; the job goes on and counts.
    let first = handle_of(&server.call_tool(2, "ac_discover", json!({})), "running");
    assert!(first.starts_with("mcp_job_"), "{first}");
    assert_eq!(
        entry_status(&mut server, 3),
        json!({"outstanding_handles": 1, "kinds": ["ac_discover"], "handles": [first], "calls_in_flight": 0}),
        "a running job counts"
    );
    hold.open();
    written.wait();
    // Its line, read with ac_run_show by the handle.
    let read = server.call_tool(4, "ac_run_show", json!({"id": first, "wait_s": 2}));
    assert_eq!(
        entry_status(&mut server, 5),
        json!({"outstanding_handles": 0, "kinds": [], "handles": [], "calls_in_flight": 0}),
        "read, it counts no more"
    );

    // Work order point 10: a line far past the budget is never cut; the job keeps it.
    let over = server.call_tool(6, "ac_pack_schedule", json!({"instance": INSTANCE}));
    assert_eq!(
        refused_code(&over),
        json!("output_budget_exceeded"),
        "{over}"
    );
    let refusal = line_of(&over);
    let values = &refusal["outcome"]["values"];
    assert!(
        values["actual_bytes"].as_u64() > values["limit_bytes"].as_u64()
            && values["limit_bytes"].as_u64().is_some(),
        "the bytes it takes, past the limit: {refusal}"
    );
    let second = refusal["handle"]
        .as_str()
        .unwrap_or_else(|| panic!("the refusal carries its job's handle: {refusal}"))
        .to_owned();
    assert_eq!(
        entry_status(&mut server, 7),
        json!({"outstanding_handles": 1, "kinds": ["ac_pack_schedule"], "handles": [second], "calls_in_flight": 0}),
        "a finished job whose line was not read counts"
    );
    let summary = server.call_tool(8, "ac_run_show", json!({"id": second}));
    assert_eq!(refused_code(&summary), json!("output_budget_exceeded"));
    let summary = line_of(&summary);
    assert_eq!(
        (&summary["handle"], &summary["job_phase"]),
        (&json!(second), &json!("done")),
        "{summary}"
    );
    assert_eq!(
        entry_status(&mut server, 9)["outstanding_handles"],
        json!(0),
        "read, it counts no more"
    );

    // A handle this process never issued.
    let unknown = server.call_tool(
        10,
        "ac_run_show",
        json!({"id": "mcp_job_0000000000000000_0"}),
    );
    assert_eq!(refused_code(&unknown), json!("handle_unknown"), "{unknown}");

    // ac_run_start answers at once: its handle is the run's request id; its job counts while it
    // submits.
    let started = server.call_tool(
        11,
        "ac_run_start",
        json!({"instance": INSTANCE, "package": package}),
    );
    let run = handle_of(&started, "submitting");
    assert!(line_of(&started)["correlation_id"].is_string(), "{started}");
    assert_eq!(
        entry_status(&mut server, 12),
        json!({"outstanding_handles": 1, "kinds": ["ac_run_start"], "handles": [run], "calls_in_flight": 0}),
        "a run's job counts while it submits"
    );
    submit.open();
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");

    assert_served_as(
        &read,
        &command_line_line(state.path(), "discover", json!({})),
    );
    let (whole, _) =
        command_line_line(state.path(), "pack-schedule", json!({"instance": INSTANCE}));
    let members = whole
        .as_object()
        .expect("a line is an object")
        .keys()
        .filter(|name| name.as_str() != "outcome")
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        summary["summary"],
        json!({"outcome": whole["outcome"], "members": members, "warnings_total": 0}),
        "the summary: the line's own outcome and its members' names"
    );
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the four");
    let submitted = record.connections[1]
        .requests
        .iter()
        .find(|request| {
            matches!(
                request.operation(),
                RuntimeOperation::RunContainedTask { .. }
            )
        })
        .expect("the run was submitted");
    assert_eq!(
        json!(submitted.request_id()),
        json!(run),
        "ac_run_start's handle is the run's request id"
    );
}

#[test]
fn mcp_run_show_and_run_stop_wait() {
    let run = made_up_request_id();
    // ac_run_show of a run not found: read once without a wait; with one of 2 s (in a call budget
    // of 3 s), read again a second later, and then not again before the wait ends.
    let mut read = Vec::new();
    for wait_s in [0_u64, 2] {
        let state = TempDir::new().expect("tempdir");
        let temp = TempDir::new().expect("tempdir");
        let runtime = ScriptedRuntime::start(state.path(), |_| {
            vec![ScriptedConnection::new().ledger(Vec::new())]
        });
        let mut server = Server::start(with_call_budget(
            actinglab_mcp(Path::new(PROGRAM), Some(state.path()), temp.path()),
            3,
        ));
        server.initialize();
        let asked = Instant::now();
        let answer = server.call_tool(2, "ac_run_show", json!({"id": run, "wait_s": wait_s}));
        let waited = asked.elapsed();
        let line = line_of(&answer);
        assert_eq!(line["state"], "not_found", "{line}");
        assert!(
            waited >= Duration::from_secs(wait_s.saturating_sub(1)),
            "waited {waited:?} with wait_s {wait_s}"
        );
        let status = server.finish();
        assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");
        read.push(queries(&only_connection(runtime.finish())));
    }
    assert!(read[0] >= 1, "{read:?}");
    assert_eq!(read[1], 2 * read[0], "read twice with the wait: {read:?}");

    // ac_run_stop: its job waited for as long as its wait_s, then its handle.
    let state = TempDir::new().expect("tempdir");
    let temp = TempDir::new().expect("tempdir");
    let hold = ScriptedGate::new();
    let runtime = ScriptedRuntime::start(state.path(), |owner_epoch| {
        let held = hold.clone();
        vec![
            // The stop's own connection: today's provenance, the run's read held, then a status
            // that names no instance for it.
            provenance()
                .request(move |request| empty_page(request).after(&held))
                .request(status_listing(owner_epoch, &[])),
        ]
    });
    let mut server = Server::start(with_call_budget(
        actinglab_mcp(Path::new(PROGRAM), Some(state.path()), temp.path()),
        3,
    ));
    server.initialize();
    // A wait past the budget, below 0 or of another type: refused, nothing sent.
    for (id, tool, wait_s, problem) in [
        (2, "ac_run_show", json!(4), "out_of_range"),
        (3, "ac_run_show", json!("2"), "wrong_type"),
        (4, "ac_run_stop", json!(4), "out_of_range"),
        (5, "ac_run_stop", json!(-1), "out_of_range"),
    ] {
        let answer = server.call_tool(id, tool, json!({"id": run, "wait_s": wait_s}));
        assert_eq!(
            refused_code(&answer),
            json!("arguments_invalid"),
            "{answer}"
        );
        let values = &line_of(&answer)["outcome"]["values"];
        assert_eq!(
            (&values["argument"], &values["argument_problem"]),
            (&json!("wait_s"), &json!(problem)),
            "{tool} {wait_s}"
        );
        if problem == "out_of_range" {
            assert_eq!(
                (&values["range_min"], &values["range_max"]),
                (&json!(0), &json!(3)),
                "{tool} {wait_s}: 0 to the call budget"
            );
        }
    }
    let asked = Instant::now();
    let stop = server.call_tool(6, "ac_run_stop", json!({"id": run, "wait_s": 1}));
    let waited = asked.elapsed();
    let handle = handle_of(&stop, "running");
    assert!(waited >= Duration::from_secs(1), "waited {waited:?}");
    hold.open();
    let stopped = server.call_tool(7, "ac_run_show", json!({"id": handle, "wait_s": 2}));
    assert_eq!(refused_code(&stopped), json!("run_unknown"), "{stopped}");
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "only the stop's connection");
}
