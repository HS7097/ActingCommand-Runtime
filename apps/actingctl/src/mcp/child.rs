// SPDX-License-Identifier: AGPL-3.0-only

//! Child-process hygiene for the subprocess tools (#338 §四 子进程卫生): the child's stdin
//! is null, its stdout and stderr are piped and read to the end at the same time, and on
//! Windows it opens no console window (`CREATE_NO_WINDOW` through std's `CommandExt`, no
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
pub(super) struct Captured {
    pub(super) status: ExitStatus,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
}

pub(super) enum ChildFailure {
    /// The program could not be started.
    Spawn(io::Error),
    /// Waiting for the child or reading its output failed.
    Io(io::Error),
    /// The child was still running at the deadline; it keeps running.
    StillRunning,
}

pub(super) fn run_captured(command: Command, deadline: Instant) -> Result<Captured, ChildFailure> {
    run(command, Some(deadline))
}

/// Waits for the child however long it runs (a Lab call's background job).
pub(super) fn run_to_end(command: Command) -> Result<Captured, ChildFailure> {
    run(command, None)
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
