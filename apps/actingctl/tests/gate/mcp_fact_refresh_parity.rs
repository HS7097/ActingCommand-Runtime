// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #384 D2-S5 (frozen model D2 v1.4.2 §6 S5; unified interface model §3.3 rule 6, §7.4:
// the D2 S5 query is a general read whose MCP side is `ac_overview`'s `fact_refresh` block):
// `actingctl fact-refresh` prints what the scheduler's fact refresh waits for
// (FactRefreshStatus) and MCP `ac_overview` answers the same object as its `fact_refresh` block.
// Both sides are the real binaries: the MCP server over pipes (fixture I5, `mcp_harness`) and the
// CLI as one process per call, on one c4 fake Runtime child (`tests/support/c4_runtime.rs`) with
// one instance and no policy catalog.
//
// The parity test (the rule of apps key 41): for the same arguments the CLI's stdout JSON without
// its `outcome` and the `fact_refresh` block are the same object. No policy cycle has run on the
// fake Runtime, so the recorder has observed no cycle and holds no transition and no waiting key.
// `--instance` / `instance` narrows the rows to the one instance. Workflow #378 A5: the CLI line
// carries `runtime_fact_refresh_reported`.

use crate::c4_runtime::RuntimeChild;
use crate::mcp_harness::{McpServer, accepted};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const CHILD_ENTRY: &str = "c4_runtime_child_process";
const ALIAS: &str = "neutral.refresh";

#[test]
fn gate_actingctl_fact_refresh_and_ac_overview_answer_the_same_fact_refresh_status() {
    let root = TempDir::new().expect("tempdir");
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();

    for (id, words, arguments) in [
        (
            2,
            vec!["fact-refresh"],
            json!({"sections": ["fact_refresh"]}),
        ),
        (
            3,
            vec!["fact-refresh", "--instance", ALIAS],
            json!({"instance": ALIAS, "sections": ["fact_refresh"]}),
        ),
    ] {
        let (code, mut line) = actingctl(root.path(), &words);
        let outcome = line
            .as_object_mut()
            .and_then(|members| members.remove("outcome"));
        assert!(
            code == Some(0)
                && outcome.as_ref().is_some_and(
                    |outcome| outcome["code"] == json!("runtime_fact_refresh_reported")
                ),
            "actingctl {words:?} did not report runtime_fact_refresh_reported: {outcome:?} {line}"
        );
        let answer = mcp.call_tool(id, "ac_overview", arguments.clone());
        let tool = accepted(&answer, "ac_overview");
        assert!(
            answer["result"]["structuredContent"]
                .get("warnings")
                .is_none(),
            "ac_overview {arguments} warned: {answer}"
        );
        let block = tool["fact_refresh"].clone();
        assert_eq!(
            line, block,
            "actingctl {words:?} and ac_overview {arguments} answer differently:\nCLI {line}\nac_overview {tool}"
        );
        for (side, status) in [("CLI", &line), ("ac_overview", &block)] {
            assert_eq!(
                status,
                &json!({"cycle_seq": 0, "transition_seq": 0}),
                "{side}: no cycle has run, so nothing is waited for: {status}"
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
