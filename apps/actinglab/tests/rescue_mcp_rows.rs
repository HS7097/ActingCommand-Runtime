// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5G5b (register §1.287, "Slice LAB-5G5b"; contracts/install-cli.md, "The MCP
//! entry"; contracts/lab-two-entries.md §7 and §8.2): the rescue module's three MCP rows, `mcp
//! plan`, `mcp entries` and `mcp config`, are served through the operation table as its other
//! rescue rows, and answer the same line on actinglab's command line (`--json`) and in `serve
//! --stdio`. The real binary runs as `tools\actinglab.exe` of a made-up single-location root in a
//! temporary folder, where the rescue module finds its root as in an installation: the neutral
//! manifests of one build whose Tools member carries a neutral file as the server program, a
//! configuration, and one status file of each generation of the MCP entry whose pid no process can
//! have. No Runtime runs; nothing else is started.

#![cfg(windows)]

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

/// The build installed at the made-up root, and another: the entry's current child's.
const BUILD: &str = "2222222222222222222222222222222222222222";
const CURRENT: &str = "1111111111111111111111111111111111111111";
/// The neutral bytes that stand for the server program in the Tools member.
const SERVER: &[u8] = b"neutral server";
/// A pid above every pid a process can have (they are 32-bit): no probe is asked about it, and
/// its status file is stale.
const NO_PID: u64 = 5_000_000_000;

/// Writes `bytes` to `path`, its folders made first.
fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("the folder");
    fs::write(path, bytes).expect("the file");
}

/// The made-up root in `base` and actinglab placed in it as `tools\actinglab.exe`.
fn made_up_root(base: &Path) -> (PathBuf, PathBuf) {
    let root = base.join("root");
    let runtime = json!({
        "repository": "neutral/runtime",
        "commit_sha": BUILD,
        "files": [],
        "runtime_payload_layout": "distribution-v1",
    });
    write(
        &root.join("runtime").join("BUILD-MANIFEST.json"),
        runtime.to_string().as_bytes(),
    );
    let tools = json!({
        "repository": "neutral/runtime",
        "commit_sha": BUILD,
        "files": [{
            "path": "actinglab-mcp.exe",
            "size_bytes": SERVER.len(),
            "sha256": format!("{:x}", Sha256::digest(SERVER)),
        }],
        "tools_payload_layout": "platform-tools-v3",
    });
    write(
        &root.join("tools").join("BUILD-MANIFEST.json"),
        tools.to_string().as_bytes(),
    );
    write(&root.join("tools").join("actinglab-mcp.exe"), SERVER);
    let config = json!({"state_root": base.join("state"), "instances": []});
    write(
        &root.join("actingd.config.json"),
        config.to_string().as_bytes(),
    );
    for generation in [1, 2] {
        let status = json!({
            "schema_version": "actingcommand.mcp-entry-status.v1",
            "entry_generation": generation,
            "entry_protocol": generation,
            "pid": NO_PID,
            "started_at_unix_ms": 1_000,
        });
        write(
            &root
                .join("mcp-entry")
                .join("state")
                .join(format!("entry-{generation}.json")),
            status.to_string().as_bytes(),
        );
    }
    let program = root.join("tools").join("actinglab.exe");
    fs::copy(env!("CARGO_BIN_EXE_actinglab"), &program).expect("actinglab placed in the root");
    (root, program)
}

/// actinglab at `program`, its environment without an initiator or an outcome detail.
fn actinglab(program: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .env_remove("ACTINGCOMMAND_INITIATOR")
        .env_remove("ACTINGCOMMAND_DETAIL");
    command
}

/// The non-empty lines of `bytes`, each read as JSON.
fn json_lines(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8(bytes.to_vec())
        .expect("UTF-8")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .collect()
}

/// The one line `actinglab --json <words>` prints.
fn command_line(program: &Path, words: &[&str]) -> Value {
    let output = actinglab(program)
        .arg("--json")
        .args(words)
        .stdin(Stdio::null())
        .output()
        .expect("actinglab runs");
    let mut lines = json_lines(&output.stdout);
    assert_eq!(
        lines.len(),
        1,
        "{words:?}: one line; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    lines.remove(0)
}

/// The lines `actinglab serve --stdio` answers to `requests`, one line each.
fn served(program: &Path, requests: &[Value]) -> Vec<Value> {
    let mut child = actinglab(program)
        .args(["serve", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("serve starts");
    let mut input = child.stdin.take().expect("its input");
    for request in requests {
        writeln!(input, "{request}").expect("a request line");
    }
    drop(input);
    let output = child.wait_with_output().expect("serve ends");
    assert!(
        output.status.success(),
        "serve ends at the end of its input: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    json_lines(&output.stdout)
}

#[test]
fn mcp_rows_answer_the_same_line_on_the_command_line_and_in_serve() {
    let temp = TempDir::new().expect("tempdir");
    let (root, program) = made_up_root(temp.path());
    let rows: [(&[&str], Value); 3] = [
        (
            &[
                "mcp",
                "plan",
                "--entry-protocol",
                "2",
                "--current-build",
                CURRENT,
            ],
            json!({"op": "mcp plan", "params": {"entry_protocol": 2, "current_build": CURRENT}}),
        ),
        (
            &["mcp", "entries", "--all"],
            json!({"op": "mcp entries", "params": {"all": true}}),
        ),
        (
            &[
                "mcp",
                "config",
                "--client",
                "codex",
                "--list",
                "core,rescue",
            ],
            json!({"op": "mcp config", "params": {"client": "codex", "list": ["core", "rescue"]}}),
        ),
    ];
    let requests = rows
        .iter()
        .map(|(_, request)| request.clone())
        .collect::<Vec<_>>();
    let answered = served(&program, &requests);
    assert_eq!(answered.len(), rows.len(), "one line each: {answered:?}");
    for ((words, request), line) in rows.iter().zip(&answered) {
        assert_eq!(&command_line(&program, words), line, "{request}");
    }

    // What the lines say. The plan: the build, a switch from the current build, the server the
    // build carries, the second generation's protocol.
    let plan = &answered[0];
    assert_eq!(plan["outcome"]["code"], "install_command_done", "{plan}");
    assert_eq!(plan["entry_protocol"], 2, "{plan}");
    assert_eq!(plan["switch"], true, "{plan}");
    assert_eq!(plan["build"]["commit"], BUILD, "{plan}");
    let child = plan["child"][0].as_str().expect("the server program");
    assert!(
        child.ends_with(r"\tools\actinglab-mcp.exe") && Path::new(child).is_file(),
        "{plan}"
    );
    // The entries: both generations' status files, side by side.
    let entries = answered[1]["entries"].as_array().expect("the entries");
    let generations = entries
        .iter()
        .map(|row| row["generation"].clone())
        .collect::<Vec<_>>();
    assert_eq!(generations, [json!(1), json!(2)], "{}", answered[1]);
    // The snippet: the second generation's program and `--list`, no tier, and the warning that
    // the program is not placed.
    let config = &answered[2];
    let snippet = config["snippet"].as_str().expect("the snippet");
    assert!(
        snippet.contains(r"\mcp-entry\g2\actingcommand-mcp-entry-g2.exe")
            && snippet.contains("'--', '--list', 'core,rescue']")
            && !snippet.contains("--tier"),
        "{snippet}"
    );
    assert_eq!(
        config["warnings"][0]["outcome"]["values"]["install_caveat"], "mcp_entry_not_placed",
        "{config}"
    );
    assert!(
        !root.join("mcp-entry").join("g2").exists(),
        "the rows wrote nothing"
    );
}
