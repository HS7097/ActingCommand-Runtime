// SPDX-License-Identifier: AGPL-3.0-only

//! Child-process hygiene for the subprocess tools (#338 §四 子进程卫生) and for `actingctl
//! diagnose`, which runs `actingd suspended` as `ac_diagnose` does (Workflow #389 MCP-7): the
//! child's stdin is null, its stdout and stderr are piped and read to the end at the same time,
//! and on Windows it opens no console window (`CREATE_NO_WINDOW` through std's `CommandExt`, no
//! new dependency). [`run_captured`] leaves a child still running at the caller's deadline
//! running: the installer's children follow the stall rule (INST-1b-F) and Lab children are not
//! killed by the model. ROUT-L RL-1c fix 1 (#411 H1): [`run_captured_ending`] ends a child still
//! running at the deadline (std's `Child::kill`, then reaps it), so `actingd suspended` never
//! outlives the diagnosis that started it. A Lab call's background job waits for its child
//! without any deadline; the child ends by its own time limits. Only children left behind when
//! this server itself is killed would need a Job Object (a new dependency) to end.

use actingcommand_contract::outcome::{IoOp, Outcome};
use std::fmt;
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
    /// Waiting for the child, ending it or reading its output failed.
    Io(io::Error),
    /// The child was still running at the deadline; it keeps running. `pid` is its process id
    /// (Workflow #421 INST-1b-F: the installer reports the child it left running).
    StillRunning { pid: u32 },
    /// RL-1c fix 1 (#411 H1): the child was still running at the deadline and was ended there
    /// ([`run_captured_ending`]); `pid` was its process id.
    Ended { pid: u32 },
}

impl ChildFailure {
    /// The OS failure beneath a spawn or I/O failure, as its `foreign_os_error` link (Workflow
    /// #378 A5). `None` when the child was still running at the deadline, and when a thread
    /// reading its output panicked: that error is actingctl's own (`reader_panicked`), not one
    /// the OS raised.
    pub(crate) fn os_failure(&self) -> Option<Outcome> {
        match self {
            Self::Spawn(error) => Some(Outcome::from_io_error(error, IoOp::Spawn)),
            Self::Io(error) if !reader_panicked(error) => {
                Some(Outcome::from_io_error(error, IoOp::Wait))
            }
            Self::Io(_) | Self::StillRunning { .. } | Self::Ended { .. } => None,
        }
    }

    /// Whether a thread reading the child's output panicked.
    pub(crate) fn reader_panicked(&self) -> bool {
        matches!(self, Self::Io(error) if reader_panicked(error))
    }
}

/// The error of an output reader thread that panicked: actingctl's own, so it is told apart from
/// the OS's I/O errors.
#[derive(Debug)]
struct ReaderPanicked;

impl fmt::Display for ReaderPanicked {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("child output reader panicked")
    }
}

impl std::error::Error for ReaderPanicked {}

fn reader_panicked(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.is::<ReaderPanicked>())
}

/// Runs `command` until `deadline`; a child still running then keeps running
/// ([`ChildFailure::StillRunning`]).
pub(crate) fn run_captured(command: Command, deadline: Instant) -> Result<Captured, ChildFailure> {
    run(command, Some(deadline), AtDeadline::Leave)
}

/// RL-1c fix 1 (#411 H1): runs `command` until `deadline`; a child still running then is ended
/// there and reaped ([`ChildFailure::Ended`]).
pub(crate) fn run_captured_ending(
    command: Command,
    deadline: Instant,
) -> Result<Captured, ChildFailure> {
    run(command, Some(deadline), AtDeadline::End)
}

/// Waits for the child however long it runs (a Lab call's background job).
#[cfg_attr(not(feature = "mcp"), allow(dead_code))]
pub(crate) fn run_to_end(command: Command) -> Result<Captured, ChildFailure> {
    run(command, None, AtDeadline::Leave)
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

fn run(
    mut command: Command,
    deadline: Option<Instant>,
    at_deadline: AtDeadline,
) -> Result<Captured, ChildFailure> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_window(&mut command);
    let mut child = command.spawn().map_err(ChildFailure::Spawn)?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let status = wait(&mut child, deadline, at_deadline, &SystemClock)?;
    Ok(Captured {
        status,
        stdout: collect(stdout)?,
        stderr: collect(stderr)?,
    })
}

/// What a wait does with a child still running at its deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtDeadline {
    /// It keeps running.
    Leave,
    /// It is ended and reaped.
    End,
}

/// The child operations a wait uses (a fake child in tests).
trait Waited {
    type Status;
    fn try_wait(&mut self) -> io::Result<Option<Self::Status>>;
    fn kill(&mut self) -> io::Result<()>;
    fn wait(&mut self) -> io::Result<Self::Status>;
    fn id(&self) -> u32;
}

impl Waited for Child {
    type Status = ExitStatus;
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }
    fn kill(&mut self) -> io::Result<()> {
        Child::kill(self)
    }
    fn wait(&mut self) -> io::Result<ExitStatus> {
        Child::wait(self)
    }
    fn id(&self) -> u32 {
        Child::id(self)
    }
}

/// The clock of a wait: now, and the pause between two looks at the child (a fake clock in
/// tests).
trait WaitClock {
    type Time: PartialOrd + Copy;
    fn now(&self) -> Self::Time;
    fn pause(&self, duration: Duration);
}

/// The wall clock.
struct SystemClock;

impl WaitClock for SystemClock {
    type Time = Instant;
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn pause(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

fn wait<W: Waited, C: WaitClock>(
    child: &mut W,
    deadline: Option<C::Time>,
    at_deadline: AtDeadline,
    clock: &C,
) -> Result<W::Status, ChildFailure> {
    let Some(deadline) = deadline else {
        return child.wait().map_err(ChildFailure::Io);
    };
    loop {
        if let Some(status) = child.try_wait().map_err(ChildFailure::Io)? {
            return Ok(status);
        }
        if clock.now() >= deadline {
            let pid = child.id();
            if at_deadline == AtDeadline::Leave {
                return Err(ChildFailure::StillRunning { pid });
            }
            if let Err(error) = child.kill() {
                // It ended between the look and the kill: its status is the answer.
                return match child.try_wait().map_err(ChildFailure::Io)? {
                    Some(status) => Ok(status),
                    None => Err(ChildFailure::Io(error)),
                };
            }
            child.wait().map_err(ChildFailure::Io)?;
            return Err(ChildFailure::Ended { pid });
        }
        clock.pause(POLL_INTERVAL);
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
        .map_err(|_| ChildFailure::Io(io::Error::other(ReaderPanicked)))?
        .map_err(ChildFailure::Io)
}

#[cfg(windows)]
fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_window(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    //! ROUT-L RL-1c fix 1 (#411 H1): a child still running at the call's deadline is ended there,
    //! and the call answers in time. A fake child that never ends on its own and a fake clock
    //! that moves only when the wait pauses; no sleep, no wall clock.

    use super::*;
    use std::cell::Cell;

    const PID: u32 = 4242;

    /// A child that never ends until it is killed; it records the kill and the reap.
    #[derive(Default)]
    struct NeverEnds {
        killed: bool,
        reaped: bool,
        looks: usize,
    }

    impl Waited for NeverEnds {
        type Status = i32;
        fn try_wait(&mut self) -> io::Result<Option<i32>> {
            self.looks += 1;
            Ok(self.killed.then_some(1))
        }
        fn kill(&mut self) -> io::Result<()> {
            self.killed = true;
            Ok(())
        }
        fn wait(&mut self) -> io::Result<i32> {
            assert!(self.killed, "the wait reaps only a child it ended");
            self.reaped = true;
            Ok(1)
        }
        fn id(&self) -> u32 {
            PID
        }
    }

    /// Milliseconds that pass only when the wait pauses.
    #[derive(Default)]
    struct FakeClock {
        now_ms: Cell<u64>,
    }

    impl WaitClock for FakeClock {
        type Time = u64;
        fn now(&self) -> u64 {
            self.now_ms.get()
        }
        fn pause(&self, duration: Duration) {
            let ms = u64::try_from(duration.as_millis()).expect("a short pause");
            self.now_ms.set(self.now_ms.get() + ms);
        }
    }

    #[test]
    fn rl1c_fix1_child_still_running_at_the_deadline_is_ended_in_time() {
        let deadline_ms = 24_000;
        let clock = FakeClock::default();
        let mut child = NeverEnds::default();
        let answer = wait(&mut child, Some(deadline_ms), AtDeadline::End, &clock);
        assert!(
            matches!(answer, Err(ChildFailure::Ended { pid: PID })),
            "the call answers that its child was ended"
        );
        assert!(
            child.killed && child.reaped,
            "the child is ended and reaped"
        );
        let poll_ms = u64::try_from(POLL_INTERVAL.as_millis()).expect("a short poll");
        assert!(
            clock.now() >= deadline_ms && clock.now() < deadline_ms + poll_ms,
            "the call answers at its deadline, within one poll: {} ms",
            clock.now()
        );
        assert_eq!(
            child.looks,
            usize::try_from(deadline_ms / poll_ms).expect("looks") + 1,
            "the wait looks once per poll until the deadline"
        );
    }
}
