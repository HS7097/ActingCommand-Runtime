// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl runs` (Workflow #389 MCP-3; MCP-0 command table v1.1 §4.3), the CLI original of
//! MCP `ac_runs`: the contained runs first recorded in a window, newest first, read run by run
//! (runtime-client `list_contained_runs`, on the listing of MCP-391c). Both sides parse their
//! arguments into one `RunsRequest`, read it with `read` and answer with `RunsAnswer::render`.
//! stdout is one line, the same JSON as the ac_runs result, with the warnings as a top-level
//! `warnings` member when there are any (D3). Only MCP cuts rows to its output budget
//! (`truncated`, §2.2). At most one status read per call names the instance and the rows'
//! aliases (§2.5); the run reads are event queries, which record nothing.

use crate::codes::ActingctlCode;
use crate::instance_selector::{self, SelectorMiss};
use actingcommand_contract::outcome::Code;
use actingcommand_contract::{EventActor, EventSource, InstanceId, RuntimeControlPlaneStatus};
use actingcommand_runtime_client::{
    ContainedRunState, ListedContainedRun, RunDispatch, RunListFilter, RunListPosition,
    RuntimeClient, RuntimeClientConfig, RuntimeClientError,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const USAGE: &str = "usage: actingctl runs --state-root <path> [--instance <alias|instance_id|ADB port>] [--since-unix-ms <ms>] [--until-unix-ms <ms>] [--task-prefix <text>] [--dispatch manual|scheduled] [--state open|succeeded|failed|cancelled|interrupted_unterminated] [--limit <1-100>] [--cursor <next_cursor>]";
pub(crate) const DEFAULT_LIMIT: u64 = 20;
pub(crate) const MAX_LIMIT: u64 = 100;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// The window when no since_unix_ms is given: the last 24 h.
const DEFAULT_WINDOW_MS: u64 = DAY_MS;
/// The furthest back since_unix_ms may reach, as for ac_diagnose.
const MAX_WINDOW_MS: u64 = 7 * DAY_MS;
/// How long the CLI reads one page: MCP's 25 s call budget less the second it keeps back to
/// answer, so that both sides end a long read alike.
const CLI_READ_BUDGET: Duration = Duration::from_secs(24);
/// `r1.<snapshot>.<before>.<since>.<until or ->.<filters>`.
const CURSOR_VERSION: &str = "r1";
/// The hex digits of the filters' SHA-256 a cursor carries.
const CURSOR_FINGERPRINT_DIGITS: usize = 16;

/// `--state` / `state`: `open` is admitted or running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateFilter {
    Open,
    Succeeded,
    Failed,
    Cancelled,
    InterruptedUnterminated,
}

impl StateFilter {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "open" => Some(Self::Open),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "interrupted_unterminated" => Some(Self::InterruptedUnterminated),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::InterruptedUnterminated => "interrupted_unterminated",
        }
    }

    fn states(self) -> Vec<ContainedRunState> {
        match self {
            Self::Open => vec![ContainedRunState::Admitted, ContainedRunState::Running],
            Self::Succeeded => vec![ContainedRunState::Succeeded],
            Self::Failed => vec![ContainedRunState::Failed],
            Self::Cancelled => vec![ContainedRunState::Cancelled],
            Self::InterruptedUnterminated => vec![ContainedRunState::InterruptedUnterminated],
        }
    }
}

/// `--dispatch` / `dispatch`: `manual` or `scheduled`.
pub(crate) fn parse_dispatch(text: &str) -> Option<RunDispatch> {
    match text {
        "manual" => Some(RunDispatch::Manual),
        "scheduled" => Some(RunDispatch::Scheduled),
        _ => None,
    }
}

const fn dispatch_name(dispatch: RunDispatch) -> &'static str {
    match dispatch {
        RunDispatch::Manual => "manual",
        RunDispatch::Scheduled => "scheduled",
        RunDispatch::Unknown => "unknown",
    }
}

/// The arguments both sides take, typed (R2: each MCP argument is the CLI flag's name).
pub(crate) struct RunsRequest {
    pub(crate) instance: Option<String>,
    pub(crate) since_unix_ms: Option<u64>,
    pub(crate) until_unix_ms: Option<u64>,
    pub(crate) task_prefix: Option<String>,
    pub(crate) dispatch: Option<RunDispatch>,
    pub(crate) state: Option<StateFilter>,
    pub(crate) limit: u64,
    pub(crate) cursor: Option<String>,
}

/// Why a run list was refused or failed.
pub(crate) enum Failure {
    /// A CLI flag is unknown, repeated, missing its value or malformed.
    Usage,
    /// An argument both sides take is out of range; `field` is its MCP name.
    Argument {
        field: &'static str,
        detail: &'static str,
    },
    SinceOutOfRange {
        oldest_unix_ms: u64,
    },
    CursorInvalid,
    Selector {
        miss: SelectorMiss,
        selector: String,
    },
    Clock,
    Output,
    Runtime(RuntimeClientError),
}

impl Failure {
    /// The registered code. A CLI argument failure is `actingctl_usage_invalid`; MCP answers
    /// `arguments_invalid` for the same one.
    pub(crate) fn code(&self) -> Code {
        match self {
            Self::Usage | Self::Argument { .. } => ActingctlCode::ActingctlUsageInvalid.code(),
            Self::SinceOutOfRange { .. } => ActingctlCode::SinceOutOfRange.code(),
            Self::CursorInvalid => ActingctlCode::CursorInvalid.code(),
            Self::Selector { miss, .. } => miss.code(),
            Self::Clock => ActingctlCode::ClockUnavailable.code(),
            Self::Output => ActingctlCode::ActingctlOutputFailed.code(),
            Self::Runtime(error) => error.code(),
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            Self::Usage => USAGE.to_owned(),
            Self::Argument { field, detail } => format!("argument {field} {detail}"),
            Self::SinceOutOfRange { oldest_unix_ms } => {
                format!("since_unix_ms reaches back at most 7 days, to {oldest_unix_ms}")
            }
            Self::CursorInvalid => "this cursor belongs to another run list (other filters or another window), or is not a next_cursor of one; read again from the first page without a cursor".to_owned(),
            Self::Selector { miss, selector } => miss.message(selector),
            Self::Clock => {
                "the system clock is before the Unix epoch or out of range".to_owned()
            }
            Self::Output => "failed to write JSON output".to_owned(),
            Self::Runtime(error) => error.to_string(),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage => write!(formatter, "{}: {USAGE}", self.code()),
            Self::Argument { field, detail } => write!(
                formatter,
                "{}: --{} {detail}\n{USAGE}",
                self.code(),
                field.replace('_', "-")
            ),
            Self::Runtime(error) => fmt::Display::fmt(error, formatter),
            _ => write!(formatter, "{}: {}", self.code(), self.message()),
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

/// A position in one run list: where the next page starts, the window it lists and the
/// fingerprint of its filters. It stays valid across processes and Runtime restarts.
struct Cursor {
    position: RunListPosition,
    since_unix_ms: u64,
    until_unix_ms: Option<u64>,
    fingerprint: String,
}

impl Cursor {
    fn encode(&self) -> String {
        format!(
            "{CURSOR_VERSION}.{}.{}.{}.{}.{}",
            self.position.snapshot_ledger_position,
            self.position.before_sequence,
            self.since_unix_ms,
            self.until_unix_ms
                .map_or_else(|| "-".to_owned(), |until| until.to_string()),
            self.fingerprint
        )
    }

    fn decode(text: &str) -> Result<Self, Failure> {
        let parts = text.split('.').collect::<Vec<_>>();
        let [version, snapshot, before, since, until, fingerprint] = parts.as_slice() else {
            return Err(Failure::CursorInvalid);
        };
        let number = |part: &str| part.parse::<u64>().map_err(|_| Failure::CursorInvalid);
        if *version != CURSOR_VERSION
            || fingerprint.len() != CURSOR_FINGERPRINT_DIGITS
            || !fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(Failure::CursorInvalid);
        }
        Ok(Self {
            position: RunListPosition {
                snapshot_ledger_position: number(*snapshot)?,
                before_sequence: number(*before)?,
            },
            since_unix_ms: number(*since)?,
            until_unix_ms: match *until {
                "-" => None,
                until => Some(number(until)?),
            },
            fingerprint: (*fingerprint).to_owned(),
        })
    }
}

/// The filters and the window of a run list, as a cursor carries them.
fn fingerprint(
    instance_id: Option<InstanceId>,
    request: &RunsRequest,
    since_unix_ms: u64,
    until_unix_ms: Option<u64>,
) -> String {
    let canonical = json!([
        instance_id,
        request.task_prefix,
        request.dispatch.map(dispatch_name),
        request.state.map(StateFilter::as_str),
        since_unix_ms,
        until_unix_ms,
    ])
    .to_string();
    Sha256::digest(canonical.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..CURSOR_FINGERPRINT_DIGITS]
        .to_owned()
}

/// One page of a run list, before rendering: its rows with their order sequences.
pub(crate) struct RunsAnswer {
    rows: Vec<(Value, u64)>,
    since_unix_ms: u64,
    until_unix_ms: Option<u64>,
    /// Where this page started: the next page of a page cut to nothing starts there again.
    from: RunListPosition,
    next: Option<RunListPosition>,
    incomplete: bool,
    snapshot: u64,
    fingerprint: String,
}

impl RunsAnswer {
    pub(crate) fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The result with the first `kept` rows. With fewer rows than the page holds it is
    /// `truncated`, and next_cursor continues after the last row kept.
    pub(crate) fn render(&self, kept: usize) -> Value {
        let kept = kept.min(self.rows.len());
        let truncated = kept < self.rows.len();
        let next = if truncated {
            Some(RunListPosition {
                snapshot_ledger_position: self.snapshot,
                before_sequence: kept
                    .checked_sub(1)
                    .map_or(self.from.before_sequence, |last| self.rows[last].1),
            })
        } else {
            self.next
        };
        let mut window = Map::new();
        window.insert("since_unix_ms".to_owned(), json!(self.since_unix_ms));
        if let Some(until) = self.until_unix_ms {
            window.insert("until_unix_ms".to_owned(), json!(until));
        }
        let mut result = Map::new();
        result.insert(
            "runs".to_owned(),
            Value::Array(
                self.rows[..kept]
                    .iter()
                    .map(|(row, _)| row.clone())
                    .collect(),
            ),
        );
        result.insert("window".to_owned(), Value::Object(window));
        if let Some(position) = next {
            let cursor = Cursor {
                position,
                since_unix_ms: self.since_unix_ms,
                until_unix_ms: self.until_unix_ms,
                fingerprint: self.fingerprint.clone(),
            };
            result.insert("next_cursor".to_owned(), json!(cursor.encode()));
        }
        if truncated {
            result.insert("truncated".to_owned(), json!(true));
        }
        if self.incomplete {
            result.insert("incomplete".to_owned(), json!(true));
        }
        result.insert("snapshot_ledger_position".to_owned(), json!(self.snapshot));
        Value::Object(result)
    }

    /// `call_budget_exhausted` when the read budget ended the page early, in the members
    /// `ToolError::into_warning` gives an MCP warning until A5 replaces both (D3).
    pub(crate) fn warnings(&self) -> Vec<Value> {
        if !self.incomplete {
            return Vec::new();
        }
        let mut warning = Map::new();
        warning.insert(
            "code".to_owned(),
            json!(ActingctlCode::CallBudgetExhausted.as_str()),
        );
        warning.insert(
            "message".to_owned(),
            json!(
                "the read budget ended before every listed run of this page was read; next_cursor continues after the last one read"
            ),
        );
        warning.insert("blocked_by".to_owned(), Value::Null);
        warning.insert("details".to_owned(), Value::Object(Map::new()));
        vec![Value::Object(warning)]
    }
}

/// One page of the run list `request` asks for, read by `client` before `deadline`.
pub(crate) fn read(
    client: &RuntimeClient,
    request: &RunsRequest,
    now_unix_ms: u64,
    deadline: Instant,
) -> Result<RunsAnswer, Failure> {
    let resumed = request.cursor.as_deref().map(Cursor::decode).transpose()?;
    // A cursor carries its window: since and until may be left out when it is sent back.
    let since_unix_ms = request
        .since_unix_ms
        .or_else(|| resumed.as_ref().map(|cursor| cursor.since_unix_ms))
        .unwrap_or_else(|| now_unix_ms.saturating_sub(DEFAULT_WINDOW_MS));
    let until_unix_ms = request
        .until_unix_ms
        .or_else(|| resumed.as_ref().and_then(|cursor| cursor.until_unix_ms));
    let oldest_unix_ms = now_unix_ms.saturating_sub(MAX_WINDOW_MS);
    if since_unix_ms < oldest_unix_ms {
        return Err(Failure::SinceOutOfRange { oldest_unix_ms });
    }
    if until_unix_ms.is_some_and(|until| until < since_unix_ms) {
        return Err(Failure::Argument {
            field: "until_unix_ms",
            detail: "is before since_unix_ms",
        });
    }
    let limit = usize::try_from(request.limit).map_err(|_| Failure::Argument {
        field: "limit",
        detail: "is out of range",
    })?;
    let mut status = None;
    let instance_id = match &request.instance {
        Some(selector) => {
            let read = client.status().map_err(Failure::Runtime)?;
            let instance_id = instance_selector::select(read.instances(), selector)
                .map_err(|miss| Failure::Selector {
                    miss,
                    selector: selector.clone(),
                })?
                .instance_id();
            status = Some(read);
            Some(instance_id)
        }
        None => None,
    };
    let fingerprint = fingerprint(instance_id, request, since_unix_ms, until_unix_ms);
    if resumed
        .as_ref()
        .is_some_and(|cursor| cursor.fingerprint != fingerprint)
    {
        return Err(Failure::CursorInvalid);
    }
    let filter = RunListFilter {
        instance_id,
        task_prefix: request.task_prefix.clone(),
        dispatch: request.dispatch,
        states: request.state.map_or_else(Vec::new, StateFilter::states),
    };
    let page = client
        .list_contained_runs(
            since_unix_ms,
            until_unix_ms,
            &filter,
            resumed.as_ref().map(|cursor| cursor.position),
            limit,
            deadline,
        )
        .map_err(Failure::Runtime)?;
    // The rows name their instance by alias: one status read, and only when a row needs it.
    if status.is_none() && !page.runs.is_empty() {
        status = Some(client.status().map_err(Failure::Runtime)?);
    }
    let rows = page
        .runs
        .iter()
        .map(|run| (row(run, status.as_ref()), run.order_sequence))
        .collect();
    Ok(RunsAnswer {
        rows,
        since_unix_ms,
        until_unix_ms,
        from: RunListPosition {
            snapshot_ledger_position: page.snapshot_ledger_position,
            before_sequence: resumed
                .as_ref()
                .map_or(u64::MAX, |cursor| cursor.position.before_sequence),
        },
        next: page.next,
        incomplete: page.incomplete,
        snapshot: page.snapshot_ledger_position,
        fingerprint,
    })
}

/// One row of §4.3: the run's identity, its alias when its instance is still configured, its
/// dispatch, origin and state, when it was first recorded and when it ended, and its terminal.
fn row(run: &ListedContainedRun, status: Option<&RuntimeControlPlaneStatus>) -> Value {
    let run_status = &run.status;
    let alias = run_status.instance_id.and_then(|instance_id| {
        status?
            .instances()
            .iter()
            .find(|instance| instance.instance_id() == instance_id)
            .map(|instance| instance.instance_alias())
    });
    let mut row = Map::new();
    row.insert("request_id".to_owned(), json!(run_status.request_id));
    if let Some(run_id) = run_status.run_id {
        row.insert("run_id".to_owned(), json!(run_id));
    }
    if let Some(task_id) = run_status.task_id {
        row.insert("task_id".to_owned(), json!(task_id));
    }
    if let Some(alias) = alias {
        row.insert("alias".to_owned(), json!(alias));
    }
    if let Some(instance_id) = run_status.instance_id {
        row.insert("instance_id".to_owned(), json!(instance_id));
    }
    row.insert("dispatch".to_owned(), json!(run_status.dispatch));
    row.insert("origin".to_owned(), json!(run_status.origin));
    row.insert("state".to_owned(), json!(run_status.state));
    row.insert(
        "admitted_at_unix_ms".to_owned(),
        json!(run.first_fact_unix_ms),
    );
    if let Some(ended) = run.terminal_unix_ms {
        row.insert("ended_at_unix_ms".to_owned(), json!(ended));
        row.insert(
            "duration_ms".to_owned(),
            json!(ended.saturating_sub(run.first_fact_unix_ms)),
        );
    }
    if let Some(terminal) = &run_status.terminal {
        row.insert("task_result".to_owned(), json!(terminal.outcome));
        if let Some(code) = &terminal.failure_code {
            row.insert("failure_code".to_owned(), json!(code));
        }
        if let Some(page) = &terminal.final_page {
            row.insert("final_page".to_owned(), json!(page));
        }
        if let Some(steps) = terminal.executed_steps {
            row.insert("executed_steps".to_owned(), json!(steps));
        }
    }
    Value::Object(row)
}

/// Runs `runs`; `None` leaves every other command to the CLI, unchanged.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != "runs" {
        return None;
    }
    Some(match run(arguments).and_then(|output| print(&output)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("FATAL actingctl: {failure}");
            ExitCode::FAILURE
        }
    })
}

fn run(arguments: &[OsString]) -> Result<Value, Failure> {
    let (state_root, request) = parse(arguments)?;
    let now_unix_ms = now_unix_ms()?;
    let deadline = Instant::now() + CLI_READ_BUDGET;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        state_root,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(Failure::Runtime)?;
    let answer = read(&client, &request, now_unix_ms, deadline)?;
    let mut output = answer.render(answer.row_count());
    let warnings = answer.warnings();
    if !warnings.is_empty() {
        output["warnings"] = Value::Array(warnings);
    }
    Ok(output)
}

/// `--state-root` and the flags of §4.3, each at most once and with a value; any other flag, a
/// repeated one or a malformed value is `actingctl_usage_invalid`.
fn parse(arguments: &[OsString]) -> Result<(PathBuf, RunsRequest), Failure> {
    let mut state_root = None;
    let mut instance = None;
    let mut since = None;
    let mut until = None;
    let mut task_prefix = None;
    let mut dispatch = None;
    let mut state = None;
    let mut limit = None;
    let mut cursor = None;
    let mut rest = arguments[1..].iter();
    while let Some(flag) = rest.next() {
        let slot = match flag.to_str() {
            Some("--state-root") => &mut state_root,
            Some("--instance") => &mut instance,
            Some("--since-unix-ms") => &mut since,
            Some("--until-unix-ms") => &mut until,
            Some("--task-prefix") => &mut task_prefix,
            Some("--dispatch") => &mut dispatch,
            Some("--state") => &mut state,
            Some("--limit") => &mut limit,
            Some("--cursor") => &mut cursor,
            _ => return Err(Failure::Usage),
        };
        let value = rest
            .next()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .ok_or(Failure::Usage)?;
        if slot.replace(value.to_owned()).is_some() {
            return Err(Failure::Usage);
        }
    }
    let number = |text: Option<String>| {
        text.map(|text| text.parse::<u64>().map_err(|_| Failure::Usage))
            .transpose()
    };
    let limit = match number(limit)? {
        None => DEFAULT_LIMIT,
        Some(limit) if (1..=MAX_LIMIT).contains(&limit) => limit,
        Some(_) => return Err(Failure::Usage),
    };
    let request = RunsRequest {
        instance,
        since_unix_ms: number(since)?,
        until_unix_ms: number(until)?,
        task_prefix,
        dispatch: dispatch
            .map(|text| parse_dispatch(&text).ok_or(Failure::Usage))
            .transpose()?,
        state: state
            .map(|text| StateFilter::parse(&text).ok_or(Failure::Usage))
            .transpose()?,
        limit,
        cursor,
    };
    Ok((PathBuf::from(state_root.ok_or(Failure::Usage)?), request))
}

fn print(output: &Value) -> Result<(), Failure> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    serde_json::to_writer(&mut writer, output).map_err(|_| Failure::Output)?;
    writer.write_all(b"\n").map_err(|_| Failure::Output)?;
    writer.flush().map_err(|_| Failure::Output)
}
