// SPDX-License-Identifier: AGPL-3.0-only

//! `actingwatch.exe` (Workflow #374): the program the Runtime watchdog's scheduled task runs
//! every minute from `<root>\tools\`. A GUI-subsystem program opens no console window, so it
//! runs the fixed entry `<root>\runtime\actingctl.exe watchdog run-once --root <root>
//! --from-task` with `CREATE_NO_WINDOW`, appends the tick's stderr to
//! `<root>\watchdog\watchdog.log` and returns the tick's exit code to Task Scheduler. It reads
//! no slot material, takes no slot occupancy and decides nothing (`contracts/runtime-watchdog.md`).
//! While acsetup holds the installation writer lock it does nothing at all (exit 0); otherwise
//! it holds that lock shared for the tick, so acsetup cannot replace this program or the fixed
//! entry while a tick runs.
//!
//! Workflow #378 A5 (model v2.2 §7.5, §7.6): the tick's exit (0, 1 or 2) passes through
//! unchanged. The launcher's own failures are registered codes (`watchdog_launcher_misplaced`,
//! `watchdog_launcher_log_unavailable`, `watchdog_launcher_writer_lock_failed`,
//! `watchdog_launcher_spawn_failed`), written as the log line
//! `ERROR actingwatch: <code> <path>: <os error text>` when the log is open, and exit 1.

#![windows_subsystem = "windows"]
#![forbid(unsafe_code)]

// The registry, for the launcher's own codes (never a literal spelling).
#[allow(dead_code)]
#[path = "codes.rs"]
mod codes;
// Test-only: Workflow #382 A-1 (test plan v2.2 §3.7, G13), the launcher's exits.
#[cfg(test)]
mod gate_382_a1_launcher;

use codes::ActingctlCode;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

/// The exit of every launcher failure: an error (model §7.6).
const FAILED: i32 = 1;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn main() {
    std::process::exit(run(&mut SystemLaunch));
}

/// Workflow #382 A-1 (test plan v2.2 §3.7, G13): what a launch reads and runs outside its own
/// decisions, namely where this program is, the installation writer lock and the fixed entry.
/// The process launches with `SystemLaunch`; a test injects its own.
trait Launch {
    /// This program's path, as `std::env::current_exe` gives it.
    fn current_exe(&self) -> Option<PathBuf>;
    /// The probe of the installation writer lock at `path`.
    fn writer_lock(&self, path: &Path) -> WriterLock;
    /// Runs the fixed entry and waits for its exit.
    fn status(&mut self, command: &mut Command) -> std::io::Result<ExitStatus>;
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

/// The process's launch: its own executable, the writer lock file and a child process.
struct SystemLaunch;

impl Launch for SystemLaunch {
    fn current_exe(&self) -> Option<PathBuf> {
        std::env::current_exe().ok()
    }

    fn writer_lock(&self, path: &Path) -> WriterLock {
        probe_writer_lock(path)
    }

    fn status(&mut self, command: &mut Command) -> std::io::Result<ExitStatus> {
        command.status()
    }
}

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
    let entry = root.join("runtime").join("actingctl.exe");
    let stderr = match log.try_clone() {
        Ok(stderr) => stderr,
        Err(error) => {
            return failed(
                Some(&mut log),
                ActingctlCode::WatchdogLauncherSpawnFailed,
                Some((entry.as_path(), error.to_string())),
            );
        }
    };
    let mut command = Command::new(&entry);
    command
        .args(["watchdog", "run-once", "--root"])
        .arg(&root)
        .arg("--from-task")
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    match launch.status(&mut command) {
        // The tick's own exit: 0, 1 or 2 by its decision's category.
        Ok(status) => match status.code() {
            Some(exit) => exit,
            None => failed(
                Some(&mut log),
                ActingctlCode::WatchdogLauncherSpawnFailed,
                Some((entry.as_path(), status.to_string())),
            ),
        },
        Err(error) => failed(
            Some(&mut log),
            ActingctlCode::WatchdogLauncherSpawnFailed,
            Some((entry.as_path(), error.to_string())),
        ),
    }
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
