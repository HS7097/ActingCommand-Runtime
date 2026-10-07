// SPDX-License-Identifier: AGPL-3.0-only

//! `actingwatch.exe` (Workflow #374): the program the Runtime watchdog's scheduled task runs
//! every minute from `<root>\tools\`. A GUI-subsystem program opens no console window, so it
//! runs the fixed entry `<root>\runtime\actingctl.exe watchdog run-once --root <root>
//! --from-task` with `CREATE_NO_WINDOW`, appends the tick's stderr to
//! `<root>\watchdog\watchdog.log` and returns the tick's exit code to Task Scheduler. It reads
//! no slot material, takes no slot occupancy and decides nothing (`contracts/runtime-watchdog.md`).
//! While acsetup holds the installation writer lock it does nothing at all; otherwise it holds
//! that lock shared for the tick, so acsetup cannot replace this program or the fixed entry
//! while a tick runs.

#![windows_subsystem = "windows"]
#![forbid(unsafe_code)]

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The launcher's own exit codes; every other code is the tick's.
const MISPLACED: i32 = 20;
const NO_WATCHDOG_DIRECTORY: i32 = 21;
const CANNOT_RUN: i32 = 22;
const WRITER_LOCK_FAILED: i32 = 23;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let Some(root) = install_root() else {
        return MISPLACED;
    };
    let Ok(mut log) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("watchdog").join("watchdog.log"))
    else {
        return NO_WATCHDOG_DIRECTORY;
    };
    let writer = root.join("install").join("writer.lock");
    let _writer = match File::open(&writer) {
        Ok(file) => match file.try_lock_shared() {
            Ok(()) => Some(file),
            // acsetup is installing, upgrading or configuring: this tick does nothing.
            Err(TryLockError::WouldBlock) => return 0,
            Err(TryLockError::Error(error)) => {
                return failed(&mut log, WRITER_LOCK_FAILED, &writer, &error);
            }
        },
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return failed(&mut log, WRITER_LOCK_FAILED, &writer, &error),
    };
    let entry = root.join("runtime").join("actingctl.exe");
    let stderr = match log.try_clone() {
        Ok(stderr) => stderr,
        Err(error) => return failed(&mut log, CANNOT_RUN, &entry, &error),
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
    match command.status() {
        Ok(status) => status.code().unwrap_or(CANNOT_RUN),
        Err(error) => failed(&mut log, CANNOT_RUN, &entry, &error),
    }
}

/// `<root>` when this program is `<root>\tools\actingwatch.exe`, in its ordinary spelling.
fn install_root() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
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

fn failed(log: &mut File, code: i32, path: &Path, error: &std::io::Error) -> i32 {
    let verb = if code == CANNOT_RUN { "run" } else { "lock" };
    // The exit code is the report; a log that refuses the line changes nothing more.
    let _ = writeln!(
        log,
        "ERROR actingwatch: cannot {verb} {}: {error}",
        path.display()
    );
    code
}
