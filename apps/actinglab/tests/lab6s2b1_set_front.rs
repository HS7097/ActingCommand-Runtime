// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-6S2-B1, fix 1 (register §1.287, "Slice LAB-6S2-B1";
//! contracts/lab-two-entries.md §4.2 and §8.2): the two places that say, once per process, which
//! front actinglab's core rows connect as, the two calls of `cli_module::set_front`, each in a
//! process of its own, since the front is set once per process; the real binary, as a person or a
//! front end starts it:
//! - `lab6s2b1_a_command_line_binds_the_core_rows_lab_with_lab_cli`: a command line of the CLI
//!   module (`cli_module::main`, `set_front(None)`), `actinglab --json resume` without the
//!   initiator's variable, binds `lab` with `Lab-CLI` and no `via`;
//! - `lab6s2b1_serve_front_mcp_binds_the_core_rows_lab_with_lab_mcp`: `serve --stdio --front mcp`
//!   (`serve::main`, `set_front(front)` after the option is read), the core row `resume` on its
//!   input, binds `lab` with `Lab-MCP` and no `via`.
//!
//! Each connection is `Health`, `BindChannel` and `ResumeScheduling`, every request with the core
//! rows' envelope of 0.12.58, (Lab, Lab). The Runtime is the runtime client's scripted one, found
//! through the state root the child is given; no clock is read and nothing waits on time.

use actingcommand_contract::{
    DeclaredInitiator, EventActor, EventSource, Initiator, RuntimeOperation, RuntimeRequest,
    RuntimeResult, SchedulingPauseScope,
};
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedConnectionRecord, ScriptedRuntime,
};
use actingcommand_runtime_client::{ChannelVia, RuntimeChannel};
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

/// A binding as a `BindChannel` declares it: the channel, `via` and the word.
type Declared = (RuntimeChannel, Option<ChannelVia>, Option<Initiator>);

/// The scripted answer to the core row `resume`: every pause lifted.
fn resumed(request: &RuntimeRequest) -> ScriptedAnswer {
    ScriptedAnswer::completed(
        request,
        RuntimeResult::SchedulingResumed {
            scope: SchedulingPauseScope::Global,
            revision: 2,
            selfcheck: None,
        },
    )
}

/// A scripted Runtime on `state_root` that answers one connection's `resume`.
fn runtime_for_one_resume(state_root: &Path) -> ScriptedRuntime {
    ScriptedRuntime::start(state_root, |_| {
        vec![ScriptedConnection::new().request(resumed)]
    })
}

/// actinglab with the scripted Runtime's state root, its environment without an initiator, the
/// detached run's marker, an outcome detail or an installation's inherited selection.
fn actinglab(state_root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_actinglab"));
    command
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", state_root)
        .env_remove("ACTINGCOMMAND_INITIATOR")
        .env_remove("ACTINGCOMMAND_DETACHED_RUN")
        .env_remove("ACTINGCOMMAND_DETAIL")
        .env_remove("ACTINGCOMMAND_INSTALL_ROOT")
        .env_remove("ACTINGCOMMAND_INSTALL_SELECTION");
    command
}

/// The one JSON line `output` printed on stdout.
fn the_line(output: &Output) -> Value {
    let stdout = String::from_utf8(output.stdout.clone()).expect("UTF-8");
    let lines = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        1,
        "one line: {stdout}; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(lines[0]).expect("a JSON line")
}

/// The binding the received `request`, a `BindChannel`, declares.
fn declared(request: &RuntimeRequest) -> Declared {
    let RuntimeOperation::BindChannel {
        channel,
        via,
        initiator,
    } = request.operation()
    else {
        panic!("BindChannel, got {:?}", request.operation());
    };
    (
        *channel,
        *via,
        initiator.as_ref().and_then(DeclaredInitiator::initiator),
    )
}

/// Asserts that the scripted Runtime received one connection, `Health`, `BindChannel` declaring
/// `lab` with `word` and no `via`, and the core row's `ResumeScheduling`, every request with the
/// core rows' envelope (Lab, Lab).
fn assert_bound_lab_with(runtime: ScriptedRuntime, word: Initiator) {
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    assert_eq!(record.connections.len(), 1);
    let connection: &ScriptedConnectionRecord = &record.connections[0];
    let operations = connection
        .requests
        .iter()
        .map(|request| {
            json!(request.operation())["operation"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(operations, ["health", "bind_channel", "resume_scheduling"]);
    assert_eq!(
        declared(&connection.requests[1]),
        (RuntimeChannel::Lab, None, Some(word))
    );
    for request in &connection.requests {
        assert_eq!(
            (request.actor(), request.source()),
            (EventActor::Lab, EventSource::Lab),
            "{:?}",
            request.operation()
        );
    }
}

#[test]
fn lab6s2b1_a_command_line_binds_the_core_rows_lab_with_lab_cli() {
    let temp = TempDir::new().expect("state root");
    let runtime = runtime_for_one_resume(temp.path());
    let output = actinglab(temp.path())
        .args(["--json", "resume"])
        .stdin(Stdio::null())
        .output()
        .expect("actinglab runs");
    let line = the_line(&output);
    assert_eq!(
        line["outcome"]["code"],
        json!("runtime_scheduling_resumed"),
        "{line}"
    );
    assert_bound_lab_with(runtime, Initiator::LabCli);
}

#[test]
fn lab6s2b1_serve_front_mcp_binds_the_core_rows_lab_with_lab_mcp() {
    let temp = TempDir::new().expect("state root");
    let runtime = runtime_for_one_resume(temp.path());
    let mut child = actinglab(temp.path())
        .args(["serve", "--stdio", "--front", "mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("serve starts");
    let mut input = child.stdin.take().expect("its input");
    writeln!(input, "{}", json!({"op": "resume", "params": {}})).expect("a request line");
    drop(input);
    let output = child.wait_with_output().expect("serve ends");
    assert!(
        output.status.success(),
        "serve ends at the end of its input: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Under `--front mcp` the answer is the envelope `{exit_code, line}` (LAB-5G4b).
    let answer = the_line(&output);
    assert_eq!(
        answer["line"]["outcome"]["code"],
        json!("runtime_scheduling_resumed"),
        "{answer}"
    );
    assert_bound_lab_with(runtime, Initiator::LabMcp);
}
