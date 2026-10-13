// SPDX-License-Identifier: AGPL-3.0-only

//! Child-process hygiene for the subprocess tools (#338 §四 子进程卫生) and for `actingctl
//! diagnose`, which runs `actingd suspended` as `ac_diagnose` does (Workflow #389 MCP-7): the
//! child's stdin is null, its stdout and stderr are piped and read to the end at the same time,
//! and on Windows it opens no console window (`CREATE_NO_WINDOW` through std's `CommandExt`, no
//! new dependency). A child still running at the caller's deadline is deliberately not
//! killed (std's `Child::kill` could end it): Lab children are not killed by the model, and
//! `actingd suspended` is a bounded batch job that ends on its own. A Lab call's background job
//! waits for its child without any deadline; the child ends by its own time limits. Only
//! children left behind when this server itself is killed would need a Job Object (a new
//! dependency) to end.

use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A finished child and everything it wrote.
pub(crate) struct Captured {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

pub(crate) enum ChildFailure {
    /// The program could not be started.
    Spawn(io::Error),
    /// Waiting for the child or reading its output failed.
    Io(io::Error),
    /// The child was still running at the deadline; it keeps running.
    StillRunning,
}

pub(crate) fn run_captured(command: Command, deadline: Instant) -> Result<Captured, ChildFailure> {
    run(command, Some(deadline))
}

/// Waits for the child however long it runs (a Lab call's background job).
#[cfg_attr(not(feature = "mcp"), allow(dead_code))]
pub(crate) fn run_to_end(command: Command) -> Result<Captured, ChildFailure> {
    run(command, None)
}

/// Pins a child of an installed process to that installation: the install root and the
/// selection travel as the launcher's snapshot, and the child starts in the install root.
pub(crate) fn pin_child(
    command: &mut Command,
    installation: Option<&actingcommand_contract::InstalledProcess>,
) {
    if let Some(installation) = installation {
        command
            .env(
                actingcommand_contract::INSTALL_ROOT_ENV,
                installation.root(),
            )
            .env(
                actingcommand_contract::INSTALL_SELECTION_ENV,
                installation.selection_json(),
            )
            .current_dir(installation.root());
    }
}

fn run(mut command: Command, deadline: Option<Instant>) -> Result<Captured, ChildFailure> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_window(&mut command);
    let mut child = command.spawn().map_err(ChildFailure::Spawn)?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let status = wait(&mut child, deadline)?;
    Ok(Captured {
        status,
        stdout: collect(stdout)?,
        stderr: collect(stderr)?,
    })
}

fn wait(child: &mut Child, deadline: Option<Instant>) -> Result<ExitStatus, ChildFailure> {
    let Some(deadline) = deadline else {
        return child.wait().map_err(ChildFailure::Io);
    };
    loop {
        if let Some(status) = child.try_wait().map_err(ChildFailure::Io)? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(ChildFailure::StillRunning);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Reads one pipe to its end on its own thread, so neither pipe can fill and stall the child.
fn drain<R: Read + Send + 'static>(source: Option<R>) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut source) = source {
            source.read_to_end(&mut bytes)?;
        }
        Ok(bytes)
    })
}

fn collect(reader: JoinHandle<io::Result<Vec<u8>>>) -> Result<Vec<u8>, ChildFailure> {
    reader
        .join()
        .map_err(|_| ChildFailure::Io(io::Error::other("child output reader panicked")))?
        .map_err(ChildFailure::Io)
}

#[cfg(windows)]
fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_window(_command: &mut Command) {}
