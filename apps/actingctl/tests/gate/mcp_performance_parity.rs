// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #385 P-m (frozen model PERF v1.4.2 §4.6, test row Tm-12): `actingctl perf-status`
// prints the shadow measurement's status (PerformanceStatus) and MCP `ac_overview` answers the
// same object as its `performance` block. Both sides are the real binaries: the MCP server over
// pipes (fixture I5, `mcp_harness`) and the CLI as one process per call, on one c4 fake Runtime
// child (`tests/support/c4_runtime.rs`) with one instance and no performance monitor.
//
// The parity test (the rule of apps key 41): for the same arguments the CLI's stdout JSON without
// its `outcome` and the `performance` block are the same object, apart from the time of each
// read (`observed_at_unix_ms`). The mode in effect is observe, so the row's stage and factor in
// effect are s0 and 1000; the monitor is unavailable. `--instance` / `instance` narrows the rows
// to the one instance. Workflow #378 A5: the CLI line carries `runtime_performance_reported`.
//
// Without actingd the CLI reports the Runtime's absence and exits 1 (§4.6: "actingd 不在时照
// `actingctl status` 今天的方式报错、退出非 0").

use crate::c4_runtime::RuntimeChild;
use crate::mcp_harness::{McpServer, accepted};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const CHILD_ENTRY: &str = "c4_runtime_child_process";
const ALIAS: &str = "neutral.performance";

#[test]
fn gate_actingctl_perf_status_and_ac_overview_answer_the_same_performance_status() {
    let root = TempDir::new().expect("tempdir");
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();

    for (id, words, arguments) in [
        (2, vec!["perf-status"], json!({"sections": ["performance"]})),
        (
            3,
            vec!["perf-status", "--instance", ALIAS],
            json!({"instance": ALIAS, "sections": ["performance"]}),
        ),
    ] {
        let (code, mut line) = actingctl(root.path(), &words);
        let outcome = line
            .as_object_mut()
            .and_then(|members| members.remove("outcome"));
        assert!(
            code == Some(0)
                && outcome.as_ref().is_some_and(
                    |outcome| outcome["code"] == json!("runtime_performance_reported")
                ),
            "actingctl {words:?} did not report runtime_performance_reported: {outcome:?} {line}"
        );
        let answer = mcp.call_tool(id, "ac_overview", arguments.clone());
        let tool = accepted(&answer, "ac_overview");
        assert!(
            answer["result"]["structuredContent"]
                .get("warnings")
                .is_none(),
            "ac_overview {arguments} warned: {answer}"
        );
        let block = tool["performance"].clone();
        assert_eq!(
            without_read_time(&line),
            without_read_time(&block),
            "actingctl {words:?} and ac_overview {arguments} answer differently:\nCLI {line}\nac_overview {tool}"
        );
        for (side, status) in [("CLI", &line), ("ac_overview", &block)] {
            assert_eq!(status["pacing_mode"], json!("observe"), "{side}: {status}");
            assert_eq!(
                status["monitor_health"],
                json!("unavailable"),
                "{side}: {status}"
            );
            assert!(status["observed_at_unix_ms"].is_u64(), "{side}: {status}");
            let rows = status["instances"].as_array().cloned().unwrap_or_default();
            assert_eq!(rows.len(), 1, "{side}: {status}");
            assert_eq!(
                (
                    &rows[0]["instance_alias"],
                    &rows[0]["effective_stage"],
                    &rows[0]["effective_factor_milli"]
                ),
                (&json!(ALIAS), &json!("s0"), &json!(1_000)),
                "{side}: {status}"
            );
            assert_eq!(
                (
                    &status["host"]["effective_stage"],
                    &status["host"]["effective_factor_milli"]
                ),
                (&json!("s0"), &json!(1_000)),
                "{side}: {status}"
            );
        }
    }

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

#[test]
fn gate_perf_status_without_actingd_exits_1_with_the_runtime_s_absence() {
    let root = TempDir::new().expect("tempdir");
    let (code, line) = actingctl(root.path(), &["perf-status"]);
    assert_eq!(code, Some(1), "{line}");
    assert_eq!(
        line["outcome"]["code"],
        json!("runtime_info_unavailable"),
        "{line}"
    );
    assert!(line.get("host").is_none(), "{line}");
}

// The status without the time of its read: each read takes it from the Runtime's clock.
fn without_read_time(status: &Value) -> Value {
    let mut status = status.clone();
    if let Some(members) = status.as_object_mut() {
        members.remove("observed_at_unix_ms");
    }
    status
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
