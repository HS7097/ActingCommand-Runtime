// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl install` (Workflow #421; `contracts/install-cli.md`): the command-line installer of
//! a single-location root. INST-1 carries `release verify` and `release stage` (a verified build
//! kept under `install\builds\<commit>`), `adopt` (a root that scripts installed taken over with
//! its live build kept), `update plan|apply` and `rollback plan|apply` (the program files
//! replaced in place by those of a kept build under the writer lock, a rollback behind its
//! gate), `undo`, `resume`, `status` and `leftovers`. Like `watchdog` and `start`
//! the family takes `--root`, not `--state-root`, and exists in the CLI only: configuration file
//! edits and daemon restarts belong to the person, so no MCP tool drives it.
//!
//! Every exit prints one result line through `crate::cli_line`: `{…result, "outcome": …}`, the
//! process exiting by the top outcome's category. The outcome is one of the family's folded codes
//! (ruling 54): `install_command_done`, `install_caveat_noted` or `install_command_refused`, its
//! `operation` the command; a step that fails after a change on disk is
//! `install_operation_interrupted`. Nothing here deletes anything: replaced and abandoned files
//! are renamed aside and kept.
//!
//! Workflow #434 LAB-3 (`contracts/lab-two-entries.md` §5 to §7): this module moved from
//! `apps/actingctl` unchanged; [`dispatch`] answers `actingctl install` with the `install-cli`
//! revision-1 face ([`Face::Install`]) until LAB-7, and builds the line that actingctl prints. The
//! Lab's rescue module runs the same commands with [`Face::Rescue`] (`crate::rescue`).

// Workflow #434 LAB-3: the modules are the crate's, so that the rescue module (`crate::rescue`)
// runs their commands and steps; nothing outside the crate reads them.
pub(crate) mod adopt;
// Workflow #434 LAB-3: the rescue module's `config replace` and `config restore`.
pub(crate) mod config_swap;
// Workflow #421 INST-2: the stop, the start, the hold's release and the composite update.
pub(crate) mod deploy;
pub(crate) mod files;
pub(crate) mod guards;
pub(crate) mod hold;
pub(crate) mod host;
// Workflow #421 INST-5 (first slice): the rescue module's `instance add`, its candidate.
pub(crate) mod instance_add;
pub(crate) mod journal;
pub(crate) mod launch;
pub(crate) mod lock;
pub(crate) mod manifest;
// Workflow #434 LAB-5G5b: the rescue module's `mcp config`, the second generation's snippet.
pub(crate) mod mcp_config;
// Workflow #421 MCP-ENTRY: `mcp-entry plan|list`; Workflow #434 LAB-5G5b: and the rescue module's
// `mcp plan` and `mcp entries`.
pub(crate) mod mcp_entry;
// Workflow #434 LAB-3: the rescue module's `place`.
pub(crate) mod place;
pub(crate) mod plan;
pub(crate) mod postcheck;
pub(crate) mod programs;
pub(crate) mod release;
pub(crate) mod runtime;
pub(crate) mod state;
pub(crate) mod status;
pub(crate) mod undo;
pub(crate) mod update;

// Test-only: Workflow #421 INST-1, test rows IT-1 to IT-4 (frozen model INST-M §12), and the
// neutral release fixture they share.
#[cfg(test)]
mod fixture;
#[cfg(test)]
mod gate_inst1_release;
// Test-only: Workflow #421 INST-1, test rows IT-5 to IT-12, IT-15 and IT-16.
#[cfg(test)]
mod gate_inst1_update;
// Test-only: Workflow #421 INST-2, test rows D-1 to D-9 (#421 deploy design §8 S2).
#[cfg(test)]
mod gate_inst2_deploy;
// Test-only: #411 P49, `mcp-entry list` judges an entry by its pid and its process creation time
// (MCP-ENTRY review L4).
#[cfg(test)]
mod gate_411_p49;
// Test-only: Workflow #429 A.4 a (R-15a), the edges of the UI that runs after an operation.
#[cfg(test)]
mod gate_r15a_ui_edges;
// Test-only: Workflow #429 R-0b, this build's `runtime-client` floor against consoles at 2, 3, 4.
#[cfg(test)]
mod gate_429_r0b_pairing;
// Test-only: Workflow #429 R-15b, the release reader's UI side (A.4 b) and the registry of the
// contract's codes and words.
#[cfg(test)]
mod gate_r15b_registry;
#[cfg(test)]
mod gate_r15b_release;
// Test-only: Workflow #434 LAB-3, the register's named tests of the rescue module and the
// unchanged `actingctl install` face.
#[cfg(test)]
mod gate_434_lab3;
// Test-only: Workflow #421 INST-5, the register's named tests of the rescue module's
// `instance add`.
#[cfg(test)]
mod gate_421_inst5;
// Test-only: Workflow #434 LAB-5G6, the register's named test of a running MCP server under a
// deploy.
#[cfg(test)]
mod gate_434_lab5g6;

// Test-only: Workflow #434 LAB-5G5b, the register's named tests of the rescue module's MCP rows
// (`mcp plan` against its checked-in sample, `mcp entries`, `mcp config`).
#[cfg(test)]
mod gate_434_lab5g5b;

// Test-only: Workflow #434 LAB-5B1b, the command lines the rescue module's handed rows build,
// pinned before the slice's refactor.
#[cfg(test)]
mod gate_434_lab5b1b_lines;

pub(crate) use host::{Host, SystemHost};
// Workflow #421 INST-1, watchdog row 7a″: what the watchdog and `actingctl start` read.
pub(crate) use state::unfinished_operation;

use crate::cli_line::{self, Exit};
use crate::codes::{
    ActingctlCode, ArgumentProblem, InstallCaveat, InstallLocation, InstallRefusal,
};
use actingcommand_contract::outcome::{Category, CauseRelation, Outcome};
use actingcommand_lab_ops::initiator::Initiator;
use journal::Operation;
use plan::PlanKind;
use serde::Serialize;
use serde_json::{Map, Value};
use state::{CurrentState, OperationKind, Paths};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::Path;

/// Workflow #434 LAB-3: which face a command answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// `actingctl install`: the `install-cli` revision-1 face, unchanged until LAB-7 deletes it
    /// (contracts/lab-two-entries.md §5, §11).
    Install,
    /// The Lab's rescue module (§4.3, §5, §6): every journal records the initiator; a one-way
    /// ledger edge needs no acknowledgment and is journalled `one_way`; every pause found on an
    /// instance is a known pause, kept and listed; a guard that fails or is not evaluated is
    /// noted (`install_caveat_noted`, `guard_failed`) and never refuses; a start waits for memory
    /// as long as `install.start.memory_wait_ms` and then goes on.
    Rescue(Initiator),
}

impl Face {
    /// The initiator a journal records: the rescue module's word, none for `actingctl install`.
    pub(crate) const fn initiator(self) -> Option<Initiator> {
        match self {
            Self::Install => None,
            Self::Rescue(initiator) => Some(initiator),
        }
    }

    /// Whether the guards report and never refuse, and the confirmations are gone.
    pub(crate) const fn rescue(self) -> bool {
        matches!(self, Self::Rescue(_))
    }

    /// The face of an operation, by its journal: one the rescue module began records its
    /// initiator, so a resumed or undone operation keeps the rules it began with.
    pub(crate) fn of(journal: &journal::Journal) -> Self {
        journal.initiator.map_or(Self::Install, Self::Rescue)
    }
}

const USAGE: &str = "usage: actingctl install release verify --dir <dir> [--facts <release-facts.json>] [--expect-tag <tag>] [--expect-commit <hex8+>]\nusage: actingctl install release stage --root <root> --dir <dir> [--facts <release-facts.json>] [--expect-tag <tag>] [--expect-commit <hex8+>] [--local-build]\nusage: actingctl install adopt --root <root>\nusage: actingctl install update plan --root <root> --build <commit> [--preview] [--accept-ledger-one-way]\nusage: actingctl install update apply --root <root> --build <commit> --plan <id> [--accept-ledger-one-way] [--stop --hold <reason> [--known-pause ALIAS=REASON]... [--start] [--ack-guard <id>]... [--detach]]\nusage: actingctl install rollback plan|apply --root <root> --to <commit> [--plan <id>] [--accept-ledger-one-way] [--stop --hold <reason> [--known-pause ALIAS=REASON]... [--start] [--ack-guard <id>]... [--detach]]\nusage: actingctl install undo|resume --root <root> --operation <id>\nusage: actingctl install status --root <root> [--operation <id>] [--check-rollback]\nusage: actingctl install leftovers --root <root>\nusage: actingctl install stop --root <root> --hold <reason> [--known-pause ALIAS=REASON]... [--ack-guard <id>]...\nusage: actingctl install start --root <root> [--ack-guard <id>]...\nusage: actingctl install release-hold --root <root> --reason <reason>\nusage: actingctl install guards --root <root> --for stop|start [--hold <reason>] [--known-pause ALIAS=REASON]...";

/// Runs `install` when the first argument names it, with the revision-1 face; `None` leaves every
/// other command to the CLI, unchanged. The caller (actingctl) prints the exit.
pub fn dispatch(arguments: &[OsString]) -> Option<Exit> {
    if arguments.first()?.to_str()? != "install" {
        return None;
    }
    if let Err(invalid) = cli_line::detail() {
        return Some(cli_line::failure(&invalid, None));
    }
    Some(print(execute(&arguments[1..], &SystemHost)))
}

const MCP_ENTRY_USAGE: &str = "usage: actingctl mcp-entry plan --root <root> --entry-protocol 1 [--current-build <commit>] [-- <child arguments>]\nusage: actingctl mcp-entry list --root <root> [--all]";

/// Runs `mcp-entry` (Workflow #421 MCP-ENTRY) when the first argument names it; `None` leaves
/// every other command to the CLI, unchanged. The caller (actingctl) prints the exit.
pub fn dispatch_mcp_entry(arguments: &[OsString]) -> Option<Exit> {
    if arguments.first()?.to_str()? != "mcp-entry" {
        return None;
    }
    if let Err(invalid) = cli_line::detail() {
        return Some(cli_line::failure(&invalid, None));
    }
    let usage = |outcome: Outcome| Answer {
        members: Map::new(),
        outcome,
        text: Some(MCP_ENTRY_USAGE),
    };
    let Some(words) = arguments[1..]
        .iter()
        .map(|word| word.to_str())
        .collect::<Option<Vec<_>>>()
    else {
        return Some(print(usage(cli_line::usage_invalid(
            None,
            ArgumentProblem::NotUtf8,
        ))));
    };
    let result = match words.as_slice() {
        ["plan", rest @ ..] => mcp_entry::plan(rest),
        ["list", rest @ ..] => mcp_entry::list(rest),
        [word, ..] => Err(cli_line::usage_invalid(
            Some(*word),
            ArgumentProblem::Unknown,
        )),
        [] => Err(Outcome::new(ActingctlCode::ActingctlUsageInvalid)),
    };
    Some(print(match result {
        Ok(answer) => answer,
        Err(outcome) if outcome.is(ActingctlCode::ActingctlUsageInvalid) => usage(outcome),
        Err(outcome) => Answer::new(outcome),
    }))
}

/// One command's answer: the result members and the top outcome.
#[derive(Debug, Clone)]
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

/// The line of `answer`, which the caller prints: `{…result, "outcome"}`, or `{outcome}` with
/// the developer text for a failure without members.
pub(crate) fn print(answer: Answer) -> Exit {
    if answer.members.is_empty() && answer.outcome.category() > Category::Info {
        cli_line::failure(&answer.outcome, answer.text)
    } else {
        cli_line::success(answer.members, &answer.outcome)
    }
}

/// Runs one `install` command (the words after `install`) against `host`, with the revision-1
/// face of `actingctl install`.
pub(crate) fn execute(arguments: &[OsString], host: &dyn Host) -> Answer {
    let Some(words) = arguments
        .iter()
        .map(|word| word.to_str())
        .collect::<Option<Vec<_>>>()
    else {
        return usage(cli_line::usage_invalid(None, ArgumentProblem::NotUtf8));
    };
    execute_as(&words, host, Face::Install)
}

/// Runs one `install` command (the words after `install`) against `host` with `face` (Workflow
/// #434 LAB-3: the rescue module runs the same commands with [`Face::Rescue`]).
pub(crate) fn execute_as(words: &[&str], host: &dyn Host, face: Face) -> Answer {
    let result = match words {
        ["release", "verify", rest @ ..] => release::verify(rest, host),
        ["release", "stage", rest @ ..] => release::stage(rest, host, face),
        ["adopt", rest @ ..] => adopt::adopt(rest, host, face),
        ["update", "plan", rest @ ..] => update::plan_command(rest, host, PlanKind::Update, face),
        ["update", "apply", rest @ ..] => update::apply_command(rest, host, PlanKind::Update, face),
        ["rollback", "plan", rest @ ..] => {
            update::plan_command(rest, host, PlanKind::Rollback, face)
        }
        ["rollback", "apply", rest @ ..] => {
            update::apply_command(rest, host, PlanKind::Rollback, face)
        }
        ["undo", rest @ ..] => undo::undo(rest, host, face),
        ["resume", rest @ ..] => resume(rest, host),
        ["status", rest @ ..] => status::status(rest, host),
        ["leftovers", rest @ ..] => status::leftovers(rest, host),
        // Workflow #421 INST-2: the formal stop, the installer's start, the release of a hold and
        // the guards report.
        ["stop", rest @ ..] => deploy::stop_command(rest, host, face),
        ["start", rest @ ..] => deploy::start_command(rest, host, face),
        ["release-hold", rest @ ..] => deploy::release_hold_command(rest, host, face),
        ["guards", rest @ ..] => deploy::guards_command(rest, host, face),
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

/// The flags of one command: each at most once, but a list flag (Workflow #421 INST-2:
/// `--known-pause`, `--ack-guard`) as often as it is given.
pub(crate) struct Flags<'a> {
    values: BTreeMap<&'static str, &'a str>,
    lists: BTreeMap<&'static str, Vec<&'a str>>,
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
        Self::parse_lists(words, with_value, &[], switches)
    }

    /// [`Self::parse`] with list flags: each of `lists` takes the next word, and may be given
    /// again.
    pub(crate) fn parse_lists(
        words: &[&'a str],
        with_value: &[&'static str],
        lists: &[&'static str],
        switches: &[&'static str],
    ) -> Result<Self, Outcome> {
        let mut flags = Self {
            values: BTreeMap::new(),
            lists: BTreeMap::new(),
            switches: BTreeSet::new(),
        };
        let mut index = 0;
        while index < words.len() {
            let word = words[index];
            if let Some(flag) = lists.iter().copied().find(|flag| *flag == word) {
                let value = words.get(index + 1).copied().ok_or_else(|| {
                    cli_line::usage_invalid(Some(word), ArgumentProblem::ValueMissing)
                })?;
                flags.lists.entry(flag).or_default().push(value);
                index += 2;
            } else if let Some(flag) = with_value.iter().copied().find(|flag| *flag == word) {
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

    /// Every value of the list flag `flag`, in the order given.
    pub(crate) fn list(&self, flag: &'static str) -> Vec<&'a str> {
        self.lists.get(flag).cloned().unwrap_or_default()
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

/// `install_command_done` of the command `operation`.
pub(crate) fn done(operation: InstallLocation) -> Outcome {
    Outcome::new(ActingctlCode::InstallCommandDone).operation(operation)
}

/// `install_command_refused` of the command `operation`, `refusal` saying why.
pub(crate) fn refusal(operation: InstallLocation, refusal: InstallRefusal) -> Outcome {
    Outcome::new(ActingctlCode::InstallCommandRefused)
        .operation(operation)
        .install_refusal(refusal)
}

/// `install_caveat_noted` of the command `operation`, `caveat` saying what was left unchecked,
/// pending or on the previous build.
pub(crate) fn caveat(operation: InstallLocation, caveat: InstallCaveat) -> Outcome {
    Outcome::new(ActingctlCode::InstallCaveatNoted)
        .operation(operation)
        .install_caveat(caveat)
}

/// `install_command_refused` (`layout_ab_unsupported`) for a root that holds acsetup's A/B
/// selection.
pub(crate) fn refuse_ab(operation: InstallLocation, paths: &Paths) -> Result<(), Outcome> {
    let selection = paths.selection();
    if selection.exists() {
        return Err(refusal(operation, InstallRefusal::LayoutAbUnsupported).path(selection));
    }
    Ok(())
}

/// `install resume --root <root> --operation <id>`: continues the unfinished operation from its
/// journal; each step's postcondition is checked on disk first. An operation whose end (or its
/// undo's) is in its journal while `current.json` still names it (a crash before the rename,
/// review S1 F5) only has `current.json` retired and answers the end it recorded.
fn resume(words: &[&str], host: &dyn Host) -> Result<Answer, Outcome> {
    let flags = Flags::parse(words, &["--root", "--operation"], &[])?;
    let paths = resolve_root(&flags)?;
    let operation_id = flags.required("--operation")?;
    match state::read_current(&paths) {
        CurrentState::Unfinished(current) if current.operation_id == operation_id => {}
        CurrentState::Unfinished(_) | CurrentState::Unreadable(_) => {
            journal::refuse_unfinished(InstallLocation::Resume, &paths)?;
            return Err(unusable("--operation"));
        }
        CurrentState::None => return Err(unusable("--operation")),
    }
    let operation = Operation::load(host, &paths, operation_id, InstallLocation::Resume)?;
    if operation.ended() {
        let ended = operation.retire_ended()?;
        return Ok(resumed(operation_id, Answer::new(ended)));
    }
    if operation.journal().finished() || operation.journal().undo.is_some() {
        return Err(unusable("--operation"));
    }
    let answer = match operation.journal().kind {
        OperationKind::ReleaseStage => release::resume(operation)?,
        OperationKind::Adopt => adopt::resume(operation)?,
        OperationKind::Update
        | OperationKind::Rollback
        | OperationKind::Stop
        | OperationKind::Start
        | OperationKind::ReleaseHold
        | OperationKind::ConfigReplace
        | OperationKind::PackPlace => update::resume(operation)?,
    };
    Ok(resumed(operation_id, answer))
}

/// `install_command_done` of `install resume` over the resumed operation's own answer: its
/// members, and its outcome (the operation's own done) as the `related` link. An operation that
/// ended refused answers its refusal.
fn resumed(operation_id: &str, answer: Answer) -> Answer {
    if answer.outcome.category() > Category::Info {
        return answer;
    }
    let outcome = done(InstallLocation::Resume)
        .install_operation_id(operation_id)
        .link(CauseRelation::Related, answer.outcome);
    Answer {
        members: answer.members,
        outcome,
        text: None,
    }
}
