// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B15 R6 (apps gate 74; register §1.116; model-b15 §5.1, §7.6;
// contracts/instance-fact-store.md, "Remedy choice facts"): `actingctl remedy set|get|lift` and
// MCP `ac_remedy_set` / `ac_remedy_get` write and read an instance's remedy choices, the instance
// fact `session.remedy.choices`. On one c4 fake Runtime child (`tests/support/c4_runtime.rs`: one
// instance, no policy section, so no package is bound and the instance's facts are its own), the
// real `actingctl` and the real `actingctl mcp-serve` over pipes (fixture I5, `mcp_harness`):
//
// - `set` writes the scope's whole list as a new version and answers {event_id, scope, key,
//   source_snapshot_id, observed_at_unix_ms, rows_total}; a write the Runtime refuses prints its
//   receipt and exits non-zero;
// - `get` answers the records that apply to the instance, most specific first, and the one in
//   effect;
// - `lift` republishes the instance's record unchanged, or an empty one when it has none, as a
//   new observation (model-b15 §7.6 item 2), naming the record it replaced.
//
// The rows name needs only as `none` rows, which name nothing a package must declare; an `allow`
// row names a need no bound package declares and is refused. Every expected value is the brief's
// requirement or the contract's.

use crate::c4_runtime::RuntimeChild;
use crate::mcp_harness::{McpServer, accepted};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const CHILD_ENTRY: &str = "c4_runtime_child_process";
const ALIAS: &str = "neutral.remedy";
const KEY: &str = "session.remedy.choices";
const PUBLISHED: &str = "request_publish_facts_completed";
const READ: &str = "request_instance_fact_snapshot_completed";

// Requirement 4: `remedy get` before any write answers no record and none in effect; `remedy
// lift` without an instance record publishes an empty one; `remedy set --instance` writes the
// rows and `get` answers that record, in effect, with those rows; `lift` republishes the same rows
// as a new version that `get` then answers; `set --game` and `set --server --game` write the
// game's and the server's record; a write the Runtime refuses (an `allow` of a need no bound
// package declares) prints its denied receipt with `session_remedy_choices_invalid` and exits
// non-zero, and writes nothing.
#[test]
fn b15w_remedy_set_get_and_lift_round_trip() {
    let root = TempDir::new().expect("tempdir");
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    let instance = json!({"kind": "instance", "instance_id": ALIAS});
    let rows = json!([
        {"row": "none", "need": "need_a"},
        {"row": "none", "need": "need_b"},
    ]);
    let rows_file = root.path().join("rows.json");
    fs::write(&rows_file, rows.to_string()).expect("the rows file");
    let rows_path = rows_file.to_str().expect("a UTF-8 path");

    let empty = line(root.path(), &["remedy", "get", "--instance", ALIAS], READ);
    assert_eq!(
        (
            &empty["instance_alias"],
            &empty["records"],
            &empty["in_effect"]
        ),
        (&json!(ALIAS), &json!([]), &Value::Null),
        "{empty}"
    );

    let lifted_empty = line(
        root.path(),
        &["remedy", "lift", "--instance", ALIAS],
        PUBLISHED,
    );
    assert_written(&lifted_empty, &instance, 0);
    assert_eq!(
        lifted_empty["previous_source_snapshot_id"],
        Value::Null,
        "{lifted_empty}"
    );

    let set = line(
        root.path(),
        &["remedy", "set", "--instance", ALIAS, "--rows", rows_path],
        PUBLISHED,
    );
    assert_written(&set, &instance, 2);
    assert_ne!(
        set["source_snapshot_id"], lifted_empty["source_snapshot_id"],
        "a new version"
    );
    assert_in_effect(
        &line(root.path(), &["remedy", "get", "--instance", ALIAS], READ),
        &set,
        &rows,
    );

    let lifted = line(
        root.path(),
        &["remedy", "lift", "--instance", ALIAS],
        PUBLISHED,
    );
    assert_written(&lifted, &instance, 2);
    assert_eq!(
        lifted["previous_source_snapshot_id"], set["source_snapshot_id"],
        "{lifted}"
    );
    assert!(
        lifted["source_snapshot_id"] != set["source_snapshot_id"]
            && lifted["event_id"] != set["event_id"]
            && lifted["observed_at_unix_ms"].as_u64() > set["observed_at_unix_ms"].as_u64(),
        "a lift is a new observation: {set} {lifted}"
    );
    assert_in_effect(
        &line(root.path(), &["remedy", "get", "--instance", ALIAS], READ),
        &lifted,
        &rows,
    );

    let game = line(
        root.path(),
        &[
            "remedy",
            "set",
            "--game",
            "neutral_game",
            "--rows",
            rows_path,
        ],
        PUBLISHED,
    );
    assert_written(
        &game,
        &json!({"kind": "game", "game_id": "neutral_game"}),
        2,
    );
    let server = line(
        root.path(),
        &[
            "remedy",
            "set",
            "--server",
            "neutral_server",
            "--game",
            "neutral_game",
            "--rows",
            rows_path,
        ],
        PUBLISHED,
    );
    assert_written(
        &server,
        &json!({"kind": "server", "server_id": "neutral_server"}),
        2,
    );

    let undeclared = root.path().join("undeclared.json");
    fs::write(
        &undeclared,
        json!([{"row": "allow", "need": "need_a", "capability": "cap_x"}]).to_string(),
    )
    .expect("the undeclared rows file");
    let (exited, refused) = actingctl(
        root.path(),
        &[
            "remedy",
            "set",
            "--instance",
            ALIAS,
            "--rows",
            undeclared.to_str().expect("a UTF-8 path"),
        ],
    );
    assert!(!exited, "a refused write exits non-zero: {refused}");
    assert_eq!(
        (
            &refused["outcome"]["code"],
            &refused["receipt"]["state"],
            &refused["outcome"]["values"]["fact_key"],
            &refused["outcome"]["values"]["field"],
            &refused["outcome"]["values"]["instance_id"],
        ),
        (
            &json!("session_remedy_choices_invalid"),
            &json!("denied"),
            &json!(KEY),
            &json!("/0/need"),
            &json!(ALIAS),
        ),
        "{refused}"
    );
    assert_in_effect(
        &line(root.path(), &["remedy", "get", "--instance", ALIAS], READ),
        &lifted,
        &rows,
    );

    runtime.stop_clean();
}

// Requirement 5: `ac_remedy_set` writes the record `ac_remedy_get` then reads in effect, with its
// rows, on a server that enables only the default tiers (general tools); `actingctl remedy get`
// answers the same object apart from the time of its own read; `ac_remedy_set` with `lift` true
// republishes the record as a new version, the one `ac_remedy_get` then reads.
#[test]
fn b15w_ac_remedy_set_and_get_write_and_read_the_same_record() {
    let root = TempDir::new().expect("tempdir");
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    let instance = json!({"kind": "instance", "instance_id": ALIAS});
    let rows = json!([{"row": "none", "need": "need_a"}]);
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();

    let answer = mcp.call_tool(2, "ac_remedy_set", json!({"instance": ALIAS, "rows": rows}));
    let set = accepted(&answer, "ac_remedy_set").clone();
    assert_written(&set, &instance, 1);

    let answer = mcp.call_tool(3, "ac_remedy_get", json!({"instance": ALIAS}));
    let got = accepted(&answer, "ac_remedy_get").clone();
    assert_in_effect(&got, &set, &rows);
    let mut cli = line(root.path(), &["remedy", "get", "--instance", ALIAS], READ);
    if let Some(members) = cli.as_object_mut() {
        members.remove("outcome");
    }
    assert_eq!(
        without_read_time(&cli),
        without_read_time(&got),
        "the CLI and MCP read the same record"
    );

    let answer = mcp.call_tool(4, "ac_remedy_set", json!({"instance": ALIAS, "lift": true}));
    let lifted = accepted(&answer, "ac_remedy_set").clone();
    assert_written(&lifted, &instance, 1);
    assert_eq!(
        lifted["previous_source_snapshot_id"], set["source_snapshot_id"],
        "{lifted}"
    );
    assert_ne!(lifted["source_snapshot_id"], set["source_snapshot_id"]);
    let answer = mcp.call_tool(5, "ac_remedy_get", json!({"instance": ALIAS}));
    assert_in_effect(accepted(&answer, "ac_remedy_get"), &lifted, &rows);

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

// A write's answer: the scope written, the key, a `remedy-choices:` snapshot id, an event id and
// an observation time, and `rows` rows.
#[track_caller]
fn assert_written(answer: &Value, scope: &Value, rows: u64) {
    assert_eq!(
        (&answer["scope"], &answer["key"], &answer["rows_total"]),
        (scope, &json!(KEY), &json!(rows)),
        "{answer}"
    );
    assert!(
        answer["source_snapshot_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("remedy-choices:"))
            && answer["event_id"].is_string()
            && answer["observed_at_unix_ms"].as_u64().is_some(),
        "{answer}"
    );
}

// A read's answer: the instance's one record, the write `written`, in effect with `rows`.
#[track_caller]
fn assert_in_effect(answer: &Value, written: &Value, rows: &Value) {
    assert_eq!(answer["instance_alias"], json!(ALIAS), "{answer}");
    let records = answer["records"].as_array().cloned().unwrap_or_default();
    assert_eq!(records.len(), 1, "{answer}");
    assert_eq!(
        (
            &records[0]["scope"],
            &records[0]["source_snapshot_id"],
            &records[0]["observed_at_unix_ms"],
            &records[0]["rows"],
            &records[0]["in_effect"],
            &answer["in_effect"],
        ),
        (
            &written["scope"],
            &written["source_snapshot_id"],
            &written["observed_at_unix_ms"],
            rows,
            &json!(true),
            &written["scope"],
        ),
        "{answer}"
    );
}

fn without_read_time(answer: &Value) -> Value {
    let mut answer = answer.clone();
    if let Some(members) = answer.as_object_mut() {
        members.remove("read_at_unix_ms");
    }
    answer
}

// The line of a successful `actingctl <words>` whose outcome is `success`.
#[track_caller]
fn line(root: &Path, words: &[&str], success: &str) -> Value {
    let (exited, line) = actingctl(root, words);
    assert!(
        exited && line["outcome"]["code"] == json!(success),
        "actingctl {words:?} did not report {success}: {line}"
    );
    line
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
