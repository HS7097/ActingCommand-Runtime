// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5G5b (register §1.287, "Slice LAB-5G5b"; contracts/install-cli.md, "The MCP
//! entry", "Registration"): the rescue row `mcp config`, the registration snippet of the MCP
//! entry's second generation for one client.
//!
//! - **The clients and the shapes** are those `actingctl mcp-config --entry` prints for the first
//!   generation (`apps/actingctl/src/mcp/mod.rs`, `entry_snippet`; copied here, actingctl is not
//!   changed): Claude Code's and Antigravity's `mcpServers` member (Antigravity with
//!   `timeoutSeconds`), Codex's `[mcp_servers.actingcommand]` table, and for DSH the `command` and
//!   `args` of its `actingcommand` entry.
//! - **What differs**: the program is `<root>\mcp-entry\g2\actingcommand-mcp-entry-g2.exe`; its
//!   words are `--root <root> --client <client> --`, and after `--` only `--list <modules>` when
//!   the row is given `list`. No `--tier`: the server has none.
//! - **`list`** takes what the server's `--list` takes, by the table's module rows of role
//!   `operations`: a word that names none (an empty word too), and a list without a word, is
//!   refused `not_recognised`, a word given twice `repeated`, each naming `--list`, as the server
//!   refuses them.
//! - **It prints the snippet only**: it writes no client file and reads none (changing a
//!   registration is the owner's act). A program not placed in `g2\` is no refusal: the answer
//!   gives the snippet and carries the warning `install_caveat_noted` (`mcp_entry_not_placed`,
//!   the program as `path`), a `note` of its outcome and its one `warnings` item.

use super::{Answer, caveat, done};
use crate::cli_line;
use crate::codes::{ArgumentProblem, InstallCaveat, InstallLocation};
use actingcommand_contract::outcome::{CauseRelation, Outcome};
use actingcommand_lab_ops::table::{self, ModuleRole};
use serde_json::json;
use std::path::Path;

/// The second generation's folder under `<root>\mcp-entry\`, and its program.
const ENTRY_GENERATION: u32 = 2;
const ENTRY_PROGRAM: &str = "actingcommand-mcp-entry-g2.exe";
/// Antigravity's `timeoutSeconds`, as `actingctl mcp-config` writes it: a call's 25 s budget with
/// the margin Codex's `tool_timeout_sec` (60) gives it.
const ANTIGRAVITY_TIMEOUT_SECONDS: u64 = 60;
/// The server's option that names the modules whose tools it lists.
const LIST: &str = "--list";

/// A client `mcp config` writes a snippet for (the words `--client` takes, the entry's too).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Client {
    Claude,
    Codex,
    Antigravity,
    Dsh,
}

impl Client {
    fn parse(word: &str) -> Option<Self> {
        match word {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "antigravity" => Some(Self::Antigravity),
            "dsh" => Some(Self::Dsh),
            _ => None,
        }
    }
}

/// The rescue row `mcp config` for the root `root` (its plain spelling), the client `client` and,
/// when given, the server's `--list` words `list` in their order: `{client, entry, snippet}` (the
/// snippet's text, as the client takes it), with the warning of a program not placed. A client
/// it does not know is `actingctl_usage_invalid` (`--client`, `not_recognised`); the module's
/// description says which `list` is refused.
pub(crate) fn snippet(
    root: &str,
    client: &str,
    list: Option<&[String]>,
) -> Result<Answer, Outcome> {
    let operation = InstallLocation::McpConfig;
    let kind = Client::parse(client)
        .ok_or_else(|| cli_line::usage_invalid(Some("--client"), ArgumentProblem::NotRecognised))?;
    let base = root.trim_end_matches(['\\', '/']);
    let entry = format!(r"{base}\mcp-entry\g{ENTRY_GENERATION}\{ENTRY_PROGRAM}");
    let mut args = vec![
        "--root".to_owned(),
        root.to_owned(),
        "--client".to_owned(),
        client.to_owned(),
        "--".to_owned(),
    ];
    if let Some(words) = list {
        args.push(LIST.to_owned());
        args.push(listed_modules(words)?);
    }
    let mut outcome = done(operation);
    let mut warnings = Vec::new();
    if !Path::new(&entry).is_file() {
        let not_placed = caveat(operation, InstallCaveat::McpEntryNotPlaced).path(&entry);
        warnings.push(cli_line::warning(&not_placed));
        outcome = outcome.link(CauseRelation::Note, not_placed);
    }
    let answer = Answer::new(outcome)
        .with("client", client)?
        .with("entry", &entry)?
        .with("snippet", text(kind, &entry, &args))?;
    if warnings.is_empty() {
        return Ok(answer);
    }
    answer.with("warnings", warnings)
}

/// The value of the server's `--list` for `words`: each the id of a module row of role
/// `operations` of the table this build embeds, at most once, joined by commas in their order.
fn listed_modules(words: &[String]) -> Result<String, Outcome> {
    let refused = |problem: ArgumentProblem| cli_line::usage_invalid(Some(LIST), problem);
    if words.is_empty() {
        return Err(refused(ArgumentProblem::NotRecognised));
    }
    let table = table::operation_table()?;
    let modules = table
        .modules
        .iter()
        .filter(|module| module.role == ModuleRole::Operations)
        .map(|module| module.id.as_str())
        .collect::<Vec<_>>();
    let mut named: Vec<&str> = Vec::new();
    for word in words {
        if !modules.contains(&word.as_str()) {
            return Err(refused(ArgumentProblem::NotRecognised));
        }
        if named.contains(&word.as_str()) {
            return Err(refused(ArgumentProblem::Repeated));
        }
        named.push(word.as_str());
    }
    Ok(named.join(","))
}

/// The snippet for `client`, in its own shape, around the program `entry` and its words `args`
/// (as `actingctl mcp-config`'s `entry_snippet` writes them).
fn text(client: Client, entry: &str, args: &[String]) -> String {
    let server = match client {
        Client::Claude => json!({"type": "stdio", "command": entry, "args": args, "env": {}}),
        Client::Antigravity => json!({
            "command": entry,
            "args": args,
            "timeoutSeconds": ANTIGRAVITY_TIMEOUT_SECONDS,
        }),
        Client::Dsh => json!({"command": entry, "args": args}),
        Client::Codex => {
            let listed = args
                .iter()
                .map(|arg| toml_string(arg.as_str()))
                .collect::<Vec<_>>()
                .join(", ");
            return format!(
                "[mcp_servers.actingcommand]\ncommand = {}\nargs = [{listed}]\nstartup_timeout_sec = 10\ntool_timeout_sec = 60\n",
                toml_string(entry)
            );
        }
    };
    let snippet = json!({"mcpServers": {"actingcommand": server}});
    let mut text = serde_json::to_string_pretty(&snippet).unwrap_or_else(|_| snippet.to_string());
    text.push('\n');
    text
}

/// A TOML literal string, or a basic string when the value holds a single quote.
fn toml_string(value: &str) -> String {
    if value.contains('\'') {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("'{value}'")
    }
}
