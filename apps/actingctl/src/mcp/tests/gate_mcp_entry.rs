// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #421 MCP-ENTRY, test row E-12: `actingctl mcp-config --entry` renders each of the
//! four clients' snippets pointing at `<root>\mcp-entry\g1\actingcommand-mcp-entry.exe`, with the
//! entry's `--root` and `--client` and the child's `mcp-serve --tier …` after `--` unchanged.

use super::super::{Client, entry_snippet};
use serde_json::{Value, json};

const ROOT: &str = r"X:\TestRoot";
const ENTRY: &str = r"X:\TestRoot\mcp-entry\g1\actingcommand-mcp-entry.exe";

fn args(client: &str) -> Value {
    json!([
        "--root",
        ROOT,
        "--client",
        client,
        "--",
        "mcp-serve",
        "--tier",
        "observer,operator,author"
    ])
}

fn server(client: Client) -> Value {
    let text = entry_snippet(client, ROOT, "observer,operator,author");
    let snippet: Value = serde_json::from_str(&text).expect("a JSON snippet");
    snippet["mcpServers"]["actingcommand"].clone()
}

#[test]
fn e12_mcp_config_entry_renders_each_clients_snippet() {
    assert_eq!(
        server(Client::Claude),
        json!({"type": "stdio", "command": ENTRY, "args": args("claude"), "env": {}})
    );
    assert_eq!(
        server(Client::Antigravity),
        json!({"command": ENTRY, "args": args("antigravity"), "timeoutSeconds": 60})
    );
    assert_eq!(
        server(Client::Dsh),
        json!({"command": ENTRY, "args": args("dsh")})
    );
    assert_eq!(
        entry_snippet(Client::Codex, ROOT, "observer,operator,author"),
        "[mcp_servers.actingcommand]\n\
         command = 'X:\\TestRoot\\mcp-entry\\g1\\actingcommand-mcp-entry.exe'\n\
         args = ['--root', 'X:\\TestRoot', '--client', 'codex', '--', 'mcp-serve', '--tier', 'observer,operator,author']\n\
         startup_timeout_sec = 10\n\
         tool_timeout_sec = 60\n"
    );
    // A trailing separator on --root does not double the entry's path separator.
    let text = entry_snippet(Client::Dsh, r"X:\TestRoot\", "observer");
    assert!(text.contains(r#""X:\\TestRoot\\mcp-entry\\g1\\actingcommand-mcp-entry.exe""#));
}
