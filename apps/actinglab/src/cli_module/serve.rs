// SPDX-License-Identifier: AGPL-3.0-only

//! `actinglab serve --stdio` (Workflow #434 LAB-2; `contracts/lab-two-entries.md` §8.2): a
//! resident mode of the Lab core. Each line on stdin is one request `{"op": <operation name>,
//! "params": {…}}`; each gets one answer line on stdout, in order: the JSON line the CLI prints
//! with `--json` for the same operation and parameters (`{…result, "outcome": …}`, or
//! `{"outcome": …}` for a refusal). A line that is no such request is answered with its refusal
//! (`actingctl_usage_invalid`) and the loop goes on; a developer line goes to stderr as the CLI
//! writes it. The loop ends with stdin, exit 0; a stdin or stdout that fails ends it with the
//! developer line `actingctl_output_failed`, exit 1. The operations are the CLI's, and the
//! initiator is checked once, at the start.

use super::{PROGRAM, words};
use actingcommand_contract::outcome::Detail;
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::codes::ArgumentProblem;
use actingcommand_lab_ops::table::OperationTable;
use serde_json::Value;
use std::io::{self, BufRead, Write};
use std::process::ExitCode;

/// The developer text of a line that is no request.
const SERVE_USAGE: &str = "serve --stdio takes one JSON object per line: \
     {\"op\": <operation name>, \"params\": {<parameter>: <value>}}";

/// `serve` with the words after it; only `--stdio` is taken.
pub(super) fn main(words: &[String]) -> ExitCode {
    if words != ["--stdio"] {
        let refused = words
            .iter()
            .find(|word| word.as_str() != "--stdio")
            .map_or_else(
                || cli_line::usage_invalid(Some("--stdio"), ArgumentProblem::Required),
                |word| cli_line::usage_invalid(Some(word.as_str()), ArgumentProblem::Unknown),
            );
        return super::print(&cli_line::failure(&refused, Some(SERVE_USAGE)), true);
    }
    if let Err(exit) = super::admitted() {
        return super::print(&exit, true);
    }
    let table = match super::table() {
        Ok(table) => table,
        Err(outcome) => return super::print(&cli_line::failure(&outcome, None), true),
    };
    let stdin = io::stdin();
    let stdout = io::stdout();
    let stderr = io::stderr();
    match serve(table, stdin.lock(), &mut stdout.lock(), &mut stderr.lock()) {
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

/// Answers every request line of `input` on `output`, one line each, and writes the developer
/// lines to `developer`; ends with `input`.
pub(crate) fn serve(
    table: &OperationTable,
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
        let exit = match String::from_utf8(bytes.clone()) {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => answer(table, line.trim_end_matches(['\r', '\n'])),
            Err(_) => cli_line::failure(
                &cli_line::usage_invalid(None, ArgumentProblem::NotUtf8),
                Some(SERVE_USAGE),
            ),
        };
        let line = match &exit.line {
            Ok(line) => line.clone(),
            Err(text) => {
                writeln!(
                    developer,
                    "{}",
                    cli_line::developer_text(PROGRAM, &Exit::output_failed(), Some(text))
                )?;
                output_failed_line()
            }
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

/// The answer to one request line.
pub(crate) fn answer(table: &OperationTable, line: &str) -> Exit {
    match words::from_json(table, line) {
        Ok(call) => super::execute(table, &call),
        Err(refusal) => cli_line::failure(&refusal, Some(SERVE_USAGE)),
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
