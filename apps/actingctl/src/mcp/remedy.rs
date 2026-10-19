// SPDX-License-Identifier: AGPL-3.0-only

//! `ac_remedy_set` and `ac_remedy_get` (Workflow #411 B15 R6; model-b15 §5.1, §7.6), the MCP
//! sides of `actingctl remedy set|lift` and `remedy get` (`crate::remedy`). Both are general
//! tools (observer until U-3, always on: an agent writes the instance's target record). The
//! write goes the provenance way on its own (Agent, Adapter) connection, the origin the
//! Runtime records on the `fact.published` event; what the rows may name is the Runtime's
//! answer, passed on unchanged (`session_remedy_choices_invalid` on top of a refusal). The scope
//! is an instance or a game; `server` is no argument (`arguments_invalid`, `unknown`), since a
//! server id is not game-qualified. The answers are the CLI's, without its outcome; a `get`
//! answer larger than the output budget is exported, as `ac_facts` does.

use super::operator::record_provenance;
use super::runtime::client_error;
use super::tools::{
    self, Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument,
};
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::remedy::{self, Failure, Target};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{EventActor, EventSource};
use serde_json::{Map, Value, json};

/// The agent's origin: the writer of the instance's target record.
const AGENT: (EventActor, EventSource) = (EventActor::Agent, EventSource::Adapter);
/// The members an exported `get` answer keeps beside `export`.
const EXPORTED_HEADER: [&str; 3] = ["instance_alias", "read_at_unix_ms", "in_effect"];

pub(super) const SET_DESCRIPTION: &str = "Writes the remedy choices of one scope, the instance's target record session.remedy.choices, as actingctl remedy set does. Exactly one of instance (alias, instance_id or ADB port, resolved with one status read) or game names the scope; there is no server scope (a server id is not game-qualified, so a server record would reach every game sharing it), and a server argument is refused as arguments_invalid; rows is the scope's whole list, [] withdraws it. A row is {row: allow, need, capability} (this capability may serve this need), {row: none, need} (this need gets no remedy) or {row: parameter, capability, parameter, value} (a provider's parameter takes this value); every member is a string, every name a token the bound packages declare (a remedy's need and capabilities, a provided capability's parameters and options). The most specific record that applies to an instance applies as a whole (instance over game); without one the configuration and then the package defaults decide. The Runtime checks every write against the packages bound on every instance the scope covers and refuses a bad one whole: session_remedy_choices_invalid with field (the JSON pointer of the row and member) and instance_id (the instance a name is unknown on); nothing is written then. Every write is a new version (source_snapshot_id remedy-choices:<sha256>) and lifts a remedy stop it changes the inputs of. lift true, with instance and without rows, republishes the instance's own record unchanged, or an empty one when it has none (an empty instance record replaces the game record for that instance), with a new observation, as actingctl remedy lift does: it lifts a remedy stop on the instance after the cause was fixed by hand. Answers {event_id, scope, key, source_snapshot_id, observed_at_unix_ms, rows_total}, with previous_source_snapshot_id (null without an instance record) for lift. The write is recorded with the agent's origin.";

pub(super) const GET_DESCRIPTION: &str = "The remedy choice records that apply to one instance, as actingctl remedy get prints them; nothing is recorded. instance (alias, instance_id or ADB port) is resolved with one status read. Answers {instance_alias, read_at_unix_ms, records, in_effect}: records are the session.remedy.choices records of the instance and its game, most specific first, each {scope, observed_at_unix_ms, expires_at_unix_ms when it has one, source_snapshot_id, rows, in_effect}; in_effect is the scope of the record that applies as a whole (the most specific one not expired at read_at_unix_ms), null when none does (then the configuration and the package defaults decide). An answer larger than the output budget is written whole to %TEMP%\\actingcommand-mcp\\materials\\<sha256>.json and answered as {instance_alias, read_at_unix_ms, in_effect, export {path, sha256, size}} with the warning answer_exported.";

/// `ac_remedy_set {instance | game, rows}` or `{instance, lift: true}`; any other argument,
/// `server` included, is `arguments_invalid` (`unknown`).
pub(super) fn set(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance", "game", "rows", "lift"])?;
    let target = match (arguments.string("instance")?, arguments.string("game")?) {
        (Some(instance), None) => Target::Instance(instance.to_owned()),
        (None, Some(game)) => Target::Game(game.to_owned()),
        (None, None) => return Err(invalid_argument("instance", ArgumentProblem::Required)),
        (Some(_), Some(_)) => {
            return Err(invalid_argument("instance", ArgumentProblem::Conflicting));
        }
    };
    let lift = match arguments.value("lift") {
        None => false,
        Some(Value::Bool(lift)) => *lift,
        Some(_) => return Err(invalid_argument("lift", ArgumentProblem::WrongType)),
    };
    let rows = match (lift, arguments.value("rows")) {
        (true, Some(_)) => return Err(invalid_argument("rows", ArgumentProblem::Conflicting)),
        (true, None) => None,
        (false, None) => return Err(invalid_argument("rows", ArgumentProblem::Required)),
        (false, Some(rows)) => Some(
            remedy::rows_from_json(rows)
                .ok_or_else(|| invalid_argument("rows", ArgumentProblem::WrongType))?,
        ),
    };
    if lift && !matches!(target, Target::Instance(_)) {
        return Err(invalid_argument("lift", ArgumentProblem::Conflicting));
    }
    let connection = context.runtime.connect_fresh(AGENT.0, AGENT.1)?;
    let write = record_provenance(connection, "ac_remedy_set", None, AGENT, None)?;
    let answer = match (rows, &target) {
        (Some(rows), _) => remedy::set(&write.client, &target, rows, remedy::MCP_DETECTOR),
        (None, Target::Instance(selector)) => {
            remedy::lift(&write.client, selector, remedy::MCP_DETECTOR)
        }
        (None, _) => return Err(invalid_argument("lift", ArgumentProblem::Conflicting)),
    }
    .map_err(|failure| tool_error(&failure))?;
    Ok(ToolSuccess {
        result: answer,
        warnings: write.warnings,
    })
}

/// `ac_remedy_get {instance}`.
pub(super) fn get(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance"])?;
    let selector = arguments.required_string("instance")?;
    let connected = context.runtime.connect()?;
    let answer = remedy::get(&connected.client, selector).map_err(|failure| match failure {
        Failure::Runtime(error) => context.runtime.failure(&connected, &error),
        other => tool_error(&other),
    })?;
    if tools::fits_success(&answer) {
        return Ok(ToolSuccess::new(answer));
    }
    let export = super::events::export_json(&answer)?;
    let mut exported = EXPORTED_HEADER
        .iter()
        .filter_map(|member| {
            answer
                .get(*member)
                .map(|value| ((*member).to_owned(), value.clone()))
        })
        .collect::<Map<_, _>>();
    let mut warning = Outcome::new(ActingctlCode::AnswerExported);
    if let Some(path) = export["path"].as_str() {
        warning = warning.path(path);
    }
    exported.insert("export".to_owned(), export);
    Ok(ToolSuccess {
        result: Value::Object(exported),
        warnings: vec![tools::warning(&warning)],
    })
}

/// A remedy failure as a tool error: a selector miss and a refused record are `usage`, a clock
/// failure `runtime`, a Runtime failure the client's.
fn tool_error(failure: &Failure) -> ToolError {
    match failure {
        Failure::Runtime(error) => client_error(error),
        Failure::Clock => ToolError::new("runtime", ActingctlCode::ClockUnavailable),
        Failure::Selector { .. } | Failure::Record(_) => {
            ToolError::outcome("usage", failure.outcome())
        }
    }
}

pub(super) fn set_input() -> Value {
    let name = |what: &str| {
        json!({
            "type": "string",
            "minLength": 1,
            "description": what,
        })
    };
    json!({
        "type": "object",
        "properties": {
            "instance": name("Instance alias, instance_id or ADB port: that instance's record."),
            "game": name("Game id: the record of every instance of that game."),
            "rows": {
                "type": "array",
                "maxItems": 256,
                "items": {
                    "type": "object",
                    "properties": {
                        "row": {"type": "string", "enum": ["allow", "none", "parameter"]},
                        "need": {"type": "string"},
                        "capability": {"type": "string"},
                        "parameter": {"type": "string"},
                        "value": {"type": "string"},
                    },
                    "required": ["row"],
                    "additionalProperties": false,
                },
                "description": "The scope's whole list; [] withdraws it. Refused with lift true.",
            },
            "lift": {
                "type": "boolean",
                "description": "true: republish the instance's own record with a new observation (instance only, without rows).",
            },
        },
        "additionalProperties": false,
    })
}

pub(super) fn set_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "event_id": {"type": "string"},
            "scope": {"type": "object"},
            "key": {"type": "string"},
            "source_snapshot_id": {"type": "string"},
            "observed_at_unix_ms": {"type": "integer"},
            "rows_total": {"type": "integer"},
            "previous_source_snapshot_id": {"type": ["string", "null"]},
        },
        "required": ["event_id", "scope", "key", "source_snapshot_id", "observed_at_unix_ms", "rows_total"],
    })
}

pub(super) fn get_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "instance_alias": {"type": "string"},
            "read_at_unix_ms": {"type": "integer"},
            "records": {"type": "array", "items": {"type": "object"}},
            "in_effect": {"type": ["object", "null"]},
            "export": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "sha256": {"type": "string"},
                    "size": {"type": "integer"},
                },
                "required": ["path", "sha256", "size"],
            },
        },
        "required": ["instance_alias", "read_at_unix_ms", "in_effect"],
    })
}
