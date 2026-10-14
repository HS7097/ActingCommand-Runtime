// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #389 MCP-13a: MCP `ac_app` is an operator tool before U-3 (MCP-0 command table v1.1
// §0.1 item 2, §4.14; FCM:116), on the real `actingctl mcp-serve` binary (fixture I5,
// `mcp_harness`). A server without the operator tier does not list it and answers a call with
// `tier_not_enabled` (its tier, `mcp_blocker` `server_tier`); a server with it lists it as a
// destructive, non-read-only tool, refuses an action `session app` does not have and a call
// without its instance as `arguments_invalid`, and answers `lab_tool_unavailable` with
// `mcp_blocker` `install_root` when it knows no installation whose actinglab it could run.

use crate::mcp_harness::McpServer;
use serde_json::{Value, json};
use tempfile::TempDir;

const ALIAS: &str = "neutral.instance";

#[test]
fn gate_ac_app_is_an_operator_tool_that_refuses_what_session_app_does_not_take() {
    let root = TempDir::new().expect("tempdir");

    let mut observer = McpServer::spawn_with_tier(root.path(), Some("observer"));
    observer.initialize();
    let listed = observer.request(2, "tools/list", json!({}));
    assert!(
        listed_tool(&listed, "ac_app").is_none(),
        "an observer server lists ac_app: {listed}"
    );
    let refused = observer.call_tool(3, "ac_app", json!({"action": "stop", "instance": ALIAS}));
    let outcome = tool_error(&refused, "safety", "tier_not_enabled");
    assert_eq!(
        (
            &outcome["values"]["required_tier"],
            &outcome["values"]["mcp_blocker"]
        ),
        (&json!("operator"), &json!("server_tier")),
        "{refused}"
    );
    drop(observer);

    let mut operator = McpServer::spawn(root.path());
    operator.initialize();
    let listed = operator.request(2, "tools/list", json!({}));
    let tool = listed_tool(&listed, "ac_app")
        .unwrap_or_else(|| panic!("an operator server does not list ac_app: {listed}"));
    assert_eq!(
        (
            &tool["annotations"]["readOnlyHint"],
            &tool["annotations"]["destructiveHint"]
        ),
        (&json!(false), &json!(true)),
        "{tool}"
    );

    // An action `session app` does not have, and a call without its instance.
    for (id, arguments, argument, problem) in [
        (
            3,
            json!({"action": "pause", "instance": ALIAS}),
            "action",
            "not_recognised",
        ),
        (4, json!({"action": "launch"}), "instance", "required"),
    ] {
        let refused = operator.call_tool(id, "ac_app", arguments);
        let outcome = tool_error(&refused, "usage", "arguments_invalid");
        assert_eq!(
            (
                &outcome["values"]["argument"],
                &outcome["values"]["argument_problem"]
            ),
            (&json!(argument), &json!(problem)),
            "{refused}"
        );
    }

    // This server was given a state root only: no installation, so no actinglab to run.
    let unavailable = operator.call_tool(
        5,
        "ac_app",
        json!({"action": "force-stop", "instance": ALIAS}),
    );
    let outcome = tool_error(&unavailable, "usage", "lab_tool_unavailable");
    assert_eq!(
        outcome["values"]["mcp_blocker"],
        json!("install_root"),
        "{unavailable}"
    );
}

// The definition of `name` in a tools/list answer.
fn listed_tool<'a>(listed: &'a Value, name: &str) -> Option<&'a Value> {
    listed["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list answered no tools: {listed}"))
        .iter()
        .find(|tool| tool["name"] == json!(name))
}

// The outcome of a tool error answer, after checking its class and code.
#[track_caller]
fn tool_error<'a>(answer: &'a Value, class: &str, code: &str) -> &'a Value {
    let result = &answer["result"];
    assert_eq!(result["isError"], json!(true), "not a tool error: {answer}");
    let error = &result["structuredContent"]["error"];
    assert_eq!(
        (&error["class"], &error["outcome"]["code"]),
        (&json!(class), &json!(code)),
        "{answer}"
    );
    &error["outcome"]
}
