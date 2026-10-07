// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl watchdog` (Workflow #374): the Runtime watchdog of an A/B installation.
//! `run-once` is one supervision tick, run every minute by Task Scheduler through
//! `<root>\tools\actingwatch.exe`; it starts the Runtime only when it is gone without a formal
//! close. `status` computes the same decision read-only and adds attention states.
//! Contract: `contracts/runtime-watchdog.md`. Nothing here touches the ledger.

mod decide;
mod log;
mod observe;
mod powershell;
mod start;
mod state;

use decide::{Decision, Journal, Observed, OwnerLock, OwnerRecord, Stage, StartProbes};
use log::Level;
use observe::{Installation, WriterProbe};
use serde_json::{Value, json};
use state::{Loaded, StartRecord, WatchdogState};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

pub(crate) const USAGE: &str =
    "usage: actingctl watchdog <status|run-once [--from-task]> --root <install root>";
const RUN_LOCK: &str = "run.lock";

/// A misconfiguration (exit 13): the code names it, the detail says where.
#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) code: String,
    pub(crate) detail: String,
}

impl Failure {
    pub(crate) fn misconfigured(code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
        }
    }

    fn decision(self) -> Decision {
        Decision::Misconfigured {
            code: self.code,
            detail: self.detail,
        }
    }
}

enum Subcommand {
    Status,
    RunOnce { from_task: bool },
}

/// Runs `watchdog …` when the first argument names it; `None` leaves every other command to
/// the CLI, unchanged.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != "watchdog" {
        return None;
    }
    let Some((subcommand, root)) = parse(&arguments[1..]) else {
        eprintln!("FATAL actingctl watchdog: {USAGE}");
        return Some(ExitCode::FAILURE);
    };
    // Under the task, stderr goes into watchdog.log: the summary line is for a person only.
    let summary = !matches!(subcommand, Subcommand::RunOnce { from_task: true });
    let (report, exit) = match subcommand {
        Subcommand::Status => status(&root),
        Subcommand::RunOnce { from_task } => run_once(&root, from_task),
    };
    let written = serde_json::to_writer(std::io::stdout().lock(), &report)
        .map_err(|error| error.to_string())
        .and_then(|()| {
            std::io::stdout()
                .lock()
                .write_all(b"\n")
                .map_err(|error| error.to_string())
        });
    if let Err(error) = written {
        eprintln!("ERROR actingctl watchdog: cannot write the report: {error}");
    }
    if exit != 0 && summary {
        eprintln!(
            "actingctl watchdog: {} exit {exit}",
            report["code"].as_str().unwrap_or("unknown")
        );
    }
    Some(ExitCode::from(exit))
}

fn parse(arguments: &[OsString]) -> Option<(Subcommand, PathBuf)> {
    let mut subcommand = match arguments.first()?.to_str()? {
        "status" => Subcommand::Status,
        "run-once" => Subcommand::RunOnce { from_task: false },
        _ => return None,
    };
    let mut root = None;
    let mut index = 1;
    while index < arguments.len() {
        match (arguments[index].to_str()?, &mut subcommand) {
            ("--root", _) if root.is_none() => {
                index += 1;
                root = Some(PathBuf::from(arguments.get(index)?));
            }
            ("--from-task", Subcommand::RunOnce { from_task }) if !*from_task => *from_task = true,
            _ => return None,
        }
        index += 1;
    }
    Some((subcommand, root?))
}

fn now_unix_ms() -> u64 {
    observe::unix_ms(SystemTime::now())
}

/// What one tick observed, for the decision and the report.
struct Tick {
    installation: Installation,
    lock: OwnerLock,
    journal: Journal,
    live: Option<Result<decide::LiveOwner, String>>,
    fatal: Option<decide::Fatal>,
    runtime_info: Option<Value>,
    writer: Option<&'static str>,
    processes: Option<Vec<decide::RuntimeProcess>>,
}

impl Tick {
    fn observe(root: &Path, watchdog_dir: &Path) -> Result<Self, Failure> {
        let installation = Installation::resolve(root)?;
        let (lock, journal) = observe::owner_journal(&installation.state_root)?;
        let live = (lock == OwnerLock::Locked)
            .then(|| observe::live_owner(&installation.state_root, None));
        // F matters only for an unlocked journal with a record: a FATAL newer than it.
        let fatal = match (&lock, &journal) {
            (
                OwnerLock::Unlocked,
                Journal::Record {
                    modified_unix_ms, ..
                },
            ) => observe::fatal_after(
                &log_directories(&installation.root, watchdog_dir),
                modified_unix_ms.saturating_sub(decide::CLOCK_SLACK_MS),
            )?,
            _ => None,
        };
        Ok(Self {
            runtime_info: observe::runtime_info(&installation.state_root),
            installation,
            lock,
            journal,
            live,
            fatal,
            writer: None,
            processes: None,
        })
    }

    fn observed(&self) -> Observed {
        Observed {
            lock: self.lock,
            live: self.live.clone(),
            journal: self.journal.clone(),
            fatal: self.fatal.clone(),
        }
    }

    fn record(&self) -> Option<&OwnerRecord> {
        match &self.journal {
            Journal::Record { record, .. } => Some(record),
            _ => None,
        }
    }

    /// Rows 7a-7b. A free writer lock stays held shared in the returned guard (review L4).
    fn probe_start(&mut self) -> Result<(StartProbes, Option<File>), Decision> {
        let guard =
            match observe::writer_lock(&self.installation.root).map_err(Failure::decision)? {
                WriterProbe::Busy => {
                    self.writer = Some("busy");
                    return Ok((
                        StartProbes {
                            writer_busy: true,
                            selection_changed: false,
                            processes: Vec::new(),
                            boot_unix_ms: None,
                        },
                        None,
                    ));
                }
                WriterProbe::Free(guard) => {
                    self.writer = Some("free");
                    guard
                }
            };
        if !self
            .installation
            .selection_unchanged()
            .map_err(Failure::decision)?
        {
            return Ok((
                StartProbes {
                    writer_busy: false,
                    selection_changed: true,
                    processes: Vec::new(),
                    boot_unix_ms: None,
                },
                guard,
            ));
        }
        let probe = observe::host_probe(&self.installation.root_plain).map_err(|detail| {
            Decision::StartFailed {
                code: "process_probe_failed".to_owned(),
                detail,
            }
        })?;
        self.processes = Some(probe.processes.clone());
        Ok((
            StartProbes {
                writer_busy: false,
                selection_changed: false,
                processes: probe.processes,
                boot_unix_ms: probe.boot_unix_ms,
            },
            guard,
        ))
    }
}

/// Where Runtime logs are searched: the root, `<root>\watchdog` and the console's log directory.
fn log_directories(root: &Path, watchdog_dir: &Path) -> Vec<PathBuf> {
    let mut directories = vec![root.to_path_buf(), watchdog_dir.to_path_buf()];
    directories.extend(observe::console_log_directory());
    directories
}

fn watchdog_dir(root: &Path) -> Result<(PathBuf, PathBuf), Failure> {
    let root = fs::canonicalize(root).map_err(|error| {
        Failure::misconfigured(
            "watchdog_root_unavailable",
            format!("{}: {error}", root.display()),
        )
    })?;
    let directory = root.join("watchdog");
    Ok((root, directory))
}

/// `status`: read-only. It writes nothing and takes no lock but the momentary writer-lock
/// probe, and only when a start would be considered.
fn status(root: &Path) -> (Value, u8) {
    let now = now_unix_ms();
    let (root, directory) = match watchdog_dir(root) {
        Ok(paths) => paths,
        Err(failure) => return misconfigured_report(None, failure, false, false),
    };
    let plain_root = observe::plain_path(&root).display().to_string();
    let (mut state, loaded) = match state::load(&directory, &plain_root) {
        Ok(loaded) => loaded,
        Err(failure) => return misconfigured_report(Some(&directory), failure, false, false),
    };
    let mut tick = match Tick::observe(&root, &directory) {
        Ok(tick) => tick,
        Err(failure) => return misconfigured_report(Some(&directory), failure, false, false),
    };
    let decision = match decide::decide_observed(&tick.observed(), &state) {
        Stage::Decided(decision) => decision,
        Stage::ConsiderStart => match tick.probe_start() {
            Ok((probes, guard)) => {
                drop(guard);
                if let Some(until) = decide::grace_after_gap(
                    state.last_task_tick_end_unix_ms,
                    now,
                    probes.boot_unix_ms,
                ) {
                    state.grace_until_unix_ms = Some(until);
                }
                decide::decide_start(&probes, &state, now, true)
            }
            Err(decision) => decision,
        },
    };
    if let Decision::Alive {
        owner,
        started_by_watchdog: false,
    } = &decision
    {
        state.formal_start_at_unix_ms = Some(owner.started_at_unix_ms);
    }
    let mut attention = Vec::new();
    if decision.exit_code() != 0 {
        attention.push((decision.key(), decision.exit_code()));
    }
    // An owner lock that nothing answering holds for 10 minutes (an error-report dialog, a
    // start that never publishes runtime-info) is attention like row 7b'; run-once stays 0.
    if matches!(decision, Decision::OwnerLockHeld { .. })
        && state.last_decision.as_deref() == Some("owner_lock_held")
        && state
            .decision_since_unix_ms
            .is_some_and(|since| now.saturating_sub(since) >= decide::PROCESS_WITHOUT_OWNER_MS)
    {
        attention.push(("owner_lock_held_without_answer".to_owned(), 15));
    }
    // Review M2 (ii): a close no log covers may hide a FATAL of an unlogged start.
    let mut close_evidence = None;
    if let (Decision::FormalClose { .. }, Some(record)) = (&decision, tick.record()) {
        let evidence = match record.closed_at_unix_ms {
            Some(closed_at) => observe::close_logged(
                &log_directories(&root, &directory),
                record.started_at_unix_ms,
                closed_at,
            ),
            None => Ok(false),
        };
        close_evidence = Some(match evidence {
            Ok(true) => "logged".to_owned(),
            Ok(false) => {
                attention.push(("formal_close_unlogged".to_owned(), 17));
                "unlogged".to_owned()
            }
            Err(failure) => {
                attention.push((format!("misconfigured:{}", failure.code), 13));
                format!("unreadable:{}: {}", failure.code, failure.detail)
            }
        });
    }
    if !matches!(decision, Decision::Alive { .. })
        && let Some(last) = state.last_start()
        && !matches!(last.outcome.as_str(), "started" | "pending")
    {
        attention.push((format!("last_start_failed:{}", last.outcome), 12));
    }
    let starts_24h = decide::starts_in_day(&state, now);
    if starts_24h >= decide::REPEATED_RESTARTS {
        attention.push(("repeated_restarts".to_owned(), 16));
    }
    let exit = [13, 10, 11, 15, 12, 14, 16, 17]
        .into_iter()
        .find(|code| attention.iter().any(|(_, exit)| exit == code))
        .unwrap_or(0);
    let mut report = report(&tick, &decision, &state, &directory, exit);
    report["attention"] = Value::Array(
        attention
            .iter()
            .map(|(code, exit)| json!({ "code": code, "exit_code": exit }))
            .collect(),
    );
    report["close_evidence"] = json!(close_evidence);
    report["watchdog_starts_24h"] = json!(starts_24h);
    report["state"] = json!(match loaded {
        Loaded::Fresh => "absent".to_owned(),
        Loaded::Existing => "present".to_owned(),
        Loaded::OtherSchema(schema) => format!("other_schema:{schema}"),
    });
    (report, exit)
}

/// `run-once`: one supervision tick (`contracts/runtime-watchdog.md`, "Decision").
fn run_once(root: &Path, from_task: bool) -> (Value, u8) {
    let now = now_unix_ms();
    let (root, directory) = match watchdog_dir(root) {
        Ok(paths) => paths,
        Err(failure) => return misconfigured_report(None, failure, true, from_task),
    };
    // Review M6: the directory is created on demand; `install` is not a precondition.
    if let Err(error) = fs::create_dir_all(&directory) {
        let failure = Failure::misconfigured(
            "watchdog_directory_unavailable",
            format!("{}: {error}", directory.display()),
        );
        return misconfigured_report(None, failure, true, from_task);
    }
    let run_lock = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(RUN_LOCK))
    {
        Ok(file) => file,
        Err(error) => {
            let failure = Failure::misconfigured(
                "watchdog_run_lock_failed",
                format!("{}: {error}", directory.join(RUN_LOCK).display()),
            );
            return misconfigured_report(Some(&directory), failure, true, from_task);
        }
    };
    match run_lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            let decision = Decision::RunInProgress;
            return (
                json!({ "decision": decision.name(), "code": decision.key(), "exit_code": 0 }),
                0,
            );
        }
        Err(std::fs::TryLockError::Error(error)) => {
            let failure = Failure::misconfigured(
                "watchdog_run_lock_failed",
                format!("{}: {error}", directory.join(RUN_LOCK).display()),
            );
            return misconfigured_report(Some(&directory), failure, true, from_task);
        }
    }
    let plain_root = observe::plain_path(&root).display().to_string();
    let mut state = match state::load(&directory, &plain_root) {
        Ok((state, Loaded::OtherSchema(schema))) => {
            if let Err(failure) = state::set_aside(&directory, now).and_then(|aside| {
                log::append(
                    &directory,
                    now,
                    Level::Warn,
                    "watchdog_state_reset",
                    &[
                        ("schema_version", schema.clone()),
                        ("set_aside", aside.display().to_string()),
                    ],
                )
            }) {
                return misconfigured_report(Some(&directory), failure, true, from_task);
            }
            state
        }
        Ok((state, _)) => state,
        Err(failure) => return misconfigured_report(Some(&directory), failure, true, from_task),
    };
    let previous_key = state.last_decision.clone();
    state.root = plain_root;
    state.last_tick_unix_ms = Some(now);
    let (decision, tick, logged) = tick_once(&root, &directory, &mut state, now, from_task);
    if let Err(failure) =
        record_decision(&directory, &mut state, &decision, previous_key, logged, now)
    {
        return misconfigured_report(Some(&directory), failure, true, from_task);
    }
    if from_task {
        state.last_task_tick_end_unix_ms = Some(now_unix_ms());
    }
    if let Err(failure) = state::save(&directory, &state) {
        return misconfigured_report(Some(&directory), failure, true, from_task);
    }
    drop(run_lock);
    let exit = decision.exit_code();
    let report = match &tick {
        Some(tick) => report(tick, &decision, &state, &directory, exit),
        None => json!({
            "decision": decision.name(),
            "code": decision.key(),
            "exit_code": exit,
            "detail": detail(&decision),
            "log": observe::plain_path(&directory.join(log::LOG_FILE)).display().to_string(),
        }),
    };
    (report, exit)
}

/// The decision of one tick, the start included. The bool says whether the start already
/// wrote its log line.
fn tick_once(
    root: &Path,
    directory: &Path,
    state: &mut WatchdogState,
    now: u64,
    from_task: bool,
) -> (Decision, Option<Tick>, bool) {
    let mut tick = match Tick::observe(root, directory) {
        Ok(tick) => tick,
        Err(failure) => return (failure.decision(), None, false),
    };
    let decision = match decide::decide_observed(&tick.observed(), state) {
        Stage::Decided(decision) => decision,
        Stage::ConsiderStart => {
            let (probes, guard) = match tick.probe_start() {
                Ok(probes) => probes,
                Err(decision) => return (decision, Some(tick), false),
            };
            state.process_present_since_unix_ms = if probes.processes.is_empty() {
                None
            } else {
                Some(state.process_present_since_unix_ms.unwrap_or(now))
            };
            // The gap grace: a task tick after a logon, reboot or resume.
            if from_task
                && let Some(until) = decide::grace_after_gap(
                    state.last_task_tick_end_unix_ms,
                    now,
                    probes.boot_unix_ms,
                )
            {
                state.grace_until_unix_ms = Some(until);
            }
            let decision = decide::decide_start(&probes, state, now, from_task);
            if let Decision::BudgetExhausted { since_unix_ms } = decision {
                state.exhausted_since_unix_ms = Some(since_unix_ms);
            }
            if decision == Decision::Start {
                let started = start_runtime(&tick, directory, state, now, from_task);
                drop(guard);
                return match started {
                    Ok(decision) => (decision, Some(tick), true),
                    Err(failure) => (failure.decision(), Some(tick), false),
                };
            }
            drop(guard);
            decision
        }
    };
    if !matches!(decision, Decision::RuntimeProcessPresent { .. })
        && !matches!(decision, Decision::RuntimeProcessWithoutOwner { .. })
    {
        state.process_present_since_unix_ms = None;
    }
    if let Decision::Alive {
        owner,
        started_by_watchdog,
    } = &decision
    {
        if *started_by_watchdog {
            // An open start that came up late is named by its epoch from now on.
            for start in state.starts.iter_mut().filter(|start| {
                start.owner_epoch.is_none()
                    && decide::started_by_watchdog(owner, std::slice::from_ref(&**start))
            }) {
                start.owner_epoch = Some(owner.owner_epoch.clone());
                start.actingd_pid = Some(owner.pid);
            }
        } else {
            // A formal start clears the budget and sticky exhaustion (re-evaluated every tick).
            state.formal_start_at_unix_ms = Some(owner.started_at_unix_ms);
            state.exhausted_since_unix_ms = None;
        }
    }
    (decision, Some(tick), false)
}

/// Row 7f: the start record is saved before the spawn, so a crash mid-start still counts.
fn start_runtime(
    tick: &Tick,
    directory: &Path,
    state: &mut WatchdogState,
    now: u64,
    from_task: bool,
) -> Result<Decision, Failure> {
    let installation = &tick.installation;
    let log_path = directory.join(format!("actingd-{now}.log"));
    let log_plain = observe::plain_path(&log_path).display().to_string();
    state.push_start(StartRecord {
        at_unix_ms: now,
        method: if from_task { "breakaway" } else { "wmi" }.to_owned(),
        log: log_plain.clone(),
        outcome: "pending".to_owned(),
        generation: Some(installation.generation),
        ..StartRecord::default()
    });
    state::save(directory, state)?;
    let outcome = start::start(installation, &log_path, now, from_task);
    let finished = now_unix_ms();
    let record = state
        .starts
        .last_mut()
        .filter(|record| record.at_unix_ms == now)
        .ok_or_else(|| {
            Failure::misconfigured("watchdog_state_unwritable", "the start record vanished")
        })?;
    record.method = outcome.method.to_owned();
    let previous = tick.record();
    let previous_epoch = previous.map_or_else(String::new, |record| record.owner_epoch.clone());
    let previous_pid = previous.map_or_else(String::new, |record| record.pid.to_string());
    match outcome.result {
        Ok(owner) => {
            record.outcome = "started".to_owned();
            record.actingd_pid = Some(owner.pid);
            record.owner_epoch = Some(owner.owner_epoch.clone());
            log::append(
                directory,
                finished,
                Level::Warn,
                "watchdog_started_runtime",
                &[
                    ("generation", installation.generation.to_string()),
                    ("method", outcome.method.to_owned()),
                    ("pid", owner.pid.to_string()),
                    ("owner_epoch", owner.owner_epoch.clone()),
                    ("log", log_plain.clone()),
                    ("previous_epoch", previous_epoch),
                    ("previous_pid", previous_pid),
                    ("ready_ms", finished.saturating_sub(now).to_string()),
                ],
            )?;
            Ok(Decision::Started {
                method: outcome.method.to_owned(),
                owner,
                log: log_plain,
            })
        }
        Err(error) => {
            record.outcome = error.code.to_owned();
            log::append(
                directory,
                finished,
                Level::Error,
                "watchdog_start_failed",
                &[
                    ("code", error.code.to_owned()),
                    ("generation", installation.generation.to_string()),
                    ("method", outcome.method.to_owned()),
                    ("log", log_plain),
                    ("previous_epoch", previous_epoch),
                    ("previous_pid", previous_pid),
                    ("detail", error.detail.clone()),
                ],
            )?;
            Ok(Decision::StartFailed {
                code: error.code.to_owned(),
                detail: error.detail,
            })
        }
    }
}

/// Log on change: one line when the decision's key differs from the last tick's.
fn record_decision(
    directory: &Path,
    state: &mut WatchdogState,
    decision: &Decision,
    previous_key: Option<String>,
    logged: bool,
    now: u64,
) -> Result<(), Failure> {
    let key = decision.key();
    if previous_key.as_deref() == Some(key.as_str()) && !logged {
        return Ok(());
    }
    state.last_decision = Some(key);
    state.decision_since_unix_ms = Some(now);
    if logged {
        return Ok(());
    }
    let (level, code, fields): (Level, &str, Vec<(&str, String)>) = match decision {
        Decision::RunInProgress | Decision::Start | Decision::Started { .. } => return Ok(()),
        Decision::StartFailed { code, detail } => (
            Level::Error,
            "watchdog_start_failed",
            vec![("code", code.clone()), ("detail", detail.clone())],
        ),
        Decision::Misconfigured { code, detail } => (
            Level::Error,
            "watchdog_misconfigured",
            vec![("code", code.clone()), ("detail", detail.clone())],
        ),
        Decision::Alive {
            owner,
            started_by_watchdog,
        } => {
            if !started_by_watchdog {
                (
                    Level::Info,
                    "watchdog_formal_start_observed",
                    vec![
                        ("pid", owner.pid.to_string()),
                        ("owner_epoch", owner.owner_epoch.clone()),
                    ],
                )
            } else if previous_key.as_deref() == Some("started") {
                return Ok(());
            } else {
                (
                    Level::Info,
                    "watchdog_alive",
                    vec![
                        ("pid", owner.pid.to_string()),
                        ("owner_epoch", owner.owner_epoch.clone()),
                        ("started_by_watchdog", "true".to_owned()),
                    ],
                )
            }
        }
        Decision::OwnerLockHeld { detail } => (
            Level::Info,
            "watchdog_owner_lock_held",
            vec![("detail", detail.clone())],
        ),
        Decision::NeverStarted => (Level::Info, "watchdog_never_started", Vec::new()),
        Decision::OwnerRetainedUnconfirmed { owner_epoch } => (
            Level::Error,
            "watchdog_owner_retained_unconfirmed",
            vec![
                ("owner_epoch", owner_epoch.clone()),
                (
                    "next",
                    "start formally; startup releases a retained owner whose process has exited"
                        .to_owned(),
                ),
            ],
        ),
        Decision::FatalHold(fatal) => (
            Level::Error,
            "watchdog_fatal_hold",
            vec![("log", fatal.log.clone()), ("line", fatal.line.clone())],
        ),
        Decision::FormalClose { owner_epoch } => (
            Level::Info,
            "watchdog_formal_close",
            vec![("owner_epoch", owner_epoch.clone())],
        ),
        Decision::InstallerBusy => (Level::Info, "watchdog_installer_busy", Vec::new()),
        Decision::SelectionChanged => (Level::Info, "watchdog_selection_changed", Vec::new()),
        Decision::RuntimeProcessPresent { processes } => (
            Level::Info,
            "watchdog_runtime_process_present",
            vec![("pids", pids(processes))],
        ),
        Decision::RuntimeProcessWithoutOwner { processes } => (
            Level::Error,
            "watchdog_runtime_process_without_owner",
            vec![("pids", pids(processes))],
        ),
        Decision::BudgetExhausted { since_unix_ms } => (
            Level::Error,
            "watchdog_budget_exhausted",
            vec![
                ("since_unix_ms", since_unix_ms.to_string()),
                ("starts", state.starts.len().to_string()),
            ],
        ),
        Decision::Grace { until_unix_ms } => (
            Level::Info,
            "watchdog_grace",
            vec![("until_unix_ms", until_unix_ms.to_string())],
        ),
    };
    log::append(directory, now, level, code, &fields)
}

fn pids(processes: &[decide::RuntimeProcess]) -> String {
    processes
        .iter()
        .map(|process| process.pid.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn detail(decision: &Decision) -> Value {
    match decision {
        Decision::Misconfigured { code, detail } | Decision::StartFailed { code, detail } => {
            json!({ "code": code, "detail": detail })
        }
        Decision::OwnerLockHeld { detail } => json!({ "detail": detail }),
        Decision::FatalHold(fatal) => json!({ "log": fatal.log, "line": fatal.line }),
        Decision::FormalClose { owner_epoch }
        | Decision::OwnerRetainedUnconfirmed { owner_epoch } => {
            json!({ "owner_epoch": owner_epoch })
        }
        Decision::Alive {
            owner,
            started_by_watchdog,
        } => json!({
            "pid": owner.pid,
            "owner_epoch": owner.owner_epoch,
            "started_at_unix_ms": owner.started_at_unix_ms,
            "started_by_watchdog": started_by_watchdog,
        }),
        Decision::Started { method, owner, log } => json!({
            "method": method,
            "pid": owner.pid,
            "owner_epoch": owner.owner_epoch,
            "log": log,
        }),
        Decision::RuntimeProcessPresent { processes }
        | Decision::RuntimeProcessWithoutOwner { processes } => processes_json(processes),
        Decision::BudgetExhausted { since_unix_ms } => json!({ "since_unix_ms": since_unix_ms }),
        Decision::Grace { until_unix_ms } => json!({ "until_unix_ms": until_unix_ms }),
        _ => Value::Null,
    }
}

fn processes_json(processes: &[decide::RuntimeProcess]) -> Value {
    Value::Array(
        processes
            .iter()
            .map(|process| json!({ "pid": process.pid, "path": process.path }))
            .collect(),
    )
}

fn report(
    tick: &Tick,
    decision: &Decision,
    state: &WatchdogState,
    directory: &Path,
    exit: u8,
) -> Value {
    let installation = &tick.installation;
    let journal = match &tick.journal {
        Journal::Record {
            record,
            modified_unix_ms,
        } => json!({
            "schema_version": record.schema_version,
            "owner_epoch": record.owner_epoch,
            "pid": record.pid,
            "started_at_unix_ms": record.started_at_unix_ms,
            "active": record.active,
            "closed_at_unix_ms": record.closed_at_unix_ms,
            "resource_disposition": record.resource_disposition,
            "modified_unix_ms": modified_unix_ms,
        }),
        Journal::Unrecognised { detail } => json!({ "unrecognised": detail }),
        Journal::Absent => Value::Null,
    };
    let now = now_unix_ms();
    json!({
        "decision": decision.name(),
        "code": decision.key(),
        "exit_code": exit,
        "detail": detail(decision),
        "root": installation.root_plain.display().to_string(),
        "generation": installation.generation,
        "slot": installation.slot,
        "state_root": installation.state_root.display().to_string(),
        "owner_lock": tick.lock.as_str(),
        "journal": journal,
        "runtime_info": tick.runtime_info,
        "live_owner": match &tick.live {
            Some(Ok(owner)) => json!({ "pid": owner.pid, "owner_epoch": owner.owner_epoch }),
            Some(Err(error)) => json!({ "error": error }),
            None => Value::Null,
        },
        "fatal": tick.fatal.as_ref().map(|fatal| json!({ "log": fatal.log, "line": fatal.line })),
        "writer_lock": tick.writer,
        "processes": tick.processes.as_deref().map(processes_json),
        "budget": {
            "starts_30min": decide::starts_in_budget(state, now),
            "limit": decide::BUDGET_STARTS,
            "exhausted_since_unix_ms": state.exhausted_since_unix_ms,
        },
        "last_start": state.last_start(),
        "formal_start_at_unix_ms": state.formal_start_at_unix_ms,
        "grace_until_unix_ms": state.grace_until_unix_ms,
        "last_tick_unix_ms": state.last_tick_unix_ms,
        "decision_since_unix_ms": state.decision_since_unix_ms,
        "log": observe::plain_path(&directory.join(log::LOG_FILE)).display().to_string(),
    })
}

/// A misconfiguration found before the state could be read or written. `run-once` writes the
/// line unless the log's last line already says the same; `status` writes nothing.
fn misconfigured_report(
    directory: Option<&Path>,
    failure: Failure,
    write: bool,
    quiet: bool,
) -> (Value, u8) {
    let mut logged = Value::Null;
    // Under the task (`quiet`) stderr also lands in watchdog.log: it repeats nothing the log
    // already holds.
    let mut in_log = false;
    if let Some(directory) = directory {
        let fields = [
            ("code", failure.code.clone()),
            ("detail", failure.detail.clone()),
        ];
        if write {
            if log::last_line_matches(directory, "watchdog_misconfigured", &failure.code) {
                in_log = true;
            } else {
                match log::append(
                    directory,
                    now_unix_ms(),
                    Level::Error,
                    "watchdog_misconfigured",
                    &fields,
                ) {
                    Ok(()) => in_log = true,
                    Err(error) => {
                        eprintln!("ERROR actingctl watchdog: {}: {}", error.code, error.detail);
                    }
                }
            }
        }
        logged = json!(
            observe::plain_path(&directory.join(log::LOG_FILE))
                .display()
                .to_string()
        );
    }
    if !(quiet && in_log) {
        eprintln!(
            "ERROR actingctl watchdog: {}: {}",
            failure.code, failure.detail
        );
    }
    let decision = failure.decision();
    (
        json!({
            "decision": decision.name(),
            "code": decision.key(),
            "exit_code": 13,
            "detail": detail(&decision),
            "log": logged,
        }),
        13,
    )
}
