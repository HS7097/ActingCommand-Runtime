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
//!
//! Fix round 1 (the independent review of #1786): an application operation's outcome is the
//! judgement the archive's manifest gives it (`archive::outcome_of`), `indeterminate` with attempts
//! and no completed receipt; a frame whose file is not there is answered with no path and the
//! warning `record frames` gives it (`around::located`, `frame_file_missing`); the answer's paths
//! are the archive module's (`archive::display`).
//!
//! #463 stage two, train (S2-2, S2-7, S2-6, S2-1c): a step's `missing` is its items of the one
//! readiness judgement `record ready` answers with (`recording_api::ready_of`): a pre-check's that
//! names it (S2-6's `select_unrefined` among them), or, once none fails, every self-check failure
//! that names it (S2-1c); an item that names no step is no step's. A step marked as holding a choice
//! (S2-6) shows its mark (`choice`); a step S2-7 dropped is no effective step and is not listed, and
//! one `restore_step` gave back is listed again.

use super::archive::{PERFORMED, display, outcome_of, step_kind, stored_frame_path};
use super::around::{FRAME_FILE_MISSING, RecordFramesWarning, Stored, located, sequence_of};
use super::boxes::box_mark_id;
use super::generate::effective_steps;
use super::model::FoldDirection;
use super::model::{
    AfterCaptureRecord, ClickGesture, LabRecording, RecordPoint, RecordRect, RecordedFrame,
    RecordedMark, RecordingStep, StepChoice, StepClick,
};
use super::preview::{
    Colour, Glyph, MARK_COLOUR, Overlay, PRESS_BOX_COLOUR, RECOGNIZE_BOX_COLOUR, render,
};
use super::ready::{ReadyItem, ReadyWarningItem, RecordReadyOutcome, one_pixel, press_frame};
use super::scan::RecordReviewScan;
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
/// Fix round 1: a press's outcome in the words `record status` names a click's
/// (`recording_api::click_outcome`): performed, and with no outcome on record.
const EXECUTED: &str = "executed";
const DECLARED: &str = "declared";

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
    /// Fix round 1: `frame_file_missing` naming each answered frame whose file is not there (its
    /// `path` `null`), once, in the order the answer names them, as `record frames` warns; empty
    /// when every file is there.
    pub warnings: Vec<RecordFramesWarning>,
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
    /// #463 stage two, train: the step's choice mark (#463 S2-6) as the step table holds it, on a
    /// step marked as holding a choice, refined or not; absent on every other step. A refined
    /// choice's press is `press`: the select's click as the step table holds it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice: Option<StepChoice>,
    /// #463 S2-8' (`contracts/lab-recording.md`, "Folding swipes into a scrolling search"): on a
    /// press step that holds a fold, the swipes folded into its scrolling search, in order, each
    /// with its screens: the list's first screen is the first swipe's screen before, then the
    /// screen after each swipe, forward or back; absent on every other step.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub folded: Vec<RecordReviewFolded>,
    /// #463 S2-9a (`contracts/lab-recording.md`, "A scrolling choice: reading the list to its
    /// end"): on a step whose select scrolls, its sheet: the list's screens in order with the
    /// regions taken from each and the names read on each, every number with its origin and the
    /// offline entry's decision, as the select's self-test left them; absent on every other step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan: Option<RecordReviewScan>,
    /// The step's items of `record ready`, as `record ready` answers them: the pre-checks' items
    /// that name this step (#463 S2-6's `select_unrefined` among them) or, once none fails, every
    /// self-check's failure that names it (#463 S2-1c); an item that names no step is no step's.
    pub missing: Vec<ReadyItem>,
    /// #463 RM-1: the warnings `record_template_near_threshold` of `record ready` whose judgement
    /// names this step by its serial number (`record_index`: its page on its own frames, the
    /// arrival gate after its effect, its optional page, or the skip target it is), as `record
    /// ready` answers them; absent when there is none, so that a step without one is answered as
    /// before. A warning of an ending names no step and is no step's. #463 SC-1 fix round 1: and
    /// the warnings `record_select_page_passes_after_press` of `record ready` whose select step is
    /// this step (`record_index`), each as `record ready` answers it, in `record ready`'s order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ReadyWarningItem>,
}

/// #463 S2-8': one swipe folded into a press step's scrolling search, as that step's sheet lists
/// it: its step, its direction against the first swipe's, and its screen before, its swipe and its
/// screen after, each as a step's own are answered (previews named by the swipe's own step).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordReviewFolded {
    pub step: u32,
    pub direction: FoldDirection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<RecordReviewFrame>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub press: Option<RecordReviewPress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<RecordReviewAfter>,
}

/// A stored frame and its preview: the preview's path, or `null` when none is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordReviewFrame {
    pub frame_id: String,
    /// The frame's file, an absolute path, as the archive's manifest names it; fix round 1: `null`
    /// when no file is there (`frame_file_missing`).
    pub path: Option<String>,
    pub preview: Option<String>,
}

/// The screen after a press: the step it opened, its frame and preview, and how it was taken where
/// the step table holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordReviewAfter {
    /// The serial number of the step the frame is stored under, the next step.
    pub step: u32,
    pub frame_id: String,
    /// As [`RecordReviewFrame::path`].
    pub path: Option<String>,
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
    /// As [`RecordReviewFrame::path`].
    pub path: Option<String>,
}

/// What a step pressed, as the step table holds it: a click's region (`rect`, one pixel for a
/// press recorded by its point), its gesture, and, once performed, the point pressed, a swipe's
/// end and the duration; or an application operation's action. `source` is the click's (`point`,
/// `swipe`, ...) or the operation's (`executed`, `declared`); `outcome` is `declared`, `executed`
/// or `indeterminate`, as `record status` names a click's (fix round 1: an application operation's
/// by the judgement of the archive's manifest, `application_outcome`).
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
    /// #463 KR-1: the key a key step presses (`back`); absent on every other step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// #463 KR-1: the press a key step's key replaced, as the step table holds it inside the key;
    /// absent on every other step and on a key that replaced none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaced: Option<Box<RecordReviewPress>>,
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
/// `lab/preview/` once every frame they are drawn on read and matched its sha256. Fix round 1: a
/// frame whose file is not there is answered without a path and named in `frame_file_missing`.
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
    let mut missing = Vec::new();
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
                display(&path)
            });
            RecordReviewFrame {
                frame_id: frame.frame_id.clone(),
                path: located(&dir, frame, &mut missing),
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
                    path: located(&dir, stored.frame, &mut missing),
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
                display(&path)
            });
            RecordReviewAfter {
                step: next,
                frame_id: frame.frame_id.clone(),
                path: located(&dir, frame, &mut missing),
                preview,
                still: record.and_then(|record| record.still),
                waited_ms: record.map(|record| record.waited_ms),
            }
        });
        let folded = folded_sheet(
            recording,
            step,
            &sequence,
            &dir,
            request.no_preview,
            &mut planned,
            &mut missing,
        );
        steps.push(RecordReviewStep {
            step: step.index,
            artifact_step: u32::try_from(position + 1).unwrap_or(u32::MAX),
            kind: step_kind(step).to_owned(),
            before,
            press: press_of(step),
            after,
            between,
            marks: step.marks.clone(),
            choice: step.choice.clone(),
            folded,
            scan: super::scan::review_sheet(step, &recording.task_id),
            missing: ready
                .missing
                .iter()
                .filter(|item| item.step == Some(step.index))
                .cloned()
                .collect(),
            warnings: ready
                .warnings
                .iter()
                .filter(|warning| match warning {
                    ReadyWarningItem::NearThreshold(near) => {
                        near.at.record_index == Some(step.index)
                    }
                    ReadyWarningItem::SelectPageAfterPress(found) => {
                        found.record_index == step.index
                    }
                    _ => false,
                })
                .cloned()
                .collect(),
        });
    }
    write_previews(&dir, &planned)?;
    // A frame answered twice (a step's screen after is the next step's screen before) is named
    // once.
    let mut named = BTreeSet::new();
    missing.retain(|frame_id| named.insert(frame_id.clone()));
    let warnings = if missing.is_empty() {
        Vec::new()
    } else {
        vec![RecordFramesWarning {
            code: FRAME_FILE_MISSING,
            frame_ids: missing,
        }]
    };
    Ok(RecordReviewOutcome {
        record_id: recording.record_id.clone(),
        session: ready.session,
        steps,
        steps_total: effective.len(),
        warnings,
    })
}

/// #463 S2-8' (`contracts/lab-recording.md`, "Folding swipes into a scrolling search"): the swipes
/// folded into the scrolling search of `step`, in the fold's order (none on a step that holds no
/// fold), each answered as a step's screen before, press and screen after are, from the same
/// sequence, its previews planned as a step's are and named by the swipe's own serial number.
fn folded_sheet<'r>(
    recording: &'r LabRecording,
    step: &RecordingStep,
    sequence: &[Stored<'r>],
    dir: &Path,
    no_preview: bool,
    planned: &mut Vec<Planned<'r>>,
    missing: &mut Vec<String>,
) -> Vec<RecordReviewFolded> {
    let Some(fold) = &step.scan_fold else {
        return Vec::new();
    };
    let mut folded = Vec::with_capacity(fold.moves.len());
    for moved in &fold.moves {
        let Some(swipe) = step_ref(recording, moved.step) else {
            continue;
        };
        let before = before_position(swipe, sequence).map(|at| {
            let frame = sequence[at].frame;
            let preview = (!no_preview).then(|| {
                let path = preview_path(dir, swipe.index, BEFORE);
                planned.push(Planned {
                    frame,
                    overlay: Overlay {
                        boxes: declared_boxes(swipe),
                        glyph: glyph(swipe),
                    },
                    path: path.clone(),
                });
                display(&path)
            });
            RecordReviewFrame {
                frame_id: frame.frame_id.clone(),
                path: located(dir, frame, missing),
                preview,
            }
        });
        let after = after_record(swipe)
            .and_then(|(frame_id, record)| {
                sequence
                    .iter()
                    .position(|stored| stored.frame.frame_id == frame_id)
                    .map(|at| (at, record))
            })
            .map(|(at, record)| {
                let (frame, next) = (sequence[at].frame, sequence[at].step);
                let preview = (!no_preview).then(|| {
                    let path = preview_path(dir, swipe.index, AFTER);
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
                    display(&path)
                });
                RecordReviewAfter {
                    step: next,
                    frame_id: frame.frame_id.clone(),
                    path: located(dir, frame, missing),
                    preview,
                    still: record.and_then(|record| record.still),
                    waited_ms: record.map(|record| record.waited_ms),
                }
            });
        folded.push(RecordReviewFolded {
            step: swipe.index,
            direction: moved.direction,
            before,
            press: press_of(swipe),
            after,
        });
    }
    folded
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
/// application operation's completed receipt names them. #463 KR-1: a key step's, as the execution
/// of the press its key replaced names them: the step's frames stand.
fn after_record(step: &RecordingStep) -> Option<(&str, Option<&AfterCaptureRecord>)> {
    match (step.recorded_click(), &step.application) {
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

/// What `step` pressed, as the step table holds it; `None` for a step without an effect. #463
/// KR-1: a key step's press is its key, declared and never sent while recording, with the press it
/// replaced beside it.
fn press_of(step: &RecordingStep) -> Option<RecordReviewPress> {
    if let Some(click) = &step.click {
        return Some(click_press(click));
    }
    if let Some(key) = &step.key {
        return Some(RecordReviewPress {
            point: None,
            to_point: None,
            duration_ms: None,
            rect: None,
            gesture: None,
            application: None,
            key: Some(key.key.clone()),
            replaced: key
                .replaced
                .as_ref()
                .map(|click| Box::new(click_press(click))),
            source: DECLARED.to_owned(),
            outcome: DECLARED.to_owned(),
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
            key: None,
            replaced: None,
            source: application.source.clone(),
            outcome: application_outcome(step),
        })
}

/// What the click `click` pressed, as the step table holds it.
fn click_press(click: &StepClick) -> RecordReviewPress {
    let execution = click.execution.as_ref();
    RecordReviewPress {
        point: execution.map(|execution| execution.point),
        to_point: execution.and_then(|execution| execution.to_point),
        duration_ms: execution.and_then(|execution| execution.duration_ms),
        rect: Some(click.rect),
        gesture: click.gesture,
        application: None,
        key: None,
        replaced: None,
        source: click.source.clone(),
        outcome: click_outcome(click).to_owned(),
    }
}

/// Fix round 1: the outcome of the application operation of `step`, by the judgement the archive's
/// manifest gives it (`archive::outcome_of`), in the words `record status` names a click's: its
/// `performed` (a completed receipt is on record) is `executed`, its `indeterminate` (attempts and
/// no completed receipt: the operation may have run) stays, and without either it is `declared`.
fn application_outcome(step: &RecordingStep) -> String {
    match outcome_of(step) {
        Some(outcome) if outcome == PERFORMED => EXECUTED.to_owned(),
        Some(outcome) => outcome,
        None => DECLARED.to_owned(),
    }
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
