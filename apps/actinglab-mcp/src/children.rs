// SPDX-License-Identifier: AGPL-3.0-only

//! The `serve` children of this server (Workflow #434 LAB-5G4b; register §1.287, "Slice
//! LAB-5G4b"; `contracts/lab-two-entries.md` §8.2 and §9.3, "Calls served by `serve` children";
//! the survey's §2, "Children, as the server must handle them"; the coordinator's rulings on board
//! #434, 2, 5 and 20 of 6775192250 and 6827665631 on this slice, not the owner's words). A row
//! neither this process's core serving code nor the rescue module serves
//! (`tools::Serving::Child`) is served by actinglab's own `serve` loop in a child process, one
//! request per child at a time.
//!
//! - **The program** ([`Worker::start`]): `actinglab.exe` beside this program's own file, started
//!   as `actinglab.exe serve --stdio --front mcp` (LAB-5G4a's option: what its commands add to a
//!   recording carries `Lab-MCP`), with the environment today's server gives its Lab child: the
//!   state root variable set to this server's state root when it is known,
//!   `ACTINGCOMMAND_INITIATOR` removed, pinned to this process's installation as today
//!   (`lab_ops::child::pin_child`); its three streams piped, no console window.
//! - **One request at a time** ([`Children::line`]): a call takes an idle child or starts one,
//!   writes one line `{"op", "params"}` and reads one line, the envelope `{exit_code, req_id?,
//!   line}` `serve` answers under `--front mcp`: the call's line is its `line`, which exits with
//!   its `exit_code`. At most `--children` children live at once; a call while every one is busy
//!   is refused, not queued, with the code a job past `--jobs` is refused with,
//!   `job_capacity_exhausted`, its `limit_count` the children's limit.
//! - **Idle** ([`reap`]): a child idle for `--child-idle` seconds is ended by closing its input,
//!   by a thread of its own, without waiting for the next call (ruling 5: a resident server keeps
//!   no idle child, which would block a deploy); with `--child-idle 0` at once after its answer.
//! - **A child that fails** ([`Worker::failed`]): one that ends, or whose output ends, before its
//!   answer, or whose answer is no envelope, is answered `lab_process_failed` (its exit code when
//!   it exited) caused by the tail of its stderr, as today's server answers a Lab child that
//!   printed no answer (`lab_ops::lab_child`); the request is never sent again, the child is
//!   dropped and the next call starts a new one. A child's stderr is read as it comes, so a full
//!   pipe never stops it, and only its last 2 KiB are kept.
//! - **Never killed**: a call not done within the call budget answers its job's handle, and its
//!   child stays busy until its line comes. At the end of the server's input
//!   ([`Children::end_of_input`]) every idle child's input is closed at once and a busy one's once
//!   its line came (or with the server's process): each ends after the line it is working on.

use crate::lock;
use crate::runtime;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use actingcommand_lab_ops::child;
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::codes::ActingctlCode;
use actingcommand_lab_ops::initiator;
use actingcommand_lab_ops::locate;
use actingcommand_lab_ops::table::Operation;
use serde_json::{Map, Value};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The program a child runs, beside this program's own file, and its words.
const ACTINGLAB: &str = "actinglab";
const WORDS: [&str; 4] = ["serve", "--stdio", "--front", "mcp"];
/// A request's members, and those of the envelope that answers it that the call reads.
const OP: &str = "op";
const PARAMS: &str = "params";
const EXIT_CODE: &str = "exit_code";
const LINE: &str = "line";
/// The tail of a child's stderr kept, today's (`lab_ops::lab_child`, 2048 bytes), and how much is
/// read at once.
const STDERR_TAIL: usize = 2048;
const STDERR_CHUNK: usize = 4096;
/// The threads of the children: the one that ends the idle ones, each child's stderr reader, and
/// the one that waits for a child whose input was closed.
const IDLE_THREAD: &str = "mcp-children-idle";
const STDERR_THREAD: &str = "mcp-child-stderr";
const ENDED_THREAD: &str = "mcp-child-ended";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The start options of the children (ruling 20 of 6775192250: a tunable is a start option with
/// a default): how many live at once (`--children`) and how long one may stay idle
/// (`--child-idle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Options {
    pub(crate) children: usize,
    pub(crate) idle: Duration,
}

impl Default for Options {
    /// The defaults of the start options.
    fn default() -> Self {
        Self {
            children: crate::DEFAULT_CHILDREN,
            idle: Duration::from_secs(crate::DEFAULT_CHILD_IDLE_S),
        }
    }
}

/// This server's `serve` children.
pub(crate) struct Children {
    pool: Arc<Pool>,
}

struct Pool {
    options: Options,
    state: Mutex<State>,
    /// Wakes the thread that ends idle children: a child went idle, or the input ended.
    changed: Condvar,
}

#[derive(Default)]
struct State {
    /// The children not working, the one used last at the end.
    idle: Vec<Idle>,
    /// The children alive, idle and working.
    live: usize,
    /// The server's input ended: no child is kept after its line.
    ended: bool,
    /// A thread ends the idle children; without one none is kept after its line.
    reaping: bool,
}

struct Idle {
    worker: Worker,
    since: Instant,
}

/// One `serve` child.
struct Worker {
    process: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    errors: Arc<Mutex<Tail>>,
    reader: Option<JoinHandle<()>>,
}

/// The last bytes of a child's stderr, and whether earlier ones were dropped.
#[derive(Default)]
struct Tail {
    bytes: Vec<u8>,
    cut: bool,
}

impl Children {
    /// Children with `options`, none of them kept idle: no thread ends idle ones.
    pub(crate) fn new(options: Options) -> Self {
        Self {
            pool: Arc::new(Pool {
                options,
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
            }),
        }
    }

    /// Children with `options` whose idle ones a thread of their own ends after `options.idle`
    /// (none with 0, which keeps no child idle); a thread the OS refuses is
    /// `mcp_server_start_failed` with its name, as the server's own threads are.
    pub(crate) fn started(options: Options) -> Result<Self, Outcome> {
        let children = Self::new(options);
        if options.idle.is_zero() {
            return Ok(children);
        }
        lock(&children.pool.state).reaping = true;
        let pool = Arc::clone(&children.pool);
        let _reaper = thread::Builder::new()
            .name(IDLE_THREAD.to_owned())
            .spawn(move || reap(&pool))
            .map_err(|error| {
                Outcome::new(ActingctlCode::McpServerStartFailed)
                    .thread(IDLE_THREAD)
                    .caused_by(Outcome::from_io_error(&error, IoOp::Spawn))
            })?;
        Ok(children)
    }

    /// The line of `operation` with `params` (the call's arguments, `calls::prepare`d), from a
    /// child: its envelope's `line` with its exit code, a refusal the child answers passed on as
    /// it is; `job_capacity_exhausted` while every child is busy; `lab_process_failed` for a child
    /// that gave no answer.
    pub(crate) fn line(&self, operation: &Operation, params: &Map<String, Value>) -> Exit {
        let mut request = Map::new();
        request.insert(OP.to_owned(), Value::String(operation.name.clone()));
        request.insert(PARAMS.to_owned(), Value::Object(params.clone()));
        let mut text = Value::Object(request).to_string().into_bytes();
        text.push(b'\n');
        let mut worker = match self.pool.take() {
            Ok(worker) => worker,
            Err(refusal) => return cli_line::failure(&refusal, None),
        };
        match worker.ask(&text) {
            Ok(exit) => {
                self.pool.give_back(worker);
                exit
            }
            Err(cause) => {
                self.pool.lost();
                cli_line::failure(&worker.failed(cause), None)
            }
        }
    }

    /// The server's input ended: every idle child's input is closed now, and a working one's once
    /// its line came; none is killed.
    pub(crate) fn end_of_input(&self) {
        let idle = {
            let mut state = lock(&self.pool.state);
            state.ended = true;
            let idle = std::mem::take(&mut state.idle);
            state.live = state.live.saturating_sub(idle.len());
            idle
        };
        self.pool.changed.notify_all();
        for entry in idle {
            entry.worker.retire();
        }
    }
}

impl Pool {
    /// An idle child that still runs, or a new one while fewer than `--children` live; else
    /// `job_capacity_exhausted` with the children's limit. An idle child that ended meanwhile is
    /// dropped and another is taken.
    fn take(&self) -> Result<Worker, Outcome> {
        loop {
            let idle = {
                let mut state = lock(&self.state);
                match state.idle.pop() {
                    Some(entry) => Some(entry.worker),
                    None if state.live >= self.options.children => {
                        return Err(Outcome::new(ActingctlCode::JobCapacityExhausted)
                            .limit_count(self.options.children));
                    }
                    None => {
                        state.live += 1;
                        None
                    }
                }
            };
            let Some(mut worker) = idle else {
                return Worker::start().inspect_err(|_| self.lost());
            };
            if worker.running() {
                return Ok(worker);
            }
            self.lost();
            worker.retire();
        }
    }

    /// A child that answered: kept idle while a thread ends idle children and the input goes on,
    /// else its input is closed now.
    fn give_back(&self, worker: Worker) {
        let mut state = lock(&self.state);
        if state.ended || !state.reaping {
            state.live = state.live.saturating_sub(1);
            drop(state);
            worker.retire();
            return;
        }
        state.idle.push(Idle {
            worker,
            since: Instant::now(),
        });
        self.changed.notify_all();
    }

    /// A child that is no longer counted.
    fn lost(&self) {
        let mut state = lock(&self.state);
        state.live = state.live.saturating_sub(1);
    }
}

/// Ends each child idle for `--child-idle` by closing its input, until the server's input ends.
fn reap(pool: &Pool) {
    let idle_for = pool.options.idle;
    let mut state = lock(&pool.state);
    loop {
        if state.ended {
            return;
        }
        let now = Instant::now();
        let (expired, kept): (Vec<Idle>, Vec<Idle>) = std::mem::take(&mut state.idle)
            .into_iter()
            .partition(|entry| now.saturating_duration_since(entry.since) >= idle_for);
        state.idle = kept;
        if !expired.is_empty() {
            state.live = state.live.saturating_sub(expired.len());
            drop(state);
            for entry in expired {
                entry.worker.retire();
            }
            state = lock(&pool.state);
            continue;
        }
        let next = state.idle.iter().map(|entry| entry.since + idle_for).min();
        state = match next {
            Some(deadline) => pool
                .changed
                .wait_timeout(state, deadline.saturating_duration_since(now))
                .map_or_else(|poisoned| poisoned.into_inner().0, |(state, _)| state),
            None => pool
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner),
        };
    }
}

impl Worker {
    /// A new child (see the module's description). A program that cannot be started is
    /// `lab_process_failed` caused by the OS failure, which names the program.
    fn start() -> Result<Self, Outcome> {
        let failed =
            |cause: Outcome| Outcome::new(ActingctlCode::LabProcessFailed).caused_by(cause);
        let program = std::env::current_exe()
            .map(|own| own.with_file_name(format!("{ACTINGLAB}{}", std::env::consts::EXE_SUFFIX)))
            .map_err(|error| failed(Outcome::from_io_error(&error, IoOp::Locate)))?;
        let mut command = Command::new(&program);
        let installation = locate::locate()
            .ok()
            .and_then(|location| location.installation);
        child::pin_child(&mut command, installation.as_ref());
        command
            .args(WORDS)
            .env_remove(initiator::ENVIRONMENT_VARIABLE)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Ok(state_root) = runtime::place().state_root {
            command.env(runtime::STATE_ROOT_VARIABLE, state_root);
        }
        hide_window(&mut command);
        let mut process = command
            .spawn()
            .map_err(|error| failed(spawn_failed(&error, program)))?;
        // Every stream is piped, so std hands each over; without one the child is not served,
        // and it ends with its input, closed here.
        let (Some(input), Some(output), Some(errors)) = (
            process.stdin.take(),
            process.stdout.take(),
            process.stderr.take(),
        ) else {
            return Err(Outcome::new(ActingctlCode::LabProcessFailed));
        };
        let tail = Arc::new(Mutex::new(Tail::default()));
        let reading = Arc::clone(&tail);
        let reader = thread::Builder::new()
            .name(STDERR_THREAD.to_owned())
            .spawn(move || read_errors(errors, &reading))
            .map_err(|error| failed(Outcome::from_io_error(&error, IoOp::Spawn)))?;
        Ok(Self {
            process,
            input,
            output: BufReader::new(output),
            errors: tail,
            reader: Some(reader),
        })
    }

    /// Whether the child has not ended.
    fn running(&mut self) -> bool {
        matches!(self.process.try_wait(), Ok(None))
    }

    /// Sends the request line `request` and reads the answer line: the exit of its envelope's
    /// `line`. `Err` when the child gave none (see [`Self::failed`]), with the failure that kept it
    /// from answering when there is one: the write's, the read's, or the line that is no envelope.
    fn ask(&mut self, request: &[u8]) -> Result<Exit, Option<Outcome>> {
        self.input
            .write_all(request)
            .and_then(|()| self.input.flush())
            .map_err(|error| Some(Outcome::from_io_error(&error, IoOp::Write)))?;
        let mut answer = Vec::new();
        let read = self
            .output
            .read_until(b'\n', &mut answer)
            .map_err(|error| Some(Outcome::from_io_error(&error, IoOp::Read)))?;
        if read == 0 {
            return Err(None);
        }
        enveloped(&answer).ok_or_else(|| {
            Some(
                Outcome::new(ContractCode::ForeignDataError)
                    .raw_source(RawSource::Json)
                    .raw_text(String::from_utf8_lossy(&answer).into_owned()),
            )
        })
    }

    /// A child that gave no answer: its input and output closed, its end waited for and its
    /// stderr read to its end; `lab_process_failed` with its exit code when it exited, caused by
    /// its stderr's tail and by `cause` when there is one.
    fn failed(self, cause: Option<Outcome>) -> Outcome {
        let Self {
            mut process,
            input,
            output,
            errors,
            reader,
        } = self;
        drop(input);
        drop(output);
        let status = process.wait();
        if let Some(reader) = reader {
            let _ = reader.join();
        }
        let exit = status.as_ref().ok().and_then(ExitStatus::code);
        let mut failure = Outcome::new(ActingctlCode::LabProcessFailed);
        if let Some(exit) = exit {
            failure = failure.exit_code(exit);
        }
        failure = failure.caused_by(lock(&*errors).text(exit));
        if let Some(cause) = cause {
            failure = failure.caused_by(cause);
        }
        if let Err(error) = &status {
            failure = failure.caused_by(Outcome::from_io_error(error, IoOp::Wait));
        }
        failure
    }

    /// Closes the child's input, so that it ends after the line it is working on, and waits for
    /// its end on a thread of its own. A thread the OS refuses is reported on stderr; the child
    /// ends all the same, unwaited.
    fn retire(self) {
        let Self {
            mut process,
            input,
            output,
            reader,
            ..
        } = self;
        drop(input);
        let spawned = thread::Builder::new()
            .name(ENDED_THREAD.to_owned())
            .spawn(move || {
                let _ = process.wait();
                drop(output);
                if let Some(reader) = reader {
                    let _ = reader.join();
                }
            });
        if let Err(error) = spawned {
            let failure = Outcome::new(ActingctlCode::JobStartFailed)
                .caused_by(Outcome::from_io_error(&error, IoOp::Spawn));
            eprintln!(
                "{}",
                cli_line::developer_text(
                    crate::PROGRAM,
                    &failure,
                    Some("a serve child whose input was closed is not waited for")
                )
            );
        }
    }
}

/// The exit of a child's answer line: its envelope's `line`, exiting with its `exit_code`; `None`
/// for a line that is no such envelope.
fn enveloped(answer: &[u8]) -> Option<Exit> {
    let envelope = serde_json::from_slice::<Value>(answer.trim_ascii()).ok()?;
    let code = envelope
        .get(EXIT_CODE)?
        .as_u64()
        .and_then(|code| u8::try_from(code).ok())?;
    let line = envelope.get(LINE)?.clone();
    Some(Exit {
        line: Ok(line),
        developer: None,
        code,
    })
}

/// The failure to start the child at `program`, which names it.
fn spawn_failed(error: &io::Error, program: PathBuf) -> Outcome {
    Outcome::from_io_error(error, IoOp::Spawn).path(program)
}

/// Reads a child's stderr as it comes into its tail, until it ends; a read that fails ends it.
fn read_errors(mut source: ChildStderr, tail: &Mutex<Tail>) {
    let mut chunk = [0_u8; STDERR_CHUNK];
    loop {
        match source.read(&mut chunk) {
            Ok(0) => return,
            Ok(read) => lock(tail).push(&chunk[..read]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

impl Tail {
    /// `bytes` after the ones kept, of which the last [`STDERR_TAIL`] stay.
    fn push(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
        if self.bytes.len() > STDERR_TAIL {
            let dropped = self.bytes.len() - STDERR_TAIL;
            self.bytes.drain(..dropped);
            self.cut = true;
        }
    }

    /// The tail as a child's text, as `lab_ops::lab_child` gives it: `foreign_other_error` from
    /// the process, its text the tail from its first whole character (`raw_text_truncated` when
    /// earlier bytes were dropped), and the exit code when the child exited.
    fn text(&self, exit: Option<i32>) -> Outcome {
        let start = self
            .bytes
            .iter()
            .position(|byte| (byte & 0b1100_0000) != 0b1000_0000)
            .unwrap_or(self.bytes.len());
        let link = Outcome::new(ContractCode::ForeignOtherError)
            .raw_source(RawSource::Process)
            .raw_text(String::from_utf8_lossy(&self.bytes[start..]).into_owned());
        let link = if self.cut {
            link.raw_text_truncated(true)
        } else {
            link
        };
        match exit {
            Some(exit) => link.exit_code(exit),
            None => link,
        }
    }
}

#[cfg(windows)]
fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_window(_command: &mut Command) {}
