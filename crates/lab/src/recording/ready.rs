// SPDX-License-Identifier: AGPL-3.0-only

//! #463 S2-1 (`contracts/lab-recording.md`, "What a recording still lacks: `record ready`"): the
//! one read that lists everything a recording still lacks before `record stop` can make its
//! package. Stage one leaves material; stage two writes the business's meaning on it offline;
//! `record stop` refuses at the first pre-check the recording fails, so this answer lists every
//! one at once (`generate::survey`, the generator's own checks, collected), then, when none is
//! failed, the self-checks `record stop` runs (`recording_api::self_checks`), and the recording's
//! state: `material`, `draft`, `ready` or `packaged`. Here the answer is put together from what the
//! generator found; the reading itself is `recording_api::record_ready`, offline, under no lock,
//! writing nothing.

use super::generate::{Lack, effective_steps};
use super::model::{LabRecording, RecordingStep};
use super::steps::{POINT_SOURCE, SWIPE_SOURCE};
use crate::codes::{ReadyCheck, ReadyMissing, ReadyWarning};
use actingcommand_contract::LabError;
use serde::Serialize;
use serde_json::{Value, json};

/// One thing a recording still lacks: a pre-check of `record stop` it fails, or the refusal of a
/// self-check (`what` the check's name).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReadyItem {
    /// 1..n, in the answer's order.
    pub order: usize,
    /// The step's serial number, as `record mark --step` takes it; `None` for an item of the whole
    /// recording.
    pub step: Option<u32>,
    /// The step's number in the package; `None` with `step`.
    pub artifact_step: Option<u32>,
    /// The frame the step's press happened on, as stage one recorded it (`before_frame`); `None`
    /// when the step has none.
    pub frame_id: Option<String>,
    /// A word of `ready_missing`, or for a self-check's refusal a word of `ready_check`.
    pub what: String,
    /// The code `record stop` answers for it.
    pub code: String,
    /// The one command that settles it, where there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// The answer of `record ready`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordReadyOutcome {
    /// `material`, `draft`, `ready` or `packaged`, by the recording alone.
    pub status: String,
    pub record_id: String,
    /// The session's state as `record status` says it: `active`, `archived` or `stopped`.
    pub session: String,
    /// Every pre-check the recording fails, the recording's own first, then by step, each step's
    /// in the order the generator checks them; or, when none, the refusal of a self-check.
    pub missing: Vec<ReadyItem>,
    pub missing_total: usize,
    /// `press_point_only` for each step whose press is a recorded point without a press region,
    /// then the generator's own warnings as `record stop` prints them.
    pub warnings: Vec<Value>,
    /// The self-checks that did not run, in the order they run; empty when every one ran.
    pub checks_after: Vec<String>,
}

/// The state of a recording, by the recording alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadyState {
    /// Nothing of stage two is written yet.
    Material,
    /// Something is written, and a pre-check fails or a self-check refuses.
    Draft,
    /// Every pre-check passes and every self-check: `record stop` would write the package.
    Ready,
    /// Stopped, its package written, and nothing changed since.
    Packaged,
}

impl ReadyState {
    const fn word(self) -> &'static str {
        match self {
            Self::Material => "material",
            Self::Draft => "draft",
            Self::Ready => "ready",
            Self::Packaged => "packaged",
        }
    }
}

/// Whether `recording` is stopped with its package written and nothing changed since (no mark
/// after the stop, `amended_at_unix_ms`).
pub(crate) fn packaged(recording: &LabRecording) -> bool {
    recording.status == "stopped"
        && recording.artifact.is_some()
        && recording.amended_at_unix_ms.is_none()
}

/// Whether stage two wrote anything on `recording`: a step carries a page name, a mark (own,
/// reused or forbidden, an anchor, a click guard), a press region (a click that is no press stage
/// one recorded: a tap or a long press by its point, a swipe; or one that follows a mark or
/// declares a retry), a choice, an optional flag or a transition; or the recording took an ending.
fn stage_two_written(recording: &LabRecording) -> bool {
    !recording.endings.is_empty()
        || recording.steps.iter().any(|step| {
            step.page.is_some()
                || !step.marks.is_empty()
                || !step.reused.is_empty()
                || !step.forbidden.is_empty()
                || step.anchor.is_some()
                || step.click_guard.is_some()
                || step.select.is_some()
                || step.optional.is_some()
                || step.transition.is_some()
                || step.click.as_ref().is_some_and(|click| {
                    (click.source != POINT_SOURCE && click.source != SWIPE_SOURCE)
                        || click.follow.is_some()
                        || click.retry.is_some()
                })
        })
}

/// The frame the press of `step` happened on, as stage one recorded it: its click's or its
/// application operation's `before_frame`.
fn press_frame(step: &RecordingStep) -> Option<String> {
    step.click
        .as_ref()
        .and_then(|click| click.execution.as_ref())
        .and_then(|execution| execution.before_frame.clone())
        .or_else(|| {
            step.application
                .as_ref()
                .and_then(|application| application.executed.as_ref())
                .and_then(|executed| executed.before_frame.clone())
        })
}

/// [`press_frame`] of the step with the serial number `serial`.
fn press_frame_of(recording: &LabRecording, serial: u32) -> Option<String> {
    recording
        .steps
        .iter()
        .find(|step| step.index == serial)
        .and_then(press_frame)
}

/// The one command that settles `what` on the step `step`, where there is one.
fn hint(what: ReadyMissing, step: Option<u32>) -> Option<String> {
    let step = step.map_or_else(|| "<k>".to_string(), |step| step.to_string());
    match what {
        ReadyMissing::Game => Some("record stop --game <g>".to_string()),
        ReadyMissing::Server => Some("record stop --server <s>".to_string()),
        ReadyMissing::Locale => Some("record stop --locale <l>".to_string()),
        ReadyMissing::PageRecognition => {
            Some(format!("record mark --step {step} --template <id>=x,y,w,h"))
        }
        ReadyMissing::OptionalFirstStep
        | ReadyMissing::OptionalFinalStep
        | ReadyMissing::OptionalApplication
        | ReadyMissing::OptionalRestartHome => {
            Some(format!("record mark --step {step} --not-optional"))
        }
        _ => None,
    }
}

/// The items of the failed pre-checks `lacks` (in the order the generator met them): the
/// recording's own first, then by the step's serial number, each step's in the generator's order.
pub(crate) fn lack_items(recording: &LabRecording, lacks: &[Lack]) -> Vec<ReadyItem> {
    let mut ordered = lacks.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|lack| lack.step.map(|(serial, _)| serial));
    ordered
        .into_iter()
        .enumerate()
        .map(|(position, lack)| {
            let step = lack.step.map(|(serial, _)| serial);
            ReadyItem {
                order: position + 1,
                step,
                artifact_step: lack.step.map(|(_, number)| number),
                frame_id: step.and_then(|serial| press_frame_of(recording, serial)),
                what: lack.what.as_str().to_string(),
                code: lack.error.code.clone(),
                hint: hint(lack.what, step),
            }
        })
        .collect()
}

/// The item of the self-check `check` that refused with `error`: the step its details name
/// (`record_index`, the serial number, and `step`, the package's number), when they name one.
pub(crate) fn check_item(
    recording: &LabRecording,
    check: ReadyCheck,
    error: &LabError,
) -> ReadyItem {
    let number = |name: &str| {
        error
            .details
            .as_ref()
            .and_then(|details| details.get(name))
            .and_then(Value::as_u64)
            .and_then(|number| u32::try_from(number).ok())
    };
    let step = number("record_index");
    ReadyItem {
        order: 1,
        step,
        artifact_step: step.and(number("step")),
        frame_id: step.and_then(|serial| press_frame_of(recording, serial)),
        what: check.as_str().to_string(),
        code: error.code.clone(),
        hint: None,
    }
}

/// `press_point_only` for each effective step, in the order a run meets them, whose press is a
/// single recorded point (a tap or a long press `tap`, `long-tap --record` declared by its point,
/// a box of one pixel) with no press region set and no mark it follows: the package would press
/// that exact pixel.
pub(crate) fn point_press_warnings(recording: &LabRecording) -> Vec<Value> {
    effective_steps(recording)
        .into_iter()
        .enumerate()
        .filter(|(_, step)| {
            step.click.as_ref().is_some_and(|click| {
                click.source == POINT_SOURCE
                    && click.rect.width == 1
                    && click.rect.height == 1
                    && click.follow.is_none()
            })
        })
        .map(|(position, step)| {
            json!({
                "step": step.index,
                "artifact_step": position + 1,
                "frame_id": press_frame(step),
                "what": ReadyWarning::PressPointOnly.as_str()
            })
        })
        .collect()
}

/// The answer for `recording` and its session's state `session`. A recording that is not
/// [`packaged`] is `material` while stage two wrote nothing on it, otherwise `ready` when nothing
/// is `missing` and `draft` when something is.
pub(crate) fn ready_outcome(
    recording: &LabRecording,
    session: String,
    missing: Vec<ReadyItem>,
    warnings: Vec<Value>,
    checks_after: &[ReadyCheck],
) -> RecordReadyOutcome {
    let state = if packaged(recording) {
        ReadyState::Packaged
    } else if !stage_two_written(recording) {
        ReadyState::Material
    } else if missing.is_empty() {
        ReadyState::Ready
    } else {
        ReadyState::Draft
    };
    RecordReadyOutcome {
        status: state.word().to_string(),
        record_id: recording.record_id.clone(),
        session,
        missing_total: missing.len(),
        missing,
        warnings,
        checks_after: checks_after
            .iter()
            .map(|check| check.as_str().to_string())
            .collect(),
    }
}
