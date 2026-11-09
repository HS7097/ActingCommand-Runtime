// SPDX-License-Identifier: AGPL-3.0-only

//! The operation and its parameters, from the CLI's short words or from one line of
//! `serve --stdio` (Workflow #434 LAB-2; `contracts/lab-two-entries.md` §9.1, §9.3). Both give the
//! same [`Call`]: the operation's name and its parameters as JSON members named as the row names
//! them, so that the CLI's `--json` answer and the serve answer of one operation are the same.
//!
//! - The words are the longest operation name of the table that begins the command line, among
//!   the operations this program offers ([`offered`]); then the row's positionals in order (the
//!   instance among them: an alias, an `instance_id` or an ADB port), its options
//!   `--name value` (`_` written `-`) and its switches `--name`.
//! - A row with `params_more` takes the further flags of its source command as they are given
//!   (`--flag value`, or `--flag` alone when no value follows), and further words, which
//!   [`MORE_WORDS`] holds; any other word or flag is refused by name.
//! - Types: `integer` a JSON number, `boolean` true or false, `json` the JSON value,
//!   `list<string>` comma-separated words, `duration` a number with `ms`, `s`, `m` or `h`, kept as
//!   written; a repeated parameter is a list. Everything else is a string.
//!
//! Every refusal is `actingctl_usage_invalid` naming the word (`argument`) and the rule
//! (`argument_problem`).

use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::codes::ArgumentProblem;
use actingcommand_lab_ops::table::{
    Operation, OperationModule, OperationStatus, OperationTable, Param, ParamForm, ParamType,
};
use serde_json::{Map, Value};
use std::time::Duration;

/// The slices whose operations this program serves: the core rows LAB-1 and LAB-1b moved into
/// `lab-ops` and the rows of LAB-2. The rescue module's rows come with LAB-3 to LAB-5, a reserved
/// row with the slice that opens it.
pub(crate) const BUILT_SLICES: [&str; 3] = ["LAB-1", "LAB-1b", "LAB-2"];

/// The member that holds the further words of a row with `params_more`.
pub(crate) const MORE_WORDS: &str = "words";

/// One operation and its parameters.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Call {
    pub(crate) name: String,
    pub(crate) params: Map<String, Value>,
}

/// Whether this program offers `operation`: a planned row of the core, debugging or authoring
/// module whose slice is built.
pub(crate) fn offered(operation: &Operation) -> bool {
    operation.status == OperationStatus::Planned
        && operation.module != OperationModule::Rescue
        && BUILT_SLICES.contains(&operation.slice.as_str())
}

/// The offered operation whose name begins the command line `words`, the longest; `None` when no
/// offered name does.
pub(crate) fn named<'a>(table: &'a OperationTable, words: &[String]) -> Option<&'a Operation> {
    table
        .operations
        .iter()
        .filter(|operation| offered(operation))
        .filter(|operation| {
            let name = operation.name.split(' ').collect::<Vec<_>>();
            words.len() >= name.len()
                && name
                    .iter()
                    .zip(words)
                    .all(|(own, word)| *own == word.as_str())
        })
        .max_by_key(|operation| operation.name.split(' ').count())
}

/// The call the command line `words` (after the front end's own `--json`) names.
pub(crate) fn parse(table: &OperationTable, words: &[String]) -> Result<Call, Outcome> {
    let operation = named(table, words)
        .ok_or_else(|| usage(words.first().map(String::as_str), ArgumentProblem::Unknown))?;
    let rest = &words[operation.name.split(' ').count()..];
    let mut params = Map::new();
    let positionals = operation
        .params
        .iter()
        .filter(|param| param.form == ParamForm::Positional)
        .collect::<Vec<_>>();
    let mut next = 0;
    let more = operation.params_more.is_some();
    let mut index = 0;
    while index < rest.len() {
        let word = rest[index].as_str();
        if let Some(flag) = word.strip_prefix("--") {
            let name = flag.replace('-', "_");
            let declared = operation
                .params
                .iter()
                .find(|param| param.form != ParamForm::Positional && param.name == name);
            match declared {
                Some(param) if param.form == ParamForm::Switch => {
                    put(&mut params, param, Value::Bool(true), word)?;
                    index += 1;
                }
                Some(param) => {
                    let value = rest
                        .get(index + 1)
                        .ok_or_else(|| usage(Some(word), ArgumentProblem::ValueMissing))?;
                    put(&mut params, param, typed(param, value, word)?, word)?;
                    index += 2;
                }
                None if more && name != MORE_WORDS && !name.is_empty() => {
                    match rest.get(index + 1).filter(|next| !next.starts_with("--")) {
                        Some(value) => {
                            push(&mut params, &name, Value::String(value.clone()));
                            index += 2;
                        }
                        None => {
                            push(&mut params, &name, Value::Bool(true));
                            index += 1;
                        }
                    }
                }
                None => return Err(usage(Some(word), ArgumentProblem::Unknown)),
            }
        } else {
            match positionals.get(next) {
                Some(param) if param.repeat => {
                    append(&mut params, &param.name, typed(param, word, word)?);
                }
                Some(param) => {
                    put(&mut params, param, typed(param, word, word)?, word)?;
                    next += 1;
                }
                None if more => append(&mut params, MORE_WORDS, Value::String(word.to_owned())),
                None => return Err(usage(Some(word), ArgumentProblem::Unknown)),
            }
            index += 1;
        }
    }
    required(operation, &params)?;
    Ok(Call {
        name: operation.name.clone(),
        params,
    })
}

/// The call one `serve --stdio` line `{op, params}` names; the parameters are those the CLI's
/// words give, as JSON.
pub(crate) fn from_json(table: &OperationTable, line: &str) -> Result<Call, Outcome> {
    let request = serde_json::from_str::<Value>(line).map_err(|error| {
        usage(None, ArgumentProblem::WrongType)
            .caused_by(actingcommand_lab_ops::foreign_data_error(&error))
    })?;
    let Value::Object(request) = request else {
        return Err(usage(None, ArgumentProblem::WrongType));
    };
    if let Some(member) = request
        .keys()
        .find(|member| !matches!(member.as_str(), "op" | "params"))
    {
        return Err(usage(Some(member.as_str()), ArgumentProblem::Unknown));
    }
    let name = match request.get("op") {
        Some(Value::String(name)) => name,
        Some(_) => return Err(usage(Some("op"), ArgumentProblem::WrongType)),
        None => return Err(usage(Some("op"), ArgumentProblem::Required)),
    };
    let operation = table
        .operation(name)
        .filter(|operation| offered(operation))
        .ok_or_else(|| usage(Some(name.as_str()), ArgumentProblem::Unknown))?;
    let params = match request.get("params") {
        None => Map::new(),
        Some(Value::Object(params)) => params.clone(),
        Some(_) => return Err(usage(Some("params"), ArgumentProblem::WrongType)),
    };
    for (member, value) in &params {
        match operation.params.iter().find(|param| &param.name == member) {
            Some(param) => checked(param, value, member)?,
            None if operation.params_more.is_some() => further(member, value)?,
            None => return Err(usage(Some(member.as_str()), ArgumentProblem::Unknown)),
        }
    }
    required(operation, &params)?;
    Ok(Call {
        name: operation.name.clone(),
        params,
    })
}

/// The members of `params` the row does not declare: the further flags of its source command
/// (never [`MORE_WORDS`]), in name order.
pub(crate) fn further_flags<'a>(
    operation: &Operation,
    params: &'a Map<String, Value>,
) -> Vec<(&'a str, &'a Value)> {
    params
        .iter()
        .filter(|(name, _)| {
            name.as_str() != MORE_WORDS
                && !operation.params.iter().any(|param| &param.name == *name)
        })
        .map(|(name, value)| (name.as_str(), value))
        .collect()
}

/// A duration as the table writes it: a whole number with `ms`, `s`, `m` or `h`.
pub(crate) fn parse_duration(text: &str) -> Option<Duration> {
    let (number, unit) = text
        .find(|character: char| !character.is_ascii_digit())
        .map(|split| text.split_at(split))?;
    let number = number.parse::<u64>().ok()?;
    match unit {
        "ms" => Some(Duration::from_millis(number)),
        "s" => Some(Duration::from_secs(number)),
        "m" => number.checked_mul(60).map(Duration::from_secs),
        "h" => number.checked_mul(3600).map(Duration::from_secs),
        _ => None,
    }
}

/// The name a refusal gives a parameter: `<name>` for a positional, `--name` for another.
fn shown(param: &Param) -> String {
    match param.form {
        ParamForm::Positional => format!("<{}>", param.name),
        _ => format!("--{}", param.name.replace('_', "-")),
    }
}

/// Every required parameter is given.
fn required(operation: &Operation, params: &Map<String, Value>) -> Result<(), Outcome> {
    match operation
        .params
        .iter()
        .find(|param| param.required && !params.contains_key(&param.name))
    {
        Some(param) => Err(usage(
            Some(shown(param).as_str()),
            ArgumentProblem::Required,
        )),
        None => Ok(()),
    }
}

/// One declared parameter, given once unless it repeats.
fn put(
    params: &mut Map<String, Value>,
    param: &Param,
    value: Value,
    word: &str,
) -> Result<(), Outcome> {
    if param.repeat {
        append(params, &param.name, value);
        return Ok(());
    }
    if params.insert(param.name.clone(), value).is_some() {
        return Err(usage(Some(word), ArgumentProblem::Repeated));
    }
    Ok(())
}

/// One more value of a repeated parameter or of the further words: always a list.
fn append(params: &mut Map<String, Value>, name: &str, value: Value) {
    match params.get_mut(name) {
        Some(Value::Array(items)) => items.push(value),
        _ => {
            params.insert(name.to_owned(), Value::Array(vec![value]));
        }
    }
}

/// One more value of a further flag: the value itself, a list once it is given twice.
fn push(params: &mut Map<String, Value>, name: &str, value: Value) {
    match params.get_mut(name) {
        Some(Value::Array(items)) => items.push(value),
        Some(given) => {
            let first = given.take();
            *given = Value::Array(vec![first, value]);
        }
        None => {
            params.insert(name.to_owned(), value);
        }
    }
}

/// `text` as the value of `param`; `word` names it in a refusal.
fn typed(param: &Param, text: &str, word: &str) -> Result<Value, Outcome> {
    let wrong = || usage(Some(word), ArgumentProblem::WrongType);
    Ok(match &param.value_type {
        ParamType::Integer => Value::from(text.parse::<i64>().map_err(|_| wrong())?),
        ParamType::Boolean => match text {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => return Err(wrong()),
        },
        ParamType::Json => serde_json::from_str(text).map_err(|_| wrong())?,
        ParamType::ListOfStrings => Value::Array(
            text.split(',')
                .map(|item| Value::String(item.to_owned()))
                .collect(),
        ),
        ParamType::Duration => {
            parse_duration(text).ok_or_else(wrong)?;
            Value::String(text.to_owned())
        }
        ParamType::Enum(words) => {
            if !words.iter().any(|known| known == text) {
                return Err(usage(Some(word), ArgumentProblem::NotRecognised));
            }
            Value::String(text.to_owned())
        }
        _ => Value::String(text.to_owned()),
    })
}

/// A serve parameter's JSON value checked against its declared type, as [`typed`] gives it.
fn checked(param: &Param, value: &Value, member: &str) -> Result<(), Outcome> {
    let wrong = || usage(Some(member), ArgumentProblem::WrongType);
    let one = |value: &Value| -> Result<(), Outcome> {
        let fits = match &param.value_type {
            ParamType::Integer => value.is_i64() || value.is_u64(),
            ParamType::Boolean => value.is_boolean(),
            ParamType::Json => true,
            ParamType::ListOfStrings => value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string)),
            ParamType::Duration => value.as_str().and_then(parse_duration).is_some(),
            ParamType::Enum(words) => value
                .as_str()
                .is_some_and(|text| words.iter().any(|known| known == text)),
            _ => value.is_string(),
        };
        if fits { Ok(()) } else { Err(wrong()) }
    };
    match (param.repeat, value) {
        (true, Value::Array(items)) if !items.is_empty() => items.iter().try_for_each(&one),
        (true, _) => Err(wrong()),
        (false, _) if param.form == ParamForm::Switch => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(wrong())
            }
        }
        (false, value) => one(value),
    }
}

/// A further flag (`true`, a string, a number, or a list of them) or the further words (a list of
/// strings) of a serve call.
fn further(member: &str, value: &Value) -> Result<(), Outcome> {
    let scalar = |value: &Value| value.is_string() || value.is_boolean() || value.is_number();
    let fits = if member == MORE_WORDS {
        value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string))
    } else {
        scalar(value)
            || value
                .as_array()
                .is_some_and(|items| !items.is_empty() && items.iter().all(scalar))
    };
    if fits {
        Ok(())
    } else {
        Err(usage(Some(member), ArgumentProblem::WrongType))
    }
}

fn usage(argument: Option<&str>, problem: ArgumentProblem) -> Outcome {
    cli_line::usage_invalid(argument, problem)
}
