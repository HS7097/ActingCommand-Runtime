// SPDX-License-Identifier: AGPL-3.0-only

//! The CLI module of `actinglab.exe` and `actinglab serve --stdio` (Workflow #434 LAB-2; register
//! §1.287, "Slice LAB-2"; `contracts/lab-two-entries.md` §4.2, §8.2 and §9.3).
//!
//! - **Which command lines** ([`route`]; LAB-2 fix 1, the coordinator's ruling of 2026-11-10:
//!   this slice changes nothing any current consumer of actinglab gets). The module answers
//!   `serve` and the operations it serves itself, those that came from actingctl ([`core_ops`]:
//!   the core rows `lab-ops` serves, and `run start`, `run stop`, `reset` and `drill`). Every
//!   other command line is actinglab's own and `main` answers it as before LAB-2: its output for
//!   people, its `--json` envelope (`ok`, `data`, `error`) and its exit codes, with its global
//!   options anywhere on the line. Two core rows take words actinglab's own commands had before
//!   LAB-2, `status` and `run show`: on the command line those words stay actinglab's own commands
//!   until actinglab's commands move to the new output (the contract's open point 10); the rows
//!   answer through `serve --stdio`. A row actinglab's own command serves under other words
//!   (`watch` for `lab watch`, `recover` for `session recover`, `user-config [<key>]` for
//!   `user-config get`, `app launch` for `session app launch` and the like) is given to that
//!   command in its words ([`engine`]) and answers as it does.
//! - **Words.** Short words taken from the operation table (`contracts/lab-operations.json`): the
//!   operations of the core, debugging and authoring modules that LAB-1, LAB-1b and LAB-2 built
//!   ([`words`]). Every operation offered is a table row. The instance is a positional (an alias,
//!   an `instance_id` or an ADB port); there is no `--root` and no `--state-root` ([`place`]).
//! - **Output** of the module's operations. For people by default: one line with the result code
//!   (the outcome catalog holds no phrase for a code, so the code alone, with its values) and the
//!   result's members as a small table. `--json` prints the one line `cli_line` builds,
//!   `{…result, "outcome": …}`. Exit codes 0, 1 and 2, by the outcome's category; a refusal exits
//!   at least 1.
//! - **`serve --stdio`.** One `{op, params}` per line on stdin, one answer per line on stdout:
//!   the line the operation's `--json` prints, the module's line for the module's operations and
//!   the envelope for actinglab's own commands ([`serve`]).
//! - **Initiator** (§4.1, §4.2). `Lab-CLI`, or `Lab-制包` / `Lab-救急` from
//!   `ACTINGCOMMAND_INITIATOR`; any other value is refused before anything is sent or written. A
//!   Lab recording's steps and marks carry it (open point 6, the coordinator's ruling), and
//!   `Lab-MCP` when MCP's actinglab child (`--via mcp`) adds them.

mod core_ops;
mod engine;
mod place;
pub(crate) mod serve;
pub(crate) mod words;

use crate::{CliError, ErrorKind};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_lab_ops::initiator::{self, Initiator};
use actingcommand_lab_ops::table::OperationTable;
use actingcommand_runtime_client::ChannelVia;
use serde_json::Value;
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::OnceLock;
use words::Call;

/// The name the developer line gives.
pub(crate) const PROGRAM: &str = "actinglab";
/// The front end's switch for the JSON line.
pub(crate) const JSON: &str = "--json";
/// The word of the resident mode.
const SERVE: &str = "serve";
/// The developer text of a command line the module refuses.
const USAGE: &str = "usage: actinglab [--json] <operation words> [<positionals>] \
     [--<option> <value>] [--<switch>]; contracts/lab-operations.json lists the operations; \
     actinglab serve --stdio answers one {op, params} per line";
/// The developer text of an `ACTINGCOMMAND_INITIATOR` this process does not take.
const INITIATOR_TEXT: &str = "ACTINGCOMMAND_INITIATOR takes Lab-制包 or Lab-救急; without it \
     actinglab is Lab-CLI";

/// What a command line is for (LAB-2 fix 1).
#[derive(Debug)]
pub(crate) enum Route {
    /// `serve`, or an operation this module serves itself: the module's lines.
    Module(Vec<String>),
    /// actinglab's own command line, as given or in the words of the row's command: answered by
    /// `main` as before LAB-2.
    Own(Vec<String>),
    /// The short words of a row actinglab's own command serves, refused: the operation's name,
    /// whether the line asks for `--json`, and actinglab's usage error, answered by `main` as its
    /// own refusals are.
    Refused {
        command: String,
        json: bool,
        error: CliError,
    },
}

/// The route of the command line `arguments` (LAB-2 fix 1; see the module's description).
pub(crate) fn route(arguments: Vec<String>) -> Route {
    let (json, words) = front_end(arguments.clone());
    if words.first().map(String::as_str) == Some(SERVE) {
        return Route::Module(arguments);
    }
    let table = table().ok();
    let named = table.and_then(|table| words::named(table, &words));
    if let Some(operation) = named
        && core_ops::serves(operation)
        && !crate::own_command(&name_words(&operation.name))
    {
        return Route::Module(arguments);
    }
    if crate::own_command_line(&arguments) {
        return Route::Own(arguments);
    }
    // The table's failure is the module's to report.
    let Some(table) = table else {
        return Route::Module(arguments);
    };
    let Some(operation) = named.filter(|operation| !core_ops::serves(operation)) else {
        return Route::Own(arguments);
    };
    let refused = |error: CliError| Route::Refused {
        command: operation.name.clone(),
        json,
        error,
    };
    let call = match words::parse(table, &words) {
        Ok(call) => call,
        Err(refusal) => return refused(refused_words(&operation.name, &refusal)),
    };
    match engine::arguments(operation, &call.params) {
        Some(own) => Route::Own(
            json.then(|| JSON.to_owned())
                .into_iter()
                .chain(own)
                .collect(),
        ),
        None => refused(CliError::usage(format!(
            "{}: no command of actinglab serves this operation",
            operation.name
        ))),
    }
}

/// An operation's name as command words.
fn name_words(name: &str) -> Vec<String> {
    name.split(' ').map(str::to_owned).collect()
}

/// actinglab's usage error for the refused short words of the row `name`: the refusal's
/// `argument` and `argument_problem` in its details.
fn refused_words(name: &str, refusal: &Outcome) -> CliError {
    let values = cli_line::outcome_value(refusal, Detail::Full)
        .get("values")
        .cloned()
        .unwrap_or(Value::Null);
    let shown = |member: &str| {
        values
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_owned()
    };
    CliError::usage(format!(
        "{name}: {} {}",
        shown("argument_problem"),
        shown("argument")
    ))
    .with_details(values)
}

/// The front end's `--json` taken out of `arguments`: whether it was there, and the words.
pub(crate) fn front_end(arguments: Vec<String>) -> (bool, Vec<String>) {
    let json = arguments.iter().any(|argument| argument == JSON);
    let words = arguments
        .into_iter()
        .filter(|argument| argument != JSON)
        .collect();
    (json, words)
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

/// The initiator word every step and mark this process adds to a Lab recording carries (§4.1,
/// §4.2): `Lab-MCP` when MCP's actinglab child runs the command (`via` `mcp`, LAB-2 fix 1), else
/// `Lab-CLI`, or the `Lab-制包` / `Lab-救急` of `ACTINGCOMMAND_INITIATOR`. Any other value of the
/// variable is `actingctl_usage_invalid`, and nothing is written.
pub(crate) fn recording_initiator(via: Option<ChannelVia>) -> Result<&'static str, CliError> {
    if via == Some(ChannelVia::Mcp) {
        return Ok(Initiator::LabMcp.as_str());
    }
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
    let (json, words) = front_end(arguments);
    if words.first().map(String::as_str) == Some(SERVE) {
        return serve::main(&words[1..]);
    }
    print(&run_line(&words), json)
}

/// The exit of one command line of the module (the front end's `--json` taken out).
pub(crate) fn run_line(words: &[String]) -> Exit {
    if let Err(exit) = admitted() {
        return exit;
    }
    let table = match table() {
        Ok(table) => table,
        Err(outcome) => return cli_line::failure(&outcome, None),
    };
    match words::parse(table, words) {
        Ok(call) => answer(table, &call),
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

/// The answer to `call`: an operation of this module its line; one of actinglab's own commands
/// serves the envelope its `--json` prints (`serve --stdio`).
pub(crate) fn answer(table: &OperationTable, call: &Call) -> Exit {
    let Some(operation) = table.operation(&call.name) else {
        return cli_line::failure(
            &cli_line::usage_invalid(Some(call.name.as_str()), ArgumentProblem::Unknown),
            Some(USAGE),
        );
    };
    if core_ops::serves(operation) {
        return core_ops::answer(operation, &call.params);
    }
    engine::envelope(operation, &call.params)
}

/// Prints `exit`: the JSON line with `json`, else the lines for people ([`people`]); then its
/// developer line on stderr. A line stdout refuses, or one that did not encode, leaves only the
/// developer line `actingctl_output_failed`, exit 1.
pub(crate) fn print(exit: &Exit, json: bool) -> ExitCode {
    let text = match stdout_text(exit, json) {
        Ok(text) => text,
        Err(text) => return output_failed(&text),
    };
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

/// What `exit` prints on stdout: its JSON line with `json`, else the lines for people; the
/// developer text of a line that did not encode.
pub(crate) fn stdout_text(exit: &Exit, json: bool) -> Result<String, String> {
    match &exit.line {
        Ok(line) if json => Ok(line.to_string()),
        Ok(line) => Ok(people(line)),
        Err(text) => Err(text.clone()),
    }
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
