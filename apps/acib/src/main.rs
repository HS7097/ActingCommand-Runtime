// SPDX-License-Identifier: AGPL-3.0-only

//! `acib`, the ActingCommand iOS Bridge (board #479, ACIB-0 to ACIB-3): the project's own bridge
//! to iOS devices, a program the Runtime will speak to as a separate process, as it speaks to
//! adb.
//!
//! One command per run (`acib devices`, `info`, `services`, `images`, `apps`, `screenshot`,
//! `orientation`, `running` and `probe`, which only read; `launch` and `stop`, which start and
//! end one application on the phone; see `args.rs` and `README.md`). Each run prints one JSON
//! line on standard output and ends with status 0 when the command was answered, 1 when it was
//! refused or failed, the line then naming this program's `problem` word and, where the library
//! gave one, its error text (of the application service, only the error's kind). After the line of a
//! picture asked for with `--out -`, the picture's bytes follow on the standard output
//! (`framing.rs`). The one file it writes is the picture `screenshot` takes, to the path `--out`
//! names, and it removes that file again when the picture could not be written whole into it;
//! it installs no log subscriber and no logger, so nothing the library traces is shown (see
//! `main`).

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
mod stages;
mod stopping;
mod tap;

use answer::{Failure, Line, Problem};
use args::Invocation;
use bridge::Context;
use framing::Printed;
use std::process::ExitCode;
use std::time::{Duration, Instant};

/// The line printed if an answer could not be serialized (nothing the program builds fails to).
const UNWRITABLE: &str = r#"{"problem":"answer_unwritable"}"#;

fn main() -> ExitCode {
    // No log subscriber (and no `log` logger) is installed here, and none may ever be without
    // redaction (the coordinator's ruling on board #479): the library traces at warning level
    // the whole error dictionary a phone answers a CoreDevice request with (idevice 0.1.68,
    // `services/core_device/mod.rs`, "XPC response did not have an output"), which can name
    // other applications, their paths and pids, and other texts of the phone's answers beside
    // it. With no subscriber those events go nowhere. A new dependency, through which one could
    // be installed, fails the test below (a tripwire against adding one unnoticed, not a proof).
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
/// (ACIB-3b) a command of the application service the stage it was waiting in and how long each
/// stage before it took, and a command whose device was found by its key the device's number.
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
        let context = Context::new(tokio::time::Instant::now().checked_add(deadline), started);
        tokio::time::timeout(deadline, bridge::answer(request, &context))
            .await
            .unwrap_or_else(|_| {
                // A screenshot keeps its stage in `stage`, a command of the application service
                // in `steps`; no command sets both.
                let steps = context.steps.borrow();
                Printed::line(Line::deadline_passed(
                    command,
                    device.or(context.device.get()),
                    via,
                    context.stage.get().or(steps.current()),
                    steps.finished(),
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

#[cfg(test)]
mod tests {
    /// The keys of the table `[dependencies]` of the manifest `manifest`, in its order: every
    /// line of it that holds `=` and is no comment, read without the blanks around it and around
    /// its `=` (so a line indented, or written without spaces around `=`, is read too). A line
    /// inside a value that spans lines (a list of features) is read only when it holds `=`, which
    /// no such line of `acib`'s manifest does; one more key read only fails the test. Every other
    /// table of dependencies (development, build or a target's) is refused, its header read
    /// without the blanks around it.
    fn dependencies(manifest: &str) -> Vec<&str> {
        let mut names = Vec::new();
        let mut in_dependencies = false;
        for line in manifest.lines().map(str::trim) {
            if line.starts_with('[') {
                assert!(
                    !line.contains("dependencies") || line == "[dependencies]",
                    "a table of dependencies of its own: {line}"
                );
                in_dependencies = line == "[dependencies]";
                continue;
            }
            if !in_dependencies || line.starts_with('#') {
                continue;
            }
            if let Some((name, _)) = line.split_once('=') {
                names.push(name.trim_end());
            }
        }
        names
    }

    /// A tripwire against adding a dependency unnoticed, not a proof that no log subscriber is
    /// installed. A program installs a subscriber or a logger through a crate that offers one
    /// (`tracing-subscriber`, `tracing` itself, `log` and its loggers); any new line of `acib`'s
    /// dependencies, in any of the forms read below, fails this test, and is then to be held
    /// against the ruling that nothing the library traces is ever shown without redaction (the
    /// comment in `main`). What the crates `acib` depends on do, or let it name, is not checked
    /// here, and whether a subscriber is installed while the program runs cannot be asked
    /// without the `tracing` crate itself.
    #[test]
    fn acib_depends_on_no_crate_that_could_install_a_log_subscriber() {
        // The forms a new dependency could be written in are read: indented, without spaces
        // around `=`, with a dotted key; a value that spans lines is read as its one key.
        let added = "[dependencies]\n\
                     serde = { workspace = true, features = [\n    \"derive\",\n] }\n\
                     \x20 log = \"0.4\"\n\
                     tracing=\"0.1\"\n\
                     \ttracing-subscriber.workspace = true\n\
                     # comment = \"no dependency\"\n";
        assert_eq!(
            dependencies(added),
            ["serde", "log", "tracing", "tracing-subscriber.workspace"]
        );
        assert_eq!(
            dependencies(include_str!("../Cargo.toml")),
            ["idevice", "serde", "serde_json", "sha2", "tokio"]
        );
    }
}
