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
//!   after today's provenance); on Windows a rescue row the rescue module serves is served here
//!   by the module, with the initiator `Lab-MCP` (LAB-5G2b's second part), but `deploy` and
//!   `rollback`, which are refused with `mcp_operation_unsupported` ([`codes`]);
//! - [`children`] (Workflow #434 LAB-5G4b): every other offered row is served through a child
//!   `actinglab.exe serve --stdio --front mcp`, one request per child at a time; [`after_child`]
//!   (its second part): the client action after a capture and the Lab overflow rule, as today's
//!   server has them around its Lab child.
//!
//! It ships in the Tools archive since LAB-5G6 (0.12.57); nothing starts it until a client is
//! registered with the MCP entry's second generation. Its manual, which its instructions name, is
//! `apps/actinglab-mcp/MANUAL.md` (Workflow #434 LAB-5G7).
//!
//! The command line: `--list <module>[,<module>...]` names the modules whose tools `tools/list`
//! lists (the ids of the table's module rows of role `operations`); without it every installed
//! module is listed, the authoring module only where its option lists it. `--workers <n>` is the
//! number of threads that run `tools/call`, 1 to 64, 8 when not given (board #434, the
//! coordinator's rulings 6775192250 points 20 and 22, and of 2026-11-12 on #1639). Workflow #434
//! LAB-5G3 ([`jobs`]; ruling 20: a tunable is a start option with a default): `--call-budget
//! <seconds>` (1 to 300, today's 25) is how long a call may take before it answers its job's
//! handle, `--jobs <n>` (1 to 64, today's 8) how many jobs run at once and `--kept-jobs <n>` (1 to
//! 1024, today's 64) how many are kept. Workflow #434 LAB-5G4b ([`children`]; ruling 20):
//! `--children <n>` (1 to 64, default 8) is how many `serve` children live at once and
//! `--child-idle <seconds>` (0 to 600, default 10) how long one may stay idle before its input is
//! closed. `--list-tools`
//! prints the `tools/list` result and exits, and takes no `--workers`, `--jobs`, `--kept-jobs`,
//! `--children` or `--child-idle` (a `--call-budget` beside it sets the waits the list
//! describes). `--tier`, today's server's
//! option, is refused: this server has no tiers. Nothing else is taken. stdout is the protocol, or the printed list; a start that
//! fails writes its developer line on stderr and exits by its outcome's category, as
//! `actingctl mcp-serve` does.

mod after_child;
mod budget;
mod calls;
mod children;
mod codes;
mod jobs;
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
// Test-only: Workflow #434 LAB-5G3, the slice's named test of its start options, the jobs' caps
// and the front's added members (`mcp_job_start_options`).
#[cfg(test)]
mod gate_434_lab5g3;
// Test-only: Workflow #434 LAB-5G7, the slice's named tests of the tools the manual
// (`apps/actinglab-mcp/MANUAL.md`) and the instructions name
// (`mcp_manual_names_only_offered_tools`, `mcp_manual_exception_is_the_refused_rows`).
#[cfg(test)]
mod gate_434_lab5g7;

use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_lab_ops::{front, table};
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tools::{Catalog, Listing};

/// The program's name in its developer lines and its `serverInfo`.
const PROGRAM: &str = "actinglab-mcp";
const USAGE: &str = "usage: actinglab-mcp [--list <module>[,<module>...]] [--workers <n>] [--call-budget <seconds>] [--jobs <n>] [--kept-jobs <n>] [--children <n>] [--child-idle <seconds>] | actinglab-mcp --list-tools [--list <module>[,<module>...]] [--call-budget <seconds>]; <module>: core, debugging, authoring, rescue; --workers 1 to 64, default 8; --call-budget 1 to 300, default 25; --jobs 1 to 64, default 8; --kept-jobs 1 to 1024, default 64; --children 1 to 64, default 8; --child-idle 0 to 600, default 10";
/// The developer text of a refused `--tier`.
const NO_TIERS: &str = "actinglab-mcp has no tiers: tools/list lists every tool of the modules --list names, by default every installed module; usage: actinglab-mcp [--list <module>[,<module>...]] [--workers <n>] [--call-budget <seconds>] [--jobs <n>] [--kept-jobs <n>] [--children <n>] [--child-idle <seconds>] | actinglab-mcp --list-tools [--list <module>[,<module>...]] [--call-budget <seconds>]";
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
/// Workflow #434 LAB-5G3 (work order point 2; ruling 20 of 6775192250: a tunable is a start option
/// with a default, never a constant): the call budget in seconds, the jobs that run at once and the
/// jobs kept, each with today's server's value as its default (`apps/actingctl/src/mcp/tools.rs`
/// `CALL_BUDGET`, `jobs.rs` `MAX_RUNNING` and `MAX_KEPT`) and its range.
const CALL_BUDGET: &str = "--call-budget";
const JOBS: &str = "--jobs";
const KEPT_JOBS: &str = "--kept-jobs";
const CALL_BUDGET_RANGE: (u64, u64) = (1, 300);
const DEFAULT_JOBS: usize = 8;
const JOBS_RANGE: (u64, u64) = (1, 64);
const DEFAULT_KEPT_JOBS: usize = 64;
const KEPT_JOBS_RANGE: (u64, u64) = (1, 1024);
/// Workflow #434 LAB-5G4b (work order point 2; ruling 20 of 6775192250): the `serve` children
/// that live at once, and how long one may stay idle in seconds (0: its input is closed at once
/// after its answer), each with its default and its range.
const CHILDREN: &str = "--children";
const CHILD_IDLE: &str = "--child-idle";
const DEFAULT_CHILDREN: usize = 8;
const CHILDREN_RANGE: (u64, u64) = (1, 64);
const DEFAULT_CHILD_IDLE_S: u64 = 10;
const CHILD_IDLE_RANGE: (u64, u64) = (0, 600);

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

/// Workflow #434 LAB-5G3: the start options of the jobs, each its default when it is not given.
#[derive(Debug, PartialEq, Eq)]
struct JobOptions {
    /// `--call-budget`: how long a call may take before it answers its job's handle.
    call_budget: Duration,
    /// `--jobs` and `--kept-jobs`: the jobs that run at once and the jobs kept.
    limits: jobs::Limits,
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
    let (invocation, job_options, child_options) = parse_every(arguments)?;
    // Without ACTINGCOMMAND_DETAIL, full (class (b), model v2.2 §7.1), as actingctl's server.
    let detail = cli_line::detail().map_err(|outcome| Ended {
        outcome,
        text: Some(USAGE),
    })?;
    tools::set_detail(detail);
    // Workflow #434 LAB-5G3: the waits the tools describe are the call budget's.
    let catalog = Catalog::of(&table::operation_table()?)?
        .with_call_budget(job_options.call_budget.as_secs())?;
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
    // Workflow #434 LAB-5G4b: the `serve` children, whose idle ones a thread of their own ends.
    let service = calls::Service::new(budget::export_directory()).with_children(child_options)?;
    server::serve(server::ServerConfig {
        catalog,
        listing,
        workers: invocation.workers,
        service,
        call_budget: job_options.call_budget,
        limits: job_options.limits,
    })
    .map_err(Ended::from)
}

/// The command line's options without the jobs' ([`parse_all`]).
#[cfg(test)]
fn parse(arguments: &[OsString]) -> Result<Invocation, Ended> {
    parse_all(arguments).map(|(invocation, _)| invocation)
}

/// The command line's options without the children's ([`parse_every`]).
#[cfg(test)]
fn parse_all(arguments: &[OsString]) -> Result<(Invocation, JobOptions), Ended> {
    parse_every(arguments).map(|(invocation, job_options, _)| (invocation, job_options))
}

/// The command line's options; every other word, an option given twice, an option without its
/// value, a `--workers` value outside its range or that is no number and `--workers` beside
/// `--list-tools` are `actingctl_usage_invalid` naming the word, before anything is read or
/// served. Workflow #434 LAB-5G3: the jobs' options (`--call-budget`, `--jobs`, `--kept-jobs`) are
/// refused as `--workers` is; `--jobs` and `--kept-jobs` beside `--list-tools`, which runs no call,
/// are `conflicting`, and `--call-budget` beside it sets the waits the printed list describes.
/// Workflow #434 LAB-5G4b: the children's options (`--children`, `--child-idle`) are refused as
/// `--jobs` is, beside `--list-tools` too.
fn parse_every(
    arguments: &[OsString],
) -> Result<(Invocation, JobOptions, children::Options), Ended> {
    let mut list: Option<Vec<String>> = None;
    let mut list_tools = false;
    let mut workers: Option<usize> = None;
    let mut call_budget: Option<u64> = None;
    let mut running: Option<u64> = None;
    let mut kept: Option<u64> = None;
    let mut child_count: Option<u64> = None;
    let mut child_idle: Option<u64> = None;
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
            CALL_BUDGET if call_budget.is_none() => {
                index += 1;
                let value = option_value(arguments, index, word)?;
                call_budget = Some(whole_number(value, CALL_BUDGET, CALL_BUDGET_RANGE)?);
            }
            JOBS if running.is_none() => {
                index += 1;
                running = Some(whole_number(
                    option_value(arguments, index, word)?,
                    JOBS,
                    JOBS_RANGE,
                )?);
            }
            KEPT_JOBS if kept.is_none() => {
                index += 1;
                kept = Some(whole_number(
                    option_value(arguments, index, word)?,
                    KEPT_JOBS,
                    KEPT_JOBS_RANGE,
                )?);
            }
            CHILDREN if child_count.is_none() => {
                index += 1;
                child_count = Some(whole_number(
                    option_value(arguments, index, word)?,
                    CHILDREN,
                    CHILDREN_RANGE,
                )?);
            }
            CHILD_IDLE if child_idle.is_none() => {
                index += 1;
                child_idle = Some(whole_number(
                    option_value(arguments, index, word)?,
                    CHILD_IDLE,
                    CHILD_IDLE_RANGE,
                )?);
            }
            LIST | LIST_TOOLS | WORKERS | CALL_BUDGET | JOBS | KEPT_JOBS | CHILDREN
            | CHILD_IDLE => {
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
    // Workflow #434 LAB-5G3: nor would the jobs' caps.
    if list_tools && running.is_some() {
        return Err(usage(ArgumentProblem::Conflicting, Some(JOBS)));
    }
    if list_tools && kept.is_some() {
        return Err(usage(ArgumentProblem::Conflicting, Some(KEPT_JOBS)));
    }
    // Workflow #434 LAB-5G4b: nor would the children's.
    if list_tools && child_count.is_some() {
        return Err(usage(ArgumentProblem::Conflicting, Some(CHILDREN)));
    }
    if list_tools && child_idle.is_some() {
        return Err(usage(ArgumentProblem::Conflicting, Some(CHILD_IDLE)));
    }
    let limits = jobs::Limits {
        running: count(running, DEFAULT_JOBS, JOBS, JOBS_RANGE)?,
        kept: count(kept, DEFAULT_KEPT_JOBS, KEPT_JOBS, KEPT_JOBS_RANGE)?,
    };
    let job_options = JobOptions {
        call_budget: Duration::from_secs(call_budget.unwrap_or(tools::DEFAULT_CALL_BUDGET_S)),
        limits,
    };
    let child_options = children::Options {
        children: count(child_count, DEFAULT_CHILDREN, CHILDREN, CHILDREN_RANGE)?,
        idle: Duration::from_secs(child_idle.unwrap_or(DEFAULT_CHILD_IDLE_S)),
    };
    let invocation = Invocation {
        list,
        list_tools,
        workers: workers.unwrap_or(DEFAULT_WORKERS),
    };
    Ok((invocation, job_options, child_options))
}

/// Workflow #434 LAB-5G3: the value of the option `word`, read as `--workers`' is: a whole number
/// within `range`; text that is no whole number is `wrong_type`, and a number outside the range
/// `out_of_range` with the range (`range_min`, `range_max`).
fn whole_number(value: &str, word: &str, range: (u64, u64)) -> Result<u64, Ended> {
    let number = value
        .parse::<i64>()
        .map_err(|_| usage(ArgumentProblem::WrongType, Some(word)))?;
    match u64::try_from(number) {
        Ok(number) if (range.0..=range.1).contains(&number) => Ok(number),
        _ => Err(out_of_range(word, range)),
    }
}

/// Workflow #434 LAB-5G3: a count the option `word` gave within `range`, or `default` without it.
fn count(
    given: Option<u64>,
    default: usize,
    word: &str,
    range: (u64, u64),
) -> Result<usize, Ended> {
    match given {
        None => Ok(default),
        Some(given) => usize::try_from(given).map_err(|_| out_of_range(word, range)),
    }
}

/// `actingctl_usage_invalid` `out_of_range` for the option `word`, with its range and the usage
/// line.
fn out_of_range(word: &str, range: (u64, u64)) -> Ended {
    Ended {
        outcome: cli_line::usage_invalid(Some(word), ArgumentProblem::OutOfRange)
            .range_min(range.0)
            .range_max(range.1),
        text: Some(USAGE),
    }
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
