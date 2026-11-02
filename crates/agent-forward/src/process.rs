// SPDX-License-Identifier: AGPL-3.0-only

//! One run of a helper program, for the transports of this crate that hand a message to another
//! program (Workflow #432; `contracts/agent-forward.md`). Standard library only: the program gets
//! its input on stdin, two threads read its stdout and stderr up to their bounds, and the calling
//! thread watches the deadline and the stop and kills the program at either. The same pattern as
//! the device crate's command runner, written here because `actingctl` cannot use that crate.
//!
//! A program that goes on past its deadline costs at most that deadline: it is killed and waited
//! for.

use std::io::{self, Read, Write};
use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// The program opens no console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// How long one wait lasts before the stop, the deadline and the program are looked at again.
const WAIT_SLICE: Duration = Duration::from_millis(50);

/// The most a program may write on each stream. One byte more stops it.
pub(crate) struct Bounds {
    pub(crate) stdout: usize,
    pub(crate) stderr: usize,
}

/// A program that ended by itself, with all it wrote.
#[derive(Debug)]
pub(crate) struct Finished {
    /// Whether it exited with 0.
    pub(crate) success: bool,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

/// Why a run gave no [`Finished`].
#[derive(Debug)]
pub(crate) enum RunFailure {
    /// The program could not be started: it never ran.
    Spawn(io::Error),
    /// The deadline came first. The program was killed.
    TimedOut,
    /// The caller asked to stop. The program was killed.
    Stopped,
    /// The program wrote more than a bound. It was killed.
    OutputTooLarge,
    /// The operating system failed once the program had started, or the program could not be
    /// killed. It may have done part of its work.
    Failed(io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Out,
    Err,
}

/// What a thread of one run tells the calling thread.
enum Event {
    Read(Stream, Vec<u8>),
    OverBound,
    ReadFailed(io::Error),
    InputFailed(io::Error),
}

/// What [`supervise`] collected from a program that ended by itself.
struct Streams {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Runs `command` once with `input` on its stdin, until it ends, until `deadline` by `now`, or
/// until `stopped` answers true. The program gets no window. It is always waited for before this
/// returns, unless it cannot be killed.
pub(crate) fn run(
    mut command: Command,
    input: Vec<u8>,
    bounds: &Bounds,
    deadline: Instant,
    stopped: &dyn Fn() -> bool,
    now: &dyn Fn() -> Instant,
) -> Result<Finished, RunFailure> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    let mut child = command.spawn().map_err(RunFailure::Spawn)?;
    let supervised = supervise(&mut child, input, bounds, deadline, stopped, now);
    let ended = end(&mut child);
    match (supervised, ended) {
        (Ok(streams), Ok(status)) => Ok(Finished {
            success: status.success(),
            stdout: streams.stdout,
            stderr: streams.stderr,
        }),
        (Ok(_), Err(error)) | (Err(_), Err(error)) => Err(RunFailure::Failed(error)),
        (Err(failure), Ok(_)) => Err(failure),
    }
}

/// Feeds the program and reads it until it has ended by itself with both streams read, or until
/// the first reason to stop it.
fn supervise(
    child: &mut Child,
    input: Vec<u8>,
    bounds: &Bounds,
    deadline: Instant,
    stopped: &dyn Fn() -> bool,
    now: &dyn Fn() -> Instant,
) -> Result<Streams, RunFailure> {
    let missing = |stream: &str| RunFailure::Failed(io::Error::other(format!("no {stream} pipe")));
    let stdin = child.stdin.take().ok_or_else(|| missing("stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| missing("stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| missing("stderr"))?;
    // `events` keeps this sender, so the channel never closes while this thread waits on it.
    let (events, received) = mpsc::channel();
    spawn_writer(stdin, input, events.clone()).map_err(RunFailure::Failed)?;
    spawn_reader(stdout, bounds.stdout, Stream::Out, events.clone()).map_err(RunFailure::Failed)?;
    spawn_reader(stderr, bounds.stderr, Stream::Err, events.clone()).map_err(RunFailure::Failed)?;
    let mut stdout = None;
    let mut stderr = None;
    let mut exited = false;
    loop {
        if stopped() {
            return Err(RunFailure::Stopped);
        }
        if now() >= deadline {
            return Err(RunFailure::TimedOut);
        }
        if !exited {
            exited = child.try_wait().map_err(RunFailure::Failed)?.is_some();
        }
        if exited && let (Some(out), Some(err)) = (&mut stdout, &mut stderr) {
            return Ok(Streams {
                stdout: std::mem::take(out),
                stderr: std::mem::take(err),
            });
        }
        let wait = deadline.saturating_duration_since(now()).min(WAIT_SLICE);
        match received.recv_timeout(wait) {
            Ok(Event::Read(Stream::Out, bytes)) => stdout = Some(bytes),
            Ok(Event::Read(Stream::Err, bytes)) => stderr = Some(bytes),
            Ok(Event::OverBound) => return Err(RunFailure::OutputTooLarge),
            Ok(Event::ReadFailed(error) | Event::InputFailed(error)) => {
                return Err(RunFailure::Failed(error));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(RunFailure::Failed(io::Error::other(
                    "the threads of the run went away",
                )));
            }
        }
    }
}

/// Ends the program: waits for it if it has ended, else kills it and waits. When it can be neither
/// killed nor seen to have ended, it is left running and the error says so.
fn end(child: &mut Child) -> io::Result<ExitStatus> {
    if let Some(status) = child.try_wait()? {
        return Ok(status);
    }
    if let Err(error) = child.kill() {
        // It may have ended between the two calls.
        return match child.try_wait()? {
            Some(status) => Ok(status),
            None => Err(io::Error::new(
                error.kind(),
                format!("the program could not be killed: {error}"),
            )),
        };
    }
    child.wait()
}

/// Writes the input and closes stdin, so the program reads to its end.
fn spawn_writer(mut stdin: ChildStdin, input: Vec<u8>, events: Sender<Event>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("actingcommand-agent-forward-helper-input".to_owned())
        .spawn(move || {
            let written = stdin.write_all(&input);
            drop(stdin);
            if let Err(error) = written {
                // The receiver is gone only once the run has ended; then nobody needs this.
                let _ = events.send(Event::InputFailed(error));
            }
        })?;
    Ok(())
}

/// Reads one stream to its end, keeping at most `bound` bytes; one byte more is reported at once.
fn spawn_reader(
    mut source: impl Read + Send + 'static,
    bound: usize,
    stream: Stream,
    events: Sender<Event>,
) -> io::Result<()> {
    let name = match stream {
        Stream::Out => "actingcommand-agent-forward-helper-stdout",
        Stream::Err => "actingcommand-agent-forward-helper-stderr",
    };
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let mut bytes = Vec::new();
            let limit = u64::try_from(bound).unwrap_or(u64::MAX).saturating_add(1);
            let event = match (&mut source).take(limit).read_to_end(&mut bytes) {
                Ok(_) if bytes.len() > bound => Event::OverBound,
                Ok(_) => Event::Read(stream, bytes),
                Err(error) => Event::ReadFailed(error),
            };
            // The receiver is gone only once the run has ended; then nobody needs this.
            let _ = events.send(event);
        })?;
    Ok(())
}
