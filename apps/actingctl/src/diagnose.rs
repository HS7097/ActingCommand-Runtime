// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl diagnose` (Workflow #389 MCP-7; MCP-0 command table v1.1 §4.8), the CLI original
//! of MCP `ac_diagnose`, and the reads and the rendering both share. A diagnosis is a composite
//! read (#389 R5) of named steps: the read-only `actingd suspended` child; the recent failed and
//! open runs; one page of the errors view; the failed and open runs of the window grouped by
//! (policy task, failure code, final page) (`runs_grouped`); and the non-run failures grouped by
//! (event type, policy task, code) (`failures_grouped`). With an instance it diagnoses that
//! instance, as `ac_diagnose` did before; without one, every instance. Every Runtime read is a
//! QueryEvents read, which records nothing in the ledger; at most one Status per call names the
//! instance and the instances of the groups (§2.5).
//!
//! stdout is one line, the same JSON as the `ac_diagnose` result, with every list whole. Only
//! MCP cuts lists to its output budget (`<list>_truncated`, §2.2; the order is in `mcp::runs`).
//!
//! `actingd suspended` reads the configuration of the generation `install\active.json` selects
//! at this call (`suspension_source`): an MCP server started before a deploy reads the deployed
//! generation, not the one it started with, also when the fixed entry started it with an
//! inherited selection (the train 4 review ruling on MCP-11, applied to the stale `current`
//! digest of a suspension lift reported on 10-13).
//!
//! Workflow #378 A5: the CLI line also carries `outcome` (`runtime_diagnosis_reported`, or the
//! failure alone), and the report status (`suspended_report`) carries the outcome of a report
//! that is unavailable or failed beside its data members.
//!
//! Workflow #386 SR-6b, MCP-19b (register §1.144 M19B-4; plan-mcp19 Q4, Q8): `not_run` lists the
//! deferred rows of the latest policy evaluation (due, or waiting for a gate; not the rows blocked
//! by their own trigger), at most `scheduling.not_run.diagnose_rows` per instance, each with its
//! `instance_alias`, and `not_run_total` when one was left out; one ScheduleExplanation read and
//! one RuntimeFactSnapshot read (`crate::schedule`), neither of which records anything. A read
//! that failed, or a bound that cannot be read, is a warning (the CLI line's `warnings`, MCP's
//! warnings) and marks the diagnosis incomplete, with no `not_run`.
//!
//! #422 QR-2b (brief rd-1028/brief-qr2b-diagnose.md, investigation rd-1028/qr2-investigation.md
//! §3): on v0.12.21 a diagnosis took 27 s and failed at the client's 5 s socket timeout. It
//! listed the window's runs twice (once for `runs`, once for `runs_grouped`), read every listed
//! run's status, its errors view and `failures_grouped` pages derived a run recovery nothing
//! reads, `runs` on one instance had its own 20 s budget, and any failed page failed the whole
//! diagnosis. Now the window's runs are listed once for both sections, a run the listing shows
//! succeeded or cancelled is not read, every page asks for no run recovery, every section reads
//! under the call's deadline only, and a section the deadline cut or whose read failed answers
//! `<section>_incomplete: true` (`runs`, `errors_page`, `runs_grouped`, `failures_grouped`) with
//! the warning `call_budget_exhausted` naming it (`member`) or the read's failure, while the
//! others answer; `not_run` cut so is absent with `call_budget_exhausted` (`member` `not_run`).

use crate::child::{self, Captured, ChildFailure};
use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::events::{self, CursorBinding};
use crate::instance_selector::{self, SelectorMiss};
use crate::overview::Warning;
use crate::packs;
use crate::schedule::{self, NotRun};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{
    Category, CauseRelation, Code, CodeStr, Detail, Outcome, OutcomeEnvelope, RawSource,
};
use actingcommand_contract::{
    CommandPayload, DiagnosticOutcomePayload, EventActor, EventPayload, EventQuery, EventSource,
    EventType, InstalledProcess, InstanceId, LedgerView, MAX_RUNTIME_EVENT_QUERY_EVENTS,
    PolicyExecutionOutcome, PolicyExecutionPayload, PolicyFailureDisposition, PolicyPayload,
    ProjectedEvent, ProjectionPayload, RequestId, RuntimeControlPlaneStatus,
    RuntimeEventQueryCursor, RuntimeEventQueryPage, RuntimeEventQueryPageRequest, RuntimePayload,
    TaskId, scheduled_failure_code_str,
};
use actingcommand_runtime_client::{
    ContainedRunState, ContainedRunStatus, RunDispatch, RuntimeClient, RuntimeClientConfig,
    RuntimeClientError, WindowRun,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const USAGE: &str = "usage: actingctl diagnose --state-root <path> [--root <install root>] [--instance <alias|instance_id|ADB port>] [--task-id <task_id>] [--since-unix-ms <ms>]\nwithout --instance every instance; the errors_page next_cursor one call prints answers cursor_invalid in any later call (page on with actingctl events --view errors)";
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// The window when no since_unix_ms is given: the last 24 h.
const DEFAULT_WINDOW_MS: u64 = DAY_MS;
/// The furthest back since_unix_ms may reach.
const MAX_WINDOW_MS: u64 = 7 * DAY_MS;
/// `runs` comes from the 10 most recent runs of the window (#422 QR-2b: of the one listing
/// `runs_grouped` shares).
const RECENT_RUNS: usize = 10;
const ERRORS_PAGE_LIMIT: u16 = 20;
/// At most this many clock-slot rows are kept (#372 K3).
const CLOCK_SLOTS: usize = 10;
/// The tail of a failed subprocess's stderr kept in the report status.
const MAX_STDERR_TAIL: usize = 2048;
/// How long the CLI reads: MCP's 25 s call budget less the second it keeps back to answer, so
/// that both sides end a long read alike.
const CLI_READ_BUDGET: Duration = Duration::from_secs(24);
/// ROUT-L RL-1c fix 1 (#411 H1): what `actingd suspended` keeps back from the call's deadline to
/// answer with what it read before it is ended there.
const SUSPENDED_ANSWER_MARGIN: Duration = Duration::from_secs(2);
const ACTINGD: &str = "actingcommand-actingd.exe";
/// The run states `runs` and `runs_grouped` keep: failed, and not ended.
const FAILED_OR_OPEN: [ContainedRunState; 4] = [
    ContainedRunState::Failed,
    ContainedRunState::Admitted,
    ContainedRunState::Running,
    ContainedRunState::InterruptedUnterminated,
];
/// The record types `failures_grouped` reads (the non-run kinds of the retired watcher,
/// `watch_w2.py:55-63`): the Runtime's own failures, refused requests, and the policy's
/// executions, of which only those that paused their task count.
const FAILURE_EVENT_TYPES: [EventType; 3] = [
    EventType::RuntimeFailed,
    EventType::CommandRejected,
    EventType::PolicyExecutionRecorded,
];

/// The parameters both sides take, typed (#389 R2: each MCP argument is the CLI flag's name).
pub(crate) struct DiagnoseRequest {
    /// Alias, instance_id or ADB port; without it every instance.
    pub(crate) instance: Option<String>,
    pub(crate) task_id: Option<TaskId>,
    pub(crate) since_unix_ms: Option<u64>,
}

impl DiagnoseRequest {
    /// The window's start: since_unix_ms, else 24 h before `now_unix_ms`; at most 7 days back.
    pub(crate) fn window(&self, now_unix_ms: u64) -> Result<u64, Failure> {
        let oldest_unix_ms = now_unix_ms.saturating_sub(MAX_WINDOW_MS);
        match self.since_unix_ms {
            Some(since) if since < oldest_unix_ms => {
                Err(Failure::SinceOutOfRange { oldest_unix_ms })
            }
            Some(since) => Ok(since),
            None => Ok(now_unix_ms.saturating_sub(DEFAULT_WINDOW_MS)),
        }
    }
}

/// `task_id` as a Runtime task_id.
pub(crate) fn parse_task_id(text: &str) -> Option<TaskId> {
    serde_json::from_value::<TaskId>(json!(text)).ok()
}

/// Why a diagnosis was refused or failed.
pub(crate) enum Failure {
    /// A CLI flag is unknown, repeated, missing its value or malformed: the flag as given.
    Argument {
        argument: String,
        problem: ArgumentProblem,
    },
    SinceOutOfRange {
        oldest_unix_ms: u64,
    },
    Selector {
        miss: SelectorMiss,
        selector: String,
    },
    /// A query of the diagnosis could not be formed: the contract's refusal of it.
    QueryInvalid(Code),
    /// The next cursor of the errors page could not be formed: `cursor_encode_failed`.
    CursorEncode(Outcome),
    Clock,
    Runtime(RuntimeClientError),
}

impl Failure {
    fn argument(argument: impl AsRef<OsStr>, problem: ArgumentProblem) -> Self {
        Self::Argument {
            argument: argument.as_ref().to_string_lossy().into_owned(),
            problem,
        }
    }

    /// The failure as an outcome (Workflow #378 A5). A CLI flag failure is
    /// `actingctl_usage_invalid` with `argument` and `argument_problem` (MCP answers
    /// `arguments_invalid` for its argument failures before any read); a window start too far
    /// back is `since_out_of_range` with the oldest start allowed (`range_min`).
    pub(crate) fn outcome(&self) -> Outcome {
        match self {
            Self::Argument { argument, problem } => {
                Outcome::new(ActingctlCode::ActingctlUsageInvalid)
                    .argument(argument.as_str())
                    .argument_problem(*problem)
            }
            Self::SinceOutOfRange { oldest_unix_ms } => {
                Outcome::new(ActingctlCode::SinceOutOfRange).range_min(*oldest_unix_ms)
            }
            Self::Selector { miss, selector } => miss.outcome(selector),
            Self::QueryInvalid(cause) => {
                Outcome::new(ActingctlCode::ErrorsQueryInvalid).caused_by(Outcome::new(*cause))
            }
            Self::CursorEncode(outcome) => outcome.clone(),
            Self::Clock => Outcome::new(ActingctlCode::ClockUnavailable),
            Self::Runtime(error) => error.outcome(),
        }
    }
}

/// Now, in Unix milliseconds.
pub(crate) fn now_unix_ms() -> Result<u64, Failure> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .ok_or(Failure::Clock)
}

/// The `actingd suspended` child of one diagnosis, started before the Runtime reads so that it
/// runs beside them; or, when it cannot start, the outcome that says why.
pub(crate) struct SuspensionReport(Result<JoinHandle<Result<Captured, ChildFailure>>, Outcome>);

impl SuspensionReport {
    /// Starts `<program root>\runtime\actingcommand-actingd.exe suspended --config <config>
    /// --deadline-ms <ms>` of the generation selected at this call, found from `root` (the CLI's
    /// `--root`, or the
    /// `--root` the MCP server was given), never from a selection this process holds from an
    /// earlier moment. ROUT-L RL-1c fix 1 (#411 H1): the child reads the ledger until
    /// `--deadline-ms`, [`SUSPENDED_ANSWER_MARGIN`] before `deadline`, and answers with what it
    /// read; a child still running at `deadline` is ended there, never left running.
    pub(crate) fn start(root: Option<&Path>, deadline: Instant) -> Self {
        let (program_root, config, installation) = match suspension_source(root) {
            Ok(source) => source,
            Err(outcome) => return Self(Err(outcome)),
        };
        let read_ms = deadline
            .saturating_duration_since(Instant::now())
            .saturating_sub(SUSPENDED_ANSWER_MARGIN)
            .as_millis();
        let mut command = Command::new(program_root.join("runtime").join(ACTINGD));
        command
            .arg("suspended")
            .arg("--config")
            .arg(config)
            .arg("--deadline-ms")
            .arg(read_ms.to_string());
        child::pin_child(&mut command, installation.as_ref());
        Self(Ok(thread::spawn(move || {
            child::run_captured_ending(command, deadline)
        })))
    }

    /// #422 QR-2b gate: a report that was not started, as `start` answers when it finds no
    /// configuration (the gate reads the Runtime's sections, not the child's).
    #[cfg(test)]
    pub(crate) fn not_started(outcome: Outcome) -> Self {
        Self(Err(outcome))
    }
}

/// The program root, the configuration and the installation of the generation selected now; or
/// the outcome that says why there is none, with the outcomes `actingctl packs` answers.
/// `packs::selected_config` reads an installed process's selection again from
/// `install\active.json` as it is now (`packs::current_selection`), not the one the process holds
/// from its start, which the fixed entry `<root>\runtime\actingctl.exe` may have handed it.
/// Without an install selection the configuration is `<root>\actingd.config.json`, which the
/// child reads anew at every call and whose folder is the program root.
fn suspension_source(
    root: Option<&Path>,
) -> Result<(PathBuf, PathBuf, Option<InstalledProcess>), Outcome> {
    let (installation, config) = packs::selected_config(root).map_err(|error| error.outcome())?;
    let program_root = match &installation {
        Some(selected) => selected.program_root(),
        // A configuration without a folder.
        None => config
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| Outcome::new(ActingctlCode::InstallRootUnresolved).path(&config))?,
    };
    Ok((program_root, config, installation))
}

/// #422 QR-2b: the clock a diagnosis checks its deadline on. A diagnosis reads the system's
/// monotonic clock (`SystemClock`); a test's clock moves as its fake Runtime serves pages, so
/// that the deadline passes inside a chosen section without a sleep.
pub(crate) trait Clock {
    fn now(&self) -> Instant;
}

/// The system's monotonic clock.
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// #422 QR-2b: the call's deadline on the call's clock.
struct Budget<'a> {
    deadline: Instant,
    clock: &'a dyn Clock,
}

impl Budget<'_> {
    /// Whether the deadline has come: no Runtime read starts any more.
    fn spent(&self) -> bool {
        self.clock.now() >= self.deadline
    }

    /// The deadline runtime-client's paged reads check between their pages, on the system's
    /// clock: what the call's clock leaves of the budget, from now.
    fn until(&self) -> Instant {
        Instant::now() + self.deadline.saturating_duration_since(self.clock.now())
    }
}

/// #422 QR-2b: a section of the diagnosis that answers `<section>_incomplete: true` when its
/// reads did not finish: the call's deadline came first (the warning `call_budget_exhausted`
/// naming it, `member`), or a read failed (the client's failure as the warning).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Section {
    Runs,
    ErrorsPage,
    RunsGrouped,
    FailuresGrouped,
}

impl Section {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Runs => "runs",
            Self::ErrorsPage => "errors_page",
            Self::RunsGrouped => "runs_grouped",
            Self::FailuresGrouped => "failures_grouped",
        }
    }
}

/// The member `call_budget_exhausted` names for `not_run` when the deadline came before its
/// reads (#422 QR-2b); `not_run` is then absent, as when its reads fail (MCP-19b).
const NOT_RUN: &str = "not_run";

/// #422 QR-2b: the sections whose reads did not finish, each once, and the warnings that say
/// why.
#[derive(Default)]
struct Unfinished {
    sections: Vec<Section>,
    warnings: Vec<Warning>,
}

impl Unfinished {
    /// The deadline came before `section` was read to its end: `call_budget_exhausted` naming
    /// it (`member`), once.
    fn cut(&mut self, section: Section) {
        if !self.sections.contains(&section) {
            self.sections.push(section);
            self.warnings.push(Warning::Outcome(
                Outcome::new(ActingctlCode::CallBudgetExhausted).member(section.name()),
            ));
        }
    }

    /// The run reads end before the deadline let them finish: `runs_grouped` is cut, and `runs`
    /// too while `recent_open` (its 10 runs are not all found).
    fn cut_runs(&mut self, recent_open: bool) {
        if recent_open {
            self.cut(Section::Runs);
        }
        self.cut(Section::RunsGrouped);
    }

    /// A read that `sections` need failed: each is unfinished, and the client's failure is one
    /// warning.
    fn failed(&mut self, sections: &[Section], error: RuntimeClientError) {
        for section in sections {
            if !self.sections.contains(section) {
                self.sections.push(*section);
            }
        }
        self.warnings.push(Warning::Runtime(error));
    }

    /// `failure` of a read that `sections` need: a Runtime failure leaves them unfinished and
    /// the diagnosis goes on; any other failure (a query the contract refuses to form) fails the
    /// diagnosis.
    fn failure(&mut self, sections: &[Section], failure: Failure) -> Result<(), Failure> {
        match failure {
            Failure::Runtime(error) => {
                self.failed(sections, error);
                Ok(())
            }
            other => Err(other),
        }
    }
}

/// One diagnosis, read with `client` until `deadline` on `clock`, of the window from
/// `since_unix_ms`. `state_root` is the state root the reads go to: the report's rows describe
/// another Runtime when `actingd suspended` read another one. Also returns the errors view query
/// the errors page's cursor continues.
///
/// #422 QR-2b: every section reads only while the deadline has not come, and a section whose
/// reads the deadline cut or that failed answers `<section>_incomplete` with its warning while
/// the others answer (`Section`); only the status read that resolves `instance` fails the whole
/// diagnosis. The bounded sections come first and the run sections last, so that the window's
/// run listing, the read whose size grows with the window, cannot leave the others unread: the
/// errors page (one page), `failures_grouped`, `not_run` (two reads of the Runtime's memory),
/// then the run listing, read once for both `runs` and `runs_grouped` (`window_runs`). Every
/// QueryEvents page asks for no run recovery: no part of a diagnosis reads it.
pub(crate) fn read(
    client: &RuntimeClient,
    request: &DiagnoseRequest,
    since_unix_ms: u64,
    report: SuspensionReport,
    state_root: Option<&Path>,
    deadline: Instant,
    clock: &dyn Clock,
) -> Result<(Diagnosis, EventQuery), Failure> {
    let budget = Budget { deadline, clock };
    let task_id = request.task_id;
    let mut status = None;
    let selected = match &request.instance {
        Some(selector) => {
            let read = client.status().map_err(Failure::Runtime)?;
            let instance =
                instance_selector::select(read.instances(), selector).map_err(|miss| {
                    Failure::Selector {
                        miss,
                        selector: selector.clone(),
                    }
                })?;
            let selected = (instance.instance_alias().to_owned(), instance.instance_id());
            status = Some(read);
            Some(selected)
        }
        None => None,
    };
    let instance_id = selected.as_ref().map(|(_, instance_id)| *instance_id);
    let alias = selected.as_ref().map(|(alias, _)| alias.as_str());
    let mut unfinished = Unfinished::default();
    let query = EventQuery {
        view: Some(LedgerView::Errors),
        from_timestamp_unix_ms: Some(since_unix_ms),
        instance_id,
        task_id,
        ..EventQuery::default()
    };
    let page = errors_page(client, &query, &budget, &mut unfinished)?;
    let failures = failure_records(
        client,
        since_unix_ms,
        instance_id,
        alias,
        task_id,
        &budget,
        &mut unfinished,
    )?;
    // Workflow #386 SR-6b, MCP-19b (register §1.144 M19B-4): the deferred rows of the latest
    // policy evaluation, of the diagnosed instance or of every one, at most the configured bound
    // per instance; a failed read leaves only a warning. #422 QR-2b: read before the run
    // listing, which grows with the window, so that a long listing does not leave them unread.
    let not_run = if budget.spent() {
        unfinished.warnings.push(Warning::Outcome(
            Outcome::new(ActingctlCode::CallBudgetExhausted).member(NOT_RUN),
        ));
        None
    } else {
        schedule::not_run(
            client.schedule_explanation(alias, true),
            client.runtime_fact_snapshot(),
            &mut unfinished.warnings,
        )
    };
    let window = window_runs(
        client,
        instance_id,
        since_unix_ms,
        task_id,
        &budget,
        &mut status,
        &mut unfinished,
    )?;
    let runs = window
        .recent
        .iter()
        .filter(|run| {
            FAILED_OR_OPEN.contains(&run.state) && task_id.is_none_or(|id| run.task_id == Some(id))
        })
        .map(|run| json!(run))
        .collect::<Vec<_>>();
    let report = report.finish(
        state_root,
        selected
            .as_ref()
            .map(|(alias, instance_id)| (alias.as_str(), json!(instance_id))),
        since_unix_ms,
    );
    let diagnosis = Diagnosis {
        instance: match &selected {
            Some((alias, instance_id)) => json!({"alias": alias, "instance_id": instance_id}),
            None => Value::Null,
        },
        since: since_unix_ms,
        incomplete: !unfinished.sections.is_empty() || !report.complete,
        incomplete_sections: unfinished.sections,
        runs,
        // #422 QR-2b: an errors page that was not read is empty (`errors_page_incomplete`).
        errors: page
            .iter()
            .flat_map(|page| page.events().iter().map(events::brief_row))
            .collect(),
        error_sequences: page
            .iter()
            .flat_map(|page| page.events().iter().map(|event| event.sequence))
            .collect(),
        errors_more: page.as_ref().is_some_and(RuntimeEventQueryPage::has_more),
        errors_next: page.as_ref().and_then(|page| page.next_cursor().cloned()),
        snapshot: page
            .as_ref()
            .map_or(0, RuntimeEventQueryPage::snapshot_ledger_position),
        // A page with a task filter is not a page `events` can continue.
        continuable: task_id.is_none(),
        repeating: report.repeating,
        suspended: report.suspended,
        packs: report.packs,
        remedies: report.remedies,
        lift: report.lift,
        report_warnings: report.warnings,
        report_status: report.status,
        report_outcome: report.outcome,
        clock_slots: report.clock_slots,
        clock_slots_total: report.clock_slots_total,
        runs_grouped: group_runs(&window.grouped, status.as_ref()),
        failures_grouped: group_failures(&failures),
        not_run,
        warnings: unfinished.warnings,
    };
    Ok((diagnosis, query))
}

/// The first page of the errors view in the window, oldest first; `None` when it was not read
/// (#422 QR-2b: the deadline had come, or the read failed, each answered by `errors_page`
/// being incomplete). Its rows are all a diagnosis reads of it (`events::brief_row`), so it asks
/// for no run recovery; its cursor binds the query and profile, not that option, so `events`
/// continues it as its own page.
fn errors_page(
    client: &RuntimeClient,
    query: &EventQuery,
    budget: &Budget<'_>,
    unfinished: &mut Unfinished,
) -> Result<Option<RuntimeEventQueryPage>, Failure> {
    if budget.spent() {
        unfinished.cut(Section::ErrorsPage);
        return Ok(None);
    }
    let page_request = RuntimeEventQueryPageRequest::new(ERRORS_PAGE_LIMIT, None)
        .map_err(|error| Failure::QueryInvalid(error.code()))?
        .without_run_recovery();
    match client.query_event_page(query.clone(), events::PROFILE, page_request) {
        Ok(page) => Ok(Some(page)),
        Err(error) => {
            unfinished.failed(&[Section::ErrorsPage], error);
            Ok(None)
        }
    }
}

/// What a diagnosis keeps of the window's runs (#422 QR-2b): the 10 most recent runs first
/// recorded in the window, newest first, of which `runs` keeps the failed and open ones; and
/// every failed or open run of the window (of `task_id` when one is given), for `runs_grouped`.
struct WindowReads {
    recent: Vec<ContainedRunStatus>,
    grouped: Vec<GroupedRun>,
}

/// #422 QR-2b: the window's runs, on `instance_id` or on every instance, listed once for both
/// `runs` and `runs_grouped` (runtime-client `list_window_runs`) and examined newest first. A
/// run is read only when a section needs it:
/// - a run the listing shows settled (its one terminal `task.completed` or `task.cancelled`)
///   and begun in the window (its admission or an entry fact there) is never read: it can be
///   neither failed nor open, so `runs_grouped` does not keep it, and it takes its place among
///   the 10 most recent from the listing (a run with an entry check begun moments before the
///   window's start is counted in it, `WindowRun::opened_in_window`; it is never shown);
/// - any other run is read (its brief status, at the listing's snapshot) while the 10 most
///   recent are not all found, and after that only when `runs_grouped` can keep it (its task is
///   `task_id`'s, when one is given);
/// - of a failed or open run `runs_grouped` keeps, the dispatch intent page is read only when
///   it is scheduled (`catalog_task_id`), and its terminal record only when its code is a
///   relayed spelling the catalog does not register.
///
/// The groups name their instances by alias: when no status was read for the instance and the
/// listing holds a run that is not settled, one status read follows the listing. Every read
/// starts only before the deadline; a listing the deadline cut leaves both sections unread (a
/// partial listing may miss a newer run), a later cut leaves `runs_grouped` incomplete, and
/// `runs` too until its 10 runs are found. A failed read ends the run reads as a cut does,
/// with its failure as the warning.
fn window_runs(
    client: &RuntimeClient,
    instance_id: Option<InstanceId>,
    since_unix_ms: u64,
    task_id: Option<TaskId>,
    budget: &Budget<'_>,
    status: &mut Option<RuntimeControlPlaneStatus>,
    unfinished: &mut Unfinished,
) -> Result<WindowReads, Failure> {
    let mut reads = WindowReads {
        recent: Vec::new(),
        grouped: Vec::new(),
    };
    if budget.spent() {
        unfinished.cut_runs(true);
        return Ok(reads);
    }
    let listing = match client.list_window_runs(instance_id, since_unix_ms, budget.until()) {
        Ok(listing) => listing,
        Err(error) => {
            unfinished.failed(&[Section::Runs, Section::RunsGrouped], error);
            return Ok(reads);
        }
    };
    if !listing.complete {
        unfinished.cut_runs(true);
        return Ok(reads);
    }
    if status.is_none() && listing.runs.iter().any(|run| run.settled.is_none()) {
        if budget.spent() {
            unfinished.cut_runs(true);
            return Ok(reads);
        }
        // A failed status read leaves the groups naming instances by instance_id.
        match client.status() {
            Ok(read) => *status = Some(read),
            Err(error) => unfinished.failed(&[Section::RunsGrouped], error),
        }
    }
    let snapshot = listing.snapshot_ledger_position;
    let mut recent_found = 0_usize;
    for (index, run) in listing.runs.iter().enumerate() {
        let recent_wanted = recent_found < RECENT_RUNS;
        if run.settled.is_some() && run.opened_in_window {
            recent_found += usize::from(recent_wanted);
            continue;
        }
        let grouped_wanted =
            run.settled.is_none() && task_id.is_none_or(|id| run.identity.task_id == id);
        if !recent_wanted && !grouped_wanted {
            continue;
        }
        // `runs` is unfinished when the run reads end before its 10 runs are found.
        if budget.spent() {
            unfinished.cut_runs(recent_wanted);
            break;
        }
        let read = match client.window_run_status(run, snapshot, budget.until()) {
            Ok(Some(read)) => read,
            Ok(None) => {
                unfinished.cut_runs(recent_wanted);
                break;
            }
            Err(error) => {
                let sections: &[Section] = if recent_wanted {
                    &[Section::Runs, Section::RunsGrouped]
                } else {
                    &[Section::RunsGrouped]
                };
                unfinished.failed(sections, error);
                break;
            }
        };
        // A run first recorded before the window is not one of its runs.
        if !read
            .first_fact_unix_ms
            .is_some_and(|first| first >= since_unix_ms)
        {
            continue;
        }
        if recent_wanted {
            recent_found += 1;
            reads.recent.push(read.status.clone());
        }
        if !grouped_wanted || !FAILED_OR_OPEN.contains(&read.status.state) {
            continue;
        }
        // `runs` is complete once its 10 runs are found or no run follows this one.
        let recent_open = recent_found < RECENT_RUNS && index + 1 < listing.runs.len();
        match grouped_run(client, run, &read.status, snapshot, budget) {
            Ok(Some(grouped)) => reads.grouped.push(grouped),
            Ok(None) => {
                unfinished.cut_runs(recent_open);
                break;
            }
            Err(failure) => {
                let sections: &[Section] = if recent_open {
                    &[Section::Runs, Section::RunsGrouped]
                } else {
                    &[Section::RunsGrouped]
                };
                unfinished.failure(sections, failure)?;
                break;
            }
        }
    }
    Ok(reads)
}

/// One failed or open run as `runs_grouped` keeps it: the policy task of a scheduled run from
/// its dispatch intent, and its failure code, a relayed spelling the catalog does not register
/// read back from its terminal record; `None` when the deadline came before one of those reads.
fn grouped_run(
    client: &RuntimeClient,
    run: &WindowRun,
    status: &ContainedRunStatus,
    snapshot: u64,
    budget: &Budget<'_>,
) -> Result<Option<GroupedRun>, Failure> {
    let catalog_task_id = match status.dispatch {
        RunDispatch::Scheduled => {
            if budget.spent() {
                return Ok(None);
            }
            client
                .window_run_catalog_task_id(run, snapshot)
                .map_err(Failure::Runtime)?
        }
        _ => None,
    };
    let terminal = status.terminal.as_ref();
    let failure_code = match terminal.and_then(|terminal| terminal.failure_code.as_ref()) {
        Some(code) if code.as_str() == ContractCode::OutcomeRelayUnregistered.as_str() => {
            if budget.spent() {
                return Ok(None);
            }
            let sequence = terminal.map_or(0, |terminal| terminal.sequence);
            Some(relayed_failure_code(client, status.request_id, sequence)?)
        }
        Some(code) => Some(code.clone()),
        None => None,
    };
    Ok(Some(GroupedRun {
        catalog_task_id,
        failure_code,
        final_page: terminal.and_then(|terminal| terminal.final_page.clone()),
        sequence: run.order_sequence,
        request_id: status.request_id,
        instance: status.instance_id,
    }))
}

/// One failed or open run of `runs_grouped`.
struct GroupedRun {
    /// The policy task a scheduled run was dispatched for; a manual run has none.
    catalog_task_id: Option<String>,
    failure_code: Option<CodeStr>,
    final_page: Option<String>,
    /// Its order in the run list: its first listing fact in the window.
    sequence: u64,
    request_id: Option<RequestId>,
    instance: Option<InstanceId>,
}

/// The failure code of a failed run whose terminal relayed a spelling the catalog does not
/// register (`outcome_relay_unregistered`): that spelling, read back from the terminal record at
/// `sequence` (A3 ruling 7, `scheduled_failure_code_str`), so that distinct failures stay distinct
/// groups, as they stay distinct failure identities for the policy. The registered code when the
/// record keeps no spelling. The page's one row is all it reads, so it asks for no run recovery
/// (#422 QR-2b).
fn relayed_failure_code(
    client: &RuntimeClient,
    request_id: Option<RequestId>,
    sequence: u64,
) -> Result<CodeStr, Failure> {
    let query = EventQuery {
        from_sequence: Some(sequence),
        to_sequence: Some(sequence),
        request_id,
        ..EventQuery::default()
    };
    let page_request = RuntimeEventQueryPageRequest::new(1, None)
        .map_err(|error| Failure::QueryInvalid(error.code()))?
        .without_run_recovery();
    let page = client
        .query_event_page(query, events::PROFILE, page_request)
        .map_err(Failure::Runtime)?;
    Ok(page
        .events()
        .first()
        .and_then(|event| full_payload(event)?.outcome())
        .map_or_else(
            || CodeStr::from(ContractCode::OutcomeRelayUnregistered.code()),
            scheduled_failure_code_str,
        ))
}

/// `runs_grouped`: the runs grouped by (policy task, failure code, final page), each group
/// `{catalog_task_id?, failure_code?, final_page?, count, first_seq, last_seq,
/// latest_request_id, instances}`, the most frequent first (the most recent first among equals),
/// so that an output budget cuts the rarest first. `instances` names each run's instance by the
/// alias `status` gives it, else by its instance_id.
fn group_runs(runs: &[GroupedRun], status: Option<&RuntimeControlPlaneStatus>) -> Vec<Value> {
    struct Group {
        count: u64,
        first_seq: u64,
        last_seq: u64,
        latest_request_id: Option<RequestId>,
        instances: BTreeSet<InstanceId>,
    }
    let mut groups: BTreeMap<_, Group> = BTreeMap::new();
    for run in runs {
        let group = groups
            .entry((
                run.catalog_task_id.as_deref(),
                run.failure_code.as_ref().map(CodeStr::as_str),
                run.final_page.as_deref(),
            ))
            .or_insert(Group {
                count: 0,
                first_seq: run.sequence,
                last_seq: run.sequence,
                latest_request_id: run.request_id,
                instances: BTreeSet::new(),
            });
        group.count += 1;
        group.first_seq = group.first_seq.min(run.sequence);
        if run.sequence >= group.last_seq {
            group.last_seq = run.sequence;
            group.latest_request_id = run.request_id;
        }
        if let Some(instance_id) = run.instance {
            group.instances.insert(instance_id);
        }
    }
    let name = |instance_id: InstanceId| -> String {
        status
            .and_then(|status| {
                status
                    .instances()
                    .iter()
                    .find(|instance| instance.instance_id() == instance_id)
            })
            .map_or_else(
                || json!(instance_id).as_str().unwrap_or_default().to_owned(),
                |instance| instance.instance_alias().to_owned(),
            )
    };
    let mut rows = groups
        .into_iter()
        .map(|((catalog_task_id, failure_code, final_page), group)| {
            let mut row = Map::new();
            if let Some(catalog_task_id) = catalog_task_id {
                row.insert("catalog_task_id".to_owned(), json!(catalog_task_id));
            }
            if let Some(failure_code) = failure_code {
                row.insert("failure_code".to_owned(), json!(failure_code));
            }
            if let Some(final_page) = final_page {
                row.insert("final_page".to_owned(), json!(final_page));
            }
            row.insert("count".to_owned(), json!(group.count));
            row.insert("first_seq".to_owned(), json!(group.first_seq));
            row.insert("last_seq".to_owned(), json!(group.last_seq));
            row.insert(
                "latest_request_id".to_owned(),
                json!(group.latest_request_id),
            );
            let instances = group
                .instances
                .iter()
                .map(|instance_id| name(*instance_id))
                .collect::<BTreeSet<_>>();
            row.insert("instances".to_owned(), json!(instances));
            (group.count, group.last_seq, Value::Object(row))
        })
        .collect::<Vec<_>>();
    order_groups(&mut rows);
    rows.into_iter().map(|(_, _, row)| row).collect()
}

/// The most frequent group first and, among equals, the most recent; an output budget cuts
/// from the end.
fn order_groups(rows: &mut [(u64, u64, Value)]) {
    rows.sort_by(|left, right| right.0.cmp(&left.0).then(right.1.cmp(&left.1)));
}

/// One record of `failures_grouped`: its type, the policy task it names and its code.
struct FailureRecord {
    event_type: String,
    catalog_task_id: Option<String>,
    code: CodeStr,
    sequence: u64,
}

/// What a record of one of the three kinds says for `failures_grouped`; `None` for a policy
/// execution that did not pause its task and for any other record. Each kind's code is read by
/// its own function, from the fields ledger revision 3 gives that kind.
fn failure_record(event: &ProjectedEvent) -> Option<FailureRecord> {
    let (catalog_task_id, code) = match full_payload(event)? {
        EventPayload::Runtime(RuntimePayload::Failed(failed)) => {
            (None, runtime_failed_code(failed))
        }
        EventPayload::Command(CommandPayload::Rejected(rejected)) => {
            (None, command_rejected_code(rejected))
        }
        EventPayload::Policy(PolicyPayload::ExecutionRecorded(execution)) => {
            let (catalog_task_id, code) = paused_task(execution)?;
            (Some(catalog_task_id), code)
        }
        _ => return None,
    };
    Some(FailureRecord {
        event_type: json!(event.event_type).as_str()?.to_owned(),
        catalog_task_id,
        code,
        sequence: event.sequence,
    })
}

/// `runtime.failed`: the Runtime's own failure, coded by the record's outcome (ledger revision
/// 3); an unregistered relayed spelling is read back from it, as the policy reads a run's
/// (A3 ruling 7, `scheduled_failure_code_str`).
fn runtime_failed_code(failed: &DiagnosticOutcomePayload) -> CodeStr {
    scheduled_failure_code_str(failed.outcome())
}

/// `command.rejected`: a refused request, coded by the record's outcome as `runtime.failed` is.
fn command_rejected_code(rejected: &DiagnosticOutcomePayload) -> CodeStr {
    scheduled_failure_code_str(rejected.outcome())
}

/// `policy.execution_recorded` of a run that paused its task (disposition `paused_task`): the
/// policy task id and the failure code the policy recorded (its failure `outcome`'s code, an
/// unregistered relayed spelling read back from its `raw_code`: `scheduled_failure_code_str`).
/// `None` for any other execution.
fn paused_task(execution: &PolicyExecutionPayload) -> Option<(String, CodeStr)> {
    match execution.policy_result() {
        PolicyExecutionOutcome::Failed { failure }
            if failure.disposition == PolicyFailureDisposition::PausedTask =>
        {
            Some((
                execution.task_id().to_owned(),
                scheduled_failure_code_str(&failure.outcome),
            ))
        }
        _ => None,
    }
}

/// Whether a policy execution belongs to the diagnosed instance: by its links, or, for an
/// execution the policy recorded without run links (one it settled at a later start), by the
/// instance its record names.
fn policy_execution_on(event: &ProjectedEvent, instance_id: InstanceId, alias: &str) -> bool {
    if let Some(linked) = event.links.instance_id() {
        return *linked == instance_id;
    }
    let Some(EventPayload::Policy(PolicyPayload::ExecutionRecorded(execution))) =
        full_payload(event)
    else {
        return false;
    };
    execution.instance_id() == alias || json!(instance_id).as_str() == Some(execution.instance_id())
}

/// The records of the three kinds in the window, on `instance_id` or on every instance and of
/// `task_id` when one is given, each type read page by page until its last page. #422 QR-2b: a
/// page starts only before the deadline; a deadline that came first, or a failed page, leaves
/// `failures_grouped` incomplete with what was read.
fn failure_records(
    client: &RuntimeClient,
    since_unix_ms: u64,
    instance_id: Option<InstanceId>,
    alias: Option<&str>,
    task_id: Option<TaskId>,
    budget: &Budget<'_>,
    unfinished: &mut Unfinished,
) -> Result<Vec<FailureRecord>, Failure> {
    let mut records = Vec::new();
    for event_type in FAILURE_EVENT_TYPES {
        // A policy execution settled without run links names its instance only in its record,
        // so that type is read on every instance and kept here.
        let policy = event_type == EventType::PolicyExecutionRecorded;
        let query = EventQuery {
            event_type: Some(event_type),
            from_timestamp_unix_ms: Some(since_unix_ms),
            instance_id: instance_id.filter(|_| !policy),
            task_id,
            ..EventQuery::default()
        };
        let read = read_pages(client, query, budget, |event| {
            let on_instance = match (policy, instance_id) {
                (true, Some(instance_id)) => {
                    policy_execution_on(event, instance_id, alias.unwrap_or_default())
                }
                _ => true,
            };
            if on_instance && let Some(record) = failure_record(event) {
                records.push(record);
            }
        });
        match read {
            Ok(true) => {}
            Ok(false) => {
                unfinished.cut(Section::FailuresGrouped);
                break;
            }
            Err(failure) => {
                unfinished.failure(&[Section::FailuresGrouped], failure)?;
                break;
            }
        }
    }
    Ok(records)
}

/// Every event `query` matches, oldest first, page by page up to the ledger position of its
/// first page; false when the deadline came before the last page. The rows are all it reads,
/// so its pages ask for no run recovery (#422 QR-2b).
fn read_pages(
    client: &RuntimeClient,
    query: EventQuery,
    budget: &Budget<'_>,
    mut each: impl FnMut(&ProjectedEvent),
) -> Result<bool, Failure> {
    let mut next = query;
    loop {
        if budget.spent() {
            return Ok(false);
        }
        let page_request = RuntimeEventQueryPageRequest::new(MAX_RUNTIME_EVENT_QUERY_EVENTS, None)
            .map_err(|error| Failure::QueryInvalid(error.code()))?
            .without_run_recovery();
        let page = client
            .query_event_page(next.clone(), events::PROFILE, page_request)
            .map_err(Failure::Runtime)?;
        for event in page.events() {
            each(event);
        }
        // Where the next page starts: after the last row, or (#411 A51) after the position a
        // read the Runtime's budget cut reached, which may hold no row.
        match page.next_cursor().map(|cursor| cursor.after_sequence()) {
            Some(last) if page.has_more() => {
                next = EventQuery {
                    from_sequence: Some(last + 1),
                    to_sequence: Some(next.to_sequence.unwrap_or(page.snapshot_ledger_position())),
                    ..next
                };
            }
            _ => return Ok(true),
        }
    }
}

/// `failures_grouped`: the records grouped by (type, policy task, code), each group `{type,
/// catalog_task_id?, code, count, first_seq, last_seq}`, the most frequent first (the most
/// recent first among equals).
fn group_failures(records: &[FailureRecord]) -> Vec<Value> {
    let mut groups: BTreeMap<_, (u64, u64, u64)> = BTreeMap::new();
    for record in records {
        let (count, first, last) = groups
            .entry((
                record.event_type.as_str(),
                record.catalog_task_id.as_deref(),
                record.code.as_str(),
            ))
            .or_insert((0, record.sequence, record.sequence));
        *count += 1;
        *first = (*first).min(record.sequence);
        *last = (*last).max(record.sequence);
    }
    let mut rows = groups
        .into_iter()
        .map(
            |((event_type, catalog_task_id, code), (count, first_seq, last_seq))| {
                let mut row = Map::new();
                row.insert("type".to_owned(), json!(event_type));
                if let Some(catalog_task_id) = catalog_task_id {
                    row.insert("catalog_task_id".to_owned(), json!(catalog_task_id));
                }
                row.insert("code".to_owned(), json!(code));
                row.insert("count".to_owned(), json!(count));
                row.insert("first_seq".to_owned(), json!(first_seq));
                row.insert("last_seq".to_owned(), json!(last_seq));
                (count, last_seq, Value::Object(row))
            },
        )
        .collect::<Vec<_>>();
    order_groups(&mut rows);
    rows.into_iter().map(|(_, _, row)| row).collect()
}

fn full_payload(event: &ProjectedEvent) -> Option<&EventPayload> {
    match &event.payload {
        ProjectionPayload::Full(payload) => Some(payload.as_ref()),
        _ => None,
    }
}

/// A diagnosis's parts, each list whole; `render` keeps the first `Kept` of each.
pub(crate) struct Diagnosis {
    /// `{alias, instance_id}` of the diagnosed instance; null when every instance is.
    pub(crate) instance: Value,
    pub(crate) since: u64,
    pub(crate) incomplete: bool,
    /// #422 QR-2b: the sections whose reads did not finish, each rendered
    /// `<section>_incomplete: true` beside the diagnosis's `incomplete`.
    pub(crate) incomplete_sections: Vec<Section>,
    pub(crate) runs: Vec<Value>,
    pub(crate) errors: Vec<Value>,
    pub(crate) error_sequences: Vec<u64>,
    pub(crate) errors_more: bool,
    pub(crate) errors_next: Option<RuntimeEventQueryCursor>,
    pub(crate) snapshot: u64,
    pub(crate) continuable: bool,
    pub(crate) repeating: Vec<Value>,
    pub(crate) suspended: Vec<Value>,
    /// Workflow #386 SR-3: the report's suspended task packages that defer the instance's pairs
    /// (every one without an instance).
    pub(crate) packs: Vec<Value>,
    /// Workflow #411 B15 R7: the report's stopped remedy rounds of the instance (every one
    /// without an instance).
    pub(crate) remedies: Vec<Value>,
    pub(crate) lift: Vec<Value>,
    pub(crate) report_warnings: Vec<Value>,
    /// The report status's data members: `{status, …}`.
    pub(crate) report_status: Value,
    /// The outcome of a report that is unavailable or failed (Workflow #378 A5), rendered into
    /// the status as `outcome` in the frontend's detail.
    pub(crate) report_outcome: Option<Outcome>,
    /// At most 10 clock-slot rows (#372 K3), refused and superseded first, then deferred and
    /// expired, then owed (#372 K4).
    pub(crate) clock_slots: Vec<Value>,
    /// The clock-slot rows in the window, kept or not.
    pub(crate) clock_slots_total: usize,
    /// Workflow #389 MCP-7: the failed and open runs grouped, the most frequent first.
    pub(crate) runs_grouped: Vec<Value>,
    /// Workflow #389 MCP-7: the non-run failures grouped, the most frequent first.
    pub(crate) failures_grouped: Vec<Value>,
    /// Workflow #386 SR-6b, MCP-19b: the deferred rows of the latest policy evaluation, at most
    /// the configured bound per instance, and how many there are; `None` when they could not be
    /// read (`warnings` says why).
    pub(crate) not_run: Option<NotRun>,
    /// Workflow #386 SR-6b, MCP-19b: the parts that could not be read, reported beside the
    /// diagnosis, which is then incomplete.
    pub(crate) warnings: Vec<Warning>,
}

/// How many of each list a rendering keeps.
#[derive(Clone, Copy)]
pub(crate) struct Kept {
    pub(crate) runs: usize,
    pub(crate) errors: usize,
    pub(crate) repeating: usize,
    pub(crate) suspended: usize,
    pub(crate) packs: usize,
    pub(crate) remedies: usize,
    pub(crate) lift: usize,
    pub(crate) report_warnings: usize,
    pub(crate) clock_slots: usize,
    pub(crate) runs_grouped: usize,
    pub(crate) failures_grouped: usize,
    /// Workflow #386 SR-6b, MCP-19b.
    pub(crate) not_run: usize,
}

impl Diagnosis {
    /// Every list whole.
    pub(crate) fn kept_all(&self) -> Kept {
        Kept {
            runs: self.runs.len(),
            errors: self.errors.len(),
            repeating: self.repeating.len(),
            suspended: self.suspended.len(),
            packs: self.packs.len(),
            remedies: self.remedies.len(),
            lift: self.lift.len(),
            report_warnings: self.report_warnings.len(),
            clock_slots: self.clock_slots.len(),
            runs_grouped: self.runs_grouped.len(),
            failures_grouped: self.failures_grouped.len(),
            not_run: self
                .not_run
                .as_ref()
                .map_or(0, |not_run| not_run.rows.len()),
        }
    }

    /// The result with the first `kept` of each list and `errors_page` as the errors page;
    /// each list cut short is marked `<list>_truncated`. `envelope` renders the report status's
    /// outcome as the frontend writes outcomes (its detail).
    pub(crate) fn render(
        &self,
        kept: &Kept,
        errors_page: Value,
        envelope: &dyn Fn(&Outcome) -> Value,
    ) -> Value {
        let mut result = Map::new();
        if !self.instance.is_null() {
            result.insert("instance".to_owned(), self.instance.clone());
        }
        result.insert("window".to_owned(), json!({"since_unix_ms": self.since}));
        list_into(&mut result, "runs", &self.runs, kept.runs);
        result.insert("errors_page".to_owned(), errors_page);
        list_into(&mut result, "suspended", &self.suspended, kept.suspended);
        list_into(&mut result, "packs", &self.packs, kept.packs);
        list_into(&mut result, "remedies", &self.remedies, kept.remedies);
        list_into(&mut result, "lift", &self.lift, kept.lift);
        list_into(&mut result, "repeating", &self.repeating, kept.repeating);
        list_into(
            &mut result,
            "report_warnings",
            &self.report_warnings,
            kept.report_warnings,
        );
        list_into(
            &mut result,
            "clock_slots",
            &self.clock_slots,
            kept.clock_slots,
        );
        if kept.clock_slots < self.clock_slots_total {
            result.insert(
                "clock_slots_total".to_owned(),
                json!(self.clock_slots_total),
            );
        }
        // Workflow #386 SR-6b, MCP-19b: absent when the rows could not be read.
        if let Some(not_run) = &self.not_run {
            list_into(&mut result, "not_run", &not_run.rows, kept.not_run);
            if kept.not_run < not_run.total {
                result.insert("not_run_total".to_owned(), json!(not_run.total));
            }
        }
        let mut report_status = self.report_status.clone();
        if let (Some(outcome), Value::Object(status)) = (&self.report_outcome, &mut report_status) {
            status.insert("outcome".to_owned(), envelope(outcome));
        }
        result.insert("suspended_report".to_owned(), report_status);
        list_into(
            &mut result,
            "runs_grouped",
            &self.runs_grouped,
            kept.runs_grouped,
        );
        list_into(
            &mut result,
            "failures_grouped",
            &self.failures_grouped,
            kept.failures_grouped,
        );
        // #422 QR-2b: each section whose reads did not finish, named; never cut.
        for section in &self.incomplete_sections {
            result.insert(format!("{}_incomplete", section.name()), json!(true));
        }
        // Workflow #386 SR-6b, MCP-19b: a part that could not be read leaves the diagnosis
        // incomplete too.
        if self.incomplete || !self.warnings.is_empty() {
            result.insert("incomplete".to_owned(), json!(true));
        }
        Value::Object(result)
    }

    /// The errors page with its first `kept` rows, oldest first. Without a task_id filter its
    /// next_cursor continues it in `events` / `ac_events` with view errors, the same instance
    /// and the same since_unix_ms, on the connection `binding` names.
    pub(crate) fn errors_page(
        &self,
        query: &EventQuery,
        binding: CursorBinding,
        kept: usize,
    ) -> Result<Value, Failure> {
        let truncated = kept < self.errors.len();
        let next = if truncated {
            match kept.checked_sub(1).map(|last| self.error_sequences[last]) {
                Some(after) => Some(
                    RuntimeEventQueryCursor::new(self.snapshot, after, query, events::PROFILE)
                        .map_err(|error| {
                            Failure::CursorEncode(
                                Outcome::new(ActingctlCode::CursorEncodeFailed)
                                    .caused_by(Outcome::new(error.code())),
                            )
                        })?,
                ),
                None => None,
            }
        } else {
            self.errors_next.clone()
        };
        let mut page = Map::new();
        page.insert(
            "events".to_owned(),
            Value::Array(self.errors[..kept].to_vec()),
        );
        page.insert("more".to_owned(), json!(truncated || self.errors_more));
        if truncated {
            page.insert("truncated".to_owned(), json!(true));
        }
        if let Some(cursor) = next.filter(|_| self.continuable) {
            let token = binding
                .encode(&cursor)
                .map_err(|error| Failure::CursorEncode(error.outcome()))?;
            page.insert("next_cursor".to_owned(), json!(token));
        }
        page.insert("snapshot_ledger_position".to_owned(), json!(self.snapshot));
        Ok(Value::Object(page))
    }
}

/// `name` with the first `kept` items, and `<name>_truncated` when items were cut.
fn list_into(result: &mut Map<String, Value>, name: &str, items: &[Value], kept: usize) {
    result.insert(name.to_owned(), Value::Array(items[..kept].to_vec()));
    if kept < items.len() {
        result.insert(format!("{name}_truncated"), json!(true));
    }
}

/// The report's rows for the diagnosed instance (every row without one), its report-level
/// warnings, and its own status: the status's data members, and the outcome of a report that is
/// unavailable or failed.
struct SuspensionRows {
    complete: bool,
    suspended: Vec<Value>,
    packs: Vec<Value>,
    remedies: Vec<Value>,
    lift: Vec<Value>,
    repeating: Vec<Value>,
    warnings: Vec<Value>,
    status: Value,
    outcome: Option<Outcome>,
    clock_slots: Vec<Value>,
    clock_slots_total: usize,
}

impl SuspensionRows {
    /// No rows: a report that is `unavailable` (actingd suspended gave none) or `failed` (it gave
    /// a failure report, or no JSON), with the data members of `status` and its outcome.
    fn without_rows(status: Map<String, Value>, outcome: Outcome) -> Self {
        Self {
            complete: false,
            suspended: Vec::new(),
            packs: Vec::new(),
            remedies: Vec::new(),
            lift: Vec::new(),
            repeating: Vec::new(),
            warnings: Vec::new(),
            status: Value::Object(status),
            outcome: Some(outcome),
            clock_slots: Vec::new(),
            clock_slots_total: 0,
        }
    }
}

/// The data members of a report status: `{status}`, and `exit_code` once the child exited.
fn report_status(status: &str, exit_code: Option<Option<i32>>) -> Map<String, Value> {
    let mut members = Map::new();
    members.insert("status".to_owned(), json!(status));
    if let Some(exit_code) = exit_code {
        members.insert("exit_code".to_owned(), json!(exit_code));
    }
    members
}

/// Whether actingd suspended printed its report (Workflow #378 A6a: suspended v2, whose
/// `outcome` is a success; v1 said `status: "ok"`).
fn report_succeeded(document: &Value) -> bool {
    document
        .get("outcome")
        .and_then(|outcome| outcome.get("category"))
        .and_then(Value::as_str)
        == Some(Category::Success.as_str())
}

/// The outcome actingd suspended's own failure report carries: its envelope (`outcome`) relayed
/// unchanged, else the code it names (`code`). A fatal one is left to the relayed `report`, as a
/// fatal link needs a fatal top and the diagnosis goes on.
fn report_outcome(document: &Value) -> Option<Outcome> {
    let relayed = match document
        .get("outcome")
        .and_then(|outcome| serde_json::from_value::<OutcomeEnvelope>(outcome.clone()).ok())
    {
        Some(envelope) => Some(envelope.into_outcome()),
        None => document
            .get("code")
            .and_then(Value::as_str)
            .map(Outcome::relayed),
    };
    relayed.filter(|outcome| outcome.category() != Category::Fatal)
}

/// The tail of actingd suspended's stderr as `foreign_other_error` evidence; `None` when it wrote
/// nothing.
fn stderr_evidence(stderr: &[u8]) -> Option<Outcome> {
    let tail = stderr_tail(stderr);
    (!tail.trim().is_empty()).then(|| {
        Outcome::new(ContractCode::ForeignOtherError)
            .raw_source(RawSource::Process)
            .raw_text(tail)
    })
}

/// `outcome` with the stderr evidence, when there is some, as a `related` link.
fn with_stderr(outcome: Outcome, stderr: &[u8]) -> Outcome {
    match stderr_evidence(stderr) {
        Some(evidence) => outcome.link(CauseRelation::Related, evidence),
        None => outcome,
    }
}

/// Whether a report row names the instance, by its alias or its instance id; every row does
/// when no instance is diagnosed.
fn of_instance(row: &Value, instance: Option<(&str, &Value)>) -> bool {
    let Some((alias, instance_id)) = instance else {
        return true;
    };
    row.get("instance_id")
        .is_some_and(|id| id.as_str() == Some(alias) || id == instance_id)
}

/// Workflow #386 SR-3: the report's `packs` rows whose suspension defers a pair of the instance
/// (its alias is among the row's `instances`), or every row when no instance is diagnosed.
pub(crate) fn pack_rows(document: &Value, instance: Option<(&str, &Value)>) -> Vec<Value> {
    document
        .get("packs")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter(|row| {
                    instance.is_none_or(|(alias, _)| {
                        row.get("instances")
                            .and_then(Value::as_array)
                            .is_some_and(|instances| {
                                instances.iter().any(|entry| entry.as_str() == Some(alias))
                            })
                    })
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Workflow #411 B15 R7: the report's `remedies` rows of the instance (its alias or its
/// instance id is the row's `instance_id`), or every row when no instance is diagnosed. An
/// unparsed row names no instance and is kept only without one.
pub(crate) fn remedy_rows(document: &Value, instance: Option<(&str, &Value)>) -> Vec<Value> {
    document
        .get("remedies")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter(|row| of_instance(row, instance))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// The instance's `clock_slots` rows of the report recorded since `since_unix_ms` (#372 K3):
/// at most 10, in the order of [`clock_slot_rank`], newest first within each rank; and how many
/// there are in the window.
pub(crate) fn clock_slot_rows(
    document: &Value,
    alias: &str,
    instance_id: &Value,
    since_unix_ms: u64,
) -> (Vec<Value>, usize) {
    clock_slot_rows_of(document, Some((alias, instance_id)), since_unix_ms)
}

/// `clock_slot_rows` of one instance, or of every instance with `None` (Workflow #389 MCP-7).
fn clock_slot_rows_of(
    document: &Value,
    instance: Option<(&str, &Value)>,
    since_unix_ms: u64,
) -> (Vec<Value>, usize) {
    let mut rows = document
        .get("clock_slots")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter(|row| {
                    of_instance(row, instance)
                        && row
                            .get("recorded_at_unix_ms")
                            .and_then(Value::as_u64)
                            .is_some_and(|recorded| recorded >= since_unix_ms)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let total = rows.len();
    // The report lists them newest first; the stable sort keeps that order within each rank.
    rows.sort_by_key(|row| clock_slot_rank(row));
    let kept = rows.into_iter().take(CLOCK_SLOTS).cloned().collect();
    (kept, total)
}

/// The rank of a `clock_slots` row in `ac_diagnose` (#372 K4.4): 0 for a slot lost or refused
/// at `warning` (a refused or a superseded record), 1 for another slot that was held back or
/// skipped (a deferred record, which carries a `gate_code`, or an expired one, which carries a
/// `grace_ms`), 2 for the rest: the owed records, the unparsed ones and any code the catalog
/// does not register. A row's category is its code's, read from the catalog this build embeds.
fn clock_slot_rank(row: &Value) -> u8 {
    if row.get("parse_state").is_some() || row.get("window_start_unix_ms").is_some() {
        return 2;
    }
    let category = row
        .get("code")
        .and_then(Value::as_str)
        .map(Outcome::relayed)
        .filter(|outcome| !outcome.is(ContractCode::OutcomeRelayUnregistered))
        .map(|outcome| outcome.category());
    if category == Some(Category::Warning) {
        0
    } else if row.get("gate_code").is_some() || row.get("grace_ms").is_some() {
        1
    } else {
        2
    }
}

/// Whether two paths name the same directory, as written or once resolved.
fn same_directory(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (fs::canonicalize(left), fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

impl SuspensionReport {
    /// Waits for the child (up to the deadline it was started with) and keeps the rows of
    /// `instance` (alias and instance id), or every row without one.
    fn finish(
        self,
        state_root: Option<&Path>,
        instance: Option<(&str, Value)>,
        since_unix_ms: u64,
    ) -> SuspensionRows {
        let instance = instance
            .as_ref()
            .map(|(alias, instance_id)| (*alias, instance_id));
        let unavailable = |outcome: Outcome| {
            SuspensionRows::without_rows(report_status("unavailable", None), outcome)
        };
        let captured = match self.0.map(JoinHandle::join) {
            Err(outcome) => return unavailable(outcome),
            Ok(Err(_)) => {
                return unavailable(Outcome::new(ActingctlCode::SuspendedReportReaderPanicked));
            }
            Ok(Ok(Err(failure))) => {
                let code = if failure.reader_panicked() {
                    ActingctlCode::SuspendedReportReaderPanicked
                } else {
                    match failure {
                        ChildFailure::Spawn(_) => ActingctlCode::SuspendedReportSpawnFailed,
                        ChildFailure::Io(_) => ActingctlCode::SuspendedReportIoFailed,
                        // Still running at the end of the call budget, and ended there (RL-1c
                        // fix 1).
                        ChildFailure::StillRunning { .. } | ChildFailure::Ended => {
                            ActingctlCode::SuspendedReportTimeout
                        }
                    }
                };
                let outcome = Outcome::new(code);
                return unavailable(match failure.os_failure() {
                    Some(cause) => outcome.caused_by(cause),
                    None => outcome,
                });
            }
            Ok(Ok(Ok(captured))) => captured,
        };
        let exit_code = captured.status.code();
        let document = serde_json::from_slice::<Value>(captured.stdout.trim_ascii());
        match document {
            Ok(document) if captured.status.success() && report_succeeded(&document) => {
                // The report reads the selected configuration's state root; when the reads go
                // to another state root, its rows describe another Runtime.
                let report_state_root = document
                    .get("state_root")
                    .and_then(Value::as_str)
                    .map(PathBuf::from);
                let same_root = match (report_state_root.as_deref(), state_root) {
                    (Some(report), Some(located)) => same_directory(report, located),
                    _ => false,
                };
                // actingd suspended read another state root (`other_root`) than the one the
                // Runtime reads go to (`state_root`): its rows are left out.
                if !same_root {
                    let mut outcome = Outcome::new(ActingctlCode::SuspendedReportStateRootMismatch);
                    if let Some(located) = state_root {
                        outcome = outcome.state_root(located);
                    }
                    if let Some(report) = &report_state_root {
                        outcome = outcome.other_root(report);
                    }
                    return unavailable(outcome);
                }
                let rows = |key: &str| {
                    document
                        .get(key)
                        .and_then(Value::as_array)
                        .map(|rows| {
                            rows.iter()
                                .filter(|row| of_instance(row, instance))
                                .cloned()
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                };
                let (clock_slots, clock_slots_total) = match instance {
                    Some((alias, instance_id)) => {
                        clock_slot_rows(&document, alias, instance_id, since_unix_ms)
                    }
                    None => clock_slot_rows_of(&document, None, since_unix_ms),
                };
                SuspensionRows {
                    // RL-1c fix 1 (#411 H1): a report whose ledger read ran out of its deadline
                    // answers with what it read; the diagnosis is then incomplete.
                    complete: !document
                        .get("warnings")
                        .and_then(Value::as_array)
                        .is_some_and(|warnings| {
                            warnings
                                .iter()
                                .any(|warning| warning == "ledger_read_incomplete")
                        }),
                    suspended: rows("suspended"),
                    packs: pack_rows(&document, instance),
                    remedies: remedy_rows(&document, instance),
                    lift: rows("lifted"),
                    repeating: rows("repeating"),
                    warnings: document
                        .get("warnings")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                    status: json!({
                        "status": "ok",
                        "through_sequence": document.get("through_sequence"),
                    }),
                    outcome: None,
                    clock_slots,
                    clock_slots_total,
                }
            }
            // actingd's own failure document, as it printed it (`report`):
            // `suspended_report_failed`, caused by the outcome the document carries.
            Ok(document) => {
                let outcome = Outcome::new(ActingctlCode::SuspendedReportFailed);
                let outcome = match report_outcome(&document) {
                    Some(cause) => outcome.caused_by(cause),
                    None => outcome,
                };
                let mut status = report_status("failed", Some(exit_code));
                status.insert("report".to_owned(), document);
                SuspensionRows::without_rows(status, with_stderr(outcome, &captured.stderr))
            }
            // No JSON report: the decoder's refusal is the cause.
            Err(error) => SuspensionRows::without_rows(
                report_status("failed", Some(exit_code)),
                with_stderr(
                    Outcome::new(ActingctlCode::SuspendedReportUnreadable).caused_by(
                        Outcome::new(ContractCode::ForeignDataError)
                            .raw_source(RawSource::Json)
                            .raw_text(error.to_string()),
                    ),
                    &captured.stderr,
                ),
            ),
        }
    }
}

fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let mut start = text.len().saturating_sub(MAX_STDERR_TAIL);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_owned()
}

/// Runs `diagnose`; `None` leaves every other command to the CLI, unchanged. One result line on
/// stdout (Workflow #378 A5): the diagnosis with `runtime_diagnosis_reported`, or `{outcome}`
/// with the failure (a flag failure's developer line adds the usage) and exit 1.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != "diagnose" {
        return None;
    }
    Some(match run(arguments) {
        Ok(diagnosis) => cli_line::success(
            cli_line::members(diagnosis, "diagnosis"),
            &Outcome::new(ActingctlCode::RuntimeDiagnosisReported),
        ),
        Err(failure @ Failure::Argument { .. }) => {
            cli_line::failure(&failure.outcome(), Some(USAGE))
        }
        Err(failure) => cli_line::failure(&failure.outcome(), None),
    })
}

fn run(arguments: &[OsString]) -> Result<Value, Failure> {
    let (state_root, root, request) = parse(arguments)?;
    let since_unix_ms = request.window(now_unix_ms()?)?;
    let deadline = Instant::now() + CLI_READ_BUDGET;
    let report = SuspensionReport::start(root.as_deref(), deadline);
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &state_root,
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(Failure::Runtime)?;
    let (diagnosis, query) = read(
        &client,
        &request,
        since_unix_ms,
        report,
        Some(state_root.as_path()),
        deadline,
        &SystemClock,
    )?;
    // This process holds one Runtime connection: its cursor answers cursor_invalid in any
    // later call, as `actingctl events` prints it.
    let binding = CursorBinding {
        session: events::process_session(),
        generation: 1,
    };
    let errors_page = diagnosis.errors_page(&query, binding, diagnosis.errors.len())?;
    let detail = cli_line::detail().unwrap_or(Detail::Full);
    let mut rendered =
        diagnosis.render(&diagnosis.kept_all(), errors_page, &|outcome: &Outcome| {
            cli_line::outcome_value(outcome, detail)
        });
    // Workflow #386 SR-6b, MCP-19b: what could not be read, as the line's `warnings` (D3).
    if let Value::Object(members) = &mut rendered
        && !diagnosis.warnings.is_empty()
    {
        let warnings = diagnosis
            .warnings
            .iter()
            .map(|warning| match warning {
                Warning::Outcome(outcome) => cli_line::warning(outcome),
                Warning::Runtime(error) => cli_line::warning(&error.outcome()),
            })
            .collect();
        members.insert("warnings".to_owned(), Value::Array(warnings));
    }
    Ok(rendered)
}

/// `--state-root` (required), `--root` and the flags of §4.8, each at most once and with a
/// value; any other flag, a repeated one or a malformed value is `actingctl_usage_invalid` with
/// the flag (`argument`) and what is wrong with it (`argument_problem`).
fn parse(arguments: &[OsString]) -> Result<(PathBuf, Option<PathBuf>, DiagnoseRequest), Failure> {
    let mut state_root = None;
    let mut root = None;
    let mut instance = None;
    let mut task_id = None;
    let mut since = None;
    let mut rest = arguments[1..].iter();
    while let Some(flag) = rest.next() {
        let (name, slot) = match flag.to_str() {
            Some(name @ "--state-root") => (name, &mut state_root),
            Some(name @ "--root") => (name, &mut root),
            Some(name @ "--instance") => (name, &mut instance),
            Some(name @ "--task-id") => (name, &mut task_id),
            Some(name @ "--since-unix-ms") => (name, &mut since),
            Some(_) => return Err(Failure::argument(flag, ArgumentProblem::Unknown)),
            None => return Err(Failure::argument(flag, ArgumentProblem::NotUtf8)),
        };
        let value = rest
            .next()
            .ok_or_else(|| Failure::argument(name, ArgumentProblem::ValueMissing))?
            .to_str()
            .ok_or_else(|| Failure::argument(name, ArgumentProblem::NotUtf8))?;
        if value.trim().is_empty() {
            return Err(Failure::argument(name, ArgumentProblem::ValueMissing));
        }
        if slot.replace(value.to_owned()).is_some() {
            return Err(Failure::argument(name, ArgumentProblem::Repeated));
        }
    }
    let request = DiagnoseRequest {
        instance,
        task_id: task_id
            .map(|text| {
                parse_task_id(&text)
                    .ok_or_else(|| Failure::argument("--task-id", ArgumentProblem::NotRecognised))
            })
            .transpose()?,
        since_unix_ms: since
            .map(|text| {
                text.parse::<u64>()
                    .map_err(|_| Failure::argument("--since-unix-ms", ArgumentProblem::WrongType))
            })
            .transpose()?,
    };
    let state_root =
        state_root.ok_or_else(|| Failure::argument("--state-root", ArgumentProblem::Required))?;
    Ok((PathBuf::from(state_root), root.map(PathBuf::from), request))
}
