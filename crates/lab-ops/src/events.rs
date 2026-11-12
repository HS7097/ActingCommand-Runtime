// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl events` and what it shares with the MCP tool `ac_events` (Workflow #389 MCP-1;
//! MCP-0 command table v1.1 §4.1): the parameters, the Runtime query, the wait for rows, the
//! rows and the page. Both read with QueryEvents, which records nothing in the ledger, and
//! resolve an instance selector with at most one Status per call. The CLI prints the page
//! whole; the MCP tool (`mcp::events`) adds only what §2.2 lets it differ in. U-3 generates
//! both from the command table; until then this module keeps their parameters and output the
//! same.
//!
//! `detail` is `brief`, `summary` or `payload`. `summary`, the MCP-1 follow-up after A3 (#721,
//! coordinator ruling Q4), adds the outcome a record carries since ledger revision 3.
//!
//! Paging. A cursor is bound to the process and the Runtime connection that issued it (§4.1).
//! MCP pages with `next_cursor` inside its server's connection. The CLI pages with
//! `--after-sequence <last_sequence>`: every `actingctl events` call is its own process, so the
//! `next_cursor` one call prints always answers `cursor_invalid` in the next call, by design;
//! `last_sequence` stays valid across calls and Runtime restarts.
//!
//! The newest records (Workflow #434 CLI-EV, register §1.306). A read that names no position and
//! no window (no `after_sequence`, cursor, `through_sequence`, `since_unix_ms` or `until_unix_ms`,
//! and no `run_id`, `request_id` or `correlation_id`) answers the newest `limit` records its other
//! filters match, oldest first within the page, with `first_sequence` (read the records before it
//! with `through_sequence <first_sequence - 1>`) and, when it found fewer than `limit` and older
//! records were not searched, `searched_from_sequence`, the lowest sequence it searched. It reads
//! the ledger's head once, then windows that end at the head and widen twice to a largest width,
//! every read bounded by sequence at the head's snapshot ([`NEWEST_WINDOWS`]). With any of those
//! members a read is as before: the page from that position or window on, oldest first.

use crate::budget::{self, Budget, Fitted};
use crate::cli_line;
use crate::codes::{self, ActingctlCode};
use crate::front::Connection;
use crate::instance_selector::{self, SelectorMiss};
use crate::material_export;
use actingcommand_contract::outcome::{Code, Detail as OutcomeDetail, Outcome};
use actingcommand_contract::{
    CorrelationId, EventQuery, EventSeverity, EventType, InstanceId, LedgerView,
    MAX_RUNTIME_EVENT_QUERY_EVENTS, OriginModule, ProjectedEvent, ProjectionPayload,
    ProjectionProfile, RequestId, RunId, RuntimeEventQueryCursor, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest, SanitizationError, TaskId,
};
use actingcommand_runtime_client::{ClientCode, RuntimeClient, RuntimeClientError};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use std::collections::hash_map::RandomState;
use std::convert::identity;
use std::ffi::OsString;
use std::hash::BuildHasher;
use std::path::PathBuf;
use std::process;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Rows carry their full payloads, where the ledger's diagnostic code lives.
pub const PROFILE: ProjectionProfile = ProjectionProfile::Forensic;
/// The subcommand; its flags are `flags_of`'s (Workflow #382 A1-FLAG, #389 MCP-2).
pub const COMMAND: &str = "events";
/// Every parameter: the MCP argument names, which are the CLI flag names without `--` and with
/// `_` for `-` (#389 R2). Workflow #389 MCP-2 adds `event_types` and `fact_key` (§4.2).
pub const PARAMETERS: [&str; 21] = [
    "instance",
    "view",
    "since_unix_ms",
    "until_unix_ms",
    "after_sequence",
    "through_sequence",
    "event_type",
    "event_types",
    "exclude_event_types",
    "fact_key",
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
const LIST_PARAMETERS: [&str; 2] = ["event_types", "exclude_event_types"];
pub const DEFAULT_LIMIT: u16 = 20;
/// The CLI's largest page: the Runtime's page bound (§2.2; MCP stops at 100).
pub const CLI_MAX_LIMIT: u16 = MAX_RUNTIME_EVENT_QUERY_EVENTS;
/// The longest wait for rows, CLI and MCP alike.
pub const MAX_WAIT_S: u64 = 20;
/// The largest value an unbounded integer parameter (a time, a sequence) takes, here and in the
/// other commands: an outcome carries an argument's range as i64 values (`range_max`,
/// Workflow #378 A5), and no time or sequence comes near it.
pub const MAX_INTEGER: u64 = i64::MAX.unsigned_abs();
/// An empty first page is read again this often while the wait lasts.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const CLI_USAGE: &str = "usage: actingctl events --state-root <dir> [--instance <alias|instance_id|ADB port>] [--view events|observation|changes|errors|health|lab] [--since-unix-ms <ms>] [--until-unix-ms <ms>] [--after-sequence <seq> | --cursor <cursor>] [--through-sequence <seq>] [--event-type <type> | --event-types <type,...>] [--exclude-event-types <type,...>] [--fact-key <key>] [--min-severity <severity>] [--max-severity <severity>] [--origin-module <module>] [--task-id <id>] [--run-id <id>] [--request-id <id>] [--correlation-id <id>] [--detail brief|summary|payload] [--limit 1-256] [--wait-s 0-20]\nwith none of --after-sequence, --cursor, --through-sequence, --since-unix-ms, --until-unix-ms, --run-id, --request-id and --correlation-id the page is the newest records, oldest first: read the records before them with --through-sequence <first_sequence - 1>\npage on with --after-sequence <last_sequence>: the next_cursor one call prints answers cursor_invalid in any later call";

/// Workflow #434 CLI-EV (ruling 3; register §1.306): the windows of a read that names no position
/// and no window, in sequences, each ending at the ledger's head: the first, the factor of each of
/// the two widenings and the largest. Constants of the contract, not settings. QP-1 (register
/// §1.292) bounds every page by its sequence range: a page reads its range in statements whose
/// spans double from one sequence up to `ledger_observer.history_scan_rows` (65 536 by default),
/// each inside its span, so a range of `w` sequences takes at most `ceil(log2(w + 1))`
/// statements. The first window is the Runtime's page bound, the CLI's largest `limit`: where every
/// record matches it holds the page whatever the limit, in nine statements. The largest is that
/// default span: no read of the search spans more sequences than one full-size statement, at most
/// seventeen statements. Each widening adds four of the doublings.
pub const NEWEST_FIRST_WINDOW: u64 = MAX_RUNTIME_EVENT_QUERY_EVENTS as u64;
pub const NEWEST_WINDOW_FACTOR: u64 = 16;
pub const NEWEST_LARGEST_WINDOW: u64 = 65_536;
/// The three windows of the search: the first, widened twice.
pub const NEWEST_WINDOWS: [u64; 3] = [
    NEWEST_FIRST_WINDOW,
    NEWEST_FIRST_WINDOW * NEWEST_WINDOW_FACTOR,
    NEWEST_FIRST_WINDOW * NEWEST_WINDOW_FACTOR * NEWEST_WINDOW_FACTOR,
];
const _: () = assert!(NEWEST_WINDOWS[2] == NEWEST_LARGEST_WINDOW);
/// The search counts records and reads no payload; only the rows it found are read with theirs.
const SEARCH_PROFILE: ProjectionProfile = ProjectionProfile::Concise;

/// What each row carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// The row's identity, links, diagnostic code and material refs.
    Brief,
    /// The brief row and the outcome the record carries (ledger revision 3, #378 A3), when it
    /// carries one; a record without one is a state change and its row stays brief (§4.1).
    Summary,
    /// The brief row and the event's projected payload, verbatim.
    Payload,
}

/// One read of `actingctl events` / `ac_events`, its parameters checked.
#[derive(Debug)]
pub struct Request {
    /// The instance selector, resolved by the caller with at most one Status.
    pub instance: Option<String>,
    /// Every filter but the instance and `after_sequence`: with the resolved instance, the query
    /// every cursor of this read is bound to.
    filters: EventQuery,
    after_sequence: Option<u64>,
    pub detail: Detail,
    limit: u16,
    cursor: Option<String>,
    pub wait: Duration,
}

impl Request {
    fn query(&self, instance_id: Option<InstanceId>) -> EventQuery {
        EventQuery {
            instance_id,
            ..self.filters.clone()
        }
    }

    /// Workflow #434 LAB-5C: every filter of the read but the instance and `after_sequence`, as
    /// the query the Runtime is sent holds them.
    pub const fn filters(&self) -> &EventQuery {
        &self.filters
    }

    /// Workflow #434 CLI-EV (ruling 1): whether the read names no position and no window: no
    /// `after_sequence`, no cursor, no `through_sequence`, no `since_unix_ms` or `until_unix_ms`
    /// and no id that pins a run (`run_id`, `request_id`, `correlation_id`). Such a read answers
    /// the newest records its other filters match; any other reads as before.
    pub const fn newest(&self) -> bool {
        let filters = &self.filters;
        self.after_sequence.is_none()
            && self.cursor.is_none()
            && filters.from_sequence.is_none()
            && filters.to_sequence.is_none()
            && filters.from_timestamp_unix_ms.is_none()
            && filters.to_timestamp_unix_ms.is_none()
            && filters.run_id.is_none()
            && filters.request_id.is_none()
            && filters.correlation_id.is_none()
    }
}

/// What is wrong with one argument or flag. It is the value `argument_problem` (`token`), with
/// `range_min` and `range_max` for a range (`range`); its text is only the developer line beside
/// the usage code (`argument <field> <problem>`). The code is decided by the frontend:
/// `actingctl_usage_invalid` on the CLI, `arguments_invalid` in MCP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentProblem {
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
    /// `detail` other than `brief`, `summary` or `payload`.
    DetailUnknown,
    /// `event_types` or `exclude_event_types` that is not a list of event types.
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
            Self::DetailUnknown => formatter.write_str("must be brief, summary or payload"),
            Self::NotEventTypes => formatter.write_str("must list event types"),
            Self::CursorWithAfterSequence => formatter.write_str(
                "and after_sequence exclude each other: a cursor already holds where its pages go on",
            ),
        }
    }
}

impl ArgumentProblem {
    /// The problem's token of the vocabulary `argument_problem`.
    pub const fn token(self) -> codes::ArgumentProblem {
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
    pub const fn range(self) -> Option<(u64, u64)> {
        match self {
            Self::OutOfRange { minimum, maximum } => Some((minimum, maximum)),
            _ => None,
        }
    }
}

/// Why `actingctl events` / `ac_events` gives no page.
#[derive(Debug)]
pub enum EventsError {
    /// An argument or flag is unknown, repeated, of the wrong type or out of its range.
    /// Workflow #411 B27: `value` is the text an event type argument was given that names no
    /// event type (for a list, the element), when it is a word a name may hold.
    Argument {
        field: String,
        problem: ArgumentProblem,
        value: Option<String>,
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
    /// Workflow #434 LAB-5D: a payload page its caller's budget did not take could not be written
    /// whole to the caller's export: the export's outcome (`material_export_failed`).
    Export(Outcome),
    /// Workflow #434 CLI-EV: the Runtime's pages of a read of the newest records do not fit
    /// together (a page at another snapshot than the head the read pinned, a row outside the range
    /// it was asked for, the rows the search found not read back): the runtime client's
    /// `runtime_event_query_pagination_invalid`, which says the same of its own paged reads.
    Pagination,
}

impl EventsError {
    fn argument(field: &str, problem: ArgumentProblem) -> Self {
        Self::Argument {
            field: field.to_owned(),
            problem,
            value: None,
        }
    }

    /// Workflow #411 B27: `text`, given to the event type argument `field`, names no event type;
    /// the refusal keeps the text when it is a word a name may hold (at most 256 bytes, no
    /// control character).
    fn unknown_event_type(field: &str, problem: ArgumentProblem, text: &str) -> Self {
        let value =
            (text.len() <= 256 && !text.chars().any(char::is_control)).then(|| text.to_owned());
        Self::Argument {
            field: field.to_owned(),
            problem,
            value,
        }
    }

    /// The failure as an outcome (Workflow #378 A5). An argument or flag is the CLI's
    /// `actingctl_usage_invalid` with `argument`, `argument_problem` and, for a range,
    /// `range_min` and `range_max`; MCP answers `arguments_invalid` with the same values.
    pub fn outcome(&self) -> Outcome {
        match self {
            Self::Argument { field, problem, .. } => {
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
            Self::Export(outcome) => outcome.clone(),
            Self::Pagination => Outcome::new(ClientCode::RuntimeEventQueryPaginationInvalid),
        }
    }
}

/// The developer line's text of an argument or flag failure: "argument <field> <problem>".
impl std::fmt::Display for EventsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Argument { field, problem, .. } => {
                write!(formatter, "argument {field} {problem}")
            }
            other => write!(formatter, "{}", other.outcome()),
        }
    }
}

/// Checks every parameter, MCP arguments or CLI flags turned into the same names and JSON
/// types; `max_limit` is the frontend's largest page (§2.2). The filters are checked here as
/// the Runtime checks them, so a query it would refuse never reaches it.
pub fn parse(arguments: &Map<String, Value>, max_limit: u16) -> Result<Request, EventsError> {
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
        Some("summary") => Detail::Summary,
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
    let filters = EventQuery {
        view: typed::<LedgerView>(arguments, "view", "a ledger view")?,
        from_timestamp_unix_ms: integer(arguments, "since_unix_ms", 0, MAX_INTEGER)?,
        to_timestamp_unix_ms: integer(arguments, "until_unix_ms", 0, MAX_INTEGER)?,
        to_sequence: integer(arguments, "through_sequence", 0, MAX_INTEGER)?,
        event_type: event_type(arguments, "event_type")?,
        // Workflow #389 MCP-2: the union of types and the fact key, checked below with the
        // rest by the Runtime's rule.
        event_types: event_types(arguments, "event_types")?,
        exclude_event_types: event_types(arguments, "exclude_event_types")?,
        fact_key: text(arguments, "fact_key")?.map(str::to_owned),
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

/// One event type. Workflow #411 B27: text that names no event type is refused with the text.
fn event_type(arguments: &Map<String, Value>, key: &str) -> Result<Option<EventType>, EventsError> {
    const PROBLEM: ArgumentProblem = ArgumentProblem::NotA("an event type");
    text(arguments, key)?
        .map(|text| parsed(text).ok_or_else(|| EventsError::unknown_event_type(key, PROBLEM, text)))
        .transpose()
}

/// A list of event types: an array in MCP, a comma-separated value on the CLI. Workflow #411
/// B27: an element that is text naming no event type is refused with that text.
fn event_types(arguments: &Map<String, Value>, key: &str) -> Result<Vec<EventType>, EventsError> {
    const PROBLEM: ArgumentProblem = ArgumentProblem::NotEventTypes;
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(types)) => types
            .iter()
            .map(|event_type| match event_type.as_str() {
                Some(text) => parsed::<EventType>(text)
                    .ok_or_else(|| EventsError::unknown_event_type(key, PROBLEM, text)),
                None => Err(EventsError::argument(key, PROBLEM)),
            })
            .collect(),
        Some(_) => Err(EventsError::argument(key, PROBLEM)),
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
pub struct CursorBinding {
    pub session: u64,
    pub generation: u64,
}

impl CursorBinding {
    /// Workflow #434 LAB-5G1b: the binding of a process that holds one Runtime connection, a
    /// session of its own ([`process_session`]) and that connection: what every command line binds
    /// its cursors to, and what actinglab passes the code that serves its rows for each call.
    pub fn fresh() -> Self {
        Self {
            session: process_session(),
            generation: 1,
        }
    }

    pub fn encode(self, cursor: &RuntimeEventQueryCursor) -> Result<String, EventsError> {
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
pub struct Page {
    events: Vec<ProjectedEvent>,
    /// The ledger position the page was read at.
    snapshot: u64,
    /// The sequence the Runtime's next page of the snapshot goes on after; `None` once the
    /// snapshot is read to its end.
    continues_after: Option<u64>,
    query: EventQuery,
    /// With a wait: how long the answered read started after the first one.
    waited_ms: Option<u64>,
    /// Workflow #434 CLI-EV: the page is the newest records (a read without a position or a
    /// window), so it says its `first_sequence`.
    newest: bool,
    /// Workflow #434 CLI-EV: the lowest sequence such a read searched, when it found fewer records
    /// than its limit and older records were not searched (`searched_from_sequence`).
    searched_from: Option<u64>,
}

impl Page {
    /// The page the Runtime answered for a read from a position or a window: its rows, snapshot
    /// and continuation as they are.
    fn answered(page: &RuntimeEventQueryPage, query: EventQuery, waited_ms: Option<u64>) -> Self {
        Self {
            events: page.events().to_vec(),
            snapshot: page.snapshot_ledger_position(),
            continues_after: page
                .next_cursor()
                .map(RuntimeEventQueryCursor::after_sequence),
            query,
            waited_ms,
            newest: false,
            searched_from: None,
        }
    }

    pub fn events(&self) -> &[ProjectedEvent] {
        &self.events
    }

    /// The cursor that goes on after `after` within this page's snapshot. It is bound to the
    /// query without `after_sequence`, so that it is sent back without it.
    pub fn cursor_after(&self, after: u64) -> Result<RuntimeEventQueryCursor, EventsError> {
        RuntimeEventQueryCursor::new(self.snapshot, after, &self.query, PROFILE)
            .map_err(|error| EventsError::CursorEncode(Some(error.code())))
    }

    /// Where the Runtime's next page starts; `None` once the snapshot is read to its end.
    pub fn next_cursor(&self) -> Result<Option<RuntimeEventQueryCursor>, EventsError> {
        self.continues_after
            .map(|after| self.cursor_after(after))
            .transpose()
    }

    /// `{events, next_cursor?, truncated?, snapshot_ledger_position, first_sequence?,
    /// last_sequence?, searched_from_sequence?, waited_ms?}` with these rows; `last_sequence` is
    /// the seq of the last of them, for the next `after_sequence`. Workflow #434 CLI-EV: a page of
    /// the newest records also has `first_sequence`, the seq of the first of them (the records
    /// before it are read with `through_sequence <first_sequence - 1>`), and
    /// `searched_from_sequence` when its search found fewer than its limit and stopped there.
    pub fn result(&self, rows: &[Value], next_cursor: Option<String>, truncated: bool) -> Value {
        let mut result = Map::new();
        result.insert("events".to_owned(), Value::Array(rows.to_vec()));
        if let Some(cursor) = next_cursor {
            result.insert("next_cursor".to_owned(), json!(cursor));
        }
        if truncated {
            result.insert("truncated".to_owned(), json!(true));
        }
        result.insert("snapshot_ledger_position".to_owned(), json!(self.snapshot));
        if self.newest
            && let Some(first) = rows.first().and_then(|row| row.get("seq"))
        {
            result.insert("first_sequence".to_owned(), first.clone());
        }
        if let Some(last) = rows.last().and_then(|row| row.get("seq")) {
            result.insert("last_sequence".to_owned(), last.clone());
        }
        if let Some(searched_from) = self.searched_from {
            result.insert("searched_from_sequence".to_owned(), json!(searched_from));
        }
        if let Some(waited_ms) = self.waited_ms {
            result.insert("waited_ms".to_owned(), json!(waited_ms));
        }
        Value::Object(result)
    }

    /// Workflow #434 LAB-5D: this page as `detail` answers it within `budget`, each row
    /// [`row`]`(event, detail, envelope)`: a brief or summary page cut ([`list_within_budget`]), a
    /// payload page written whole through the budget's export ([`payload_within_budget`]).
    /// Without a budget the page whole, the page the CLI prints. `bind` writes a cursor as the
    /// caller binds it to its connection.
    pub fn within_budget(
        &self,
        detail: Detail,
        envelope: &dyn Fn(&Outcome) -> Value,
        bind: Binder<'_>,
        budget: Option<&Budget<'_>>,
    ) -> Result<Fitted, EventsError> {
        let rows = self
            .events()
            .iter()
            .map(|event| row(event, detail, envelope))
            .collect::<Vec<_>>();
        match detail {
            Detail::Brief | Detail::Summary => {
                list_within_budget(rows, budget, &|rows, kept| self.render(rows, kept, bind))
                    .map(Fitted::whole)
            }
            Detail::Payload => {
                let count = rows.len();
                let page = self.render(&rows, count, bind)?;
                payload_within_budget(page, count, budget).map_err(EventsError::Export)
            }
        }
    }

    /// The page with the first `kept` of `rows`, the rows of its events in order (moved
    /// unchanged from MCP's `ac_events`): with fewer than all, `truncated` and the cursor after
    /// the last row kept; otherwise the cursor where the Runtime's next page starts, if any.
    /// `bind` writes a cursor.
    fn render(&self, rows: &[Value], kept: usize, bind: Binder<'_>) -> Result<Value, EventsError> {
        let truncated = kept < rows.len();
        let next = if truncated {
            Some(self.cursor_after(self.events()[kept - 1].sequence))
        } else {
            self.next_cursor().transpose()
        };
        let next = next
            .transpose()
            .and_then(|next| next.map(|cursor| bind(&cursor)).transpose())?;
        Ok(self.result(&rows[..kept], next, truncated))
    }
}

/// Workflow #434 LAB-5D: how a caller writes a cursor of a page it answers, bound to its own
/// process and connection ([`CursorBinding::encode`] on its binding); the binding stays the
/// caller's.
pub type Binder<'a> = &'a dyn Fn(&RuntimeEventQueryCursor) -> Result<String, EventsError>;

/// Workflow #434 LAB-5D: how [`list_within_budget`] renders a brief or summary page:
/// `render(rows, kept)` answers the page with the first `kept` of `rows`, or the caller's failure.
pub type Renderer<'a, E> = &'a dyn Fn(&[Value], usize) -> Result<Value, E>;

/// Workflow #434 LAB-5D (moved unchanged from MCP's `ac_events`): a brief or summary page within
/// `budget`. `render(rows, kept)` answers the page with the first `kept` of `rows`: with fewer
/// than all, `truncated` and a cursor after the last row kept. Rows are cut from the end first;
/// one row alone over the budget has its material list shortened from its end, the row marked
/// `materials_truncated: true`, rather than be dropped; a row with nothing left to shorten is
/// given as it is, for the caller to refuse. Without a budget the page whole.
pub fn list_within_budget<E>(
    mut rows: Vec<Value>,
    budget: Option<&Budget<'_>>,
    render: Renderer<'_, E>,
) -> Result<Value, E> {
    let mut kept = rows.len();
    loop {
        let result = render(&rows, kept)?;
        if budget::fits(budget, &result) {
            return Ok(result);
        }
        if kept > 1 {
            kept -= 1;
            continue;
        }
        // One row alone is over the budget: shorten its material list, and say so, rather than
        // drop the row.
        let Some(materials) = rows
            .first_mut()
            .and_then(|row| row.get_mut("materials"))
            .and_then(Value::as_array_mut)
            .filter(|materials| !materials.is_empty())
        else {
            // Nothing left to shorten: the caller refuses it.
            return Ok(result);
        };
        materials.pop();
        rows[0]["materials_truncated"] = json!(true);
    }
}

/// Workflow #434 LAB-5D (moved unchanged from MCP's `ac_events`): a payload page within
/// `budget`. `page` is the whole page, the page the CLI prints (without its `outcome`), and
/// `events_count` its rows. It is given whole without a budget, when the budget takes it or for a
/// caller with no export; otherwise it is written whole through the caller's export
/// ([`budget::write_json`]) and answered as `{export, events_count, next_cursor?,
/// first_sequence?, last_sequence?, searched_from_sequence?}` with the warning `answer_exported`
/// naming the file (`path`); Workflow #434 CLI-EV: `first_sequence` and `searched_from_sequence`
/// as the page has them (a page of the newest records).
pub fn payload_within_budget(
    page: Value,
    events_count: usize,
    budget: Option<&Budget<'_>>,
) -> Result<Fitted, Outcome> {
    let Some(export) = budget::export_for(budget, &page) else {
        return Ok(Fitted::whole(page));
    };
    let reference = budget::write_json(export, &page)?;
    let mut note = Outcome::new(ActingctlCode::AnswerExported);
    if let Some(path) = reference["path"].as_str() {
        note = note.path(path);
    }
    let mut answer = Map::new();
    answer.insert("export".to_owned(), reference);
    answer.insert("events_count".to_owned(), json!(events_count));
    if let Some(next) = page.get("next_cursor") {
        answer.insert("next_cursor".to_owned(), next.clone());
    }
    for member in ["first_sequence", "last_sequence", "searched_from_sequence"] {
        if let Some(value) = page.get(member) {
            answer.insert(member.to_owned(), value.clone());
        }
    }
    Ok(Fitted {
        result: Value::Object(answer),
        exported: Some(note),
    })
}

/// Reads one page of `request` with the resolved instance. A page from `after_sequence` starts
/// at the next sequence; a cursor resumes where its page ended. With a wait, an empty page is
/// read again every second until rows arrive, `wait_until` comes or `stop` says so. Workflow #434
/// CLI-EV: a request without a position or a window ([`Request::newest`]) reads the newest
/// records (`read_newest`).
pub fn read(
    client: &RuntimeClient,
    request: &Request,
    instance_id: Option<InstanceId>,
    binding: CursorBinding,
    wait_until: Instant,
    stop: &dyn Fn() -> bool,
) -> Result<Page, EventsError> {
    let query = request.query(instance_id);
    if request.newest() {
        return read_newest(client, request, query, wait_until, stop);
    }
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
        // #411 A51: a page the Runtime's read budget cut is answered at once with its cursor,
        // never read again from the same place.
        if !waiting
            || !page.events().is_empty()
            || page.has_more()
            || stop()
            || Instant::now() + POLL_INTERVAL >= wait_until
        {
            let waited_ms =
                u64::try_from(started.duration_since(first).as_millis()).unwrap_or(u64::MAX);
            return Ok(Page::answered(&page, query, waiting.then_some(waited_ms)));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Workflow #434 CLI-EV (rulings 1 and 3): [`read`] of a request without a position or a window:
/// the newest `limit` records its filters match, oldest first. The head is read once
/// ([`ledger_head`]); the search reads windows that end there, at its snapshot, each by its
/// sequence range and without run recovery ([`Search::newest`]); the rows it found are read once
/// more with their payloads ([`Search::rows`]). With a wait, an answer that found nothing (and was
/// not cut short) is searched again every second among the records appended since, until rows
/// arrive, `wait_until` comes or `stop` says so.
fn read_newest(
    client: &RuntimeClient,
    request: &Request,
    query: EventQuery,
    wait_until: Instant,
    stop: &dyn Fn() -> bool,
) -> Result<Page, EventsError> {
    let waiting = !request.wait.is_zero();
    let first = Instant::now();
    let mut started = first;
    let mut head = ledger_head(client)?;
    let mut found = Search {
        client,
        query: &query,
        head,
    }
    .newest(request.limit, 1)?;
    // The lowest sequence of the range searched through the head without a gap.
    let mut reached = found.reached;
    while waiting
        && found.sequences.is_empty()
        && !found.cut
        && !stop()
        && Instant::now() + POLL_INTERVAL < wait_until
    {
        thread::sleep(POLL_INTERVAL);
        started = Instant::now();
        let latest = ledger_head(client)?;
        if latest > head {
            let floor = head + 1;
            found = Search {
                client,
                query: &query,
                head: latest,
            }
            .newest(request.limit, floor)?;
            // A search that reached its floor goes on from the range searched before; one that
            // stopped above it leaves a gap, below which nothing counts as searched.
            if found.reached > floor {
                reached = found.reached;
            }
            head = latest;
        }
    }
    let Rows {
        events,
        continues_after,
    } = Search {
        client,
        query: &query,
        head,
    }
    .rows(&found.sequences)?;
    let searched_from =
        (found.sequences.len() < usize::from(request.limit) && reached > 1).then_some(reached);
    let waited_ms = u64::try_from(started.duration_since(first).as_millis()).unwrap_or(u64::MAX);
    Ok(Page {
        events,
        snapshot: head,
        continues_after,
        query,
        waited_ms: waiting.then_some(waited_ms),
        newest: true,
        searched_from,
    })
}

/// Workflow #434 CLI-EV: the ledger's head, read once per search: the snapshot of one page that
/// selects nothing (its range starts past every sequence a ledger reaches, with no other
/// condition, one row, no run recovery), which the Runtime answers without reading a record: a
/// page's statements run only while its range is not empty (QP-1, register §1.292).
fn ledger_head(client: &RuntimeClient) -> Result<u64, EventsError> {
    let request = RuntimeEventQueryPageRequest::new(1, None)
        .map_err(|_| EventsError::Pagination)?
        .without_run_recovery();
    let nothing = EventQuery {
        from_sequence: Some(MAX_INTEGER),
        ..EventQuery::default()
    };
    let page = client
        .query_event_page(nothing, SEARCH_PROFILE, request)
        .map_err(EventsError::Runtime)?;
    if !page.events().is_empty() {
        return Err(EventsError::Pagination);
    }
    Ok(page.snapshot_ledger_position())
}

/// Workflow #434 CLI-EV: the reads of one search for the newest records: the records `query`
/// matches at the snapshot `head`, each read by a sequence range and without run recovery.
struct Search<'a> {
    client: &'a RuntimeClient,
    query: &'a EventQuery,
    head: u64,
}

/// What a search found: the sequences of the newest records it found, oldest first; `reached`,
/// the lowest sequence of the range it searched through the head without a gap (`head + 1` when
/// it searched none); `cut` when a read the Runtime cut short (its read budget) ended it.
struct Found {
    sequences: Vec<u64>,
    reached: u64,
    cut: bool,
}

impl Found {
    /// The records of `page`, older than every record found so far, go before them; the range
    /// searched now starts at `from`.
    fn take(&mut self, page: &RuntimeEventQueryPage, from: u64) {
        let mut sequences = page
            .events()
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>();
        sequences.append(&mut self.sequences);
        self.sequences = sequences;
        self.reached = from;
    }
}

impl Search<'_> {
    /// The records the query matches from `from` through `through`, at most `limit`, in
    /// `profile`, at the search's snapshot. A page at another snapshot or with a row outside the
    /// range is refused.
    fn read(
        &self,
        (from, through): (u64, u64),
        limit: u16,
        profile: ProjectionProfile,
    ) -> Result<RuntimeEventQueryPage, EventsError> {
        let request = RuntimeEventQueryPageRequest::new(limit, None)
            .and_then(|request| request.at_snapshot(self.head))
            .map_err(|_| EventsError::Pagination)?
            .without_run_recovery();
        let query = EventQuery {
            from_sequence: Some(from),
            to_sequence: Some(through),
            ..self.query.clone()
        };
        let page = self
            .client
            .query_event_page(query, profile, request)
            .map_err(EventsError::Runtime)?;
        if page.snapshot_ledger_position() != self.head
            || page
                .events()
                .iter()
                .any(|event| event.sequence < from || event.sequence > through)
        {
            return Err(EventsError::Pagination);
        }
        Ok(page)
    }

    /// The newest `limit` records at or above `floor`, through the head. The windows of
    /// [`NEWEST_WINDOWS`] end at the head; each read is the part of its window the one before did
    /// not hold, so no record is read twice, and the last reaches `floor` or the largest window's
    /// lower bound, whichever is higher. A part that holds the records still wanted, or more,
    /// ends the search ([`Self::narrow`]); a read the Runtime cut short ends it with what was
    /// found.
    fn newest(&self, limit: u16, floor: u64) -> Result<Found, EventsError> {
        let lowest = floor
            .max(self.head.saturating_sub(NEWEST_LARGEST_WINDOW - 1))
            .max(1);
        let mut found = Found {
            sequences: Vec::new(),
            reached: self.head.saturating_add(1),
            cut: false,
        };
        let mut need = limit;
        let mut top = self.head;
        for width in NEWEST_WINDOWS {
            if top < lowest {
                break;
            }
            let from = self.head.saturating_sub(width - 1).max(lowest);
            let page = self.read((from, top), need, SEARCH_PROFILE)?;
            let rows = page.returned_count();
            if page.has_more() {
                if rows < need {
                    found.cut = true;
                    return Ok(found);
                }
                return self.narrow(found, (from, top), need);
            }
            found.take(&page, from);
            need = need.checked_sub(rows).ok_or(EventsError::Pagination)?;
            if need == 0 {
                break;
            }
            top = from - 1;
        }
        Ok(found)
    }

    /// `range` holds at least `need` of the records, all older than those `found` holds: its
    /// newest `need` go before them. The first read is the range's top `need` sequences (where
    /// every record matches, they are the ones wanted); each later read is the upper half of what
    /// is left. A read that holds more than `need` leaves the rest of the range below it, which
    /// is dropped; otherwise its records are taken and the search goes on below it. Each read
    /// halves the range, so a range of `w` sequences takes at most `log2(w) + 2` reads; the loop's
    /// bound only stops a Runtime whose pages contradict each other.
    fn narrow(
        &self,
        mut found: Found,
        (mut low, mut high): (u64, u64),
        mut need: u16,
    ) -> Result<Found, EventsError> {
        let mut start = high.saturating_sub(u64::from(need) - 1).max(low);
        for _ in 0..=u64::BITS {
            let page = self.read((start, high), need, SEARCH_PROFILE)?;
            let rows = page.returned_count();
            if page.has_more() {
                if rows < need {
                    found.cut = true;
                    return Ok(found);
                }
                low = start;
            } else {
                found.take(&page, start);
                need = need.checked_sub(rows).ok_or(EventsError::Pagination)?;
                if need == 0 {
                    return Ok(found);
                }
                // The whole range read held fewer than it was found to hold.
                if start == low {
                    return Err(EventsError::Pagination);
                }
                high = start - 1;
            }
            start = low + (high - low).div_ceil(2);
        }
        Err(EventsError::Pagination)
    }

    /// The records the search found, in [`PROFILE`] (their payloads), by one read from the first
    /// to the last of them, with the sequence the Runtime's next page goes on after when it cut
    /// the page to its response bound. Every record of that range was found, so the page holds
    /// them all, or the first of them with the rest to come.
    fn rows(&self, sequences: &[u64]) -> Result<Rows, EventsError> {
        let (Some(&first), Some(&last)) = (sequences.first(), sequences.last()) else {
            return Ok(Rows {
                events: Vec::new(),
                continues_after: None,
            });
        };
        let count = u16::try_from(sequences.len()).map_err(|_| EventsError::Pagination)?;
        let page = self.read((first, last), count, PROFILE)?;
        let read = page
            .events()
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>();
        let whole = read.len() == sequences.len();
        if !sequences.starts_with(&read) || !(whole || page.has_more()) {
            return Err(EventsError::Pagination);
        }
        let continues_after = if whole {
            None
        } else {
            page.next_cursor()
                .map(RuntimeEventQueryCursor::after_sequence)
        };
        Ok(Rows {
            events: page.events().to_vec(),
            continues_after,
        })
    }
}

/// The rows a search found, read with their payloads, and the sequence the Runtime's next page
/// goes on after when it cut them to its response bound.
struct Rows {
    events: Vec<ProjectedEvent>,
    continues_after: Option<u64>,
}

/// A row of `detail`; `envelope` writes an outcome as the frontend writes it (the CLI line's and
/// the MCP server's outcome detail).
pub fn row(event: &ProjectedEvent, detail: Detail, envelope: &dyn Fn(&Outcome) -> Value) -> Value {
    let mut row = brief_row(event);
    match detail {
        Detail::Brief => {}
        Detail::Summary => {
            if let ProjectionPayload::Full(payload) = &event.payload
                && let Some(outcome) = payload.outcome()
            {
                row["outcome"] = envelope(outcome);
            }
        }
        Detail::Payload => row["payload"] = json!(event.payload),
    }
    row
}

/// `{seq, ts, type, severity, instance?, request_id?, run_id?, task_id?, correlation_id?,
/// code?, materials?}`: the ledger's diagnostic code and the material refs for `ac_material`.
pub fn brief_row(event: &ProjectedEvent) -> Value {
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
pub fn dispatch(arguments: &[OsString]) -> Option<cli_line::Exit> {
    dispatch_on(arguments, Connection::per_call())
}

/// Workflow #434 LAB-5G1: [`dispatch`] on the caller's `connection`.
pub fn dispatch_on(arguments: &[OsString], connection: Connection<'_>) -> Option<cli_line::Exit> {
    dispatch_within(arguments, connection, None, CursorBinding::fresh())
}

/// Workflow #434 LAB-5G1b: [`dispatch_on`] within the caller's `budget` (none: the whole page, as
/// the command line prints it; with one, the page within it, [`Page::within_budget`], an export
/// carried on the line as its warning), its cursors taken and issued under the caller's
/// `binding` (the command line: [`CursorBinding::fresh`]).
pub fn dispatch_within(
    arguments: &[OsString],
    connection: Connection<'_>,
    budget: Option<&Budget<'_>>,
    binding: CursorBinding,
) -> Option<cli_line::Exit> {
    if arguments.first()?.to_str()? != COMMAND {
        return None;
    }
    let ran = run_cli(&arguments[1..], connection, budget, binding);
    Some(match ran {
        Ok(page) => cli_line::success(
            budget::line_members(page, "events", Vec::new()),
            &Outcome::new(ActingctlCode::RuntimeEventsReported),
        ),
        Err(error @ EventsError::Argument { .. }) => cli_line::failure(
            &error.outcome(),
            Some(format!("{error}\n{CLI_USAGE}").as_str()),
        ),
        Err(error) => cli_line::failure(&error.outcome(), None),
    })
}

/// Workflow #434 LAB-5C: the read the words after `events` ask for, checked as [`dispatch`] checks
/// them, before any connection: the state root and the request.
pub fn cli_request(arguments: &[OsString]) -> Result<(PathBuf, Request), EventsError> {
    let (state_root, parameters) = cli_parameters(arguments)?;
    Ok((state_root, parse(&parameters, CLI_MAX_LIMIT)?))
}

fn run_cli(
    arguments: &[OsString],
    connection: Connection<'_>,
    budget: Option<&Budget<'_>>,
    binding: CursorBinding,
) -> Result<Fitted, EventsError> {
    let (state_root, request) = cli_request(arguments)?;
    let client = connection
        .reach(&state_root, identity)
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
    // Workflow #434 LAB-5G1b: the caller's binding (the command line's process holds one Runtime
    // connection: `CursorBinding::fresh`).
    let page = read(
        &client,
        &request,
        instance_id,
        binding,
        Instant::now() + request.wait,
        &|| false,
    )?;
    // The outcome detail main checked before this command ran (Workflow #378 A5).
    let outcome_detail = cli_line::detail().unwrap_or(OutcomeDetail::Full);
    let envelope = |outcome: &Outcome| cli_line::outcome_value(outcome, outcome_detail);
    // Without a budget the whole page, its rows each `row` and its next cursor bound to
    // `binding`, as before; with one, the page within it (Workflow #434 LAB-5D's shaping).
    page.within_budget(
        request.detail,
        &envelope,
        &|cursor: &RuntimeEventQueryCursor| binding.encode(cursor),
        budget,
    )
}

/// `--state-root` and every other flag `flags_of` names for `events`, under its parameter name,
/// typed as MCP types it: integers as numbers, lists split at commas, the rest as strings. Every
/// flag takes a value and may be given once.
fn cli_parameters(arguments: &[OsString]) -> Result<(PathBuf, Map<String, Value>), EventsError> {
    let takes = crate::flags_of(COMMAND, None).unwrap_or_default();
    let mut state_root = None;
    let mut parameters = Map::new();
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index]
            .to_str()
            .ok_or_else(|| EventsError::argument("flag", ArgumentProblem::NotUtf8))?;
        // Workflow #382 A1-FLAG: a flag `events` does not take is unknown, with or without a
        // value after it.
        let name = flag
            .strip_prefix("--")
            .filter(|_| takes.contains(&flag))
            .ok_or_else(|| EventsError::argument(flag, ArgumentProblem::UnknownFlag))?;
        let key = name.replace('-', "_");
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
pub fn process_session() -> u64 {
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    RandomState::new().hash_one((process::id(), started))
}

// Test-only: Workflow #378 A5, an argument failure of `events` as an outcome; Workflow #389
// MCP-2, the query rule of `event_types` and `fact_key`; the MCP-1 follow-up, `detail summary`.
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

    // Workflow #389 MCP-2 (MCP-0 command table v1.1 §4.2): `event_types` and `fact_key` follow the
    // rule the Runtime holds every query to, checked before any read. `event_types` is the
    // alternative of `event_type`, at most 32 types, each once, none of them excluded, or
    // `invalid_event_query_event_type_filter`; `fact_key` is an instance fact key or a runtime
    // fact key, or `invalid_event_query_fact_key_filter`. A union and either kind of key that keep
    // the rule reach the query as given.
    #[test]
    fn mcp2_event_types_and_fact_key_keep_the_rule_of_the_runtime_query() {
        use actingcommand_contract::codes::SanitizationCode;

        let thirty_three = [
            "provider.startup_observed",
            "runtime.started",
            "runtime.takeover",
            "runtime.failed",
            "runtime.lifecycle_observed",
            "runtime.instance_bound",
            "runtime.fact_recorded",
            "runtime.fact_invalidated",
            "runtime.fact_snapshot",
            "monitor.probe_requested",
            "monitor.probe_started",
            "monitor.probe_completed",
            "monitor.probe_failed",
            "monitor.recovery_admitted",
            "monitor.recovery_deferred",
            "perf.pressure_started",
            "perf.pressure_ended",
            "perf.stutter_detected",
            "perf.summary",
            "perf.monitor_degraded",
            "perf.monitor_recovered",
            "perf.balance_changed",
            "fact.published",
            "fact.invalidated",
            "approval.decision",
            "command.received",
            "command.validated",
            "command.rejected",
            "scheduler.admitted",
            "scheduler.queued",
            "scheduler.denied",
            "scheduler.preempted",
            "policy.dispatch_intent",
        ];
        let refused = [
            (
                json!({"event_type": "fact.published", "event_types": ["fact.published", "runtime.failed"]}),
                SanitizationCode::InvalidEventQueryEventTypeFilter,
            ),
            (
                json!({"event_types": thirty_three.to_vec()}),
                SanitizationCode::InvalidEventQueryEventTypeFilter,
            ),
            (
                json!({"event_types": ["fact.published", "runtime.failed", "fact.published"]}),
                SanitizationCode::InvalidEventQueryEventTypeFilter,
            ),
            (
                json!({"event_types": ["fact.published", "runtime.failed"], "exclude_event_types": ["runtime.failed"]}),
                SanitizationCode::InvalidEventQueryEventTypeFilter,
            ),
            (
                json!({"fact_key": "neutral.key"}),
                SanitizationCode::InvalidEventQueryFactKeyFilter,
            ),
        ];
        for (arguments, code) in refused {
            let Value::Object(arguments) = arguments else {
                panic!("the arguments are an object")
            };
            match parse(&arguments, CLI_MAX_LIMIT) {
                Err(error @ EventsError::Query(_)) => {
                    let outcome = error.outcome();
                    assert!(
                        outcome.is(code) && outcome.category() == Category::Error,
                        "{arguments:?}: {outcome}"
                    );
                }
                Err(other) => panic!("{arguments:?} was refused as {other}, not by the query rule"),
                Ok(request) => panic!("{arguments:?} was taken: {request:?}"),
            }
        }

        let union = thirty_three[..32].to_vec();
        for (arguments, event_types, fact_key) in [
            (json!({"event_types": union}), union.clone(), None),
            (
                json!({"event_types": ["fact.published"], "fact_key": "session.neutral.key"}),
                ["fact.published"].to_vec(),
                Some("session.neutral.key"),
            ),
            (
                json!({"fact_key": "application.foreground"}),
                Vec::new(),
                Some("application.foreground"),
            ),
        ] {
            let Value::Object(arguments) = arguments else {
                panic!("the arguments are an object")
            };
            let request = parse(&arguments, CLI_MAX_LIMIT)
                .unwrap_or_else(|error| panic!("{arguments:?} was refused: {error}"));
            let query = request.query(None);
            assert_eq!(
                (json!(query.event_types), query.fact_key.as_deref()),
                (json!(event_types), fact_key),
                "{arguments:?}"
            );
        }
    }

    // Workflow #389 MCP-1 follow-up (MCP-0 command table v1.1 §4.1, coordinator ruling Q4):
    // `detail` takes `summary` beside `brief` (the default) and `payload`; any other value is
    // refused naming `detail` as not recognised.
    #[test]
    fn mcp1_follow_up_detail_takes_summary_beside_brief_and_payload() {
        for (given, detail) in [
            (None, Detail::Brief),
            (Some("brief"), Detail::Brief),
            (Some("summary"), Detail::Summary),
            (Some("payload"), Detail::Payload),
        ] {
            let mut arguments = Map::new();
            if let Some(given) = given {
                arguments.insert("detail".to_owned(), json!(given));
            }
            let request = parse(&arguments, CLI_MAX_LIMIT)
                .unwrap_or_else(|error| panic!("detail {given:?} was refused: {error}"));
            assert_eq!(request.detail, detail, "detail {given:?}");
        }
        let mut arguments = Map::new();
        arguments.insert("detail".to_owned(), json!("full"));
        let error = parse(&arguments, CLI_MAX_LIMIT).expect_err("detail full was taken");
        let outcome = error.outcome();
        assert!(
            outcome.is(ActingctlCode::ActingctlUsageInvalid)
                && outcome.value("argument") == Some(&json!("detail"))
                && outcome.value("argument_problem") == Some(&json!("not_recognised")),
            "{outcome}"
        );
    }
}
