// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5G2b (register §1.287, "Slice LAB-5G2b" and its second part;
//! contracts/lab-two-entries.md §9.3): the real `actinglab-mcp` over pipes serves the calls of the
//! core rows in its own process and, on Windows, those of the rescue rows (the slice's second
//! part, from a copy of the program in a made-up root). The Runtime is the runtime client's
//! scripted one (`test_support`) on loopback in a temporary state root, which the server finds
//! through `ACTINGCOMMAND_RUNTIME_STATE_ROOT`, as actinglab does; `TMP` and `TEMP` name a
//! temporary folder, so its exports stay there. No device, no clock, no sleep (but the bounded
//! waits of LAB-5G4b's pool test, below); `GATE_WAIT` only bounds a broken run.
//!
//! - `mcp_stdio_one_row_per_kind`: a core read (`discover`) on the server's kept connection and a
//!   core write (`pause`) on a connection of its own, each refused by the scripted Runtime: each
//!   answer's text is the line the command line's code (`lab_ops::core_ops`) gives for the same
//!   parameters, the ids each run issues masked, `structuredContent` repeats it and `isError` is
//!   true exactly when that line exits non-zero. The write's connection carries today's
//!   provenance before the write, the read's nothing of it. The second part, on Windows: a rescue
//!   row that needs no Runtime (`leftovers`), answered by a copy of the program in a made-up root,
//!   is the line the rescue module's code gives for it on that root. Workflow #434 LAB-5G4b: a row
//!   a `serve` child serves (`package check` of a package that is not there), answered through the
//!   real actinglab the CI job builds beside this program (`ACTINGCOMMAND_TEST_ACTINGLAB_EXE`), is
//!   the line that actinglab's command line prints for it with `--json`, `isError` its non-zero
//!   exit.
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
//! - `mcp_cursor_bound_to_connection`: an `events` cursor is taken back on the connection it was
//!   issued on (the rest of the page) and is `cursor_invalid`, nothing read, once the connection was
//!   made again; a `codes` cursor gives the page the command line gives for it, and a `runs` cursor
//!   the line the command line's code gives for it.
//! - `mcp_run_show_and_run_stop_wait`: `ac_run_show` of a run not found reads it once without a
//!   wait and once more a second later with one; `ac_run_stop` answers its job's handle after its
//!   `wait_s`, and `ac_run_show` of the handle then the job's line; a wait past the budget, below
//!   0 or of another type is refused before anything is sent.
//!
//! The second part's own (Windows, where the rescue module runs; a made-up single-location root in
//! a temporary folder whose build carries a copy of this program in `tools\`, adopted, no Runtime):
//! - `mcp_plan_through_the_server_is_the_command_lines`: `ac_mcp_plan` answers the line the rescue
//!   module's code gives for `mcp plan` on that root, in the shape of LAB-5G5b's checked-in sample,
//!   its child the server's own file.
//! - `mcp_detached_rows_refused_and_nothing_started`: `ac_deploy` and `ac_rollback` answer
//!   `mcp_operation_unsupported` with their tool, and the root holds the same files after them (no
//!   detached log, no journal).
//! - `mcp_rescue_rows_refused_before_the_module`: an undeclared member (`unknown`) and a relative
//!   path in `release stage`'s `dir` (`not_absolute`) are `arguments_invalid`, which the rescue
//!   module never answers, with the argument; the root holds the same files after them.
//!
//! Workflow #434 LAB-5G4b (register §1.287, "Slice LAB-5G4b"), the `serve` children, with this
//! package's made-up `actinglab` (`src/test_child.rs`), copied with a copy of the program into a
//! temporary folder, where the server finds it beside itself; it logs each start, request and end
//! to a folder of the test's. Bounded waits on that log, no sleep of a default:
//! - `mcp_serve_children_pool`: `--children` and `--child-idle` refused as `--jobs` is (outside
//!   their ranges, not a whole number, without a value, twice, beside `--list-tools`); a child is
//!   started on the first call and used again by the next; two calls at once use two children,
//!   and a third past `--children 2` is refused `job_capacity_exhausted` with `limit_count` 2;
//!   every child is started as `serve --stdio --front mcp` with the state root variable set and
//!   `ACTINGCOMMAND_INITIATOR` removed; at the end of the server's input every child's input is
//!   closed and each ends. A child that ends before its answer gives `lab_process_failed` with its
//!   exit code and its stderr's tail, its request is not sent again, and the next call starts a new
//!   child under `--children 1`. With `--child-idle 0`, and with 1, the child ends after its
//!   answer without another call while the server goes on.
//!
//! Workflow #434 LAB-5G4b, its second part (register §1.287, "Slice LAB-5G4b"), with the same
//! made-up `actinglab` answering each request of a test's instance with the envelope the test
//! wrote for it (its line, exit code and request id):
//! - `mcp_lab_overflow_rule`: a child's success line far past the budget is written whole to the
//!   server's export directory (the file holds the child's line, its SHA-256 and size as the
//!   reference says) and answered `{req_id, export, overflowed: true}` with `answer_exported`
//!   first and the child's own warnings after it: `record stop` keeps its four binding parts under
//!   `lab`; `observe` and `do` their frame parts in order, a part that does not fit left out and
//!   the next one kept. With a file where the export directory would be: no `export`, the export's
//!   `material_export_failed` first, the parts kept. A failure line past the budget is not
//!   exported: it is sent in `structuredContent` alone beside a summary text, and no file is
//!   written.
//! - `mcp_capture_records_client_action`: after the child of `ac_lab_observe` or `ac_do` with
//!   `capture` true answered, the scripted Runtime receives on a connection of its own the identity
//!   card and the client action (surface `mcp`, the tool, kind `command`, the call's instance, the
//!   envelope's `req_id` as its value), from (Cli, Cli); without a request id it is recorded
//!   without a value and the line carries `client_action_without_req_id`; a recording the Runtime
//!   refuses gives `client_action_not_recorded` caused by the refusal and the call's answer stands;
//!   `capture` absent or false records nothing; `capture` with `dry_run` records, as today. Each
//!   Lab command reaches the child once.

use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin,
    EventPayloadDraft, EventSeverity, EventSource, FactScope, IdentifierIssuer, OriginModule,
    OwnerEpoch, PackSchedulePackage, PackScheduleScopeVersion, PackScheduleSource,
    PackScheduleView, ProjectedEvent, ProjectionPayload, RuntimeControlPlaneStatus,
    RuntimeErrorCode, RuntimeErrorProjection, RuntimeEventQueryCursor, RuntimeEventQueryPage,
    RuntimeInstanceStatus, RuntimeOperation, RuntimePayloadDraft, RuntimeReceipt,
    RuntimeReceiptState, RuntimeRequest, RuntimeResult, SanitizationError, SecretField,
    SecretFingerprinter, Sha256Fingerprint, TerminalEvent,
};
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::code_lookup;
use actingcommand_lab_ops::codes::ActingctlCode;
use actingcommand_lab_ops::core_ops::{self, Caller, Place};
use actingcommand_lab_ops::events::CursorBinding;
use actingcommand_lab_ops::front::{self, Connection, Front};
use actingcommand_lab_ops::table;
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedConnectionRecord, ScriptedGate, ScriptedRecord,
    ScriptedRuntime,
};
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
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

    // Workflow #434 LAB-5G2b, second part: the rescue row.
    #[cfg(windows)]
    rescue_row_on_a_made_up_root(temp.path());

    // Workflow #434 LAB-5G4b: a row a `serve` child serves.
    child_row_from_the_real_actinglab(temp.path());
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
/// The time of the made-up record at sequence 0; a record's time is this plus its sequence.
const RECORDED_FROM_UNIX_MS: u64 = 1_700_000_000_000;

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

/// One page of `events` for the event query `request` sends, as the scripted ledger answers one:
/// the records after its cursor and from its first sequence, oldest first, at the snapshot of the
/// last record, with the cursor after the page while more remain.
fn page_of(events: &[ProjectedEvent], request: &RuntimeRequest) -> ScriptedAnswer {
    let RuntimeOperation::QueryEvents {
        query,
        profile,
        page,
    } = request.operation()
    else {
        panic!("an event query, not {:?}", request.operation());
    };
    let snapshot = events.last().map_or(0, |event| event.sequence);
    let after = page
        .cursor()
        .map_or(0, RuntimeEventQueryCursor::after_sequence);
    let mut matching = events.iter().filter(|event| {
        event.sequence > after
            && query
                .from_sequence
                .is_none_or(|from| event.sequence >= from)
    });
    let rows = matching
        .by_ref()
        .take(usize::from(page.limit()))
        .cloned()
        .collect::<Vec<_>>();
    let has_more = matching.next().is_some();
    let next = rows.last().filter(|_| has_more).map(|last| {
        RuntimeEventQueryCursor::new(snapshot, last.sequence, query, *profile)
            .expect("the page's next cursor")
    });
    let page = RuntimeEventQueryPage::new(rows, snapshot, page.limit(), has_more, next)
        .expect("an event page");
    ScriptedAnswer::completed(request, RuntimeResult::EventPage { page })
}

struct NoSecrets;

impl SecretFingerprinter for NoSecrets {
    fn fingerprint(
        &self,
        _field: SecretField,
        _original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        panic!("the made-up records hold no secret")
    }
}

/// Made-up records at the sequences 1 to `count`, as a forensic read projects them.
fn ledger(count: u64) -> Vec<ProjectedEvent> {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    (1..=count)
        .map(|sequence| {
            let payload: EventPayloadDraft =
                RuntimePayloadDraft::started(EventAction::RuntimeStart, AuditInput::new()).into();
            let sanitized = EventDraft::new(
                issuer.mint_event_id().expect("event id"),
                RECORDED_FROM_UNIX_MS + sequence,
                EventSeverity::Info,
                EventOrigin::new(
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                ),
                EventLinksDraft::default(),
                payload,
            )
            .sanitize(&NoSecrets)
            .expect("sanitize a made-up record");
            ProjectedEvent {
                views: Vec::new(),
                artifact_evictions: Vec::new(),
                schema_version: sanitized.schema_version().to_owned(),
                sequence,
                event_id: *sanitized.event_id(),
                timestamp_unix_ms: sanitized.timestamp_unix_ms(),
                event_type: sanitized.event_type(),
                severity: sanitized.severity(),
                sensitivity: sanitized.sensitivity(),
                origin: sanitized.origin().clone(),
                links: sanitized.links().clone(),
                payload_schema: sanitized.payload_schema().to_owned(),
                payload: ProjectionPayload::Full(Box::new(sanitized.payload().clone())),
                artifacts: Vec::new(),
            }
        })
        .collect()
}

/// The seq of each row of an `events` line.
fn sequences(line: &Value) -> Vec<u64> {
    line["events"]
        .as_array()
        .unwrap_or_else(|| panic!("a line without events: {line}"))
        .iter()
        .map(|row| row["seq"].as_u64().expect("a row's seq"))
        .collect()
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
fn mcp_cursor_bound_to_connection() {
    let state = TempDir::new().expect("tempdir");
    let temp = TempDir::new().expect("tempdir");
    let records = ledger(4);
    let runtime = ScriptedRuntime::start(state.path(), |owner_epoch| {
        let (first_page, rest) = (records.clone(), records.clone());
        vec![
            // The kept connection: a page of two of four records, then the rest from its cursor;
            // then its Health answered by a close, so the connection is made again.
            ScriptedConnection::new()
                .request(move |request| page_of(&first_page, request))
                .request(healthy(owner_epoch))
                .request(move |request| page_of(&rest, request))
                .request(|request| {
                    assert!(
                        matches!(request.operation(), RuntimeOperation::Health),
                        "a kept connection is checked with Health first, not {:?}",
                        request.operation()
                    );
                    ScriptedAnswer::Close
                }),
            // The connection made again: the cursor refused, nothing read; then `runs`.
            ScriptedConnection::new().request(healthy(owner_epoch)),
            // The command line's code: `runs` with the same cursor.
            ScriptedConnection::new(),
        ]
    });
    let mut server = Server::start(actinglab_mcp(
        Path::new(PROGRAM),
        Some(state.path()),
        temp.path(),
    ));
    server.initialize();

    // `events`: a cursor issued on the kept connection, the first one.
    let first = line_of(&server.call_tool(2, "ac_events", json!({"after": 0, "limit": 2})));
    assert_eq!(sequences(&first), [1, 2], "{first}");
    let cursor = first["next_cursor"]
        .as_str()
        .unwrap_or_else(|| panic!("no next_cursor: {first}"))
        .to_owned();
    let parts = cursor.split('.').collect::<Vec<_>>();
    assert_eq!(
        (parts.first().copied(), parts.get(2).copied()),
        (Some("v1"), Some("1")),
        "bound to this server's first connection: {cursor}"
    );
    let taken = server.call_tool(3, "ac_events", json!({"cursor": cursor}));
    assert_eq!(taken["result"]["isError"], json!(false), "{taken}");
    assert_eq!(sequences(&line_of(&taken)), [3, 4], "the rest of the page");
    // Once the connection was made again: cursor_invalid, nothing read.
    let refused = server.call_tool(4, "ac_events", json!({"cursor": cursor}));
    assert_eq!(refused_code(&refused), json!("cursor_invalid"), "{refused}");

    // `codes`: its cursor carries its own position; the next page is the command line's.
    let codes =
        line_of(&server.call_tool(5, "ac_codes", json!({"prefix": "actingctl_", "limit": 2})));
    let next = codes["next_cursor"]
        .as_str()
        .unwrap_or_else(|| panic!("no next_cursor: {codes}"))
        .to_owned();
    let second = line_of(&server.call_tool(
        6,
        "ac_codes",
        json!({"prefix": "actingctl_", "limit": 2, "cursor": next}),
    ));
    let words = [
        "codes",
        "--prefix",
        "actingctl_",
        "--limit",
        "2",
        "--cursor",
        next.as_str(),
    ]
    .map(OsString::from);
    let expected = code_lookup::dispatch(&words)
        .expect("the words of codes")
        .line
        .expect("the line encodes");
    assert_eq!(
        second, expected,
        "the next page, as the command line gives it"
    );
    assert_ne!(second["codes"], codes["codes"], "the next page");

    // `runs`: its cursor is its row's own and passes as the command line takes it.
    let runs = server.call_tool(7, "ac_runs", json!({"cursor": "r1.0"}));
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");
    let expected = command_line_line(state.path(), "runs", json!({"cursor": "r1.0"}));
    assert_served_as(&runs, &expected);
    let record = runtime.finish();
    assert!(
        record.unscripted.is_empty(),
        "a connection beyond the three"
    );
    assert_eq!(
        record.connections.iter().map(queries).collect::<Vec<_>>(),
        [2, 0, 0],
        "the refused cursor read nothing"
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

// ---- Workflow #434 LAB-5G2b, second part: the rescue rows, on Windows, where the rescue module
// runs, served by a copy of the program in a made-up root.

/// The build the made-up root installs, and another, the current build `mcp plan` is asked with.
#[cfg(windows)]
const BUILD: &str = "2222222222222222222222222222222222222222";
#[cfg(windows)]
const CURRENT: &str = "1111111111111111111111111111111111111111";
/// A file a deploy kept aside in the made-up root's `tools\`, which `leftovers` lists.
#[cfg(windows)]
const KEPT_ASIDE: &str = "neutral.exe.old-1000";
/// LAB-5G5b's checked-in answer of `mcp plan`, whose shape the server's answer has.
#[cfg(windows)]
const PLAN_SAMPLE: &str =
    include_str!("../../../crates/lab-rescue/tests/fixtures/mcp-plan-entry-protocol-2.json");

/// Writes `bytes` to `path`, its folders made first.
#[cfg(windows)]
fn write_file(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("make the folder");
    std::fs::write(path, bytes).expect("write the file");
}

/// A made-up single-location root in `base` whose build carries this server, and the program in
/// it: the program copied as `tools\actinglab-mcp.exe` and listed with its size and SHA-256 in the
/// Tools manifest, the Runtime manifest of the same build listing nothing, a configuration whose
/// state root is in `base`, `install\state.json` of an adopted root without a kept build (so that
/// a detached run would start there), and a file a deploy kept aside in `tools\`.
#[cfg(windows)]
fn made_up_root(base: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    use sha2::{Digest, Sha256};
    let root = base.join("root");
    let program = root.join("tools").join("actinglab-mcp.exe");
    std::fs::create_dir_all(root.join("tools")).expect("make the Tools folder");
    std::fs::copy(PROGRAM, &program).expect("copy the program into the root");
    let copied = std::fs::read(&program).expect("read the copy");
    let runtime = json!({
        "repository": "neutral/runtime",
        "commit_sha": BUILD,
        "files": [],
        "runtime_payload_layout": "distribution-v1",
    });
    write_file(
        &root.join("runtime").join("BUILD-MANIFEST.json"),
        runtime.to_string().as_bytes(),
    );
    let tools = json!({
        "repository": "neutral/runtime",
        "commit_sha": BUILD,
        "files": [{
            "path": "actinglab-mcp.exe",
            "size_bytes": copied.len(),
            "sha256": format!("{:x}", Sha256::digest(copied.as_slice())),
        }],
        "tools_payload_layout": "platform-tools-v3",
    });
    write_file(
        &root.join("tools").join("BUILD-MANIFEST.json"),
        tools.to_string().as_bytes(),
    );
    write_file(&root.join("tools").join(KEPT_ASIDE), b"neutral");
    let config = json!({"state_root": base.join("state"), "instances": []});
    write_file(
        &root.join("actingd.config.json"),
        config.to_string().as_bytes(),
    );
    let state = json!({
        "schema_version": "actingcommand.install-state.v1",
        "live": null,
        "builds": [],
        "packs": [],
        "config_sha256": null,
    });
    write_file(
        &root.join("install").join("state.json"),
        state.to_string().as_bytes(),
    );
    (root, program)
}

/// The line the rescue module's code gives for its row `name` with `params` on the root of the
/// program at `program`, with the initiator `Lab-MCP`, as actinglab's `--json` prints a line
/// (`apps/actinglab/src/cli_module.rs`, `rescue::line`); these rows answer one.
#[cfg(windows)]
fn module_line(program: &Path, name: &str, params: Value) -> (Value, u8) {
    use actingcommand_lab_ops::initiator::Initiator;
    use actingcommand_lab_ops::locate;
    use actingcommand_lab_rescue::agent_channel::Answer;
    let Value::Object(params) = params else {
        panic!("parameters are an object");
    };
    let program = std::fs::canonicalize(program).expect("the copy's path");
    let location = locate::locate_from(&program, None).expect("the copy's root locates");
    let Answer::Line(exit) =
        actingcommand_lab_rescue::rescue::answer(name, &params, &location, Initiator::LabMcp)
    else {
        panic!("{name} answers a line");
    };
    line_and_code(exit)
}

/// The line a `tools/call` answer carries as its text.
#[cfg(windows)]
fn answered_line(answer: &Value) -> Value {
    let text = answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("an answer without its text: {answer}"));
    serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("the text is no JSON ({error}): {answer}"))
}

/// Every file under `root`, as a path relative to it.
#[cfg(windows)]
fn files_under(root: &Path) -> std::collections::BTreeSet<std::path::PathBuf> {
    let mut found = std::collections::BTreeSet::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).expect("read a folder of the root") {
            let path = entry.expect("an entry of the root").path();
            if path.is_dir() {
                folders.push(path);
            } else {
                let relative = path.strip_prefix(root).expect("a file under the root");
                found.insert(relative.to_path_buf());
            }
        }
    }
    found
}

/// `mcp_stdio_one_row_per_kind`'s rescue row: `leftovers`, which needs no Runtime, answered over
/// pipes by a copy of the program in a made-up root, is the line the rescue module's code gives
/// for it on that root (the file kept aside listed). Its exports go under `temp`.
#[cfg(windows)]
fn rescue_row_on_a_made_up_root(temp: &Path) {
    let base = TempDir::new().expect("tempdir");
    let (_, program) = made_up_root(base.path());
    let mut server = Server::start(actinglab_mcp(&program, None, temp));
    server.initialize();
    let answer = server.call_tool(2, "ac_leftovers", json!({}));
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");
    let expected = module_line(&program, "leftovers", json!({}));
    let listed = expected.0["leftovers"].as_array().is_some_and(|rows| {
        rows.iter().any(|row| {
            row["path"]
                .as_str()
                .is_some_and(|path| path.ends_with(KEPT_ASIDE))
        })
    });
    assert!(
        listed,
        "setup: the module lists the file kept aside: {}",
        expected.0
    );
    assert_served_as(&answer, &expected);
}

#[cfg(windows)]
#[test]
fn mcp_plan_through_the_server_is_the_command_lines() {
    use std::collections::BTreeSet;
    let base = TempDir::new().expect("tempdir");
    let temp = TempDir::new().expect("tempdir");
    let (_, program) = made_up_root(base.path());
    let arguments = json!({"entry_protocol": 2, "current_build": CURRENT});
    let mut server = Server::start(actinglab_mcp(&program, None, temp.path()));
    server.initialize();
    let answer = server.call_tool(2, "ac_mcp_plan", arguments.clone());
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");
    let expected = module_line(&program, "mcp plan", arguments);
    assert_served_as(&answer, &expected);

    // The shape of LAB-5G5b's sample: its members, the build's and the tunables', the plan given
    // (not refused), a switch from the current build, and one child, the server's own file.
    let line = &expected.0;
    let sample = serde_json::from_str::<Value>(PLAN_SAMPLE).expect("the sample is JSON");
    let members = |value: &Value| {
        value
            .as_object()
            .map(|object| object.keys().cloned().collect::<BTreeSet<_>>())
            .unwrap_or_default()
    };
    for (given, sampled) in [
        (line, &sample),
        (&line["build"], &sample["build"]),
        (&line["tunables"], &sample["tunables"]),
    ] {
        assert_eq!(
            members(given),
            members(sampled),
            "the sample's members: {line}"
        );
    }
    assert_eq!(
        (
            &line["outcome"]["code"],
            &line["switch"],
            &line["build"]["commit"],
            &line["entry_protocol"],
        ),
        (
            &sample["outcome"]["code"],
            &json!(true),
            &json!(BUILD),
            &json!(2),
        ),
        "{line}"
    );
    let [child] = line["child"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    else {
        panic!("one child: {line}");
    };
    let child = child
        .as_str()
        .unwrap_or_else(|| panic!("the child is a path: {line}"));
    assert_eq!(
        std::fs::canonicalize(child).expect("the child is in place"),
        std::fs::canonicalize(&program).expect("the copy's path"),
        "the child is the server's own file: {line}"
    );
}

#[cfg(windows)]
#[test]
fn mcp_detached_rows_refused_and_nothing_started() {
    let base = TempDir::new().expect("tempdir");
    let temp = TempDir::new().expect("tempdir");
    let (root, program) = made_up_root(base.path());
    let before = files_under(&root);
    let mut server = Server::start(actinglab_mcp(&program, None, temp.path()));
    server.initialize();
    let deploy = server.call_tool(2, "ac_deploy", json!({"target": BUILD}));
    let rollback = server.call_tool(3, "ac_rollback", json!({}));
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");
    for (tool, answer) in [("ac_deploy", &deploy), ("ac_rollback", &rollback)] {
        assert_eq!(answer["result"]["isError"], json!(true), "{tool}: {answer}");
        let line = answered_line(answer);
        assert_eq!(
            (
                &line["outcome"]["code"],
                &line["outcome"]["values"]["mcp_tool"]
            ),
            (&json!("mcp_operation_unsupported"), &json!(tool)),
            "{tool}: {line}"
        );
    }
    assert_eq!(
        files_under(&root),
        before,
        "nothing started: no detached log, no journal"
    );
}

#[cfg(windows)]
#[test]
fn mcp_rescue_rows_refused_before_the_module() {
    let base = TempDir::new().expect("tempdir");
    let temp = TempDir::new().expect("tempdir");
    let (root, program) = made_up_root(base.path());
    let before = files_under(&root);
    let mut server = Server::start(actinglab_mcp(&program, None, temp.path()));
    server.initialize();
    // Each case: the tool, its arguments, and the argument refused with its problem.
    let cases = [
        (
            "ac_leftovers",
            json!({"neutral_undeclared": true}),
            "neutral_undeclared",
            "unknown",
        ),
        (
            "ac_release_stage",
            json!({"dir": "relative\\release"}),
            "dir",
            "not_absolute",
        ),
    ];
    for (id, (tool, arguments, argument, problem)) in (2_u64..).zip(cases) {
        let answer = server.call_tool(id, tool, arguments);
        assert_eq!(answer["result"]["isError"], json!(true), "{tool}: {answer}");
        let line = answered_line(&answer);
        let outcome = &line["outcome"];
        assert_eq!(
            (
                &outcome["code"],
                &outcome["values"]["argument"],
                &outcome["values"]["argument_problem"],
            ),
            (
                &json!("arguments_invalid"),
                &json!(argument),
                &json!(problem)
            ),
            "{tool}: {line}"
        );
    }
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");
    assert_eq!(files_under(&root), before, "the module wrote nothing");
}

// ---- Workflow #434 LAB-5G4b: the rows served through `serve` children.

/// The variable CI sets in the job these tests run in to the actinglab it builds
/// (`.github/workflows/ci.yml`, the `apps` job), which is the file beside this program.
const ACTINGLAB_VARIABLE: &str = "ACTINGCOMMAND_TEST_ACTINGLAB_EXE";
/// This package's made-up `actinglab` (`src/test_child.rs`).
const TEST_CHILD: &str = env!("CARGO_BIN_EXE_actinglab-mcp-test-child");
/// The folder the made-up child logs to, and the file there that releases a held request.
const CHILD_LOG_VARIABLE: &str = "ACTINGCOMMAND_TEST_CHILD_LOG";
const RELEASE: &str = "release";
/// What the made-up child writes on stderr before it ends without an answer, and its exit code.
const CHILD_FAILURE_TEXT: &str = "neutral child failure text";
const CHILD_FAILURE_EXIT: i64 = 3;
/// The tools of three rows the children serve: one the made-up child answers at once, one it holds
/// until released, one whose request ends it; and the rows' names, as the child logs them.
const ANSWERED: &str = "ac_record_status";
const HELD: &str = "ac_watch";
const HELD_ROW: &str = "watch";
const FAILED: &str = "ac_record_draft";
const FAILED_ROW: &str = "record draft";
/// How often a bounded wait reads the made-up children's logs again.
const LOG_POLL: Duration = Duration::from_millis(20);

/// `mcp_stdio_one_row_per_kind`'s child row: `package check` of a package that is not there,
/// served through the real actinglab the CI job builds beside this program, answers the line that
/// actinglab's command line prints for it with `--json`, `isError` its non-zero exit. Its exports
/// go under `temp`.
fn child_row_from_the_real_actinglab(temp: &Path) {
    let actinglab = std::env::var_os(ACTINGLAB_VARIABLE)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| panic!("{ACTINGLAB_VARIABLE} names the actinglab the job builds"));
    let beside =
        Path::new(PROGRAM).with_file_name(format!("actinglab{}", std::env::consts::EXE_SUFFIX));
    assert_eq!(
        fs::canonicalize(&actinglab).expect("the built actinglab"),
        fs::canonicalize(beside).expect("an actinglab beside this program"),
        "the server's child is the actinglab the job built"
    );
    let package = temp.join("neutral-missing-package.json");
    let mut server = Server::start(actinglab_mcp(Path::new(PROGRAM), None, temp));
    server.initialize();
    let answer = server.call_tool(2, "ac_package_check", json!({"package": package}));
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");
    let output = Command::new(actinglab)
        .arg("--json")
        .args(["package", "check", "--package"])
        .arg(package)
        .env("TMP", temp)
        .env("TEMP", temp)
        .env_remove("ACTINGCOMMAND_DETAIL")
        .env_remove("ACTINGCOMMAND_INITIATOR")
        .env_remove(STATE_ROOT_VARIABLE)
        .stdin(Stdio::null())
        .output()
        .expect("run the command line");
    let text = String::from_utf8(output.stdout).expect("a UTF-8 line");
    let line = serde_json::from_str::<Value>(text.trim())
        .unwrap_or_else(|error| panic!("the command line printed no line ({error}): {text}"));
    let code = output
        .status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .expect("the command line's exit code");
    assert_ne!(code, 0, "setup: the package is not there: {line}");
    assert_served_as(&answer, &(line, code));
}

/// A copy of this program in `base`'s folder `bin`, the made-up child beside it as its
/// `actinglab`, and the folder the children log to.
fn beside_a_made_up_child(base: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let bin = base.join("bin");
    let log = base.join("log");
    fs::create_dir_all(&bin).expect("make the program's folder");
    fs::create_dir_all(&log).expect("make the log folder");
    let suffix = std::env::consts::EXE_SUFFIX;
    let program = bin.join(format!("actinglab-mcp{suffix}"));
    fs::copy(PROGRAM, &program).expect("copy the server");
    fs::copy(TEST_CHILD, bin.join(format!("actinglab{suffix}"))).expect("copy the made-up child");
    (program, log)
}

/// The server at `program` with `options`, its state root `state`, its exports under `temp`, its
/// children logging to `log`, and `ACTINGCOMMAND_INITIATOR` set, which the children must not
/// inherit.
fn with_children(
    program: &Path,
    log: &Path,
    state: &Path,
    temp: &Path,
    options: &[&str],
) -> Command {
    let mut command = actinglab_mcp(program, Some(state), temp);
    command
        .args(options)
        .env(CHILD_LOG_VARIABLE, log)
        .env("ACTINGCOMMAND_INITIATOR", "neutral");
    command
}

/// Every event the made-up children wrote whole under `log`, each `{event, pid, …}`.
fn child_events(log: &Path) -> Vec<Value> {
    let mut events = Vec::new();
    for entry in fs::read_dir(log).expect("read the log folder") {
        let path = entry.expect("a log entry").path();
        if path
            .extension()
            .is_none_or(|extension| extension != "jsonl")
        {
            continue;
        }
        let text = fs::read_to_string(path).expect("read a child's log");
        for line in text
            .split_inclusive('\n')
            .filter(|line| line.ends_with('\n'))
        {
            events
                .push(serde_json::from_str(line.trim()).unwrap_or_else(|error| {
                    panic!("a log line that is no JSON ({error}): {line}")
                }));
        }
    }
    events
}

/// The made-up children's events once `done` holds for them, read again every `LOG_POLL` for at
/// most `GATE_WAIT`.
fn events_once(log: &Path, what: &str, done: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    let deadline = Instant::now() + GATE_WAIT;
    loop {
        let events = child_events(log);
        if done(events.as_slice()) {
            return events;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: not within GATE_WAIT: {events:?}"
        );
        thread::sleep(LOG_POLL);
    }
}

/// The processes that logged `event`.
fn pids_with(events: &[Value], event: &str) -> std::collections::BTreeSet<u64> {
    events
        .iter()
        .filter(|logged| logged["event"] == event)
        .filter_map(|logged| logged["pid"].as_u64())
        .collect()
}

/// How many requests of the row `row` reached a child.
fn requests_of(events: &[Value], row: &str) -> usize {
    events
        .iter()
        .filter(|logged| logged["event"] == "request" && logged["op"] == row)
        .count()
}

/// The process a made-up child's answer names; the answer is no error.
#[track_caller]
fn child_pid(answer: &Value) -> u64 {
    assert_eq!(answer["result"]["isError"], json!(false), "{answer}");
    line_of(answer)["child_pid"]
        .as_u64()
        .unwrap_or_else(|| panic!("no child_pid: {answer}"))
}

#[test]
fn mcp_serve_children_pool() {
    use std::collections::BTreeSet;
    // The start options, refused as `--jobs` is.
    for (words, argument, problem) in [
        (vec!["--children", "0"], "--children", "out_of_range"),
        (vec!["--children", "65"], "--children", "out_of_range"),
        (vec!["--children", "-1"], "--children", "out_of_range"),
        (vec!["--children", "many"], "--children", "wrong_type"),
        (vec!["--children"], "--children", "value_missing"),
        (
            vec!["--children", "2", "--children", "3"],
            "--children",
            "repeated",
        ),
        (
            vec!["--list-tools", "--children", "2"],
            "--children",
            "conflicting",
        ),
        (vec!["--child-idle", "601"], "--child-idle", "out_of_range"),
        (vec!["--child-idle", "-1"], "--child-idle", "out_of_range"),
        (vec!["--child-idle", "soon"], "--child-idle", "wrong_type"),
        (vec!["--child-idle"], "--child-idle", "value_missing"),
        (
            vec!["--child-idle", "1", "--child-idle", "2"],
            "--child-idle",
            "repeated",
        ),
        (
            vec!["--list-tools", "--child-idle", "2"],
            "--child-idle",
            "conflicting",
        ),
    ] {
        let output = Command::new(PROGRAM)
            .args(&words)
            .stdin(Stdio::null())
            .output()
            .expect("run the server");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{words:?}: {stderr}");
        assert!(output.stdout.is_empty(), "{words:?} printed on stdout");
        assert!(
            stderr.contains("actingctl_usage_invalid")
                && stderr.contains(&format!("argument={argument}"))
                && stderr.contains(&format!("argument_problem={problem}")),
            "{words:?}: {stderr}"
        );
    }

    let temp = TempDir::new().expect("tempdir");
    // One child used again; two at once; a third past `--children 2` refused; the end of input.
    let base = TempDir::new().expect("tempdir");
    let state = base.path().join("state");
    let (program, log) = beside_a_made_up_child(base.path());
    let mut server = Server::start(with_children(
        &program,
        &log,
        &state,
        temp.path(),
        &["--children", "2", "--child-idle", "600"],
    ));
    server.initialize();
    let first = child_pid(&server.call_tool(2, ANSWERED, json!({})));
    let again = child_pid(&server.call_tool(3, ANSWERED, json!({})));
    assert_eq!(
        again, first,
        "the next call uses the child the first one started"
    );
    assert_eq!(
        pids_with(&child_events(&log), "started"),
        BTreeSet::from([first]),
        "one child started for both calls"
    );
    for id in [4_u64, 5] {
        server.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": HELD, "arguments": {}},
        }));
    }
    events_once(&log, "both held requests reach a child", |events| {
        requests_of(events, HELD_ROW) == 2
    });
    let refused = server.call_tool(6, ANSWERED, json!({}));
    assert_eq!(refused["result"]["isError"], json!(true), "{refused}");
    let outcome = &line_of(&refused)["outcome"];
    assert_eq!(
        (&outcome["code"], &outcome["values"]["limit_count"]),
        (&json!("job_capacity_exhausted"), &json!(2)),
        "past --children: {outcome}"
    );
    fs::write(log.join(RELEASE), b"").expect("release the held requests");
    let mut ids = BTreeSet::new();
    let mut pids = BTreeSet::new();
    for _ in 0..2 {
        let answer = server
            .next_message()
            .expect("a held call's answer before stdout ends");
        ids.insert(answer["id"].as_u64());
        pids.insert(child_pid(&answer));
    }
    assert_eq!(ids, BTreeSet::from([Some(4), Some(5)]), "the held calls");
    assert_eq!(
        pids.len(),
        2,
        "two calls at once use two children: {pids:?}"
    );
    assert!(pids.contains(&first), "one of them the idle child");
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");
    let events = events_once(&log, "every child ends at the end of input", |events| {
        pids_with(events, "ended") == pids_with(events, "started")
    });
    assert_eq!(pids_with(&events, "started"), pids, "two children in all");
    for started in events.iter().filter(|logged| logged["event"] == "started") {
        assert_eq!(
            started["arguments"],
            json!(["serve", "--stdio", "--front", "mcp"]),
            "{started}"
        );
        assert_eq!(
            started["initiator"],
            json!(false),
            "ACTINGCOMMAND_INITIATOR removed: {started}"
        );
        assert_eq!(
            started["state_root"],
            json!(state),
            "the state root variable set: {started}"
        );
    }

    // A child that ends before its answer; the next call gets a new child.
    let base = TempDir::new().expect("tempdir");
    let (program, log) = beside_a_made_up_child(base.path());
    let mut server = Server::start(with_children(
        &program,
        &log,
        &state,
        temp.path(),
        &["--children", "1", "--child-idle", "600"],
    ));
    server.initialize();
    let first = child_pid(&server.call_tool(2, ANSWERED, json!({})));
    let failed = server.call_tool(3, FAILED, json!({}));
    assert_eq!(failed["result"]["isError"], json!(true), "{failed}");
    let outcome = line_of(&failed)["outcome"].clone();
    assert_eq!(
        (&outcome["code"], &outcome["values"]["exit_code"]),
        (&json!("lab_process_failed"), &json!(CHILD_FAILURE_EXIT)),
        "{outcome}"
    );
    let tail = outcome["causes"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|cause| {
            cause["code"] == "foreign_other_error"
                && cause["values"]["raw_text"]
                    .as_str()
                    .is_some_and(|text| text.contains(CHILD_FAILURE_TEXT))
        });
    assert!(tail, "the child's stderr tail: {outcome}");
    let next = child_pid(&server.call_tool(4, ANSWERED, json!({})));
    assert_ne!(next, first, "the next call gets a new child");
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");
    let events = events_once(&log, "the new child ends at the end of input", |events| {
        pids_with(events, "ended").contains(&next)
    });
    assert_eq!(
        requests_of(&events, FAILED_ROW),
        1,
        "the request is never sent again"
    );
    assert_eq!(pids_with(&events, "started"), BTreeSet::from([first, next]));

    // A child idle for `--child-idle` ends without another call, and the server goes on.
    for idle in ["0", "1"] {
        let base = TempDir::new().expect("tempdir");
        let (program, log) = beside_a_made_up_child(base.path());
        let mut server = Server::start(with_children(
            &program,
            &log,
            &state,
            temp.path(),
            &["--child-idle", idle],
        ));
        server.initialize();
        let child = child_pid(&server.call_tool(2, ANSWERED, json!({})));
        events_once(
            &log,
            &format!("a child idle for --child-idle {idle} ends"),
            |events| pids_with(events, "ended").contains(&child),
        );
        let ping = server.request(3, "ping", json!({}));
        assert_eq!(ping["result"], json!({}), "the server goes on: {ping}");
        let status = server.finish();
        assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");
    }
}

// ---- Workflow #434 LAB-5G4b, second part: the Lab overflow rule and the client action after a
// capture, the made-up child answering each test instance with the envelope the test wrote.

/// Made-up text that puts a line far past the budget: some 20 KB, as text and again as
/// `structuredContent`, where the server's measure takes a line of about 11 KB.
const PAST_THE_BUDGET: usize = 20_000;
/// A frame part too large to keep beside an exported line: some 12 KB.
const TOO_LARGE_A_PART: usize = 12_000;
/// A failure line that the 24 KiB limit takes in `structuredContent` alone but not again as its
/// text: some 16 KB.
const FAILURE_PAST_THE_BUDGET: usize = 16_000;
/// The request ids the made-up envelopes carry.
const OBSERVE_REQ_ID: &str = "req-neutral-observe";
const DO_REQ_ID: &str = "req-neutral-do";
/// The tools of the rows of today's `ac_record_stop`, `ac_lab_observe` and `ac_lab_do`.
const RECORD_STOP: &str = "ac_record_stop";
const LAB_OBSERVE: &str = "ac_lab_observe";
const LAB_DO: &str = "ac_do";
/// The frame parts an exported `observe` or `do` line keeps, in order.
const FRAME_PARTS: [&str; 4] = [
    "frame_coordinates",
    "frame_path",
    "preview",
    "after_attachment",
];

/// `bytes` of made-up text.
fn filler(bytes: usize) -> String {
    "n".repeat(bytes)
}

/// Writes the envelope the made-up child in `log` answers a request of the instance `key` with.
fn can_answer(log: &Path, key: &str, envelope: &Value) {
    fs::write(log.join(format!("answer-{key}.json")), envelope.to_string())
        .expect("write the made-up child's answer");
}

/// The outcome of an own command's success line, as actinglab's line carries it.
fn completed() -> Value {
    let exit = cli_line::success(
        Map::new(),
        &Outcome::new(ActingctlCode::LabOperationCompleted),
    );
    exit.line.expect("the line encodes")["outcome"].clone()
}

/// The outcome of a failure line.
fn failed() -> Value {
    let exit = cli_line::failure(&Outcome::new(ActingctlCode::LabProcessFailed), None);
    exit.line.expect("the line encodes")["outcome"].clone()
}

/// The server at `program` beside the made-up child logging to `log`, its state root `state`, its
/// exports under `temp` (named by `TMPDIR` too, which is where they go elsewhere than on Windows).
fn exporting_into(program: &Path, log: &Path, state: &Path, temp: &Path) -> Command {
    let mut command = with_children(program, log, state, temp, &[]);
    command.env("TMPDIR", temp);
    command
}

/// The line of an answer that is no error.
#[track_caller]
fn answered_as(answer: &Value) -> Value {
    assert_eq!(answer["result"]["isError"], json!(false), "{answer}");
    line_of(answer)
}

/// The names of a line's members, in order.
fn members_of(line: &Value) -> Vec<String> {
    line.as_object()
        .map(|members| members.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default()
}

/// The codes of the warnings a line carries, in order.
fn warning_codes(line: &Value) -> Vec<String> {
    line["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|warning| {
            warning["outcome"]["code"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

/// The files in `folder`, none when it is not there.
fn files_in(folder: &Path) -> std::collections::BTreeSet<std::path::PathBuf> {
    fs::read_dir(folder)
        .map(|entries| {
            entries
                .map(|entry| entry.expect("an entry of the folder").path())
                .collect::<std::collections::BTreeSet<_>>()
        })
        .unwrap_or_default()
}

/// The export an overflowed `line` names holds `child_line` whole: a file in `exports`, the
/// child's line as JSON, its SHA-256 and size as the reference says; and the first warning is
/// `answer_exported` naming it.
#[track_caller]
fn assert_exported_whole(line: &Value, child_line: &Value, exports: &Path) {
    use sha2::{Digest, Sha256};
    let export = &line["export"];
    let path = export["path"]
        .as_str()
        .unwrap_or_else(|| panic!("no export path: {line}"));
    let bytes = fs::read(path).expect("read the export");
    let written = serde_json::from_slice::<Value>(&bytes).expect("the export is JSON");
    assert!(
        written == *child_line,
        "the file holds the child's line whole"
    );
    assert_eq!(
        (&export["sha256"], &export["size"]),
        (
            &json!(format!("sha256:{:x}", Sha256::digest(bytes.as_slice()))),
            &json!(bytes.len()),
        ),
        "{line}"
    );
    assert_eq!(
        fs::canonicalize(Path::new(path).parent().expect("the export's folder"))
            .expect("the export's folder"),
        fs::canonicalize(exports).expect("the export directory"),
        "written to the server's export directory"
    );
    let first = &line["warnings"][0]["outcome"];
    assert_eq!(
        (
            &first["code"],
            &first["values"]["path"],
            &first["values"]["actual_sha256"],
            &first["values"]["actual_bytes"],
        ),
        (
            &json!("answer_exported"),
            &export["path"],
            &export["sha256"],
            &export["size"],
        ),
        "{line}"
    );
}

#[test]
fn mcp_lab_overflow_rule() {
    let temp = TempDir::new().expect("tempdir");
    let base = TempDir::new().expect("tempdir");
    let state = base.path().join("state");
    let (program, log) = beside_a_made_up_child(base.path());
    let completed = completed();
    let own_warning = json!({"code": "neutral_child_warning", "values": {}});
    // `record stop`: its four binding parts under `lab`, beside far more.
    let binding = json!({
        "binding_example": {"procedure_ref": "neutral.package"},
        "binding_requires": ["neutral.prerequisite"],
        "prerequisite_entry_example": {"directory": "neutral"},
        "catalog_on_failure_example": {"action": "pause"},
    });
    let mut lab = binding.clone();
    lab["neutral_detail"] = json!(filler(PAST_THE_BUDGET));
    let stopped = json!({
        "lab": lab,
        "lab_status": "neutral",
        "warnings": [own_warning],
        "outcome": completed,
    });
    can_answer(
        &log,
        "neutral.stop",
        &json!({"exit_code": 0, "line": stopped}),
    );
    // `observe` and `do`: the four frame parts beside far more; `observe`'s preview too large.
    let framed = |preview: Value| {
        json!({
            "frame_coordinates": {"device_pixels": [2, 1], "orientation": "landscape"},
            "frame_path": "neutral-frame.png",
            "preview": preview,
            "after_attachment": {"kind": "frame", "frame_path": "neutral-after.png"},
            "neutral_detail": filler(PAST_THE_BUDGET),
            "outcome": completed,
        })
    };
    let observed = framed(json!({"path": filler(TOO_LARGE_A_PART), "scale": 1}));
    can_answer(
        &log,
        "neutral.observe",
        &json!({"exit_code": 0, "req_id": OBSERVE_REQ_ID, "line": observed}),
    );
    let acted = framed(json!({"path": "neutral-preview.jpg", "scale": 1}));
    can_answer(
        &log,
        "neutral.do",
        &json!({"exit_code": 0, "req_id": DO_REQ_ID, "line": acted}),
    );
    // A failure line past the budget.
    let refusal = json!({
        "message": "neutral failure",
        "details": {"neutral_detail": filler(FAILURE_PAST_THE_BUDGET)},
        "outcome": failed(),
    });
    can_answer(
        &log,
        "neutral.failure",
        &json!({"exit_code": 1, "line": refusal}),
    );

    let mut server = Server::start(exporting_into(&program, &log, &state, temp.path()));
    server.initialize();
    let stop = answered_as(&server.call_tool(2, RECORD_STOP, json!({"instance": "neutral.stop"})));
    let observe =
        answered_as(&server.call_tool(3, LAB_OBSERVE, json!({"instance": "neutral.observe"})));
    let act = answered_as(&server.call_tool(4, LAB_DO, json!({"instance": "neutral.do"})));
    let exports = temp.path().join("actingcommand-mcp").join("materials");
    let before = files_in(&exports);
    let failure = server.call_tool(5, RECORD_STOP, json!({"instance": "neutral.failure"}));
    let after = files_in(&exports);
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");

    // `record stop`: written whole, its binding parts kept, the child's own warning after the
    // first; no request id.
    assert_eq!(
        members_of(&stop),
        [
            "req_id",
            "export",
            "overflowed",
            "lab",
            "warnings",
            "outcome"
        ],
        "{stop}"
    );
    assert_eq!(
        (&stop["req_id"], &stop["overflowed"], &stop["lab"]),
        (&Value::Null, &json!(true), &binding),
        "{stop}"
    );
    assert_exported_whole(&stop, &stopped, &exports);
    assert_eq!(
        stop["warnings"]
            .as_array()
            .map(|warnings| warnings[1..].to_vec()),
        Some(vec![own_warning.clone()]),
        "the child's own warnings after the first: {stop}"
    );
    assert_eq!(stop["outcome"], completed, "{stop}");

    // `observe`: its frame parts in order, the preview that does not fit left out and the part
    // after it kept.
    assert_eq!(
        members_of(&observe),
        [
            "req_id",
            "export",
            "overflowed",
            "frame_coordinates",
            "frame_path",
            "after_attachment",
            "warnings",
            "outcome",
        ],
        "{observe}"
    );
    assert_eq!(observe["req_id"], json!(OBSERVE_REQ_ID), "{observe}");
    for part in ["frame_coordinates", "frame_path", "after_attachment"] {
        assert_eq!(observe[part], observed[part], "{part}: {observe}");
    }
    assert_exported_whole(&observe, &observed, &exports);
    assert_eq!(warning_codes(&observe), ["answer_exported"], "{observe}");

    // `do`: its four frame parts, in order.
    let mut kept = vec!["req_id", "export", "overflowed"];
    kept.extend(FRAME_PARTS);
    kept.extend(["warnings", "outcome"]);
    assert_eq!(members_of(&act), kept, "{act}");
    assert_eq!(act["req_id"], json!(DO_REQ_ID), "{act}");
    for part in FRAME_PARTS {
        assert_eq!(act[part], acted[part], "{part}: {act}");
    }
    assert_exported_whole(&act, &acted, &exports);

    // A failure line is not exported: sent whole in `structuredContent` alone beside a summary.
    assert_eq!(before.len(), 3, "the three lines written: {before:?}");
    assert_eq!(after, before, "no file for the failure line");
    let result = &failure["result"];
    assert_eq!(result["isError"], json!(true), "the failure is an error");
    assert!(
        result["structuredContent"] == refusal,
        "the failure line whole, not exported"
    );
    let summary = serde_json::from_str::<Value>(
        result["content"][0]["text"]
            .as_str()
            .expect("the failure's text"),
    )
    .expect("the summary is JSON");
    assert!(
        summary.get("structured_content_only").is_some() && summary.get("overflowed").is_none(),
        "a summary beside it: {summary}"
    );

    // The export's failure: a file where the export directory would be.
    let blocked = TempDir::new().expect("tempdir");
    fs::write(blocked.path().join("actingcommand-mcp"), b"neutral")
        .expect("block the export directory");
    let mut server = Server::start(exporting_into(&program, &log, &state, blocked.path()));
    server.initialize();
    let unwritten =
        answered_as(&server.call_tool(2, RECORD_STOP, json!({"instance": "neutral.stop"})));
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");
    assert_eq!(
        members_of(&unwritten),
        ["req_id", "overflowed", "lab", "warnings", "outcome"],
        "no export: {unwritten}"
    );
    assert_eq!(
        (&unwritten["overflowed"], &unwritten["lab"]),
        (&json!(true), &binding),
        "{unwritten}"
    );
    assert_eq!(
        (
            &unwritten["warnings"][0]["outcome"]["code"],
            &unwritten["warnings"][1],
            unwritten["warnings"].as_array().map(Vec::len),
        ),
        (&json!("material_export_failed"), &own_warning, Some(2)),
        "the export's failure first, the child's own after it: {unwritten}"
    );
}

/// A capture's own connection whose client action the Runtime refuses: the identity card
/// accepted, the client action denied.
fn refused_recording() -> ScriptedConnection {
    ScriptedConnection::new()
        .request(|request| match request.operation() {
            RuntimeOperation::DeclareGovernanceIdentity { .. } => {
                ScriptedAnswer::completed(request, RuntimeResult::GovernanceIdentityAccepted)
            }
            other => panic!("the identity card comes first, not {other:?}"),
        })
        .request(|request| {
            assert!(
                matches!(
                    request.operation(),
                    RuntimeOperation::RecordClientAction { .. }
                ),
                "the client action comes second, not {:?}",
                request.operation()
            );
            denied(request)
        })
}

#[test]
fn mcp_capture_records_client_action() {
    let state = TempDir::new().expect("tempdir");
    let temp = TempDir::new().expect("tempdir");
    let base = TempDir::new().expect("tempdir");
    let (program, log) = beside_a_made_up_child(base.path());
    let completed = completed();
    let observed = json!({"neutral_frame": "observed", "outcome": completed});
    let acted = json!({"neutral_action": "acted", "outcome": completed});
    can_answer(
        &log,
        "neutral.observe",
        &json!({"exit_code": 0, "req_id": OBSERVE_REQ_ID, "line": observed}),
    );
    can_answer(
        &log,
        "neutral.unnamed",
        &json!({"exit_code": 0, "line": acted}),
    );
    can_answer(
        &log,
        "neutral.do",
        &json!({"exit_code": 0, "req_id": DO_REQ_ID, "line": acted}),
    );
    // One connection per capture: recorded, recorded, refused, recorded.
    let runtime = ScriptedRuntime::start(state.path(), |_| {
        vec![
            provenance(),
            provenance(),
            refused_recording(),
            provenance(),
        ]
    });
    let mut server = Server::start(with_children(
        &program,
        &log,
        state.path(),
        temp.path(),
        &[],
    ));
    server.initialize();
    let with_id = server.call_tool(
        2,
        LAB_OBSERVE,
        json!({"instance": "neutral.observe", "capture": true}),
    );
    let without_id = server.call_tool(
        3,
        LAB_DO,
        json!({"instance": "neutral.unnamed", "capture": true}),
    );
    let not_recorded = server.call_tool(
        4,
        LAB_DO,
        json!({"instance": "neutral.do", "capture": true}),
    );
    let absent = server.call_tool(5, LAB_OBSERVE, json!({"instance": "neutral.observe"}));
    let off = server.call_tool(
        6,
        LAB_DO,
        json!({"instance": "neutral.unnamed", "capture": false}),
    );
    let planned = server.call_tool(
        7,
        LAB_DO,
        json!({"instance": "neutral.do", "capture": true, "dry_run": true}),
    );
    let status = server.finish();
    assert_eq!(status.code(), Some(0), "exit at stdin EOF: {status}");

    // The answers: the child's line; the warnings of the recording after its own.
    assert_eq!(answered_as(&with_id), observed, "recorded: no warning");
    let line = answered_as(&without_id);
    assert_eq!(line["neutral_action"], acted["neutral_action"], "{line}");
    assert_eq!(
        warning_codes(&line),
        ["client_action_without_req_id"],
        "{line}"
    );
    let line = answered_as(&not_recorded);
    assert_eq!(
        (&line["neutral_action"], &line["outcome"]),
        (&acted["neutral_action"], &completed),
        "the call's answer stands: {line}"
    );
    assert_eq!(
        warning_codes(&line),
        ["client_action_not_recorded"],
        "{line}"
    );
    assert!(
        line["warnings"][0]["outcome"]["causes"]
            .as_array()
            .is_some_and(|causes| !causes.is_empty()),
        "caused by the recording's failure: {line}"
    );
    assert_eq!(answered_as(&absent), observed, "capture absent");
    assert_eq!(answered_as(&off), acted, "capture false");
    assert_eq!(answered_as(&planned), acted, "capture with dry_run");

    // The scripted Runtime: one connection of its own per capture that a child answered, carrying
    // the identity card and the client action, nothing else.
    let record = runtime.finish();
    assert!(
        record.unscripted.is_empty(),
        "a recording past the four captures: {record:?}"
    );
    assert_eq!(
        record.connections.len(),
        4,
        "one connection per capture a child answered, the dry-run's too: {record:?}"
    );
    let expected = [
        (LAB_OBSERVE, "neutral.observe", Some(OBSERVE_REQ_ID)),
        (LAB_DO, "neutral.unnamed", None),
        (LAB_DO, "neutral.do", Some(DO_REQ_ID)),
        (LAB_DO, "neutral.do", Some(DO_REQ_ID)),
    ];
    for (connection, (tool, instance, req_id)) in record.connections.iter().zip(expected) {
        assert_eq!(
            operations(connection),
            [
                "health",
                "bind_channel",
                "declare_governance_identity",
                "record_client_action",
            ],
            "{tool} {instance}: a connection of its own"
        );
        let request = sent(&connection.requests[3]);
        let action = &request["operation"]["action"];
        assert_eq!(
            (
                &action["surface_id"],
                &action["control_id"],
                &action["kind"],
                &action["instance_alias"],
            ),
            (
                &json!("mcp"),
                &json!(tool),
                &json!("command"),
                &json!(instance)
            ),
            "{action}"
        );
        let value = req_id.map_or(
            Value::Null,
            |id| json!({"type": "path_safe_string", "value": id}),
        );
        assert_eq!(action["value"], value, "the envelope's req_id: {action}");
        assert_eq!(
            (&request["actor"], &request["source"]),
            (&json!(EventActor::Cli), &json!(EventSource::Cli)),
            "{request}"
        );
    }
    let events = child_events(&log);
    assert_eq!(
        (requests_of(&events, "observe"), requests_of(&events, "do")),
        (2, 4),
        "each Lab command reached the child once"
    );
}
