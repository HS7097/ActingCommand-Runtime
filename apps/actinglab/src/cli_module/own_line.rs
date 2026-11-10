// SPDX-License-Identifier: AGPL-3.0-only

//! The line actinglab's own commands print (Workflow #434 LAB-2b; `contracts/lab-two-entries.md`
//! §9.3 and open point 10; register §1.287, "Slice LAB-2b"): the CLI module's line, built by
//! `cli_line` and printed by [`super::print`], from the result each command answers as before.
//!
//! - **Success.** The code `lab_operation_completed`, the result's members flat at the top level
//!   beside `outcome`; a result that is no JSON object goes into the one member `result`, as the
//!   module's own operations do. Warnings a result carries stay where they are, a member
//!   `warnings` of the result in their own shape `{code, values}`: the category stays success and
//!   no developer line is written.
//! - **A result whose top level has a member named `outcome`** is never flattened (the
//!   coordinator's ruling of 2026-11-10 on #434 LAB-2b): it goes whole, unchanged, into the one
//!   member its command declares in [`WRAPPED`], the one place a command declares it. A command
//!   that declares none is refused with `actingctl_output_failed` (exit 1) and nothing on stdout,
//!   so no line ever carries a second `outcome` and a command that would collide fails loud.
//! - **Failure.** `{"outcome": {code, category: "error", values}, "message", "details"}`: the code
//!   is actinglab's own; `values` are those of the outcome of the same code the details carry (the
//!   details are one, or their member `outcome` is one: `resource verify`'s report, a refusal whose
//!   details are `{outcome}`), else the members of the details whose names the outcome catalog
//!   registers as that code's values (none for a code the catalog does not list); `message` is the
//!   error's message; `details` its details, nested whole and never flattened (some carry an
//!   `outcome` of their own), absent when the error has none. Exit 1, and the developer line
//!   `ERROR actinglab: <code> … <message>` on stderr.
//! - **Exit codes** 0, 1 and 2, by the outcome's category: 0 a success; 1 every failure, and the
//!   installation's refusal before the command runs (the module's `cli_line::refused` of its code;
//!   every such code is an error, so the developer line is `ERROR actinglab: <code>` and stdout
//!   carries `{outcome}`); 2 an outcome of category fatal, which no own command line has today.
//! - **Without `--json`** the lines for people (`super::people`), into a pipe or a file too.
//! - **A command line with `--via`** (only MCP's actinglab child sends one) is not answered here:
//!   `main` prints the envelope for it as before, byte for byte, with its exit codes 2 to 6, until
//!   LAB-5 ([`via_line`]).

use crate::cli_result::CliResult;
use crate::codes::ActinglabCode;
use actingcommand_contract::EnvelopeError;
use actingcommand_contract::outcome::{
    Category, OUTCOME_SCHEMA_VERSION, Outcome, OutcomeEnvelope, catalog_json,
};
use actingcommand_lab_ops::cli_line::{self, Exit};
use serde_json::{Map, Value, json};

/// The global option of the counter that forwarded the work (`--via mcp`, MCP's actinglab child).
const VIA: &str = "--via";
/// The line's own outcome; no result member takes the name.
const OUTCOME: &str = "outcome";
/// The member a result that is no JSON object goes under (the module's `core_ops` uses the same).
const RESULT_MEMBER: &str = "result";
/// The failure line's members beside its outcome.
const MESSAGE: &str = "message";
const DETAILS: &str = "details";
/// The members of an outcome nested in a failure's details that name its code and its values.
const CODE_MEMBER: &str = "code";
const VALUES_MEMBER: &str = "values";

/// The commands whose result has a top-level member named `outcome`, each with the one member
/// its line carries that result in, whole and unchanged (the coordinator's ruling of 2026-11-10 on
/// #434 LAB-2b): `resource verify`'s `actinglab.resource-verify.v1` report, whose `outcome` is the
/// report's own, and `run summary`'s `actingcommand.run-summary.v1` document, whose `outcome` is
/// the run's. The one place a command declares it.
pub(crate) const WRAPPED: [(&str, &str); 2] =
    [("resource verify", "report"), ("run summary", "summary")];

/// Whether the command line `arguments` carries `--via`: MCP's actinglab child, whose lines keep
/// the envelope until LAB-5.
pub(crate) fn via_line(arguments: &[String]) -> bool {
    arguments.iter().any(|argument| argument == VIA)
}

/// Whether `main` prints the envelope for `arguments` without `--json`: only a `--via` line, and
/// only when stdout is no terminal (`stdout_piped`), as before. Every other line prints for people
/// without `--json`, into a pipe too.
pub(crate) fn envelope_by_default(arguments: &[String], stdout_piped: bool) -> bool {
    stdout_piped && via_line(arguments)
}

/// The member command `command`'s line carries its whole result in, when it declares one.
pub(crate) fn wrapping_member(command: &str) -> Option<&'static str> {
    WRAPPED
        .iter()
        .find(|(name, _)| *name == command)
        .map(|(_, member)| *member)
}

/// What `main` prints for actinglab's own command line `invocation` (no `--via`): its exit and
/// whether `--json` asked for the JSON line. The installation this process runs from is checked
/// first, as the module checks it: a refusal runs nothing.
pub(crate) fn answer(invocation: crate::OwnInvocation) -> (Exit, bool) {
    let json = match &invocation {
        Ok(invocation) => invocation.global.json,
        Err((_, json, _)) => *json,
    };
    if invocation.is_ok()
        && let Err(error) = actingcommand_contract::process_installation()
    {
        return (cli_line::refused(&Outcome::new(error.code()), None), json);
    }
    let result = crate::run_invocation(invocation);
    (exit(&result), result.print_json)
}

/// The exit of one own command's result: its success or its failure line.
pub(crate) fn exit(result: &CliResult) -> Exit {
    let envelope = &result.envelope;
    match &envelope.error {
        Some(error) => failure(error),
        None => success(
            &envelope.command,
            envelope.data.clone().unwrap_or(Value::Null),
        ),
    }
}

/// The success line of `command` with its result `data`.
pub(crate) fn success(command: &str, data: Value) -> Exit {
    let members = match wrapping_member(command) {
        Some(member) => {
            let mut members = Map::new();
            members.insert(member.to_owned(), data);
            members
        }
        None => cli_line::members(data, RESULT_MEMBER),
    };
    if members.contains_key(OUTCOME) {
        return Exit {
            line: Err(format!(
                "the result of actinglab {command} has a top-level member named {OUTCOME}, and \
                 the command declares no member to carry it whole (cli_module::own_line::WRAPPED)"
            )),
            developer: None,
            code: 1,
        };
    }
    cli_line::success(members, &Outcome::new(ActinglabCode::LabOperationCompleted))
}

/// The failure line of `error`: its outcome, its message and its details whole; exit 1.
fn failure(error: &EnvelopeError) -> Exit {
    let outcome = match received(&error.code, error.details.as_ref()) {
        Ok(outcome) => outcome,
        Err(text) => {
            return Exit {
                line: Err(text),
                developer: None,
                code: 1,
            };
        }
    };
    let mut members = Map::new();
    members.insert(MESSAGE.to_owned(), Value::String(error.message.clone()));
    if let Some(details) = &error.details {
        members.insert(DETAILS.to_owned(), details.clone());
    }
    cli_line::finish(Some(members), &outcome, false, Some(error.message.as_str()))
}

/// actinglab's error code `spelling` as the line's outcome: category error, and as its values
/// those of the outcome of the same code `details` carries ([`nested_values`]), else the members
/// of `details` whose names the outcome catalog registers as the code's values
/// ([`registered_values`]). actinglab still carries its codes as spellings, and not every one is
/// registered, so the outcome is read as one received from that channel, which is relayed and
/// never checked as an emission.
fn received(spelling: &str, details: Option<&Value>) -> Result<Outcome, String> {
    let values = match nested_values(spelling, details) {
        Some(values) => values.clone(),
        None => registered_values(spelling, details)?,
    };
    serde_json::from_value::<OutcomeEnvelope>(json!({
        "schema_version": OUTCOME_SCHEMA_VERSION,
        "code": spelling,
        "category": Category::Error,
        "values": values,
    }))
    .map(OutcomeEnvelope::into_outcome)
    .map_err(|error| format!("the outcome of actinglab's code {spelling} does not build: {error}"))
}

/// The values of the outcome of the code `spelling` that `details` carries: `details` itself when
/// it is one, else its member `outcome` when that is one (Workflow #434 LAB-2b fix 1: a refusal
/// that nests its values, as `resource verify`'s report or the `{outcome}` of the Lab's OCR, touch
/// and standard-pack refusals, keeps them on the line). `None` when it carries none.
fn nested_values<'a>(spelling: &str, details: Option<&'a Value>) -> Option<&'a Map<String, Value>> {
    let details = details?;
    [Some(details), details.get(OUTCOME)]
        .into_iter()
        .flatten()
        .filter(|outcome| outcome.get(CODE_MEMBER).and_then(Value::as_str) == Some(spelling))
        .find_map(|outcome| outcome.get(VALUES_MEMBER).and_then(Value::as_object))
}

/// The members of `details` whose names the outcome catalog registers as the values of the code
/// `spelling`: none for a code the catalog does not list, or without details.
fn registered_values(
    spelling: &str,
    details: Option<&Value>,
) -> Result<Map<String, Value>, String> {
    let catalog = catalog_json()
        .map_err(|error| format!("the embedded outcome catalog does not parse: {error}"))?;
    let registered = catalog
        .get("codes")
        .and_then(|codes| codes.get(spelling))
        .and_then(|entry| entry.get(VALUES_MEMBER))
        .and_then(Value::as_object);
    Ok(match (registered, details.and_then(Value::as_object)) {
        (Some(keys), Some(details)) => details
            .iter()
            .filter(|(name, _)| keys.contains_key(name.as_str()))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
        _ => Map::new(),
    })
}
