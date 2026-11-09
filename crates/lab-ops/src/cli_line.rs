// SPDX-License-Identifier: AGPL-3.0-only

//! The one result line of every actingctl exit (Workflow #378 A5; model v2.2 §7.1, §7.3,
//! §7.6). Every subcommand but `mcp-serve` and `mcp-config` prints exactly one JSON line on
//! stdout before any stderr text: `{…result, "outcome": …}` on success, `{"outcome": …}` on
//! failure. A refused or failed Runtime request whose receipt the subcommand prints keeps that
//! receipt beside its outcome (A4: a consumer judges refusal by the receipt's `state`).
//! stderr carries only the developer line `<LEVEL> actingctl: <code> …` of a warning, error
//! or fatal outcome. The process exits by the top outcome's category (0, 1 or 2); a refused
//! or failed receipt exits non-zero whatever its category.
//!
//! The outcome is full by default (class (b), §7.1); `ACTINGCOMMAND_DETAIL=codes` keeps only
//! its registered names. An unknown value is a usage error.
//!
//! Workflow #434 LAB-2 (register §1.287, LAB-2 row): this crate builds the line, the developer
//! line and the exit code of an exit ([`Exit`]) and returns them; the program prints them
//! (actingctl and actinglab each name themselves in the developer line). Nothing here writes to
//! stdout or stderr.

use crate::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_contract::outcome::{Category, Detail, Outcome};
use serde_json::{Map, Value};

/// The environment variable that switches actingctl's outcomes to codes only (§7.1).
pub const DETAIL_VARIABLE: &str = "ACTINGCOMMAND_DETAIL";
/// The longest command-line word a usage outcome names as its `argument` (the key table's name
/// bound).
const MAX_ARGUMENT_BYTES: usize = 256;

/// `actingctl_usage_invalid` for one command-line word and what is wrong with it
/// (`argument_problem`). The word is named as `argument` when it is one a name may hold (at most
/// 256 bytes, no control character); another word is refused unnamed.
pub fn usage_invalid(argument: Option<&str>, problem: ArgumentProblem) -> Outcome {
    let outcome = Outcome::new(ActingctlCode::ActingctlUsageInvalid).argument_problem(problem);
    match argument.filter(|argument| {
        argument.len() <= MAX_ARGUMENT_BYTES && !argument.chars().any(char::is_control)
    }) {
        Some(argument) => outcome.argument(argument),
        None => outcome,
    }
}

/// The detail `ACTINGCOMMAND_DETAIL` asks for: full when it is unset; `codes` or `full`
/// otherwise; any other value is refused with `actingctl_usage_invalid` and the variable.
pub fn detail() -> Result<Detail, Outcome> {
    match std::env::var_os(DETAIL_VARIABLE) {
        None => Ok(Detail::Full),
        Some(value) => match value.to_str() {
            Some("full") => Ok(Detail::Full),
            Some("codes") => Ok(Detail::Codes),
            _ => Err(Outcome::new(ActingctlCode::ActingctlUsageInvalid).variable(DETAIL_VARIABLE)),
        },
    }
}

/// The level word of a developer line (model §7.3: FATAL only for a fatal outcome).
pub const fn level(category: Category) -> &'static str {
    match category {
        Category::Success | Category::Info => "INFO",
        Category::Warning => "WARN",
        Category::Error => "ERROR",
        Category::Fatal => "FATAL",
    }
}

/// The developer line `<LEVEL> <program>: <outcome>[ <text>]` for stderr, without its line end;
/// `text` is developer English only (a usage line, a foreign detail the outcome does not hold).
/// Workflow #434 LAB-2: the program prints it and names itself (`actingctl`, `actinglab`).
pub fn developer_text(program: &str, outcome: &Outcome, text: Option<&str>) -> String {
    match text {
        Some(text) => format!("{} {program}: {outcome} {text}", level(outcome.category())),
        None => format!("{} {program}: {outcome}", level(outcome.category())),
    }
}

/// The developer line of an exit: its outcome and its developer text.
#[derive(Debug, Clone)]
pub struct Developer {
    pub outcome: Outcome,
    pub text: Option<String>,
}

impl Developer {
    /// The line `program` prints on stderr ([`developer_text`]).
    pub fn line(&self, program: &str) -> String {
        developer_text(program, &self.outcome, self.text.as_deref())
    }
}

/// What one exit prints and how it ends (Workflow #434 LAB-2): the result line for stdout, the
/// developer line for stderr when the exit has one, and the exit code. The program prints the
/// line first, then the developer line. A line that did not encode (`line` is `Err` with the
/// encoder's text) leaves only the developer line `actingctl_output_failed` with that text, and
/// the exit is 1 (§7.3); so does a stdout that refuses the line.
#[derive(Debug, Clone)]
pub struct Exit {
    pub line: Result<Value, String>,
    pub developer: Option<Developer>,
    pub code: u8,
}

impl Exit {
    /// The outcome of the developer line when the line could not be printed:
    /// `actingctl_output_failed`.
    pub fn output_failed() -> Outcome {
        Outcome::new(ActingctlCode::ActingctlOutputFailed)
    }
}

/// A successful exit: `{…result, "outcome": …}`.
pub fn success(result: Map<String, Value>, outcome: &Outcome) -> Exit {
    finish(Some(result), outcome, false, None)
}

/// A failed exit: `{"outcome": …}`, then the developer line with `text`.
pub fn failure(outcome: &Outcome, text: Option<&str>) -> Exit {
    finish(None, outcome, false, text)
}

/// A refused exit: `{outcome}` and the developer line with `text`, non-zero whatever the top's
/// category (a refusal the Runtime topped with an info or warning code still failed; A4 part B
/// L2).
pub fn refused(outcome: &Outcome, text: Option<&str>) -> Exit {
    finish(None, outcome, true, text)
}

/// The line of a subcommand whose result holds a Runtime receipt: `refused` is true when that
/// receipt's state is denied or failed, and the process then exits non-zero whatever the
/// outcome's category (A4 part B L2).
pub fn receipt(result: Map<String, Value>, outcome: &Outcome, refused: bool) -> Exit {
    finish(Some(result), outcome, refused, None)
}

/// A line with `result` beside `outcome`, its developer line with `text` (when `refused` or the
/// outcome is worse than info) and its exit code (see [`exit_code`]).
pub fn finish(
    result: Option<Map<String, Value>>,
    outcome: &Outcome,
    refused: bool,
    text: Option<&str>,
) -> Exit {
    let detail = detail().unwrap_or(Detail::Full);
    let line = line(result, outcome, detail).map_err(|error| error.to_string());
    let developer = (refused || outcome.category() > Category::Info).then(|| Developer {
        outcome: outcome.clone(),
        text: text.map(str::to_owned),
    });
    Exit {
        line,
        developer,
        code: exit_code(outcome.category(), refused),
    }
}

/// The result line: the result's members and `outcome`, the envelope in `detail`.
pub fn line(
    result: Option<Map<String, Value>>,
    outcome: &Outcome,
    detail: Detail,
) -> Result<Value, serde_json::Error> {
    let mut line = result.unwrap_or_default();
    debug_assert!(
        !line.contains_key("outcome"),
        "a result member may not be named outcome: the name is the envelope's"
    );
    line.insert(
        "outcome".to_owned(),
        serde_json::to_value(outcome.envelope(detail))?,
    );
    Ok(Value::Object(line))
}

/// The exit code of a top outcome of `category` (§7.6): 0 for success, info or warning, 1 for
/// error, 2 for fatal; a refused or failed receipt (`refused`) exits at least 1 (A4 part B L2).
pub fn exit_code(category: Category, refused: bool) -> u8 {
    category.exit_code().max(u8::from(refused))
}

/// An outcome as an interface value: its `actingcommand.outcome.v1` envelope in `detail`. An
/// envelope always serialises; were it ever to fail, the code alone is shown.
pub fn outcome_value(outcome: &Outcome, detail: Detail) -> Value {
    serde_json::to_value(outcome.envelope(detail))
        .unwrap_or_else(|_| Value::String(outcome.code().as_str().to_owned()))
}

/// A warning beside a result, as the CLI line and MCP both carry it: `{outcome}` (Workflow #378
/// A5; MCP-0 command table D3: a top-level `warnings` member, present only when not empty).
pub fn warning_value(outcome: &Outcome, detail: Detail) -> Value {
    let mut warning = Map::new();
    warning.insert("outcome".to_owned(), outcome_value(outcome, detail));
    Value::Object(warning)
}

/// A warning of this process's result line: [`warning_value`] in the process's detail.
pub fn warning(outcome: &Outcome) -> Value {
    warning_value(outcome, detail().unwrap_or(Detail::Full))
}

/// A JSON value as the members of a result line: an object's members, or any other value under
/// `member`.
pub fn members(value: Value, member: &str) -> Map<String, Value> {
    match value {
        Value::Object(members) => members,
        other => {
            let mut members = Map::new();
            members.insert(member.to_owned(), other);
            members
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Workflow #378 A5 (model v2.2 §7.3): a success line is the result's members and `outcome`;
    // a failure line is `{outcome}` alone; the outcome is the `actingcommand.outcome.v1` envelope.
    #[test]
    fn a_result_line_is_the_result_members_and_one_outcome_envelope() {
        let mut members = Map::new();
        members.insert("records".to_owned(), Value::Array(Vec::new()));
        let success = line(
            Some(members),
            &Outcome::new(ActingctlCode::RuntimeConfigReported),
            Detail::Full,
        )
        .expect("success line");
        let object = success.as_object().expect("an object");
        assert_eq!(
            object.keys().map(String::as_str).collect::<Vec<_>>(),
            ["records", "outcome"]
        );
        assert_eq!(
            success["outcome"]["schema_version"],
            actingcommand_contract::outcome::OUTCOME_SCHEMA_VERSION
        );
        assert_eq!(success["outcome"]["code"], "runtime_config_reported");
        assert_eq!(success["outcome"]["category"], "success");

        let failure = line(
            None,
            &Outcome::new(ActingctlCode::ActingctlUsageInvalid),
            Detail::Full,
        )
        .expect("failure line");
        let object = failure.as_object().expect("an object");
        assert_eq!(
            object.keys().map(String::as_str).collect::<Vec<_>>(),
            ["outcome"]
        );
        assert_eq!(failure["outcome"]["code"], "actingctl_usage_invalid");
        assert_eq!(failure["outcome"]["category"], "error");
    }

    // Model v2.2 §7.6: the exit code is the top's category (0, 1, 2); a refused or failed
    // receipt exits at least 1 (A4 part B L2).
    #[test]
    fn the_exit_code_follows_the_category_and_a_refusal_is_never_zero() {
        for (category, refused, expected) in [
            (Category::Success, false, 0),
            (Category::Info, false, 0),
            (Category::Warning, false, 0),
            (Category::Error, false, 1),
            (Category::Fatal, false, 2),
            (Category::Info, true, 1),
            (Category::Warning, true, 1),
            (Category::Error, true, 1),
            (Category::Fatal, true, 2),
        ] {
            assert_eq!(
                exit_code(category, refused),
                expected,
                "{category} refused={refused}"
            );
        }
    }
}
