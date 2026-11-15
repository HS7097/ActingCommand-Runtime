// SPDX-License-Identifier: AGPL-3.0-only

//! #463 S1-7 (`contracts/lab-recording.md`, "A recording holds its instance"): what a recording
//! holds, as its session header keeps it (`record-<instance>.json`, member `hold`), and what
//! `record status` shows of it. The Lab's `record` command takes the hold through the Runtime at
//! `record start` (the instance's scheduling paused for the recording) and releases it at the end
//! of the recording; it hands the facts it met in here, and writes the header. Nothing here asks
//! the Runtime anything.
//!
//! - **The reason** of the recording's own pause is `lab-record-<ms>`, `<ms>` the recording's start
//!   (its session header's `started_at_unix_ms`), as the Lab's rescue module names its holds
//!   `lab-<word>-<ms>` ([`RecordingHold::reason_for`]).
//! - **What a new recording holds** ([`RecordingHold::at_start`]): the pause it sets (the command
//!   writes it into the header before it sends the pause, fix round 2); or the pause the instance
//!   already showed, its own when it is the pause of the recording whose header it replaces (taken
//!   over, so neither doubled nor lost), another party's otherwise; or, when nothing listens where
//!   the installation's Runtime would, the replaced recording's own pause, which the Runtime
//!   restores at its start, else nothing.
//! - **At the end** ([`RecordingHold::ended`]): an own pause stays the header's, marked kept, until
//!   the release lifts it (the command then writes the header without it); another party's is no
//!   longer the recording's.
//! - **What `record status` shows** ([`RecordingHoldView`]): `own`, `foreign` with its reason,
//!   `none`, or `kept`: an own pause that the end of the recording could not release.
//!
//! A header without the member is one without a hold: every header written before this slice,
//! and every recording started while nothing listened, byte for byte as before.

use super::store::read_old_record;
use actingcommand_contract::LabResult;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The pause reason of a recording's own hold, before the recording's start in milliseconds.
pub const LAB_RECORDING_HOLD_PREFIX: &str = "lab-record-";

/// `record start`'s warning word in `hold.warnings` (fix round 2): no connection to the
/// installation's Runtime could be opened at all (no state root located, no runtime-info that
/// names an address, nothing listening there), so no scheduler can dispatch and the recording
/// holds nothing of its own. A Runtime that listens but does not complete the handshake fails the
/// start instead.
pub const LAB_RECORDING_WARNING_NO_RUNTIME_LISTENING: &str = "no_runtime_listening";
/// `record start --force`'s warning word in `hold.warnings` (fix round 2): the session header it
/// replaces could not be read, so a pause the earlier recording took, if any, is not released by
/// this recording (it is another party's to it).
pub const LAB_RECORDING_WARNING_REPLACED_HEADER_UNREAD: &str = "replaced_header_unread";
/// The warning word of a `--record` press or application operation in `record.warnings`: the
/// status read before it showed the instance's scheduling not paused, so the material may be mixed
/// with a scheduled task's screens.
pub const LAB_RECORDING_WARNING_INSTANCE_NOT_HELD: &str = "instance_not_held";
/// The warning word of a `--record` press or application operation in `record.warnings`: the
/// status read before it got no answer, so whether the instance is held is not known.
pub const LAB_RECORDING_WARNING_HOLD_UNREAD: &str = "instance_hold_unread";

/// What a recording holds, as its session header keeps it (the member `hold`): the instance's
/// scheduling pause it runs under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingHold {
    /// The reason the instance's pause shows.
    pub reason: String,
    /// The recording's own pause (`record start` set it, or took over the one of the recording
    /// whose header it replaced); `false`: another party's pause, which the recording never
    /// resumes.
    pub own: bool,
    /// An own pause the end of the recording could not release, or has not released yet: it stays
    /// until a resume lifts it. Absent while the recording runs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub kept: bool,
}

/// What `record start` met on the instance, handed in by the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingHoldFound {
    /// The command pauses the instance's scheduling with this reason (the header names it before
    /// the pause is sent).
    Paused(String),
    /// The instance's scheduling was already paused, with this reason; nothing was sent.
    Shown(String),
    /// Nothing listens where the installation's Runtime would: nothing can dispatch, and nothing
    /// was paused.
    NoRuntime,
}

impl RecordingHold {
    /// The reason of the own pause of the recording that starts at `started_at_unix_ms`.
    pub fn reason_for(started_at_unix_ms: u64) -> String {
        format!("{LAB_RECORDING_HOLD_PREFIX}{started_at_unix_ms}")
    }

    /// What a new recording holds, from what `record start` met (`found`) and the hold of the
    /// session header it replaces (`replaced`, `None` without one):
    /// - the pause it sets: its own;
    /// - a pause the instance already showed: its own when the replaced header held exactly that
    ///   pause as its own (taken over), another party's otherwise;
    /// - nothing listening: the replaced header's own pause, which the Runtime restores at its
    ///   start, taken over; else nothing.
    pub fn at_start(found: &RecordingHoldFound, replaced: Option<&Self>) -> Option<Self> {
        let replaced_own = replaced.filter(|replaced| replaced.own);
        match found {
            RecordingHoldFound::Paused(reason) => Some(Self::own(reason)),
            RecordingHoldFound::Shown(reason) => Some(
                if replaced_own.is_some_and(|replaced| replaced.reason == *reason) {
                    Self::own(reason)
                } else {
                    Self {
                        reason: reason.clone(),
                        own: false,
                        kept: false,
                    }
                },
            ),
            RecordingHoldFound::NoRuntime => {
                replaced_own.map(|replaced| Self::own(&replaced.reason))
            }
        }
    }

    /// Whether the new recording's own hold `self` is the pause of the recording whose header it
    /// replaced (`replaced`), taken over rather than set.
    pub fn taken_over(&self, replaced: Option<&Self>) -> bool {
        self.own && replaced.is_some_and(|replaced| replaced.own && replaced.reason == self.reason)
    }

    /// What the session header keeps once the recording ended (`record stop`, `record archive`),
    /// before the release is tried: an own pause, kept until the release lifts it; another party's,
    /// nothing.
    pub fn ended(&self) -> Option<Self> {
        self.own.then(|| Self {
            reason: self.reason.clone(),
            own: true,
            kept: true,
        })
    }

    fn own(reason: &str) -> Self {
        Self {
            reason: reason.to_owned(),
            own: true,
            kept: false,
        }
    }
}

/// The state of a recording's hold as `record status` shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RecordingHoldState {
    /// The recording runs under its own pause.
    #[serde(rename = "own")]
    Own,
    /// The recording runs under another party's pause, which it never resumes.
    #[serde(rename = "foreign")]
    Foreign,
    /// The recording holds nothing.
    #[serde(rename = "none")]
    Unheld,
    /// The recording ended and its own pause stays: its end could not release it.
    #[serde(rename = "kept")]
    Kept,
}

/// A recording's hold as `record status` shows it: its state and the reason of the pause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordingHoldView {
    pub state: RecordingHoldState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl RecordingHoldView {
    /// The view of the hold `hold` of a session header whose session is `active`: an own pause of
    /// a session that ended is kept (its end could not release it, or did not get as far).
    pub fn of(hold: Option<&RecordingHold>, active: bool) -> Self {
        let Some(hold) = hold else {
            return Self {
                state: RecordingHoldState::Unheld,
                reason: None,
            };
        };
        let state = match (hold.own, hold.kept || !active) {
            (false, _) => RecordingHoldState::Foreign,
            (true, true) => RecordingHoldState::Kept,
            (true, false) => RecordingHoldState::Own,
        };
        Self {
            state,
            reason: Some(hold.reason.clone()),
        }
    }
}

/// `record status`'s hold of `instance`: the hold of the session header in `state_dir` as the Lab
/// reads the header; `none` without a header.
pub fn record_hold_status(state_dir: &Path, instance: &str) -> LabResult<RecordingHoldView> {
    let header = read_old_record(state_dir, instance)?;
    Ok(match header {
        Some(header) => RecordingHoldView::of(header.hold.as_ref(), header.status == "active"),
        None => RecordingHoldView::of(None, false),
    })
}
