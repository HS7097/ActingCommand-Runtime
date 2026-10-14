// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #389 MCP-4 (MCP-0 command table v1.1 §2.3, §4.4): the parity test of `actingctl facts
// --instance`, `facts --program`, `status --config` and MCP `ac_facts`. On one c4 fake Runtime
// child (`tests/support/c4_runtime.rs`, no policy section), the same arguments go once to the
// real `actingctl` and once to the real `actingctl mcp-serve` over pipes (fixture I5,
// `mcp_harness`). The CLI's stdout JSON without its `outcome` and the ac_facts result are the
// same object, apart from the times each read takes on its own (`read_at_unix_ms`, and the
// program store's `taken_at_unix_ms`); every answer fits the output budget, so §2.2 leaves no
// other difference. The expected answers are §4.4's: `{scope, read_at_unix_ms, snapshot |
// records, records_total, records_kept}`, the instance's snapshot `{instance_alias, records}`
// (no server or game: the configuration binds none), `key_prefix` keeping only the records whose
// key starts with it. A Runtime without configuration facts answers `config_facts_missing` on
// both sides. Workflow #378 A5: the CLI line carries its outcome beside the answer.

use crate::c4_runtime::RuntimeChild;
use crate::mcp_harness::{McpServer, accepted};
use actingcommand_contract::priority_offset_fact_key;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const CHILD_ENTRY: &str = "c4_runtime_child_process";
const ALIAS: &str = "neutral.facts";
// Two manual priority offsets of the instance: instance-scoped facts the CLI publishes.
const OFFSETS: [(&str, i64); 2] = [("neutral.task.a", 250), ("neutral.task.b", 100)];
// The key prefix of the first offset only.
const KEY_PREFIX: &str = "session.task.neutral.task.a.";

#[test]
fn gate_actingctl_facts_and_ac_facts_answer_the_same_facts_for_the_same_arguments() {
    let root = TempDir::new().expect("tempdir");
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    for (task, offset) in OFFSETS {
        let offset = offset.to_string();
        let (exited, line) = actingctl(
            root.path(),
            &["task-offset", task, &offset, "--instance", ALIAS],
        );
        assert!(
            exited && line["outcome"]["code"] == json!("task_offset_published"),
            "setup: actingctl task-offset {task} {offset}: {line}"
        );
    }
    let (_, status) = actingctl(root.path(), &["status"]);
    let instance_id = status["instances"]
        .as_array()
        .and_then(|instances| {
            instances
                .iter()
                .find(|instance| instance["instance_alias"] == json!(ALIAS))
        })
        .and_then(|instance| instance["instance_id"].as_str())
        .unwrap_or_else(|| panic!("setup: the status lists no instance {ALIAS}: {status}"))
        .to_owned();
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let mut id = 1;

    // One instance, named by its alias and by its instance_id: both offsets.
    for selector in [ALIAS, instance_id.as_str()] {
        let (cli, tool) = parity(
            &mut mcp,
            &mut id,
            root.path(),
            &["facts", "--instance", selector],
            "instance_facts_reported",
            json!({"scope": "instance", "instance": selector}),
        );
        for (side, answer) in [("CLI", &cli), ("ac_facts", &tool)] {
            assert_instance_answer(side, answer, &OFFSETS, OFFSETS.len());
        }
    }

    // key_prefix keeps the first offset of the two the read returned.
    let (cli, tool) = parity(
        &mut mcp,
        &mut id,
        root.path(),
        &["facts", "--instance", ALIAS, "--key-prefix", KEY_PREFIX],
        "instance_facts_reported",
        json!({"scope": "instance", "instance": ALIAS, "key_prefix": KEY_PREFIX}),
    );
    for (side, answer) in [("CLI", &cli), ("ac_facts", &tool)] {
        assert_instance_answer(side, answer, &OFFSETS[..1], OFFSETS.len());
    }

    // The program store, whole.
    let (cli, tool) = parity(
        &mut mcp,
        &mut id,
        root.path(),
        &["facts", "--program"],
        "runtime_facts_reported",
        json!({"scope": "program"}),
    );
    for (side, answer) in [("CLI", &cli), ("ac_facts", &tool)] {
        let records = answer["snapshot"]["records"].as_array().unwrap_or_else(|| {
            panic!("{side}: the program answer has no snapshot.records: {answer}")
        });
        assert_eq!(
            members(answer),
            BTreeSet::from([
                "read_at_unix_ms",
                "records_kept",
                "records_total",
                "scope",
                "snapshot"
            ]),
            "{side}: {answer}"
        );
        assert_eq!(
            (
                &answer["scope"],
                &answer["records_total"],
                &answer["records_kept"]
            ),
            (
                &json!("program"),
                &json!(records.len()),
                &json!(records.len())
            ),
            "{side}: {answer}"
        );
    }

    // Without configuration facts, `status --config` and scope config refuse alike.
    let (exited, line) = actingctl(root.path(), &["status", "--config"]);
    assert!(
        !exited && line["outcome"]["code"] == json!("config_facts_missing"),
        "actingctl status --config: {line}"
    );
    id += 1;
    let answer = mcp.call_tool(id, "ac_facts", json!({"scope": "config"}));
    assert_eq!(
        (
            &answer["result"]["isError"],
            &answer["result"]["structuredContent"]["error"]["outcome"]["code"]
        ),
        (&json!(true), &json!("config_facts_missing")),
        "ac_facts scope config: {answer}"
    );

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

// The CLI's answer and the ac_facts result for the same arguments, each accepted, and equal
// apart from the times of their own reads.
fn parity(
    mcp: &mut McpServer,
    id: &mut u64,
    root: &Path,
    words: &[&str],
    success: &str,
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
                .is_some_and(|outcome| outcome["code"] == json!(success)),
        "actingctl {words:?} did not report {success}: {outcome:?} {line}"
    );
    *id += 1;
    let answer = mcp.call_tool(*id, "ac_facts", arguments.clone());
    let tool = accepted(&answer, "ac_facts").clone();
    assert_eq!(
        without_read_times(&line),
        without_read_times(&tool),
        "actingctl {words:?} and ac_facts {arguments} answer differently:\nCLI {line}\nac_facts {tool}"
    );
    (line, tool)
}

// §4.4's answer for the instance: scope instance, the instance's snapshot without a server or a
// game, the offsets `kept` in key order with their scope and value, `total` records read, and a
// read time no earlier than any record's observation.
#[track_caller]
fn assert_instance_answer(side: &str, answer: &Value, kept: &[(&str, i64)], total: usize) {
    let what = format!("{side}: {answer}");
    assert_eq!(
        members(answer),
        BTreeSet::from([
            "read_at_unix_ms",
            "records_kept",
            "records_total",
            "scope",
            "snapshot"
        ]),
        "{what}"
    );
    assert_eq!(
        (
            &answer["scope"],
            &answer["records_total"],
            &answer["records_kept"]
        ),
        (&json!("instance"), &json!(total), &json!(kept.len())),
        "{what}"
    );
    let snapshot = &answer["snapshot"];
    assert_eq!(
        members(snapshot),
        BTreeSet::from(["instance_alias", "records"]),
        "{what}"
    );
    assert_eq!(snapshot["instance_alias"], json!(ALIAS), "{what}");
    let records = snapshot["records"].as_array().cloned().unwrap_or_default();
    assert_eq!(records.len(), kept.len(), "{what}");
    let read_at = answer["read_at_unix_ms"].as_u64().unwrap_or_default();
    for (record, (task, offset)) in records.iter().zip(kept) {
        assert_eq!(
            (&record["scope"], &record["key"], &record["content"]),
            (
                &json!({"kind": "instance", "instance_id": ALIAS}),
                &json!(priority_offset_fact_key(task)),
                &json!({"kind": "inline", "value": {"type": "integer", "value": offset}}),
            ),
            "{what}"
        );
        assert!(
            record["observed_at_unix_ms"]
                .as_u64()
                .is_some_and(|observed| observed <= read_at),
            "the read time is before an observation: {what}"
        );
    }
}

// The answer without `read_at_unix_ms` and the program store's `taken_at_unix_ms`: each read
// takes them on its own.
fn without_read_times(answer: &Value) -> Value {
    let mut answer = answer.clone();
    if let Some(members) = answer.as_object_mut() {
        members.remove("read_at_unix_ms");
    }
    if let Some(snapshot) = answer.get_mut("snapshot").and_then(Value::as_object_mut) {
        snapshot.remove("taken_at_unix_ms");
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
