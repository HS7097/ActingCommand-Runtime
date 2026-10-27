// SPDX-License-Identifier: AGPL-3.0-only

//! `actingcommand-mcp-entry` (Workflow #421 MCP-ENTRY): the one stable MCP entry every client
//! starts, from `<root>\mcp-entry\g<generation>\`.
//!
//! usage: `actingcommand-mcp-entry --root <root> [--client claude|codex|antigravity|dsh] --
//! <child arguments>`, or `actingcommand-mcp-entry --generation`.
//!
//! Everything after `--` is passed verbatim to every child, `<root>\runtime\actingctl.exe`
//! (`mcp-serve --tier …`). The entry decides nothing about builds: it asks the current
//! install's `actingctl mcp-entry plan` (see `logic`). It writes
//! `<root>\mcp-entry\state\entry-<pid>.json` and never deletes a status file.

mod codes;
mod logic;

#[cfg(test)]
mod tests;

use crate::codes::EntryCode;
use crate::logic::{
    ClientKind, Core, ENTRY_PROTOCOL, EntryConfig, GENERATION, Host, PlanEnd, Stamp,
};
use actingcommand_contract::outcome::{Detail, Outcome};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::{self, File, TryLockError};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitCode, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const USAGE: &str = "usage: actingcommand-mcp-entry --root <root> [--client claude|codex|antigravity|dsh] -- <child arguments>\nusage: actingcommand-mcp-entry --generation";
/// How long the entry waits, after the client closed its input, for the current child to
/// answer what it still owes and end.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// How long a stopped child may take to end before the entry ends it.
const STOP_GRACE: Duration = Duration::from_secs(5);
const REAP_POLL: Duration = Duration::from_millis(50);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

enum Event {
    Client(Vec<u8>),
    ClientEnd,
    ChildLine(u64, Vec<u8>),
    ChildEnd(u64),
    Plan(PlanEnd),
    Tick,
}

struct Arguments {
    root: PathBuf,
    client: Option<ClientKind>,
    child: Vec<String>,
}

fn main() -> ExitCode {
    let words = std::env::args().skip(1).collect::<Vec<_>>();
    if words.len() == 1 && words[0] == "--generation" {
        println!("{GENERATION}");
        return ExitCode::SUCCESS;
    }
    let Some(arguments) = parse(&words) else {
        let outcome = Outcome::new(EntryCode::UsageInvalid);
        eprintln!(
            "ERROR actingcommand-mcp-entry: {} {USAGE}",
            serde_json::to_string(&outcome.envelope(Detail::Full))
                .unwrap_or_else(|_| EntryCode::UsageInvalid.as_str().to_owned())
        );
        return ExitCode::from(2);
    };
    run(arguments)
}

fn parse(words: &[String]) -> Option<Arguments> {
    let split = words.iter().position(|word| word == "--")?;
    let (flags, child) = (&words[..split], &words[split + 1..]);
    if child.is_empty() {
        return None;
    }
    let mut root = None;
    let mut client = None;
    let mut index = 0;
    while index < flags.len() {
        let value = flags.get(index + 1)?;
        match flags[index].as_str() {
            "--root" if root.is_none() => root = Some(PathBuf::from(value)),
            "--client" if client.is_none() => client = Some(ClientKind::parse(value)?),
            _ => return None,
        }
        index += 2;
    }
    Some(Arguments {
        root: root?,
        client,
        child: child.to_vec(),
    })
}

fn run(arguments: Arguments) -> ExitCode {
    let (sender, receiver) = mpsc::channel::<Event>();
    let poll = Arc::new(AtomicU64::new(logic::Tunables::FIRST.poll_ms));
    let pid = std::process::id();
    let host = RealHost {
        root: arguments.root.clone(),
        child_args: arguments.child.clone(),
        events: sender.clone(),
        children: HashMap::new(),
        status_path: arguments
            .root
            .join("mcp-entry")
            .join("state")
            .join(format!("entry-{pid}.json")),
        stdout: io::stdout(),
    };
    let default_exe = arguments
        .root
        .join("runtime")
        .join("actingctl.exe")
        .to_string_lossy()
        .into_owned();
    let mut entry = Core::new(
        host,
        EntryConfig {
            default_exe,
            child_args: arguments.child,
            client_kind: arguments.client,
            pid,
        },
    );
    entry.start();
    let client_events = sender.clone();
    thread::spawn(move || read_client(&client_events));
    let tick_poll = Arc::clone(&poll);
    thread::spawn(move || ticker(&sender, &tick_poll));
    serve(&mut entry, &receiver, &poll);
    entry.host.end_all();
    ExitCode::SUCCESS
}

fn serve(core: &mut Core<RealHost>, receiver: &Receiver<Event>, poll: &AtomicU64) {
    let mut ending: Option<Instant> = None;
    loop {
        let event = match ending {
            Some(deadline) => {
                match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(event) => event,
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return,
                }
            }
            None => match receiver.recv() {
                Ok(event) => event,
                Err(_) => return,
            },
        };
        match event {
            Event::Client(line) => core.client_line(&line),
            Event::ClientEnd => {
                core.client_closed();
                ending = Some(Instant::now() + SHUTDOWN_GRACE);
            }
            Event::ChildLine(child, line) => core.child_line(child, &line),
            Event::ChildEnd(child) => {
                core.child_exited(child);
                core.host.reap(child);
            }
            Event::Plan(end) => core.plan_done(end),
            Event::Tick => core.tick(),
        }
        core.host.reap_overdue();
        poll.store(core.poll_ms().max(1), Ordering::Relaxed);
        if ending.is_some() && core.current_child().is_none() {
            return;
        }
    }
}

fn read_client(events: &Sender<Event>) {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    loop {
        match read_line(&mut input) {
            Ok(Some(line)) => {
                if !line.iter().all(u8::is_ascii_whitespace)
                    && events.send(Event::Client(line)).is_err()
                {
                    return;
                }
            }
            Ok(None) => break,
            Err(error) => {
                eprintln!("ERROR actingcommand-mcp-entry: the client's input failed: {error}");
                break;
            }
        }
    }
    let _ = events.send(Event::ClientEnd);
}

/// One newline-delimited line without its line end; `None` at the end of the input.
fn read_line(input: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    if input.read_until(b'\n', &mut line)? == 0 {
        return Ok(None);
    }
    while line
        .last()
        .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
    {
        line.pop();
    }
    Ok(Some(line))
}

fn ticker(events: &Sender<Event>, poll: &AtomicU64) {
    loop {
        thread::sleep(Duration::from_millis(poll.load(Ordering::Relaxed)));
        if events.send(Event::Tick).is_err() {
            return;
        }
    }
}

struct Running {
    process: Child,
    stdin: Option<ChildStdin>,
    /// When its input was closed (review L1); it is ended past [`STOP_GRACE`].
    stopped_at: Option<Instant>,
}

struct RealHost {
    root: PathBuf,
    child_args: Vec<String>,
    events: Sender<Event>,
    children: HashMap<u64, Running>,
    status_path: PathBuf,
    stdout: io::Stdout,
}

fn command(program: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new(program);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    {
        Command::new(program)
    }
}

fn file_stamp(path: &Path) -> Option<(u64, u128)> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_nanos());
    Some((metadata.len(), modified))
}

/// Fix round 1 (review L1): whether a child stopped at `stopped_at` is due to be ended at
/// `now`, its `grace` passed. A child never stopped is due at once (the entry is ending).
fn end_due(stopped_at: Option<Instant>, now: Instant, grace: Duration) -> bool {
    stopped_at.is_none_or(|stopped| now.saturating_duration_since(stopped) >= grace)
}

/// Ends `running` and waits for it.
fn kill(running: &mut Running) {
    if let Err(error) = running.process.kill() {
        eprintln!(
            "ERROR actingcommand-mcp-entry: cannot end child pid {}: {error}",
            running.process.id()
        );
    }
    let _ = running.process.wait();
}

impl RealHost {
    /// Fix round 1 (review L1): a stopped child stays in `children` until it has ended, so
    /// [`RealHost::end_all`] still finds it; one whose output ended is removed once it exited.
    fn reap(&mut self, child: u64) {
        let exited = self.children.get_mut(&child).is_some_and(|running| {
            drop(running.stdin.take());
            if running.stopped_at.is_none() {
                running.stopped_at = Some(Instant::now());
            }
            matches!(running.process.try_wait(), Ok(Some(_)))
        });
        if exited {
            self.children.remove(&child);
        }
    }

    /// Removes the stopped children that ended and ends those past [`STOP_GRACE`].
    fn reap_overdue(&mut self) {
        let now = Instant::now();
        let mut done = Vec::new();
        for (id, running) in &mut self.children {
            if running.stopped_at.is_none() {
                continue;
            }
            if matches!(running.process.try_wait(), Ok(Some(_))) {
                done.push(*id);
            } else if end_due(running.stopped_at, now, STOP_GRACE) {
                kill(running);
                done.push(*id);
            }
        }
        for id in done {
            self.children.remove(&id);
        }
    }

    /// Ends every child still in `children`, stopped ones included: each gets what is left of
    /// its grace, then it is ended.
    fn end_all(&mut self) {
        for (_, mut running) in self.children.drain() {
            drop(running.stdin.take());
            loop {
                match running.process.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if !end_due(running.stopped_at, Instant::now(), STOP_GRACE) => {
                        thread::sleep(REAP_POLL);
                    }
                    Ok(None) | Err(_) => {
                        kill(&mut running);
                        break;
                    }
                }
            }
        }
    }
}

impl Host for RealHost {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            })
    }

    fn stamp(&self) -> Stamp {
        let runtime = self.root.join("runtime");
        Stamp {
            files: vec![
                file_stamp(&runtime.join("BUILD-MANIFEST.json")),
                file_stamp(&runtime.join("actingctl.exe")),
            ],
        }
    }

    /// Only the writer lock (fix round 1, review M1): whether an unfinished operation blocks a
    /// switch is the plan's decision (`operation_unfinished`), in the hot-switchable `actingctl`.
    fn busy(&mut self) -> bool {
        let lock = self.root.join("install").join("writer.lock");
        let Ok(file) = File::open(&lock) else {
            return false;
        };
        match file.try_lock_shared() {
            Ok(()) => false,
            Err(TryLockError::WouldBlock) => true,
            Err(TryLockError::Error(error)) => {
                eprintln!(
                    "ERROR actingcommand-mcp-entry: cannot probe {}: {error}; the switch waits",
                    lock.display()
                );
                true
            }
        }
    }

    fn spawn(&mut self, id: u64, command_line: &[String]) -> io::Result<u32> {
        let Some((program, rest)) = command_line.split_first() else {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        };
        let mut process = command(program)
            .args(rest)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let pid = process.id();
        let stdin = process.stdin.take();
        if let Some(stdout) = process.stdout.take() {
            let events = self.events.clone();
            thread::spawn(move || {
                let mut output = BufReader::new(stdout);
                while let Ok(Some(line)) = read_line(&mut output) {
                    if !line.is_empty() && events.send(Event::ChildLine(id, line)).is_err() {
                        return;
                    }
                }
                let _ = events.send(Event::ChildEnd(id));
            });
        }
        self.children.insert(
            id,
            Running {
                process,
                stdin,
                stopped_at: None,
            },
        );
        Ok(pid)
    }

    fn send(&mut self, id: u64, line: &[u8]) -> bool {
        let Some(stdin) = self
            .children
            .get_mut(&id)
            .and_then(|running| running.stdin.as_mut())
        else {
            return false;
        };
        stdin
            .write_all(line)
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .is_ok()
    }

    /// Closes the child's input; its output still reaches the entry until it ends, and it is
    /// ended if it has not within the grace.
    fn stop(&mut self, id: u64) {
        self.reap(id);
    }

    fn start_plan(&mut self, current_build: Option<&str>, timeout_ms: u64) {
        let exe = self.root.join("runtime").join("actingctl.exe");
        let mut words = vec![
            "mcp-entry".to_owned(),
            "plan".to_owned(),
            "--root".to_owned(),
            self.root.to_string_lossy().into_owned(),
            "--entry-protocol".to_owned(),
            ENTRY_PROTOCOL.to_string(),
        ];
        if let Some(build) = current_build {
            words.push("--current-build".to_owned());
            words.push(build.to_owned());
        }
        words.push("--".to_owned());
        words.extend(self.child_args.iter().cloned());
        let events = self.events.clone();
        thread::spawn(move || {
            let end = run_plan(&exe, &words, Duration::from_millis(timeout_ms));
            let _ = events.send(Event::Plan(end));
        });
    }

    fn to_client(&mut self, line: &[u8]) {
        let mut output = self.stdout.lock();
        let written = output
            .write_all(line)
            .and_then(|()| output.write_all(b"\n"))
            .and_then(|()| output.flush());
        if let Err(error) = written {
            eprintln!("ERROR actingcommand-mcp-entry: cannot write to the client: {error}");
        }
    }

    fn write_status(&mut self, status: &Value) {
        if let Err(error) = write_atomic(&self.status_path, status) {
            eprintln!(
                "ERROR actingcommand-mcp-entry: cannot write {}: {error}",
                self.status_path.display()
            );
        }
    }

    fn note(&mut self, text: &str) {
        eprintln!("actingcommand-mcp-entry: {text}");
    }
}

fn write_atomic(path: &Path, value: &Value) -> io::Result<()> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)
}

/// Runs `mcp-entry plan`; its last stdout line is its answer. A plan that does not end within
/// `timeout` is ended and reported.
fn run_plan(exe: &Path, words: &[String], timeout: Duration) -> PlanEnd {
    let spawned = command(&exe.to_string_lossy())
        .args(words)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut process = match spawned {
        Ok(process) => process,
        Err(error) => {
            return PlanEnd::Failed(json!({"program": exe, "spawn_error": error.to_string()}));
        }
    };
    let stdout = process.stdout.take().map(collect);
    let stderr = process.stderr.take().map(collect);
    let deadline = Instant::now() + timeout;
    let status = loop {
        match process.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(REAP_POLL),
            Ok(None) | Err(_) => {
                let _ = process.kill();
                let _ = process.wait();
                break None;
            }
        }
    };
    let stdout = stdout
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let stderr = stderr
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let answer = String::from_utf8_lossy(&stdout)
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .and_then(|line| serde_json::from_str::<Value>(line).ok());
    match (status, answer) {
        (Some(_), Some(answer)) => PlanEnd::Answered(answer),
        (status, _) => PlanEnd::Failed(json!({
            "program": exe,
            "exit": status.and_then(|status| status.code()),
            "timed_out": status.is_none(),
            "stderr_tail": tail(&stderr),
        })),
    }
}

fn collect(mut reader: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = reader.read_to_end(&mut bytes);
        bytes
    })
}

fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let start = text.len().saturating_sub(2_000);
    let start = (start..text.len())
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or(text.len());
    text[start..].to_owned()
}
