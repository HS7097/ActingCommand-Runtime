// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #389 MCP-5a: the parity test of `actingctl observe --export` and MCP `ac_observe`
//! (MCP-0 command table v1.1 §2.3, §4.5). The Runtime is the c4 fake child process
//! (`tests/support/c4_runtime.rs`); the MCP server is the real binary over pipes (fixture I5,
//! `mcp_harness`). With the same instance argument, first the alias and then the instance_id,
//! the CLI's stdout JSON and the ac_observe result are the same object apart from two members:
//! `export.path`, a presentation difference of §2.2 (the CLI writes to the directory it is given,
//! MCP to its fixed export directory), and `frame.ref`, because every call captures a frame of its
//! own and names its own ledger event; each `frame.ref` is read back with ac_material instead.
//! Every expected value comes from §4.5 and the fixture's frame. Workflow #378 A5: the CLI line
//! carries the outcome `instance_frame_exported` beside the answer; the answer is the rest of
//! the line.

use crate::c4_runtime::{RuntimeChild, write_sealed_frame};
use crate::mcp_harness::{McpServer, accepted};
use actingcommand_contract::{EventActor, EventSource};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

const CHILD_ENTRY: &str = "c4_runtime_child_process";
const ALIAS: &str = "neutral.observe";
/// The fixture's frame (`c4_runtime::page_frame`): 16 by 9 pixels, taken by its screencap
/// capture.
const FRAME_WIDTH: u64 = 16;
const FRAME_HEIGHT: u64 = 9;
const FRAME_CAPTURE: &str = "adb_screencap";

#[test]
fn gate_observe_export_and_ac_observe_answer_the_same_frame_for_the_same_instance_argument() {
    let root = TempDir::new().expect("tempdir");
    write_sealed_frame(&root.path().join("sealed.png"));
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    let instance_id = registered_instance_id(root.path());
    let cli_directory = TempDir::new().expect("tempdir for the CLI export");
    // The server inherits this process's environment, so its export directory is this one.
    let mcp_directory = env::temp_dir().join("actingcommand-mcp").join("materials");
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let mut id = 1;

    for selector in [ALIAS.to_owned(), instance_id] {
        let cli = cli_observe(root.path(), &selector, cli_directory.path());
        id += 1;
        let answer = mcp.call_tool(id, "ac_observe", json!({"instance": selector}));
        let tool = accepted(&answer, "ac_observe").clone();

        for (side, result, directory) in [
            ("CLI", &cli, cli_directory.path()),
            ("ac_observe", &tool, mcp_directory.as_path()),
        ] {
            assert_answer(side, &selector, result, directory);
            id += 1;
            let material = mcp.call_tool(
                id,
                "ac_material",
                json!({"ref": result["frame"]["ref"], "mode": "export"}),
            );
            let read_back = accepted(&material, "ac_material");
            assert_eq!(
                (&read_back["sha256"], &read_back["size"]),
                (&result["frame"]["sha256"], &result["frame"]["size"]),
                "{side} with instance {selector}: ac_material does not read frame.ref back as this frame: {material}"
            );
        }
        assert_eq!(
            without_presentation(&cli),
            without_presentation(&tool),
            "with instance {selector}, the CLI's stdout JSON and the ac_observe result differ beyond export.path and frame.ref:\nCLI {cli}\nac_observe {tool}"
        );
    }

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

/// One side's answer: the members of §4.5, the observation of the fixture's frame, and an export
/// in `directory` that is that frame: named `<sha256 hex>.png`, with that SHA-256 and size.
#[track_caller]
fn assert_answer(side: &str, selector: &str, result: &Value, directory: &Path) {
    let what = format!("{side} with instance {selector}: {result}");
    assert_eq!(
        members(result),
        BTreeSet::from([
            "capture_backend",
            "export",
            "frame",
            "height",
            "verdict",
            "width"
        ]),
        "{what}"
    );
    assert_eq!(
        members(&result["frame"]),
        BTreeSet::from(["media_type", "ref", "sha256", "size"]),
        "{what}"
    );
    assert_eq!(
        members(&result["export"]),
        BTreeSet::from(["path", "sha256", "size"]),
        "{what}"
    );
    assert_eq!(
        (result["width"].as_u64(), result["height"].as_u64()),
        (Some(FRAME_WIDTH), Some(FRAME_HEIGHT)),
        "{what}"
    );
    assert_eq!(result["capture_backend"], json!(FRAME_CAPTURE), "{what}");
    assert!(result["verdict"].is_string(), "{what}");
    assert_eq!(result["frame"]["media_type"], json!("image/png"), "{what}");
    // frame.ref is `<seq>:<event_id>:<artifact_id>`, as ac_events gives it.
    let reference = result["frame"]["ref"].as_str().unwrap_or_default();
    let parts = reference.split(':').collect::<Vec<_>>();
    assert!(
        parts.len() == 3
            && parts[0].parse::<u64>().is_ok()
            && parts.iter().all(|part| !part.is_empty()),
        "{what}"
    );

    let path = PathBuf::from(
        result["export"]["path"]
            .as_str()
            .unwrap_or_else(|| panic!("an export without a path: {what}")),
    );
    let bytes = fs::read(&path).unwrap_or_else(|error| {
        panic!("cannot read the exported file {}: {error}", path.display())
    });
    let digest = format!("{:x}", Sha256::digest(&bytes));
    assert_eq!(path.parent(), Some(directory), "{what}");
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some(format!("{digest}.png").as_str()),
        "{what}"
    );
    assert_eq!(
        result["export"]["sha256"],
        json!(format!("sha256:{digest}")),
        "{what}"
    );
    assert_eq!(result["export"]["size"], json!(bytes.len()), "{what}");
    assert_eq!(
        (&result["frame"]["sha256"], &result["frame"]["size"]),
        (&result["export"]["sha256"], &result["export"]["size"]),
        "the frame and its export differ: {what}"
    );
}

/// The answer without `export.path` (§2.2: where an export goes) and `frame.ref` (each call's
/// own frame).
fn without_presentation(result: &Value) -> Value {
    let mut result = result.clone();
    if let Some(export) = result.get_mut("export").and_then(Value::as_object_mut) {
        export.remove("path");
    }
    if let Some(frame) = result.get_mut("frame").and_then(Value::as_object_mut) {
        frame.remove("ref");
    }
    result
}

fn members(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

/// `actingctl observe --export`: it exits 0 and prints one JSON line, whose `outcome` is
/// `instance_frame_exported`; the answer is the line without it.
fn cli_observe(root: &Path, selector: &str, directory: &Path) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .arg("observe")
        .arg("--state-root")
        .arg(root)
        .args(["--instance", selector])
        .arg("--export")
        .arg(directory)
        .output()
        .expect("run actingctl observe --export");
    assert!(
        output.status.success(),
        "actingctl observe --export with instance {selector} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let mut line: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "actingctl observe --export printed no JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    let outcome = line
        .as_object_mut()
        .and_then(|members| members.remove("outcome"));
    let Some(outcome) = outcome else {
        panic!("actingctl observe --export with instance {selector} printed no outcome: {line}")
    };
    assert_eq!(
        outcome["code"],
        json!("instance_frame_exported"),
        "actingctl observe --export with instance {selector}: {outcome}"
    );
    line
}

/// The instance_id the Runtime's status lists for the fixture's alias.
fn registered_instance_id(root: &Path) -> String {
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        root,
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect a status client");
    let status = client.status().expect("read the Runtime status");
    drop(client);
    let instance = status
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == ALIAS)
        .unwrap_or_else(|| panic!("the Runtime status lists no instance {ALIAS}"));
    serde_json::to_value(instance.instance_id())
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .expect("an instance_id is a JSON string")
}
