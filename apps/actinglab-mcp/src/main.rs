// SPDX-License-Identifier: AGPL-3.0-only

//! `actinglab-mcp`, the Lab's MCP server (Workflow #434 LAB-5; `contracts/lab-two-entries.md`
//! §8.1 and §9.3; register §1.287, "Slice LAB-5G2a"): its tools are the operation table's, one
//! for each row the table offers, generated from the row; it has no tiers. It runs beside today's
//! MCP server (`actingctl mcp-serve`), which it replaces only at the end of LAB-5.
//!
//! This build is the server's shell (LAB-5G2a) and serves the calls it can serve in its own
//! process (LAB-5G2b):
//! - [`protocol`] and [`server`]: the JSON-RPC framing, the two MCP protocol eras, the stdio loop,
//!   batches, progress, cancellation and `actingcommand/entryStatus`, copied from today's server
//!   without its tiers and its jobs;
//! - [`tools`]: the tools generated from the table, `tools/list`, the listing option and the
//!   instructions;
//! - [`calls`], [`runtime`] and [`budget`] (Workflow #434 LAB-5G2b): a call of a row
//!   `lab_ops::core_ops` serves is served here and
//!   answers the line `actinglab --json` prints, within today's answer budget, on one kept
//!   Runtime connection of `lab_ops::front`'s `Front::Mcp` (a write on a connection of its own,
//!   after today's provenance); every other call is refused with `mcp_operation_unsupported`
//!   ([`codes`]) until a later slice serves it. It is in no release and no Tools archive until it
//!   is packaged.
//!
//! The command line: `--list <module>[,<module>...]` names the modules whose tools `tools/list`
//! lists (the ids of the table's module rows of role `operations`); without it every installed
//! module is listed, the authoring module only where its option lists it. `--workers <n>` is the
//! number of threads that run `tools/call`, 1 to 64, 8 when not given (board #434, the
//! coordinator's rulings 6775192250 points 20 and 22, and of 2026-11-12 on #1639). `--list-tools`
//! prints the `tools/list` result and exits, and takes no `--workers`. `--tier`, today's server's
//! option, is refused: this server has no tiers. Nothing else is taken. stdout is the protocol, or the printed list; a start that
//! fails writes its developer line on stderr and exits by its outcome's category, as
//! `actingctl mcp-serve` does.

mod budget;
mod calls;
mod codes;
mod protocol;
mod runtime;
mod server;
mod tools;

// Test-only: Workflow #434 LAB-5G2a, the slice's named tests of the tool list
// (`mcp_tools_from_op_table`, the names, `mcp_list_option_and_authoring_gate`).
#[cfg(test)]
mod gate_434_lab5g2a;
// Test-only: Workflow #434 LAB-5G2b, the slice's named tests of what is served and how
// (`mcp_served_and_refused_partition`, `mcp_front_members_in_one_place`,
// `mcp_budget_cut_and_export`).
#[cfg(test)]
mod gate_434_lab5g2b;

use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_lab_ops::{front, table};
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::{Mutex, MutexGuard, PoisonError};
use tools::{Catalog, Listing};

/// The program's name in its developer lines and its `serverInfo`.
const PROGRAM: &str = "actinglab-mcp";
const USAGE: &str = "usage: actinglab-mcp [--list <module>[,<module>...]] [--workers <n>] | actinglab-mcp --list-tools [--list <module>[,<module>...]]; <module>: core, debugging, authoring, rescue; <n>: 1 to 64, default 8";
/// The developer text of a refused `--tier`.
const NO_TIERS: &str = "actinglab-mcp has no tiers: tools/list lists every tool of the modules --list names, by default every installed module; usage: actinglab-mcp [--list <module>[,<module>...]] [--workers <n>] | actinglab-mcp --list-tools [--list <module>[,<module>...]]";
const LIST: &str = "--list";
const LIST_TOOLS: &str = "--list-tools";
const WORKERS: &str = "--workers";
const TIER: &str = "--tier";
/// The threads that run `tools/call` when `--workers` is not given (board #434, the
/// coordinator's ruling 6775192250, point 22: eight, as the job cap).
const DEFAULT_WORKERS: usize = 8;
/// The fewest and the most `--workers` takes.
const MIN_WORKERS: usize = 1;
const MAX_WORKERS: usize = 64;

/// Why the program ended without serving (Workflow #378 A5, model v2.2 §7.3): one outcome, and
/// the developer text of its line (the usage line of a usage error). Nothing is printed on
/// stdout: the developer line on stderr carries it, and the process exits by its category.
#[derive(Debug)]
struct Ended {
    outcome: Outcome,
    text: Option<&'static str>,
}

impl From<Outcome> for Ended {
    fn from(outcome: Outcome) -> Self {
        Self {
            outcome,
            text: None,
        }
    }
}

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
struct Invocation {
    /// `--list`: the words of its value, split at its commas; `None` without it.
    list: Option<Vec<String>>,
    /// `--list-tools`: print the `tools/list` result and exit.
    list_tools: bool,
    /// `--workers`, or [`DEFAULT_WORKERS`] without it: the threads that run `tools/call`.
    workers: usize,
}

fn main() -> ExitCode {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    run(&arguments).unwrap_or_else(|ended| {
        eprintln!(
            "{}",
            cli_line::developer_text(PROGRAM, &ended.outcome, ended.text)
        );
        ExitCode::from(cli_line::exit_code(ended.outcome.category(), false))
    })
}

fn run(arguments: &[OsString]) -> Result<ExitCode, Ended> {
    let invocation = parse(arguments)?;
    // Without ACTINGCOMMAND_DETAIL, full (class (b), model v2.2 §7.1), as actingctl's server.
    let detail = cli_line::detail().map_err(|outcome| Ended {
        outcome,
        text: Some(USAGE),
    })?;
    tools::set_detail(detail);
    let catalog = Catalog::of(&table::operation_table()?)?;
    let listing = match &invocation.list {
        None => Listing::Default,
        Some(words) => catalog
            .named(words)
            .map_err(|problem| usage(problem, Some(LIST)))?,
    };
    if invocation.list_tools {
        let text =
            serde_json::to_string_pretty(&catalog.list_result(&listing)).map_err(|error| {
                Outcome::new(ActingctlCode::ActingctlOutputFailed).caused_by(
                    Outcome::new(ContractCode::ForeignDataError)
                        .raw_source(RawSource::Json)
                        .raw_text(error.to_string()),
                )
            })?;
        print_text(&text)?;
        return Ok(ExitCode::SUCCESS);
    }
    // Workflow #434 LAB-5G2b: this process's commands connect as the Lab's MCP server does until
    // LAB-6 (the MCP channel, today's envelope), and `status` reads the overview's `watchdog`
    // block as actinglab reads it, on Windows only, where the watchdog runs.
    let _ = front::set(front::Front::Mcp);
    #[cfg(windows)]
    let _ =
        actingcommand_lab_ops::overview::set_watchdog_block(actingcommand_watchdog::overview_block);
    server::serve(server::ServerConfig {
        catalog,
        listing,
        workers: invocation.workers,
        service: calls::Service::new(budget::export_directory()),
    })
    .map_err(Ended::from)
}

/// The command line's options; every other word, an option given twice, an option without its
/// value, a `--workers` value outside its range or that is no number and `--workers` beside
/// `--list-tools` are `actingctl_usage_invalid` naming the word, before anything is read or
/// served.
fn parse(arguments: &[OsString]) -> Result<Invocation, Ended> {
    let mut list: Option<Vec<String>> = None;
    let mut list_tools = false;
    let mut workers: Option<usize> = None;
    let mut index = 0;
    while index < arguments.len() {
        let word = arguments[index]
            .to_str()
            .ok_or_else(|| usage(ArgumentProblem::NotUtf8, None))?;
        match word {
            LIST if list.is_none() => {
                index += 1;
                let value = option_value(arguments, index, word)?;
                list = Some(value.split(',').map(str::to_owned).collect());
            }
            LIST_TOOLS if !list_tools => list_tools = true,
            WORKERS if workers.is_none() => {
                index += 1;
                workers = Some(worker_count(option_value(arguments, index, word)?)?);
            }
            LIST | LIST_TOOLS | WORKERS => {
                return Err(usage(ArgumentProblem::Repeated, Some(word)));
            }
            // Today's server's tiers: this one has none, and says so.
            _ if word == TIER || word.starts_with("--tier=") => {
                return Err(Ended {
                    outcome: cli_line::usage_invalid(Some(TIER), ArgumentProblem::Unknown),
                    text: Some(NO_TIERS),
                });
            }
            _ => return Err(usage(ArgumentProblem::Unknown, Some(word))),
        }
        index += 1;
    }
    // The printed list runs no call: the worker count would change nothing there.
    if list_tools && workers.is_some() {
        return Err(usage(ArgumentProblem::Conflicting, Some(WORKERS)));
    }
    Ok(Invocation {
        list,
        list_tools,
        workers: workers.unwrap_or(DEFAULT_WORKERS),
    })
}

/// The value of the option `word`, the argument at `index`: `value_missing` when there is none,
/// `not_utf8` when it is not UTF-8.
fn option_value<'a>(arguments: &'a [OsString], index: usize, word: &str) -> Result<&'a str, Ended> {
    arguments
        .get(index)
        .ok_or_else(|| usage(ArgumentProblem::ValueMissing, Some(word)))?
        .to_str()
        .ok_or_else(|| usage(ArgumentProblem::NotUtf8, Some(word)))
}

/// `--workers`' value: a whole number from [`MIN_WORKERS`] to [`MAX_WORKERS`]. Text that is no
/// whole number is `wrong_type`; a number outside the range is `out_of_range` with the range
/// (`range_min`, `range_max`).
fn worker_count(value: &str) -> Result<usize, Ended> {
    let count = value
        .parse::<i64>()
        .map_err(|_| usage(ArgumentProblem::WrongType, Some(WORKERS)))?;
    match usize::try_from(count) {
        Ok(count) if (MIN_WORKERS..=MAX_WORKERS).contains(&count) => Ok(count),
        _ => Err(Ended {
            outcome: cli_line::usage_invalid(Some(WORKERS), ArgumentProblem::OutOfRange)
                .range_min(MIN_WORKERS)
                .range_max(MAX_WORKERS),
            text: Some(USAGE),
        }),
    }
}

/// `actingctl_usage_invalid` for `word` and what is wrong with it, with the usage line.
fn usage(problem: ArgumentProblem, word: Option<&str>) -> Ended {
    Ended {
        outcome: cli_line::usage_invalid(word, problem),
        text: Some(USAGE),
    }
}

/// A mutex whose holder panicked still guards consistent data here: every critical
/// section only inserts, removes or sets whole values.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
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
