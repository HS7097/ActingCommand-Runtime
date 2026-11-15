// SPDX-License-Identifier: AGPL-3.0-only

//! #463 S1-7 (`contracts/lab-recording.md`, "A recording holds its instance"; register §1.424):
//! the Runtime half of a recording's hold, for the `record` command and for the `--record` presses
//! and application operations. The recording library keeps the hold's facts and says what they
//! mean (`actingcommand_lab::RecordingHold`); the requests are `lab_ops::scheduling`'s, on a
//! connection to the Runtime the `--record` commands reach (`ACTINGCOMMAND_RUNTIME_STATE_ROOT`,
//! else (#463 S1-8) the state root of the installation actinglab runs from, else the default state
//! root: `crate::state_roots::runtime_state_root`), as every press connects.
//!
//! - `record start` ([`take`]; fix round 2: the header knows the hold before the Runtime is asked
//!   to pause): a connection and one status read, with nothing to undo so far. An instance the
//!   status shows paused is held by that pause and nothing is sent. Otherwise the session header is
//!   written first, naming the recording's own hold `lab-record-<ms>`, and only then is the pause
//!   of the `pause` row sent: a pause the Runtime refuses puts the header back as it was (none when
//!   there was none) and fails the start; any other failure of the pause leaves the header as
//!   written, since a pause with that reason may exist, and fails the start with the hold in its
//!   details, so that `record stop` or `record start --force` releases it by its reason. Nothing
//!   listening where the installation's Runtime would ([`reach`]: no runtime-info, or the address
//!   it names refuses the connection): nothing is held and the answer says `no_runtime_listening`.
//!   Something listening that does not complete the handshake, or a status the Runtime refuses or
//!   leaves unanswered, fails the start before anything is written; so (the fix round after the
//!   train's review) do a state root that cannot be resolved (`validation_failed`, as the Lab's
//!   other commands answer it), a plain connection that times out and a runtime-info that names no
//!   address (as a handshake that does not complete): the Lab cannot tell that nothing listens.
//! - The end, `record stop` and `record archive` ([`end`]): another party's pause is left alone;
//!   the own pause is found by its reason and resumed by the pause the status shows now
//!   (`lab_ops::scheduling::release_recording_hold`). A release that fails, or a Runtime that does
//!   not answer, keeps the hold: the end's own work stands, and the answer is the release's failure
//!   with the end's answer, the kept hold and `next.release`, the command that tries the release
//!   again and can release only the recording's own pause ([`Kept::answer`]), as the Lab's rescue
//!   module answers a hold it keeps.
//! - A press or an application operation with `--record` ([`hold_warning_read`]): one status read
//!   before it, on a connection of its own, so that a read that fails never stands in its way;
//!   `instance_not_held` in its `record.warnings` when the status shows the instance's scheduling
//!   not paused, `instance_hold_unread` when the read got no answer. The press or the operation is
//!   made and recorded all the same.

use super::session_record::{SessionRecordContext, with_archive_context};
use crate::{CliError, CliOutcome, runtime_state_root, write_json_file_atomic};
use actingcommand_contract::{RUNTIME_INFO_FILE, RuntimeInfo, SchedulingPauseReason};
use actingcommand_lab::{
    LAB_RECORDING_WARNING_HOLD_UNREAD, LAB_RECORDING_WARNING_INSTANCE_NOT_HELD,
    LAB_RECORDING_WARNING_NO_RUNTIME_LISTENING, LAB_RECORDING_WARNING_REPLACED_HEADER_UNREAD,
    RecordingHold, RecordingHoldFound, RecordingHoldView,
};
use actingcommand_lab_ops::scheduling::{self, RecordingHoldReleased};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use serde_json::{Map, Value, json};
use std::fs;
use std::io;
use std::net::TcpStream;
use std::path::Path;

/// A connection to the Runtime the `--record` commands reach; when none can be made (no state
/// root located, no runtime-info, nobody listening, a handshake that fails), the text of why.
fn connect() -> Result<RuntimeClient, String> {
    let state_root = runtime_state_root().map_err(|error| error.message)?;
    RuntimeClient::connect(crate::lab_client_config(state_root)).map_err(|error| error.to_string())
}

/// What `record start` met when it reached for the Runtime the `--record` commands reach.
enum Reached {
    /// A connection, its handshake done.
    Connected(RuntimeClient),
    /// Nothing listens ([`Probed::NothingListens`]). The text of why.
    NothingListens(String),
    /// Something may listen there, but no connection could be made (the connection's handshake,
    /// `Health` and `BindChannel`, failed, was refused or got no answer; the fix round after the
    /// train's review: or the plain connection timed out, or the runtime-info names no address):
    /// the failure as the Lab's commands answer a Runtime failure
    /// (`runtime_slice_cli::map_runtime_error`).
    NotHandshaken(CliError),
}

/// #463 S1-7 fix round 2: the connection `record start` holds its instance on, or which of two
/// cases kept it from one. The client's connect reads the runtime-info, opens the connection and
/// shakes hands in one call, and its failure does not say by itself whether anything listened; so
/// after a failed connect one plain connection to the address the runtime-info names, opened and
/// closed at once with nothing sent, within the client's IO timeout ([`probe`]), tells them apart
/// ([`not_reached`]). The fix round after the train's review (the coordinator's ruling: a recording
/// never starts unheld because the Lab could not tell): a state root that cannot be resolved (an
/// empty variable, neither `LOCALAPPDATA` nor `APPDATA`; #463 S1-8: an installation whose state
/// root cannot be located) is the caller's mistake or a broken installation, not "no Runtime": the
/// failure the Lab's other commands answer for it (`runtime_state_root`).
fn reach() -> CliOutcome<Reached> {
    let config = crate::lab_client_config(runtime_state_root()?);
    Ok(match RuntimeClient::connect(config.clone()) {
        Ok(client) => Reached::Connected(client),
        Err(error) => match not_reached(error, probe(&config)) {
            Ok(cause) => Reached::NothingListens(cause),
            Err(error) => Reached::NotHandshaken(error),
        },
    })
}

/// The fix round after the train's review: what one plain connection to the Runtime's address
/// met after the client's connect failed ([`probe`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Probed {
    /// Nothing listens: the state root holds no runtime-info (a Runtime publishes one when it
    /// starts and removes it when it stops), or the address it names refused the connection.
    NothingListens,
    /// Something may listen: the connection was accepted (its handshake failed), timed out or
    /// failed otherwise, or the runtime-info cannot be read or names no address.
    MayListen,
}

impl Probed {
    /// What a plain connection to an address the runtime-info names met: refused, nothing listens
    /// there; anything else (accepted, timed out, any other failure), something may.
    pub(crate) fn of_connection(connected: io::Result<()>) -> Self {
        match connected {
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => Self::NothingListens,
            _ => Self::MayListen,
        }
    }
}

/// The fix round after the train's review: what a failed connect of the client (`error`) means for
/// `record start`, by what the plain connection met (`probed`): the text of why nothing listens,
/// or the failure as the Lab's commands answer a Runtime failure, which fails the start as a
/// handshake that does not complete fails it (one code path, one code).
pub(crate) fn not_reached(error: RuntimeClientError, probed: Probed) -> Result<String, CliError> {
    match probed {
        Probed::NothingListens => Ok(error.to_string()),
        Probed::MayListen => Err(crate::runtime_slice_cli::map_runtime_error(error)),
    }
}

/// One plain connection to the address the runtime-info of `config`'s state root names, opened
/// and closed at once with nothing sent, within `config`'s IO timeout: what it met. No
/// runtime-info there (not found) is [`Probed::NothingListens`]; one that cannot be read, decoded
/// or names no address is [`Probed::MayListen`] (the fix round after the train's review: the Lab
/// cannot tell that nothing listens).
fn probe(config: &RuntimeClientConfig) -> Probed {
    let bytes = match fs::read(config.state_root().join(RUNTIME_INFO_FILE)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Probed::NothingListens,
        Err(_) => return Probed::MayListen,
    };
    let address = serde_json::from_slice::<RuntimeInfo>(&bytes)
        .ok()
        .and_then(|info| info.socket_addr().ok());
    match address {
        Some(address) => Probed::of_connection(
            TcpStream::connect_timeout(&address, config.io_timeout()).map(drop),
        ),
        None => Probed::MayListen,
    }
}

/// The recording's own pause reason as the Runtime takes it.
fn pause_reason(reason: &str) -> CliOutcome<SchedulingPauseReason> {
    SchedulingPauseReason::new(reason).map_err(|_| {
        CliError::usage(format!(
            "the recording's hold reason {reason} is no scheduling pause reason"
        ))
    })
}

/// `error` with the hold a session header names in its details (`hold {reason, own}`), when it
/// names one: a pause with that reason may exist, and the header's end releases it by its reason.
pub(super) fn with_hold(error: CliError, hold: Option<&RecordingHold>) -> CliError {
    match hold {
        Some(hold) => with_archive_context(error, &[("hold", json!(hold))]),
        None => error,
    }
}

/// The session header `record` written at `path`; one that cannot be written is the failure, with
/// the hold the header was to name in its details.
fn write_header(path: &Path, record: &SessionRecordContext) -> CliOutcome<()> {
    write_json_file_atomic(path, record).map_err(|error| with_hold(error, record.hold.as_ref()))
}

/// The session header at `path` put back as it was before this start: `before`'s bytes, or no
/// header when there was none (fix round 2).
fn put_back(path: &Path, before: Option<&[u8]>) -> std::io::Result<()> {
    match before {
        Some(bytes) => fs::write(path, bytes),
        None => fs::remove_file(path),
    }
}

/// The start's failure `error` while no pause of its own can exist: its message says the start
/// paused and started nothing on `instance`; its details, when it has none, name the instance and
/// the reason of the hold it was to take.
fn started_nothing(instance: &str, reason: &str, mut error: CliError) -> CliError {
    error.message = format!(
        "record start paused nothing and started nothing on {instance}: {}",
        error.message
    );
    if error.details.is_none() {
        error = error.with_details(json!({"instance": instance, "hold": {"reason": reason}}));
    }
    error
}

/// The start's failure `error` once the pause was sent and got no answer that says it was not
/// made (the connection lost, the wait ended, a receipt that does not match): the session header
/// stays as written and names the recording's own hold `hold`, since a pause with its reason may
/// exist; the message says so and names what releases it, the details carry the hold.
fn pause_unsettled(instance: &str, hold: Option<&RecordingHold>, mut error: CliError) -> CliError {
    error.message = format!(
        "record start sent the pause of {instance} and got no answer that says no pause was made: \
         {}; the session header stays and names the recording's own hold, so that record stop or \
         record start --force releases a pause with its reason",
        error.message
    );
    with_hold(error, hold)
}

/// `record start`'s hold of the instance of `record`, the session header it writes at `path`;
/// `replaced` the hold of the header it replaces, `before` that header's bytes (`None`: there was
/// none, or they could not be read). It writes the header with what the recording holds and
/// answers the answer's member `hold` (`{state, reason?, taken_over?, paused?, warnings?,
/// runtime_error?}`):
/// - a connection and one status read come first, with nothing written: something that listens
///   but does not complete the handshake, or a status refused or unanswered, fails the start (the
///   fix round after the train's review: so do a state root that cannot be resolved, a plain
///   connection that times out and a runtime-info that names no address, [`reach`]); nothing
///   listening (no runtime-info, or a refused connection): the header holds what it carries over,
///   and the answer warns `no_runtime_listening`;
/// - the instance shown paused: the header holds that pause, its own when taken over, another
///   party's otherwise; nothing is sent;
/// - otherwise the header is written naming the own hold, then the pause is sent: a refusal puts
///   the header back as it was and fails the start; any other failure leaves the header and fails
///   the start with the hold in its details ([`pause_unsettled`]).
///
/// A header that cannot be written fails the start with the hold in its details.
pub(super) fn take(
    path: &Path,
    record: &mut SessionRecordContext,
    replaced: Option<&RecordingHold>,
    before: Option<&[u8]>,
) -> CliOutcome<Value> {
    let instance = record.instance.clone();
    let reason = RecordingHold::reason_for(record.started_at_unix_ms);
    let own = pause_reason(&reason)?;
    // The fix round after the train's review: a state root that cannot be resolved fails the
    // start before anything is written, as a handshake that does not complete fails it.
    let reached = reach().map_err(|error| started_nothing(&instance, &reason, error))?;
    let client = match reached {
        Reached::Connected(client) => client,
        Reached::NothingListens(cause) => {
            record.hold = RecordingHold::at_start(&RecordingHoldFound::NoRuntime, replaced);
            let mut answer = start_answer(record.hold.as_ref(), replaced);
            add_hold_warning(
                &mut answer,
                Some(LAB_RECORDING_WARNING_NO_RUNTIME_LISTENING),
            );
            answer["runtime_error"] = json!(cause);
            write_header(path, record)?;
            return Ok(answer);
        }
        Reached::NotHandshaken(error) => return Err(started_nothing(&instance, &reason, error)),
    };
    let shown = scheduling::instance_pause_reason(&client, &instance).map_err(|error| {
        started_nothing(
            &instance,
            &reason,
            crate::runtime_slice_cli::map_runtime_error(error),
        )
    })?;
    if let Some(shown) = shown {
        record.hold = RecordingHold::at_start(
            &RecordingHoldFound::Shown(shown.as_str().to_owned()),
            replaced,
        );
        write_header(path, record)?;
        return Ok(start_answer(record.hold.as_ref(), replaced));
    }
    // Fix round 2: the header names the own hold before the pause is sent, so that a start killed
    // or timed out in the pause's drain leaves a header whose end releases the pause by its reason.
    record.hold = RecordingHold::at_start(&RecordingHoldFound::Paused(reason.clone()), replaced);
    let mut answer = start_answer(record.hold.as_ref(), replaced);
    write_header(path, record)?;
    match scheduling::pause_for_recording(&client, &instance, own) {
        Ok(paused) => {
            answer["paused"] = json!(paused);
            Ok(answer)
        }
        Err(error) if scheduling::pause_refused(&error) => {
            let refused = started_nothing(
                &instance,
                &reason,
                crate::runtime_slice_cli::map_runtime_error(error),
            );
            match put_back(path, before) {
                Ok(()) => Err(refused),
                Err(failure) => {
                    let mut unrestored = with_hold(refused, record.hold.as_ref());
                    unrestored.message = format!(
                        "{}; the session header {} could not be put back as it was ({failure}): \
                         it names a hold whose release finds no pause",
                        unrestored.message,
                        path.display()
                    );
                    Err(unrestored)
                }
            }
        }
        Err(error) => Err(pause_unsettled(
            &instance,
            record.hold.as_ref(),
            crate::runtime_slice_cli::map_runtime_error(error),
        )),
    }
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

/// `record start --force`'s `hold` member `answer` once the session header it replaced could not
/// be read (`error`): the warning `replaced_header_unread`, and `replaced_header` naming the
/// header's `path` (as the answer's own `path` names it) and why it could not be read. A pause the
/// earlier recording took, if any, is not released by this recording.
pub(super) fn replaced_header_unread(answer: &mut Value, path: &Path, error: &CliError) {
    add_hold_warning(answer, Some(LAB_RECORDING_WARNING_REPLACED_HEADER_UNREAD));
    answer["replaced_header"] = json!({
        "path": path.display().to_string(),
        "error": error.message
    });
}

/// The command that tries the release of a kept hold again (fix round 2): `record` with `words`
/// (`stop` for a stopped recording, `archive --record-id <id>` for an archived one), on the
/// instance `instance` and the session state directory `state_dir`. It can release only the
/// recording's own pause, found by its reason.
pub(super) fn release_again(instance: &str, state_dir: &Path, words: &[&str]) -> Vec<String> {
    let mut command = vec!["record".to_owned()];
    command.extend(words.iter().map(|word| (*word).to_owned()));
    command.extend([
        "--instance".to_owned(),
        instance.to_owned(),
        "--state-dir".to_owned(),
        state_dir.display().to_string(),
    ]);
    command
}

/// An own hold that stays at the end of a recording: why the release did not lift it (the
/// release's failure as the Lab answers a Runtime failure: `runtime_not_running` when no Runtime
/// answered) and its reason.
pub(super) struct Kept {
    error: CliError,
    reason: String,
}

impl Kept {
    /// The answer of an end whose own hold stays: the release's failure, its message saying that
    /// `ended` (the end's own work) is done and the hold kept, its details the end's answer `done`
    /// with `hold` (`{reason, own: true, kept: true}`) and `next.release`, `release` (fix round 2:
    /// the command that tries the release again, [`release_again`]; never an unconditional
    /// `resume`, which could lift a pause that is not the recording's).
    pub(super) fn answer(
        self,
        instance: &str,
        ended: &str,
        done: Value,
        release: &[String],
    ) -> CliError {
        let Self { mut error, reason } = self;
        error.message = format!(
            "{ended}; its hold {reason} on {instance} is kept, since the release failed: {}; \
             `{}` tries the release again and resumes only a pause with that reason",
            error.message,
            release.join(" ")
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
        details.insert("next".to_owned(), json!({"release": release}));
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
    let kept = |error: CliError| Kept {
        error,
        reason: held.reason.clone(),
    };
    let own = pause_reason(&held.reason).map_err(kept)?;
    let client = connect().map_err(|cause| kept(CliError::runtime_not_running(cause)))?;
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
        Err(error) => Err(kept(crate::runtime_slice_cli::map_runtime_error(error))),
    }
}

/// #463 S1-7: the warning word a `--record` press or application operation on `instance` adds to
/// its `record.warnings`, from one status read on `client` before it: none when the instance's
/// scheduling shows paused (its own pause or the global pause), `instance_not_held` when it does
/// not, `instance_hold_unread` when the read got no answer.
fn hold_warning(client: &RuntimeClient, instance: &str) -> Option<&'static str> {
    match scheduling::instance_scheduling_paused(client, instance) {
        Ok(true) => None,
        Ok(false) => Some(LAB_RECORDING_WARNING_INSTANCE_NOT_HELD),
        Err(_) => Some(LAB_RECORDING_WARNING_HOLD_UNREAD),
    }
}

/// #463 S1-7: [`hold_warning`] read on a connection of its own, `instance_hold_unread` when none
/// can be made. Every `--record` press (fix round 2), `do --capture --record` (fix round 2) and
/// `session app --record` read it so, before the press or the operation opens its own connection:
/// a read that fails never leaves that connection dead.
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

/// A failure's details' `record`, when it has one that is an object, with `warning` added as
/// [`add_hold_warning`] adds it.
pub(crate) fn add_hold_warning_to_failure(error: &mut CliError, warning: Option<&str>) {
    if let Some(record) = error
        .details
        .as_mut()
        .and_then(|details| details.get_mut("record"))
    {
        add_hold_warning(record, warning);
    }
}
