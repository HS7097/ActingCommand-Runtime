// SPDX-License-Identifier: AGPL-3.0-only

//! The scheduling operations (Workflow #434 LAB-1b; register §1.287), moved from actingctl's
//! `main.rs` and MCP's `ac_pause` and `ac_resume`. Each sends its request on the connection it is
//! given and answers what the Runtime answered:
//!
//! - `pause` (`actingctl pause`, MCP `ac_pause`): `PauseScheduling`; without an instance the
//!   pause is global (Workflow #191 ps1);
//! - `resume` (`actingctl resume`, MCP `ac_resume`): `ResumeScheduling`; MCP lifts only the pause
//!   it read (`SchedulingPauseExpectation`);
//! - `offset` (`actingctl task-offset`): one manual priority offset through the ordinary fact
//!   publication, `PublishFacts` (Workflow #308 slice 4a-2).

use actingcommand_contract::{
    EventId, FactObservation, FactRecord, FactScope, OwnerEpoch, RuntimeResult, SanitizationError,
    SchedulingPauseExpectation, SchedulingPauseReason, SchedulingPauseScope,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientError};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

/// The CLI's `pause` defaults (Workflow #191 ps1): the reason code and the instance drain
/// timeout.
pub const CLI_PAUSE_REASON: &str = "operator";
pub const CLI_DRAIN_TIMEOUT_MS: u64 = 60_000;
/// MCP `ac_pause`'s reason code by default, and its drain timeout in seconds by default and at
/// most.
pub const MCP_PAUSE_REASON: &str = "mcp.pause";
pub const MCP_DRAIN_TIMEOUT_S: u64 = 60;
pub const MCP_MAX_DRAIN_TIMEOUT_S: u64 = 600;
/// `source_detector` of the priority offsets `offset` publishes.
pub const TASK_OFFSET_DETECTOR: &str = "actingctl.task-offset";

/// The scope of a pause or resume: the instance `instance_alias`, or every instance without one.
pub fn pause_scope(instance_alias: Option<String>) -> SchedulingPauseScope {
    match instance_alias {
        Some(instance_alias) => SchedulingPauseScope::Instance { instance_alias },
        None => SchedulingPauseScope::Global,
    }
}

/// The instance a scope names; `None` for the global scope.
pub fn scope_alias(scope: &SchedulingPauseScope) -> Option<&str> {
    match scope {
        SchedulingPauseScope::Instance { instance_alias } => Some(instance_alias.as_str()),
        SchedulingPauseScope::Global => None,
    }
}

/// `pause`: scheduling paused on `scope` for `reason`, an instance's runs drained within
/// `drain_timeout_ms`; the Runtime's result.
pub fn pause(
    client: &RuntimeClient,
    scope: SchedulingPauseScope,
    reason: SchedulingPauseReason,
    drain_timeout_ms: u64,
) -> Result<RuntimeResult, RuntimeClientError> {
    client.pause_scheduling(scope, reason, drain_timeout_ms)
}

/// MCP's answer of a pause: `{paused, owner_epoch}`, the epoch `resume` expects back.
pub fn paused_answer(paused: &RuntimeResult, owner_epoch: OwnerEpoch) -> Value {
    json!({"paused": paused, "owner_epoch": owner_epoch})
}

/// `resume`: scheduling resumed on `scope`, whatever pause it holds; the Runtime's result.
pub fn resume(
    client: &RuntimeClient,
    scope: SchedulingPauseScope,
) -> Result<RuntimeResult, RuntimeClientError> {
    client.resume_scheduling(scope)
}

/// `resume` of only the pause the caller read (`expected`); the Runtime's result.
pub fn resume_expected(
    client: &RuntimeClient,
    scope: SchedulingPauseScope,
    expected: SchedulingPauseExpectation,
) -> Result<RuntimeResult, RuntimeClientError> {
    client.resume_scheduling_expected(scope, expected)
}

/// MCP's answer of a resume: `{resumed}`.
pub fn resumed_answer(resumed: &RuntimeResult) -> Value {
    json!({"resumed": resumed})
}

/// Why `offset` published nothing.
#[derive(Debug)]
pub enum OffsetFailure {
    /// The Runtime refused a request (its receipt's outcome on top), or did not answer.
    Runtime(RuntimeClientError),
    /// The runtime status lists no instance with this alias.
    InstanceUnknown(String),
    /// Without an instance: the registered instances do not name exactly one configured game.
    ScopeAmbiguous,
    /// The task identifier cannot form a valid priority offset record.
    Invalid(SanitizationError),
    /// The system clock is before the Unix epoch or out of range.
    Clock,
}

/// One priority offset `offset` published.
pub struct PublishedOffset {
    pub event_id: EventId,
    pub scope: FactScope,
    pub key: String,
    pub offset_milli: i64,
}

impl PublishedOffset {
    /// The answer: `{event_id, scope, key, offset_milli}`; a member that does not encode is the
    /// encoder's error.
    pub fn answer(&self) -> Result<Map<String, Value>, serde_json::Error> {
        let mut answer = Map::new();
        answer.insert("event_id".to_owned(), encoded(&self.event_id)?);
        answer.insert("scope".to_owned(), encoded(&self.scope)?);
        answer.insert("key".to_owned(), encoded(&self.key)?);
        answer.insert("offset_milli".to_owned(), encoded(&self.offset_milli)?);
        Ok(answer)
    }
}

/// One member of an answer, encoded.
fn encoded(value: &impl Serialize) -> Result<Value, serde_json::Error> {
    serde_json::to_value(value)
}

/// `offset`: the priority offset `offset_milli` of the task `task_id`, on the instance
/// `instance_alias` or, without one, on every instance of the configured game (the registered
/// instances must name exactly one), after one status read.
pub fn offset(
    client: &RuntimeClient,
    task_id: &str,
    offset_milli: i64,
    instance_alias: Option<&str>,
) -> Result<PublishedOffset, OffsetFailure> {
    let status = client.status().map_err(OffsetFailure::Runtime)?;
    let scope = match instance_alias {
        Some(alias) => {
            if !status
                .instances()
                .iter()
                .any(|registered| registered.instance_alias() == alias)
            {
                return Err(OffsetFailure::InstanceUnknown(alias.to_owned()));
            }
            FactScope::Instance {
                instance_id: alias.to_owned(),
            }
        }
        // A task-level offset applies to every instance of the configured game; the registered
        // instances must name exactly one.
        None => {
            let games = status
                .instances()
                .iter()
                .map(|registered| registered.game_id())
                .collect::<Vec<_>>();
            match games.first() {
                Some(Some(game_id)) if games.iter().all(|game| *game == Some(*game_id)) => {
                    FactScope::Game {
                        game_id: (*game_id).to_owned(),
                    }
                }
                _ => return Err(OffsetFailure::ScopeAmbiguous),
            }
        }
    };
    let observed_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .filter(|millis| *millis > 0)
        .ok_or(OffsetFailure::Clock)?;
    let record = FactRecord::priority_offset(
        scope,
        task_id,
        offset_milli,
        observed_at_unix_ms,
        TASK_OFFSET_DETECTOR,
    )
    .map_err(OffsetFailure::Invalid)?;
    let (scope, key) = (record.scope.clone(), record.key.clone());
    let event_id = client
        .publish_facts(FactObservation {
            records: vec![record],
        })
        .map_err(OffsetFailure::Runtime)?;
    Ok(PublishedOffset {
        event_id,
        scope,
        key,
        offset_milli,
    })
}
