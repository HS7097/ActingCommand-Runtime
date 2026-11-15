// SPDX-License-Identifier: AGPL-3.0-only

//! #463 S1-7 (`contracts/lab-recording.md`, "A recording holds its instance"; register §1.424):
//! the Runtime half of a recording's hold, for the `record` command and for the `--record` presses
//! and application operations. The recording library keeps the hold's facts and says what they
//! mean (`actingcommand_lab::RecordingHold`); the requests are `lab_ops::scheduling`'s, on a
//! connection to the Runtime the `--record` commands reach (`ACTINGCOMMAND_RUNTIME_STATE_ROOT` or
//! the default state root), as every press connects.
//!
//! - `record start` ([`take`]): one status read, then, when the instance's scheduling is not
//!   paused, the pause of the `pause` row with the recording's own reason, `lab-record-<ms>`.
//!   No Runtime answering: nothing is held and the answer says `runtime_not_answering`. A status or
//!   pause the Runtime refuses fails the start before anything is written.
//! - The end, `record stop` and `record archive` ([`end`]): another party's pause is left alone;
//!   the own pause is found by its reason and resumed by the pause the status shows now
//!   (`lab_ops::scheduling::release_recording_hold`). A release that fails, or a Runtime that does
//!   not answer, keeps the hold: the end's own work stands, and the answer is the release's failure
//!   with the end's answer, the kept hold and the `resume` row's command in its details
//!   ([`Kept::answer`]), as the Lab's rescue module answers a hold it keeps.
//! - A press or an application operation with `--record` ([`hold_warning`], [`hold_warning_read`]):
//!   one status read before it, a press's on its own connection, an application operation's on a
//!   connection of its own; `instance_not_held` in its `record.warnings` when the status shows the
//!   instance's scheduling not paused, `instance_hold_unread` when the read got no answer. The press
//!   or the operation is made and recorded all the same.

use super::session_record::SessionRecordContext;
use crate::{CliError, CliOutcome, runtime_state_root, write_json_file_atomic};
use actingcommand_contract::{SchedulingPauseExpectation, SchedulingPauseReason};
use actingcommand_lab::{
    LAB_RECORDING_WARNING_HOLD_UNREAD, LAB_RECORDING_WARNING_INSTANCE_NOT_HELD,
    LAB_RECORDING_WARNING_RUNTIME_NOT_ANSWERING, RecordingHold, RecordingHoldFound,
    RecordingHoldView,
};
use actingcommand_lab_ops::scheduling::{
    self, RecordingHoldKept, RecordingHoldReleased, RecordingHoldTaken,
};
use actingcommand_runtime_client::RuntimeClient;
use serde_json::{Map, Value, json};
use std::path::Path;

/// A connection to the Runtime the `--record` commands reach; when none can be made (no state
/// root located, no runtime-info, nobody listening, a handshake that fails), the text of why.
fn connect() -> Result<RuntimeClient, String> {
    let state_root = runtime_state_root().map_err(|error| error.message)?;
    RuntimeClient::connect(crate::lab_client_config(state_root)).map_err(|error| error.to_string())
}

/// The recording's own pause reason as the Runtime takes it.
fn pause_reason(reason: &str) -> CliOutcome<SchedulingPauseReason> {
    SchedulingPauseReason::new(reason).map_err(|_| {
        CliError::usage(format!(
            "the recording's hold reason {reason} is no scheduling pause reason"
        ))
    })
}

/// `record start`'s hold of `instance` for the recording that starts at `started_at_unix_ms`, the
/// session header it replaces holding `replaced`: what the new header holds, and the answer's
/// member `hold` (`{state, reason?, taken_over?, paused?, warnings?, runtime_error?}`). A status
/// or pause the Runtime refuses or leaves unanswered after the connection was made is the start's
/// failure: nothing is written.
pub(super) fn take(
    instance: &str,
    started_at_unix_ms: u64,
    replaced: Option<&RecordingHold>,
) -> CliOutcome<(Option<RecordingHold>, Value)> {
    let reason = RecordingHold::reason_for(started_at_unix_ms);
    let own = pause_reason(&reason)?;
    let client = match connect() {
        Ok(client) => client,
        Err(cause) => {
            let hold = RecordingHold::at_start(&RecordingHoldFound::NoRuntime, replaced);
            let mut answer = start_answer(hold.as_ref(), replaced);
            answer["warnings"] = json!([LAB_RECORDING_WARNING_RUNTIME_NOT_ANSWERING]);
            answer["runtime_error"] = json!(cause);
            return Ok((hold, answer));
        }
    };
    let taken = scheduling::hold_for_recording(&client, instance, own).map_err(|error| {
        let mut refused = crate::runtime_slice_cli::map_runtime_error(error);
        refused.message = format!(
            "record start paused nothing and started nothing on {instance}: {}",
            refused.message
        );
        if refused.details.is_none() {
            refused =
                refused.with_details(json!({"instance": instance, "hold": {"reason": reason}}));
        }
        refused
    })?;
    let (found, paused) = match taken {
        RecordingHoldTaken::Paused(paused) => (RecordingHoldFound::Paused(reason), Some(paused)),
        RecordingHoldTaken::Shown(shown) => {
            (RecordingHoldFound::Shown(shown.as_str().to_owned()), None)
        }
    };
    let hold = RecordingHold::at_start(&found, replaced);
    let mut answer = start_answer(hold.as_ref(), replaced);
    if let Some(paused) = paused {
        answer["paused"] = json!(paused);
    }
    Ok((hold, answer))
}

/// `record start`'s `hold` member: the view of what the recording holds, with `taken_over: true`
/// when its own pause is the one of the recording whose header it replaced.
fn start_answer(hold: Option<&RecordingHold>, replaced: Option<&RecordingHold>) -> Value {
    let mut answer = json!(RecordingHoldView::of(hold, true));
    if hold.is_some_and(|hold| hold.taken_over(replaced)) {
        answer["taken_over"] = Value::Bool(true);
    }
    answer
}

/// An own hold that stays at the end of a recording: why the release did not lift it (the
/// release's failure as the Lab answers a Runtime failure: `runtime_not_running` when no Runtime
/// answered), its reason, and the pause the status showed with that reason, when it was read.
pub(super) struct Kept {
    error: CliError,
    reason: String,
    expected: Option<SchedulingPauseExpectation>,
}

impl Kept {
    /// The answer of an end whose own hold stays: the release's failure, its message saying that
    /// `ended` (the end's own work) is done and the hold kept, its details the end's answer `done`
    /// with `hold` (`{reason, own: true, kept: true}`) and `next.resume`, the `resume` row's command
    /// that lifts the pause (with `--expect` naming the pause the status showed, when it was read).
    pub(super) fn answer(self, instance: &str, ended: &str, done: Value) -> CliError {
        let Self {
            mut error,
            reason,
            expected,
        } = self;
        let mut resume = vec![json!("resume"), json!(instance)];
        if let Some(expected) = expected {
            resume.push(json!("--expect"));
            resume.push(json!(json!(expected).to_string()));
        }
        error.message = format!(
            "{ended}; its hold {reason} on {instance} is kept, since the release failed: {}; \
             resume it once status shows the instance paused with reason {reason}",
            error.message
        );
        let mut details = match done {
            Value::Object(members) => members,
            other => Map::from_iter([("result".to_owned(), other)]),
        };
        if let Some(cause) = error.details.take() {
            details.insert("cause".to_owned(), cause);
        }
        details.insert(
            "hold".to_owned(),
            json!({"reason": reason, "own": true, "kept": true}),
        );
        details.insert("next".to_owned(), json!({"resume": resume}));
        error.with_details(Value::Object(details))
    }
}

/// What the end of a recording did with what its session header held.
pub(super) enum Ended {
    /// It held nothing.
    Nothing,
    /// The answer's member `hold`: `{reason, own, release}`, `release` `not_own` (another
    /// party's pause, left alone), `resumed` (with the Runtime's `resumed`) or `already_gone` (no
    /// pause showed the reason; `shown`, the reason of the one that did, or `null`).
    Hold(Value),
    /// The own hold stays ([`Kept::answer`]).
    Kept(Box<Kept>),
}

/// The end (`record stop`, `record archive`) of the recording whose session header `record`, at
/// `path`, held `held` before the end; the header is already written with what it keeps at the
/// end (`RecordingHold::ended`: an own pause kept, another party's nothing). An own pause is
/// released ([`release`]); once it is lifted or found gone, the header is written again without
/// it. A header that cannot be written then is the failure, `hold` in its details.
pub(super) fn end(
    path: &Path,
    record: &mut SessionRecordContext,
    held: Option<RecordingHold>,
) -> CliOutcome<Ended> {
    let Some(held) = held else {
        return Ok(Ended::Nothing);
    };
    if !held.own {
        return Ok(Ended::Hold(json!({
            "reason": held.reason,
            "own": false,
            "release": "not_own"
        })));
    }
    match release(&record.instance, &held) {
        Ok(released) => {
            record.hold = None;
            write_json_file_atomic(path, &*record)
                .map_err(|error| error.with_details(json!({"hold": released})))?;
            Ok(Ended::Hold(released))
        }
        Err(kept) => Ok(Ended::Kept(Box::new(kept))),
    }
}

/// The release of the recording's own hold `held` on `instance`: the answer's member `hold`, or
/// why it stays.
fn release(instance: &str, held: &RecordingHold) -> Result<Value, Kept> {
    let kept = |error: CliError, expected| Kept {
        error,
        reason: held.reason.clone(),
        expected,
    };
    let own = pause_reason(&held.reason).map_err(|error| kept(error, None))?;
    let client = connect().map_err(|cause| kept(CliError::runtime_not_running(cause), None))?;
    match scheduling::release_recording_hold(&client, instance, &own) {
        Ok(RecordingHoldReleased::Resumed(resumed)) => Ok(json!({
            "reason": held.reason,
            "own": true,
            "release": "resumed",
            "resumed": resumed
        })),
        Ok(RecordingHoldReleased::Gone(shown)) => Ok(json!({
            "reason": held.reason,
            "own": true,
            "release": "already_gone",
            "shown": shown
        })),
        Err(RecordingHoldKept { error, expected }) => Err(kept(
            crate::runtime_slice_cli::map_runtime_error(*error),
            expected,
        )),
    }
}

/// #463 S1-7: the warning word a `--record` press or application operation on `instance` adds to
/// its `record.warnings`, from one status read on its own connection `client` before it: none when
/// the instance's scheduling shows paused (its own pause or the global pause), `instance_not_held`
/// when it does not, `instance_hold_unread` when the read got no answer.
pub(crate) fn hold_warning(client: &RuntimeClient, instance: &str) -> Option<&'static str> {
    match scheduling::instance_scheduling_paused(client, instance) {
        Ok(true) => None,
        Ok(false) => Some(LAB_RECORDING_WARNING_INSTANCE_NOT_HELD),
        Err(_) => Some(LAB_RECORDING_WARNING_HOLD_UNREAD),
    }
}

/// #463 S1-7: [`hold_warning`] read on a connection of its own: `instance_hold_unread` when none
/// can be made (`session app --record`, whose operation's own connection must not meet a read that
/// failed on it).
pub(crate) fn hold_warning_read(instance: &str) -> Option<&'static str> {
    match connect() {
        Ok(client) => hold_warning(&client, instance),
        Err(_) => Some(LAB_RECORDING_WARNING_HOLD_UNREAD),
    }
}

/// `record`, an answer's member `record`, with `warning` added to its `warnings` (the member made
/// when it has none); nothing without a warning or on a `record` that is no object.
pub(crate) fn add_hold_warning(record: &mut Value, warning: Option<&str>) {
    let (Some(warning), Some(record)) = (warning, record.as_object_mut()) else {
        return;
    };
    if let Value::Array(warnings) = record
        .entry("warnings")
        .or_insert_with(|| Value::Array(Vec::new()))
    {
        warnings.push(Value::String(warning.to_owned()));
    }
}
