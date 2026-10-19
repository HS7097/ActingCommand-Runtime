// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl mcp-serve` and `actingctl mcp-config` (Workflow #338 S1). The server is a thin
//! stdio MCP layer for Claude Code and Codex: protocol, tier gate, composition, paging and
//! output budget. It decides no business rule; what a run is, whether it may run and
//! whether it is done come from the Runtime and runtime-client unchanged.

// Workflow #389 MCP-7: the child-process runner moved to the crate, where `actingctl diagnose`
// runs `actingd suspended` with it too.
use crate::child;
// Workflow #410 item 6: ac_capability_gap.
mod capability_gap;
// Workflow #389 MCP-14: ac_outcome_code.
mod code_lookup;
// Workflow #398 RD-1c: ac_devices, ac_device_bind, ac_device_unbind.
mod devices;
mod events;
// Workflow #389 MCP-4: ac_facts.
mod facts;
mod jobs;
mod lab;
// Workflow #389 MCP-5a: ac_observe.
mod observe_export;
mod observer;
mod operator;
mod packs;
mod protocol;
// Workflow #389 MCP-3: ac_runs.
mod run_list;
// Workflow #389 MCP-6: ac_run_evidence.
mod run_evidence;
mod runs;
mod runtime;
mod server;
// Workflow #387 D1b: ac_config_set.
mod settings;
mod tools;

use crate::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{Detail, IoOp, Outcome, RawSource};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Mutex, MutexGuard, PoisonError};
use tools::TierSet;

const SERVE_USAGE: &str = "usage: actingctl mcp-serve [--root <install root>] [--state-root <dir>] [--tier observer|operator|author[,...]] [--detail codes|full] | actingctl mcp-serve --list-tools [--format json|markdown]";
const CONFIG_USAGE: &str =
    "usage: actingctl mcp-config --client claude|codex [--tier observer|operator|author[,...]]";

/// Why `mcp-serve` or `mcp-config` ended without doing its work (Workflow #378 A5, model v2.2
/// §7.3): one outcome, and for a usage error the usage line. Neither command prints an
/// outcome line on stdout (stdout is the protocol or the printed configuration): the developer
/// line on stderr carries it, and the process exits by its category.
struct Ended {
    outcome: Outcome,
    usage: Option<&'static str>,
}

impl From<Outcome> for Ended {
    fn from(outcome: Outcome) -> Self {
        Self {
            outcome,
            usage: None,
        }
    }
}

/// Runs `mcp-serve` or `mcp-config` when the first argument names one; `None` leaves every
/// other command to the CLI, unchanged.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    let command = arguments.first()?.to_str()?;
    let result = match command {
        "mcp-serve" => serve(&arguments[1..]),
        "mcp-config" => config(&arguments[1..]),
        _ => return None,
    };
    Some(result.unwrap_or_else(|ended| {
        crate::cli_line::developer_line(&ended.outcome, ended.usage);
        ExitCode::from(crate::cli_line::exit_code(ended.outcome.category(), false))
    }))
}

/// `actingctl_usage_invalid` for one flag (`argument`, when it is a word a name may hold) and
/// what is wrong with it (`argument_problem`), with the usage line for the developer line.
fn usage_error(usage: &'static str, problem: ArgumentProblem, flag: Option<&str>) -> Ended {
    Ended {
        outcome: crate::cli_line::usage_invalid(flag, problem),
        usage: Some(usage),
    }
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

fn serve(arguments: &[OsString]) -> Result<ExitCode, Ended> {
    let mut root = None;
    let mut state_root = None;
    let mut tiers = None;
    let mut list_tools = false;
    let mut format = None;
    let mut detail = None;
    let mut index = 0;
    while index < arguments.len() {
        let word = flag(arguments, index, SERVE_USAGE)?;
        match word {
            "--root" if root.is_none() => {
                root = Some(PathBuf::from(value(
                    arguments,
                    &mut index,
                    word,
                    SERVE_USAGE,
                )?));
            }
            "--state-root" if state_root.is_none() => {
                state_root = Some(PathBuf::from(value(
                    arguments,
                    &mut index,
                    word,
                    SERVE_USAGE,
                )?));
            }
            "--tier" if tiers.is_none() => {
                tiers = Some(parse_tiers(arguments, &mut index, word, SERVE_USAGE)?);
            }
            "--list-tools" if !list_tools => list_tools = true,
            "--format" if format.is_none() => {
                format = Some(
                    match value(arguments, &mut index, word, SERVE_USAGE)?.to_str() {
                        Some("json") => ListFormat::Json,
                        Some("markdown") => ListFormat::Markdown,
                        _ => {
                            return Err(usage_error(
                                SERVE_USAGE,
                                ArgumentProblem::NotRecognised,
                                Some(word),
                            ));
                        }
                    },
                );
            }
            // Workflow #378 A5: the outcome detail of every tool error and warning.
            "--detail" if detail.is_none() => {
                detail = Some(
                    match value(arguments, &mut index, word, SERVE_USAGE)?.to_str() {
                        Some("codes") => Detail::Codes,
                        Some("full") => Detail::Full,
                        _ => {
                            return Err(usage_error(
                                SERVE_USAGE,
                                ArgumentProblem::NotRecognised,
                                Some(word),
                            ));
                        }
                    },
                );
            }
            "--root" | "--state-root" | "--tier" | "--list-tools" | "--format" | "--detail" => {
                return Err(usage_error(
                    SERVE_USAGE,
                    ArgumentProblem::Repeated,
                    Some(word),
                ));
            }
            _ => {
                return Err(usage_error(
                    SERVE_USAGE,
                    ArgumentProblem::Unknown,
                    Some(word),
                ));
            }
        }
        index += 1;
    }
    if list_tools {
        // The whole build table, every tier; tools/list serves the enabled tiers only.
        let conflicting = [
            ("--root", root.is_some()),
            ("--state-root", state_root.is_some()),
            ("--tier", tiers.is_some()),
            ("--detail", detail.is_some()),
        ]
        .into_iter()
        .find_map(|(flag, given)| given.then_some(flag));
        if let Some(flag) = conflicting {
            return Err(usage_error(
                SERVE_USAGE,
                ArgumentProblem::Conflicting,
                Some(flag),
            ));
        }
        let text = match format.unwrap_or(ListFormat::Json) {
            ListFormat::Json => {
                serde_json::to_string_pretty(&tools::list_json()).map_err(|error| {
                    Outcome::new(ActingctlCode::ActingctlOutputFailed).caused_by(
                        Outcome::new(ContractCode::ForeignDataError)
                            .raw_source(RawSource::Json)
                            .raw_text(error.to_string()),
                    )
                })?
            }
            ListFormat::Markdown => tools::list_markdown(),
        };
        print_text(&text)?;
        return Ok(ExitCode::SUCCESS);
    }
    if format.is_some() {
        return Err(usage_error(
            SERVE_USAGE,
            ArgumentProblem::Conflicting,
            Some("--format"),
        ));
    }
    // Without --detail, ACTINGCOMMAND_DETAIL or full (class (b), model v2.2 §7.1).
    let detail = match detail {
        Some(detail) => detail,
        None => crate::cli_line::detail().map_err(|outcome| Ended {
            outcome,
            usage: Some(SERVE_USAGE),
        })?,
    };
    tools::set_detail(detail);
    server::serve(server::ServerConfig {
        root,
        state_root,
        tiers: tiers.unwrap_or_default(),
    })
    .map_err(Ended::from)
}

enum Client {
    Claude,
    Codex,
}

/// Prints the client configuration for this executable; it writes no client file.
fn config(arguments: &[OsString]) -> Result<ExitCode, Ended> {
    let mut client = None;
    let mut tiers = None;
    let mut index = 0;
    while index < arguments.len() {
        let word = flag(arguments, index, CONFIG_USAGE)?;
        match word {
            "--client" if client.is_none() => {
                client = Some(
                    match value(arguments, &mut index, word, CONFIG_USAGE)?.to_str() {
                        Some("claude") => Client::Claude,
                        Some("codex") => Client::Codex,
                        _ => {
                            return Err(usage_error(
                                CONFIG_USAGE,
                                ArgumentProblem::NotRecognised,
                                Some(word),
                            ));
                        }
                    },
                );
            }
            "--tier" if tiers.is_none() => {
                tiers = Some(parse_tiers(arguments, &mut index, word, CONFIG_USAGE)?);
            }
            "--client" | "--tier" => {
                return Err(usage_error(
                    CONFIG_USAGE,
                    ArgumentProblem::Repeated,
                    Some(word),
                ));
            }
            _ => {
                return Err(usage_error(
                    CONFIG_USAGE,
                    ArgumentProblem::Unknown,
                    Some(word),
                ));
            }
        }
        index += 1;
    }
    let client = client
        .ok_or_else(|| usage_error(CONFIG_USAGE, ArgumentProblem::Required, Some("--client")))?;
    let tiers: TierSet = tiers.unwrap_or_default();
    let program = registered_program()?;
    // A path no configuration line can carry: not Unicode, or (for claude) a double quote.
    let unsupported = || Outcome::new(ActingctlCode::McpConfigProgramUnsupported).program(&program);
    let executable = program.to_str().ok_or_else(unsupported)?;
    let tier_argument = tiers.argument();
    let text = match client {
        Client::Claude => {
            if executable.contains('"') {
                return Err(unsupported().into());
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

/// The program a client registers (#359 item 7). Run from a slot of an A/B installation,
/// that is the fixed entry `<root>\runtime\actingctl.exe`, which forwards to the active
/// slot; a slot path stops working after a switch. Otherwise it is this executable. When it
/// cannot be named, `mcp_config_program_unresolved` says what is known (`program`, and for a
/// slot `install_root` and `entry`), caused by the failure behind it.
fn registered_program() -> Result<PathBuf, Outcome> {
    let unresolved = || Outcome::new(ActingctlCode::McpConfigProgramUnresolved);
    let executable = std::env::current_exe()
        .map_err(|error| unresolved().caused_by(Outcome::from_io_error(&error, IoOp::Locate)))?;
    // The launcher's inherited selection, or a direct slot entry's active.json; either
    // way the selected slot holds this executable.
    let selected = actingcommand_contract::process_installation().map_err(|error| {
        unresolved()
            .program(&executable)
            .caused_by(Outcome::new(error.code()))
    })?;
    let root = match selected {
        Some(selected) => Some(selected.root().to_path_buf()),
        None => match slot_install_root(&executable) {
            Some(root) => {
                let selection = root.join(actingcommand_contract::INSTALL_SELECTION_PATH);
                selection
                    .try_exists()
                    .map_err(|error| {
                        unresolved().program(&executable).caused_by(
                            Outcome::from_io_error(&error, IoOp::Exists).path(&selection),
                        )
                    })?
                    .then_some(root)
            }
            None => None,
        },
    };
    let Some(root) = root else {
        return Ok(executable);
    };
    let root = plain_path(&root);
    let entry = root.join("runtime").join("actingctl.exe");
    // A slot's fixed entry that is not a file: register it once it is restored, never a slot.
    let missing = || {
        unresolved()
            .program(&executable)
            .install_root(&root)
            .entry(&entry)
    };
    match std::fs::metadata(&entry) {
        Ok(metadata) if metadata.is_file() => Ok(entry),
        Ok(_) => Err(missing()),
        Err(error) => Err(missing().caused_by(Outcome::from_io_error(&error, IoOp::Metadata))),
    }
}

/// `<root>` when the executable sits at `<root>\<A|B>\runtime\<file>`.
fn slot_install_root(executable: &Path) -> Option<PathBuf> {
    let runtime = executable
        .parent()
        .filter(|directory| directory.file_name().is_some_and(|name| name == "runtime"))?;
    let slot = runtime.parent().filter(|directory| {
        directory
            .file_name()
            .is_some_and(|name| name == "A" || name == "B")
    })?;
    slot.parent().map(Path::to_path_buf)
}

/// The canonical installation root carries the Windows verbatim prefix; a client
/// configuration takes the ordinary spelling of the same path.
fn plain_path(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{share}"))
    } else if let Some(drive) = text
        .strip_prefix(r"\\?\")
        .filter(|rest| rest.as_bytes().get(1) == Some(&b':'))
    {
        PathBuf::from(drive)
    } else {
        path.to_path_buf()
    }
}

/// A TOML literal string, or a basic string when the value holds a single quote.
fn toml_string(value: &str) -> String {
    if value.contains('\'') {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("'{value}'")
    }
}

/// The flag word at `index`; a word that is not UTF-8 is `not_utf8`.
fn flag<'a>(
    arguments: &'a [OsString],
    index: usize,
    usage: &'static str,
) -> Result<&'a str, Ended> {
    arguments[index]
        .to_str()
        .ok_or_else(|| usage_error(usage, ArgumentProblem::NotUtf8, None))
}

/// The value after the flag `word`; a flag without one is `value_missing`.
fn value<'a>(
    arguments: &'a [OsString],
    index: &mut usize,
    word: &str,
    usage: &'static str,
) -> Result<&'a OsString, Ended> {
    *index += 1;
    arguments
        .get(*index)
        .ok_or_else(|| usage_error(usage, ArgumentProblem::ValueMissing, Some(word)))
}

fn parse_tiers(
    arguments: &[OsString],
    index: &mut usize,
    word: &str,
    usage: &'static str,
) -> Result<TierSet, Ended> {
    value(arguments, index, word, usage)?
        .to_str()
        .and_then(TierSet::parse)
        .ok_or_else(|| usage_error(usage, ArgumentProblem::NotRecognised, Some(word)))
}

/// Writes `text` to stdout; a refused write is `actingctl_output_failed` caused by the OS
/// error.
fn print_text(text: &str) -> Result<(), Outcome> {
    write_text(&mut io::stdout().lock(), text).map_err(|error| {
        Outcome::new(ActingctlCode::ActingctlOutputFailed)
            .caused_by(Outcome::from_io_error(&error, IoOp::Write))
    })
}

fn write_text(output: &mut impl Write, text: &str) -> io::Result<()> {
    output.write_all(text.as_bytes())?;
    if !text.ends_with('\n') {
        output.write_all(b"\n")?;
    }
    output.flush()
}

// Test-only: Workflow #382 MCP-T1 (test plan A-5a), the result envelope, budget and jobs gate;
// Workflow #372 K3, ac_diagnose's clock-slot rows.
// Workflow #387 D1b, D1-M8: actingctl config set|clear and ac_config_set send the same changes.
// Workflow #389 MCP-8: ac_overview's truncation order.
// Workflow #386 SR-3: ac_diagnose's suspended task packages.
#[cfg(test)]
mod tests {
    mod clock_slots;
    mod gate_386_sr3;
    mod gate_387_d1b;
    mod gate_mcp8;
    // Workflow #411 A8: lab::tests::gate_rc2_a8 takes its unreached deadline.
    pub(super) mod gate_mcpt1;
}
