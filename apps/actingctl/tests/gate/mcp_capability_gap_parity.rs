// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #410 item 6 (board #410, approved by Alice 2026-10-15; the 0.12 end): a local
// capability-gap log. An agent records a missing capability with a short why; the record stays
// on this computer and is never sent anywhere. `actingctl capability-gap --capability <text>
// --why <text>` and the MCP tool `ac_capability_gap` (capability, why) are one command (MCP-0
// command table v1.1 §2.3): for the same texts both append one line to the same local log and
// answer the same `{path, gap}`, the line differing only in `via` (`cli`, `mcp`) and its time.
// The server runs with its default tiers, observer only: the tool is there in every session.
// Both sides are the real binaries with one local application data folder and no Runtime.

use crate::mcp_harness::{McpServer, accepted};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const CAPABILITY: &str = "batch lab actions";
const WHY: &str = "scrolling one list took twelve ac_lab_do calls";

#[test]
fn gate_actingctl_capability_gap_and_ac_capability_gap_record_the_same_gap_locally() {
    let local = TempDir::new().expect("local application data");
    let state = TempDir::new().expect("state root");
    let local_path = local.path().to_str().expect("UTF-8 local application data");
    let state_path = state.path().to_str().expect("UTF-8 state root");
    let log = local
        .path()
        .join("ActingCommand")
        .join("actingctl")
        .join("capability-gaps.jsonl");

    let output = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args(["capability-gap", "--capability", CAPABILITY, "--why", WHY])
        .env("LOCALAPPDATA", local.path())
        .env_remove("ACTINGCOMMAND_DETAIL")
        .output()
        .expect("run actingctl capability-gap");
    let mut line: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "actingctl capability-gap printed no JSON line ({error}): {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(output.status.code(), Some(0), "{line}");
    let outcome = line
        .as_object_mut()
        .and_then(|members| members.remove("outcome"));
    assert_eq!(
        outcome.map(|outcome| outcome["code"].clone()),
        Some(json!("capability_gap_recorded")),
        "{line}"
    );

    let mut mcp = McpServer::spawn_program(
        Path::new(env!("CARGO_BIN_EXE_actingctl")),
        &["--state-root", state_path],
        &[("LOCALAPPDATA", local_path)],
    );
    mcp.initialize();
    let answer = mcp.call_tool(
        2,
        "ac_capability_gap",
        json!({"capability": CAPABILITY, "why": WHY}),
    );
    let result = accepted(&answer, "ac_capability_gap").clone();
    mcp.close_input();
    drop(mcp);

    // The same log, the same answer but for the channel and the time.
    let path = json!(log.display().to_string());
    assert_eq!((&line["path"], &result["path"]), (&path, &path));
    let without_channel_and_time = |answer: &Value| {
        let mut gap = answer["gap"].clone();
        if let Some(members) = gap.as_object_mut() {
            members.remove("via");
            members.remove("recorded_at_unix_ms");
        }
        gap
    };
    assert_eq!(
        without_channel_and_time(&line),
        without_channel_and_time(&result),
        "the CLI and the tool recorded different gaps:\nCLI {line}\ntool {result}"
    );
    assert_eq!(
        (&line["gap"]["capability"], &line["gap"]["why"]),
        (&json!(CAPABILITY), &json!(WHY))
    );
    assert_eq!(
        (&line["gap"]["via"], &result["gap"]["via"]),
        (&json!("cli"), &json!("mcp"))
    );

    // Both lines are in the log, in order, as answered; nothing else is.
    let text = fs::read_to_string(&log).expect("the local log");
    let lines = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("one JSON object per line"))
        .collect::<Vec<_>>();
    assert_eq!(lines, vec![line["gap"].clone(), result["gap"].clone()]);
}
