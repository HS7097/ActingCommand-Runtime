// SPDX-License-Identifier: AGPL-3.0-only

//! `actingwatch.exe` (Workflow #374): the program the Runtime watchdog's scheduled task runs
//! every minute from `<root>\tools\`. A GUI-subsystem program opens no console window. It reads
//! no slot material and takes no slot occupancy of its own (`contracts/runtime-watchdog.md`).
//! While acsetup holds the installation writer lock it does nothing at all (exit 0); otherwise
//! it holds that lock shared for the tick, so acsetup cannot replace this program while a tick
//! runs.
//!
//! Workflow #434 LAB-4 (`contracts/lab-two-entries.md` §7, §8.1; register §1.287, "Slice LAB-4"):
//! it runs the tick itself, in this process, from `crates/watchdog`: what its child
//! `<root>\runtime\actingctl.exe watchdog run-once --root <root> --from-task` did before, step by
//! step as actingctl's `main` ran it (the diagnostic sink, the channel fallback report, the
//! process's installation, `ACTINGCOMMAND_DETAIL`, then the tick), with the same decisions,
//! files, log lines, codes and exits. What that child wrote to its stderr, which this program
//! appended to `<root>\watchdog\watchdog.log`, is appended there: the developer and `DIAG` lines
//! and a panic's message. `actingwatch status|install|uninstall` answer as `actingctl watchdog
//! <word> --root <root>` does for the root this program lies in.
//!
//! Workflow #434 LAB-4 fix 1: a GUI-subsystem program started from a console has no standard
//! handles there, so a subcommand first attaches to its parent's console when there is one
//! (`actingcommand_host_metrics::standard_handles::attach_parent_console`; this crate stays free
//! of unsafe code): its report, its developer lines and its errors appear in that console, and
//! its exit code is set on every path. The task's tick (no argument) attaches nothing and stays
//! windowless. The crate's developer lines go where this program names them, never to a default.
//!
//! Workflow #378 A5 (model v2.2 §7.5, §7.6): the tick's exit (0, 1 or 2) is this program's. The
//! launcher's own failures are registered codes (`watchdog_launcher_misplaced`,
//! `watchdog_launcher_log_unavailable`, `watchdog_launcher_writer_lock_failed`), written as the
//! log line `ERROR actingwatch: <code> <path>: <os error text>` when the log is open, and exit 1.

#![windows_subsystem = "windows"]
#![forbid(unsafe_code)]

// Test-only: Workflow #382 A-1 (test plan v2.2 §3.7, G13), the launcher's exits.
#[cfg(test)]
mod gate_382_a1_launcher;

use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::cli_line;
// The registry, for the launcher's own codes (never a literal spelling): actingctl's, in
// `crates/lab-ops` since Workflow #434 LAB-1.
use actingcommand_lab_ops::codes::{ActingctlCode, ArgumentProblem};
use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::OnceLock;

/// The exit of every launcher failure: an error (model §7.6).
const FAILED: i32 = 1;
/// The developer text of a command line actingwatch does not take.
const USAGE: &str = "usage: actingwatch [status|install|uninstall]";
/// The developer text of an `ACTINGCOMMAND_DETAIL` the watchdog does not know (actingctl's).
const DETAIL_USAGE: &str = "ACTINGCOMMAND_DETAIL takes codes or full";

fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    if arguments.is_empty() {
        // The task's tick: 0, 1 or 2. It attaches to no console and stays windowless.
        ExitCode::from(u8::try_from(run(&mut SystemLaunch)).unwrap_or(1))
    } else {
        // Workflow #434 LAB-4 fix 1: a subcommand started from a console prints there. A parent
        // without a console leaves the standard handles as they were (a redirected one still
        // receives the lines); the exit code is set either way.
        let _attached = actingcommand_host_metrics::standard_handles::attach_parent_console();
        command(&SystemLaunch, &arguments)
    }
}

/// Workflow #382 A-1 (test plan v2.2 §3.7, G13): what a launch reads and runs outside its own
/// decisions, namely where this program is, the installation writer lock and, from Workflow #434
/// LAB-4, the tick. The process launches with `SystemLaunch`; a test injects its own.
trait Launch {
    /// This program's path, as `std::env::current_exe` gives it.
    fn current_exe(&self) -> Option<PathBuf>;
    /// The probe of the installation writer lock at `path`.
    fn writer_lock(&self, path: &Path) -> WriterLock;
    /// Runs the task's tick of the installation at `root` in this process, its developer lines
    /// appended to `log`; the tick's exit code.
    fn tick(&mut self, root: &Path, log: &Path) -> i32;
}

/// What the probe of the installation writer lock found.
enum WriterLock {
    /// Free: this handle holds it shared for the tick.
    Shared(File),
    /// There is no writer lock file.
    Absent,
    /// acsetup holds it.
    Busy,
    /// It cannot be probed.
    Failed(std::io::Error),
}

/// The process's launch: its own executable, the writer lock file and the tick of
/// `crates/watchdog`.
struct SystemLaunch;

impl Launch for SystemLaunch {
    fn current_exe(&self) -> Option<PathBuf> {
        std::env::current_exe().ok()
    }

    fn writer_lock(&self, path: &Path) -> WriterLock {
        probe_writer_lock(path)
    }

    fn tick(&mut self, root: &Path, log: &Path) -> i32 {
        // One tick per process: the log is never set already.
        tick_in_process(root, TICK_LOG.get_or_init(|| LogLines(log.to_path_buf())))
    }
}

/// The task tick's `<root>\watchdog\watchdog.log`, set once at its start: the runtime client
/// takes its channel fallback reporter as a plain function ([`tick_channel_report`]), which reads
/// the log here.
static TICK_LOG: OnceLock<LogLines> = OnceLock::new();

/// Tries the writer lock at `path` shared, without waiting.
fn probe_writer_lock(path: &Path) -> WriterLock {
    match File::open(path) {
        Ok(file) => match file.try_lock_shared() {
            Ok(()) => WriterLock::Shared(file),
            Err(TryLockError::WouldBlock) => WriterLock::Busy,
            Err(TryLockError::Error(error)) => WriterLock::Failed(error),
        },
        Err(error) if error.kind() == ErrorKind::NotFound => WriterLock::Absent,
        Err(error) => WriterLock::Failed(error),
    }
}

fn run(launch: &mut impl Launch) -> i32 {
    // Not `<root>\tools\actingwatch.exe`: no root, so no log either.
    let Some(root) = install_root(launch.current_exe()) else {
        return failed(None, ActingctlCode::WatchdogLauncherMisplaced, None);
    };
    let log_path = root.join("watchdog").join("watchdog.log");
    let mut log = match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(log) => log,
        // The log is what cannot be opened: nowhere to write the line.
        Err(_) => return failed(None, ActingctlCode::WatchdogLauncherLogUnavailable, None),
    };
    let writer = root.join("install").join("writer.lock");
    let _writer = match launch.writer_lock(&writer) {
        WriterLock::Shared(file) => Some(file),
        WriterLock::Absent => None,
        // acsetup is installing, upgrading or configuring: this tick does nothing.
        WriterLock::Busy => return 0,
        WriterLock::Failed(error) => {
            return failed(
                Some(&mut log),
                ActingctlCode::WatchdogLauncherWriterLockFailed,
                Some((writer.as_path(), error.to_string())),
            );
        }
    };
    // Workflow #434 LAB-4: the tick in this process, while the writer lock is held shared.
    launch.tick(&root, &log_path)
}

/// `<root>\watchdog\watchdog.log`, appended to line by line: where the tick's stderr went.
struct LogLines(PathBuf);

impl LogLines {
    /// Appends `line`. A log that refuses it changes nothing more: the tick's own log lines and
    /// its exit stay its report, as when the launcher appended the child's stderr.
    fn append(&self, line: &str) {
        if let Ok(mut log) = OpenOptions::new().create(true).append(true).open(&self.0) {
            let _ = writeln!(log, "{line}");
        }
    }
}

/// The developer line `<LEVEL> actingctl: <outcome>[ <text>]`, as actingctl's stderr held it.
fn developer_text(outcome: &Outcome, text: Option<&str>) -> String {
    let level = cli_line::level(outcome.category());
    match text {
        Some(text) => format!("{level} actingctl: {outcome} {text}"),
        None => format!("{level} actingctl: {outcome}"),
    }
}

/// The task's tick of `root`, as actingctl's `main` ran `watchdog run-once --root <root>
/// --from-task`, with what it wrote to stderr appended to the log.
fn tick_in_process(root: &Path, log: &'static LogLines) -> i32 {
    if let Err(error) =
        actingcommand_contract::outcome::install_diagnostic_sink(Box::new(move |outcome, text| {
            log.append(&format!("DIAG actingctl: {outcome} {text}"));
        }))
    {
        log.append(&format!("DIAG actingctl: {error}"));
    }
    // Workflow #434 LAB-4 fix 1: the tick's developer lines go to the log, named for every call.
    let developer: actingcommand_watchdog::DeveloperSink<'_> = &|outcome, text| {
        log.append(&developer_text(outcome, text));
    };
    std::panic::set_hook(Box::new(move |info: &std::panic::PanicHookInfo<'_>| {
        log.append(&format!("thread 'main' {info}"));
    }));
    actingcommand_runtime_client::set_channel_unsupported_reporter(tick_channel_report);
    if let Err(error) = actingcommand_contract::process_installation() {
        let outcome = Outcome::new(error.code());
        developer(&outcome, None);
        return i32::from(cli_line::exit_code(outcome.category(), true));
    }
    if let Err(outcome) = cli_line::detail() {
        developer(&outcome, Some(DETAIL_USAGE));
        return i32::from(cli_line::exit_code(outcome.category(), true));
    }
    let arguments = [
        OsString::from("watchdog"),
        OsString::from("run-once"),
        OsString::from("--root"),
        root.as_os_str().to_owned(),
        OsString::from("--from-task"),
    ];
    actingcommand_watchdog::dispatch_exit(&arguments, developer).map_or(FAILED, i32::from)
}

/// The runtime client's channel fallback report during the task's tick: its developer line,
/// appended to the tick's log, which [`SystemLaunch::tick`] set before the tick installed this
/// reporter.
fn tick_channel_report(outcome: &Outcome) {
    if let Some(log) = TICK_LOG.get() {
        log.append(&developer_text(outcome, None));
    }
}

/// The developer line of a subcommand (Workflow #434 LAB-4 fix 1): on stderr, which the parent's
/// console, when there is one, now shows; the words `cli_line::developer_line` printed for it.
fn stderr_line(outcome: &Outcome, text: Option<&str>) {
    eprintln!("{}", developer_text(outcome, text));
}

/// `actingwatch status|install|uninstall`: `actingctl watchdog <word> --root <root>` for the root
/// this program lies in, as actingctl's `main` runs it (its report on stdout, its developer line
/// on stderr, its exit).
fn command(launch: &impl Launch, arguments: &[OsString]) -> ExitCode {
    if let Err(error) =
        actingcommand_contract::outcome::install_diagnostic_sink(Box::new(|outcome, text| {
            eprintln!("DIAG actingctl: {outcome} {text}");
        }))
    {
        eprintln!("DIAG actingctl: {error}");
    }
    actingcommand_runtime_client::set_channel_unsupported_reporter(|outcome| {
        stderr_line(outcome, None);
    });
    let word = arguments[0].to_str();
    if !matches!(word, Some("status" | "install" | "uninstall")) {
        return print(cli_line::refused(
            &cli_line::usage_invalid(word, ArgumentProblem::Unknown),
            Some(USAGE),
        ));
    }
    if let Some(extra) = arguments.get(1) {
        return print(cli_line::refused(
            &cli_line::usage_invalid(extra.to_str(), ArgumentProblem::Unknown),
            Some(USAGE),
        ));
    }
    let Some(root) = install_root(launch.current_exe()) else {
        return print(cli_line::refused(
            &Outcome::new(ActingctlCode::WatchdogLauncherMisplaced),
            None,
        ));
    };
    if let Err(error) = actingcommand_contract::process_installation() {
        return print(cli_line::refused(&Outcome::new(error.code()), None));
    }
    if let Err(outcome) = cli_line::detail() {
        return print(cli_line::refused(&outcome, Some(DETAIL_USAGE)));
    }
    let arguments = [
        OsString::from("watchdog"),
        arguments[0].clone(),
        OsString::from("--root"),
        root.into_os_string(),
    ];
    ExitCode::from(actingcommand_watchdog::dispatch_exit(&arguments, &stderr_line).unwrap_or(1))
}

/// Prints an exit `lab-ops` built as `cli_line` printed it before Workflow #434 LAB-2, which
/// moved the printing into the programs: the line on stdout, then the developer line
/// `<LEVEL> actingctl: …` on stderr ([`stderr_line`]); a line stdout refuses, or one that did not
/// encode, leaves only the developer line `actingctl_output_failed`, exit 1.
fn print(exit: cli_line::Exit) -> ExitCode {
    let line = match exit.line {
        Ok(line) => line,
        Err(text) => return output_failed(&text),
    };
    let written = {
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        writeln!(writer, "{line}").and_then(|()| writer.flush())
    };
    if let Err(error) = written {
        return output_failed(&error.to_string());
    }
    if let Some(developer) = &exit.developer {
        stderr_line(&developer.outcome, developer.text.as_deref());
    }
    ExitCode::from(exit.code)
}

/// The developer line `actingctl_output_failed` with `text`, exit 1.
fn output_failed(text: &str) -> ExitCode {
    stderr_line(&cli_line::Exit::output_failed(), Some(text));
    ExitCode::FAILURE
}

/// `<root>` when this program (`executable`) is `<root>\tools\actingwatch.exe`, in its ordinary
/// spelling.
fn install_root(executable: Option<PathBuf>) -> Option<PathBuf> {
    let executable = executable?.canonicalize().ok()?;
    let named = |path: &Path, name: &str| {
        path.file_name()
            .and_then(|file| file.to_str())
            .is_some_and(|file| file.eq_ignore_ascii_case(name))
    };
    if !named(&executable, "actingwatch.exe") {
        return None;
    }
    let tools = executable.parent().filter(|tools| named(tools, "tools"))?;
    tools.parent().map(plain_path)
}

/// The canonical path carries the Windows verbatim prefix; children take the plain spelling.
fn plain_path(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{share}"))
    } else if let Some(drive) = text
        .strip_prefix(r"\\?\")
        .filter(|rest| rest.as_bytes().get(1) == Some(&b':'))
    {
        PathBuf::from(drive)
    } else {
        path.to_path_buf()
    }
}

/// A launcher failure: the line `ERROR actingwatch: <code> <path>: <os error text>` when the
/// log is open, and exit 1. The exit is the report; a log that refuses the line changes
/// nothing more.
fn failed(log: Option<&mut File>, failure: ActingctlCode, at: Option<(&Path, String)>) -> i32 {
    if let Some(log) = log {
        let line = match at {
            Some((path, text)) => format!(
                "ERROR actingwatch: {} {}: {text}",
                failure.as_str(),
                path.display()
            ),
            None => format!("ERROR actingwatch: {}", failure.as_str()),
        };
        let _ = writeln!(log, "{line}");
    }
    FAILED
}
