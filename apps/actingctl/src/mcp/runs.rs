// SPDX-License-Identifier: AGPL-3.0-only

//! Run tools (#338 §四 工具表, R1): ac_overview's run per instance, ac_get_run and
//! ac_diagnose. Every run is runtime-client's `actingcommand.run-status.v1` projection,
//! passed on unchanged; this module only picks which runs to read, waits, and bounds the
//! output. Every `recent_runs` read gets a bounded window, which the result echoes.

use super::child::{self, Captured, ChildFailure};
use super::lab;
use super::observer::{EVENTS_PROFILE, encode_cursor, event_row, select_instance};
use super::runtime::Connected;
use super::tools::{
    self, Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument,
};
use crate::codes::ActingctlCode;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    EventQuery, LedgerView, RequestId, RunId, RuntimeEventQueryCursor,
    RuntimeEventQueryPageRequest, RuntimeInstanceStatus, TaskId,
};
use actingcommand_runtime_client::{ContainedRunState, RecentContainedRuns, RunKey, RunStatusMode};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// The run window of ac_overview and, by default, of ac_diagnose.
const DEFAULT_WINDOW_MS: u64 = DAY_MS;
/// The furthest back ac_diagnose's since_unix_ms may reach.
const MAX_WINDOW_MS: u64 = 7 * DAY_MS;
const MAX_WAIT_S: u64 = 25;
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Time kept back from the call budget for the last full run read of a wait.
const WAIT_READ_RESERVE: Duration = Duration::from_secs(5);
/// Time kept back from the call budget before another Runtime read starts.
const READ_RESERVE: Duration = Duration::from_secs(1);
/// ac_diagnose reads the 10 most recent runs, the most `recent_runs` returns.
const DIAGNOSE_RUNS: usize = 10;
const ERRORS_PAGE_LIMIT: u16 = 20;
/// ac_diagnose keeps at most this many of the instance's clock-slot rows (#372 K3).
const DIAGNOSE_CLOCK_SLOTS: usize = 10;
/// The tail of a failed subprocess's stderr kept in the result.
const MAX_STDERR_TAIL: usize = 2048;

type SuspensionChild = JoinHandle<Result<Captured, ChildFailure>>;

fn now_unix_ms() -> Result<u64, ToolError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .ok_or_else(|| {
            ToolError::new(
                "runtime",
                ActingctlCode::ClockUnavailable,
                "the system clock is before the Unix epoch or out of range",
            )
        })
}

/// The start of the default 24 h run window.
pub(super) fn default_window() -> Result<u64, ToolError> {
    Ok(now_unix_ms()?.saturating_sub(DEFAULT_WINDOW_MS))
}

/// The newest run first recorded on `instance` since `since_unix_ms`: one brief R1 read.
pub(super) fn latest_run(
    context: &ToolContext<'_>,
    connected: &Connected,
    instance: &RuntimeInstanceStatus,
    since_unix_ms: u64,
) -> Result<RecentContainedRuns, ToolError> {
    if Instant::now() + READ_RESERVE >= context.deadline {
        return Err(ToolError::new(
            "runtime",
            ActingctlCode::CallBudgetExhausted,
            format!(
                "the 25 s call budget left no time to read the runs of {}",
                instance.instance_alias()
            ),
        ));
    }
    connected
        .client
        .recent_runs(instance.instance_id(), since_unix_ms, 1)
        .map_err(|error| context.runtime.failure(connected, &error))
}

/// ac_get_run: R1 Full by handle (request_id) or run_id; with wait_s it reads again every
/// second while the state is not_found, admitted or running.
pub(super) fn get_run(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["handle", "run_id", "wait_s"])?;
    let wait = Duration::from_secs(arguments.integer("wait_s", 0, MAX_WAIT_S)?.unwrap_or(0));
    let last_read = context
        .deadline
        .checked_sub(WAIT_READ_RESERVE)
        .unwrap_or(context.deadline);
    let wait_until = (Instant::now() + wait).min(last_read);
    let handle = arguments.string("handle")?;
    let job = handle.and_then(|handle| context.jobs.find(handle));
    // A job of this process that is not a run (a pause, a resume, an emulator control, a
    // stop, a Lab call) has no run status: its phase and, once it ended, its outcome.
    if let Some(job) = job.as_ref().filter(|job| job.kind != "run_pack") {
        if arguments.string("run_id")?.is_some() {
            return Err(invalid_argument(
                "handle",
                "or run_id: give exactly one of them",
            ));
        }
        job.wait_until(wait_until, context.cancelled);
        return Ok(ToolSuccess::new(
            json!({"handle": job.handle, "job": job.snapshot()}),
        ));
    }
    let key = match (handle, arguments.string("run_id")?) {
        (Some(handle), None) => RunKey::RequestId(
            serde_json::from_value::<RequestId>(json!(handle)).map_err(|_| {
                if handle.starts_with("correlation_") || handle.starts_with(lab::HANDLE_PREFIX) {
                    ToolError::usage(
                        ActingctlCode::HandleUnknown,
                        "this job handle is not held by this MCP process: the process that started it ended, or it is another one's",
                    )
                } else {
                    invalid_argument("handle", "must be a Runtime request_id or a job handle")
                }
            })?,
        ),
        (None, Some(run_id)) => RunKey::RunId(
            serde_json::from_value::<RunId>(json!(run_id))
                .map_err(|_| invalid_argument("run_id", "must be a Runtime run_id"))?,
        ),
        _ => {
            return Err(invalid_argument(
                "handle",
                "or run_id: give exactly one of them",
            ));
        }
    };
    let connected = context.runtime.connect()?;
    loop {
        let status = connected
            .client
            .contained_run_status(key, RunStatusMode::Full)
            .map_err(|error| context.runtime.failure(&connected, &error))?;
        let may_change = matches!(
            status.state,
            ContainedRunState::NotFound | ContainedRunState::Admitted | ContainedRunState::Running
        );
        // A submit job of this process that failed before admission leaves no run.
        let job_failed_first = job.as_ref().is_some_and(|job| job.finished())
            && status.state == ContainedRunState::NotFound;
        if !may_change
            || job_failed_first
            || context.cancelled.load(Ordering::SeqCst)
            || Instant::now() + POLL_INTERVAL >= wait_until
        {
            let mut result = json!(status);
            if let Some(job) = &job {
                result["job"] = job.snapshot();
            }
            return Ok(ToolSuccess::new(result));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// ac_diagnose: the instance's recent failed or open runs, its errors page and its rows of
/// `actingd suspended`, all within one bounded window.
pub(super) fn diagnose(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance", "task_id", "since_unix_ms"])?;
    let selector = arguments.required_string("instance")?;
    let task_id = arguments
        .string("task_id")?
        .map(|text| {
            serde_json::from_value::<TaskId>(json!(text))
                .map_err(|_| invalid_argument("task_id", "must be a Runtime task_id"))
        })
        .transpose()?;
    let now = now_unix_ms()?;
    let oldest = now.saturating_sub(MAX_WINDOW_MS);
    let since = match arguments.integer("since_unix_ms", 0, u64::MAX)? {
        Some(since) if since < oldest => {
            return Err(ToolError::usage(
                ActingctlCode::SinceOutOfRange,
                format!("since_unix_ms reaches back at most 7 days, to {oldest}"),
            )
            .with_detail("oldest_unix_ms", json!(oldest)));
        }
        Some(since) => since,
        None => now.saturating_sub(DEFAULT_WINDOW_MS),
    };
    // The read-only `actingd suspended` runs beside the Runtime reads.
    let location = context.runtime.locate();
    let suspension = start_suspension_report(context, &location);
    let connected = context.runtime.connect()?;
    let status = connected
        .client
        .status()
        .map_err(|error| context.runtime.failure(&connected, &error))?;
    let instance = select_instance(status.instances(), selector)?;
    let alias = instance.instance_alias().to_owned();
    let instance_id = instance.instance_id();
    let recent = connected
        .client
        .recent_runs(instance_id, since, DIAGNOSE_RUNS)
        .map_err(|error| context.runtime.failure(&connected, &error))?;
    let runs = recent
        .runs
        .iter()
        .filter(|run| {
            matches!(
                run.state,
                ContainedRunState::Failed
                    | ContainedRunState::Admitted
                    | ContainedRunState::Running
                    | ContainedRunState::InterruptedUnterminated
            ) && task_id.is_none_or(|task_id| run.task_id == Some(task_id))
        })
        .map(|run| json!(run))
        .collect::<Vec<_>>();
    let query = EventQuery {
        view: Some(LedgerView::Errors),
        from_timestamp_unix_ms: Some(since),
        instance_id: Some(instance_id),
        task_id,
        ..EventQuery::default()
    };
    let request = RuntimeEventQueryPageRequest::new(ERRORS_PAGE_LIMIT, None).map_err(|error| {
        ToolError::new(
            "runtime",
            ActingctlCode::ErrorsQueryInvalid,
            format!("cannot form the errors page request: {error}"),
        )
    })?;
    let page = connected
        .client
        .query_event_page(query.clone(), EVENTS_PROFILE, request)
        .map_err(|error| context.runtime.failure(&connected, &error))?;
    let report = finish_suspension_report(
        suspension,
        location.state_root.as_deref().ok(),
        &alias,
        &json!(instance_id),
        since,
    );
    let diagnosis = Diagnosis {
        instance: json!({"alias": alias, "instance_id": instance_id}),
        since,
        incomplete: recent.incomplete || !report.complete,
        runs,
        errors: page.events().iter().map(event_row).collect(),
        error_sequences: page.events().iter().map(|event| event.sequence).collect(),
        errors_more: page.has_more(),
        errors_next: page.next_cursor().cloned(),
        snapshot: page.snapshot_ledger_position(),
        // A page with a task filter is not a page ac_events can continue.
        continuable: task_id.is_none(),
        repeating: report.repeating,
        suspended: report.suspended,
        lift: report.lift,
        report_warnings: report.warnings,
        report_status: report.status,
        clock_slots: report.clock_slots,
        clock_slots_total: report.clock_slots_total,
    };
    diagnosis.render_within_budget(|kept| diagnosis.errors_page(context, &connected, &query, kept))
}

/// ac_diagnose's parts, cut from their ends until the result fits the output budget.
pub(super) struct Diagnosis {
    pub(super) instance: Value,
    pub(super) since: u64,
    pub(super) incomplete: bool,
    pub(super) runs: Vec<Value>,
    pub(super) errors: Vec<Value>,
    pub(super) error_sequences: Vec<u64>,
    pub(super) errors_more: bool,
    pub(super) errors_next: Option<RuntimeEventQueryCursor>,
    pub(super) snapshot: u64,
    pub(super) continuable: bool,
    pub(super) repeating: Vec<Value>,
    pub(super) suspended: Vec<Value>,
    pub(super) lift: Vec<Value>,
    pub(super) report_warnings: Vec<Value>,
    pub(super) report_status: Value,
    /// At most 10 of the instance's clock-slot rows (#372 K3), refused and deferred first.
    pub(super) clock_slots: Vec<Value>,
    /// The instance's clock-slot rows in the window, kept or not.
    pub(super) clock_slots_total: usize,
}

/// How many of each list a rendering keeps.
#[derive(Clone, Copy)]
struct Kept {
    runs: usize,
    errors: usize,
    repeating: usize,
    suspended: usize,
    lift: usize,
    report_warnings: usize,
    clock_slots: usize,
}

impl Diagnosis {
    /// Renders with every list whole, then cuts until the result fits. `errors_page` renders
    /// the errors page with its first `kept` rows.
    pub(super) fn render_within_budget(
        &self,
        errors_page: impl Fn(usize) -> Result<Value, ToolError>,
    ) -> ToolOutcome {
        let mut kept = Kept {
            runs: self.runs.len(),
            errors: self.errors.len(),
            repeating: self.repeating.len(),
            suspended: self.suspended.len(),
            lift: self.lift.len(),
            report_warnings: self.report_warnings.len(),
            clock_slots: self.clock_slots.len(),
        };
        loop {
            let result = self.render(kept, &errors_page)?;
            if tools::fits_success(&result) {
                return Ok(ToolSuccess::new(result));
            }
            // The clock-slot rows go first, from the end of their list (the owed rows before
            // the refused and deferred ones, each oldest first): the full list stays in
            // actingd suspended and clock_slots_total points there (#372 K3). Then what
            // another tool can read again: the errors page (ac_events), then the runs, oldest
            // first (ac_get_run / ac_events). The other rows of the actingd suspended report
            // have no other MCP route, so they go last: repeating, lift, suspended, then the
            // report's warnings.
            if kept.clock_slots > 0 {
                kept.clock_slots -= 1;
            } else if kept.errors > 0 {
                kept.errors -= 1;
            } else if kept.runs > 0 {
                kept.runs -= 1;
            } else if kept.repeating > 0 {
                kept.repeating -= 1;
            } else if kept.lift > 0 {
                kept.lift -= 1;
            } else if kept.suspended > 0 {
                kept.suspended -= 1;
            } else if kept.report_warnings > 0 {
                kept.report_warnings -= 1;
            } else {
                // Nothing left to cut: the result answers output_budget_exceeded.
                return Ok(ToolSuccess::new(result));
            }
        }
    }

    fn render(
        &self,
        kept: Kept,
        errors_page: &impl Fn(usize) -> Result<Value, ToolError>,
    ) -> Result<Value, ToolError> {
        let mut result = Map::new();
        result.insert("instance".to_owned(), self.instance.clone());
        result.insert("window".to_owned(), json!({"since_unix_ms": self.since}));
        list_into(&mut result, "runs", &self.runs, kept.runs);
        result.insert("errors_page".to_owned(), errors_page(kept.errors)?);
        list_into(&mut result, "suspended", &self.suspended, kept.suspended);
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
        result.insert("suspended_report".to_owned(), self.report_status.clone());
        if self.incomplete {
            result.insert("incomplete".to_owned(), json!(true));
        }
        Ok(Value::Object(result))
    }

    /// One page of the instance's errors view in the window, oldest first. Without a
    /// task_id filter, next_cursor continues it in ac_events with view errors, the same
    /// instance and the same since_unix_ms.
    fn errors_page(
        &self,
        context: &ToolContext<'_>,
        connected: &Connected,
        query: &EventQuery,
        kept: usize,
    ) -> Result<Value, ToolError> {
        let truncated = kept < self.errors.len();
        let next = if truncated {
            match kept.checked_sub(1).map(|last| self.error_sequences[last]) {
                Some(after) => Some(
                    RuntimeEventQueryCursor::new(self.snapshot, after, query, EVENTS_PROFILE)
                        .map_err(|error| {
                            ToolError::new(
                                "runtime",
                                ActingctlCode::CursorEncodeFailed,
                                format!("cannot form the next cursor: {error}"),
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
            page.insert(
                "next_cursor".to_owned(),
                json!(encode_cursor(context, connected, &cursor)?),
            );
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

/// Starts `<root>\runtime\actingcommand-actingd.exe suspended --config
/// <root>\actingd.config.json`, read-only beside a running daemon.
fn start_suspension_report(
    context: &ToolContext<'_>,
    location: &super::runtime::Location,
) -> Result<SuspensionChild, Value> {
    location.check().map_err(|_| json!({ "status": "unavailable", "code": SharedCode::InstallSelectionUnavailable.as_str(), "message": location.state_root.as_ref().err() }))?;
    let Some(root) = location.root.as_deref() else {
        return Err(json!({
            "status": "unavailable",
            "code": ActingctlCode::InstallRootUnresolved.as_str(),
            "message": "actingctl does not run from an install root and mcp-serve got no --root, so actingd suspended cannot be found",
        }));
    };
    let mut command = Command::new(root.join("runtime").join("actingcommand-actingd.exe"));
    command.arg("suspended").arg("--config").arg(
        location
            .config
            .clone()
            .unwrap_or_else(|| root.join("actingd.config.json")),
    );
    super::runtime::pin_child(&mut command, location.installation.as_ref());
    let deadline = context
        .deadline
        .checked_sub(READ_RESERVE)
        .unwrap_or(context.deadline);
    Ok(thread::spawn(move || {
        child::run_captured(command, deadline)
    }))
}

/// The report's rows for one instance, its report-level warnings, and its own status.
struct SuspensionRows {
    complete: bool,
    suspended: Vec<Value>,
    lift: Vec<Value>,
    repeating: Vec<Value>,
    warnings: Vec<Value>,
    status: Value,
    clock_slots: Vec<Value>,
    clock_slots_total: usize,
}

/// Whether a report row names the instance, by its alias or its instance id.
fn of_instance(row: &Value, alias: &str, instance_id: &Value) -> bool {
    row.get("instance_id")
        .is_some_and(|id| id.as_str() == Some(alias) || id == instance_id)
}

/// The instance's `clock_slots` rows of the report recorded since `since_unix_ms` (#372 K3):
/// at most 10, the newest refused and deferred rows (those with a `gate_code`) first, then the
/// newest owed rows; and how many there are in the window.
pub(super) fn clock_slot_rows(
    document: &Value,
    alias: &str,
    instance_id: &Value,
    since_unix_ms: u64,
) -> (Vec<Value>, usize) {
    let rows = document
        .get("clock_slots")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter(|row| {
                    of_instance(row, alias, instance_id)
                        && row
                            .get("recorded_at_unix_ms")
                            .and_then(Value::as_u64)
                            .is_some_and(|recorded| recorded >= since_unix_ms)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let total = rows.len();
    // The report lists them newest first; the partition keeps that order on each side.
    let (stopped, owed): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(|row| row.get("gate_code").is_some());
    let kept = stopped
        .into_iter()
        .chain(owed)
        .take(DIAGNOSE_CLOCK_SLOTS)
        .cloned()
        .collect();
    (kept, total)
}

/// Whether two paths name the same directory, as written or once resolved.
fn same_directory(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (fs::canonicalize(left), fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

fn finish_suspension_report(
    child: Result<SuspensionChild, Value>,
    state_root: Option<&Path>,
    alias: &str,
    instance_id: &Value,
    since_unix_ms: u64,
) -> SuspensionRows {
    let unavailable = |status: Value| SuspensionRows {
        complete: false,
        suspended: Vec::new(),
        lift: Vec::new(),
        repeating: Vec::new(),
        warnings: Vec::new(),
        status,
        clock_slots: Vec::new(),
        clock_slots_total: 0,
    };
    let captured = match child.map(JoinHandle::join) {
        Err(status) => return unavailable(status),
        Ok(Err(_)) => {
            return unavailable(json!({
                "status": "unavailable",
                "code": ActingctlCode::SuspendedReportReaderPanicked.as_str(),
            }));
        }
        Ok(Ok(Err(failure))) => {
            let (code, message) = match failure {
                ChildFailure::Spawn(error) => {
                    (ActingctlCode::SuspendedReportSpawnFailed, error.to_string())
                }
                ChildFailure::Io(error) => {
                    (ActingctlCode::SuspendedReportIoFailed, error.to_string())
                }
                ChildFailure::StillRunning => (
                    ActingctlCode::SuspendedReportTimeout,
                    "actingd suspended was still running at the end of the call budget".to_owned(),
                ),
            };
            return unavailable(json!({
                "status": "unavailable",
                "code": code.as_str(),
                "message": message,
            }));
        }
        Ok(Ok(Ok(captured))) => captured,
    };
    let stderr_tail = stderr_tail(&captured.stderr);
    let exit_code = captured.status.code();
    let document = serde_json::from_slice::<Value>(captured.stdout.trim_ascii());
    match document {
        Ok(document)
            if captured.status.success()
                && document.get("status").and_then(Value::as_str) == Some("ok") =>
        {
            // The report reads <root>\actingd.config.json's state root; when mcp-serve was
            // given another --state-root, its rows describe another Runtime.
            let report_state_root = document
                .get("state_root")
                .and_then(Value::as_str)
                .map(PathBuf::from);
            let same_root = match (report_state_root.as_deref(), state_root) {
                (Some(report), Some(located)) => same_directory(report, located),
                _ => false,
            };
            if !same_root {
                return unavailable(json!({
                    "status": "unavailable",
                    "code": ActingctlCode::SuspendedReportStateRootMismatch.as_str(),
                    "message": "actingd suspended read another state root than the one this server reads; its rows are left out",
                    "details": {
                        "report_state_root": report_state_root.map(|path| path.display().to_string()),
                        "state_root": state_root.map(|path| path.display().to_string()),
                    },
                }));
            }
            let rows = |key: &str| {
                document
                    .get(key)
                    .and_then(Value::as_array)
                    .map(|rows| {
                        rows.iter()
                            .filter(|row| of_instance(row, alias, instance_id))
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            };
            let (clock_slots, clock_slots_total) =
                clock_slot_rows(&document, alias, instance_id, since_unix_ms);
            SuspensionRows {
                complete: true,
                suspended: rows("suspended"),
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
                clock_slots,
                clock_slots_total,
            }
        }
        // actingd's own failure document, as it printed it.
        Ok(document) => unavailable(json!({
            "status": "failed",
            "exit_code": exit_code,
            "report": document,
            "stderr_tail": stderr_tail,
        })),
        Err(error) => unavailable(json!({
            "status": "failed",
            "code": ActingctlCode::SuspendedReportUnreadable.as_str(),
            "message": format!("actingd suspended printed no JSON report: {error}"),
            "exit_code": exit_code,
            "stderr_tail": stderr_tail,
        })),
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
