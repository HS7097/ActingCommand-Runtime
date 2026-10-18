// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #336 PKG-S4 (option 丙), model M-PKG v1.1 §4.5 S4-T4, on the real `actingctl
// mcp-serve` binary (fixture I5, `mcp_harness`): `ac_run_pack` takes a package and a recovery
// package by their paths, with neither `package_ref` nor `recovery_package_ref`, and starts no
// `actinglab package digest` (a server that knows no installation, so no actinglab, still hands
// the run to the Runtime); `ac_pack_check` takes the package alone and answers only its
// check (Workflow #411 A5: actinglab `package check`, no longer its preflight).

use crate::mcp_harness::McpServer;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use tempfile::TempDir;

const ALIAS: &str = "neutral.instance";

#[test]
fn s4_t4_ac_run_pack_and_ac_pack_check_take_no_package_digest() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("neutral-task.zip");
    fs::write(&package, b"package bytes").expect("write package");
    let package = package.to_str().expect("package path").to_owned();
    let mut mcp = McpServer::spawn(root.path());
    mcp.initialize();

    let listed = mcp.request(2, "tools/list", json!({}));
    assert_eq!(
        properties(
            listed_tool(&listed, "ac_run_pack"),
            "/inputSchema/properties"
        ),
        BTreeSet::from(["instance", "package", "recovery_package", "deadline_s"]),
        "{listed}"
    );
    let pack_check = listed_tool(&listed, "ac_pack_check");
    assert_eq!(
        properties(pack_check, "/inputSchema/properties"),
        BTreeSet::from(["package"]),
        "{pack_check}"
    );
    // Workflow #411 A5 (M-PKG S5-T4, replacing S4-T4's interim preflight answer): the answer is
    // actinglab package check's data, {status, package_id, entry_task_id}.
    let answer = properties(pack_check, "/outputSchema/properties/result/properties");
    for member in ["status", "package_id", "entry_task_id"] {
        assert!(answer.contains(member), "{member}: {pack_check}");
    }
    for removed in ["preflight", "digest", "package_ref_check"] {
        assert!(!answer.contains(removed), "{removed}: {pack_check}");
    }

    // A digest argument is no argument of either tool.
    for (id, tool, arguments, argument) in [
        (
            3,
            "ac_run_pack",
            json!({"instance": ALIAS, "package": package, "package_ref": "0".repeat(64)}),
            "package_ref",
        ),
        (
            4,
            "ac_run_pack",
            json!({
                "instance": ALIAS,
                "package": package,
                "recovery_package": package,
                "recovery_package_ref": "0".repeat(64),
            }),
            "recovery_package_ref",
        ),
        (
            5,
            "ac_pack_check",
            json!({"package": package, "package_ref": "0".repeat(64)}),
            "package_ref",
        ),
    ] {
        let refused = mcp.call_tool(id, tool, arguments);
        let error = &refused["result"]["structuredContent"]["error"];
        assert_eq!(
            (
                &error["outcome"]["code"],
                &error["outcome"]["values"]["argument"],
                &error["outcome"]["values"]["argument_problem"],
            ),
            (
                &json!("arguments_invalid"),
                &json!(argument),
                &json!("unknown")
            ),
            "{tool}: {refused}"
        );
    }

    // This server knows no installation, so it has no actinglab to compute a digest with; the
    // run is handed to the Runtime all the same (there is none at this state root).
    let answer = mcp.call_tool(
        6,
        "ac_run_pack",
        json!({"instance": ALIAS, "package": package}),
    );
    let outcome = &answer["result"]["structuredContent"]["error"]["outcome"];
    assert_ne!(outcome["code"], json!("lab_tool_unavailable"), "{answer}");
    assert_ne!(
        outcome["values"]["mcp_blocker"],
        json!("install_root"),
        "{answer}"
    );
}

// The definition of `name` in a tools/list answer.
fn listed_tool<'a>(listed: &'a Value, name: &str) -> &'a Value {
    listed["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list answered no tools: {listed}"))
        .iter()
        .find(|tool| tool["name"] == json!(name))
        .unwrap_or_else(|| panic!("tools/list does not list {name}: {listed}"))
}

// The property names of the schema object at `pointer` in a tool definition (its arguments, or
// the result of its answer).
fn properties<'a>(tool: &'a Value, pointer: &str) -> BTreeSet<&'a str> {
    tool.pointer(pointer)
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("{pointer} is no schema object: {tool}"))
        .keys()
        .map(String::as_str)
        .collect()
}
