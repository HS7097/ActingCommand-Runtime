// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl watchdog` (Workflow #374): the Runtime watchdog of an A/B installation or, from
//! v0.12.1 (Workflow #390 WD-1), of a single-location root.
//! `run-once` is one supervision tick, run every minute by Task Scheduler through
//! `<root>\tools\actingwatch.exe`; it starts the Runtime only when it is gone without a formal
//! close. `status` computes the same decision read-only and adds attention outcomes, the task's
//! among them. `install` and `uninstall` register and remove that task.
//! Contract: `contracts/runtime-watchdog.md`. Nothing here touches the ledger.
//!
//! Workflow #378 A5 (model v2.2 §7.5, §7.6): every subcommand, and the usage exit, prints one
//! stdout line, the report `actingcommand.watchdog.report.v2`: `{schema_version, outcome,
//! attention, …data}`. The top outcome is the decision (`run-once`, `status`) or the result of
//! `install` and `uninstall`; `attention` lists status's attention outcomes (empty for the other
//! subcommands) and never changes the exit. The process exits by the top's category: 0
//! success, info or warning; 1 error (usage included); 2 fatal.

// Workflow #389 MCP-10: `decide`, `observe` and `start` are crate-visible, because `actingctl
// start` and `restart` (`crate::runtime_start`) reuse the manual start, the owner observation
// and `hold_run_lock`.
pub(crate) mod decide;
// Test-only: Workflow #381 A, test plan A-2 G3a-G3d and H-1 G2.
#[cfg(test)]
mod gate_381a;
// Test-only: Workflow #382 A-4, test plan v2.2 APPS-G12 (a whole tick on the APPS-I3 seams).
#[cfg(test)]
mod gate_382_a4;
// Test-only: Workflow #390 WD-1, the watchdog of a single-location root.
#[cfg(test)]
mod gate_wd1;
mod log;
pub(crate) mod observe;
mod powershell;
pub(crate) mod start;
mod state;
mod task;

use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{
    Category, CauseRelation, Code, Detail, IoOp, Outcome, RawSource,
};
use decide::{Decision, Journal, Observed, OwnerLock, OwnerRecord, Stage, StartProbes};
use log::Level;
use observe::{Installation, WriterProbe};
use serde_json::{Map, Value, json};
use start::TickSeams;
use state::{Loaded, Recorded, StartRecord, WatchdogState};
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, ExitStatus};
use std::time::{Duration, SystemTime};

pub(crate) const USAGE: &str = "usage: actingctl watchdog <status|run-once [--from-task]|install|uninstall> --root <install root>";
/// The report's schema (model §7.5).
pub(crate) const REPORT_SCHEMA: &str = "actingcommand.watchdog.report.v2";
const RUN_LOCK: &str = "run.lock";

/// `code` for `path`, caused by the error the OS raised for `op` (`foreign_os_error`).
pub(crate) fn io_failure(
    code: ActingctlCode,
    path: impl AsRef<Path>,
    error: &io::Error,
    op: IoOp,
) -> Outcome {
    Outcome::new(code)
        .path(path)
        .caused_by(Outcome::from_io_error(error, op))
}

/// A JSON parser's refusal: `foreign_data_error` with `raw_source` json and its message.
pub(crate) fn json_failure(error: &serde_json::Error) -> Outcome {
    Outcome::new(ContractCode::ForeignDataError)
        .raw_source(RawSource::Json)
        .raw_text(error.to_string())
}

/// A child process that failed: what it printed as the raw text of `foreign_other_error`
/// (`raw_source` process), with its exit code when it exited.
pub(crate) fn process_failure(status: ExitStatus, printed: &[u8]) -> Outcome {
    let failure = Outcome::new(ContractCode::ForeignOtherError).raw_source(RawSource::Process);
    let failure = match status.code() {
        Some(exit_code) => failure.exit_code(exit_code),
        None => failure,
    };
    failure.raw_text(printed)
}

/// An outcome inside a report member (a journal, a live owner, the task): its envelope in the
/// process's detail, which `dispatch` checked before anything ran.
fn envelope(outcome: &Outcome) -> Value {
    cli_line::outcome_value(outcome, cli_line::detail().unwrap_or(Detail::Full))
}

/// One report (`actingcommand.watchdog.report.v2`): the top outcome, status's attention
/// outcomes and the data members.
struct Report {
    outcome: Outcome,
    attention: Vec<Outcome>,
    data: Map<String, Value>,
}

impl Report {
    fn new(outcome: Outcome) -> Self {
        Self {
            outcome,
            attention: Vec::new(),
            data: Map::new(),
        }
    }

    fn with(mut self, member: &str, value: Value) -> Self {
        self.insert(member, value);
        self
    }

    fn insert(&mut self, member: &str, value: Value) {
        debug_assert!(
            !matches!(member, "schema_version" | "outcome" | "attention"),
            "a data member may not take a name of the report's own"
        );
        self.data.insert(member.to_owned(), value);
    }

    /// The exit code by the top outcome's category (model §7.6); attention never changes it.
    fn exit_code(&self) -> u8 {
        cli_line::exit_code(self.outcome.category(), false)
    }

    /// The stdout line: `schema_version`, `outcome`, `attention`, then the data members.
    fn line(&self, detail: Detail) -> Result<Value, serde_json::Error> {
        let mut line = Map::new();
        line.insert("schema_version".to_owned(), Value::from(REPORT_SCHEMA));
        line.insert(
            "outcome".to_owned(),
            serde_json::to_value(self.outcome.envelope(detail))?,
        );
        line.insert(
            "attention".to_owned(),
            Value::Array(
                self.attention
                    .iter()
                    .map(|outcome| cli_line::outcome_value(outcome, detail))
                    .collect(),
            ),
        );
        line.extend(self.data.clone());
        Ok(Value::Object(line))
    }
}

enum Subcommand {
    Status,
    RunOnce { from_task: bool },
    Install,
    Uninstall,
}

/// Runs `watchdog …` when the first argument names it; `None` leaves every other command to
/// the CLI, unchanged.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != "watchdog" {
        return None;
    }
    let detail = match cli_line::detail() {
        Ok(detail) => detail,
        Err(invalid) => return Some(print(&Report::new(invalid), Detail::Full, false, None)),
    };
    let (subcommand, root) = match parse(&arguments[1..]) {
        Ok(parsed) => parsed,
        Err(usage) => return Some(print(&Report::new(usage), detail, false, Some(USAGE))),
    };
    // Under the task, stderr goes into watchdog.log: the developer line is for a person only.
    let quiet = matches!(subcommand, Subcommand::RunOnce { from_task: true });
    let report = match subcommand {
        Subcommand::Status => status(&root),
        Subcommand::RunOnce { from_task } => run_once(&root, from_task),
        Subcommand::Install => install(&root),
        Subcommand::Uninstall => uninstall(&root),
    };
    Some(print(&report, detail, quiet, None))
}

/// Prints the report's one stdout line, then, unless `quiet`, the developer line of a warning,
/// error or fatal top (`text` is developer English, such as the usage); returns the exit code.
/// A stdout that refuses the line leaves the developer line `actingctl_output_failed`, exit 1.
fn print(report: &Report, detail: Detail, quiet: bool, text: Option<&str>) -> ExitCode {
    let written = report
        .line(detail)
        .map_err(io::Error::from)
        .and_then(|line| write_line(&line));
    if let Err(error) = written {
        return output_failed(&error);
    }
    if !quiet && report.outcome.category() > Category::Info {
        cli_line::developer_line(&report.outcome, text);
    }
    ExitCode::from(report.exit_code())
}

fn write_line(line: &Value) -> io::Result<()> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    serde_json::to_writer(&mut writer, line).map_err(io::Error::from)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn output_failed(error: &dyn fmt::Display) -> ExitCode {
    cli_line::developer_line(
        &Outcome::new(ActingctlCode::ActingctlOutputFailed),
        Some(&error.to_string()),
    );
    ExitCode::FAILURE
}

/// The subcommand and `--root`. A flag the subcommand does not take, a flag given twice and a
/// stray word are refused by name (Workflow #382 A1-FLAG); any other usage error unnamed.
fn parse(arguments: &[OsString]) -> Result<(Subcommand, PathBuf), Outcome> {
    let usage = || Outcome::new(ActingctlCode::ActingctlUsageInvalid);
    let mut subcommand = match arguments.first().and_then(|word| word.to_str()) {
        Some("status") => Subcommand::Status,
        Some("run-once") => Subcommand::RunOnce { from_task: false },
        Some("install") => Subcommand::Install,
        Some("uninstall") => Subcommand::Uninstall,
        _ => return Err(usage()),
    };
    let mut root = None;
    let mut index = 1;
    while index < arguments.len() {
        let word = arguments[index].to_str().ok_or_else(usage)?;
        let repeated = || cli_line::usage_invalid(Some(word), ArgumentProblem::Repeated);
        match (word, &mut subcommand) {
            ("--root", _) => {
                if root.is_some() {
                    return Err(repeated());
                }
                index += 1;
                root = Some(PathBuf::from(arguments.get(index).ok_or_else(usage)?));
            }
            ("--from-task", Subcommand::RunOnce { from_task }) => {
                if *from_task {
                    return Err(repeated());
                }
                *from_task = true;
            }
            _ => {
                return Err(cli_line::usage_invalid(
                    Some(word),
                    ArgumentProblem::Unknown,
                ));
            }
        }
        index += 1;
    }
    Ok((subcommand, root.ok_or_else(usage)?))
}

fn now_unix_ms() -> u64 {
    observe::unix_ms(SystemTime::now())
}

/// What one tick observed, for the decision and the report.
struct Tick {
    installation: Installation,
    lock: OwnerLock,
    journal: Journal,
    live: Option<Result<decide::LiveOwner, Outcome>>,
    terminal: Option<decide::Terminal>,
    earlier_terminal: Option<decide::Terminal>,
    runtime_info: Option<Value>,
    writer: Option<&'static str>,
    processes: Option<Vec<decide::RuntimeProcess>>,
}

impl Tick {
    fn observe(root: &Path, watchdog_dir: &Path) -> Result<Self, Outcome> {
        let installation = Installation::resolve(root)?;
        // F matters only for an unlocked journal with a record: a terminal outcome newer than
        // it, of this root.
        let observed = observe::observe_owner(
            &installation.state_root,
            &log_directories(&installation.root, watchdog_dir),
            Some(installation.root.as_path()),
        )?;
        let live = (observed.lock == OwnerLock::Locked)
            .then(|| observe::live_owner(&installation.state_root, None));
        Ok(Self {
            runtime_info: observe::runtime_info(&installation.state_root),
            installation,
            lock: observed.lock,
            journal: observed.journal,
            live,
            terminal: observed.terminal,
            earlier_terminal: observed.earlier_terminal,
            writer: None,
            processes: None,
        })
    }

    fn observed(&self) -> Observed {
        Observed {
            lock: self.lock,
            live: self.live.clone(),
            journal: self.journal.clone(),
            terminal: self.terminal.clone(),
            earlier_terminal: self.earlier_terminal.clone(),
        }
    }

    fn record(&self) -> Option<&OwnerRecord> {
        match &self.journal {
            Journal::Record { record, .. } => Some(record),
            _ => None,
        }
    }

    /// Rows 7a-7b. A free writer lock stays held shared in the returned guard (review L4).
    fn probe_start(
        &mut self,
        seams: &dyn TickSeams,
    ) -> Result<(StartProbes, Option<File>), Decision> {
        let guard =
            match observe::writer_lock(&self.installation.root).map_err(Decision::Misconfigured)? {
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
            .map_err(Decision::Misconfigured)?
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
        let probe = seams
            .host_probe(&self.installation.root_plain)
            .map_err(Decision::StartFailed)?;
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

fn watchdog_dir(root: &Path) -> Result<(PathBuf, PathBuf), Outcome> {
    let root = fs::canonicalize(root).map_err(|error| {
        io_failure(
            ActingctlCode::WatchdogRootUnavailable,
            root,
            &error,
            IoOp::Canonicalize,
        )
    })?;
    let directory = root.join("watchdog");
    Ok((root, directory))
}

fn watchdog_log(directory: &Path) -> Value {
    Value::from(
        observe::plain_path(&directory.join(log::LOG_FILE))
            .display()
            .to_string(),
    )
}

/// Workflow #389 MCP-9a (MCP-0 command table v1.1 §4.10): the `watchdog` block of `actingctl
/// overview` and `ac_overview`, read as [`status`] reads it (read-only; it writes nothing): the
/// decision, the attention outcomes, each written as `envelope` writes an outcome, the watchdog's
/// starts of the last 24 h and its last tick. A member the report does not hold is left out.
pub(crate) fn overview_block(root: &Path, envelope: &dyn Fn(&Outcome) -> Value) -> Value {
    let report = status(root);
    let mut block = Map::new();
    block.insert("decision".to_owned(), envelope(&report.outcome));
    block.insert(
        "attention".to_owned(),
        Value::Array(report.attention.iter().map(envelope).collect()),
    );
    for (member, from) in [
        ("starts_24h", "watchdog_starts_24h"),
        ("last_tick", "last_tick_unix_ms"),
    ] {
        if let Some(value) = report.data.get(from).filter(|value| !value.is_null()) {
            block.insert(member.to_owned(), value.clone());
        }
    }
    Value::Object(block)
}

/// `status`: read-only. It writes nothing and takes no lock but the momentary writer-lock
/// probe, and only when a start would be considered. Its top is the decision; its attention
/// outcomes (warnings, and the failures that kept something unread) leave the exit alone.
fn status(root: &Path) -> Report {
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
        Stage::ConsiderStart => match tick.probe_start(&start::system()) {
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
    // An owner lock that nothing answering holds for 10 minutes (an error-report dialog, a
    // start that never publishes runtime-info) needs attention like row 7b'; run-once stays 0.
    if matches!(decision, Decision::OwnerLockHeld { .. })
        && state
            .last_decision
            .as_ref()
            .is_some_and(|last| last.is(ActingctlCode::WatchdogOwnerLockHeld))
        && let Some(since) = state.decision_since_unix_ms
        && now.saturating_sub(since) >= decide::PROCESS_WITHOUT_OWNER_MS
    {
        attention.push(
            Outcome::new(ActingctlCode::WatchdogOwnerLockUnanswered)
                .elapsed_ms(now.saturating_sub(since)),
        );
    }
    // Review M2 (ii): a close no log covers may hide a FATAL of an unlogged start.
    let mut close_evidence = None;
    if let (Decision::FormalClose { owner_epoch }, Some(record)) = (&decision, tick.record()) {
        let evidence = match record.closed_at_unix_ms {
            Some(closed_at) => observe::close_logged(
                &log_directories(&root, &directory),
                record.started_at_unix_ms,
                closed_at,
            ),
            None => Ok(false),
        };
        close_evidence = Some(match evidence {
            Ok(true) => "logged",
            Ok(false) => {
                attention.push(
                    Outcome::new(ActingctlCode::WatchdogFormalCloseUnlogged)
                        .owner_epoch(owner_epoch.clone()),
                );
                "unlogged"
            }
            Err(failure) => {
                attention.push(Decision::Misconfigured(failure).outcome());
                "unreadable"
            }
        });
    }
    if !matches!(decision, Decision::Alive { .. })
        && let Some(last) = state.last_start()
        && !last.start_result.is(ActingctlCode::WatchdogStartedRuntime)
        && !last.start_result.is(ActingctlCode::WatchdogStartPending)
    {
        attention.push(
            Outcome::new(ActingctlCode::WatchdogLastStartFailed)
                .at_unix_ms(last.at_unix_ms)
                .caused_by(last.start_result.outcome()),
        );
    }
    let starts_24h = decide::starts_in_day(&state, now);
    if starts_24h >= decide::REPEATED_RESTARTS {
        attention.push(Outcome::new(ActingctlCode::WatchdogRestartsRepeated).count(starts_24h));
    }
    let task_section = task_report(&tick.installation.root_plain, &mut attention);
    let mut report = report(&tick, &decision, &state, &directory);
    report.attention = attention;
    report.insert("close_evidence", json!(close_evidence));
    report.insert("watchdog_starts_24h", json!(starts_24h));
    report.insert("task", task_section);
    match loaded {
        Loaded::Fresh => report.insert("state", json!("absent")),
        Loaded::Existing => report.insert("state", json!("present")),
        Loaded::OtherSchema(schema) => {
            report.insert("state", json!("other_schema"));
            report.insert("state_schema", json!(schema));
        }
    }
    report
}

/// `run-once`: one supervision tick (`contracts/runtime-watchdog.md`, "Decision").
fn run_once(root: &Path, from_task: bool) -> Report {
    run_once_with(&start::system(), root, from_task)
}

/// One tick on `seams` (Workflow #382 A-4, test plan v2.2 APPS-I3).
fn run_once_with(seams: &dyn TickSeams, root: &Path, from_task: bool) -> Report {
    let now = seams.now_unix_ms();
    let (root, directory) = match watchdog_dir(root) {
        Ok(paths) => paths,
        Err(failure) => return misconfigured_report(None, failure, true, from_task),
    };
    // Review M6: the directory is created on demand; `install` is not a precondition.
    if let Err(error) = fs::create_dir_all(&directory) {
        let failure = io_failure(
            ActingctlCode::WatchdogDirectoryUnavailable,
            &directory,
            &error,
            IoOp::CreateDir,
        );
        return misconfigured_report(None, failure, true, from_task);
    }
    let lock_path = directory.join(RUN_LOCK);
    let lock_failed = |error: &io::Error, op: IoOp| {
        io_failure(ActingctlCode::WatchdogRunLockFailed, &lock_path, error, op)
    };
    let run_lock = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
    {
        Ok(file) => file,
        Err(error) => {
            let failure = lock_failed(&error, IoOp::Open);
            return misconfigured_report(Some(&directory), failure, true, from_task);
        }
    };
    match run_lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Report::new(Decision::RunInProgress.outcome());
        }
        Err(std::fs::TryLockError::Error(error)) => {
            let failure = lock_failed(&error, IoOp::Lock);
            return misconfigured_report(Some(&directory), failure, true, from_task);
        }
    }
    let plain_root = observe::plain_path(&root).display().to_string();
    let mut state = match state::load(&directory, &plain_root) {
        // Another schema version (the version 1 included) is set aside; the budget restarts.
        Ok((state, Loaded::OtherSchema(schema))) => {
            if let Err(failure) = state::set_aside(&directory, now).and_then(|aside| {
                log::append(
                    &directory,
                    now,
                    Level::Warn,
                    ActingctlCode::WatchdogStateReset,
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
    let previous = state.last_decision.clone();
    state.root = plain_root;
    state.last_tick_unix_ms = Some(now);
    let (decision, tick, logged) = tick_once(seams, &root, &directory, &mut state, now, from_task);
    if let Err(failure) = record_decision(&directory, &mut state, &decision, previous, logged, now)
    {
        return misconfigured_report(Some(&directory), failure, true, from_task);
    }
    if from_task {
        state.last_task_tick_end_unix_ms = Some(seams.now_unix_ms());
    }
    if let Err(failure) = state::save(&directory, &state) {
        return misconfigured_report(Some(&directory), failure, true, from_task);
    }
    drop(run_lock);
    match &tick {
        Some(tick) => report(tick, &decision, &state, &directory),
        None => Report::new(decision.outcome()).with("log", watchdog_log(&directory)),
    }
}

/// The decision of one tick, the start included. The bool says whether the start already
/// wrote its log line.
fn tick_once(
    seams: &dyn TickSeams,
    root: &Path,
    directory: &Path,
    state: &mut WatchdogState,
    now: u64,
    from_task: bool,
) -> (Decision, Option<Tick>, bool) {
    let mut tick = match Tick::observe(root, directory) {
        Ok(tick) => tick,
        Err(failure) => return (Decision::Misconfigured(failure), None, false),
    };
    let decision = match decide::decide_observed(&tick.observed(), state) {
        Stage::Decided(decision) => decision,
        Stage::ConsiderStart => {
            let (probes, guard) = match tick.probe_start(seams) {
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
            let decision = match decide::decide_start(&probes, state, now, from_task) {
                // Workflow #378 A6a (model section 7.5): the terminal outcomes of the window's
                // starts decide between a run-out budget and `panic_unrecovered`.
                Decision::BudgetExhausted { since_unix_ms, .. } => {
                    state.exhausted_since_unix_ms = Some(since_unix_ms);
                    // The starts that ran the budget out: those of the window that ended when it
                    // ran out, also on the ticks of a sticky exhaustion.
                    decide::budget_decision(
                        since_unix_ms,
                        &observe::start_terminals(state, since_unix_ms),
                    )
                }
                decision => decision,
            };
            if decision == Decision::Start {
                let started = start_runtime(seams, &tick, directory, state, now, from_task);
                drop(guard);
                return match started {
                    Ok(decision) => (decision, Some(tick), true),
                    Err(failure) => (Decision::Misconfigured(failure), Some(tick), false),
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

/// Row 7f: the start record is saved before the spawn, so a crash mid-start still counts. Its
/// result replaces `watchdog_start_pending`: `watchdog_started_runtime` or the start's failure,
/// each with its values (state.json v2).
fn start_runtime(
    seams: &dyn TickSeams,
    tick: &Tick,
    directory: &Path,
    state: &mut WatchdogState,
    now: u64,
    from_task: bool,
) -> Result<Decision, Outcome> {
    let installation = &tick.installation;
    let start_log = directory.join(format!("actingd-{now}.log"));
    let log_plain = observe::plain_path(&start_log).display().to_string();
    state.push_start(StartRecord {
        at_unix_ms: now,
        method: if from_task { "breakaway" } else { "wmi" }.to_owned(),
        log: log_plain.clone(),
        // A single-location root has no generation (Workflow #390 WD-1).
        generation: installation.generation(),
        ..StartRecord::default()
    });
    state::save(directory, state)?;
    let attempt = start::start(seams, installation, &start_log, now, from_task);
    let finished = seams.now_unix_ms();
    // The record saved above; were it gone, the state is not what this tick wrote.
    let record = state
        .starts
        .last_mut()
        .filter(|record| record.at_unix_ms == now)
        .ok_or_else(|| {
            Outcome::new(ActingctlCode::WatchdogStateUnwritable)
                .path(directory.join(state::STATE_FILE))
        })?;
    record.method = attempt.method.to_owned();
    let previous = tick.record();
    let previous_epoch = previous.map_or_else(String::new, |record| record.owner_epoch.clone());
    let previous_pid = previous.map_or_else(String::new, |record| record.pid.to_string());
    let (level, code, decision, last_line) = match attempt.result {
        Ok(owner) => {
            record.actingd_pid = Some(owner.pid);
            record.owner_epoch = Some(owner.owner_epoch.clone());
            let decision = Decision::Started {
                method: attempt.method.to_owned(),
                owner,
                log: log_plain,
            };
            record.start_result = Recorded::of(&decision.outcome());
            (
                Level::Warn,
                ActingctlCode::WatchdogStartedRuntime,
                decision,
                None,
            )
        }
        Err(failure) => {
            record.start_result = Recorded::of(&failure);
            (
                Level::Error,
                ActingctlCode::WatchdogStartFailed,
                Decision::StartFailed(failure),
                Some(start::last_log_line(&start_log)),
            )
        }
    };
    let outcome = decision.outcome();
    let mut fields = Vec::new();
    if let Some(generation) = installation.generation() {
        fields.push(("generation", generation.to_string()));
    }
    fields.push(("method", attempt.method.to_owned()));
    fields.extend(log::outcome_fields(&outcome));
    fields.push(("previous_epoch", previous_epoch));
    fields.push(("previous_pid", previous_pid));
    match last_line {
        Some(line) => fields.push(("last_line", line)),
        None => fields.push(("ready_ms", finished.saturating_sub(now).to_string())),
    }
    log::append(directory, finished, level, code, &fields)?;
    Ok(decision)
}

/// Log on change: one line when the decision's code or values differ from the last tick's.
fn record_decision(
    directory: &Path,
    state: &mut WatchdogState,
    decision: &Decision,
    previous: Option<Recorded>,
    logged: bool,
    now: u64,
) -> Result<(), Outcome> {
    let outcome = decision.outcome();
    let current = Recorded::of(&outcome);
    if previous.as_ref() == Some(&current) && !logged {
        return Ok(());
    }
    state.last_decision = Some(current);
    state.decision_since_unix_ms = Some(now);
    if logged {
        return Ok(());
    }
    let after_start = previous
        .as_ref()
        .is_some_and(|previous| previous.is(ActingctlCode::WatchdogStartedRuntime));
    let (level, code, text): (Level, Code, Vec<(&str, String)>) = match decision {
        Decision::RunInProgress | Decision::Start | Decision::Started { .. } => return Ok(()),
        Decision::StartFailed(_) => (
            Level::Error,
            ActingctlCode::WatchdogStartFailed.code(),
            Vec::new(),
        ),
        Decision::Misconfigured(_) => (
            Level::Error,
            ActingctlCode::WatchdogMisconfigured.code(),
            Vec::new(),
        ),
        Decision::Alive {
            started_by_watchdog: false,
            ..
        } => (
            Level::Info,
            ActingctlCode::WatchdogFormalStartObserved.code(),
            Vec::new(),
        ),
        Decision::Alive { .. } if after_start => return Ok(()),
        Decision::Alive { .. } => (
            Level::Info,
            ActingctlCode::WatchdogAlive.code(),
            vec![("started_by_watchdog", "true".to_owned())],
        ),
        Decision::OwnerLockHeld { .. } => (
            Level::Info,
            ActingctlCode::WatchdogOwnerLockHeld.code(),
            Vec::new(),
        ),
        Decision::NeverStarted => (
            Level::Info,
            ActingctlCode::WatchdogNeverStarted.code(),
            Vec::new(),
        ),
        Decision::OwnerRetainedUnconfirmed { .. } => (
            Level::Error,
            ActingctlCode::WatchdogOwnerRetainedUnconfirmed.code(),
            vec![(
                "next",
                "start formally; startup releases a retained owner whose process has exited"
                    .to_owned(),
            )],
        ),
        Decision::FatalHold(terminal) => (
            Level::Error,
            ActingctlCode::WatchdogFatalHold.code(),
            vec![("terminal", terminal.outcome.code().as_str().to_owned())],
        ),
        Decision::FormalClose { .. } => (
            Level::Info,
            ActingctlCode::WatchdogFormalClose.code(),
            Vec::new(),
        ),
        Decision::InstallerBusy => (
            Level::Info,
            ActingctlCode::WatchdogInstallerBusy.code(),
            Vec::new(),
        ),
        Decision::SelectionChanged => (
            Level::Info,
            ActingctlCode::WatchdogSelectionChanged.code(),
            Vec::new(),
        ),
        Decision::RuntimeProcessPresent { processes } => (
            Level::Info,
            ActingctlCode::WatchdogRuntimeProcessPresent.code(),
            vec![("pids", pids(processes))],
        ),
        Decision::RuntimeProcessWithoutOwner { processes } => (
            Level::Error,
            ActingctlCode::WatchdogRuntimeProcessWithoutOwner.code(),
            vec![("pids", pids(processes))],
        ),
        Decision::BudgetExhausted { .. } => (
            Level::Error,
            ActingctlCode::WatchdogBudgetExhausted.code(),
            vec![("starts", state.starts.len().to_string())],
        ),
        // Workflow #378 A6a: every start in the window ended in `runtime_restart_required`.
        Decision::PanicUnrecovered { .. } => (
            Level::Error,
            ContractCode::PanicUnrecovered.code(),
            vec![("starts", state.starts.len().to_string())],
        ),
        Decision::Grace { .. } => (Level::Info, ActingctlCode::WatchdogGrace.code(), Vec::new()),
    };
    let mut fields = log::outcome_fields(&outcome);
    fields.extend(text);
    log::append(directory, now, level, code, &fields)
}

fn pids(processes: &[decide::RuntimeProcess]) -> String {
    processes
        .iter()
        .map(|process| process.pid.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn processes_json(processes: &[decide::RuntimeProcess]) -> Value {
    Value::Array(
        processes
            .iter()
            .map(|process| json!({ "pid": process.pid, "path": process.path }))
            .collect(),
    )
}

/// The report of a tick: the decision on top and what the tick observed as data.
fn report(tick: &Tick, decision: &Decision, state: &WatchdogState, directory: &Path) -> Report {
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
        Journal::Unrecognised { failure } => json!({ "unrecognised": envelope(failure) }),
        Journal::Absent => Value::Null,
    };
    let live_owner = match &tick.live {
        Some(Ok(owner)) => json!({
            "pid": owner.pid,
            "owner_epoch": owner.owner_epoch,
            "started_at_unix_ms": owner.started_at_unix_ms,
            "started_by_watchdog": decide::started_by_watchdog(owner, &state.starts),
        }),
        Some(Err(failure)) => json!({ "error": envelope(failure) }),
        None => Value::Null,
    };
    // A fatal hold names its log and terminal outcome: the newest, or (R5b) an earlier
    // owner's (Workflow #378 A6a: it was the FATAL line).
    let terminal = match decision {
        Decision::FatalHold(terminal) => Some(terminal),
        _ => tick.terminal.as_ref(),
    };
    let now = now_unix_ms();
    let data = json!({
        "root": installation.root_plain.display().to_string(),
        // Workflow #390 WD-1: `ab` or `single`, and the config the fixed entry gets; a
        // single-location root has no generation or slot.
        "layout": installation.layout_name(),
        "config": installation.config_plain.display().to_string(),
        "generation": installation.generation(),
        "slot": installation.slot(),
        "state_root": installation.state_root.display().to_string(),
        "owner_lock": tick.lock.as_str(),
        "journal": journal,
        "runtime_info": tick.runtime_info,
        "live_owner": live_owner,
        "terminal": terminal.map(|terminal| json!({
            "log": terminal.log,
            "outcome": envelope(&terminal.outcome),
        })),
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
    });
    let mut report = Report::new(decision.outcome());
    report.data = cli_line::members(data, "data");
    report.insert("log", watchdog_log(directory));
    report
}

/// A misconfiguration found before the state could be read or written: the decision
/// `watchdog_misconfigured` with the failure as its link. `run-once` writes the line unless the
/// log's last line already names the same failure; `status` writes nothing.
fn misconfigured_report(
    directory: Option<&Path>,
    failure: Outcome,
    write: bool,
    quiet: bool,
) -> Report {
    let failure_code = decide::registered_code(&failure);
    let outcome = Decision::Misconfigured(failure).outcome();
    // Under the task (`quiet`) stderr also lands in watchdog.log: it repeats nothing the log
    // already holds.
    let mut in_log = false;
    let log = match directory {
        Some(directory) => {
            if write {
                if failure_code.is_some_and(|failure_code| {
                    log::last_line_matches(
                        directory,
                        ActingctlCode::WatchdogMisconfigured.code(),
                        failure_code,
                    )
                }) {
                    in_log = true;
                } else {
                    match log::append(
                        directory,
                        now_unix_ms(),
                        Level::Error,
                        ActingctlCode::WatchdogMisconfigured,
                        &log::outcome_fields(&outcome),
                    ) {
                        Ok(()) => in_log = true,
                        Err(error) => cli_line::developer_line(&error, None),
                    }
                }
            }
            watchdog_log(directory)
        }
        None => Value::Null,
    };
    if quiet && !in_log {
        cli_line::developer_line(&outcome, None);
    }
    Report::new(outcome).with("log", log)
}

/// `status`'s task section (read-only, through the ScheduledTasks module). A missing task is
/// the attention `watchdog_task_missing`; a disabled or different definition is
/// `watchdog_task_mismatch` with the `names` of what differs; a query that failed is
/// `watchdog_misconfigured` with the failure, which the section also carries as `error`.
fn task_report(root_plain: &Path, attention: &mut Vec<Outcome>) -> Value {
    let name = task::task_name(root_plain);
    let registration = match task::query(&name) {
        Ok((registration, _)) => registration,
        Err(failure) => {
            let section = json!({ "task_name": name, "error": envelope(&failure) });
            attention.push(Decision::Misconfigured(failure).outcome());
            return section;
        }
    };
    match registration {
        task::Registration::Absent => {
            attention.push(Outcome::new(ActingctlCode::WatchdogTaskMissing).name(&name));
            json!({ "task_name": name, "registered": false })
        }
        task::Registration::Present(definition, run) => {
            let mismatches = task::mismatches(&definition, root_plain);
            if !mismatches.is_empty() {
                attention.push(
                    Outcome::new(ActingctlCode::WatchdogTaskMismatch)
                        .name(&name)
                        .names(mismatches.iter().copied()),
                );
            }
            json!({
                "task_name": name,
                "registered": true,
                "command": definition.command,
                "interval": definition.interval,
                "logon_type": definition.logon_type,
                "run_level": definition.run_level.clone().unwrap_or_else(|| task::RUN_LEVEL.to_owned()),
                "enabled": definition.enabled,
                "mismatches": mismatches,
                "status": run.state,
                "last_run_time": run.last_run_time,
                "last_result": run.last_result,
                "next_run_time": run.next_run_time,
            })
        }
    }
}

/// `install`: registers or refreshes the task (`schtasks /Create /XML /F`), then checks what
/// Task Scheduler holds. Its top is `watchdog_installed` (`name`) or the failure.
fn install(root: &Path) -> Report {
    let now = now_unix_ms();
    let installation = match Installation::resolve(root) {
        Ok(installation) => installation,
        Err(failure) => return install_failed(None, failure),
    };
    let root_plain = installation.root_plain.clone();
    let launcher = task::launcher(&root_plain);
    let entry = root_plain.join("runtime").join("actingctl.exe");
    for program in [&entry, &launcher] {
        // Deploy the Runtime and Tools that carry the watchdog first.
        if !program.is_file() {
            let failure = Outcome::new(ActingctlCode::WatchdogLauncherMissing).path(program);
            return install_failed(None, failure);
        }
    }
    match observe::writer_lock(&installation.root) {
        Ok(WriterProbe::Free(guard)) => drop(guard),
        // acsetup holds install\writer.lock: install the watchdog after it finishes.
        Ok(WriterProbe::Busy) => return install_failed(None, Decision::InstallerBusy.outcome()),
        Err(failure) => return install_failed(None, failure),
    }
    let directory = installation.root.join("watchdog");
    if let Err(error) = fs::create_dir_all(&directory) {
        let failure = io_failure(
            ActingctlCode::WatchdogDirectoryUnavailable,
            &directory,
            &error,
            IoOp::CreateDir,
        );
        return install_failed(None, failure);
    }
    // A tick in flight would write state.json behind this rewrite: wait for it, briefly.
    let run_lock = match hold_run_lock(&directory, Duration::from_secs(30)) {
        Ok(run_lock) => run_lock,
        Err(failure) => return install_failed(Some(&directory), failure),
    };
    let installed = install_task(&directory, &root_plain, now);
    drop(run_lock);
    match installed {
        Ok(report) => report,
        Err(failure) => install_failed(Some(&directory), failure),
    }
}

/// `install` under the run lock: a fresh or kept state, the definition, its registration and
/// the check of what Task Scheduler holds.
fn install_task(directory: &Path, root_plain: &Path, now: u64) -> Result<Report, Outcome> {
    let plain = root_plain.display().to_string();
    let mut state = match state::load(directory, &plain) {
        Ok((state, Loaded::Existing | Loaded::Fresh)) => state,
        Ok((state, Loaded::OtherSchema(schema))) => {
            let aside = state::set_aside(directory, now)?;
            log::append(
                directory,
                now,
                Level::Warn,
                ActingctlCode::WatchdogStateReset,
                &[
                    ("schema_version", schema),
                    ("set_aside", aside.display().to_string()),
                ],
            )?;
            state
        }
        // An unreadable state is moved aside: install is an operator's fresh start.
        Err(unreadable) => {
            let from = directory.join(state::STATE_FILE);
            let to = directory.join(format!("{}.unreadable-{now}", state::STATE_FILE));
            state::move_aside(&from, &to)?;
            log::append(
                directory,
                now,
                Level::Warn,
                ActingctlCode::WatchdogStateReset,
                &[
                    ("failure_code", unreadable.code().to_string()),
                    ("set_aside", to.display().to_string()),
                ],
            )?;
            WatchdogState::fresh(&plain)
        }
    };
    // Review L5: the first task tick after an install opens no gap grace.
    state.last_task_tick_end_unix_ms = Some(now);
    state::save(directory, &state)?;
    let user = task::current_user_sid()?;
    let name = task::task_name(root_plain);
    let xml = directory.join(task::TASK_XML);
    let start_boundary = format!("{}Z", &log::rfc3339_utc(now)[..19]);
    fs::write(
        &xml,
        task::utf16le_with_bom(&task::definition_xml(root_plain, &user, &start_boundary)),
    )
    .map_err(|error| {
        io_failure(
            ActingctlCode::WatchdogTaskRegisterFailed,
            &xml,
            &error,
            IoOp::Write,
        )
        .name(&name)
    })?;
    task::register(&name, &observe::plain_path(&xml))?;
    let definition = match task::query(&name)?.0 {
        task::Registration::Present(definition, _) => definition,
        // Task Scheduler holds no task of that name after schtasks /Create.
        task::Registration::Absent => {
            return Err(Outcome::new(ActingctlCode::WatchdogTaskMismatch).name(&name));
        }
    };
    let mismatches = task::mismatches(&definition, root_plain);
    if !mismatches.is_empty() {
        return Err(Outcome::new(ActingctlCode::WatchdogTaskMismatch)
            .name(&name)
            .names(mismatches));
    }
    log::append(
        directory,
        now_unix_ms(),
        Level::Info,
        ActingctlCode::WatchdogInstalled,
        &[("task", name.clone()), ("user", user.clone())],
    )?;
    let data = json!({
        "task_name": name,
        "command": definition.command,
        "interval": definition.interval,
        "logon_type": definition.logon_type,
        "run_level": definition.run_level.clone().unwrap_or_else(|| task::RUN_LEVEL.to_owned()),
        "enabled": definition.enabled,
        "user": user,
        "xml": observe::plain_path(&xml).display().to_string(),
        "log": watchdog_log(directory),
    });
    let mut report = Report::new(Outcome::new(ActingctlCode::WatchdogInstalled).name(&name));
    report.data = cli_line::members(data, "data");
    Ok(report)
}

/// A failed install: the failure on top, or, when it is an info decision (the installer holds
/// its writer lock, a tick holds the run lock), `watchdog_install_refused` caused by it, so the
/// refusal exits 1. Once the watchdog directory exists the failure is also logged.
fn install_failed(directory: Option<&Path>, failure: Outcome) -> Report {
    let outcome = if failure.category() < Category::Error {
        Outcome::new(ActingctlCode::WatchdogInstallRefused).caused_by(failure)
    } else {
        failure
    };
    let log = match directory {
        Some(directory) => {
            if let Err(error) =
                log::append_outcome(directory, now_unix_ms(), Level::Error, &outcome)
            {
                cli_line::developer_line(&error, None);
            }
            watchdog_log(directory)
        }
        None => Value::Null,
    };
    Report::new(outcome).with("log", log)
}

/// `uninstall`: `schtasks /Delete /F`. Every file under `<root>\watchdog\` stays. The root
/// need not be a working installation any more. Its top is `watchdog_uninstalled` (`name`), with
/// a `schtasks /End` that did not run as a `secondary` link; otherwise the failure.
fn uninstall(root: &Path) -> Report {
    let resolved = fs::canonicalize(root).or_else(|_| std::path::absolute(root));
    let root_plain = match resolved {
        Ok(path) => observe::plain_path(&path),
        Err(error) => {
            return Report::new(io_failure(
                ActingctlCode::WatchdogRootUnavailable,
                root,
                &error,
                IoOp::Absolute,
            ));
        }
    };
    let name = task::task_name(&root_plain);
    let (registration, other_tasks) = match task::query(&name) {
        Ok(found) => found,
        Err(failure) => return Report::new(failure),
    };
    let was_registered = matches!(registration, task::Registration::Present(..));
    let mut ended = None;
    let mut end_failure = None;
    if was_registered {
        // A tick in flight is ended first; a start it already issued keeps running.
        match task::end(&name) {
            Ok(exit_status) => ended = exit_status,
            Err(failure) => end_failure = Some(failure),
        }
        if let Err(failure) = task::delete(&name) {
            return Report::new(failure);
        }
    }
    let directory = root_plain.join("watchdog");
    // The task is gone; a tick that still holds run.lock is waited for, up to its own limit.
    let tick_in_progress = if directory.is_dir() {
        match hold_run_lock(
            &directory,
            Duration::from_millis(decide::READY_TIMEOUT_MS + 20_000),
        ) {
            Ok(_) => false,
            Err(failure) if failure.is(ActingctlCode::WatchdogRunInProgress) => true,
            Err(failure) => return Report::new(failure),
        }
    } else {
        false
    };
    if was_registered
        && directory.is_dir()
        && let Err(failure) = log::append(
            &directory,
            now_unix_ms(),
            Level::Info,
            ActingctlCode::WatchdogUninstalled,
            &[("task", name.clone())],
        )
    {
        return Report::new(failure);
    }
    let outcome = Outcome::new(ActingctlCode::WatchdogUninstalled).name(&name);
    let outcome = match end_failure {
        Some(failure) => outcome.link(CauseRelation::Secondary, failure),
        None => outcome,
    };
    Report::new(outcome)
        .with("task_name", json!(name))
        .with("registered", json!(false))
        .with("was_registered", json!(was_registered))
        .with("end", json!(ended))
        .with("tick_in_progress", json!(tick_in_progress))
        .with("other_watchdog_tasks", json!(other_tasks))
}

/// `<root>\watchdog\run.lock`, held exclusively, retried until `wait` has passed. A tick that
/// still holds it then is `watchdog_run_in_progress` (info: `path`, `timeout_ms`); a lock that
/// cannot be opened or taken is `watchdog_run_lock_failed` (`path`).
pub(crate) fn hold_run_lock(directory: &Path, wait: Duration) -> Result<File, Outcome> {
    let path = directory.join(RUN_LOCK);
    let failed = |error: &io::Error, op: IoOp| {
        io_failure(ActingctlCode::WatchdogRunLockFailed, &path, error, op)
    };
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| failed(&error, IoOp::Open))?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(500));
            }
            // Run the command again once the tick has ended.
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(Outcome::new(ActingctlCode::WatchdogRunInProgress)
                    .path(&path)
                    .timeout_ms(wait));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(failed(&error, IoOp::Lock)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// An A/B root with slot A and generation 1 whose zero-instance config names
    /// `<root>\state` (the process tests' fixture layout).
    fn installation(base: &Path) -> Installation {
        let root = base.join("install");
        let generation = root.join("install").join("generations").join("1");
        for directory in [root.join("A"), generation.clone(), root.join("state")] {
            fs::create_dir_all(directory).expect("create fixture directory");
        }
        let members = br#"{"fixture":"watchdog-report"}"#;
        fs::write(root.join("A").join("MEMBERS.json"), members).expect("write members");
        let config = serde_json::to_vec(&json!({
            "schema_version": "actingcommand.actingd.config.v1",
            "state_root": root.join("state"),
            "bind_host": "127.0.0.1",
            "bind_port": 0,
            "secret_fingerprint_salt": "watchdog-report-test-salt",
            "instances": []
        }))
        .expect("config JSON");
        fs::write(generation.join("actingd.config.json"), &config).expect("write config");
        fs::write(root.join("install").join("slot-A.lock"), b"").expect("write slot lock");
        let digest = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
        let selection = json!({
            "schema_version": "actingcommand.install-selection.v1",
            "slot": "A",
            "generation": 1,
            "members": { "path": "A/MEMBERS.json", "sha256": digest(members) },
            "config": {
                "path": "install/generations/1/actingd.config.json",
                "sha256": digest(&config)
            }
        });
        fs::write(
            root.join("install").join("active.json"),
            serde_json::to_vec(&selection).expect("selection JSON"),
        )
        .expect("write selection");
        Installation::resolve(&root).expect("the fixture installation")
    }

    /// Workflow #378 A5 (model v2.2 §7.5; design item 1): the report of a status of a live owner
    /// is `actingcommand.watchdog.report.v2`: the decision's envelope as `outcome`, the
    /// attention list of envelopes, the data members (the live owner with
    /// `started_by_watchdog`), and no `decision`, `code`, `exit_code` or `detail` member. The
    /// task section itself comes from Task Scheduler and is not built here.
    #[test]
    fn report_v2_of_a_live_owner_has_its_schema_outcome_and_attention() {
        let base = tempfile::tempdir().expect("tempdir");
        let owner = decide::LiveOwner {
            pid: 4242,
            owner_epoch: "epoch_live".to_owned(),
            started_at_unix_ms: 1_800_000_000_000,
        };
        let tick = Tick {
            installation: installation(base.path()),
            lock: OwnerLock::Locked,
            journal: Journal::Absent,
            live: Some(Ok(owner.clone())),
            terminal: None,
            earlier_terminal: None,
            runtime_info: None,
            writer: None,
            processes: None,
        };
        let decision = Decision::Alive {
            owner,
            started_by_watchdog: false,
        };
        let state = WatchdogState::fresh("R");
        let directory = base.path().join("install").join("watchdog");
        let mut report = report(&tick, &decision, &state, &directory);
        report.attention.push(
            Outcome::new(ActingctlCode::WatchdogTaskMissing)
                .name("ActingCommand Runtime watchdog 000000000000"),
        );
        assert_eq!(report.exit_code(), 0);
        let line = report.line(Detail::Full).expect("the report line");

        assert_eq!(line["schema_version"], "actingcommand.watchdog.report.v2");
        assert_eq!(
            line["outcome"]["schema_version"],
            "actingcommand.outcome.v1"
        );
        assert_eq!(line["outcome"]["code"], "watchdog_alive");
        assert_eq!(line["outcome"]["category"], "info");
        assert_eq!(line["outcome"]["values"]["owner_epoch"], "epoch_live");
        assert_eq!(line["outcome"]["values"]["pid"], 4242);
        let attention = line["attention"].as_array().expect("the attention list");
        assert_eq!(attention.len(), 1);
        assert_eq!(attention[0]["code"], "watchdog_task_missing");
        assert_eq!(attention[0]["category"], "warning");
        for removed in ["decision", "code", "exit_code", "detail"] {
            assert!(line.get(removed).is_none(), "{removed}: {line}");
        }
        assert_eq!(line["live_owner"]["owner_epoch"], "epoch_live");
        assert_eq!(line["live_owner"]["started_by_watchdog"], false);
        assert_eq!(line["owner_lock"], "locked");
        assert_eq!(line["generation"], 1);
        assert_eq!(line["slot"], "A");
    }
}
