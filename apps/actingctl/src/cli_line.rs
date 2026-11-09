// SPDX-License-Identifier: AGPL-3.0-only

//! actingctl's result line (Workflow #378 A5), printed here (Workflow #434 LAB-2): `lab-ops`
//! builds the line, the developer line and the exit code (`actingcommand_lab_ops::cli_line`) and
//! this program prints them, unchanged: the line on stdout, then the developer line
//! `<LEVEL> actingctl: …` on stderr; a line stdout refuses, or one that did not encode, leaves
//! only the developer line `actingctl_output_failed` and exits 1 (§7.3). Every other item of
//! `lab-ops`' module is used as it is.

pub use actingcommand_lab_ops::cli_line::*;

use actingcommand_contract::outcome::Outcome;
use serde_json::{Map, Value};
use std::io::{self, Write};
use std::process::ExitCode;

/// The name the developer line gives.
const PROGRAM: &str = "actingctl";

/// The developer line `<LEVEL> actingctl: <outcome>[ <text>]` on stderr.
pub fn developer_line(outcome: &Outcome, text: Option<&str>) {
    eprintln!("{}", developer_text(PROGRAM, outcome, text));
}

/// Prints an exit `lab-ops` built and returns its exit code.
pub fn print(exit: Exit) -> ExitCode {
    let line = match exit.line {
        Ok(line) => line,
        Err(text) => return output_failed(&text),
    };
    if let Err(error) = write_line(&line) {
        return output_failed(&error.to_string());
    }
    if let Some(developer) = &exit.developer {
        eprintln!("{}", developer.line(PROGRAM));
    }
    ExitCode::from(exit.code)
}

/// A successful exit: `{…result, "outcome": …}`.
pub fn success(result: Map<String, Value>, outcome: &Outcome) -> ExitCode {
    print(actingcommand_lab_ops::cli_line::success(result, outcome))
}

/// A failed exit: `{"outcome": …}`, then the developer line with `text`.
pub fn failure(outcome: &Outcome, text: Option<&str>) -> ExitCode {
    print(actingcommand_lab_ops::cli_line::failure(outcome, text))
}

/// A refused exit: `{outcome}` and the developer line with `text`, non-zero whatever the top's
/// category.
pub fn refused(outcome: &Outcome, text: Option<&str>) -> ExitCode {
    print(actingcommand_lab_ops::cli_line::refused(outcome, text))
}

/// The line of a subcommand whose result holds a Runtime receipt (`refused`: its state is denied
/// or failed).
pub fn receipt(result: Map<String, Value>, outcome: &Outcome, refused: bool) -> ExitCode {
    print(actingcommand_lab_ops::cli_line::receipt(
        result, outcome, refused,
    ))
}

fn write_line(line: &Value) -> io::Result<()> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    serde_json::to_writer(&mut writer, line)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn output_failed(text: &str) -> ExitCode {
    developer_line(&Exit::output_failed(), Some(text));
    ExitCode::FAILURE
}
