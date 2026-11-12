// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5G2a (register §1.287, "Slice LAB-5G2a"): the real `actinglab-mcp` over
//! pipes, as today's server's own protocol tests drive `actingctl mcp-serve`
//! (`apps/actingctl/tests/gate/mcp_harness.rs`, `mcp_shell.rs`; neither moved nor edited). The
//! program runs from the build's folder, outside any installation root, so its default list holds
//! every module this platform offers. No Runtime and no file: every read returns as soon as its
//! message arrives; `GATE_WAIT` only bounds a broken run.
//!
//! - `mcp_tier_refused`: `--tier` refuses the start with `actingctl_usage_invalid` naming
//!   `--tier`, a developer line that says the server has no tiers, nothing on stdout and exit 1.
//! - `mcp_calls_refused_unsupported_and_an_unknown_tool`: a call of a listed tool answers the
//!   refusal `mcp_operation_unsupported` with the tool, as the CLI line `{outcome}`, text and
//!   structuredContent, `isError` true; so does a tool `--list` leaves out; `ac_observe` and a name
//!   no row has answer what today's server answers for an unknown tool (-32602
//!   `mcp_params_invalid`, `name`, `not_recognised`), and arguments that are no object -32602.
//! - `mcp_loop_initialize_list_batch_and_cancel`: `initialize` negotiates as today's server and
//!   carries the instructions; `tools/list` answers `--list-tools`' list, in either era; under
//!   2025-03-26 a batch is one array of its answers, under a later version -32600, an empty one
//!   -32600 always; a cancellation of a request not in flight, or of a call already answered,
//!   answers nothing and the server goes on; `actingcommand/entryStatus` holds nothing; at stdin
//!   EOF the server exits 0.

use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

/// Bounds a broken run only.
const GATE_WAIT: Duration = Duration::from_secs(120);
const PROGRAM: &str = env!("CARGO_BIN_EXE_actinglab-mcp");
const MODERN: &str = "2026-07-28";
const INVALID_REQUEST: i64 = -32600;
const INVALID_PARAMS: i64 = -32602;
const UNSUPPORTED: &str = "mcp_operation_unsupported";

/// `actinglab-mcp` over pipes. Dropped, it is killed if still running.
struct Server {
    child: Option<Child>,
    input: Option<ChildStdin>,
    lines: Receiver<Result<String, String>>,
}

impl Server {
    fn spawn(arguments: &[&str]) -> Self {
        let mut child = Command::new(PROGRAM)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn actinglab-mcp");
        let input = child.stdin.take().expect("actinglab-mcp stdin");
        let output = child.stdout.take().expect("actinglab-mcp stdout");
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
        let input = self.input.as_mut().expect("actinglab-mcp stdin is open");
        writeln!(input, "{message}").expect("write a line to actinglab-mcp");
        input.flush().expect("flush actinglab-mcp stdin");
    }

    /// The next message, or `None` once stdout ended.
    fn next_message(&mut self) -> Option<Value> {
        let line = match self.lines.recv_timeout(GATE_WAIT) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => panic!("cannot read actinglab-mcp stdout: {error}"),
            Err(RecvTimeoutError::Disconnected) => return None,
            Err(RecvTimeoutError::Timeout) => {
                panic!("actinglab-mcp wrote nothing within GATE_WAIT")
            }
        };
        Some(serde_json::from_str(&line).unwrap_or_else(|error| {
            panic!("actinglab-mcp wrote a line that is not JSON ({error}): {line}")
        }))
    }

    fn recv(&mut self) -> Value {
        self.next_message()
            .unwrap_or_else(|| panic!("actinglab-mcp stdout ended before the next message"))
    }

    /// Sends one request and returns its answer, the next message.
    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let answer = self.recv();
        assert_eq!(
            answer["id"],
            json!(id),
            "{method}: the next message is not its answer"
        );
        answer
    }

    /// A legacy `initialize` asking for `version`, then `notifications/initialized`.
    fn initialize(&mut self, id: u64, version: &str) -> Value {
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

    fn call_tool(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        )
    }

    /// Closes stdin, reads stdout to its end and returns every message still unread, then the
    /// exit status of the process.
    fn finish(mut self) -> (Vec<Value>, ExitStatus) {
        drop(self.input.take());
        let mut messages = Vec::new();
        while let Some(message) = self.next_message() {
            messages.push(message);
        }
        let mut child = self.child.take().expect("actinglab-mcp child");
        let (sender, exited) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(child.wait().map_err(|error| error.to_string()));
        });
        let status = match exited.recv_timeout(GATE_WAIT) {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => panic!("cannot wait for actinglab-mcp: {error}"),
            Err(error) => panic!("actinglab-mcp did not exit after its stdout ended: {error}"),
        };
        (messages, status)
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

/// A JSON-RPC error answer's integer code and its outcome, whose code is also its `message`.
#[track_caller]
fn rpc_error(answer: &Value) -> (i64, &Value) {
    let error = answer
        .get("error")
        .unwrap_or_else(|| panic!("not a JSON-RPC error answer: {answer}"));
    let code = error["code"]
        .as_i64()
        .unwrap_or_else(|| panic!("a JSON-RPC error without an integer code: {answer}"));
    let outcome = &error["data"]["outcome"];
    assert_eq!(
        error["message"], outcome["code"],
        "a JSON-RPC error whose message is not its outcome code: {answer}"
    );
    (code, outcome)
}

/// A `tools/call` refusal: the CLI line `{outcome}` as text and, when `structured`, as
/// structuredContent, with `isError` true; its outcome.
#[track_caller]
fn refusal(answer: &Value, structured: bool) -> Value {
    let result = &answer["result"];
    assert_eq!(result["isError"], json!(true), "not a refusal: {answer}");
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a refusal without its text: {answer}"));
    let line: Value = serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("the text is not JSON ({error}): {answer}"));
    let members = line
        .as_object()
        .map(|members| members.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    assert_eq!(
        members,
        ["outcome"],
        "the line of a refusal is {{outcome}}: {answer}"
    );
    assert_eq!(
        result.get("structuredContent"),
        structured.then_some(&line),
        "structuredContent repeats the line where the version has it: {answer}"
    );
    line["outcome"].clone()
}

/// The refusal of this build, which serves no call, for the tool `tool`.
#[track_caller]
fn assert_unsupported(outcome: &Value, tool: &str) {
    assert_eq!(outcome["code"], UNSUPPORTED, "{outcome}");
    assert_eq!(outcome["category"], "error", "{outcome}");
    assert_eq!(outcome["values"]["mcp_tool"], tool, "{outcome}");
}

/// `params._meta` of a 2026-07-28 request.
fn modern_meta() -> Value {
    let mut meta = Map::new();
    meta.insert(
        "io.modelcontextprotocol/protocolVersion".to_owned(),
        json!(MODERN),
    );
    meta.insert(
        "io.modelcontextprotocol/clientCapabilities".to_owned(),
        json!({}),
    );
    Value::Object(meta)
}

/// What `actinglab-mcp <arguments>` prints and how it ends, stdin closed.
fn run(arguments: &[&str]) -> (String, String, Option<i32>) {
    let output = Command::new(PROGRAM)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("run actinglab-mcp");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code(),
    )
}

/// The tools `actinglab-mcp --list-tools <arguments>` prints.
fn printed_tools(arguments: &[&str]) -> Value {
    let mut words = vec!["--list-tools"];
    words.extend_from_slice(arguments);
    let (stdout, stderr, code) = run(&words);
    assert_eq!(code, Some(0), "--list-tools {arguments:?}: {stderr}");
    let printed: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("--list-tools printed no JSON ({error}): {stdout}"));
    printed["tools"].clone()
}

fn tool_names(tools: &Value) -> Vec<String> {
    tools
        .as_array()
        .unwrap_or_else(|| panic!("no tool list: {tools}"))
        .iter()
        .map(|tool| {
            tool["name"]
                .as_str()
                .unwrap_or_else(|| panic!("a tool without a name: {tool}"))
                .to_owned()
        })
        .collect()
}

#[test]
fn mcp_tier_refused() {
    for arguments in [vec!["--tier", "observer"], vec!["--tier=observer,operator"]] {
        let (stdout, stderr, code) = run(&arguments);
        assert_eq!(code, Some(1), "{arguments:?}: {stderr}");
        assert!(
            stdout.is_empty(),
            "{arguments:?} printed on stdout: {stdout}"
        );
        assert!(
            stderr.contains("actingctl_usage_invalid")
                && stderr.contains("argument=--tier")
                && stderr.contains("argument_problem=unknown")
                && stderr.contains("has no tiers"),
            "{arguments:?}: {stderr}"
        );
    }
}

#[test]
fn mcp_calls_refused_unsupported_and_an_unknown_tool() {
    let mut server = Server::spawn(&[]);
    server.initialize(1, "2025-11-25");
    let refused = server.call_tool(2, "ac_status", json!({}));
    assert_unsupported(&refusal(&refused, true), "ac_status");
    let refused = server.call_tool(3, "ac_frame", json!({"instance": "neutral.shell"}));
    assert_unsupported(&refusal(&refused, true), "ac_frame");
    for (id, name) in [(4, "ac_observe"), (5, "ac_no_such_tool")] {
        let answer = server.call_tool(id, name, json!({}));
        let (code, outcome) = rpc_error(&answer);
        assert_eq!(code, INVALID_PARAMS, "{name}: {answer}");
        assert_eq!(
            (
                &outcome["code"],
                &outcome["values"]["argument"],
                &outcome["values"]["argument_problem"],
            ),
            (
                &json!("mcp_params_invalid"),
                &json!("name"),
                &json!("not_recognised"),
            ),
            "{name}: {answer}"
        );
    }
    let answer = server.request(
        6,
        "tools/call",
        json!({"name": "ac_status", "arguments": "not an object"}),
    );
    let (code, outcome) = rpc_error(&answer);
    assert_eq!(code, INVALID_PARAMS, "{answer}");
    assert_eq!(outcome["values"]["argument"], "arguments", "{answer}");
    let (_, status) = server.finish();
    assert_eq!(status.code(), Some(0), "exit status at stdin EOF: {status}");

    // A tool --list leaves out is called as a listed one: not listed is never refused.
    let core = printed_tools(&["--list", "core"]);
    assert!(
        tool_names(&core).contains(&"ac_status".to_owned())
            && !tool_names(&core).contains(&"ac_record_start".to_owned()),
        "--list core lists the core tools only: {core}"
    );
    let mut server = Server::spawn(&["--list", "core"]);
    server.initialize(1, "2025-11-25");
    let listed = server.request(2, "tools/list", json!({}));
    assert_eq!(
        listed["result"]["tools"], core,
        "tools/list is --list-tools'"
    );
    let refused = server.call_tool(3, "ac_record_start", json!({}));
    assert_unsupported(&refusal(&refused, true), "ac_record_start");
}

#[test]
fn mcp_loop_initialize_list_batch_and_cancel() {
    let every = printed_tools(&[]);
    let names = tool_names(&every);
    assert!(
        ["ac_status", "ac_frame", "ac_lab_observe"]
            .iter()
            .all(|name| names.contains(&(*name).to_owned()))
            && !names.contains(&"ac_observe".to_owned()),
        "the default list outside any root: {names:?}"
    );
    for tool in every.as_array().into_iter().flatten() {
        assert!(
            tool.get("title").is_none() && tool.get("outputSchema").is_none(),
            "no title and no outputSchema: {tool}"
        );
    }

    // initialize, tools/list in both eras, entryStatus, cancellations.
    let mut server = Server::spawn(&[]);
    for (requested, negotiated) in [("2025-06-18", "2025-06-18"), ("2024-11-05", "2025-11-25")] {
        let answer = server.initialize(1, requested);
        let result = &answer["result"];
        assert_eq!(result["protocolVersion"], negotiated, "{answer}");
        assert_eq!(
            result["capabilities"]["tools"]["listChanged"], false,
            "{answer}"
        );
        assert_eq!(result["serverInfo"]["name"], "actinglab-mcp", "{answer}");
        let instructions = result["instructions"].as_str().unwrap_or_default();
        assert!(
            instructions.contains("ac_frame") && instructions.contains("ac_lab_observe"),
            "the instructions name the two exceptions: {answer}"
        );
    }
    let legacy = server.request(2, "tools/list", json!({}));
    assert_eq!(
        legacy["result"],
        json!({"tools": every}),
        "a legacy tools/list"
    );
    let modern = server.request(3, "tools/list", json!({"_meta": modern_meta()}));
    let result = &modern["result"];
    assert_eq!(result["resultType"], "complete", "{modern}");
    assert_eq!(result["tools"], every, "a modern tools/list");
    assert_eq!(
        (&result["ttlMs"], &result["cacheScope"]),
        (&json!(0), &json!("private"))
    );
    let status = server.request(4, "actingcommand/entryStatus", json!({}));
    assert_eq!(
        status["result"],
        json!({"outstanding_handles": 0, "kinds": [], "handles": [], "calls_in_flight": 0}),
        "{status}"
    );
    // A cancellation of a request not in flight answers nothing: the next answer is the ping's.
    server.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": {"requestId": 99, "reason": "gate"},
    }));
    let ping = server.request(5, "ping", json!({}));
    assert_eq!(ping["result"], json!({}), "{ping}");
    // A cancellation of a call already answered changes nothing.
    let refused = server.call_tool(6, "ac_status", json!({}));
    assert_unsupported(&refusal(&refused, true), "ac_status");
    server.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": {"requestId": 6},
    }));
    let ping = server.request(7, "ping", json!({}));
    assert_eq!(ping["result"], json!({}), "{ping}");
    let (left, exited) = server.finish();
    assert!(
        left.is_empty(),
        "nothing is left unanswered or sent twice: {left:?}"
    );
    assert_eq!(exited.code(), Some(0), "exit status at stdin EOF: {exited}");

    // A batch: one array under 2025-03-26, -32600 under a later version, an empty one always.
    let batch = json!([
        {"jsonrpc": "2.0", "id": 2, "method": "ping"},
        {
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "ac_status", "arguments": {}},
        },
    ]);
    let mut server = Server::spawn(&[]);
    server.initialize(1, "2025-03-26");
    server.send(&batch);
    let answer = server.recv();
    let answers = answer
        .as_array()
        .unwrap_or_else(|| panic!("a 2025-03-26 batch is not answered by one array: {answer}"));
    assert_eq!(
        answers
            .iter()
            .map(|answered| answered["id"].as_u64())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([Some(2), Some(3)]),
        "{answer}"
    );
    for answered in answers {
        if answered["id"] == json!(3) {
            assert_unsupported(&refusal(answered, false), "ac_status");
        } else {
            assert_eq!(answered["result"], json!({}), "{answer}");
        }
    }
    server.send(&json!([]));
    let empty = server.recv();
    let (code, outcome) = rpc_error(&empty);
    assert_eq!(
        (code, &outcome["code"]),
        (INVALID_REQUEST, &json!("mcp_batch_empty"))
    );
    drop(server);

    let mut server = Server::spawn(&[]);
    server.initialize(1, "2025-11-25");
    server.send(&batch);
    let refused = server.recv();
    let (code, outcome) = rpc_error(&refused);
    assert_eq!(
        (code, &outcome["code"]),
        (INVALID_REQUEST, &json!("mcp_batch_unsupported"))
    );
}
