// SPDX-License-Identifier: AGPL-3.0-only

//! `agent-channel` (Workflow #432 AGT-T1; `contracts/agent-forward.md`), moved from actingctl's
//! `main.rs` by Workflow #434 LAB-3 so that the Lab's rescue module serves it too (`agent …`).
//! `crates/agent-forward` parses the words and does the work; this gives its answer as the line,
//! the text or the silence the program prints, unchanged: a hook's stdout would enter the
//! session's context and the notice is text for a person, so neither prints a result line, and a
//! registered session's failed hook exits 2 (the exit its client shows on that event) with its
//! one stderr line.

use crate::cli_line::{self, Exit};
use crate::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_agent_forward::cli::{self, CliAnswer, CliFailure, UsageProblem};
use actingcommand_contract::outcome::Outcome;
use std::ffi::OsString;

/// The exit code of a registered session's failed hook.
pub const HOOK_EXIT: u8 = 2;

/// What an `agent-channel` command answers, for the program to print.
#[derive(Debug)]
pub enum Answer {
    /// One result line: the members and `agent_channel_reported`, or a failure.
    Line(Exit),
    /// Text for a person, printed as it is (`notice`).
    Text(&'static str),
    /// Nothing printed (a hook that did its work).
    Silent,
    /// A registered session's hook that did not do its work: only its developer line
    /// (`agent_channel_failed` with the text) on stderr, exit [`HOOK_EXIT`].
    Hook { outcome: Outcome, text: String },
}

/// Runs `agent-channel` when the first argument names it; `None` leaves every other command to
/// the CLI.
pub fn dispatch(arguments: &[OsString]) -> Option<Answer> {
    let (command, words) = arguments.split_first()?;
    if command.to_str() != Some(cli::COMMAND) {
        return None;
    }
    Some(run(words))
}

/// The words after `agent-channel`, run.
pub fn run(words: &[OsString]) -> Answer {
    match cli::run(words) {
        Ok(CliAnswer::Members(members)) => Answer::Line(cli_line::success(
            members,
            &Outcome::new(ActingctlCode::AgentChannelReported),
        )),
        Ok(CliAnswer::Text(text)) => Answer::Text(text),
        Ok(CliAnswer::Silent) => Answer::Silent,
        Err(CliFailure::Usage {
            argument,
            problem,
            usage,
        }) => {
            let problem = match problem {
                UsageProblem::Unknown => ArgumentProblem::Unknown,
                UsageProblem::Required => ArgumentProblem::Required,
                UsageProblem::Repeated => ArgumentProblem::Repeated,
                UsageProblem::ValueMissing => ArgumentProblem::ValueMissing,
                UsageProblem::NotUtf8 => ArgumentProblem::NotUtf8,
                UsageProblem::NotRecognised => ArgumentProblem::NotRecognised,
            };
            Answer::Line(cli_line::failure(
                &cli_line::usage_invalid(argument.as_deref(), problem),
                Some(usage),
            ))
        }
        Err(CliFailure::Failed { text }) => Answer::Line(cli_line::failure(
            &Outcome::new(ActingctlCode::AgentChannelFailed),
            Some(&text),
        )),
        Err(CliFailure::Hook { text }) => Answer::Hook {
            outcome: Outcome::new(ActingctlCode::AgentChannelFailed),
            text,
        },
    }
}
