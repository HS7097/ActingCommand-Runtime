// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl events` and what it shares with the MCP tool `ac_events` (Workflow #389 MCP-1;
//! MCP-0 command table v1.1 §4.1): the parameters, the Runtime query, the wait for rows, the
//! rows and the page. Both read with QueryEvents, which records nothing in the ledger, and
//! resolve an instance selector with at most one Status per call. The CLI prints the page
//! whole; the MCP tool (`mcp::events`) adds only what §2.2 lets it differ in. U-3 generates
//! both from the command table; until then this module keeps their parameters and output the
//! same.
//!
//! `detail` opens `brief` and `payload`; `summary`, which reads A3's `outcome`, follows once A3
//! (#721) has merged (coordinator ruling Q4).
//!
//! Paging. A cursor is bound to the process and the Runtime connection that issued it (§4.1).
//! MCP pages with `next_cursor` inside its server's connection. The CLI pages with
//! `--after-sequence <last_sequence>`: every `actingctl events` call is its own process, so the
//! `next_cursor` one call prints always answers `cursor_invalid` in the next call, by design;
//! `last_sequence` stays valid across calls and Runtime restarts.

use crate::cli_line;
use crate::codes::{self, ActingctlCode};
use crate::instance_selector::{self, SelectorMiss};
use crate::material_export;
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{
    CorrelationId, EventActor, EventQuery, EventSeverity, EventSource, EventType, InstanceId,
    LedgerView, MAX_RUNTIME_EVENT_QUERY_EVENTS, OriginModule, ProjectedEvent, ProjectionPayload,
    ProjectionProfile, RequestId, RunId, RuntimeEventQueryCursor, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest, SanitizationError, TaskId,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use std::collections::hash_map::RandomState;
use std::ffi::OsString;
use std::hash::BuildHasher;
use std::path::PathBuf;
use std::process::{self, ExitCode};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Rows carry their full payloads, where the ledger's diagnostic code lives.
pub(crate) const PROFILE: ProjectionProfile = ProjectionProfile::Forensic;
/// Every parameter: the MCP argument names, which are the CLI flag names without `--` and with
/// `_` for `-` (#389 R2).
pub(crate) const PARAMETERS: [&str; 19] = [
    "instance",
    "view",
    "since_unix_ms",
    "until_unix_ms",
    "after_sequence",
    "through_sequence",
    "event_type",
    "exclude_event_types",
    "min_severity",
    "max_severity",
    "origin_module",
    "task_id",
    "run_id",
    "request_id",
    "correlation_id",
    "detail",
    "limit",
    "cursor",
    "wait_s",
];
const INTEGER_PARAMETERS: [&str; 6] = [
    "since_unix_ms",
    "until_unix_ms",
    "after_sequence",
    "through_sequence",
    "limit",
    "wait_s",
];
/// Parameters that take a list: an array in MCP, a comma-separated value on the CLI.
const LIST_PARAMETERS: [&str; 1] = ["exclude_event_types"];
pub(crate) const DEFAULT_LIMIT: u16 = 20;
/// The CLI's largest page: the Runtime's page bound (§2.2; MCP stops at 100).
pub(crate) const CLI_MAX_LIMIT: u16 = MAX_RUNTIME_EVENT_QUERY_EVENTS;
/// The longest wait for rows, CLI and MCP alike.
pub(crate) const MAX_WAIT_S: u64 = 20;
/// The largest value an unbounded integer parameter (a time, a sequence) takes, here and in the
/// other commands: an outcome carries an argument's range as i64 values (`range_max`,
/// Workflow #378 A5), and no time or sequence comes near it.
pub(crate) const MAX_INTEGER: u64 = i64::MAX.unsigned_abs();
/// An empty first page is read again this often while the wait lasts.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const CLI_USAGE: &str = "usage: actingctl events --state-root <dir> [--instance <alias|instance_id|ADB port>] [--view events|observation|changes|errors|health|lab] [--since-unix-ms <ms>] [--until-unix-ms <ms>] [--after-sequence <seq> | --cursor <cursor>] [--through-sequence <seq>] [--event-type <type>] [--exclude-event-types <type,...>] [--min-severity <severity>] [--max-severity <severity>] [--origin-module <module>] [--task-id <id>] [--run-id <id>] [--request-id <id>] [--correlation-id <id>] [--detail brief|payload] [--limit 1-256] [--wait-s 0-20]\npage on with --after-sequence <last_sequence>: the next_cursor one call prints answers cursor_invalid in any later call";

/// What each row carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Detail {
    /// The row's identity, links, diagnostic code and material refs.
    Brief,
    /// The brief row and the event's projected payload, verbatim.
    Payload,
}

/// One read of `actingctl events` / `ac_events`, its parameters checked.
#[derive(Debug)]
pub(crate) struct Request {
    /// The instance selector, resolved by the caller with at most one Status.
    pub(crate) instance: Option<String>,
    /// Every filter but the instance and `after_sequence`: with the resolved instance, the query
    /// every cursor of this read is bound to.
    filters: EventQuery,
    after_sequence: Option<u64>,
    pub(crate) detail: Detail,
    limit: u16,
    cursor: Option<String>,
    pub(crate) wait: Duration,
}

impl Request {
    fn query(&self, instance_id: Option<InstanceId>) -> EventQuery {
        EventQuery {
            instance_id,
            ..self.filters.clone()
        }
    }
}

/// What is wrong with one argument or flag. It is the value `argument_problem` (`token`), with
/// `range_min` and `range_max` for a range (`range`); its text is only the developer line beside
/// the usage code (`argument <field> <problem>`). The code is decided by the frontend:
/// `actingctl_usage_invalid` on the CLI, `arguments_invalid` in MCP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArgumentProblem {
    /// Not a parameter of `actingctl events` / `ac_events`.
    UnknownArgument,
    /// Not a flag of `actingctl events`.
    UnknownFlag,
    /// A CLI flag or value that is not UTF-8.
    NotUtf8,
    /// A CLI flag without its value.
    ValueMissing,
    /// A CLI flag given more than once.
    GivenTwice,
    /// A required CLI flag that is absent.
    Required,
    /// Not a non-empty string.
    NotText,
    /// A CLI value that is not an unsigned integer.
    NotUnsigned,
    /// An integer outside its range.
    OutOfRange { minimum: u64, maximum: u64 },
    /// Text that is none of its type's spellings; the type, as in "a ledger view".
    NotA(&'static str),
    /// `detail` other than `brief` or `payload`.
    DetailUnknown,
    /// `exclude_event_types` that is not a list of event types.
    NotEventTypes,
    /// `cursor` together with `after_sequence`.
    CursorWithAfterSequence,
}

impl std::fmt::Display for ArgumentProblem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownArgument => formatter.write_str("is not an argument of this tool"),
            Self::UnknownFlag => formatter.write_str("is not a flag of actingctl events"),
            Self::NotUtf8 => formatter.write_str("is not UTF-8"),
            Self::ValueMissing => formatter.write_str("needs a value"),
            Self::GivenTwice => formatter.write_str("is given twice"),
            Self::Required => formatter.write_str("is required"),
            Self::NotText => formatter.write_str("must be a non-empty string"),
            Self::NotUnsigned => formatter.write_str("must be an unsigned integer"),
            Self::OutOfRange { minimum, maximum } => {
                write!(formatter, "must be an integer from {minimum} to {maximum}")
            }
            Self::NotA(what) => write!(formatter, "is not {what}"),
            Self::DetailUnknown => formatter.write_str("must be brief or payload"),
            Self::NotEventTypes => formatter.write_str("must list event types"),
            Self::CursorWithAfterSequence => formatter.write_str(
                "and after_sequence exclude each other: a cursor already holds where its pages go on",
            ),
        }
    }
}

impl ArgumentProblem {
    /// The problem's token of the vocabulary `argument_problem`.
    pub(crate) const fn token(self) -> codes::ArgumentProblem {
        match self {
            Self::UnknownArgument | Self::UnknownFlag => codes::ArgumentProblem::Unknown,
            Self::NotUtf8 => codes::ArgumentProblem::NotUtf8,
            Self::ValueMissing => codes::ArgumentProblem::ValueMissing,
            Self::GivenTwice => codes::ArgumentProblem::Repeated,
            Self::Required => codes::ArgumentProblem::Required,
            Self::NotText | Self::NotUnsigned => codes::ArgumentProblem::WrongType,
            Self::OutOfRange { .. } => codes::ArgumentProblem::OutOfRange,
            Self::NotA(_) | Self::DetailUnknown | Self::NotEventTypes => {
                codes::ArgumentProblem::NotRecognised
            }
            Self::CursorWithAfterSequence => codes::ArgumentProblem::Conflicting,
        }
    }

    /// The range an out-of-range value had to be in: `(range_min, range_max)`.
    pub(crate) const fn range(self) -> Option<(u64, u64)> {
        match self {
            Self::OutOfRange { minimum, maximum } => Some((minimum, maximum)),
            _ => None,
        }
    }
}

/// Why `actingctl events` / `ac_events` gives no page.
#[derive(Debug)]
pub(crate) enum EventsError {
    /// An argument or flag is unknown, repeated, of the wrong type or out of its range.
    Argument {
        field: String,
        problem: ArgumentProblem,
    },
    /// The instance selector names no single instance.
    Selector {
        miss: SelectorMiss,
        selector: String,
    },
    /// The filters break the query rule the Runtime holds every query to
    /// (`EventQuery::validate`): its code, as the Runtime would refuse it.
    Query(SanitizationError),
    /// The cursor belongs to another query, Runtime connection or process.
    CursorInvalid,
    /// The next page's cursor could not be formed: the contract's refusal of it, when it gave
    /// one (`None`: the Runtime's cursor carries no query fingerprint).
    CursorEncode(Option<Code>),
    /// The Runtime read failed.
    Runtime(RuntimeClientError),
}

impl EventsError {
    fn argument(field: &str, problem: ArgumentProblem) -> Self {
        Self::Argument {
            field: field.to_owned(),
            problem,
        }
    }

    /// The failure as an outcome (Workflow #378 A5). An argument or flag is the CLI's
    /// `actingctl_usage_invalid` with `argument`, `argument_problem` and, for a range,
    /// `range_min` and `range_max`; MCP answers `arguments_invalid` with the same values.
    pub(crate) fn outcome(&self) -> Outcome {
        match self {
            Self::Argument { field, problem } => {
                let outcome = Outcome::new(ActingctlCode::ActingctlUsageInvalid)
                    .argument(field.as_str())
                    .argument_problem(problem.token());
                match problem.range() {
                    Some((minimum, maximum)) => outcome.range_min(minimum).range_max(maximum),
                    None => outcome,
                }
            }
            Self::Selector { miss, selector } => miss.outcome(selector),
            Self::Query(error) => Outcome::new(error.code()),
            Self::CursorInvalid => Outcome::new(ActingctlCode::CursorInvalid),
            Self::CursorEncode(cause) => {
                let outcome = Outcome::new(ActingctlCode::CursorEncodeFailed);
                match cause {
                    Some(cause) => outcome.caused_by(Outcome::new(*cause)),
                    None => outcome,
                }
            }
            Self::Runtime(error) => error.outcome(),
        }
    }
}

/// The developer line's text of an argument or flag failure: "argument <field> <problem>".
impl std::fmt::Display for EventsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Argument { field, problem } => write!(formatter, "argument {field} {problem}"),
            other => write!(formatter, "{}", other.outcome()),
        }
    }
}

/// Checks every parameter, MCP arguments or CLI flags turned into the same names and JSON
/// types; `max_limit` is the frontend's largest page (§2.2). The filters are checked here as
/// the Runtime checks them, so a query it would refuse never reaches it.
pub(crate) fn parse(
    arguments: &Map<String, Value>,
    max_limit: u16,
) -> Result<Request, EventsError> {
    if let Some(unknown) = arguments
        .keys()
        .find(|key| !PARAMETERS.contains(&key.as_str()))
    {
        return Err(EventsError::argument(
            unknown,
            ArgumentProblem::UnknownArgument,
        ));
    }
    let detail = match text(arguments, "detail")? {
        None | Some("brief") => Detail::Brief,
        Some("payload") => Detail::Payload,
        Some(_) => {
            return Err(EventsError::argument(
                "detail",
                ArgumentProblem::DetailUnknown,
            ));
        }
    };
    let limit = integer(arguments, "limit", 1, u64::from(max_limit))?
        .map_or(Ok(DEFAULT_LIMIT), u16::try_from)
        .map_err(|_| {
            EventsError::argument(
                "limit",
                ArgumentProblem::OutOfRange {
                    minimum: 1,
                    maximum: u64::from(max_limit),
                },
            )
        })?;
    let wait = Duration::from_secs(integer(arguments, "wait_s", 0, MAX_WAIT_S)?.unwrap_or(0));
    let after_sequence = integer(arguments, "after_sequence", 0, MAX_INTEGER)?;
    let cursor = text(arguments, "cursor")?.map(str::to_owned);
    if cursor.is_some() && after_sequence.is_some() {
        return Err(EventsError::argument(
            "cursor",
            ArgumentProblem::CursorWithAfterSequence,
        ));
    }
    let exclude_event_types = match arguments.get("exclude_event_types") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(types)) => types
            .iter()
            .map(|event_type| {
                event_type
                    .as_str()
                    .and_then(parsed::<EventType>)
                    .ok_or_else(|| {
                        EventsError::argument("exclude_event_types", ArgumentProblem::NotEventTypes)
                    })
            })
            .collect::<Result<Vec<EventType>, _>>()?,
        Some(_) => {
            return Err(EventsError::argument(
                "exclude_event_types",
                ArgumentProblem::NotEventTypes,
            ));
        }
    };
    let filters = EventQuery {
        view: typed::<LedgerView>(arguments, "view", "a ledger view")?,
        from_timestamp_unix_ms: integer(arguments, "since_unix_ms", 0, MAX_INTEGER)?,
        to_timestamp_unix_ms: integer(arguments, "until_unix_ms", 0, MAX_INTEGER)?,
        to_sequence: integer(arguments, "through_sequence", 0, MAX_INTEGER)?,
        event_type: typed::<EventType>(arguments, "event_type", "an event type")?,
        exclude_event_types,
        minimum_severity: typed::<EventSeverity>(arguments, "min_severity", "a severity")?,
        maximum_severity: typed::<EventSeverity>(arguments, "max_severity", "a severity")?,
        origin_module: typed::<OriginModule>(arguments, "origin_module", "an origin module")?,
        task_id: typed::<TaskId>(arguments, "task_id", "a Runtime task_id")?,
        run_id: typed::<RunId>(arguments, "run_id", "a Runtime run_id")?,
        request_id: typed::<RequestId>(arguments, "request_id", "a Runtime request_id")?,
        correlation_id: typed::<CorrelationId>(
            arguments,
            "correlation_id",
            "a Runtime correlation_id",
        )?,
        ..EventQuery::default()
    };
    filters.validate().map_err(EventsError::Query)?;
    Ok(Request {
        instance: text(arguments, "instance")?.map(str::to_owned),
        filters,
        after_sequence,
        detail,
        limit,
        cursor,
        wait,
    })
}

fn text<'a>(arguments: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, EventsError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.is_empty() => Ok(Some(text.as_str())),
        Some(_) => Err(EventsError::argument(key, ArgumentProblem::NotText)),
    }
}

fn integer(
    arguments: &Map<String, Value>,
    key: &str,
    minimum: u64,
    maximum: u64,
) -> Result<Option<u64>, EventsError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|number| (minimum..=maximum).contains(number))
            .map(Some)
            .ok_or_else(|| {
                EventsError::argument(key, ArgumentProblem::OutOfRange { minimum, maximum })
            }),
    }
}

fn parsed<T: DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(json!(text)).ok()
}

fn typed<T: DeserializeOwned>(
    arguments: &Map<String, Value>,
    key: &str,
    what: &'static str,
) -> Result<Option<T>, EventsError> {
    text(arguments, key)?
        .map(|text| {
            parsed(text).ok_or_else(|| EventsError::argument(key, ArgumentProblem::NotA(what)))
        })
        .transpose()
}

/// Binds a cursor to the process and the Runtime connection that issued it:
/// `v1.<session>.<connection>.<snapshot>.<after>.<query fingerprint>`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CursorBinding {
    pub(crate) session: u64,
    pub(crate) generation: u64,
}

impl CursorBinding {
    pub(crate) fn encode(self, cursor: &RuntimeEventQueryCursor) -> Result<String, EventsError> {
        let fingerprint = json!(cursor)
            .get("query_fingerprint")
            .and_then(Value::as_str)
            .and_then(|fingerprint| fingerprint.strip_prefix("sha256:"))
            .map(str::to_owned)
            .ok_or(EventsError::CursorEncode(None))?;
        Ok(format!(
            "v1.{:016x}.{}.{}.{}.{fingerprint}",
            self.session,
            self.generation,
            cursor.snapshot_ledger_position(),
            cursor.after_sequence()
        ))
    }

    /// The Runtime cursor of `token` when this process and connection issued it for `query`.
    fn decode(self, token: &str, query: &EventQuery) -> Option<RuntimeEventQueryCursor> {
        let parts = token.split('.').collect::<Vec<_>>();
        let [version, session, generation, snapshot, after, fingerprint] = parts.as_slice() else {
            return None;
        };
        if *version != "v1"
            || u64::from_str_radix(session, 16).ok() != Some(self.session)
            || generation.parse::<u64>().ok() != Some(self.generation)
        {
            return None;
        }
        let cursor = serde_json::from_value::<RuntimeEventQueryCursor>(json!({
            "snapshot_ledger_position": snapshot.parse::<u64>().ok()?,
            "after_sequence": after.parse::<u64>().ok()?,
            "query_fingerprint": format!("sha256:{fingerprint}"),
        }))
        .ok()?;
        (cursor.validate().is_ok() && cursor.matches(query, PROFILE).unwrap_or(false))
            .then_some(cursor)
    }
}

/// One page as the Runtime answered it, with the query its cursors are bound to.
pub(crate) struct Page {
    page: RuntimeEventQueryPage,
    query: EventQuery,
    /// With a wait: how long the answered read started after the first one.
    waited_ms: Option<u64>,
}

impl Page {
    pub(crate) fn events(&self) -> &[ProjectedEvent] {
        self.page.events()
    }

    /// The cursor that goes on after `after` within this page's snapshot. It is bound to the
    /// query without `after_sequence`, so that it is sent back without it.
    pub(crate) fn cursor_after(&self, after: u64) -> Result<RuntimeEventQueryCursor, EventsError> {
        RuntimeEventQueryCursor::new(
            self.page.snapshot_ledger_position(),
            after,
            &self.query,
            PROFILE,
        )
        .map_err(|error| EventsError::CursorEncode(Some(error.code())))
    }

    /// Where the Runtime's next page starts; `None` once the snapshot is read to its end.
    pub(crate) fn next_cursor(&self) -> Result<Option<RuntimeEventQueryCursor>, EventsError> {
        self.page
            .next_cursor()
            .map(|next| self.cursor_after(next.after_sequence()))
            .transpose()
    }

    /// `{events, next_cursor?, truncated?, snapshot_ledger_position, last_sequence?,
    /// waited_ms?}` with these rows; `last_sequence` is the seq of the last of them, for the
    /// next `after_sequence`.
    pub(crate) fn result(
        &self,
        rows: &[Value],
        next_cursor: Option<String>,
        truncated: bool,
    ) -> Value {
        let mut result = Map::new();
        result.insert("events".to_owned(), Value::Array(rows.to_vec()));
        if let Some(cursor) = next_cursor {
            result.insert("next_cursor".to_owned(), json!(cursor));
        }
        if truncated {
            result.insert("truncated".to_owned(), json!(true));
        }
        result.insert(
            "snapshot_ledger_position".to_owned(),
            json!(self.page.snapshot_ledger_position()),
        );
        if let Some(last) = rows.last().and_then(|row| row.get("seq")) {
            result.insert("last_sequence".to_owned(), last.clone());
        }
        if let Some(waited_ms) = self.waited_ms {
            result.insert("waited_ms".to_owned(), json!(waited_ms));
        }
        Value::Object(result)
    }
}

/// Reads one page of `request` with the resolved instance. A page from `after_sequence` starts
/// at the next sequence; a cursor resumes where its page ended. With a wait, an empty page is
/// read again every second until rows arrive, `wait_until` comes or `stop` says so.
pub(crate) fn read(
    client: &RuntimeClient,
    request: &Request,
    instance_id: Option<InstanceId>,
    binding: CursorBinding,
    wait_until: Instant,
    stop: &dyn Fn() -> bool,
) -> Result<Page, EventsError> {
    let query = request.query(instance_id);
    let resume = request
        .cursor
        .as_deref()
        .map(|token| {
            binding
                .decode(token, &query)
                .ok_or(EventsError::CursorInvalid)
        })
        .transpose()?;
    let sent = match request.after_sequence {
        Some(after) => EventQuery {
            from_sequence: Some(after + 1),
            ..query.clone()
        },
        None => query.clone(),
    };
    let page_request = RuntimeEventQueryPageRequest::new(request.limit, resume)
        .map_err(|_| EventsError::CursorInvalid)?;
    let waiting = !request.wait.is_zero();
    let first = Instant::now();
    loop {
        let started = Instant::now();
        let page = client
            .query_event_page(sent.clone(), PROFILE, page_request.clone())
            .map_err(EventsError::Runtime)?;
        if !waiting
            || !page.events().is_empty()
            || stop()
            || Instant::now() + POLL_INTERVAL >= wait_until
        {
            let waited_ms =
                u64::try_from(started.duration_since(first).as_millis()).unwrap_or(u64::MAX);
            return Ok(Page {
                page,
                query,
                waited_ms: waiting.then_some(waited_ms),
            });
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// A row of `detail`.
pub(crate) fn row(event: &ProjectedEvent, detail: Detail) -> Value {
    let mut row = brief_row(event);
    if detail == Detail::Payload {
        row["payload"] = json!(event.payload);
    }
    row
}

/// `{seq, ts, type, severity, instance?, request_id?, run_id?, task_id?, correlation_id?,
/// code?, materials?}`: the ledger's diagnostic code and the material refs for `ac_material`.
pub(crate) fn brief_row(event: &ProjectedEvent) -> Value {
    let mut row = Map::new();
    row.insert("seq".to_owned(), json!(event.sequence));
    row.insert("ts".to_owned(), json!(event.timestamp_unix_ms));
    row.insert("type".to_owned(), json!(event.event_type));
    row.insert("severity".to_owned(), json!(event.severity));
    let links = &event.links;
    if let Some(instance_id) = links.instance_id() {
        row.insert("instance".to_owned(), json!(instance_id));
    }
    if let Some(request_id) = links.request_id() {
        row.insert("request_id".to_owned(), json!(request_id));
    }
    if let Some(run_id) = links.run_id() {
        row.insert("run_id".to_owned(), json!(run_id));
    }
    if let Some(task_id) = links.task_id() {
        row.insert("task_id".to_owned(), json!(task_id));
    }
    if let Some(correlation_id) = links.correlation_id() {
        row.insert("correlation_id".to_owned(), json!(correlation_id));
    }
    let code = match &event.payload {
        ProjectionPayload::Full(payload) => payload.diagnostic_code(),
        _ => None,
    };
    if let Some(code) = code {
        row.insert("code".to_owned(), json!(code.as_str()));
    }
    if !event.artifacts.is_empty() {
        let materials = event
            .artifacts
            .iter()
            .map(|artifact| {
                json!({
                    "ref": material_export::material_ref(event, artifact),
                    "media_type": artifact.media_type,
                    "size": artifact.byte_count,
                })
            })
            .collect();
        row.insert("materials".to_owned(), Value::Array(materials));
    }
    Value::Object(row)
}

/// `actingctl events …` when the first argument is `events`: one result line on stdout
/// (Workflow #378 A5), the page with `runtime_events_reported`, or `{outcome}` with the failure
/// (an argument failure's developer line adds the usage) and exit 1.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != "events" {
        return None;
    }
    Some(match run_cli(&arguments[1..]) {
        Ok(page) => cli_line::success(
            cli_line::members(page, "events"),
            &Outcome::new(ActingctlCode::RuntimeEventsReported),
        ),
        Err(error @ EventsError::Argument { .. }) => cli_line::failure(
            &error.outcome(),
            Some(format!("{error}\n{CLI_USAGE}").as_str()),
        ),
        Err(error) => cli_line::failure(&error.outcome(), None),
    })
}

fn run_cli(arguments: &[OsString]) -> Result<Value, EventsError> {
    let (state_root, parameters) = cli_parameters(arguments)?;
    let request = parse(&parameters, CLI_MAX_LIMIT)?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &state_root,
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(EventsError::Runtime)?;
    // One Status for the whole call, never one per read of the wait (§2.5).
    let instance_id = match &request.instance {
        Some(selector) => {
            let status = client.status().map_err(EventsError::Runtime)?;
            let instance =
                instance_selector::select(status.instances(), selector).map_err(|miss| {
                    EventsError::Selector {
                        miss,
                        selector: selector.clone(),
                    }
                })?;
            Some(instance.instance_id())
        }
        None => None,
    };
    // This process holds one Runtime connection.
    let binding = CursorBinding {
        session: process_session(),
        generation: 1,
    };
    let page = read(
        &client,
        &request,
        instance_id,
        binding,
        Instant::now() + request.wait,
        &|| false,
    )?;
    let rows = page
        .events()
        .iter()
        .map(|event| row(event, request.detail))
        .collect::<Vec<_>>();
    let next_cursor = page
        .next_cursor()?
        .map(|cursor| binding.encode(&cursor))
        .transpose()?;
    Ok(page.result(&rows, next_cursor, false))
}

/// `--state-root` and every other flag under its parameter name, typed as MCP types it:
/// integers as numbers, lists split at commas, the rest as strings. Every flag takes a value and
/// may be given once.
fn cli_parameters(arguments: &[OsString]) -> Result<(PathBuf, Map<String, Value>), EventsError> {
    let mut state_root = None;
    let mut parameters = Map::new();
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index]
            .to_str()
            .ok_or_else(|| EventsError::argument("flag", ArgumentProblem::NotUtf8))?;
        let name = flag
            .strip_prefix("--")
            .filter(|name| !name.contains('_'))
            .ok_or_else(|| EventsError::argument(flag, ArgumentProblem::UnknownFlag))?;
        // Workflow #382 A1-FLAG: a flag `events` does not take is unknown, with or without a
        // value after it.
        let key = name.replace('-', "_");
        if name != "state-root" && !PARAMETERS.contains(&key.as_str()) {
            return Err(EventsError::argument(flag, ArgumentProblem::UnknownFlag));
        }
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| EventsError::argument(flag, ArgumentProblem::ValueMissing))?;
        index += 2;
        if name == "state-root" {
            if state_root.replace(PathBuf::from(value)).is_some() {
                return Err(EventsError::argument(flag, ArgumentProblem::GivenTwice));
            }
            continue;
        }
        let text = value
            .to_str()
            .ok_or_else(|| EventsError::argument(flag, ArgumentProblem::NotUtf8))?;
        let typed = if INTEGER_PARAMETERS.contains(&key.as_str()) {
            json!(
                text.parse::<u64>()
                    .map_err(|_| EventsError::argument(flag, ArgumentProblem::NotUnsigned))?
            )
        } else if LIST_PARAMETERS.contains(&key.as_str()) {
            Value::Array(text.split(',').map(|item| json!(item)).collect())
        } else {
            json!(text)
        };
        if parameters.insert(key, typed).is_some() {
            return Err(EventsError::argument(flag, ArgumentProblem::GivenTwice));
        }
    }
    let state_root = state_root
        .ok_or_else(|| EventsError::argument("--state-root", ArgumentProblem::Required))?;
    Ok((state_root, parameters))
}

/// Distinct per process: a cursor this CLI printed answers `cursor_invalid` in any other
/// (`actingctl diagnose` binds its errors page cursor the same way, Workflow #389 MCP-7).
pub(crate) fn process_session() -> u64 {
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    RandomState::new().hash_one((process::id(), started))
}

// Test-only: Workflow #378 A5, an argument failure of `events` as an outcome.
#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::outcome::Category;

    #[test]
    fn each_argument_problem_of_events_is_the_usage_outcome_with_its_argument_values() {
        let cases = [
            (ArgumentProblem::UnknownArgument, "unknown", None),
            (ArgumentProblem::UnknownFlag, "unknown", None),
            (ArgumentProblem::NotUtf8, "not_utf8", None),
            (ArgumentProblem::ValueMissing, "value_missing", None),
            (ArgumentProblem::GivenTwice, "repeated", None),
            (ArgumentProblem::Required, "required", None),
            (ArgumentProblem::NotText, "wrong_type", None),
            (ArgumentProblem::NotUnsigned, "wrong_type", None),
            (
                ArgumentProblem::OutOfRange {
                    minimum: 1,
                    maximum: 256,
                },
                "out_of_range",
                Some((1_u64, 256_u64)),
            ),
            (
                ArgumentProblem::NotA("a ledger view"),
                "not_recognised",
                None,
            ),
            (ArgumentProblem::DetailUnknown, "not_recognised", None),
            (ArgumentProblem::NotEventTypes, "not_recognised", None),
            (
                ArgumentProblem::CursorWithAfterSequence,
                "conflicting",
                None,
            ),
        ];
        for (problem, token, range) in cases {
            let outcome = EventsError::argument("limit", problem).outcome();
            assert!(
                outcome.is(ActingctlCode::ActingctlUsageInvalid),
                "{problem:?}: {outcome}"
            );
            assert_eq!(
                outcome.category(),
                Category::Error,
                "{problem:?}: {outcome}"
            );
            assert_eq!(
                outcome.value("argument"),
                Some(&json!("limit")),
                "{problem:?}: {outcome}"
            );
            assert_eq!(
                outcome.value("argument_problem"),
                Some(&json!(token)),
                "{problem:?}: {outcome}"
            );
            assert_eq!(
                (
                    outcome.value("range_min").cloned(),
                    outcome.value("range_max").cloned()
                ),
                (
                    range.map(|(minimum, _)| json!(minimum)),
                    range.map(|(_, maximum)| json!(maximum))
                ),
                "{problem:?}: {outcome}"
            );
        }
    }
}
