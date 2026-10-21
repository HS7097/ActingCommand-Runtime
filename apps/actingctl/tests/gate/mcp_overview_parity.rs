// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #389 MCP-8 (MCP-0 command table v1.1 §2.3, §4.9): `actingctl overview` and MCP
// `ac_overview`. Both sides are the real binaries: the MCP server over pipes (fixture I5,
// `mcp_harness`) and the CLI as one process per call, on one c4 fake Runtime child
// (`tests/support/c4_runtime.rs`) with one instance, paused.
//
// The parity test: for the same arguments the CLI's stdout JSON without its `outcome` and the
// ac_overview result are the same object, apart from the start of the run window, which each
// read takes from its own clock. Every answer fits the output budget, so §2.2 leaves no other
// difference and nothing is marked truncated. Without `sections` every block is answered
// (daemon, instances, run_window, lab_tool, install, Workflow #385 P-m's performance, whose
// read time is its own like the run window's start, Workflow #389 MCP-9a's catalog,
// config_generation and watchdog, and Workflow #384 D2-S5's fact_refresh); with `sections` only
// those named and the daemon. The fake
// Runtime has no active policy catalog and no install root: catalog and watchdog are null and
// config_generation names neither generation. The instance is named by its alias and by its
// instance_id (D4). Workflow #378 A5: the
// CLI line carries `runtime_overview_reported` beside the overview.
//
// The refusal test: a section name §4.9 does not have is refused before any read, by the CLI as
// `actingctl_usage_invalid` naming `--sections`, by MCP as `arguments_invalid` naming `sections`,
// both with `argument_problem` `not_recognised`.
//
// The foreground test (Workflow #389 MCP-5b) is described at its function.

use crate::c4_runtime::{RuntimeChild, neutral_foreground, write_sealed_frame};
use crate::mcp_harness::{McpServer, accepted};
use actingcommand_contract::{
    APPLICATION_FOREGROUND_FACT_KEY, EventActor, EventSource, FactValue, InputAction,
    RuntimeFactScope,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const CHILD_ENTRY: &str = "c4_runtime_child_process";
const ALIAS: &str = "neutral.overview";
// A block name §4.9 does not have.
const UNKNOWN_SECTION: &str = "neutral_block";

#[test]
fn gate_actingctl_overview_and_ac_overview_answer_the_same_overview_for_the_same_arguments() {
    let root = TempDir::new().expect("tempdir");
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    // An instance pause: the row carries it.
    let (exited, line) = actingctl(root.path(), &["pause", "--instance", ALIAS]);
    assert!(
        exited && line["outcome"]["code"] == json!("runtime_scheduling_paused"),
        "setup: actingctl pause: {line}"
    );
    let state_root = root.path().display().to_string();
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let mut id = 1;

    // Every block.
    let (cli, tool) = parity(&mut mcp, &mut id, root.path(), &["overview"], json!({}));
    for (side, answer) in [("CLI", &cli), ("ac_overview", &tool)] {
        assert_eq!(
            members(answer),
            BTreeSet::from([
                "catalog",
                "config_generation",
                "daemon",
                "fact_refresh",
                "install",
                "instances",
                "lab_tool",
                "performance",
                "run_window",
                "watchdog",
            ]),
            "{side}: {answer}"
        );
        assert_daemon(side, answer);
        assert_rows(side, answer);
        assert!(
            answer["run_window"]["since_unix_ms"].is_u64(),
            "{side}: no run window start: {answer}"
        );
        assert_eq!(
            (
                &answer["catalog"],
                &answer["config_generation"],
                &answer["watchdog"]
            ),
            (&Value::Null, &json!({}), &Value::Null),
            "{side}: {answer}"
        );
        assert_eq!(answer["lab_tool"], json!({"present": false}), "{side}");
        assert_eq!(
            answer["install"],
            json!({"state_root": state_root}),
            "{side}"
        );
    }

    // One instance by its instance_id, and two blocks.
    let instance_id = cli["instances"][0]["instance_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the overview names no instance_id: {cli}"))
        .to_owned();
    let (cli, tool) = parity(
        &mut mcp,
        &mut id,
        root.path(),
        &[
            "overview",
            "--instance",
            &instance_id,
            "--sections",
            "instances,install",
        ],
        json!({"instance": instance_id, "sections": ["instances", "install"]}),
    );
    for (side, answer) in [("CLI", &cli), ("ac_overview", &tool)] {
        assert_eq!(
            members(answer),
            BTreeSet::from(["daemon", "install", "instances"]),
            "{side}: {answer}"
        );
        assert_daemon(side, answer);
        assert_rows(side, answer);
        assert_eq!(
            answer["install"],
            json!({"state_root": state_root}),
            "{side}"
        );
    }

    // The instance by its alias, and the daemon alone with the run window.
    let (cli, tool) = parity(
        &mut mcp,
        &mut id,
        root.path(),
        &["overview", "--instance", ALIAS, "--sections", "run_window"],
        json!({"instance": ALIAS, "sections": ["run_window"]}),
    );
    for (side, answer) in [("CLI", &cli), ("ac_overview", &tool)] {
        assert_eq!(
            members(answer),
            BTreeSet::from(["daemon", "run_window"]),
            "{side}: {answer}"
        );
        assert_daemon(side, answer);
    }

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

#[test]
fn gate_overview_and_ac_overview_refuse_an_unknown_section_naming_it() {
    let root = TempDir::new().expect("tempdir");
    let sections = format!("instances,{UNKNOWN_SECTION}");
    let (exited, line) = actingctl(root.path(), &["overview", "--sections", &sections]);
    assert!(!exited, "actingctl overview exited 0: {line}");
    let outcome = &line["outcome"];
    assert_eq!(
        (
            &outcome["code"],
            &outcome["values"]["argument"],
            &outcome["values"]["argument_problem"]
        ),
        (
            &json!("actingctl_usage_invalid"),
            &json!("--sections"),
            &json!("not_recognised")
        ),
        "actingctl overview: {line}"
    );

    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let answer = mcp.call_tool(
        2,
        "ac_overview",
        json!({"sections": ["instances", UNKNOWN_SECTION]}),
    );
    let error = &answer["result"]["structuredContent"]["error"];
    assert_eq!(
        (
            &answer["result"]["isError"],
            &error["class"],
            &error["outcome"]["code"],
            &error["outcome"]["values"]["argument"],
            &error["outcome"]["values"]["argument_problem"]
        ),
        (
            &json!(true),
            &json!("usage"),
            &json!("arguments_invalid"),
            &json!("sections"),
            &json!("not_recognised")
        ),
        "ac_overview: {answer}"
    );
    mcp.close_input();
    drop(mcp);
}

// Workflow #389 MCP-5b (MCP-0 command table v1.1 §2.3, §4.6, D10, ruling Q7): each instance row
// carries `foreground {package, observed_at_unix_ms}`, the instance program fact
// `application.foreground` the foreground gate records before a pointer input, so the package
// last seen, not a live read. Before any pointer input the instance has no such fact and its row
// no `foreground`, on both sides and without a warning; after one tap the row carries the
// package the fake device reports (`neutral_foreground`) and the time the fact was observed. For
// the same arguments the CLI's stdout JSON and the ac_overview result are the same object, apart
// from the start of the run window.
#[test]
fn gate_actingctl_overview_and_ac_overview_answer_the_same_foreground_the_instance_last_reported() {
    let root = TempDir::new().expect("tempdir");
    write_sealed_frame(&root.path().join("sealed.png"));
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let mut id = 1;
    let words = ["overview", "--instance", ALIAS, "--sections", "instances"];
    let arguments = json!({"instance": ALIAS, "sections": ["instances"]});

    // No pointer input yet: no fact, no block.
    let (cli, tool) = parity(&mut mcp, &mut id, root.path(), &words, arguments.clone());
    for (side, answer) in [("CLI", &cli), ("ac_overview", &tool)] {
        assert!(
            answer["instances"][0].get("foreground").is_none(),
            "{side}: a row carries a foreground no input has seen: {answer}"
        );
    }

    // One tap through the foreground gate.
    let holder = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect a Runtime client");
    let token = holder.acquire_lease(ALIAS).expect("hold the instance");
    let tapped = holder.input(&token, InputAction::Tap { x: 1, y: 1 });
    holder.release_lease(&token).expect("release the instance");
    let instance_id = holder
        .status()
        .expect("read the Runtime status")
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == ALIAS)
        .expect("setup: the fake instance is not registered")
        .instance_id();
    let observed_at_unix_ms = holder
        .runtime_fact_snapshot()
        .expect("read the program facts")
        .records
        .iter()
        .find(|record| {
            record.key == APPLICATION_FOREGROUND_FACT_KEY
                && record.scope == RuntimeFactScope::Instance { instance_id }
        })
        .map(|record| record.observed_at_unix_ms)
        .unwrap_or_else(|| {
            panic!("setup: the tap left no foreground fact of the instance: {tapped:?}")
        });
    drop(holder);
    let package = neutral_foreground()
        .foreground
        .expect("the fake device reports a foreground package");

    let (cli, tool) = parity(&mut mcp, &mut id, root.path(), &words, arguments);
    for (side, answer) in [("CLI", &cli), ("ac_overview", &tool)] {
        assert_eq!(
            answer["instances"][0]["foreground"],
            json!({"package": package, "observed_at_unix_ms": observed_at_unix_ms}),
            "{side}: {answer}"
        );
    }

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

// #411 A7 (register §1.98 A7-1; apps gate 72): each instance row carries backend_selfcheck, the
// last self-check of each backend entry the instance's program facts hold, {status,
// checked_at_unix_ms} verbatim from backend.selfcheck.<entry>.status and .checked_at_unix_ms.
// After one self-check of the fake instance (its preparation opens input and capture), the CLI's
// stdout JSON and the ac_overview result for the same arguments carry the same member, and it is
// the two entries' facts as the Runtime holds them.
#[test]
fn a7_cli_and_mcp_rows_carry_the_same_backend_selfcheck() {
    let root = TempDir::new().expect("tempdir");
    write_sealed_frame(&root.path().join("sealed.png"));
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let mut id = 1;

    let holder = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect a Runtime client");
    let checked = holder.self_check_instance(ALIAS);
    assert!(checked.is_ok(), "setup: the self-check: {checked:?}");
    let instance_id = holder
        .status()
        .expect("read the Runtime status")
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == ALIAS)
        .expect("setup: the fake instance is not registered")
        .instance_id();
    let facts = holder
        .runtime_fact_snapshot()
        .expect("read the program facts");
    drop(holder);
    let fact = |key: String| {
        facts
            .records
            .iter()
            .find(|record| {
                record.key == key && record.scope == RuntimeFactScope::Instance { instance_id }
            })
            .map(|record| record.value.clone())
            .unwrap_or_else(|| panic!("setup: the self-check left no {key} fact"))
    };
    let entry = |name: &str| {
        let status = fact(format!("backend.selfcheck.{name}.status"));
        let checked_at = fact(format!("backend.selfcheck.{name}.checked_at_unix_ms"));
        let (FactValue::String(status), FactValue::Integer(checked_at)) = (status, checked_at)
        else {
            panic!("setup: the {name} self-check facts are not text and integer");
        };
        json!({"status": status, "checked_at_unix_ms": checked_at})
    };
    let expected = json!({"capture": entry("capture"), "input": entry("input")});

    let (cli, tool) = parity(
        &mut mcp,
        &mut id,
        root.path(),
        &["overview", "--instance", ALIAS, "--sections", "instances"],
        json!({"instance": ALIAS, "sections": ["instances"]}),
    );
    for (side, answer) in [("CLI", &cli), ("ac_overview", &tool)] {
        assert_eq!(
            answer["instances"][0]["backend_selfcheck"], expected,
            "{side}: {answer}"
        );
    }

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

// Workflow #390 九处-1 (board #390 comment 6095942731, item 1; corrected ruling 4 of wave 18): the
// fake Runtime's instance is not discovery-bound, so no discovery or emulator control has read
// its emulator; actingctl status and the overview both show its emulator_state as unknown, never
// off.
#[test]
fn nine1_status_and_overview_show_unknown_while_the_runtime_holds_no_reading() {
    let root = TempDir::new().expect("tempdir");
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    let unknown = json!({"state": "unknown"});

    let (exited, status) = actingctl(root.path(), &["status"]);
    assert!(
        exited && status["outcome"]["code"] == json!("runtime_status_reported"),
        "actingctl status: {status}"
    );
    let instance = &status["instances"][0];
    assert_eq!(
        (&instance["instance_alias"], &instance["emulator_state"]),
        (&json!(ALIAS), &unknown),
        "actingctl status: {status}"
    );

    let (exited, overview) = actingctl(root.path(), &["overview", "--sections", "instances"]);
    assert!(exited, "actingctl overview: {overview}");
    let row = &overview["instances"][0];
    assert_eq!(
        (&row["alias"], &row["emulator_state"]),
        (&json!(ALIAS), &unknown),
        "actingctl overview: {overview}"
    );

    runtime.stop_clean();
}

// The CLI's answer and the ac_overview result for the same arguments, each accepted without
// warnings, and equal apart from the start of the run window.
fn parity(
    mcp: &mut McpServer,
    id: &mut u64,
    root: &Path,
    words: &[&str],
    arguments: Value,
) -> (Value, Value) {
    let (exited, mut line) = actingctl(root, words);
    let outcome = line
        .as_object_mut()
        .and_then(|members| members.remove("outcome"));
    assert!(
        exited
            && outcome
                .as_ref()
                .is_some_and(|outcome| outcome["code"] == json!("runtime_overview_reported")),
        "actingctl {words:?} did not report runtime_overview_reported: {outcome:?} {line}"
    );
    *id += 1;
    let answer = mcp.call_tool(*id, "ac_overview", arguments.clone());
    let tool = accepted(&answer, "ac_overview").clone();
    assert!(
        answer["result"]["structuredContent"]
            .get("warnings")
            .is_none(),
        "ac_overview {arguments} warned: {answer}"
    );
    assert_eq!(
        without_window_start(&line),
        without_window_start(&tool),
        "actingctl {words:?} and ac_overview {arguments} answer differently:\nCLI {line}\nac_overview {tool}"
    );
    (line, tool)
}

// The daemon answers, with the owner epoch of its runtime-info.json.
#[track_caller]
fn assert_daemon(side: &str, answer: &Value) {
    let daemon = &answer["daemon"];
    assert_eq!(
        members(daemon),
        BTreeSet::from(["online", "owner_epoch"]),
        "{side}: {answer}"
    );
    assert_eq!(daemon["online"], json!(true), "{side}: {answer}");
}

// The one instance's row: its identity, lease_active, queued and its pause; no monitor and no
// run were set up, so neither is there. Workflow #390 九处-1: and its emulator_state, unknown,
// since the fake Runtime's instance is not discovery-bound and the Runtime holds no reading.
// #411 A7: and its backend_selfcheck, an object, since the program facts were read.
// #411 A42: and run_none_in_window, since the instance has no run in the window.
#[track_caller]
fn assert_rows(side: &str, answer: &Value) {
    let rows = answer["instances"].as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 1, "{side}: {answer}");
    let row = &rows[0];
    assert_eq!(
        members(row),
        BTreeSet::from([
            "alias",
            "backend_selfcheck",
            "emulator_state",
            "instance_id",
            "lease_active",
            "pause",
            "queued",
            "run_none_in_window"
        ]),
        "{side}: {answer}"
    );
    assert!(
        row["alias"] == json!(ALIAS)
            && row["lease_active"].is_boolean()
            && row["queued"].is_u64()
            && row["emulator_state"] == json!({"state": "unknown"})
            && row["backend_selfcheck"].is_object(),
        "{side}: {answer}"
    );
    assert_eq!(
        members(&row["pause"]),
        BTreeSet::from(["pause_reason", "revision", "stage"]),
        "{side}: {answer}"
    );
}

// The answer without the start of the run window and the time of the performance read (Workflow
// #385 P-m): each read takes them from its own clock. #411 A42: so does each row's window with
// no run in it.
fn without_window_start(answer: &Value) -> Value {
    let mut answer = answer.clone();
    if let Some(window) = answer.get_mut("run_window").and_then(Value::as_object_mut) {
        window.remove("since_unix_ms");
    }
    for row in answer
        .get_mut("instances")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(window) = row
            .get_mut("run_none_in_window")
            .and_then(Value::as_object_mut)
        {
            window.remove("since_unix_ms");
            window.remove("until_unix_ms");
        }
    }
    if let Some(performance) = answer.get_mut("performance").and_then(Value::as_object_mut) {
        performance.remove("observed_at_unix_ms");
    }
    answer
}

fn members(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

// `actingctl <words> --state-root <root>`: whether it exited 0, and its one stdout line.
fn actingctl(root: &Path, words: &[&str]) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args(words)
        .arg("--state-root")
        .arg(root)
        .env_remove("ACTINGCOMMAND_DETAIL")
        .output()
        .expect("run actingctl");
    let line = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "actingctl {words:?} printed no JSON line ({error}): {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), line)
}
