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
//! gave one, its error text (of `probe`'s ways and, from the installation proxy's lookup on, of
//! `running`, `launch` and `stop`, only the error's kind). After the line of a
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
    // it; (ACIB-3c) its instruments client traces at debug level every message it sends and
    // reads (`services/dvt/remote_server.rs` 727, 865), the phone's list of running processes
    // among them. With no subscriber those events go nowhere. A new dependency, through which
    // one could be installed, fails the test below (a tripwire against adding one unnoticed, not
    // a proof).
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
/// (ACIB-3b) `running`, `launch` and `stop` the stage they were waiting in and how long each
/// stage before it took, and a command whose device was found by its key the device's number.
fn run(invocation: Invocation, started: Instant) -> Printed {
    let Invocation {
        request,
        timeout_ms,
    } = invocation;
    let (command, device, via) = (request.name(), request.device(), request.via());
    // (ACIB-3d) A line of `stop` names the way of its kill requests, a refusal too.
    let stop_way = request.stop_way();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let failure = Failure::library(Problem::AsyncRuntimeFailed, &error);
            let line = Line::refused_on(Some(command), device, via, failure);
            return Printed::line(line.on_stop_way(stop_way));
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
                // A screenshot keeps its stage in `stage`, `running`, `launch` and `stop` in
                // `steps`; no command sets both.
                let steps = context.steps.borrow();
                let line = Line::deadline_passed(
                    command,
                    device.or(context.device.get()),
                    via,
                    context.stage.get().or(steps.current()),
                    steps.finished(),
                );
                Printed::line(line.on_stop_way(stop_way))
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
    use std::path::Path;

    /// (ACIB-3d, fix round 2) The calls of idevice 0.1.68's public interface this program never
    /// makes, each by its type and its name, in the order of the README's section "Calls this
    /// program never makes", which names each with its file and line and what it would do to the
    /// phone: they restart, shut down, put to sleep or into recovery a phone, unpair it, change
    /// its settings, install, upgrade or remove an application or change its data, or change what
    /// the phone has mounted, holds on its pasteboard or shows.
    const NEVER_MADE: [(&str, &str); 40] = [
        ("DiagnosticsRelayClient", "restart"),
        ("DiagnosticsRelayClient", "shutdown"),
        ("DiagnosticsRelayClient", "sleep"),
        ("LockdownClient", "enter_recovery"),
        ("RestoreServiceClient", "enter_recovery"),
        ("RestoreServiceClient", "reboot"),
        ("LockdownClient", "unpair"),
        ("UsbmuxdConnection", "delete_pair_record"),
        ("UsbmuxdConnection", "save_pair_record"),
        ("LockdownClient", "set_value"),
        ("LockdownClient", "remove_value"),
        ("ConfigurationServiceClient", "set_user_interface_style"),
        ("ConfigurationServiceClient", "set_liquid_glass_opacity"),
        ("ConfigurationServiceClient", "set_color_filter"),
        ("ConfigurationServiceClient", "set_device_text_size"),
        ("ConfigurationServiceClient", "set_reduce_motion"),
        ("ConfigurationServiceClient", "set_increase_contrast"),
        ("ConfigurationServiceClient", "set_show_borders"),
        ("ConfigurationServiceClient", "set_reduce_transparency"),
        ("SpringBoardServicesClient", "set_icon_state"),
        ("SpringBoardServicesClient", "set_icon_state_with_version"),
        ("RestoreServiceClient", "restore_lang"),
        ("InstallationProxyClient", "install"),
        ("InstallationProxyClient", "install_with_callback"),
        ("InstallationProxyClient", "upgrade"),
        ("InstallationProxyClient", "upgrade_with_callback"),
        ("InstallationProxyClient", "uninstall"),
        ("InstallationProxyClient", "uninstall_with_callback"),
        ("AppServiceClient", "uninstall_app"),
        ("FileServiceClient", "propose_empty_file"),
        ("ImageMounter", "upload_image"),
        ("ImageMounter", "upload_image_with_progress"),
        ("ImageMounter", "mount_image"),
        ("ImageMounter", "unmount_image"),
        ("ImageMounter", "mount_developer"),
        ("ImageMounter", "roll_personalization_nonce"),
        ("ImageMounter", "roll_cryptex_nonce"),
        ("PasteboardServiceClient", "set_text"),
        ("PasteboardServiceClient", "set_image"),
        ("OrientationServiceClient", "rotate"),
    ];

    /// (ACIB-3d, fix round 2) The heading of the README's section of the calls this program never
    /// makes; the section ends at the next heading.
    const NEVER_MADE_SECTION: &str = "\n### Calls this program never makes\n";

    /// (ACIB-3d, fix round 2) Every source file of `acib`, by its path under `src/`, with its text.
    /// A file added under `src/` is added here (its path and `include_str!` of it), or
    /// [`the_source_names_none_of_the_calls_this_program_never_makes`] fails, naming it.
    const SOURCES: [(&str, &str); 13] = [
        ("answer.rs", include_str!("answer.rs")),
        ("args.rs", include_str!("args.rs")),
        ("bridge.rs", include_str!("bridge.rs")),
        ("framing.rs", include_str!("framing.rs")),
        ("key.rs", include_str!("key.rs")),
        ("main.rs", include_str!("main.rs")),
        ("matching.rs", include_str!("matching.rs")),
        ("out_file.rs", include_str!("out_file.rs")),
        ("picture.rs", include_str!("picture.rs")),
        ("redact.rs", include_str!("redact.rs")),
        ("stages.rs", include_str!("stages.rs")),
        ("stopping.rs", include_str!("stopping.rs")),
        ("tap.rs", include_str!("tap.rs")),
    ];

    /// Whether the line of code `code` names the method `method` as a call is written: `.method(`
    /// or `.method::<`, or a path to it, `Segment::method` not followed by a character of an
    /// identifier, whose segment is a type's (it starts with an upper-case letter, as a type, an
    /// alias of one and `Self` do) or closes a list of generic arguments (`>`). A function of a
    /// module (as `time::sleep`) is no such path: each call this program never makes is a method.
    fn names_call(code: &str, method: &str) -> bool {
        let identifier = |c: char| c.is_ascii_alphanumeric() || c == '_';
        let called = [[".", method, "("].concat(), [".", method, "::<"].concat()];
        if called.iter().any(|form| code.contains(form.as_str())) {
            return true;
        }
        let path = ["::", method].concat();
        code.match_indices(path.as_str()).any(|(at, _)| {
            let (before, after) = (&code[..at], &code[at + path.len()..]);
            let segment = before
                .rsplit(|c: char| !identifier(c))
                .next()
                .unwrap_or_default();
            let of_a_type = segment.starts_with(|c: char| c.is_ascii_uppercase())
                || (segment.is_empty() && before.ends_with('>'));
            of_a_type && !after.starts_with(identifier)
        })
    }

    /// The paths of the files below `folder`, each written after `prefix` with `/` between its
    /// parts, into `found`.
    fn files_below(folder: &Path, prefix: &str, found: &mut Vec<String>) {
        let entries = std::fs::read_dir(folder)
            .unwrap_or_else(|error| panic!("list {}: {error}", folder.display()));
        for entry in entries {
            let entry =
                entry.unwrap_or_else(|error| panic!("an entry of {}: {error}", folder.display()));
            let name = entry
                .file_name()
                .into_string()
                .unwrap_or_else(|name| panic!("a file name that is no text: {name:?}"));
            let written = format!("{prefix}{name}");
            if entry.path().is_dir() {
                files_below(&entry.path(), &format!("{written}/"), found);
            } else {
                found.push(written);
            }
        }
    }

    /// (ACIB-3d, fix round 2) No source file of `acib` names a call this program never makes
    /// ([`NEVER_MADE`]), in a method call or a path to it ([`names_call`]; lines that open with
    /// `//` are comments and not read): a tripwire for how a call is written, not a proof (a
    /// general request of the library, as `Channel::call_method`, sends whatever it is given, and
    /// no name holds it). The forms are read, first, on lines written here from their parts, so
    /// that this file names none of the calls itself. The README's section names exactly these
    /// calls, in this order, each as the first call of an item of its list. The files read are
    /// [`SOURCES`], and every file under `src/` is one of them: a file the test does not know fails
    /// it, by its path.
    #[test]
    fn the_source_names_none_of_the_calls_this_program_never_makes() {
        // A method call, one with generic arguments, a path through the type, through an alias of
        // it and through its generic arguments are each read as naming the call; a module's
        // function of the same name, a longer name and a longer path are not.
        for (line, method) in [
            (["    client.", "restart", "().await?;"].concat(), "restart"),
            (
                [
                    "        .",
                    "install_with_callback",
                    "::<_, ()>(package, None)",
                ]
                .concat(),
                "install_with_callback",
            ),
            (
                ["    let call = DiagnosticsRelayClient::", "shutdown", ";"].concat(),
                "shutdown",
            ),
            (
                [
                    "    AppService::",
                    "uninstall_app",
                    "(&mut service, bundle)",
                ]
                .concat(),
                "uninstall_app",
            ),
            (
                [
                    "    AppServiceClient::<Box<dyn ReadWrite>>::",
                    "uninstall_app",
                    "(service)",
                ]
                .concat(),
                "uninstall_app",
            ),
        ] {
            assert!(names_call(&line, method), "{line}");
        }
        for (line, method) in [
            ("        tokio::time::sleep(LOOK_INTERVAL).await;", "sleep"),
            ("    let count = client.restart_count();", "restart"),
            ("    let stage = Stage::restarted;", "restart"),
        ] {
            assert!(!names_call(line, method), "{line}");
        }

        let readme = include_str!("../README.md").replace("\r\n", "\n");
        let start = readme
            .find(NEVER_MADE_SECTION)
            .expect("the README has its section of the calls this program never makes")
            + NEVER_MADE_SECTION.len();
        let section = readme[start..].split("\n#").next().unwrap_or_default();
        let listed: Vec<&str> = section
            .lines()
            .filter_map(|line| line.strip_prefix("- `"))
            .map(|item| item.split('`').next().unwrap_or_default())
            .collect();
        let held: Vec<String> = NEVER_MADE
            .iter()
            .map(|(client, method)| format!("{client}::{method}"))
            .collect();
        assert_eq!(
            listed, held,
            "the README's list is the test's, in its order"
        );

        let mut found = Vec::new();
        files_below(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            "",
            &mut found,
        );
        let known: Vec<&str> = SOURCES.iter().map(|(path, _)| *path).collect();
        for file in &found {
            assert!(
                known.contains(&file.as_str()),
                "src/{file} is a source file this test does not read: add it to SOURCES"
            );
        }
        assert_eq!(found.len(), known.len(), "SOURCES names each file once");

        for (file, text) in SOURCES {
            let code = text
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"));
            for line in code {
                for (client, method) in NEVER_MADE {
                    assert!(
                        !names_call(line, method),
                        "src/{file} names {client}::{method}: {line}"
                    );
                }
            }
        }
    }

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
