// SPDX-License-Identifier: AGPL-3.0-only

//! The CLI module of `actinglab.exe` and `actinglab serve --stdio` (Workflow #434 LAB-2; register
//! §1.287, "Slice LAB-2"; `contracts/lab-two-entries.md` §4.2, §8.2 and §9.3).
//!
//! - **Which command lines** ([`route`]; LAB-2 fix 1). The module answers `serve` and the
//!   operations it serves itself, those that came from actingctl ([`core_ops`]: the core rows
//!   `lab-ops` serves, `status` and `run show` among them since LAB-2b, and `run start`, `run
//!   stop`, `reset` and `drill`), also when actinglab's own global options stand before the core
//!   row's words, so that the row refuses them by name (LAB-2b fix 3; never a line with `--via`).
//!   Every other command line is actinglab's own, parsed and run by `main` with its global options
//!   anywhere on the line. Since LAB-2b (open point 10) it prints this module's line
//!   ([`own_line`]): the output for people by default, `--json` the `cli_line` line, exit codes 0,
//!   1 and 2; a line with `--via` (MCP's actinglab child) still prints the envelope (`ok`, `data`,
//!   `error`) with its exit codes 2 to 6, until LAB-7 (Workflow #434 LAB-5A: LAB-5 adds the new
//!   server and replaces nothing, contract §11). A row
//!   actinglab's own command serves under other words (`watch` for `lab watch`, `recover` for
//!   `session recover`, `user-config [<key>]` for `user-config get`, `app launch` for `session app
//!   launch` and the like) is given to that command in its words ([`engine`]) and answers as it
//!   does.
//! - **Words.** Short words taken from the operation table (`contracts/lab-operations.json`): the
//!   operations of the core, debugging and authoring modules that LAB-1, LAB-1b and LAB-2 built
//!   ([`words`]). Every operation offered is a table row. The instance is a positional (an alias,
//!   an `instance_id` or an ADB port), which (Workflow #434 CLI-IN) the command line also takes as
//!   actinglab's global `--instance <x>`, after the row's words or before them, never both; there
//!   is no `--root` and no `--state-root` ([`place`]).
//! - **Output** of the module's operations. For people by default: one line with the result code
//!   (the outcome catalog holds no phrase for a code, so the code alone, with its values) and the
//!   result's members as a small table. `--json` prints the one line `cli_line` builds,
//!   `{…result, "outcome": …}`. Exit codes 0, 1 and 2, by the outcome's category; a refusal exits
//!   at least 1.
//! - **`serve --stdio`.** One `{op, params}` per line on stdin, one answer per line on stdout:
//!   the line the operation's `--json` prints, the module's line for every operation, actinglab's
//!   own commands included (LAB-2b; [`serve`]).
//! - **Initiator** (§4.1, §4.2). `Lab-CLI`, or `Lab-制包` / `Lab-救急` from
//!   `ACTINGCOMMAND_INITIATOR`; any other value is refused before anything is sent or written. A
//!   Lab recording's steps and marks carry it (open point 6, the coordinator's ruling), and
//!   `Lab-MCP` when MCP's actinglab child (`--via mcp`) adds them, or (Workflow #434 LAB-5G4a) a
//!   `serve --stdio --front mcp` process, an MCP server's child, whose rescue rows take `Lab-MCP`
//!   too ([`serve::Front`]).
//! - **The authoring option** (§8.3; Workflow #434 LAB-5F, [`authoring`]): it decides only
//!   whether the authoring rows are listed by default; they are served everywhere (fix 2).

// Workflow #434 LAB-5F: the authoring option.
pub(crate) mod authoring;
// Workflow #434 LAB-5C: what a row hands `lab-ops` is read by the slice's test.
// Workflow #434 LAB-5G1: the code that serves these rows moved to `lab_ops::core_ops`; this module
// gives it what stays actinglab's (`Caller`): the program's place (`place`), the developer text of
// a state root it could not locate, and actinglab's code `lab_operation_completed`; and, as before,
// a connection made for each call as the program in force connects.
pub(crate) mod core_ops {
    use super::place;
    use crate::codes::ActinglabCode;
    use actingcommand_lab_ops::cli_line::Exit;
    pub(crate) use actingcommand_lab_ops::core_ops::serves;
    use actingcommand_lab_ops::core_ops::{self as serving, Caller, Place};
    #[cfg(test)]
    pub(crate) use actingcommand_lab_ops::core_ops::{
        MaterialRead, handed_words, material_read, offline_words,
    };
    use actingcommand_lab_ops::front::Connection;
    use actingcommand_lab_ops::table::Operation;
    use serde_json::{Map, Value};

    /// The operation `operation` with `params`, as its line.
    pub(crate) fn answer(operation: &Operation, params: &Map<String, Value>) -> Exit {
        serving::answer(operation, params, &caller())
    }

    /// What this program gives the serving code (the slice's test serves with it too).
    pub(crate) fn caller() -> Caller<'static> {
        Caller {
            place: &here,
            unlocated: place::UNLOCATED,
            completed: ActinglabCode::LabOperationCompleted.into(),
            connection: Connection::per_call(),
        }
    }

    /// This process's place.
    fn here() -> Place {
        let located = place::place();
        Place {
            state_root: located.state_root,
            root: located.root,
        }
    }

    /// Workflow #434 LAB-5C: `material --out`'s line for a material written into `out`.
    #[cfg(test)]
    pub(crate) fn material_written(
        out: &std::path::Path,
        sha256: &str,
        extension: &str,
        bytes: &[u8],
    ) -> Exit {
        serving::material_written(
            out,
            sha256,
            extension,
            bytes,
            ActinglabCode::LabOperationCompleted.into(),
        )
    }
}
// Workflow #434 LAB-5A: its command table is read by the slice's test.
pub(crate) mod engine;
// Workflow #434 LAB-5B2: the kinds of the debugging and authoring rows and what each strict row's
// own command accepts, read by the slice's test only.
#[cfg(test)]
pub(crate) mod own_flags;
// Workflow #434 LAB-2b: the line actinglab's own commands print.
pub(crate) mod own_line;
mod place;
pub(crate) mod serve;
pub(crate) mod words;

use crate::{CliError, ErrorKind};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_lab_ops::initiator::{self, Initiator};
use actingcommand_lab_ops::table::{OperationModule, OperationTable};
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
     [--<option> <value>] [--<switch>]; a row's <instance> may be written --instance <instance> \
     instead; contracts/lab-operations.json lists the operations; actinglab serve --stdio answers \
     one {op, params} per line";
/// The developer text of an `ACTINGCOMMAND_INITIATOR` this process does not take.
const INITIATOR_TEXT: &str = "ACTINGCOMMAND_INITIATOR takes Lab-制包 or Lab-救急; without it \
     actinglab is Lab-CLI";

/// What a command line is for (LAB-2 fix 1).
#[derive(Debug)]
pub(crate) enum Route {
    /// `serve`, or an operation this module serves itself: the module's lines.
    Module(Vec<String>),
    /// actinglab's own command line, as given or in the words of the row's command: parsed and
    /// run by `main`, printed with [`own_line`] (or as the envelope with `--via`).
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
    let loaded = table().ok();
    // Workflow #434 CLI-IN: also the row whose words follow `--instance <x>` when it takes the
    // instance (`words::instance_first`), but on a line with `--via`, which stays as it was.
    let named = loaded.and_then(|table| {
        words::named(table, &words).or_else(|| {
            if own_line::via_line(&arguments) {
                None
            } else {
                words::instance_first(table, &words).map(|(operation, _)| operation)
            }
        })
    });
    if let Some(operation) = named
        && core_ops::serves(operation)
        && !crate::own_command(&name_words(&operation.name))
    {
        return Route::Module(arguments);
    }
    // Workflow #434 LAB-3: the rescue module's rows; none takes a word of actinglab's own commands.
    if let Some(operation) = named
        && operation.module == OperationModule::Rescue
        && !crate::own_command(&name_words(&operation.name))
    {
        return Route::Module(arguments);
    }
    if crate::own_command_line(&arguments) {
        return Route::Own(arguments);
    }
    // The table's failure is the module's to report.
    let Some(table) = loaded else {
        return Route::Module(arguments);
    };
    // Workflow #434 LAB-2b fix 3: actinglab's own global options before a core row's words
    // (`--runtime-endpoint <endpoint> status`) give the line to the module as it is, which refuses
    // the first of them by name, as it refuses one after the words, and sends nothing. A line
    // with `--via` stays actinglab's own.
    if named.is_none()
        && !own_line::via_line(&arguments)
        && core_row_after_options(table, &arguments)
    {
        return Route::Module(arguments);
    }
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

/// Workflow #434 LAB-2b fix 3: whether the words of the command line `arguments`, actinglab's own
/// global options taken out, name a core row this module serves that is no command of actinglab's
/// own (`status`, `run show`).
fn core_row_after_options(table: &OperationTable, arguments: &[String]) -> bool {
    let Ok(invocation) = crate::parse_invocation(arguments.iter().cloned(), true) else {
        return false;
    };
    let command_words = [invocation.command, invocation.args].concat();
    words::named(table, &command_words).is_some_and(|operation| {
        core_ops::serves(operation) && !crate::own_command(&name_words(&operation.name))
    })
}

/// An operation's name as command words.
fn name_words(name: &str) -> Vec<String> {
    name.split(' ').map(str::to_owned).collect()
}

/// Workflow #434 LAB-5B4 (register §1.287, "Slice LAB-5B4"): actinglab's usage error for a word
/// the own command `command` does not read on its command line (a flag off its read list, a
/// positional beyond it) or for an option at the end of it without its value: the refusal the
/// module gives a row's short words ([`refused_words`]), `argument` the word and
/// `argument_problem` `problem`.
pub(crate) fn refused_word(command: &str, word: &str, problem: ArgumentProblem) -> CliError {
    refused_words(command, &cli_line::usage_invalid(Some(word), problem))
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
/// §4.2): `Lab-MCP` when MCP's actinglab child runs the command (`via` `mcp`, LAB-2 fix 1) or
/// `serve --stdio --front mcp` does (Workflow #434 LAB-5G4a: the loop runs the command with `via`
/// `mcp`, [`serve::Front::via`]), else `Lab-CLI`, or the `Lab-制包` / `Lab-救急` of
/// `ACTINGCOMMAND_INITIATOR`. Any other value of the variable is `actingctl_usage_invalid`, and
/// nothing is written.
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
    // The register's LAB-2 row: `status` reads the overview's `watchdog` block through
    // `crates/watchdog` (LAB-4), as actingctl does; on Windows only, where the watchdog runs.
    #[cfg(windows)]
    actingcommand_lab_ops::overview::set_watchdog_block(actingcommand_watchdog::overview_block);
    let (json, words) = front_end(arguments);
    if words.first().map(String::as_str) == Some(SERVE) {
        return serve::main(&words[1..]);
    }
    #[cfg(windows)]
    if !json && let Some(exit) = agent_channel(&words) {
        return exit;
    }
    print(&run_line(&words), json)
}

/// Workflow #434 LAB-3: the rescue module's `agent …` rows on the command line without `--json`,
/// printed as `actingctl agent-channel` prints them (`contracts/agent-forward.md`): a hook that
/// did its work prints nothing (its stdout would enter the session's context), the notice its
/// text, a registered session's failed hook only its developer line, exit 2; the other answers
/// their line. `None` for every other command line.
#[cfg(windows)]
fn agent_channel(words: &[String]) -> Option<ExitCode> {
    use actingcommand_lab_rescue::agent_channel::{Answer, HOOK_EXIT};
    let table = table().ok()?;
    let call = words::parse(table, words).ok()?;
    if !call.name.starts_with("agent ") {
        return None;
    }
    if let Err(exit) = admitted() {
        return Some(print(&exit, false));
    }
    Some(match rescue::answer(&call, None) {
        Answer::Line(exit) => print(&exit, false),
        Answer::Text(text) => {
            let written = {
                let stdout = io::stdout();
                let mut writer = stdout.lock();
                writer
                    .write_all(text.as_bytes())
                    .and_then(|()| writer.flush())
            };
            match written {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => output_failed(&error.to_string()),
            }
        }
        Answer::Silent => ExitCode::SUCCESS,
        Answer::Hook { outcome, text } => {
            eprintln!(
                "{}",
                cli_line::developer_text(PROGRAM, &outcome, Some(&text))
            );
            ExitCode::from(HOOK_EXIT)
        }
    })
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
        Ok(call) => answer(table, &call, None),
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
/// serves the line its `--json` prints (`serve --stdio`; [`own_line`], Workflow #434 LAB-2b).
/// `front` is the front end `serve --stdio --front` named (Workflow #434 LAB-5G4a), `None` on the
/// command line and without the option.
pub(crate) fn answer(table: &OperationTable, call: &Call, front: Option<serve::Front>) -> Exit {
    let Some(operation) = table.operation(&call.name) else {
        return cli_line::failure(
            &cli_line::usage_invalid(Some(call.name.as_str()), ArgumentProblem::Unknown),
            Some(USAGE),
        );
    };
    if core_ops::serves(operation) {
        return core_ops::answer(operation, &call.params);
    }
    // Workflow #434 LAB-3: the rescue module's rows (offered on Windows only, where it runs).
    #[cfg(windows)]
    if operation.module == OperationModule::Rescue {
        return rescue::line(rescue::answer(call, front));
    }
    engine::line(operation, &call.params, front)
}

/// Workflow #434 LAB-3: the rescue module's rows (`crates/lab-rescue`), for the root actinglab
/// runs from and with this process's initiator.
#[cfg(windows)]
mod rescue {
    use super::serve::Front;
    use super::words::Call;
    use actingcommand_contract::outcome::Outcome;
    use actingcommand_lab_ops::cli_line::{self, Exit};
    use actingcommand_lab_ops::codes::ActingctlCode;
    use actingcommand_lab_ops::initiator::Initiator;
    use actingcommand_lab_rescue::agent_channel::Answer;
    use serde_json::{Map, Value};

    /// The rescue row `call` names, answered; under `serve --stdio --front mcp` (`front`; Workflow
    /// #434 LAB-5G4a) with the initiator `Lab-MCP`, else with this process's word (§4.2).
    pub(super) fn answer(call: &Call, front: Option<Front>) -> Answer {
        let initiator = match front {
            Some(Front::Mcp) => Ok(Initiator::LabMcp),
            None => Initiator::of_this_lab_process(),
        };
        match initiator {
            Ok(initiator) => {
                actingcommand_lab_rescue::rescue::answer_here(&call.name, &call.params, initiator)
            }
            Err(outcome) => Answer::Line(cli_line::failure(&outcome, None)),
        }
    }

    /// The one line of an answer, as `--json` and `serve --stdio` print it: a hook that did its
    /// work and the notice, which the command line prints as they are (`super::main`), are the
    /// line `agent_channel_reported` (with the notice's `text`); a failed hook its failure.
    pub(super) fn line(answered: Answer) -> Exit {
        match answered {
            Answer::Line(exit) => exit,
            Answer::Text(text) => {
                let mut members = Map::new();
                members.insert("text".to_owned(), Value::String(text.to_owned()));
                cli_line::success(members, &Outcome::new(ActingctlCode::AgentChannelReported))
            }
            Answer::Silent => cli_line::success(
                Map::new(),
                &Outcome::new(ActingctlCode::AgentChannelReported),
            ),
            Answer::Hook { outcome, text } => cli_line::refused(&outcome, Some(&text)),
        }
    }
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
