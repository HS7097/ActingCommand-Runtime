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

use crate::codes::ActingctlCode;
use actingcommand_contract::outcome::{Category, Detail, Outcome};
use serde_json::{Map, Value};
use std::io::{self, Write};
use std::process::ExitCode;

/// The environment variable that switches actingctl's outcomes to codes only (§7.1).
pub(crate) const DETAIL_VARIABLE: &str = "ACTINGCOMMAND_DETAIL";

/// The detail `ACTINGCOMMAND_DETAIL` asks for: full when it is unset; `codes` or `full`
/// otherwise; any other value is refused with `actingctl_usage_invalid` and the variable.
pub(crate) fn detail() -> Result<Detail, Outcome> {
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
pub(crate) const fn level(category: Category) -> &'static str {
    match category {
        Category::Success | Category::Info => "INFO",
        Category::Warning => "WARN",
        Category::Error => "ERROR",
        Category::Fatal => "FATAL",
    }
}

/// The developer line `<LEVEL> actingctl: <outcome>[ <text>]` on stderr; `text` is developer
/// English only (a usage line, a foreign detail the outcome does not hold).
pub(crate) fn developer_line(outcome: &Outcome, text: Option<&str>) {
    match text {
        Some(text) => eprintln!("{} actingctl: {outcome} {text}", level(outcome.category())),
        None => eprintln!("{} actingctl: {outcome}", level(outcome.category())),
    }
}

/// A successful exit: `{…result, "outcome": …}`.
pub(crate) fn success(result: Map<String, Value>, outcome: &Outcome) -> ExitCode {
    finish(Some(result), outcome, false, None)
}

/// A failed exit: `{"outcome": …}`, then the developer line with `text`.
pub(crate) fn failure(outcome: &Outcome, text: Option<&str>) -> ExitCode {
    finish(None, outcome, false, text)
}

/// A refused exit: `{outcome}` and the developer line with `text`, non-zero whatever the top's
/// category (a refusal the Runtime topped with an info or warning code still failed; A4 part B
/// L2).
pub(crate) fn refused(outcome: &Outcome, text: Option<&str>) -> ExitCode {
    finish(None, outcome, true, text)
}

/// The line of a subcommand whose result holds a Runtime receipt: `refused` is true when that
/// receipt's state is denied or failed, and the process then exits non-zero whatever the
/// outcome's category (A4 part B L2).
pub(crate) fn receipt(result: Map<String, Value>, outcome: &Outcome, refused: bool) -> ExitCode {
    finish(Some(result), outcome, refused, None)
}

/// Prints the line and the developer line and returns the exit code (see [`exit_code`]). When
/// stdout refuses the line, the developer line `actingctl_output_failed` is all that is left
/// and the exit is 1 (§7.3).
fn finish(
    result: Option<Map<String, Value>>,
    outcome: &Outcome,
    refused: bool,
    text: Option<&str>,
) -> ExitCode {
    let detail = detail().unwrap_or(Detail::Full);
    let line = match line(result, outcome, detail) {
        Ok(line) => line,
        Err(error) => return output_failed(&error.to_string()),
    };
    if let Err(error) = write_line(&line) {
        return output_failed(&error.to_string());
    }
    if refused || outcome.category() > Category::Info {
        developer_line(outcome, text);
    }
    ExitCode::from(exit_code(outcome.category(), refused))
}

/// The result line: the result's members and `outcome`, the envelope in `detail`.
pub(crate) fn line(
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
pub(crate) fn exit_code(category: Category, refused: bool) -> u8 {
    category.exit_code().max(u8::from(refused))
}

/// An outcome as an interface value: its `actingcommand.outcome.v1` envelope in `detail`. An
/// envelope always serialises; were it ever to fail, the code alone is shown.
pub(crate) fn outcome_value(outcome: &Outcome, detail: Detail) -> Value {
    serde_json::to_value(outcome.envelope(detail))
        .unwrap_or_else(|_| Value::String(outcome.code().as_str().to_owned()))
}

/// A warning beside a result, as the CLI line and MCP both carry it: `{outcome}` (Workflow #378
/// A5; MCP-0 command table D3: a top-level `warnings` member, present only when not empty).
pub(crate) fn warning_value(outcome: &Outcome, detail: Detail) -> Value {
    let mut warning = Map::new();
    warning.insert("outcome".to_owned(), outcome_value(outcome, detail));
    Value::Object(warning)
}

fn write_line(line: &Value) -> io::Result<()> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    serde_json::to_writer(&mut writer, line)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn output_failed(text: &str) -> ExitCode {
    developer_line(
        &Outcome::new(ActingctlCode::ActingctlOutputFailed),
        Some(text),
    );
    ExitCode::FAILURE
}

/// A warning of this process's result line: [`warning_value`] in the process's detail.
pub(crate) fn warning(outcome: &Outcome) -> Value {
    warning_value(outcome, detail().unwrap_or(Detail::Full))
}

/// A JSON value as the members of a result line: an object's members, or any other value under
/// `member`.
pub(crate) fn members(value: Value, member: &str) -> Map<String, Value> {
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
