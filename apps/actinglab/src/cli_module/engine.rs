// SPDX-License-Identifier: AGPL-3.0-only

//! The operations actinglab's own commands serve (Workflow #434 LAB-2; register §1.287, LAB-2
//! row): the debugging and authoring rows, the core rows `app launch|restart|stop` (actinglab's
//! `session app`) and the two `user-config` rows. Each row names its command in `params_more`
//! (`actinglab <words>`), or below. The operation's short words are given to that command: its
//! words, the row's positional words and the further words, then the instance as `--instance`,
//! the row's options and switches and every further flag as the caller gave it.
//!
//! The command answers as it did before LAB-2 (LAB-2 fix 1, the coordinator's ruling of
//! 2026-11-10): on the command line `main` prints its output and exits with its exit code, and
//! `serve --stdio` answers the envelope its `--json` prints ([`envelope`]).

use super::{JSON, words};
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

/// The answer of `serve --stdio` to `operation` with `params`: the envelope `actinglab --json`
/// prints for the command line of its command, with that command's exit code.
pub(super) fn envelope(operation: &Operation, params: &Map<String, Value>) -> Exit {
    let Some(words) = arguments(operation, params) else {
        return cli_line::failure(
            &cli_line::usage_invalid(Some(operation.name.as_str()), ArgumentProblem::Unknown),
            Some("no command of actinglab serves this operation"),
        );
    };
    let line = std::iter::once(JSON.to_owned()).chain(words);
    let result = crate::run_invocation(crate::cli_parse::parse_invocation(line, true));
    let code = u8::try_from(result.exit_code()).unwrap_or(1);
    match serde_json::from_str::<Value>(&result.envelope_json()) {
        Ok(envelope) => Exit {
            line: Ok(envelope),
            developer: None,
            code,
        },
        Err(error) => Exit {
            line: Err(format!("the envelope cannot be read back: {error}")),
            developer: None,
            code: 1,
        },
    }
}

/// The command line of `operation`: its words, the row's positional words, the further words,
/// then the instance, the row's options and switches and the further flags. Positional words go
/// first so that a switch is never followed by one (actinglab reads `--flag word` as a value).
pub(super) fn arguments(operation: &Operation, params: &Map<String, Value>) -> Option<Vec<String>> {
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
