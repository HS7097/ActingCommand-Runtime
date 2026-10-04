// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl mcp-serve` and `actingctl mcp-config` (Workflow #338 S1). The server is a thin
//! stdio MCP layer for Claude Code and Codex: protocol, tier gate, composition, paging and
//! output budget. It decides no business rule; what a run is, whether it may run and
//! whether it is done come from the Runtime and runtime-client unchanged.

mod child;
mod jobs;
mod observer;
mod operator;
mod protocol;
mod runs;
mod runtime;
mod server;
mod tools;

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Mutex, MutexGuard, PoisonError};
use tools::TierSet;

const SERVE_USAGE: &str = "usage: actingctl mcp-serve [--root <install root>] [--state-root <dir>] [--tier observer|operator|author[,...]] | actingctl mcp-serve --list-tools [--format json|markdown]";
const CONFIG_USAGE: &str =
    "usage: actingctl mcp-config --client claude|codex [--tier observer|operator|author[,...]]";

/// Runs `mcp-serve` or `mcp-config` when the first argument names one; `None` leaves every
/// other command to the CLI, unchanged.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    let command = arguments.first()?.to_str()?;
    let result = match command {
        "mcp-serve" => serve(&arguments[1..]),
        "mcp-config" => config(&arguments[1..]),
        _ => return None,
    };
    Some(result.unwrap_or_else(|message| {
        eprintln!("FATAL actingctl {command}: {message}");
        ExitCode::FAILURE
    }))
}

/// A mutex whose holder panicked still guards consistent data here: every critical
/// section only inserts, removes or sets whole values.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

enum ListFormat {
    Json,
    Markdown,
}

fn serve(arguments: &[OsString]) -> Result<ExitCode, String> {
    let mut root = None;
    let mut state_root = None;
    let mut tiers = None;
    let mut list_tools = false;
    let mut format = None;
    let mut index = 0;
    while index < arguments.len() {
        match flag(arguments, index, SERVE_USAGE)? {
            "--root" if root.is_none() => {
                root = Some(PathBuf::from(value(arguments, &mut index, SERVE_USAGE)?));
            }
            "--state-root" if state_root.is_none() => {
                state_root = Some(PathBuf::from(value(arguments, &mut index, SERVE_USAGE)?));
            }
            "--tier" if tiers.is_none() => {
                tiers = Some(parse_tiers(arguments, &mut index, SERVE_USAGE)?);
            }
            "--list-tools" if !list_tools => list_tools = true,
            "--format" if format.is_none() => {
                format = Some(match value(arguments, &mut index, SERVE_USAGE)?.to_str() {
                    Some("json") => ListFormat::Json,
                    Some("markdown") => ListFormat::Markdown,
                    _ => return Err(SERVE_USAGE.to_owned()),
                });
            }
            _ => return Err(SERVE_USAGE.to_owned()),
        }
        index += 1;
    }
    if list_tools {
        // The whole build table, every tier; tools/list serves the enabled tiers only.
        if root.is_some() || state_root.is_some() || tiers.is_some() {
            return Err(SERVE_USAGE.to_owned());
        }
        let text = match format.unwrap_or(ListFormat::Json) {
            ListFormat::Json => serde_json::to_string_pretty(&tools::list_json())
                .map_err(|error| format!("cannot encode the tool table: {error}"))?,
            ListFormat::Markdown => tools::list_markdown(),
        };
        print_text(&text)?;
        return Ok(ExitCode::SUCCESS);
    }
    if format.is_some() {
        return Err(SERVE_USAGE.to_owned());
    }
    server::serve(server::ServerConfig {
        root,
        state_root,
        tiers: tiers.unwrap_or_default(),
    })
}

enum Client {
    Claude,
    Codex,
}

/// Prints the client configuration for this executable; it writes no client file.
fn config(arguments: &[OsString]) -> Result<ExitCode, String> {
    let mut client = None;
    let mut tiers = None;
    let mut index = 0;
    while index < arguments.len() {
        match flag(arguments, index, CONFIG_USAGE)? {
            "--client" if client.is_none() => {
                client = Some(match value(arguments, &mut index, CONFIG_USAGE)?.to_str() {
                    Some("claude") => Client::Claude,
                    Some("codex") => Client::Codex,
                    _ => return Err(CONFIG_USAGE.to_owned()),
                });
            }
            "--tier" if tiers.is_none() => {
                tiers = Some(parse_tiers(arguments, &mut index, CONFIG_USAGE)?);
            }
            _ => return Err(CONFIG_USAGE.to_owned()),
        }
        index += 1;
    }
    let client = client.ok_or_else(|| CONFIG_USAGE.to_owned())?;
    let tiers: TierSet = tiers.unwrap_or_default();
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot find the actingctl executable: {error}"))?;
    let executable = executable
        .to_str()
        .ok_or_else(|| "the actingctl path is not valid Unicode".to_owned())?;
    let tier_argument = tiers.argument();
    let text = match client {
        Client::Claude => {
            if executable.contains('"') {
                return Err("the actingctl path contains a double quote".to_owned());
            }
            format!(
                "claude mcp add --scope user actingcommand -- \"{executable}\" mcp-serve --tier {tier_argument}\n"
            )
        }
        Client::Codex => {
            let command = toml_string(executable);
            let enabled = tools::TOOLS
                .iter()
                .filter(|tool| tiers.contains(tool.tier))
                .map(|tool| format!("'{}'", tool.name))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "[mcp_servers.actingcommand]\ncommand = {command}\nargs = ['mcp-serve','--tier','{tier_argument}']\nstartup_timeout_sec = 10\ntool_timeout_sec = 60\n# Optional second layer: let Codex expose only some tools, for example\n# enabled_tools = [{enabled}]\n"
            )
        }
    };
    print_text(&text)?;
    Ok(ExitCode::SUCCESS)
}

/// A TOML literal string, or a basic string when the value holds a single quote.
fn toml_string(value: &str) -> String {
    if value.contains('\'') {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("'{value}'")
    }
}

fn flag<'a>(arguments: &'a [OsString], index: usize, usage: &str) -> Result<&'a str, String> {
    arguments[index].to_str().ok_or_else(|| usage.to_owned())
}

fn value<'a>(
    arguments: &'a [OsString],
    index: &mut usize,
    usage: &str,
) -> Result<&'a OsString, String> {
    *index += 1;
    arguments.get(*index).ok_or_else(|| usage.to_owned())
}

fn parse_tiers(arguments: &[OsString], index: &mut usize, usage: &str) -> Result<TierSet, String> {
    value(arguments, index, usage)?
        .to_str()
        .and_then(TierSet::parse)
        .ok_or_else(|| usage.to_owned())
}

fn print_text(text: &str) -> Result<(), String> {
    write_text(&mut io::stdout().lock(), text)
        .map_err(|error| format!("cannot write stdout: {error}"))
}

fn write_text(output: &mut impl Write, text: &str) -> io::Result<()> {
    output.write_all(text.as_bytes())?;
    if !text.ends_with('\n') {
        output.write_all(b"\n")?;
    }
    output.flush()
}
