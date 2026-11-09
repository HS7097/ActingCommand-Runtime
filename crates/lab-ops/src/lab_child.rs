// SPDX-License-Identifier: AGPL-3.0-only

//! actinglab as a child (Workflow #434 LAB-1b; register §1.287, L1b-6), moved from MCP's Lab
//! tools (#338 §四 author, S3): the install's `actinglab.exe --json --via <counter> …`, pinned to
//! its installation and talking to the Runtime of a state root, run to its end, and its envelope
//! read into a value. `app launch|restart|stop` run it so ([`crate::apps::run`]); MCP's other Lab
//! tools run it the same way. Finding the install, the jobs, the call budget and the form of a tool
//! error stay with the caller.
//!
//! An error envelope (exit 2..6 with a code) keeps actinglab's class by its exit code
//! (`cli_result.rs:93-100`: 2 usage, 3 safety, 4 device, 5 runtime, 6 usage) as
//! `lab_tool_failed {exit_code, req_id?}`, caused by actinglab's code (`not_implemented` for exit
//! 6) and, when the error has details, by a `foreign_other_error` from the process holding those
//! details' JSON as `raw_text` (actinglab's data, until its own outcomes carry them); Workflow #433
//! LABDO-1: and, when it has a message, by a last `foreign_other_error` holding actinglab's message
//! verbatim as `raw_text`. Any other exit code, or no envelope, is runtime `lab_process_failed`,
//! caused by a `foreign_other_error` holding the stderr tail (and by the details and the message of
//! an error it printed). The child is never killed: it ends by its own time limits.

use crate::child::{self, Captured, ChildFailure};
use crate::codes::ActingctlCode;
use actingcommand_contract::InstalledProcess;
use actingcommand_contract::codes::{ContractCode, SharedCode};
use actingcommand_contract::outcome::{Outcome, RawSource};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

/// actinglab finds the Runtime it talks to here (`state_roots.rs:11-24`).
const STATE_ROOT_ENV: &str = "ACTINGCOMMAND_RUNTIME_STATE_ROOT";
/// Workflow #389 U-2: actinglab's global option naming the counter that forwarded the work (model
/// section 3.2 rule 7).
const VIA: &str = "--via";
/// The tail of a failed child's stderr kept as `raw_text`.
const MAX_STDERR_TAIL: usize = 2048;

/// The install's actinglab and the Runtime it talks to.
pub struct Actinglab<'a> {
    /// `<install root>\tools\actinglab.exe`.
    pub program: &'a Path,
    /// The installation the child is pinned to (`child::pin_child`), when the caller runs from one.
    pub installation: Option<&'a InstalledProcess>,
    /// The state root of the Runtime the child talks to, when it is known.
    pub state_root: Option<&'a Path>,
    /// The counter that forwarded the work (`mcp` for MCP): `--via <via>` after `--json`, so the
    /// child's connections bind the Lab channel with it.
    pub via: &'a str,
}

/// A failure of one child: its class (#338 R6: `usage`, `safety`, `device` or `runtime`) and its
/// outcome.
pub struct Failure {
    pub class: &'static str,
    pub outcome: Outcome,
}

/// What one child answered.
pub struct Answer {
    /// The envelope's `data`, or the failure its exit code names.
    pub result: Result<Value, Failure>,
    /// The envelope's req_id (in `data`, or in the error's details).
    pub req_id: Option<String>,
    /// Whether the child ran to its end (and may have recorded a Lab request).
    pub returned: bool,
}

impl Actinglab<'_> {
    /// `actinglab --json --via <via> <arguments>`, talking to the Runtime of the state root when it
    /// is known.
    pub fn command(&self, arguments: &[String]) -> Command {
        let mut command = Command::new(self.program);
        child::pin_child(&mut command, self.installation);
        command.arg("--json").args([VIA, self.via]).args(arguments);
        if let Some(state_root) = self.state_root {
            command.env(STATE_ROOT_ENV, state_root);
        }
        command
    }

    /// actinglab with `arguments`, run to its end.
    pub fn call(&self, arguments: &[String]) -> Answer {
        run(self.command(arguments))
    }
}

/// `command` run to its end, however long it runs: its envelope's data, or the failure that
/// kept it from answering.
pub(crate) fn run(command: Command) -> Answer {
    match child::run_to_end(command) {
        Ok(captured) => interpret(&captured),
        Err(failure) => Answer {
            result: Err(child_failure(&failure)),
            req_id: None,
            returned: false,
        },
    }
}

/// An actinglab child that gave no answer: runtime `lab_process_failed`, caused by the OS
/// failure that kept it from starting or from being waited for (none when a thread reading its
/// output panicked), or `lab_process_timeout`.
fn child_failure(failure: &ChildFailure) -> Failure {
    let outcome = match failure {
        ChildFailure::StillRunning { .. } | ChildFailure::Ended => {
            Outcome::new(ActingctlCode::LabProcessTimeout)
        }
        ChildFailure::Spawn(_) | ChildFailure::Io(_) => {
            let outcome = Outcome::new(ActingctlCode::LabProcessFailed);
            match failure.os_failure() {
                Some(cause) => outcome.caused_by(cause),
                None => outcome,
            }
        }
    };
    Failure {
        class: "runtime",
        outcome,
    }
}

/// A child's stderr as foreign text: `foreign_other_error` from the process, with the last
/// `MAX_STDERR_TAIL` bytes as `raw_text` (`raw_text_truncated` when that is not all of it) and
/// the exit code when the child exited.
pub fn process_text(captured: &Captured) -> Outcome {
    let text = String::from_utf8_lossy(&captured.stderr);
    let mut start = text.len().saturating_sub(MAX_STDERR_TAIL);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let link = Outcome::new(ContractCode::ForeignOtherError)
        .raw_source(RawSource::Process)
        .raw_text(&text[start..]);
    let link = if start > 0 {
        link.raw_text_truncated(true)
    } else {
        link
    };
    match captured.status.code() {
        Some(exit) => link.exit_code(exit),
        None => link,
    }
}

/// The envelope's data, or the failure its exit code names (`cli_result.rs:93-100`).
pub fn interpret(captured: &Captured) -> Answer {
    let envelope = serde_json::from_slice::<Value>(captured.stdout.trim_ascii()).ok();
    let ok = envelope
        .as_ref()
        .and_then(|envelope| envelope.get("ok"))
        .and_then(Value::as_bool);
    let data = envelope.as_ref().and_then(|envelope| envelope.get("data"));
    let lab_error = envelope
        .as_ref()
        .and_then(|envelope| envelope.get("error"))
        .filter(|error| error.is_object());
    let req_id = match ok {
        Some(true) => data.and_then(|data| data.get("req_id")),
        _ => lab_error
            .and_then(|error| error.get("details"))
            .and_then(|details| details.get("req_id")),
    }
    .and_then(Value::as_str)
    .map(str::to_owned);
    // actinglab's code, as it spells it.
    let spelling = lab_error
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str);
    // Workflow #433 LABDO-1: actinglab's message, kept.
    let message = lab_error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str);
    let result = match (captured.status.code(), ok, spelling) {
        (Some(0), Some(true), _) => Ok(data.cloned().unwrap_or(Value::Null)),
        (Some(exit @ 2..=6), Some(false), Some(spelling)) => Err(command_failed(
            exit,
            spelling,
            req_id.as_deref(),
            lab_error.and_then(|error| error.get("details")),
            message,
        )),
        _ => Err(process_failed(
            captured,
            req_id.as_deref(),
            lab_error.and_then(|error| error.get("details")),
            message,
        )),
    };
    Answer {
        result,
        req_id,
        returned: true,
    }
}

/// An actinglab error envelope: `lab_tool_failed` with actinglab's exit code (and its req_id),
/// caused by actinglab's code (its `spelling`), in the class that exit code names; exit 6 is
/// `not_implemented`. The error's `details`, when it has any, are a second cause (see
/// [`details_text`]), and its `message` the last (see [`message_text`]). Workflow #433
/// LABDO-1: so a Lab operation the Runtime answered and the client's check then refused
/// (`lab_operation_check_failed`, exit 5) is class runtime with its req_id, its details (the
/// effect among them) and actinglab's message.
fn command_failed(
    exit: i32,
    spelling: &str,
    req_id: Option<&str>,
    details: Option<&Value>,
    message: Option<&str>,
) -> Failure {
    let (class, cause) = match exit {
        3 => ("safety", Outcome::relayed(spelling)),
        4 => ("device", Outcome::relayed(spelling)),
        5 => ("runtime", Outcome::relayed(spelling)),
        6 => ("usage", Outcome::new(SharedCode::NotImplemented)),
        _ => ("usage", Outcome::relayed(spelling)),
    };
    let outcome = Outcome::new(ActingctlCode::LabToolFailed).exit_code(exit);
    let outcome = match req_id {
        Some(req_id) => outcome.req_id(req_id),
        None => outcome,
    };
    let outcome = outcome.caused_by(cause);
    let outcome = match details_text(details, Some(exit)) {
        Some(details) => outcome.caused_by(details),
        None => outcome,
    };
    let outcome = match message_text(message, Some(exit)) {
        Some(message) => outcome.caused_by(message),
        None => outcome,
    };
    Failure { class, outcome }
}

/// Workflow #433 LABDO-1: the message of an actinglab error envelope, actinglab's own text about
/// the failure (for a Lab operation, the client's failure after the Runtime answered): a
/// `foreign_other_error` from the process with that text verbatim as `raw_text` (cut at its
/// bound, the whole text then going to the diagnostic sink) and actinglab's exit code when it
/// exited. `None` when the error has no message or an empty one.
fn message_text(message: Option<&str>, exit: Option<i32>) -> Option<Outcome> {
    let message = message.filter(|message| !message.is_empty())?;
    let link = Outcome::new(ContractCode::ForeignOtherError)
        .raw_source(RawSource::Process)
        .raw_text(message);
    Some(match exit {
        Some(exit) => link.exit_code(exit),
        None => link,
    })
}

/// The details of an actinglab error envelope, actinglab's data about the failure: a
/// `foreign_other_error` from the process with their JSON text as `raw_text` (cut at its bound,
/// the whole text then going to the diagnostic sink) and actinglab's exit code when it exited.
/// `None` when the error has no details or empty ones.
fn details_text(details: Option<&Value>, exit: Option<i32>) -> Option<Outcome> {
    let details = details.filter(|details| match details {
        Value::Null => false,
        Value::Object(members) => !members.is_empty(),
        _ => true,
    })?;
    let link = Outcome::new(ContractCode::ForeignOtherError)
        .raw_source(RawSource::Process)
        .raw_text(details.to_string());
    Some(match exit {
        Some(exit) => link.exit_code(exit),
        None => link,
    })
}

/// actinglab answered no usable envelope (none, an unexpected exit code, or `ok` that does not
/// match it): runtime `lab_process_failed`, caused by its stderr and, when it printed an error
/// with details, by those details (see [`details_text`]), and with a message by that message
/// (see [`message_text`]).
fn process_failed(
    captured: &Captured,
    req_id: Option<&str>,
    details: Option<&Value>,
    message: Option<&str>,
) -> Failure {
    let outcome = Outcome::new(ActingctlCode::LabProcessFailed);
    let outcome = match captured.status.code() {
        Some(exit) => outcome.exit_code(exit),
        None => outcome,
    };
    let outcome = match req_id {
        Some(req_id) => outcome.req_id(req_id),
        None => outcome,
    };
    let outcome = outcome.caused_by(process_text(captured));
    let outcome = match details_text(details, captured.status.code()) {
        Some(details) => outcome.caused_by(details),
        None => outcome,
    };
    let outcome = match message_text(message, captured.status.code()) {
        Some(message) => outcome.caused_by(message),
        None => outcome,
    };
    Failure {
        class: "runtime",
        outcome,
    }
}
