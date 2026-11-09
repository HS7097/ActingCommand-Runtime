// SPDX-License-Identifier: AGPL-3.0-only

//! The CLI module of `actinglab.exe` and `actinglab serve --stdio` (Workflow #434 LAB-2; register
//! §1.287, "Slice LAB-2"; `contracts/lab-two-entries.md` §4.2, §8.2 and §9.3).
//!
//! - **Words.** Short words taken from the operation table (`contracts/lab-operations.json`): the
//!   operations of the core, debugging and authoring modules that LAB-1, LAB-1b and LAB-2 built
//!   ([`words`]). Every operation offered is a table row. The instance is a positional (an alias,
//!   an `instance_id` or an ADB port); there is no `--root` and no `--state-root` ([`place`]).
//! - **Serving.** The core rows by `crates/lab-ops` on the Lab channel ([`core_ops`]); the
//!   debugging and authoring rows by actinglab's own commands ([`engine`]).
//! - **Output.** For people by default: one line with the result code (the outcome catalog holds
//!   no phrase for a code, so the code alone, with its values) and the result's members as a small
//!   table. `--json` prints the one line `cli_line` builds, `{…result, "outcome": …}`. Exit codes
//!   0, 1 and 2, by the outcome's category; a refusal exits at least 1.
//! - **`serve --stdio`.** One `{op, params}` per line on stdin, one answer per line on stdout,
//!   the same JSON as the CLI's `--json` line for the same operation ([`serve`]).
//! - **Initiator** (§4.2). `Lab-CLI`, or `Lab-制包` / `Lab-救急` from `ACTINGCOMMAND_INITIATOR`;
//!   any other value is refused before anything is sent or written. A Lab recording's steps and
//!   marks carry it (open point 6, the coordinator's ruling).
//! - **The envelope form** (transitional). `--envelope`, and `--via` (the MCP server's actinglab
//!   child), run the command words of actinglab before LAB-2 with their envelope and exit codes,
//!   unchanged, so that every MCP answer and the protocol goldens stay as they are until LAB-5
//!   builds MCP on the table; the retired and reserved words are gone from it too.

mod core_ops;
pub(crate) mod engine;
mod place;
pub(crate) mod serve;
pub(crate) mod words;

use crate::codes::ActinglabCode;
use crate::{CliError, ErrorKind, RUNTIME_STATE_ROOT_ENV};
use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_lab_ops::initiator::{self, Initiator};
use actingcommand_lab_ops::table::OperationTable;
use serde_json::{Map, Value};
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::OnceLock;
use words::Call;

/// The name the developer line gives.
pub(crate) const PROGRAM: &str = "actinglab";
/// The envelope form's switch (transitional, see the module's description).
pub(crate) const ENVELOPE: &str = "--envelope";
/// actinglab's option naming the counter that forwarded the work (the MCP server's child).
const VIA: &str = "--via";
/// The front end's switch for the JSON line.
const JSON: &str = "--json";
/// The developer text of a command line that names no operation.
const USAGE: &str = "usage: actinglab [--json] <operation words> [<positionals>] \
     [--<option> <value>] [--<switch>]; actinglab help lists the operations; \
     actinglab serve --stdio answers one {op, params} per line";
/// The developer text of an `ACTINGCOMMAND_INITIATOR` this process does not take.
const INITIATOR_TEXT: &str = "ACTINGCOMMAND_INITIATOR takes Lab-制包 or Lab-救急; without it \
     actinglab is Lab-CLI";

/// Whether `arguments` ask for the envelope form: `--envelope`, or `--via` (the MCP server's
/// actinglab child).
pub(crate) fn envelope_form(arguments: &[String]) -> bool {
    arguments
        .iter()
        .any(|argument| argument == ENVELOPE || argument == VIA)
}

/// `arguments` without the envelope form's switch.
pub(crate) fn without_envelope(arguments: Vec<String>) -> Vec<String> {
    arguments
        .into_iter()
        .filter(|argument| argument != ENVELOPE)
        .collect()
}

/// The operation table this build embeds.
static TABLE: OnceLock<Result<OperationTable, Outcome>> = OnceLock::new();

/// The operation table this build embeds, read once.
pub(crate) fn table() -> Result<&'static OperationTable, Outcome> {
    TABLE
        .get_or_init(actingcommand_lab_ops::table::operation_table)
        .as_ref()
        .map_err(Clone::clone)
}

/// The initiator word every step and mark this process adds to a Lab recording carries (§4.2):
/// `Lab-CLI`, or the `Lab-制包` / `Lab-救急` of `ACTINGCOMMAND_INITIATOR`. Any other value of the
/// variable is `actingctl_usage_invalid`, and nothing is written.
pub(crate) fn recording_initiator() -> Result<&'static str, CliError> {
    Initiator::of_this_lab_process()
        .map(Initiator::as_str)
        .map_err(|_| {
            CliError::new(
                ErrorKind::UsageValidation,
                ActingctlCode::ActingctlUsageInvalid.as_str(),
                format!(
                    "{} names no initiator of an actinglab process: {INITIATOR_TEXT}",
                    initiator::ENVIRONMENT_VARIABLE
                ),
                &[],
            )
            .with_details(serde_json::json!({"variable": initiator::ENVIRONMENT_VARIABLE}))
        })
}

/// The CLI module: one command line (`serve --stdio` included), printed, and its exit code.
pub(crate) fn main(arguments: Vec<String>) -> ExitCode {
    actingcommand_lab_ops::front::set(actingcommand_lab_ops::front::Front::Actinglab);
    let json = arguments.iter().any(|argument| argument == JSON);
    let words = arguments
        .into_iter()
        .filter(|argument| argument != JSON)
        .collect::<Vec<_>>();
    if words.first().map(String::as_str) == Some("serve") {
        return serve::main(&words[1..]);
    }
    print(&run(&words), json)
}

/// The exit of one command line (the front end's `--json` taken out).
pub(crate) fn run(words: &[String]) -> Exit {
    match words {
        [] => return help(),
        [word] if word == "help" => return help(),
        [word] if word == "--version" => return version(),
        _ => {}
    }
    if let Err(exit) = admitted() {
        return exit;
    }
    let table = match table() {
        Ok(table) => table,
        Err(outcome) => return cli_line::failure(&outcome, None),
    };
    match words::parse(table, words) {
        Ok(call) => execute(table, &call),
        Err(refusal) => cli_line::failure(&refusal, Some(USAGE)),
    }
}

/// What every operation needs first: the installation this process runs from, and its initiator
/// word (§4.2); a refusal sends and writes nothing.
pub(crate) fn admitted() -> Result<(), Exit> {
    if let Err(error) = actingcommand_contract::process_installation() {
        return Err(cli_line::refused(&Outcome::new(error.code()), None));
    }
    Initiator::of_this_lab_process()
        .map(|_| ())
        .map_err(|outcome| cli_line::failure(&outcome, Some(INITIATOR_TEXT)))
}

/// The operation `call` names, run, as its line.
pub(crate) fn execute(table: &OperationTable, call: &Call) -> Exit {
    let Some(operation) = table.operation(&call.name) else {
        return cli_line::failure(
            &cli_line::usage_invalid(Some(call.name.as_str()), ArgumentProblem::Unknown),
            Some(USAGE),
        );
    };
    if core_ops::serves(operation) {
        return core_ops::execute(operation, &call.params);
    }
    // actinglab's commands read the state root the configuration names, unless the environment
    // names one.
    if std::env::var_os(RUNTIME_STATE_ROOT_ENV).is_none()
        && let Ok(state_root) = place::place().state_root
    {
        crate::state_roots::use_located_state_root(state_root);
    }
    engine::execute(operation, &call.params)
}

/// `help`: the operations this program offers, in the table's order.
fn help() -> Exit {
    let table = match table() {
        Ok(table) => table,
        Err(outcome) => return cli_line::failure(&outcome, None),
    };
    let operations = table
        .operations
        .iter()
        .filter(|operation| words::offered(operation))
        .map(|operation| Value::String(operation.name.clone()))
        .collect();
    let mut members = Map::new();
    members.insert("usage".to_owned(), Value::from(USAGE));
    members.insert("operations".to_owned(), Value::Array(operations));
    cli_line::success(members, &Outcome::new(ActinglabCode::LabOperationCompleted))
}

/// `--version`.
fn version() -> Exit {
    let mut members = Map::new();
    members.insert("name".to_owned(), Value::from(PROGRAM));
    members.insert(
        "cli_version".to_owned(),
        Value::from(env!("CARGO_PKG_VERSION")),
    );
    members.insert(
        "schema_version".to_owned(),
        Value::from(crate::SCHEMA_VERSION),
    );
    cli_line::success(members, &Outcome::new(ActinglabCode::LabOperationCompleted))
}

/// Prints `exit`: the JSON line with `json`, else the lines for people ([`people`]); then its
/// developer line on stderr. A line stdout refuses, or one that did not encode, leaves only the
/// developer line `actingctl_output_failed`, exit 1.
pub(crate) fn print(exit: &Exit, json: bool) -> ExitCode {
    let line = match &exit.line {
        Ok(line) => line,
        Err(text) => return output_failed(text),
    };
    let text = if json { line.to_string() } else { people(line) };
    let written = {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        writeln!(writer, "{text}").and_then(|()| writer.flush())
    };
    if let Err(error) = written {
        return output_failed(&error.to_string());
    }
    if let Some(developer) = &exit.developer {
        eprintln!("{}", developer.line(PROGRAM));
    }
    ExitCode::from(exit.code)
}

fn output_failed(text: &str) -> ExitCode {
    eprintln!(
        "{}",
        cli_line::developer_text(PROGRAM, &Exit::output_failed(), Some(text))
    );
    ExitCode::FAILURE
}

/// A line for people: the result code with the outcome's values (the outcome catalog holds no
/// phrase for a code, and none is made up), then the result's members as a small table, a
/// scalar as it is and a list or an object by its size.
pub(crate) fn people(line: &Value) -> String {
    let outcome = line.get("outcome");
    let mut text = outcome
        .and_then(|outcome| outcome.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("outcome")
        .to_owned();
    if let Some(values) = outcome
        .and_then(|outcome| outcome.get("values"))
        .and_then(Value::as_object)
        .filter(|values| !values.is_empty())
    {
        let values = values
            .iter()
            .map(|(name, value)| format!("{name}={}", shown(value)))
            .collect::<Vec<_>>();
        text = format!("{text} ({})", values.join(", "));
    }
    let Some(members) = line.as_object() else {
        return text;
    };
    let rows = members
        .iter()
        .filter(|(name, _)| name.as_str() != "outcome")
        .collect::<Vec<_>>();
    let width = rows
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    for (name, value) in rows {
        let value = match value {
            Value::Array(items) => format!("[{} items]", items.len()),
            Value::Object(members) => format!("{{{} members}}", members.len()),
            other => shown(other),
        };
        text = format!("{text}\n  {name:width$}  {value}");
    }
    text
}

/// A scalar as people read it: a string without its quotes.
fn shown(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
