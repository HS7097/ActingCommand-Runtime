// SPDX-License-Identifier: AGPL-3.0-only

//! `actinglab serve --stdio` (Workflow #434 LAB-2; `contracts/lab-two-entries.md` §8.2): a
//! resident mode of the Lab core. Each line on stdin is one request `{"op": <operation name>,
//! "params": {…}}`; each gets one answer line on stdout, in order: the line the operation's
//! `--json` prints for the same parameters (LAB-2 fix 1), the line `cli_line` builds
//! (`{…result, "outcome": …}`, or `{"outcome": …}` for a refusal), for an operation one of
//! actinglab's own commands serves too (Workflow #434 LAB-2b, `super::own_line`: its failure
//! carries `message` and `details` beside the outcome). A line that is no such request is
//! answered with its refusal
//! (`actingctl_usage_invalid`) and the loop goes on; a developer line goes to stderr as the CLI
//! writes it. The loop ends with stdin, exit 0; a stdin or stdout that fails ends it with the
//! developer line `actingctl_output_failed`, exit 1. The operations are the CLI's, and the
//! initiator is checked once, at the start.
//!
//! Workflow #434 LAB-5G4a (register §1.287, "Slice LAB-5G4a"; contract §4.2 and §8.2):
//! - **`--front mcp`**, an option only `serve --stdio` takes, says that an MCP server started the
//!   process as its child ([`Front`]). The process's initiator is then `Lab-MCP`: every step and
//!   mark its commands add to a recording carries it (the loop runs actinglab's own commands with
//!   `via` `mcp`, as MCP's `--via mcp` child runs them), and so does a rescue row's journal. The
//!   connections actinglab's own commands open bind as that child's do
//!   (`crate::lab_client_config`: the Lab channel, `via` `mcp`, the envelope (Lab, Lab)), until
//!   LAB-6; the core rows `lab-ops` serves, which that child never serves, connect as before. Its
//!   lines stay this loop's (since LAB-5G4b each inside the envelope below), never the envelope a
//!   `--via` line prints. With the option,
//!   `ACTINGCOMMAND_INITIATOR` set to anything is refused before the loop starts: the variable
//!   and the option name different initiators, and the MCP server removes the variable for its
//!   children. Without the option nothing changes.
//! - **A value that begins with `--`**: a parameter whose string value, or a string in its list,
//!   begins with `--` is refused for every row before anything runs (`words::from_json`): the
//!   command line of its row would read it as a flag.
//!
//! Workflow #434 LAB-5G4b (register §1.287, "Slice LAB-5G4b"; contract §8.2; the coordinator's
//! ruling on board #434, 6827665631, not the owner's words): **under `--front mcp` each answer
//! line is the envelope** `{"exit_code", "req_id"?, "line"}` ([`enveloped`]): `line` is the line
//! the loop answers without the option, byte for byte, `exit_code` the code the command line would
//! exit with for it, and `req_id` the request id of the own command that served the row, when its
//! result carries one (`super::own_line::request_id`). The MCP server answers `isError` by the
//! exit code, and the client action after a capture names the request id; neither can be read
//! from every line. Without the option every line is as before. The refusals before the loop
//! (`--front` itself, the installation, the initiator, the table) stay one line each, without the
//! envelope.

use super::{PROGRAM, words};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_lab_ops::initiator;
use actingcommand_lab_ops::table::OperationTable;
use actingcommand_runtime_client::ChannelVia;
use serde_json::{Map, Value};
use std::ffi::OsStr;
use std::io::{self, BufRead, Write};
use std::process::ExitCode;

/// The developer text of a line that is no request.
const SERVE_USAGE: &str = "serve --stdio takes one JSON object per line: \
     {\"op\": <operation name>, \"params\": {<parameter>: <value>}}";
/// The word of the resident mode.
const STDIO: &str = "--stdio";
/// Workflow #434 LAB-5G4a: the option that names the front end, and its one value.
const FRONT: &str = "--front";
const FRONT_MCP: &str = "mcp";
/// The developer text of a refused `--front`.
const FRONT_USAGE: &str = "serve --stdio takes --front mcp at most once: the front end that \
     started serve, an MCP server";
/// The developer text of `--front mcp` with `ACTINGCOMMAND_INITIATOR` set.
const FRONT_INITIATOR_TEXT: &str = "serve --stdio --front mcp records Lab-MCP; \
     ACTINGCOMMAND_INITIATOR names another initiator and may not be set with it";
/// Workflow #434 LAB-5G4b: the members of the envelope an answer line is under `--front mcp`.
const EXIT_CODE: &str = "exit_code";
const REQ_ID: &str = "req_id";
const LINE: &str = "line";

/// Workflow #434 LAB-5G4a: the front end that started `serve --stdio`, as `--front` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Front {
    /// `--front mcp`: an MCP server's child. Its initiator is `Lab-MCP`, and its connections bind
    /// as MCP's `--via mcp` child's do.
    Mcp,
}

impl Front {
    /// The counter that forwarded the work, as MCP's `--via mcp` child names it: the `via` the
    /// connections of actinglab's own commands bind (`crate::lab_client_config`) and the commands
    /// the loop runs carry, so that their recordings carry `Lab-MCP`
    /// (`super::recording_initiator`).
    pub(crate) const fn via(self) -> ChannelVia {
        match self {
            Self::Mcp => ChannelVia::Mcp,
        }
    }
}

/// `serve` with the words after it: `--stdio`, and (Workflow #434 LAB-5G4a) `--front mcp`.
pub(super) fn main(words: &[String]) -> ExitCode {
    let front = match front(
        words,
        std::env::var_os(initiator::ENVIRONMENT_VARIABLE).as_deref(),
    ) {
        Ok(front) => front,
        Err(refused) => return super::print(&refused, true),
    };
    if let Err(exit) = super::admitted() {
        return super::print(&exit, true);
    }
    // Workflow #434 LAB-5G4a: every connection actinglab's own commands open binds as MCP's
    // `--via mcp` child's does. `main` sets this only from a command line's `--via`, which never
    // reaches `serve`.
    if let Some(front) = front {
        let _ = crate::CHANNEL_VIA.set(front.via());
    }
    let table = match super::table() {
        Ok(table) => table,
        Err(outcome) => return super::print(&cli_line::failure(&outcome, None), true),
    };
    let stdin = io::stdin();
    let stdout = io::stdout();
    let stderr = io::stderr();
    match serve_as(
        table,
        front,
        stdin.lock(),
        &mut stdout.lock(),
        &mut stderr.lock(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "{}",
                cli_line::developer_text(PROGRAM, &Exit::output_failed(), Some(&error.to_string()))
            );
            ExitCode::FAILURE
        }
    }
}

/// Workflow #434 LAB-5G4a: the front end the words after `serve` name, given the value of
/// `ACTINGCOMMAND_INITIATOR` (`variable`, `None` when it is not set), or the refusal `main`
/// prints. Without `--front` the words are checked as before, byte for byte: only `--stdio`. With
/// it: `--stdio` once and `--front mcp` once, in either order; another value of the option, the
/// option twice, the option without `--stdio` and every other word are refused as an unknown
/// word, and the option without a value as `value_missing`. With the option, the variable set to
/// anything is refused with `variable`, before anything runs.
pub(crate) fn front(words: &[String], variable: Option<&OsStr>) -> Result<Option<Front>, Exit> {
    if !words.iter().any(|word| word == FRONT) {
        return stdio_only(words).map(|()| None);
    }
    let refused = |word: &str, problem: ArgumentProblem| {
        cli_line::failure(
            &cli_line::usage_invalid(Some(word), problem),
            Some(FRONT_USAGE),
        )
    };
    let mut stdio = false;
    let mut front = None;
    let mut index = 0;
    while index < words.len() {
        match words[index].as_str() {
            STDIO if !stdio => stdio = true,
            FRONT if front.is_none() => {
                index += 1;
                match words.get(index).map(String::as_str) {
                    Some(FRONT_MCP) => front = Some(Front::Mcp),
                    Some(value) => return Err(refused(value, ArgumentProblem::Unknown)),
                    None => return Err(refused(FRONT, ArgumentProblem::ValueMissing)),
                }
            }
            other => return Err(refused(other, ArgumentProblem::Unknown)),
        }
        index += 1;
    }
    if !stdio {
        return Err(refused(FRONT, ArgumentProblem::Unknown));
    }
    if variable.is_some() {
        return Err(cli_line::failure(
            &Outcome::new(ActingctlCode::ActingctlUsageInvalid)
                .variable(initiator::ENVIRONMENT_VARIABLE),
            Some(FRONT_INITIATOR_TEXT),
        ));
    }
    Ok(front)
}

/// The words after `serve` without `--front`: only `--stdio` is taken, as before LAB-5G4a.
fn stdio_only(words: &[String]) -> Result<(), Exit> {
    if words == [STDIO] {
        return Ok(());
    }
    let refused = words
        .iter()
        .find(|word| word.as_str() != STDIO)
        .map_or_else(
            || cli_line::usage_invalid(Some(STDIO), ArgumentProblem::Required),
            |word| cli_line::usage_invalid(Some(word.as_str()), ArgumentProblem::Unknown),
        );
    Err(cli_line::failure(&refused, Some(SERVE_USAGE)))
}

/// [`serve_as`] with no front end named, as `serve --stdio` without `--front` runs.
#[cfg(test)]
pub(crate) fn serve(
    table: &OperationTable,
    input: impl BufRead,
    output: &mut impl Write,
    developer: &mut impl Write,
) -> io::Result<()> {
    serve_as(table, None, input, output, developer)
}

/// Answers every request line of `input` on `output`, one line each, and writes the developer
/// lines to `developer`; ends with `input`. `front` is the front end `--front` named (Workflow
/// #434 LAB-5G4a), `None` without the option. Workflow #434 LAB-5G4b: with it each line is the
/// envelope of the line ([`enveloped`]).
pub(crate) fn serve_as(
    table: &OperationTable,
    front: Option<Front>,
    mut input: impl BufRead,
    output: &mut impl Write,
    developer: &mut impl Write,
) -> io::Result<()> {
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        if input.read_until(b'\n', &mut bytes)? == 0 {
            return Ok(());
        }
        let (exit, request_id) = match String::from_utf8(bytes.clone()) {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => answer_line(table, front, line.trim_end_matches(['\r', '\n'])),
            Err(_) => (
                cli_line::failure(
                    &cli_line::usage_invalid(None, ArgumentProblem::NotUtf8),
                    Some(SERVE_USAGE),
                ),
                None,
            ),
        };
        let (line, code) = match &exit.line {
            Ok(line) => (line.clone(), exit.code),
            Err(text) => {
                writeln!(
                    developer,
                    "{}",
                    cli_line::developer_text(PROGRAM, &Exit::output_failed(), Some(text))
                )?;
                // The command line prints no line then, and exits as the failure's category says.
                let code = cli_line::exit_code(Exit::output_failed().category(), false);
                (output_failed_line(), code)
            }
        };
        let line = match front {
            None => line,
            Some(Front::Mcp) => enveloped(code, request_id.as_deref(), line),
        };
        writeln!(output, "{line}")?;
        output.flush()?;
        if exit.line.is_ok()
            && let Some(line) = &exit.developer
        {
            writeln!(developer, "{}", line.line(PROGRAM))?;
        }
    }
}

/// Workflow #434 LAB-5G4b: the answer line under `--front mcp`, `{"exit_code", "req_id"?,
/// "line"}`: `line` as the loop answers it without the option, `exit_code` the code the command
/// line would exit with for it, `req_id` the request id of the own command that served the row,
/// absent when its result carries none.
pub(crate) fn enveloped(exit_code: u8, request_id: Option<&str>, line: Value) -> Value {
    let mut envelope = Map::new();
    envelope.insert(EXIT_CODE.to_owned(), Value::from(exit_code));
    if let Some(request_id) = request_id {
        envelope.insert(REQ_ID.to_owned(), Value::String(request_id.to_owned()));
    }
    envelope.insert(LINE.to_owned(), line);
    Value::Object(envelope)
}

/// The answer to one request line, for the front end `front`, and (Workflow #434 LAB-5G4b) the
/// request id of the own command that served it, when its result carries one.
fn answer_line(table: &OperationTable, front: Option<Front>, line: &str) -> (Exit, Option<String>) {
    match words::from_json(table, line) {
        Ok(call) => super::answered(table, &call, front),
        Err(refusal) => (cli_line::failure(&refusal, Some(SERVE_USAGE)), None),
    }
}

/// `{"outcome": actingctl_output_failed}`: the answer of a request whose own answer did not
/// encode, so that every request still gets its line.
fn output_failed_line() -> Value {
    cli_line::line(
        None,
        &Exit::output_failed(),
        cli_line::detail().unwrap_or(Detail::Full),
    )
    .unwrap_or_else(|_| Value::String(Exit::output_failed().code().as_str().to_owned()))
}
