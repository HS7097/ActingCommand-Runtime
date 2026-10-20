// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #386 SR-6b, MCP-19b (apps key 80; register §1.144 M19B-1, M19B-2; plan-mcp19 §2):
// `actingctl schedule` prints why each candidate of the latest policy evaluation did not run
// (ScheduleExplanation, with every candidate) and MCP `ac_schedule` answers the same object.
// Both sides are the real binaries: the MCP server over pipes (fixture I5, `mcp_harness`) and the
// CLI as one process per call, on one c4 fake Runtime child (`tests/support/c4_runtime.rs`) with
// one instance and no policy catalog.
//
// The parity test (the rule of apps key 41): for the same arguments the CLI's stdout JSON without
// its `outcome` and the `ac_schedule` result are the same object. No policy evaluation has run on
// the fake Runtime, so the answer has no evaluation time, no catalog and no entry.
// `--instance` / `instance` (the alias) narrows it to the one instance, resolved with one status
// read. Workflow #378 A5: the CLI line carries `runtime_schedule_reported`, and exits 0.

use crate::c4_runtime::RuntimeChild;
use crate::mcp_harness::{McpServer, accepted};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const CHILD_ENTRY: &str = "c4_runtime_child_process";
const ALIAS: &str = "neutral.instance";

#[test]
fn gate_actingctl_schedule_and_ac_schedule_answer_the_same_explanation() {
    let root = TempDir::new().expect("tempdir");
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();

    for (id, words, arguments) in [
        (2, vec!["schedule"], json!({})),
        (
            3,
            vec!["schedule", "--instance", ALIAS],
            json!({"instance": ALIAS}),
        ),
    ] {
        let (code, mut line) = actingctl(root.path(), &words);
        let outcome = line
            .as_object_mut()
            .and_then(|members| members.remove("outcome"));
        assert!(
            code == Some(0)
                && outcome
                    .as_ref()
                    .is_some_and(|outcome| outcome["code"] == json!("runtime_schedule_reported")),
            "actingctl {words:?} did not report runtime_schedule_reported: {outcome:?} {line}"
        );
        let answer = mcp.call_tool(id, "ac_schedule", arguments.clone());
        let tool = accepted(&answer, "ac_schedule").clone();
        assert!(
            answer["result"]["structuredContent"]
                .get("warnings")
                .is_none(),
            "ac_schedule {arguments} warned: {answer}"
        );
        assert_eq!(
            line, tool,
            "actingctl {words:?} and ac_schedule {arguments} answer differently:\nCLI {line}\nac_schedule {tool}"
        );
        for (side, explanation) in [("CLI", &line), ("ac_schedule", &tool)] {
            assert_eq!(
                explanation,
                &json!({"instances": []}),
                "{side}: no evaluation has run, so there is no evaluation time and no entry"
            );
        }
    }

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

// `actingctl <words> --state-root <root>`: its exit code and its one stdout line.
fn actingctl(root: &Path, words: &[&str]) -> (Option<i32>, Value) {
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
    (output.status.code(), line)
}
