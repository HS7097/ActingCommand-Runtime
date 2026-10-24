// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl install` (Workflow #421; `contracts/install-cli.md`): the command-line installer of
//! a single-location root. INST-1 carries `release verify` and `release stage` (a verified build
//! kept under `install\builds\<commit>`), `adopt` (a root that scripts installed taken over with
//! its live build kept) and `resume` of their journalled operations. Like `watchdog` and `start`
//! the family takes `--root`, not `--state-root`, and exists in the CLI only: configuration file
//! edits and daemon restarts belong to the person, so no MCP tool drives it.
//!
//! Every exit prints one result line through `crate::cli_line`: `{…result, "outcome": …}`, the
//! process exiting by the top outcome's category. Nothing here deletes anything: replaced and
//! abandoned files are renamed aside and kept.

mod adopt;
pub(crate) mod files;
mod host;
mod journal;
mod manifest;
mod release;
mod state;

// Test-only: Workflow #421 INST-1, test rows IT-1 to IT-4 (frozen model INST-M §12), and the
// neutral release fixture they share.
#[cfg(test)]
mod fixture;
#[cfg(test)]
mod gate_inst1_release;

pub(crate) use host::{Host, SystemHost};

use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_contract::outcome::{Category, Outcome};
use journal::Operation;
use serde::Serialize;
use serde_json::{Map, Value};
use state::{CurrentState, OperationKind, Paths};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: actingctl install release verify --dir <dir> [--facts <release-facts.json>] [--expect-tag <tag>] [--expect-commit <hex8+>]\nusage: actingctl install release stage --root <root> --dir <dir> [--facts <release-facts.json>] [--expect-tag <tag>] [--expect-commit <hex8+>] [--local-build]\nusage: actingctl install adopt --root <root>\nusage: actingctl install resume --root <root> --operation <id>";

/// Runs `install` when the first argument names it; `None` leaves every other command to the
/// CLI, unchanged.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != "install" {
        return None;
    }
    if let Err(invalid) = cli_line::detail() {
        return Some(cli_line::failure(&invalid, None));
    }
    Some(print(execute(&arguments[1..], &SystemHost)))
}

/// One command's answer: the result members and the top outcome.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) members: Map<String, Value>,
    pub(crate) outcome: Outcome,
    /// Developer text for the stderr line (the usage).
    pub(crate) text: Option<&'static str>,
}

impl Answer {
    pub(crate) fn new(outcome: Outcome) -> Self {
        Self {
            members: Map::new(),
            outcome,
            text: None,
        }
    }

    /// The answer with one more result member.
    pub(crate) fn with(mut self, member: &str, value: impl Serialize) -> Result<Self, Outcome> {
        debug_assert!(
            member != "outcome",
            "a result member may not be named outcome"
        );
        let value = serde_json::to_value(value).map_err(|error| files::json_failure(&error))?;
        self.members.insert(member.to_owned(), value);
        Ok(self)
    }
}

fn print(answer: Answer) -> ExitCode {
    if answer.members.is_empty() && answer.outcome.category() > Category::Info {
        cli_line::failure(&answer.outcome, answer.text)
    } else {
        cli_line::success(answer.members, &answer.outcome)
    }
}

/// Runs one `install` command (the words after `install`) against `host`.
pub(crate) fn execute(arguments: &[OsString], host: &dyn Host) -> Answer {
    let Some(words) = arguments
        .iter()
        .map(|word| word.to_str())
        .collect::<Option<Vec<_>>>()
    else {
        return usage(cli_line::usage_invalid(None, ArgumentProblem::NotUtf8));
    };
    let result = match words.as_slice() {
        ["release", "verify", rest @ ..] => release::verify(rest, host),
        ["release", "stage", rest @ ..] => release::stage(rest, host),
        ["adopt", rest @ ..] => adopt::adopt(rest, host),
        ["resume", rest @ ..] => resume(rest, host),
        [word, ..] => Err(cli_line::usage_invalid(
            Some(*word),
            ArgumentProblem::Unknown,
        )),
        [] => Err(Outcome::new(ActingctlCode::ActingctlUsageInvalid)),
    };
    match result {
        Ok(answer) => answer,
        Err(outcome) if outcome.is(ActingctlCode::ActingctlUsageInvalid) => usage(outcome),
        Err(outcome) => Answer::new(outcome),
    }
}

fn usage(outcome: Outcome) -> Answer {
    Answer {
        members: Map::new(),
        outcome,
        text: Some(USAGE),
    }
}

/// The flags of one command: each at most once.
pub(crate) struct Flags<'a> {
    values: BTreeMap<&'static str, &'a str>,
    switches: BTreeSet<&'static str>,
}

impl<'a> Flags<'a> {
    /// Parses `words`: each of `with_value` takes the next word, each of `switches` none. An
    /// unknown flag, a repeated one and a flag without its value are refused by name.
    pub(crate) fn parse(
        words: &[&'a str],
        with_value: &[&'static str],
        switches: &[&'static str],
    ) -> Result<Self, Outcome> {
        let mut flags = Self {
            values: BTreeMap::new(),
            switches: BTreeSet::new(),
        };
        let mut index = 0;
        while index < words.len() {
            let word = words[index];
            if let Some(flag) = with_value.iter().copied().find(|flag| *flag == word) {
                let value = words.get(index + 1).copied().ok_or_else(|| {
                    cli_line::usage_invalid(Some(word), ArgumentProblem::ValueMissing)
                })?;
                if flags.values.insert(flag, value).is_some() {
                    return Err(cli_line::usage_invalid(
                        Some(word),
                        ArgumentProblem::Repeated,
                    ));
                }
                index += 2;
            } else if let Some(flag) = switches.iter().copied().find(|flag| *flag == word) {
                if !flags.switches.insert(flag) {
                    return Err(cli_line::usage_invalid(
                        Some(word),
                        ArgumentProblem::Repeated,
                    ));
                }
                index += 1;
            } else {
                return Err(cli_line::usage_invalid(
                    Some(word),
                    ArgumentProblem::Unknown,
                ));
            }
        }
        Ok(flags)
    }

    pub(crate) fn value(&self, flag: &'static str) -> Option<&'a str> {
        self.values.get(flag).copied()
    }

    /// A flag's value; `actingctl_usage_invalid` (`required`) when it was not given.
    pub(crate) fn required(&self, flag: &'static str) -> Result<&'a str, Outcome> {
        self.value(flag)
            .ok_or_else(|| cli_line::usage_invalid(Some(flag), ArgumentProblem::Required))
    }

    pub(crate) fn switch(&self, flag: &'static str) -> bool {
        self.switches.contains(flag)
    }
}

/// `actingctl_usage_invalid` for a flag whose value names something that cannot be used.
pub(crate) fn unusable(flag: &str) -> Outcome {
    cli_line::usage_invalid(Some(flag), ArgumentProblem::Unusable)
}

/// The root `--root` names; one that cannot be resolved is unusable, with the OS's failure.
pub(crate) fn resolve_root(flags: &Flags<'_>) -> Result<Paths, Outcome> {
    let root = flags.required("--root")?;
    Paths::resolve(Path::new(root)).map_err(|cause| unusable("--root").caused_by(cause))
}

/// `install_layout_ab_unsupported` for a root that holds acsetup's A/B selection.
pub(crate) fn refuse_ab(paths: &Paths) -> Result<(), Outcome> {
    let selection = paths.selection();
    if selection.exists() {
        return Err(Outcome::new(ActingctlCode::InstallLayoutAbUnsupported).path(selection));
    }
    Ok(())
}

/// `install resume --root <root> --operation <id>`: continues the unfinished operation from its
/// journal; each step's postcondition is checked on disk first.
fn resume(words: &[&str], host: &dyn Host) -> Result<Answer, Outcome> {
    let flags = Flags::parse(words, &["--root", "--operation"], &[])?;
    let paths = resolve_root(&flags)?;
    let operation_id = flags.required("--operation")?;
    match state::read_current(&paths) {
        CurrentState::Unfinished(current) if current.operation_id == operation_id => {}
        CurrentState::Unfinished(_) | CurrentState::Unreadable(_) => {
            journal::refuse_unfinished(&paths)?;
            return Err(unusable("--operation"));
        }
        CurrentState::None => return Err(unusable("--operation")),
    }
    let operation = Operation::load(host, &paths, operation_id)?;
    if operation.journal().finished() || operation.journal().undo.is_some() {
        return Err(unusable("--operation"));
    }
    let answer = match operation.journal().kind {
        OperationKind::ReleaseStage => release::resume(operation)?,
        OperationKind::Adopt => adopt::resume(operation)?,
    };
    Ok(resumed(operation_id, answer))
}

/// `install_operation_resumed` over the resumed operation's own answer: its members, and its
/// outcome as the `related` link. An operation that ended refused answers its refusal.
fn resumed(operation_id: &str, answer: Answer) -> Answer {
    if answer.outcome.category() > Category::Info {
        return answer;
    }
    let outcome = Outcome::new(ActingctlCode::InstallOperationResumed)
        .install_operation_id(operation_id)
        .link(
            actingcommand_contract::outcome::CauseRelation::Related,
            answer.outcome,
        );
    Answer {
        members: answer.members,
        outcome,
        text: None,
    }
}
