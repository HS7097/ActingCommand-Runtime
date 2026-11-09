// SPDX-License-Identifier: AGPL-3.0-only

//! The debugging and authoring operations served by actinglab's own commands (Workflow #434
//! LAB-2; register §1.287, LAB-2 row), and the core operations that are actinglab's own
//! (`app launch|restart|stop`, its `session app`) and the two `user-config` rows. Each row names
//! its command in `params_more` (`actinglab <words>`), or below: the operation's words are given
//! to that command with the row's parameters (the instance as `--instance`) and every further flag
//! and word as the caller gave it, and the command's answer becomes the line: its data beside
//! `lab_operation_completed`, or its error's code (relayed: a code the catalog does not list is
//! `outcome_relay_unregistered` with `raw_code`) with every member of its details beside it and
//! its message as the developer text. `lab_operation_check_failed` so keeps `req_id`, `effect`,
//! `executed` and the rest, and exits 1.

use super::words;
use crate::codes::ActinglabCode;
use crate::{CliError, CliOutcome};
use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::codes::ArgumentProblem;
use actingcommand_lab_ops::table::{Operation, ParamForm, ParamType};
use serde_json::{Map, Value};

/// The command of a row whose `params_more` names none of actinglab's.
const COMMANDS: [(&str, &[&str]); 5] = [
    ("app launch", &["session", "app", "launch"]),
    ("app restart", &["session", "app", "restart"]),
    ("app stop", &["session", "app", "stop"]),
    // Open point 7 (coordinator's ruling): actinglab's own `config get|set`, renamed.
    ("user-config", &["user-config", "get"]),
    ("user-config set", &["user-config", "set"]),
];

/// actinglab's command words of `operation`; `None` for a row no command of actinglab serves.
fn command_words(operation: &Operation) -> Option<Vec<String>> {
    if let Some(words) = operation
        .params_more
        .as_deref()
        .and_then(|source| source.strip_prefix("actinglab "))
    {
        return Some(words.split(' ').map(str::to_owned).collect());
    }
    COMMANDS
        .iter()
        .find(|(name, _)| *name == operation.name)
        .map(|(_, words)| words.iter().map(|word| (*word).to_owned()).collect())
}

/// The operation `operation` with `params`, run by actinglab's command, as its line.
pub(super) fn execute(operation: &Operation, params: &Map<String, Value>) -> Exit {
    let Some(arguments) = arguments(operation, params) else {
        return cli_line::failure(
            &cli_line::usage_invalid(Some(operation.name.as_str()), ArgumentProblem::Unknown),
            Some("no command of actinglab serves this operation"),
        );
    };
    line(run(arguments))
}

/// The command line of `operation`: its words, the row's positional words, the further words,
/// then the instance, the row's options and switches and the further flags. Positional words go
/// first so that a switch is never followed by one (actinglab reads `--flag word` as a value).
fn arguments(operation: &Operation, params: &Map<String, Value>) -> Option<Vec<String>> {
    let mut line = command_words(operation)?;
    let mut flags = Vec::new();
    for param in &operation.params {
        let Some(value) = params.get(&param.name) else {
            continue;
        };
        match param.form {
            ParamForm::Positional if param.value_type == ParamType::Instance => {
                flags.push("--instance".to_owned());
                flags.push(scalar(value));
            }
            ParamForm::Positional => line.extend(each(value).map(scalar)),
            ParamForm::Switch => {
                if value.as_bool() == Some(true) {
                    flags.push(flag(&param.name));
                }
            }
            ParamForm::Option => {
                for item in each(value) {
                    flags.push(flag(&param.name));
                    flags.push(scalar(item));
                }
            }
        }
    }
    if let Some(further) = params.get(words::MORE_WORDS) {
        line.extend(each(further).map(scalar));
    }
    for (name, value) in words::further_flags(operation, params) {
        for item in each(value) {
            match item {
                Value::Bool(true) => flags.push(flag(name)),
                Value::Bool(false) => {}
                other => {
                    flags.push(flag(name));
                    flags.push(scalar(other));
                }
            }
        }
    }
    line.extend(flags);
    Some(line)
}

/// actinglab's command run on `arguments`: its data or its error.
fn run(arguments: Vec<String>) -> CliOutcome<Value> {
    let invocation =
        crate::cli_parse::parse_invocation(arguments, true).map_err(|(_, _, error)| error)?;
    crate::execute(&invocation)
}

/// The line of a command's answer.
pub(crate) fn line(answer: CliOutcome<Value>) -> Exit {
    match answer {
        Ok(data) => cli_line::success(
            members(data),
            &Outcome::new(ActinglabCode::LabOperationCompleted),
        ),
        Err(error) => failed(error),
    }
}

/// A command's error: its code relayed, every member of its details beside it, and its message
/// as the developer text; non-zero (exit 1, or 2 for a fatal code).
fn failed(error: CliError) -> Exit {
    let outcome = Outcome::relayed(&error.code);
    let mut members = Map::new();
    match error.details {
        Some(Value::Object(details)) if !details.contains_key("outcome") => members.extend(details),
        Some(Value::Null) | None => {}
        Some(other) => {
            members.insert("details".to_owned(), other);
        }
    }
    if !error.blocked_by.is_empty() && !members.contains_key("blocked_by") {
        members.insert("blocked_by".to_owned(), Value::from(error.blocked_by));
    }
    cli_line::finish(Some(members), &outcome, true, Some(&error.message))
}

/// A command's data as the members of its line: an object's members, or anything else (and an
/// object with a member named `outcome`, the line's own) under `result`.
fn members(data: Value) -> Map<String, Value> {
    match data {
        Value::Object(members) if !members.contains_key("outcome") => members,
        other => {
            let mut members = Map::new();
            members.insert("result".to_owned(), other);
            members
        }
    }
}

/// The items of a value given once or repeated.
fn each(value: &Value) -> Box<dyn Iterator<Item = &Value> + '_> {
    match value {
        Value::Array(items) => Box::new(items.iter()),
        other => Box::new(std::iter::once(other)),
    }
}

fn flag(name: &str) -> String {
    format!("--{}", name.replace('_', "-"))
}

/// A value as one command-line word.
fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
