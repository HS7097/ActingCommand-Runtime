// SPDX-License-Identifier: AGPL-3.0-only

//! #463 S2-2 (`contracts/lab-recording.md`, "Reviewing a recording step by step: `record
//! review`"): each effective step of a recording as stage two of package making reads it: the
//! screen before, where the press went, the screen after, with the step's marks and its items of
//! `record ready`, and a preview of each screen with the press and the declared boxes drawn on a
//! copy (`super::preview`). The read takes no lock; nothing recorded is written: the only files
//! written are the previews, under the recording's `lab/preview/`.
//!
//! - **What it reads.** Only what the step table holds: the press's `before_frame` (else the
//!   step's live primary frame, the screen of a step without a press), the click's execution or the
//!   application operation's receipt (its point, a swipe's end, a long press's duration, its
//!   `after_frame` and `after_capture`), the frames stored between (the sequence `record frames`
//!   reads, `super::around`), the step's own marks. A member the step table does not hold is
//!   absent: the step table holds no note, no change measure of a frame and no list of the frames
//!   of the same screen, so the answer has none.
//! - **The previews.** Every stored frame a preview is drawn on is read, and its bytes checked
//!   against its sha256, before any preview is written: a frame that does not match is
//!   `record_frame_hash_mismatch` and no preview is written. Each is then read and checked again,
//!   drawn on a copy and written atomically (a temporary file, then a rename), so that two reviews
//!   at once leave no torn file.

use super::archive::{step_kind, stored_frame_path};
use super::around::{Stored, sequence_of};
use super::boxes::box_mark_id;
use super::generate::effective_steps;
use super::model::{
    AfterCaptureRecord, ClickGesture, LabRecording, RecordPoint, RecordRect, RecordedFrame,
    RecordedMark, RecordingStep,
};
use super::preview::{
    Colour, Glyph, MARK_COLOUR, Overlay, PRESS_BOX_COLOUR, RECOGNIZE_BOX_COLOUR, render,
};
use super::ready::{ReadyItem, RecordReadyOutcome, one_pixel, press_frame};
use super::steps::{effective_step, live_primary, step_ref};
use super::store::{lab_dir, read_verified, write_atomic};
use crate::recording_api::click_outcome;
use actingcommand_contract::LabResult;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The directory of the previews in a recording's `lab/` directory.
const PREVIEW_DIR: &str = "preview";
/// The roles of a step's two previews, in their file names.
const BEFORE: &str = "before";
const AFTER: &str = "after";

/// `record review`: the recording (`record_id`, or the one the instance's session header names),
/// the one step to answer (`step`, its serial number; `None`: every effective step) and whether no
/// preview is written (`no_preview`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordReviewRequest {
    pub record_id: Option<String>,
    pub step: Option<u32>,
    pub no_preview: bool,
}

/// The answer of `record review`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordReviewOutcome {
    pub record_id: String,
    /// The session's state as `record ready` says it: `active`, `archived` or `stopped`.
    pub session: String,
    /// The steps answered, in the order a run meets them: every effective step, or the one asked.
    pub steps: Vec<RecordReviewStep>,
    /// How many effective steps the recording has, answered or not: where the list may be cut, as
    /// `record frames` counts its sides, so that a long recording is read one step at a time.
    pub steps_total: usize,
}

/// One step: its screen before, its press and its screen after, as the step table holds them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordReviewStep {
    /// The step's serial number, as `record mark --step` takes it.
    pub step: u32,
    /// The step's number in the package (`step_<nn>`).
    pub artifact_step: u32,
    /// The step's kind, as the archive's manifest and `record frames` name it.
    pub kind: String,
    /// The screen the press happened on; absent on a step without a frame (an entry launch).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<RecordReviewFrame>,
    /// What was pressed; absent on a step without an effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub press: Option<RecordReviewPress>,
    /// The screen after the press, where the step table names it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<RecordReviewAfter>,
    /// The frames stored between the screen before and the screen after, in the order they
    /// entered the step table.
    pub between: Vec<RecordReviewBetween>,
    /// The step's own marks, as stored (a recognize box is a template mark `box/<nn>/...`).
    pub marks: Vec<RecordedMark>,
    /// The step's items of `record ready`, as `record ready` answers them.
    pub missing: Vec<ReadyItem>,
}

/// A stored frame and its preview: the preview's path, or `null` when none is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordReviewFrame {
    pub frame_id: String,
    /// The frame's file, an absolute path, as the archive's manifest names it.
    pub path: String,
    pub preview: Option<String>,
}

/// The screen after a press: the step it opened, its frame and preview, and how it was taken where
/// the step table holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordReviewAfter {
    /// The serial number of the step the frame is stored under, the next step.
    pub step: u32,
    pub frame_id: String,
    pub path: String,
    pub preview: Option<String>,
    /// Whether the captures after the press found the screen still (`after_capture.still`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub still: Option<bool>,
    /// From the press to the start of the capture that is the frame after
    /// (`after_capture.waited_ms`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waited_ms: Option<u64>,
}

/// A frame stored between a press's screen before and its screen after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordReviewBetween {
    pub frame_id: String,
    pub path: String,
}

/// What a step pressed, as the step table holds it: a click's region (`rect`, one pixel for a
/// press recorded by its point), its gesture, and, once performed, the point pressed, a swipe's
/// end and the duration; or an application operation's action. `source` is the click's (`point`,
/// `swipe`, ...) or the operation's (`executed`, `declared`); `outcome` is `declared`, `executed`
/// or `indeterminate`, as `record status` names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordReviewPress {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub point: Option<RecordPoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_point: Option<RecordPoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rect: Option<RecordRect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gesture: Option<ClickGesture>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    pub source: String,
    pub outcome: String,
}

/// One preview to write: the stored frame it is drawn on, what is drawn and where it goes.
struct Planned<'a> {
    frame: &'a RecordedFrame,
    overlay: Overlay,
    path: PathBuf,
}

/// The answer of `record review` for `recording` (read as `record ready` reads it) and `ready`,
/// the answer of `record ready` for it, its session's state among it: each effective step, or the
/// one `request.step` names (`record_step_not_found` for one that does not exist, is dropped or
/// was converted), and, unless `request.no_preview`, the previews written under the recording's
/// `lab/preview/` once every frame they are drawn on read and matched its sha256.
pub(crate) fn review_outcome(
    state_dir: &Path,
    recording: &LabRecording,
    ready: RecordReadyOutcome,
    request: &RecordReviewRequest,
) -> LabResult<RecordReviewOutcome> {
    let dir = lab_dir(state_dir, &recording.record_id);
    let effective = effective_steps(recording);
    let listed = match request.step {
        Some(serial) => {
            let asked = effective_step(recording, serial)?;
            effective
                .iter()
                .copied()
                .enumerate()
                .filter(|(_, step)| step.index == asked.index)
                .collect::<Vec<_>>()
        }
        None => effective.iter().copied().enumerate().collect(),
    };
    let sequence = sequence_of(recording);
    let mut planned = Vec::new();
    let mut steps = Vec::with_capacity(listed.len());
    for (position, step) in listed {
        let before_at = before_position(step, &sequence);
        let after = after_record(step).and_then(|(frame_id, record)| {
            sequence
                .iter()
                .position(|stored| stored.frame.frame_id == frame_id)
                .map(|at| (at, record))
        });
        let before = before_at.map(|at| {
            let frame = sequence[at].frame;
            let preview = (!request.no_preview).then(|| {
                let path = preview_path(&dir, step.index, BEFORE);
                planned.push(Planned {
                    frame,
                    overlay: Overlay {
                        boxes: declared_boxes(step),
                        glyph: glyph(step),
                    },
                    path: path.clone(),
                });
                absolute(&path)
            });
            RecordReviewFrame {
                frame_id: frame.frame_id.clone(),
                path: stored_frame_path(&dir, frame),
                preview,
            }
        });
        // The frames that entered the step table after the screen before and before the screen
        // after, stored under the step the screen after opened: the captures of the press's after
        // loop it did not name.
        let between = match (before_at, after) {
            (Some(first), Some((last, _))) => sequence
                .get(first + 1..last)
                .unwrap_or_default()
                .iter()
                .filter(|stored| stored.step == sequence[last].step)
                .map(|stored| RecordReviewBetween {
                    frame_id: stored.frame.frame_id.clone(),
                    path: stored_frame_path(&dir, stored.frame),
                })
                .collect(),
            _ => Vec::new(),
        };
        let after = after.map(|(at, record)| {
            let (frame, next) = (sequence[at].frame, sequence[at].step);
            let preview = (!request.no_preview).then(|| {
                let path = preview_path(&dir, step.index, AFTER);
                planned.push(Planned {
                    frame,
                    overlay: Overlay {
                        boxes: step_ref(recording, next)
                            .filter(|shown| shown.is_effective())
                            .map(declared_boxes)
                            .unwrap_or_default(),
                        glyph: None,
                    },
                    path: path.clone(),
                });
                absolute(&path)
            });
            RecordReviewAfter {
                step: next,
                frame_id: frame.frame_id.clone(),
                path: stored_frame_path(&dir, frame),
                preview,
                still: record.and_then(|record| record.still),
                waited_ms: record.map(|record| record.waited_ms),
            }
        });
        steps.push(RecordReviewStep {
            step: step.index,
            artifact_step: u32::try_from(position + 1).unwrap_or(u32::MAX),
            kind: step_kind(step).to_owned(),
            before,
            press: press_of(step),
            after,
            between,
            marks: step.marks.clone(),
            missing: ready
                .missing
                .iter()
                .filter(|item| item.step == Some(step.index))
                .cloned()
                .collect(),
        });
    }
    write_previews(&dir, &planned)?;
    Ok(RecordReviewOutcome {
        record_id: recording.record_id.clone(),
        session: ready.session,
        steps,
        steps_total: effective.len(),
    })
}

/// The place in `sequence` of the screen the press of `step` happened on: its `before_frame` as
/// `record ready` names it (`ready::press_frame`), else the step's live primary frame.
fn before_position(step: &RecordingStep, sequence: &[Stored<'_>]) -> Option<usize> {
    let frame_id =
        press_frame(step).or_else(|| live_primary(step).map(|frame| frame.frame_id.clone()))?;
    sequence
        .iter()
        .position(|stored| stored.frame.frame_id == frame_id)
}

/// The frame after the effect of `step` and how it was taken, as its click's execution or its
/// application operation's completed receipt names them.
fn after_record(step: &RecordingStep) -> Option<(&str, Option<&AfterCaptureRecord>)> {
    match (&step.click, &step.application) {
        (Some(click), _) => click.execution.as_ref().and_then(|execution| {
            execution
                .after_frame
                .as_deref()
                .map(|frame_id| (frame_id, execution.after_capture.as_ref()))
        }),
        (None, Some(application)) => application.executed.as_ref().and_then(|executed| {
            executed
                .after_frame
                .as_deref()
                .map(|frame_id| (frame_id, executed.after_capture.as_ref()))
        }),
        (None, None) => None,
    }
}

/// What `step` pressed, as the step table holds it; `None` for a step without an effect.
fn press_of(step: &RecordingStep) -> Option<RecordReviewPress> {
    if let Some(click) = &step.click {
        let execution = click.execution.as_ref();
        return Some(RecordReviewPress {
            point: execution.map(|execution| execution.point),
            to_point: execution.and_then(|execution| execution.to_point),
            duration_ms: execution.and_then(|execution| execution.duration_ms),
            rect: Some(click.rect),
            gesture: click.gesture,
            application: None,
            source: click.source.clone(),
            outcome: click_outcome(click).to_owned(),
        });
    }
    step.application
        .as_ref()
        .map(|application| RecordReviewPress {
            point: None,
            to_point: None,
            duration_ms: None,
            rect: None,
            gesture: None,
            application: Some(application.action.clone()),
            source: application.source.clone(),
            outcome: if application.executed.is_some() {
                "executed"
            } else {
                "declared"
            }
            .to_owned(),
        })
}

/// What is drawn of the press of `step` on its screen before: its performed click's point, by its
/// gesture (a tap, a swipe to its end, a long press); nothing for another gesture, a click not
/// performed or an application operation.
fn glyph(step: &RecordingStep) -> Option<Glyph> {
    let click = step.click.as_ref()?;
    let execution = click.execution.as_ref()?;
    let point = execution.point;
    match click.gesture {
        None => Some(Glyph::Tap(point)),
        Some(ClickGesture::Drag { .. }) => execution
            .to_point
            .map(|to| Glyph::Swipe { from: point, to }),
        Some(ClickGesture::LongPress { .. }) => Some(Glyph::LongPress(point)),
        Some(_) => None,
    }
}

/// The boxes `step` declares, in the order they are drawn: each own mark's region, a recognize
/// box's (the template mark a box drew, its id `box/<nn>/<x>_<y>_<w>_<h>` of this step and that
/// region) in its colour and every other in the marks' colour; then its press box, the region its
/// click presses when that is no recorded point (and a drag's end when that is no point either).
fn declared_boxes(step: &RecordingStep) -> Vec<(RecordRect, Colour)> {
    let mut boxes = step
        .marks
        .iter()
        .filter_map(|mark| {
            let region = mark.region?;
            let colour = if mark.id == box_mark_id(step.index, region) {
                RECOGNIZE_BOX_COLOUR
            } else {
                MARK_COLOUR
            };
            Some((region, colour))
        })
        .collect::<Vec<_>>();
    if let Some(click) = &step.click {
        if !one_pixel(click.rect) {
            boxes.push((click.rect, PRESS_BOX_COLOUR));
        }
        if let Some(ClickGesture::Drag { to, .. }) = click.gesture
            && !one_pixel(to)
        {
            boxes.push((to, PRESS_BOX_COLOUR));
        }
    }
    boxes
}

/// Where the preview of `role` of the step `serial` is written: `lab/preview/s<nn>-<role>.png`.
fn preview_path(dir: &Path, serial: u32, role: &str) -> PathBuf {
    dir.join(PREVIEW_DIR)
        .join(format!("s{serial:02}-{role}.png"))
}

/// The absolute form of `path` for the answer.
fn absolute(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

/// Writes `planned`: first every stored frame they are drawn on is read and its bytes checked
/// against its sha256, each once, so that a frame that does not match
/// (`record_frame_hash_mismatch`) leaves no preview written; then each preview is drawn on a copy
/// of its frame, read and checked again, and written atomically.
fn write_previews(dir: &Path, planned: &[Planned<'_>]) -> LabResult<()> {
    let mut checked = BTreeSet::new();
    for preview in planned {
        if checked.insert(preview.frame.sha256.as_str()) {
            read_verified(
                Path::new(&stored_frame_path(dir, preview.frame)),
                &preview.frame.sha256,
            )?;
        }
    }
    for preview in planned {
        let source = stored_frame_path(dir, preview.frame);
        let bytes = read_verified(Path::new(&source), &preview.frame.sha256)?;
        let png = render(bytes, &preview.overlay, &source)?;
        write_atomic(&preview.path, &png)?;
    }
    Ok(())
}
