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
//!
//! #463 S1-7 (`contracts/lab-recording.md`, "A recording holds its instance"): the hold of a
//! recording, which the Lab's `record` commands take and release through these: `record start`
//! reads the pause the instance shows ([`instance_pause_reason`]) and, when it shows none, writes
//! its session header naming its own hold and then pauses the instance's scheduling as `pause`
//! does ([`pause_for_recording`]; fix round 2: the header first, so that a start that is killed in
//! the drain leaves a header that names the pause); a pause the Runtime refused left no pause
//! ([`pause_refused`]). The end of the recording resumes only the pause that shows its reason,
//! found by that reason as the rescue module's `release_own` finds its own
//! ([`release_recording_hold`]), and each `--record` press or application operation reads whether
//! the instance is held ([`instance_scheduling_paused`]).

use actingcommand_contract::{
    EventId, FactObservation, FactRecord, FactScope, InstancePauseState, OwnerEpoch,
    RuntimeControlPlaneStatus, RuntimeResult, SanitizationError, SchedulingPauseExpectation,
    SchedulingPauseReason, SchedulingPauseScope,
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
/// The drain of the Lab's `pause` row when none is given (the operation table's `drain` default,
/// 300 s); #463 S1-7: a recording's hold waits as that pause waits.
pub const LAB_PAUSE_DRAIN_TIMEOUT_MS: u64 = 300_000;
/// `source_detector` of the priority offsets `offset` publishes.
const TASK_OFFSET_DETECTOR: &str = "actingctl.task-offset";

/// The scope of a pause or resume: the instance `instance_alias`, or every instance without one.
pub fn pause_scope(instance_alias: Option<String>) -> SchedulingPauseScope {
    match instance_alias {
        Some(instance_alias) => SchedulingPauseScope::Instance { instance_alias },
        None => SchedulingPauseScope::Global,
    }
}

// Workflow #434 LAB-6S4a (0.12.63): `scope_alias`, the instance MCP's provenance and its jobs
// named, went with the provenance before a write.

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

/// #463 S1-7: `record start`'s status read on `client`: the reason of the pause it shows on the
/// instance `alias`, or none. A status the Runtime refuses, or does not answer, is the error.
pub fn instance_pause_reason(
    client: &RuntimeClient,
    alias: &str,
) -> Result<Option<SchedulingPauseReason>, RuntimeClientError> {
    let status = client.status()?;
    Ok(instance_pause(&status, alias).map(|pause| pause.pause_reason.clone()))
}

/// #463 S1-7: `record start`'s pause of the instance `alias` on `client` for the recording's own
/// reason `hold`, as the `pause` row pauses it, with that row's drain
/// ([`LAB_PAUSE_DRAIN_TIMEOUT_MS`]): the answer waits as that pause waits (a run in flight drained,
/// or stopped at its next checkpoint after the drain; the client waits for the receipt the drain,
/// the pause's checkpoint grace and its IO timeout). The Runtime's result.
pub fn pause_for_recording(
    client: &RuntimeClient,
    alias: &str,
    hold: SchedulingPauseReason,
) -> Result<RuntimeResult, RuntimeClientError> {
    pause(
        client,
        pause_scope(Some(alias.to_owned())),
        hold,
        LAB_PAUSE_DRAIN_TIMEOUT_MS,
    )
}

/// #463 S1-7 fix round 2: whether the failure `error` of a pause is the Runtime's refusal, a
/// receipt that says no pause was made: the Runtime's own receipt of the pause arrived and carries
/// the failure (the client keeps such a receipt as the received one). By the pause's rules
/// (`contracts/scheduling-pause.md`) a refused pause changes nothing, and a stage that fails fails
/// the request as a whole and lifts the gate it closed. Every other failure (the connection lost,
/// the wait for the receipt ended, a receipt that did not match the request) leaves it unknown
/// whether the pause was made.
pub fn pause_refused(error: &RuntimeClientError) -> bool {
    error.received_receipt().is_some()
}

/// #463 S1-7: what the release of a recording's own hold did.
#[derive(Debug)]
pub enum RecordingHoldReleased {
    /// The instance's pause that showed the hold's reason was resumed by the pause it showed: the
    /// Runtime's result.
    Resumed(Box<RuntimeResult>),
    /// No pause of the instance showed the hold's reason: the reason of the pause it showed
    /// instead (another party's), or none. Nothing was sent.
    Gone(Option<SchedulingPauseReason>),
}

/// #463 S1-7: the release of the recording's own hold `hold` on the instance `alias`, as the
/// rescue module's `release_own` releases its own: one status read; only the instance's pause
/// that shows exactly `hold` is resumed, by the pause it shows now (that owner epoch, that
/// revision: the Runtime's restore numbers every pause anew, so a remembered revision is not
/// enough); a pause with another reason, or none, is left as it is. A status or resume the
/// Runtime refuses, or does not answer, is the error: the hold stays.
pub fn release_recording_hold(
    client: &RuntimeClient,
    alias: &str,
    hold: &SchedulingPauseReason,
) -> Result<RecordingHoldReleased, RuntimeClientError> {
    let status = client.status()?;
    let shown = instance_pause(&status, alias);
    let Some(pause) = shown.filter(|pause| pause.pause_reason == *hold) else {
        return Ok(RecordingHoldReleased::Gone(
            shown.map(|pause| pause.pause_reason.clone()),
        ));
    };
    let expected = SchedulingPauseExpectation {
        owner_epoch: status.owner_epoch(),
        revision: pause.revision,
    };
    resume_expected(client, pause_scope(Some(alias.to_owned())), expected)
        .map(|resumed| RecordingHoldReleased::Resumed(Box::new(resumed)))
}

/// #463 S1-7: whether one status read shows the scheduling of the instance `alias` paused: its
/// own pause, or the global pause.
pub fn instance_scheduling_paused(
    client: &RuntimeClient,
    alias: &str,
) -> Result<bool, RuntimeClientError> {
    let status = client.status()?;
    Ok(status.scheduling_pause().is_some() || instance_pause(&status, alias).is_some())
}

/// The pause the status `status` shows on the instance `alias`, if any.
fn instance_pause<'a>(
    status: &'a RuntimeControlPlaneStatus,
    alias: &str,
) -> Option<&'a InstancePauseState> {
    status
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == alias)
        .and_then(|instance| instance.pause())
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
