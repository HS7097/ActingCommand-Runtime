// SPDX-License-Identifier: AGPL-3.0-only

//! The operations actinglab's own commands serve (Workflow #434 LAB-2; register §1.287, LAB-2
//! row): the debugging and authoring rows, the core rows `app launch|restart|stop` (actinglab's
//! `session app`) and the two `user-config` rows. [`COMMANDS`] names each row's command
//! (Workflow #434 LAB-5A: before it, a row named its command in `params_more`). The operation's
//! short words are given to that command: its words, the row's positional words and, on the
//! command line of a lenient row, the further words, then the instance as `--instance`, the row's
//! options and switches and the further flags the command line handed on.
//!
//! The command answers as it does under its own words: on the command line `main` prints its line
//! and exits with its exit code, and `serve --stdio` answers the line its `--json` prints
//! ([`line`]; Workflow #434 LAB-2b, the CLI module's line, `super::own_line`).

use super::serve::Front;
use super::{JSON, own_line, words};
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::codes::ArgumentProblem;
use actingcommand_lab_ops::table::{Operation, ParamForm, ParamType};
use serde_json::{Map, Value};

/// How the command line of a row reaches its command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Line {
    /// The row's words as the row declares them, typed, as every row of the CLI module's.
    Declared,
    /// Workflow #434 LAB-5A: the row's flags handed on as written, with actinglab's global options
    /// (`words::GLOBAL_OPTIONS`): the command reads and checks them, as it did when the row took
    /// its command's further flags (`params_more`, before LAB-5A).
    AsWritten,
}

/// The command of every row actinglab's own command serves: the row's name, the command's words
/// and how the row's command line reaches it.
pub(crate) const COMMANDS: [(&str, &[&str], Line); 60] = [
    ("app launch", &["session", "app", "launch"], Line::Declared),
    (
        "app restart",
        &["session", "app", "restart"],
        Line::Declared,
    ),
    ("app stop", &["session", "app", "stop"], Line::Declared),
    ("tap", &["tap"], Line::AsWritten),
    ("swipe", &["swipe"], Line::AsWritten),
    ("long-tap", &["long-tap"], Line::AsWritten),
    ("key", &["key"], Line::AsWritten),
    ("text", &["text"], Line::AsWritten),
    ("touch-probe", &["touch-probe"], Line::AsWritten),
    ("recover", &["session", "recover"], Line::AsWritten),
    ("do", &["do"], Line::AsWritten),
    ("ensure", &["ensure"], Line::AsWritten),
    ("wait", &["wait"], Line::AsWritten),
    ("tap-target", &["tap-target"], Line::AsWritten),
    ("navigate", &["navigate"], Line::AsWritten),
    ("watch", &["lab", "watch"], Line::AsWritten),
    (
        "signatures register",
        &["lab", "signatures", "register"],
        Line::AsWritten,
    ),
    (
        "signatures match",
        &["lab", "signatures", "match"],
        Line::AsWritten,
    ),
    (
        "signatures retire",
        &["lab", "signatures", "retire"],
        Line::AsWritten,
    ),
    ("unpin", &["lab", "unpin"], Line::AsWritten),
    ("debug-package", &["lab", "debug-package"], Line::AsWritten),
    (
        "export-evidence",
        &["lab", "export-evidence"],
        Line::AsWritten,
    ),
    (
        "recognize-artifact",
        &["recognize-artifact"],
        Line::AsWritten,
    ),
    // Open point 7 (coordinator's ruling): actinglab's own `config get|set`, renamed.
    ("user-config", &["user-config", "get"], Line::Declared),
    ("user-config set", &["user-config", "set"], Line::Declared),
    ("observe", &["observe"], Line::AsWritten),
    ("record start", &["record", "start"], Line::AsWritten),
    ("record mark", &["record", "mark"], Line::AsWritten),
    ("record status", &["record", "status"], Line::AsWritten),
    ("record stop", &["record", "stop"], Line::AsWritten),
    (
        "record candidates",
        &["record", "candidates"],
        Line::AsWritten,
    ),
    ("record amend", &["record", "amend"], Line::AsWritten),
    ("record step", &["record", "step"], Line::AsWritten),
    ("package check", &["package", "check"], Line::AsWritten),
    (
        "package preflight",
        &["package", "preflight"],
        Line::AsWritten,
    ),
    (
        "package validate",
        &["package", "validate"],
        Line::AsWritten,
    ),
    ("package inspect", &["package", "inspect"], Line::AsWritten),
    ("package bundle", &["package", "bundle"], Line::AsWritten),
    ("package loading", &["package", "loading"], Line::AsWritten),
    ("package fmt", &["package", "fmt"], Line::AsWritten),
    ("package edit", &["package", "edit"], Line::AsWritten),
    (
        "resource catalog",
        &["resource", "catalog"],
        Line::AsWritten,
    ),
    (
        "resource validate",
        &["resource", "validate"],
        Line::AsWritten,
    ),
    ("resource verify", &["resource", "verify"], Line::AsWritten),
    (
        "resource restore",
        &["resource", "restore"],
        Line::AsWritten,
    ),
    (
        "resource compile-maa",
        &["resource", "compile-maa"],
        Line::AsWritten,
    ),
    (
        "standard-pack check",
        &["standard-pack", "check"],
        Line::AsWritten,
    ),
    (
        "standard-pack compose",
        &["standard-pack", "compose"],
        Line::AsWritten,
    ),
    (
        "scheduling compile",
        &["scheduling", "compile"],
        Line::AsWritten,
    ),
    (
        "scheduling timeline",
        &["scheduling", "timeline"],
        Line::AsWritten,
    ),
    (
        "operation validate",
        &["operation", "validate"],
        Line::AsWritten,
    ),
    (
        "operation inspect",
        &["operation", "inspect"],
        Line::AsWritten,
    ),
    (
        "operation explain",
        &["operation", "explain"],
        Line::AsWritten,
    ),
    ("regression", &["recognition-regression"], Line::AsWritten),
    ("replay", &["replay"], Line::AsWritten),
    ("recognize", &["recognize"], Line::AsWritten),
    ("detect-page", &["detect-page"], Line::AsWritten),
    ("current-page", &["current-page"], Line::AsWritten),
    ("is-visible", &["is-visible"], Line::AsWritten),
    ("locate", &["locate"], Line::AsWritten),
];

/// The command's words of `operation` in [`COMMANDS`], and how its command line reaches it;
/// `None` for a row no command of actinglab serves.
fn command(operation: &Operation) -> Option<(&'static [&'static str], Line)> {
    COMMANDS
        .iter()
        .find(|(name, _, _)| *name == operation.name)
        .map(|(_, words, line)| (*words, *line))
}

/// Workflow #434 LAB-5A: whether the command line of `operation` hands its flags on as written
/// ([`Line::AsWritten`]).
pub(crate) fn as_written(operation: &Operation) -> bool {
    command(operation).is_some_and(|(_, line)| line == Line::AsWritten)
}

/// The answer of `serve --stdio` to `operation` with `params`: the line `actinglab --json` prints
/// for the command line of its command (Workflow #434 LAB-2b), with its exit code. Workflow #434
/// LAB-5G4a: under `--front mcp` (`front`) the command runs with the `via` MCP's `--via mcp` child
/// gives it, so that what it adds to a recording carries `Lab-MCP`; its line stays this one.
pub(super) fn line(
    operation: &Operation,
    params: &Map<String, Value>,
    front: Option<Front>,
) -> Exit {
    let Some(words) = arguments(operation, params) else {
        return cli_line::failure(
            &cli_line::usage_invalid(Some(operation.name.as_str()), ArgumentProblem::Unknown),
            Some("no command of actinglab serves this operation"),
        );
    };
    let line = std::iter::once(JSON.to_owned()).chain(words);
    let mut invocation = crate::cli_parse::parse_invocation(line, true);
    if let Some(front) = front
        && let Ok(invocation) = &mut invocation
    {
        invocation.global.via = Some(front.via());
    }
    own_line::exit(&crate::run_invocation(invocation))
}

/// The command line of `operation`: its words, the row's positional words, the further words,
/// then the instance, the row's options and switches and the further flags. Positional words go
/// first so that a switch is never followed by one (actinglab reads `--flag word` as a value).
/// Workflow #434 LAB-5A: a flag's value decides how it is written, whether the row declares it
/// or the command line handed it on: `true` the flag alone, `false` nothing, a list each item,
/// any other value the flag and its value (a declared option is never a boolean value: serve
/// checks its type, and the command line hands it on as written).
pub(crate) fn arguments(operation: &Operation, params: &Map<String, Value>) -> Option<Vec<String>> {
    let (command_words, _) = command(operation)?;
    let mut line = command_words
        .iter()
        .map(|word| (*word).to_owned())
        .collect::<Vec<_>>();
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
            ParamForm::Switch | ParamForm::Option => written(&mut flags, &param.name, value),
        }
    }
    if let Some(further) = params.get(words::MORE_WORDS) {
        line.extend(each(further).map(scalar));
    }
    for (name, value) in words::further_flags(operation, params) {
        written(&mut flags, name, value);
    }
    line.extend(flags);
    Some(line)
}

/// The flag `name` with `value`, as [`arguments`] writes it.
fn written(flags: &mut Vec<String>, name: &str, value: &Value) {
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
