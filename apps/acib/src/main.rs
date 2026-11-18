// SPDX-License-Identifier: AGPL-3.0-only

//! `acib`, the ActingCommand iOS Bridge (board #479, ACIB-0 to ACIB-3): the project's own bridge
//! to iOS devices, a program the Runtime will speak to as a separate process, as it speaks to
//! adb.
//!
//! One command per run (`acib devices`, `info`, `services`, `images`, `apps`, `screenshot`,
//! `orientation` and `running`, which only read; `launch` and `stop`, which start and end one
//! application on the phone; see `args.rs` and `README.md`). Each run prints one JSON line on
//! standard output and ends with status 0 when the command was answered, 1 when it was refused
//! or failed, the line then naming this program's `problem` word and, where the library gave
//! one, its error text (of the application service, only the error's kind). After the line of a
//! picture asked for with `--out -`, the picture's bytes follow on the standard output
//! (`framing.rs`). The one file it writes is the picture `screenshot` takes, to the path `--out`
//! names, and it removes that file again when the picture could not be written whole into it;
//! it installs no logger, so nothing the library traces is shown.

#![forbid(unsafe_code)]

mod answer;
mod args;
mod bridge;
mod framing;
mod key;
mod matching;
mod out_file;
mod picture;
mod redact;

use answer::{Failure, Line, Problem};
use args::Invocation;
use bridge::Context;
use framing::Printed;
use std::process::ExitCode;
use std::time::{Duration, Instant};

/// The line printed if an answer could not be serialized (nothing the program builds fails to).
const UNWRITABLE: &str = r#"{"problem":"answer_unwritable"}"#;

fn main() -> ExitCode {
    let started = Instant::now();
    let arguments: Option<Vec<String>> = std::env::args_os()
        .skip(1)
        .map(|argument| argument.into_string().ok())
        .collect();
    let printed = match arguments.as_deref().and_then(args::parse) {
        Some(invocation) => run(invocation, started),
        None => Printed::line(Line::refused(
            None,
            None,
            Failure::own(Problem::CommandLineInvalid),
        )),
    };
    emit(&printed)
}

/// Runs the command, which started at `started`, on a current-thread runtime of its own,
/// within its deadline; a screenshot whose deadline passes names the stage it was waiting in,
/// and a command whose device was found by its key the device's number.
fn run(invocation: Invocation, started: Instant) -> Printed {
    let Invocation {
        request,
        timeout_ms,
    } = invocation;
    let (command, device, via) = (request.name(), request.device(), request.via());
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let failure = Failure::library(Problem::AsyncRuntimeFailed, &error);
            return Printed::line(Line::refused_on(Some(command), device, via, failure));
        }
    };
    let deadline = Duration::from_millis(timeout_ms);
    runtime.block_on(async {
        // The context's deadline is taken before the command's own timeout starts, so a reader
        // of `orientation` that is cut by it is cut no later than the whole command.
        let context = Context::new(tokio::time::Instant::now().checked_add(deadline));
        tokio::time::timeout(deadline, bridge::answer(request, started, &context))
            .await
            .unwrap_or_else(|_| {
                Printed::line(Line::deadline_passed(
                    command,
                    device.or(context.device.get()),
                    via,
                    context.stage.get(),
                ))
            })
    })
}

/// Prints the line, and the picture's bytes after it when they were asked for on the standard
/// output; the status is 0 only for an answered command whose output was written whole.
fn emit(printed: &Printed) -> ExitCode {
    let (text, picture, answered) = match serde_json::to_string(&printed.line) {
        Ok(text) => (text, printed.picture.as_deref(), printed.line.answered()),
        Err(_) => (UNWRITABLE.to_owned(), None, false),
    };
    let written = framing::write_framed(std::io::stdout().lock(), &text, picture);
    if written.is_ok() && answered {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
