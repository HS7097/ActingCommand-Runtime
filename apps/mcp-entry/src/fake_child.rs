// SPDX-License-Identifier: AGPL-3.0-only

//! Test helper of `tests/entry_process.rs` (Workflow #421 MCP-ENTRY): a fake `actingctl`.
//! Copied to `<root>\runtime\actingctl.exe` of a temporary root, it answers
//! `mcp-entry plan --root <root> [--current-build <commit>] -- <arguments>` from
//! `<root>\runtime\BUILD-MANIFEST.json`, and otherwise serves a minimal MCP whose one tool is
//! named after the build it started with. Never shipped.

use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

fn main() {
    let words = std::env::args().skip(1).collect::<Vec<_>>();
    if words.first().map(String::as_str) == Some("mcp-entry") {
        plan(&words);
        return;
    }
    serve();
}

fn manifest_commit(runtime: &Path) -> String {
    std::fs::read(runtime.join("BUILD-MANIFEST.json"))
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
    let flag = |name: &str| {
        words
            .iter()
            .position(|word| word == name)
            .and_then(|index| words.get(index + 1))
            .cloned()
    };
    let root = PathBuf::from(flag("--root").unwrap_or_default());
    let runtime = root.join("runtime");
    let commit = manifest_commit(&runtime);
    let current = flag("--current-build");
    let switch = current.as_ref().is_some_and(|current| *current != commit);
    let mut child = vec![runtime.join("actingctl.exe").to_string_lossy().into_owned()];
    if let Some(split) = words.iter().position(|word| word == "--") {
        child.extend(words[split + 1..].iter().cloned());
    }
    println!(
        "{}",
        json!({
            "switch": switch,
            "build": {"commit": commit, "manifest_sha256": "0".repeat(64)},
            "child": child,
            "tunables": {"poll_ms": 50, "settle_ms": 0, "verify_timeout_ms": 10_000, "drain_max_ms": 60_000, "refused_retry_max_ms": 300_000},
            "entry_protocol": 1,
        })
    );
}

fn serve() {
    let runtime = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let commit = manifest_commit(&runtime);
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
                json!({"content": [{"type": "text", "text": std::process::id().to_string()}]})
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
