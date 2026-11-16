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
//!
//! Fix round 2: an item carries what its refusal's details say ([`ReadyDetails`]), and the
//! warnings are typed ([`ReadyWarningItem`]), with the JSON they had.

use super::generate::{Lack, effective_steps};
use super::model::{LabRecording, OpaqueJson, RecordRect, RecordingStep};
use super::steps::{POINT_SOURCE, SWIPE_SOURCE};
use super::store::invalid;
use crate::codes::{ReadyCheck, ReadyMissing, ReadyWarning};
use actingcommand_contract::{LabError, LabResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// #463 S2-1 fix round 2: what the refusal of a failed pre-check says of its item beside the
/// item's own members (its `step` and `artifact_step` are the item's; its `select_problem`,
/// `ending_problem`, `transition` and `outcome` say again the item's word and code), so that two
/// items of one word are told apart: the marks it names, an ending's outcome key and page name,
/// the step after a stop, the pages after the last launch or restart, the parts of the package's
/// identity that are not known, and the counts of a recording with too few or too many steps.
/// Read from the refusal's `details`; for `ending_mark_missing`, whose refusal names the mark
/// alone, `outcome_key` is the ending's. Empty for a self-check's refusal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadyDetails {
    /// The marks the refusal names (`reused_mark_missing`, `forbidden_mark_missing`,
    /// `anchor_not_own_template`, `ending_mark_missing`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<ReadyMark>,
    /// The ending's outcome key (the ending words).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome_key: Option<String>,
    /// The ending's page name (`ending_page_taken`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    /// The serial number of the step after a stop (`effect_after_stop`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_step: Option<u32>,
    /// The pages after the last launch or restart (`application_without_home`), none among them
    /// the main interface.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub later_pages: Option<Vec<String>>,
    /// The parts of the package's identity that are not known (`game`, `server`, `locale`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// How many effective steps the recording has (`too_few_steps`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_steps: Option<usize>,
    /// How many steps have an effect, and how many a package holds (`too_many_effects`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_steps: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<usize>,
}

impl ReadyDetails {
    /// Whether the refusal says nothing beside the item's own members.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// #463 S2-1 fix round 2: a mark a refusal names: its id, why it is refused (`reuse_source_missing`,
/// `forbidden_source_missing`, `anchor_invalid`) and, for an anchor, what is wrong with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyMark {
    pub id: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

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
    /// The frame the step's press happened on, as stage one recorded it (`before_frame`; fix round
    /// 2: on a step whose press `--reopen-step` moved aside, its click's last attempt's); `None`
    /// when the step has none.
    pub frame_id: Option<String>,
    /// A word of `ready_missing`, or for a self-check's refusal a word of `ready_check`.
    pub what: String,
    /// The code `record stop` answers for it.
    pub code: String,
    /// The one command that settles it, where there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Fix round 2: what its refusal says beside these members; absent when nothing.
    #[serde(skip_serializing_if = "ReadyDetails::is_empty")]
    pub details: ReadyDetails,
}

/// #463 S2-1 fix round 2: `press_point_only` for one step (`{step, artifact_step, frame_id,
/// what}`, as before).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PressPointOnly {
    /// The step's serial number, as `record mark --step` takes it.
    pub step: u32,
    /// The step's number in the package.
    pub artifact_step: u32,
    /// The frame the step's press happened on ([`ReadyItem::frame_id`]).
    pub frame_id: Option<String>,
    /// `press_point_only`.
    pub what: String,
}

/// #463 S2-1 fix round 2: one warning of `record ready` (it never blocks), with the JSON it had: a
/// step's `press_point_only`, or a warning of the generator or of a self-check as `record stop`
/// prints it (its word in `code`, its other members its own; a dozen shapes, carried as they are).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ReadyWarningItem {
    PressPointOnly(PressPointOnly),
    Generator(OpaqueJson),
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
    pub warnings: Vec<ReadyWarningItem>,
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
/// declares a retry; #463 stage two, train 1: or a press box, a press recorded by its point whose
/// rectangle is no longer that one pixel, the test [`point_press_warnings`] makes), a choice, an
/// optional flag or a transition; or the recording took an ending.
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
                        || (click.source == POINT_SOURCE && !one_pixel(click.rect))
                        || click.follow.is_some()
                        || click.retry.is_some()
                })
        })
}

/// The frame the press of `step` happened on, as stage one recorded it: its click's or its
/// application operation's `before_frame`. Fix round 2: a click without an execution, whose press
/// `--reopen-step` moved aside, names its last attempt's (an application operation moved aside
/// keeps no frame).
fn press_frame(step: &RecordingStep) -> Option<String> {
    step.click
        .as_ref()
        .and_then(|click| click.execution.as_ref().or_else(|| click.attempts.last()))
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

/// Fix round 2: the [`ReadyDetails`] of the failed pre-check `lack`, read from its refusal's
/// details, with the ending it is about where the refusal does not name it. Details that do not
/// read so are the generator's own fault, `validation_failed`.
fn lack_details(lack: &Lack) -> LabResult<ReadyDetails> {
    let mut details = match &lack.error.details {
        Some(refused) => {
            serde_json::from_value::<ReadyDetails>(refused.clone()).map_err(|problem| {
                invalid(
                    "validation_failed",
                    format!(
                        "the details of the refusal {} do not read as an item's: {problem}",
                        lack.error.code
                    ),
                )
            })?
        }
        None => ReadyDetails::default(),
    };
    if details.outcome_key.is_none() {
        details.outcome_key.clone_from(&lack.outcome_key);
    }
    Ok(details)
}

/// The items of the failed pre-checks `lacks` (in the order the generator met them): the
/// recording's own first, then by the step's serial number, each step's in the generator's order.
pub(crate) fn lack_items(recording: &LabRecording, lacks: &[Lack]) -> LabResult<Vec<ReadyItem>> {
    let mut ordered = lacks.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|lack| lack.step.map(|(serial, _)| serial));
    ordered
        .into_iter()
        .enumerate()
        .map(|(position, lack)| {
            let step = lack.step.map(|(serial, _)| serial);
            Ok(ReadyItem {
                order: position + 1,
                step,
                artifact_step: lack.step.map(|(_, number)| number),
                frame_id: step.and_then(|serial| press_frame_of(recording, serial)),
                what: lack.what.as_str().to_string(),
                code: lack.error.code.clone(),
                hint: hint(lack.what, step),
                details: lack_details(lack)?,
            })
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
        details: ReadyDetails::default(),
    }
}

/// Workflow #392 DA-5cd: `missing` with, first when `size` is the refusal of a recording whose
/// frames are not the base size, its item (an item of the whole recording; fix round 1: `what`
/// `size_not_base`, `code` the one `record stop` answers, `record_size_not_base`), the others
/// numbered after it.
pub(crate) fn with_size_item(size: Option<&LabError>, missing: Vec<ReadyItem>) -> Vec<ReadyItem> {
    let Some(refusal) = size else {
        return missing;
    };
    let item = ReadyItem {
        order: 1,
        step: None,
        artifact_step: None,
        frame_id: None,
        what: ReadyMissing::SizeNotBase.as_str().to_string(),
        code: refusal.code.clone(),
        hint: None,
        details: ReadyDetails::default(),
    };
    std::iter::once(item)
        .chain(missing)
        .enumerate()
        .map(|(position, mut item)| {
            item.order = position + 1;
            item
        })
        .collect()
}

/// #463 stage two, train 1: whether `rect` is one pixel, the rectangle of a press recorded by its
/// point until a press box (#463 S2-5) gives it a region.
const fn one_pixel(rect: RecordRect) -> bool {
    rect.width == 1 && rect.height == 1
}

/// `press_point_only` for each effective step, in the order a run meets them, whose press is a
/// single recorded point (a tap or a long press `tap`, `long-tap --record` declared by its point,
/// a box of one pixel) with no press region set and no mark it follows: the package would press
/// that exact pixel.
pub(crate) fn point_press_warnings(recording: &LabRecording) -> Vec<ReadyWarningItem> {
    effective_steps(recording)
        .into_iter()
        .enumerate()
        .filter(|(_, step)| {
            step.click.as_ref().is_some_and(|click| {
                click.source == POINT_SOURCE && one_pixel(click.rect) && click.follow.is_none()
            })
        })
        .map(|(position, step)| {
            ReadyWarningItem::PressPointOnly(PressPointOnly {
                step: step.index,
                artifact_step: u32::try_from(position + 1).unwrap_or(u32::MAX),
                frame_id: press_frame(step),
                what: ReadyWarning::PressPointOnly.as_str().to_string(),
            })
        })
        .collect()
}

/// Fix round 2: the generator's and the self-checks' `warnings`, each as `record stop` prints it.
pub(crate) fn generator_warnings(warnings: &[Value]) -> LabResult<Vec<ReadyWarningItem>> {
    warnings
        .iter()
        .map(|warning| OpaqueJson::from_serializable(warning).map(ReadyWarningItem::Generator))
        .collect()
}

/// The answer for `recording` and its session's state `session`. A recording that is not
/// [`packaged`] is `material` while stage two wrote nothing on it, otherwise `ready` when nothing
/// is `missing` and `draft` when something is.
pub(crate) fn ready_outcome(
    recording: &LabRecording,
    session: String,
    missing: Vec<ReadyItem>,
    warnings: Vec<ReadyWarningItem>,
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
