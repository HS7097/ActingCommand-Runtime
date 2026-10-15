// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #398 RD-1c (model-398 v1.1 §3.3, §12 row RD-1c-T4; MCP-0 command table v1.1 §2.3):
// `actingctl devices list|bind|unbind` and the MCP tools `ac_devices`, `ac_device_bind`,
// `ac_device_unbind` answer the same for the same arguments. Both sides are the real binaries,
// the MCP server over pipes (fixture I5) started with the same install root and state root the
// CLI is given. The install root has no adb here (no device is ever touched), so the three calls
// are the ones that need none, or end at it:
//
// - `list`: the install's adb does not start, so both refuse with device_list_failed naming that
//   adb, caused by the OS's failure;
// - `bind` of an alias that is no handset: both refuse with device_kind_unsupported naming it;
// - `unbind` of an alias the private file does not bind: both answer {alias, unbound: false,
//   restart_required: false}.
//
// A refusal is the CLI's one line `{outcome}` (exit 1) and the tool error `{class, outcome}` with
// the same outcome; an answer is the CLI's line without its `outcome` (device_binding_changed)
// and the tool's result. The success paths with a device listed are RD-1c-T1 to T3 and T5
// (actingctl's unit tests, with a fake adb).

use crate::mcp_harness::{McpServer, accepted};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const PHONE: &str = "node.phone";
const EMULATOR: &str = "node.emulator";

#[test]
fn gate_actingctl_devices_and_the_ac_device_tools_answer_the_same_for_the_same_arguments() {
    let install = TempDir::new().expect("install root");
    let state = TempDir::new().expect("state root");
    fs::write(
        install.path().join("actingd.config.json"),
        json!({
            "instances": [
                {"alias": PHONE, "device": {"kind": "handset", "transport": "usb"}},
                {"alias": EMULATOR, "port": 5555},
            ],
        })
        .to_string(),
    )
    .expect("write the configuration");
    let install_root = install.path().to_str().expect("UTF-8 install root");
    let state_root = state.path().to_str().expect("UTF-8 state root");
    let mut mcp = McpServer::spawn_program(
        Path::new(env!("CARGO_BIN_EXE_actingctl")),
        &["--root", install_root, "--state-root", state_root],
        &[],
    );
    mcp.initialize();
    let roots = ["--state-root", state_root, "--root", install_root];

    // list: the install has no adb.
    let line = actingctl(&[&["devices", "list"][..], &roots[..]].concat());
    let answer = mcp.call_tool(2, "ac_devices", json!({}));
    assert_eq!(
        line["outcome"]["code"],
        json!("device_list_failed"),
        "actingctl devices list: {line}"
    );
    assert_refused_alike(&line, &answer, "runtime");

    // bind: the alias is an emulator.
    let line = actingctl(&[&["devices", "bind", EMULATOR, "usb:7"][..], &roots[..]].concat());
    let answer = mcp.call_tool(
        3,
        "ac_device_bind",
        json!({"alias": EMULATOR, "handle": "usb:7"}),
    );
    assert_eq!(
        (
            &line["outcome"]["code"],
            &line["outcome"]["values"]["alias"]
        ),
        (&json!("device_kind_unsupported"), &json!(EMULATOR)),
        "actingctl devices bind: {line}"
    );
    assert_refused_alike(&line, &answer, "usage");

    // unbind: nothing is bound to the alias.
    let mut line = actingctl(&["devices", "unbind", PHONE, "--state-root", state_root]);
    let outcome = line
        .as_object_mut()
        .and_then(|members| members.remove("outcome"));
    assert_eq!(
        outcome.map(|outcome| outcome["code"].clone()),
        Some(json!("device_binding_changed")),
        "actingctl devices unbind: {line}"
    );
    let answer = mcp.call_tool(4, "ac_device_unbind", json!({"alias": PHONE}));
    let result = accepted(&answer, "ac_device_unbind");
    assert_eq!(
        (&line, result),
        (
            &json!({"alias": PHONE, "unbound": false, "restart_required": false}),
            &json!({"alias": PHONE, "unbound": false, "restart_required": false})
        ),
        "actingctl devices unbind and ac_device_unbind answer differently: {answer}"
    );

    mcp.close_input();
    drop(mcp);
}

// The CLI refused with exit 1 and one line `{outcome}`; the tool answered the error
// `{class, outcome}` with `class` and the same outcome.
#[track_caller]
fn assert_refused_alike(line: &Value, answer: &Value, class: &str) {
    let error = &answer["result"]["structuredContent"]["error"];
    assert_eq!(
        (&answer["result"]["isError"], &error["class"]),
        (&json!(true), &json!(class)),
        "the tool did not refuse with class {class}: {answer}"
    );
    assert_eq!(
        &line["outcome"], &error["outcome"],
        "the CLI and the tool refused differently:\nCLI {line}\ntool {answer}"
    );
}

// `actingctl <words>`: its one stdout line, which must be JSON; a success exits 0, a refusal 1.
fn actingctl(words: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args(words)
        .env_remove("ACTINGCOMMAND_DETAIL")
        .output()
        .expect("run actingctl");
    let line: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "actingctl {words:?} printed no JSON line ({error}): {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let refused = line
        .as_object()
        .is_some_and(|members| members.len() == 1 && members.contains_key("outcome"))
        && line["outcome"]["category"] == json!("error");
    assert_eq!(
        output.status.code(),
        Some(if refused { 1 } else { 0 }),
        "actingctl {words:?}: {line}"
    );
    line
}
