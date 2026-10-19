// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl remedy set|get|lift` (Workflow #411 B15 R6; model-b15 §5.1, §7.6;
//! `contracts/instance-fact-store.md`, "Remedy choice facts"), the CLI originals of MCP
//! `ac_remedy_set` and `ac_remedy_get` (`mcp::remedy`). A person or an agent writes the remedy
//! choices of an instance, a server or a game as the instance fact `session.remedy.choices`
//! through the ordinary fact publication; the Runtime checks every write (its shape, and the
//! names against the packages bound on every instance the scope covers) and refuses a bad one
//! whole with `session_remedy_choices_invalid`. Both sides build the same record and answer the
//! same object:
//!
//! - `set` writes the scope's whole list (`[]` withdraws it) as a new observation:
//!   `{event_id, scope, key, source_snapshot_id, observed_at_unix_ms, rows_total}`;
//! - `get` reads the instance's facts and answers the records that apply, most specific first,
//!   and the one in effect: `{instance_alias, read_at_unix_ms, records, in_effect}`;
//! - `lift` republishes the instance's own record unchanged, or an empty one when it has none,
//!   with a new observation (model-b15 §7.6 item 2: a new version lifts a remedy stop): the
//!   answer of `set` and `previous_source_snapshot_id`.
//!
//! The CLI writes with its (Cli, Cli) origin, MCP with the agent's (Agent, Adapter). The CLI
//! line's outcome is the completed receipt's code of the request the command made
//! (`request_publish_facts_completed`, `request_instance_fact_snapshot_completed`).

use crate::cli_line;
use crate::codes::ArgumentProblem;
use crate::instance_selector::{self, SelectorMiss};
use actingcommand_contract::codes::ReceiptCode;
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{
    EventActor, EventSource, FactContent, FactObservation, FactRecord, FactScalar, FactScope,
    FactValue, REMEDY_CHOICES_FACT_KEY, SanitizationError,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const COMMAND: &str = "remedy";
pub(crate) const SET: &str = "set";
pub(crate) const GET: &str = "get";
pub(crate) const LIFT: &str = "lift";
const USAGE: &str = "usage: actingctl remedy set (--instance <alias|instance_id|ADB port> | --server <id> --game <id> | --game <id>) --rows <file> --state-root <path>\nusage: actingctl remedy get|lift --instance <alias|instance_id|ADB port> --state-root <path>";
/// `source_detector` of the records the CLI writes.
const CLI_DETECTOR: &str = "actingctl.remedy";
/// `source_detector` of the records MCP writes.
#[cfg(feature = "mcp")]
pub(crate) const MCP_DETECTOR: &str = "actingctl-mcp.remedy";
/// The largest rows file read: a fact observation's bound.
const MAX_ROWS_FILE_BYTES: usize = actingcommand_contract::MAX_FACT_OBSERVATION_BYTES;

/// The scope a `set` writes.
pub(crate) enum Target {
    /// An instance's alias, instance_id or ADB port, resolved with one status read.
    Instance(String),
    Server(String),
    Game(String),
}

/// Why a remedy command answered nothing.
pub(crate) enum Failure {
    /// The selector names no instance of the Runtime's status, or more than one.
    Selector {
        miss: SelectorMiss,
        selector: String,
    },
    /// The Runtime refused the request (its receipt's outcome on top), or did not answer.
    Runtime(RuntimeClientError),
    /// The system clock is before the Unix epoch or out of range.
    Clock,
    /// The contract refuses the record (a scope identifier or a row it cannot hold).
    Record(SanitizationError),
}

impl Failure {
    /// The failure as one outcome (Workflow #378 A5).
    pub(crate) fn outcome(&self) -> Outcome {
        match self {
            Self::Selector { miss, selector } => miss.outcome(selector),
            Self::Runtime(error) => error.outcome(),
            Self::Clock => Outcome::new(crate::codes::ActingctlCode::ClockUnavailable),
            Self::Record(error) => Outcome::new(error.code()),
        }
    }
}

/// The rows of a write from JSON: a list of objects whose members are strings, each member a
/// field of the row; `None` for anything else. Which rows the Runtime takes is its check.
pub(crate) fn rows_from_json(value: &Value) -> Option<Vec<BTreeMap<String, FactScalar>>> {
    value
        .as_array()?
        .iter()
        .map(|row| {
            row.as_object()?
                .iter()
                .map(|(member, value)| {
                    value
                        .as_str()
                        .map(|text| (member.clone(), FactScalar::String(text.to_owned())))
                })
                .collect()
        })
        .collect()
}

/// `set`: the scope's whole list as a new observation now.
pub(crate) fn set(
    client: &RuntimeClient,
    target: &Target,
    rows: Vec<BTreeMap<String, FactScalar>>,
    detector: &str,
) -> Result<Value, Failure> {
    let scope = match target {
        Target::Instance(selector) => FactScope::Instance {
            instance_id: resolve(client, selector)?,
        },
        Target::Server(server_id) => FactScope::Server {
            server_id: server_id.clone(),
        },
        Target::Game(game_id) => FactScope::Game {
            game_id: game_id.clone(),
        },
    };
    let record = FactRecord::remedy_choices(scope, rows, observed_now()?, detector)
        .map_err(Failure::Record)?;
    publish(client, record)
}

/// `get`: the `session.remedy.choices` records that apply to the instance `selector` names,
/// most specific first, and the scope of the one in effect (the most specific not expired at
/// the read; `null` when none is).
pub(crate) fn get(client: &RuntimeClient, selector: &str) -> Result<Value, Failure> {
    let alias = resolve(client, selector)?;
    let (view, source) = client
        .instance_fact_snapshot(&alias)
        .map_err(Failure::Runtime)?;
    let read_at_unix_ms = source.sampled_completed_at_unix_ms;
    let mut records = view
        .records
        .iter()
        .filter(|record| record.key == REMEDY_CHOICES_FACT_KEY)
        .collect::<Vec<_>>();
    records.sort_by_key(|record| specificity(&record.scope));
    let in_effect = records
        .iter()
        .position(|record| !record.is_expired(read_at_unix_ms));
    let listed = records
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let mut listed = Map::new();
            listed.insert("scope".to_owned(), json!(record.scope));
            listed.insert(
                "observed_at_unix_ms".to_owned(),
                json!(record.observed_at_unix_ms),
            );
            if let Some(expires_at_unix_ms) = record.expires_at_unix_ms {
                listed.insert("expires_at_unix_ms".to_owned(), json!(expires_at_unix_ms));
            }
            listed.insert(
                "source_snapshot_id".to_owned(),
                json!(record.source_snapshot_id),
            );
            listed.insert("rows".to_owned(), rows_value(&record.content));
            listed.insert("in_effect".to_owned(), json!(in_effect == Some(index)));
            Value::Object(listed)
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "instance_alias": alias,
        "read_at_unix_ms": read_at_unix_ms,
        "records": listed,
        "in_effect": in_effect.map(|index| json!(records[index].scope)),
    }))
}

/// `lift` (model-b15 §7.6 item 2): the instance's own record republished unchanged (its rows,
/// and its lifetime's length when it has one), or an empty one when it has none, as a new
/// observation, strictly newer than the one it replaces.
pub(crate) fn lift(
    client: &RuntimeClient,
    selector: &str,
    detector: &str,
) -> Result<Value, Failure> {
    let alias = resolve(client, selector)?;
    let (view, _) = client
        .instance_fact_snapshot(&alias)
        .map_err(Failure::Runtime)?;
    let scope = FactScope::Instance { instance_id: alias };
    let previous = view
        .records
        .iter()
        .find(|record| record.key == REMEDY_CHOICES_FACT_KEY && record.scope == scope);
    let rows = match previous.map(|record| &record.content) {
        Some(FactContent::Inline {
            value: FactValue::RecordList(rows),
        }) => rows.clone(),
        _ => Vec::new(),
    };
    let mut observed_at_unix_ms = observed_now()?;
    if let Some(previous) = previous {
        observed_at_unix_ms = observed_at_unix_ms.max(previous.observed_at_unix_ms + 1);
    }
    let mut record = FactRecord::remedy_choices(scope, rows, observed_at_unix_ms, detector)
        .map_err(Failure::Record)?;
    if let Some(previous) = previous {
        // Unchanged: whatever the stored record holds goes out again, for the Runtime to check.
        record.content = previous.content.clone();
        if let (Some(expires_at_unix_ms), Some(ttl_policy)) =
            (previous.expires_at_unix_ms, &previous.ttl_policy)
        {
            record.expires_at_unix_ms = Some(
                observed_at_unix_ms
                    + expires_at_unix_ms.saturating_sub(previous.observed_at_unix_ms),
            );
            record.ttl_policy = Some(ttl_policy.clone());
        }
    }
    let mut answer = publish(client, record)?;
    answer["previous_source_snapshot_id"] =
        json!(previous.map(|record| record.source_snapshot_id.as_str()));
    Ok(answer)
}

/// Publishes `record` alone and answers `{event_id, scope, key, source_snapshot_id,
/// observed_at_unix_ms, rows_total}`.
fn publish(client: &RuntimeClient, record: FactRecord) -> Result<Value, Failure> {
    let rows_total = match &record.content {
        FactContent::Inline {
            value: FactValue::RecordList(rows),
        } => rows.len(),
        _ => 0,
    };
    let mut answer = json!({
        "scope": record.scope,
        "key": record.key,
        "source_snapshot_id": record.source_snapshot_id,
        "observed_at_unix_ms": record.observed_at_unix_ms,
        "rows_total": rows_total,
    });
    let event_id = client
        .publish_facts(FactObservation {
            records: vec![record],
        })
        .map_err(Failure::Runtime)?;
    answer["event_id"] = json!(event_id);
    Ok(answer)
}

/// The alias of the one instance `selector` names, with one status read.
fn resolve(client: &RuntimeClient, selector: &str) -> Result<String, Failure> {
    let status = client.status().map_err(Failure::Runtime)?;
    Ok(instance_selector::select(status.instances(), selector)
        .map_err(|miss| Failure::Selector {
            miss,
            selector: selector.to_owned(),
        })?
        .instance_alias()
        .to_owned())
}

/// A record's rows as JSON: each row's members, a string as itself and any other value as its
/// typed form; a record that holds no inline row list as `null`.
fn rows_value(content: &FactContent) -> Value {
    let FactContent::Inline {
        value: FactValue::RecordList(rows),
    } = content
    else {
        return Value::Null;
    };
    Value::Array(
        rows.iter()
            .map(|row| {
                Value::Object(
                    row.iter()
                        .map(|(member, value)| {
                            let value = match value {
                                FactScalar::String(text) => json!(text),
                                other => json!(other),
                            };
                            (member.clone(), value)
                        })
                        .collect(),
                )
            })
            .collect(),
    )
}

/// Instance before server before game.
fn specificity(scope: &FactScope) -> u8 {
    match scope {
        FactScope::Instance { .. } => 0,
        FactScope::Server { .. } => 1,
        FactScope::Game { .. } => 2,
    }
}

/// Now, in Unix milliseconds; `Failure::Clock` before the epoch or out of range.
fn observed_now() -> Result<u64, Failure> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .filter(|millis| *millis > 0)
        .ok_or(Failure::Clock)
}

// ---- the CLI

/// One parsed command line.
struct Parsed {
    action: &'static str,
    state_root: PathBuf,
    instance: Option<String>,
    server: Option<String>,
    game: Option<String>,
    rows: Option<PathBuf>,
}

/// The `remedy` command, when it is the first word.
pub(crate) fn dispatch(arguments: &[OsString], detail: Detail) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != COMMAND {
        return None;
    }
    let parsed = match parse(arguments) {
        Ok(parsed) => parsed,
        Err(outcome) => return Some(cli_line::failure(&outcome, Some(USAGE))),
    };
    Some(match run(&parsed, detail) {
        Ok((answer, outcome)) => cli_line::success(cli_line::members(answer, COMMAND), &outcome),
        Err(Ended::Usage(outcome)) => cli_line::failure(&outcome, Some(USAGE)),
        // A refused request prints the Runtime's receipt beside its outcome and exits non-zero.
        Err(Ended::Failure(Failure::Runtime(error))) => match error.received_receipt() {
            Some(receipt) => match serde_json::to_value(receipt) {
                Ok(receipt) => {
                    let mut members = Map::new();
                    members.insert("receipt".to_owned(), receipt);
                    cli_line::receipt(members, &error.outcome(), true)
                }
                Err(_) => cli_line::refused(&error.outcome(), Some(&error.to_string())),
            },
            None => cli_line::refused(&error.outcome(), Some(&error.to_string())),
        },
        Err(Ended::Failure(failure)) => cli_line::refused(&failure.outcome(), None),
    })
}

/// Why the CLI ended without an answer.
enum Ended {
    Usage(Outcome),
    Failure(Failure),
}

/// `remedy <set|get|lift>` and the flags `flags_of` names for the action, each once with a
/// value; for `set`, exactly one target form (`--instance`, `--server` with `--game`, or
/// `--game`) and `--rows`; for `get` and `lift`, `--instance`; `--state-root` always.
fn parse(arguments: &[OsString]) -> Result<Parsed, Outcome> {
    let action = match arguments.get(1).and_then(|word| word.to_str()) {
        Some(SET) => SET,
        Some(GET) => GET,
        Some(LIFT) => LIFT,
        Some(word) => {
            return Err(cli_line::usage_invalid(
                Some(word),
                ArgumentProblem::NotRecognised,
            ));
        }
        None => return Err(cli_line::usage_invalid(None, ArgumentProblem::Required)),
    };
    let takes = crate::flags_of(COMMAND, Some(action)).unwrap_or_default();
    let mut values = BTreeMap::<&str, String>::new();
    let mut words = arguments[2..].iter();
    while let Some(word) = words.next() {
        let Some(word) = word.to_str() else {
            return Err(cli_line::usage_invalid(None, ArgumentProblem::NotUtf8));
        };
        let Some(flag) = takes.iter().copied().find(|flag| *flag == word) else {
            return Err(cli_line::usage_invalid(
                Some(word),
                ArgumentProblem::Unknown,
            ));
        };
        if values.contains_key(flag) {
            return Err(cli_line::usage_invalid(
                Some(flag),
                ArgumentProblem::Repeated,
            ));
        }
        let value = words
            .next()
            .ok_or_else(|| cli_line::usage_invalid(Some(flag), ArgumentProblem::ValueMissing))?
            .to_str()
            .ok_or_else(|| cli_line::usage_invalid(Some(flag), ArgumentProblem::NotUtf8))?;
        if value.trim().is_empty() {
            return Err(cli_line::usage_invalid(
                Some(flag),
                ArgumentProblem::ValueMissing,
            ));
        }
        values.insert(flag, value.to_owned());
    }
    let state_root = values
        .remove("--state-root")
        .map(PathBuf::from)
        .ok_or_else(|| cli_line::usage_invalid(Some("--state-root"), ArgumentProblem::Required))?;
    let parsed = Parsed {
        action,
        state_root,
        instance: values.remove("--instance"),
        server: values.remove("--server"),
        game: values.remove("--game"),
        rows: values.remove("--rows").map(PathBuf::from),
    };
    match (
        action,
        &parsed.instance,
        &parsed.server,
        &parsed.game,
        &parsed.rows,
    ) {
        (_, None, None, None, _) => Err(cli_line::usage_invalid(
            Some("--instance"),
            ArgumentProblem::Required,
        )),
        (SET, Some(_), Some(_), _, _) => Err(cli_line::usage_invalid(
            Some("--server"),
            ArgumentProblem::Conflicting,
        )),
        (SET, Some(_), None, Some(_), _) => Err(cli_line::usage_invalid(
            Some("--game"),
            ArgumentProblem::Conflicting,
        )),
        (SET, None, Some(_), None, _) => Err(cli_line::usage_invalid(
            Some("--game"),
            ArgumentProblem::Required,
        )),
        (SET, _, _, _, None) => Err(cli_line::usage_invalid(
            Some("--rows"),
            ArgumentProblem::Required,
        )),
        _ => Ok(parsed),
    }
}

/// Connects with the CLI's (Cli, Cli) origin and runs the action: its answer and the line's
/// outcome.
fn run(parsed: &Parsed, detail: Detail) -> Result<(Value, Outcome), Ended> {
    let rows = match &parsed.rows {
        Some(path) => Some(read_rows(path).map_err(Ended::Usage)?),
        None => None,
    };
    let config = RuntimeClientConfig::new(
        &parsed.state_root,
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    );
    let config = match detail {
        Detail::Codes => config.with_detail(Detail::Codes),
        Detail::Full => config,
    };
    let client =
        RuntimeClient::connect(config).map_err(|error| Ended::Failure(Failure::Runtime(error)))?;
    let instance = parsed.instance.as_deref().unwrap_or_default();
    let (answer, code) = match parsed.action {
        GET => (
            get(&client, instance),
            ReceiptCode::InstanceFactSnapshotCompleted,
        ),
        LIFT => (
            lift(&client, instance, CLI_DETECTOR),
            ReceiptCode::PublishFactsCompleted,
        ),
        _ => {
            let target = match (&parsed.instance, &parsed.server, &parsed.game) {
                (Some(instance), _, _) => Target::Instance(instance.clone()),
                (None, Some(server), _) => Target::Server(server.clone()),
                (None, None, Some(game)) => Target::Game(game.clone()),
                (None, None, None) => {
                    return Err(Ended::Usage(cli_line::usage_invalid(
                        Some("--instance"),
                        ArgumentProblem::Required,
                    )));
                }
            };
            (
                set(&client, &target, rows.unwrap_or_default(), CLI_DETECTOR),
                ReceiptCode::PublishFactsCompleted,
            )
        }
    };
    Ok((answer.map_err(Ended::Failure)?, Outcome::new(code)))
}

/// `--rows <file>`: a bounded JSON list of row objects whose members are strings. A file that
/// cannot be read or decoded is `actingctl_usage_invalid` (`--rows`, `unusable`) caused by the
/// failure; a larger file is (`--rows`, `out_of_range`, `range_max` its bound in bytes); JSON of
/// another shape is (`--rows`, `wrong_type`).
fn read_rows(path: &Path) -> Result<Vec<BTreeMap<String, FactScalar>>, Outcome> {
    let refused = |problem| cli_line::usage_invalid(Some("--rows"), problem);
    let bytes =
        crate::read_bounded(path, MAX_ROWS_FILE_BYTES).map_err(|problem| match problem {
            crate::InputProblem::Io(error, op) => refused(ArgumentProblem::Unusable)
                .caused_by(Outcome::from_io_error(&error, op).path(path)),
            crate::InputProblem::TooLarge(limit) => refused(ArgumentProblem::OutOfRange)
                .range_max(i64::try_from(limit).unwrap_or(i64::MAX)),
            _ => refused(ArgumentProblem::Unusable),
        })?;
    let value = serde_json::from_slice::<Value>(&bytes).map_err(|error| {
        refused(ArgumentProblem::Unusable).caused_by(crate::foreign_data_error(&error))
    })?;
    rows_from_json(&value).ok_or_else(|| refused(ArgumentProblem::WrongType))
}
