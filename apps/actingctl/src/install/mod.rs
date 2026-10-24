// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl install` (Workflow #421; `contracts/install-cli.md`): the command-line installer of
//! a single-location root. Red proof of INST-1 part a: the family is wired, every command answers
//! `actingctl_usage_invalid` until the implementation lands.

mod host;

// Test-only: Workflow #421 INST-1, test rows IT-1 to IT-4 (frozen model INST-M §12), and the
// neutral release fixture they share.
#[cfg(test)]
mod fixture;
#[cfg(test)]
mod gate_inst1_release;

pub(crate) use host::{Host, SystemHost};

use crate::cli_line;
use crate::codes::ActingctlCode;
use actingcommand_contract::outcome::{Category, Outcome};
use serde_json::{Map, Value};
use std::ffi::OsString;
use std::process::ExitCode;

const USAGE: &str =
    "usage: actingctl install release verify|stage, adopt, resume (not implemented yet)";

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

fn print(answer: Answer) -> ExitCode {
    if answer.members.is_empty() && answer.outcome.category() > Category::Info {
        cli_line::failure(&answer.outcome, answer.text)
    } else {
        cli_line::success(answer.members, &answer.outcome)
    }
}

/// Runs one `install` command (the words after `install`) against `host`: not implemented yet.
pub(crate) fn execute(_arguments: &[OsString], host: &dyn Host) -> Answer {
    let _ = (host.now_unix_ms(), host.installer(), host.work_dir());
    Answer {
        members: Map::new(),
        outcome: Outcome::new(ActingctlCode::ActingctlUsageInvalid),
        text: Some(USAGE),
    }
}
