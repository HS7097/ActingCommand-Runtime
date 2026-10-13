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

use crate::codes::ActingctlCode;
use crate::instance_selector::{self, SelectorError};
use actingcommand_contract::outcome::Code;
use actingcommand_contract::{
    CorrelationId, EventActor, EventQuery, EventSeverity, EventSource, EventType, InstanceId,
    LedgerView, MAX_RUNTIME_EVENT_QUERY_EVENTS, OriginModule, ProjectedArtifactReference,
    ProjectedEvent, ProjectionPayload, ProjectionProfile, RequestId, RunId,
    RuntimeEventQueryCursor, RuntimeEventQueryPage, RuntimeEventQueryPageRequest,
    SanitizationError, TaskId,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use std::collections::hash_map::RandomState;
use std::ffi::OsString;
use std::hash::BuildHasher;
use std::io::{self, Write};
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
/// An empty first page is read again this often while the wait lasts.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const CLI_USAGE: &str = "usage: actingctl events --state-root <dir> [--instance <alias|instance_id|ADB port>] [--view events|observation|changes|errors|health|lab] [--since-unix-ms <ms>] [--until-unix-ms <ms>] [--after-sequence <seq> | --cursor <cursor>] [--through-sequence <seq>] [--event-type <type>] [--exclude-event-types <type,...>] [--min-severity <severity>] [--max-severity <severity>] [--origin-module <module>] [--task-id <id>] [--run-id <id>] [--request-id <id>] [--correlation-id <id>] [--detail brief|payload] [--limit 1-256] [--wait-s 0-20]";

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

/// What is wrong with one argument or flag. Its text is only the developer message beside the
/// usage code (`argument <field> <problem>`); the code is decided by the frontend.
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
        error: SelectorError,
        selector: String,
    },
    /// The filters break the query rule the Runtime holds every query to
    /// (`EventQuery::validate`): its code, as the Runtime would refuse it.
    Query(SanitizationError),
    /// The cursor belongs to another query, Runtime connection or process.
    CursorInvalid,
    /// The next page's cursor could not be formed.
    CursorEncode(String),
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

    /// The message beside the code; for an argument, "argument <field> <problem>".
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Argument { field, problem } => format!("argument {field} {problem}"),
            Self::Selector { error, selector } => error.message(selector),
            Self::Query(error) => format!("the Runtime refuses this event query: {error}"),
            Self::CursorInvalid => "this cursor belongs to another query, Runtime connection or process; read again from the first page, or from after_sequence, without a cursor".to_owned(),
            Self::CursorEncode(reason) => format!("cannot form the next cursor: {reason}"),
            Self::Runtime(error) => error.to_string(),
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
    let after_sequence = integer(arguments, "after_sequence", 0, u64::MAX - 1)?;
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
        from_timestamp_unix_ms: integer(arguments, "since_unix_ms", 0, u64::MAX)?,
        to_timestamp_unix_ms: integer(arguments, "until_unix_ms", 0, u64::MAX)?,
        to_sequence: integer(arguments, "through_sequence", 0, u64::MAX)?,
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
            .ok_or_else(|| {
                EventsError::CursorEncode(
                    "the Runtime cursor carries no query fingerprint".to_owned(),
                )
            })?;
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
        .map_err(|error| EventsError::CursorEncode(error.to_string()))
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
                    "ref": material_ref(event, artifact),
                    "media_type": artifact.media_type,
                    "size": artifact.byte_count,
                })
            })
            .collect();
        row.insert("materials".to_owned(), Value::Array(materials));
    }
    Value::Object(row)
}

/// `<seq>:<event_id>:<artifact_id>`: enough to find the material again in the ledger.
fn material_ref(event: &ProjectedEvent, artifact: &ProjectedArtifactReference) -> String {
    format!(
        "{}:{}:{}",
        event.sequence,
        text_of(json!(event.event_id)),
        text_of(json!(artifact.artifact_id))
    )
}

fn text_of(value: Value) -> String {
    match value {
        Value::String(text) => text,
        other => other.to_string(),
    }
}

/// `actingctl events …` when the first argument is `events`: one page as one line of JSON on
/// stdout, or `FATAL actingctl: <code>: <message>` on stderr and exit 1.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != "events" {
        return None;
    }
    let printed = run_cli(&arguments[1..]).and_then(|page| {
        let mut stdout = io::stdout().lock();
        serde_json::to_writer(&mut stdout, &page)
            .map_err(io::Error::from)
            .and_then(|()| stdout.write_all(b"\n"))
            .and_then(|()| stdout.flush())
            .map_err(|error| CliError::Output(error.to_string()))
    });
    Some(match printed {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FATAL actingctl: {error}");
            ExitCode::FAILURE
        }
    })
}

enum CliError {
    Events(EventsError),
    Output(String),
}

impl From<EventsError> for CliError {
    fn from(error: EventsError) -> Self {
        Self::Events(error)
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The client error's own text leads with its code.
            Self::Events(EventsError::Runtime(error)) => std::fmt::Display::fmt(error, formatter),
            Self::Events(error @ EventsError::Argument { .. }) => {
                write!(
                    formatter,
                    "{}: {}\n{CLI_USAGE}",
                    cli_code(error),
                    error.message()
                )
            }
            Self::Events(error) => write!(formatter, "{}: {}", cli_code(error), error.message()),
            Self::Output(reason) => write!(
                formatter,
                "{}: cannot write the page to stdout: {reason}",
                ActingctlCode::ActingctlOutputFailed.code()
            ),
        }
    }
}

/// The code of a CLI failure: argument errors are actingctl's usage code (the MCP tool answers
/// `arguments_invalid` for them), the others the same codes as the MCP tool.
fn cli_code(error: &EventsError) -> Code {
    match error {
        EventsError::Argument { .. } => ActingctlCode::ActingctlUsageInvalid.code(),
        EventsError::Selector { error, .. } => error.code(),
        EventsError::Query(error) => error.code(),
        EventsError::CursorInvalid => ActingctlCode::CursorInvalid.code(),
        EventsError::CursorEncode(_) => ActingctlCode::CursorEncodeFailed.code(),
        EventsError::Runtime(error) => error.code(),
    }
}

fn run_cli(arguments: &[OsString]) -> Result<Value, CliError> {
    let (state_root, parameters) = cli_parameters(arguments)?;
    let request = parse(&parameters, CLI_MAX_LIMIT)?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &state_root,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(EventsError::Runtime)?;
    // One Status for the whole call, never one per read of the wait (§2.5).
    let instance_id = match &request.instance {
        Some(selector) => {
            let status = client.status().map_err(EventsError::Runtime)?;
            let instance =
                instance_selector::select(status.instances(), selector).map_err(|error| {
                    EventsError::Selector {
                        error,
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
        let key = name.replace('-', "_");
        if !PARAMETERS.contains(&key.as_str()) {
            return Err(EventsError::argument(flag, ArgumentProblem::UnknownFlag));
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

/// Distinct per process: a cursor this CLI printed answers `cursor_invalid` in any other.
fn process_session() -> u64 {
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    RandomState::new().hash_one((process::id(), started))
}
