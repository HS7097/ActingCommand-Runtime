// SPDX-License-Identifier: AGPL-3.0-only

//! `acib`, the ActingCommand iOS Bridge (board #479, ACIB-0): the project's own bridge to iOS
//! devices, a program the Runtime will speak to as a separate process, as it speaks to adb.
//!
//! This first version only reads, one command per run (`acib devices`, `info`, `services`,
//! `images`, `apps`; see `args.rs` and `README.md`). Each run prints one JSON line on standard
//! output and ends with status 0 when the command was answered, 1 when it was refused or failed,
//! the line then naming this program's `problem` word and, where the library gave one, its error
//! text. It writes no file and installs no logger, so nothing the library traces is shown.

#![forbid(unsafe_code)]

mod answer;
mod args;
mod bridge;
mod redact;

use answer::{Failure, Line, Problem};
use args::Invocation;
use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

/// The line printed if an answer could not be serialized (nothing the program builds fails to).
const UNWRITABLE: &str = r#"{"problem":"answer_unwritable"}"#;

fn main() -> ExitCode {
    let arguments: Option<Vec<String>> = std::env::args_os()
        .skip(1)
        .map(|argument| argument.into_string().ok())
        .collect();
    let line = match arguments.as_deref().and_then(args::parse) {
        Some(invocation) => run(invocation),
        None => Line::refused(None, None, Failure::own(Problem::CommandLineInvalid)),
    };
    emit(&line)
}

/// Runs the command on a current-thread runtime of its own, within its deadline.
fn run(invocation: Invocation) -> Line {
    let Invocation {
        request,
        timeout_ms,
    } = invocation;
    let (command, device) = (request.name(), request.device());
    let refused = |failure| Line::refused(Some(command), device, failure);
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return refused(Failure::library(Problem::AsyncRuntimeFailed, &error)),
    };
    let deadline = Duration::from_millis(timeout_ms);
    runtime
        .block_on(async { tokio::time::timeout(deadline, bridge::answer(request)).await })
        .unwrap_or_else(|_| refused(Failure::own(Problem::DeadlinePassed)))
}

/// Prints the line; the status is 0 only for an answered command whose line was written.
fn emit(line: &Line) -> ExitCode {
    let (text, answered) = match serde_json::to_string(line) {
        Ok(text) => (text, line.answered()),
        Err(_) => (UNWRITABLE.to_owned(), false),
    };
    let mut stdout = std::io::stdout().lock();
    let written = writeln!(stdout, "{text}").and_then(|()| stdout.flush());
    if written.is_ok() && answered {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
