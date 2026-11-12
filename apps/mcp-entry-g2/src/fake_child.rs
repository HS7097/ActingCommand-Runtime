// SPDX-License-Identifier: AGPL-3.0-only

//! Test helper of `tests/entry_process.rs` (Workflow #434 LAB-5G5a, copied from the first
//! generation's): a fake of the second generation's two programs. Copied to
//! `<root>\tools\actinglab.exe` of a temporary root, it answers
//! `--json mcp plan --entry-protocol 2 [--current-build <commit>]` for the root it is in from
//! `<root>\runtime\BUILD-MANIFEST.json`, naming the `actinglab-mcp.exe` the entry would find as
//! the child. Copied to `<root>\tools\actinglab-mcp.exe`, it serves a minimal MCP whose one tool
//! is named after the build it started with and whose call answers its pid and its arguments.
//! Never shipped.

use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

fn main() {
    let words = std::env::args().skip(1).collect::<Vec<_>>();
    let plan_words =
        words.len() >= 3 && words[0] == "--json" && words[1] == "mcp" && words[2] == "plan";
    if plan_words {
        plan(&words);
        return;
    }
    serve(&words);
}

/// The root of the program: the folder above the `runtime\` or `tools\` it runs from.
fn own_root() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().and_then(Path::parent).map(Path::to_path_buf))
        .unwrap_or_default()
}

/// `<root>\runtime\<name>` when it is a file, else `<root>\tools\<name>`, as the entry finds it.
fn find_program(root: &Path, name: &str) -> PathBuf {
    let runtime = root.join("runtime").join(name);
    if runtime.is_file() {
        return runtime;
    }
    root.join("tools").join(name)
}

fn manifest_commit(root: &Path) -> String {
    std::fs::read(root.join("runtime").join("BUILD-MANIFEST.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|manifest| {
            manifest
                .get("commit_sha")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

fn plan(words: &[String]) {
    let current = words
        .iter()
        .position(|word| word == "--current-build")
        .and_then(|index| words.get(index + 1))
        .cloned();
    let root = own_root();
    let commit = manifest_commit(&root);
    let switch = current.as_ref().is_some_and(|current| *current != commit);
    let child = vec![
        find_program(&root, "actinglab-mcp.exe")
            .to_string_lossy()
            .into_owned(),
    ];
    println!(
        "{}",
        json!({
            "switch": switch,
            "build": {"commit": commit, "manifest_sha256": "0".repeat(64)},
            "child": child,
            "tunables": {"poll_ms": 50, "settle_ms": 0, "verify_timeout_ms": 10_000, "drain_max_ms": 60_000, "refused_retry_max_ms": 300_000},
            "entry_protocol": 2,
        })
    );
}

fn serve(arguments: &[String]) {
    let commit = manifest_commit(&own_root());
    let tool = format!("fake_{}", commit.get(..8).unwrap_or(&commit));
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            return;
        };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (Some(id), Some(method)) = (
            message.get("id").cloned(),
            message.get("method").and_then(Value::as_str),
        ) else {
            continue;
        };
        let result = match method {
            "initialize" => json!({
                "protocolVersion": message.pointer("/params/protocolVersion").cloned().unwrap_or(Value::Null),
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "fake-child", "version": "1"},
            }),
            "tools/list" => json!({"tools": [{"name": tool, "inputSchema": {"type": "object"}}]}),
            "tools/call" => {
                let served = json!({"pid": std::process::id(), "args": arguments});
                json!({"content": [{"type": "text", "text": served.to_string()}]})
            }
            "actingcommand/entryStatus" => {
                json!({"outstanding_handles": 0, "kinds": [], "calls_in_flight": 0})
            }
            _ => {
                let mut failure = serde_json::Map::new();
                failure.insert("code".to_owned(), json!(-32601));
                failure.insert("message".to_owned(), json!("unknown"));
                let error = json!({"jsonrpc": "2.0", "id": id, "error": failure});
                if writeln!(stdout, "{error}")
                    .and_then(|()| stdout.flush())
                    .is_err()
                {
                    return;
                }
                continue;
            }
        };
        let answer = json!({"jsonrpc": "2.0", "id": id, "result": result});
        if writeln!(stdout, "{answer}")
            .and_then(|()| stdout.flush())
            .is_err()
        {
            return;
        }
    }
}
