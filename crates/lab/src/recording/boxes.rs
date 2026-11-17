// SPDX-License-Identifier: AGPL-3.0-only

//! #463 S2-5 (`contracts/lab-recording.md`, "Drawing a box"; the design of board #471): a
//! `record mark` request draws boxes on the step's primary frame, as a person draws one on a
//! screenshot, each a region the step's page is recognized by or the region its click presses.
//! Once the target step is known they stand for members a request has today: a recognize box is a
//! template mark with an id of its own, `box/<nn>/<x>_<y>_<w>_<h>`, cut from the primary frame and
//! searched in today's default area; a press box is the click's rectangle. Everything after that
//! is `record mark`'s own code, but for one case: a press box on a step whose press was recorded
//! by the command that made it (`source: "point"`) becomes that click's rectangle and leaves the
//! press as recorded (`steps.rs`, `apply_click`).
//!
//! The base images (the owner's answer on board #471: a box is drawn on one state of one screen,
//! and its base images are the frames of the action that drew it whose similarity reaches 0.95):
//! the frames of the action are the request's own samples and, on a draft, the captures stage one
//! stored for the step on the way to its screen ([`stage_one_captures`]). Each is scored with the
//! self-test's own function on that frame alone ([`call_frame`]). Fix round 2 (the coordinator's
//! ruling on the review's point d): a capture is a frame of the same state of the screen only when
//! its whole frame has not changed from the step's live frame, as stage one judges two captures
//! unchanged (`steps.rs`, `frame_unchanged`); one that changed is not added ([`FRAME_CHANGED`]).

use super::frames::{LoadedFrame, crop_png, rect_inside};
use super::marks::{EvalFrame, MarkRejection, evaluate_mark};
use super::model::{
    BoxBecause, BoxFrameTaken, BoxFrameView, BoxSpec, BoxUse, BoxView, ClickSpec, LabRecording,
    MarkFamily, MarkRequest, MarkSpec, RecordRect, RecordedFrame, RecordedMark, RecordingDefaults,
    SelfTestStatus,
};
use super::store::{invalid, with_details};
use crate::LabOcr;
use actingcommand_contract::{LabError, LabResult};
use actingcommand_recognition::TemplatePixelSums;
use serde_json::json;
use std::borrow::Cow;
use std::collections::BTreeMap;

/// #463 S2-5: the word of a frame of the action on which a recognize box of the request scores
/// below its threshold (`because.reason`): another state of the screen, not added.
pub(crate) const BELOW_THRESHOLD: &str = "below_threshold";

/// #463 S2-5 fix round 2: the word of a capture of the step whose whole frame changed from the
/// step's live frame by the measure stage one calls two captures unchanged by (`because.reason`,
/// with `frame_change_milli`): the frame shows another state of the screen, not added.
pub(crate) const FRAME_CHANGED: &str = "frame_changed";

/// #463 S2-5: the `source` of a sample a capture of the step became.
pub(crate) const RECORDING_FRAME_SOURCE: &str = "recording_frame";

/// The rectangle of a box: the smaller coordinate of its corners and their distance plus one, so
/// that both corner pixels are inside it (equal corners are a box of one pixel). A side too long
/// for a rectangle is the longest one, which no frame holds.
pub(crate) fn box_rect(spec: &BoxSpec) -> RecordRect {
    let [first, second] = spec.corners;
    let length = |a: i32, b: i32| i32::try_from(i64::from(a.abs_diff(b)) + 1).unwrap_or(i32::MAX);
    RecordRect {
        x: first.x.min(second.x),
        y: first.y.min(second.y),
        width: length(first.x, second.x),
        height: length(first.y, second.y),
    }
}

/// The id of a recognize box's template mark on step `step`: the step's serial number in two
/// digits (more above 99) and the box's rectangle, so the same box on the same step is always the
/// same mark.
pub(crate) fn box_mark_id(step: u32, rect: RecordRect) -> String {
    format!(
        "box/{step:02}/{}_{}_{}_{}",
        rect.x, rect.y, rect.width, rect.height
    )
}

/// The rectangle of the request's press box, if it has one.
pub(crate) fn press_rect(boxes: &[BoxSpec]) -> Option<RecordRect> {
    boxes
        .iter()
        .find(|spec| spec.usage == BoxUse::Press)
        .map(box_rect)
}

/// The template marks the request's recognize boxes stand for on step `step`, in their order.
fn recognize_specs(boxes: &[BoxSpec], step: u32) -> impl Iterator<Item = MarkSpec> + '_ {
    boxes
        .iter()
        .filter(|spec| spec.usage == BoxUse::Recognize)
        .map(move |spec| {
            let rect = box_rect(spec);
            MarkSpec::region_only(box_mark_id(step, rect), MarkFamily::Template, rect)
        })
}

fn boxes_invalid(problem: &str, message: String, detail: serde_json::Value) -> LabError {
    let mut details = json!({"argument": "boxes", "argument_problem": problem});
    if let (Some(object), Some(members)) = (details.as_object_mut(), detail.as_object()) {
        object.extend(members.clone());
    }
    with_details(invalid("validation_failed", message), details)
}

/// The rules of a request's boxes before any recording is read (`validation_failed`, exit 2,
/// `argument: "boxes"`): boxes are members of a step, so never beside a step operation or a
/// transition (`conflicting`, `member` naming it); a step has one click, so one press box
/// (`repeated`) beside no `click` (`conflicting`); a recognize box is one mark, so given once
/// (`repeated`). A box beside an ending or an inserted optional step is their own refusal
/// (`with_other_change`).
pub(crate) fn validate_boxes(request: &MarkRequest) -> LabResult<()> {
    if request.boxes.is_empty() {
        return Ok(());
    }
    for (member, given) in [
        ("step_action", request.step_action.is_some()),
        ("transition", request.transition.is_some()),
    ] {
        if given {
            return Err(boxes_invalid(
                "conflicting",
                format!(
                    "boxes mark a step and press its click; they cannot be combined with \
                     {member}: send the boxes in a request of their own"
                ),
                json!({"member": member}),
            ));
        }
    }
    let mut press = None;
    let mut recognized: Vec<(usize, RecordRect)> = Vec::new();
    for (index, spec) in request.boxes.iter().enumerate() {
        let rect = box_rect(spec);
        match spec.usage {
            BoxUse::Press => {
                if let Some(first) = press {
                    return Err(boxes_invalid(
                        "repeated",
                        format!(
                            "boxes {first} and {index} are both press boxes; a step has one click"
                        ),
                        json!({"box": index}),
                    ));
                }
                press = Some(index);
            }
            BoxUse::Recognize => {
                if let Some((first, _)) = recognized.iter().find(|(_, seen)| *seen == rect) {
                    return Err(boxes_invalid(
                        "repeated",
                        format!(
                            "boxes {first} and {index} are the same recognize box, one mark; give \
                             it once"
                        ),
                        json!({"box": index}),
                    ));
                }
                recognized.push((index, rect));
            }
        }
    }
    if let Some(index) = press
        && request.click.is_some()
    {
        return Err(boxes_invalid(
            "conflicting",
            format!(
                "box {index} is a press box, the step's click, and the request also gives click; \
                 give one of them"
            ),
            json!({"member": "click", "box": index}),
        ));
    }
    Ok(())
}

/// The request with its press box as the click's rectangle, for the rules of one effect a step has
/// (an application operation or a select beside a click); the request itself without one.
pub(crate) fn with_press_click(request: &MarkRequest) -> Cow<'_, MarkRequest> {
    match press_rect(&request.boxes) {
        Some(rect) if request.click.is_none() => {
            let mut pressed = request.clone();
            pressed.click = Some(ClickSpec {
                region: Some(rect),
                ..ClickSpec::default()
            });
            Cow::Owned(pressed)
        }
        _ => Cow::Borrowed(request),
    }
}

/// The request on step `step` with its boxes as the members they stand for: each recognize box a
/// template mark after the request's own `add`, the press box the click's rectangle. The boxes stay
/// in it, so that the code after the expansion knows a press box from a click. A request without
/// boxes is itself.
pub(crate) fn expanded_request(request: &MarkRequest, step: u32) -> Cow<'_, MarkRequest> {
    if request.boxes.is_empty() {
        return Cow::Borrowed(request);
    }
    let mut expanded = with_press_click(request).into_owned();
    expanded.add.extend(recognize_specs(&request.boxes, step));
    Cow::Owned(expanded)
}

/// The marks the request adds on step `step`, its recognize boxes' among them: what a resent
/// one-call step asks of the step it marked.
pub(crate) fn expanded_add(request: &MarkRequest, step: u32) -> Vec<MarkSpec> {
    request
        .add
        .iter()
        .cloned()
        .chain(recognize_specs(&request.boxes, step))
        .collect()
}

/// The recognize boxes whose crop on the primary frame holds one gray value: a template has no
/// shape to match there (the Runtime's `ccoeff_normed` cannot score it, `ccorr_normed` scores
/// every flat window alike), so each is refused before its mark is made (`box_flat`, with
/// `instead: "color"`, the family that marks a region by its colour). A box outside the frame is
/// left to its mark's `region_outside_frame`.
pub(crate) fn flat_boxes(
    primary: &LoadedFrame,
    boxes: &[BoxSpec],
    step: u32,
) -> LabResult<Vec<MarkRejection>> {
    let mut flat = Vec::new();
    for (index, spec) in boxes.iter().enumerate() {
        let rect = box_rect(spec);
        if spec.usage != BoxUse::Recognize || !rect_inside(rect, primary.size()) {
            continue;
        }
        let crop = crop_png(primary, rect)?;
        let sums = TemplatePixelSums::of_png(&crop, None).map_err(|error| {
            invalid(
                "record_frame_unreadable",
                format!("the crop of box {index} cannot be read: {error}"),
            )
        })?;
        if !sums.has_variance() {
            flat.push(MarkRejection::new(
                &box_mark_id(step, rect),
                "box_flat",
                json!({"box": index, "rect": rect, "instead": MarkFamily::Color.as_str()}),
            ));
        }
    }
    Ok(flat)
}

/// A recognize box's template mark as the request made it on the step, with its crop, by the
/// box's index in `boxes`.
pub(crate) struct BoxMark {
    pub(crate) index: usize,
    pub(crate) mark: RecordedMark,
    pub(crate) crop: Vec<u8>,
}

/// The template marks the request's recognize boxes became on step `step`: those of the batch's
/// own marks that this request made (`crops`, the batch's new crops), in the boxes' order. A box
/// refused before its mark was made (`box_flat`, `region_outside_frame`) has none.
pub(crate) fn made_box_marks(
    boxes: &[BoxSpec],
    step: u32,
    own: &[RecordedMark],
    crops: &BTreeMap<String, Vec<u8>>,
) -> Vec<BoxMark> {
    boxes
        .iter()
        .enumerate()
        .filter(|(_, spec)| spec.usage == BoxUse::Recognize)
        .filter_map(|(index, spec)| {
            let id = box_mark_id(step, box_rect(spec));
            let crop = crops.get(&id)?;
            let mark = own.iter().find(|mark| mark.id == id)?;
            Some(BoxMark {
                index,
                mark: mark.clone(),
                crop: crop.clone(),
            })
        })
        .collect()
}

/// A recognize box's template score on one frame, as the mark-time self-test computes it on that
/// frame alone (`evaluate_mark`: the recording's metric and the mark's threshold, the Runtime's
/// score), and whether it reaches the threshold.
pub(crate) struct BoxScore {
    pub(crate) index: usize,
    pub(crate) score: f64,
    pub(crate) matched_rect: Option<RecordRect>,
    pub(crate) reaches: bool,
}

/// Every recognize box's score on one frame, and the first box (by its index in `boxes`) below
/// its threshold there, if any.
pub(crate) struct CallFrame {
    pub(crate) scores: Vec<BoxScore>,
    pub(crate) below: Option<usize>,
}

/// `marks` scored on `frame`. A score that cannot be computed (the template search failed) is no
/// other state of the screen: it is the mark's rejection, `self_test_error` with the frame, as the
/// self-test gives it.
pub(crate) fn call_frame(
    marks: &[BoxMark],
    frame: &EvalFrame<'_>,
    defaults: &RecordingDefaults,
    ocr: &LabOcr,
) -> LabResult<Result<CallFrame, MarkRejection>> {
    let mut scores = Vec::new();
    for made in marks {
        let (test, failure) = evaluate_mark(
            &made.mark,
            Some(made.crop.as_slice()),
            std::slice::from_ref(frame),
            defaults,
            ocr,
        )?;
        let Some(score) = test.min_score else {
            return Ok(Err(MarkRejection::new(
                &made.mark.id,
                test.reason.as_deref().unwrap_or("self_test_error"),
                failure.unwrap_or_else(|| json!({"frame_id": frame.id})),
            )));
        };
        scores.push(BoxScore {
            index: made.index,
            score,
            matched_rect: test.matched_rect,
            reaches: test.status == SelfTestStatus::Passed,
        });
    }
    let below = scores
        .iter()
        .find(|score| !score.reaches)
        .map(|score| score.index);
    Ok(Ok(CallFrame { scores, below }))
}

/// Why a frame is not added when a recognize box is below its threshold on it: `below_threshold`
/// with the first such box and its mark.
pub(crate) fn below_because(marks: &[BoxMark], scores: &CallFrame) -> Option<BoxBecause> {
    let index = scores.below?;
    let made = marks.iter().find(|made| made.index == index)?;
    Some(BoxBecause {
        reason: BELOW_THRESHOLD.to_owned(),
        mark: Some(made.mark.id.clone()),
        index: Some(index),
        frame_change_milli: None,
    })
}

/// A frame the answer lists for the recognize boxes, with what it is to them and every box's score
/// on it: `frame_id` the frame as the recording holds it, `sample` a sample of the request by its
/// index, `superseded` a capture of the step by its frame id, `step` the other step of a frame
/// `elsewhere`.
pub(crate) struct ScoredFrame {
    pub(crate) frame_id: Option<String>,
    pub(crate) sample: Option<usize>,
    pub(crate) superseded: Option<String>,
    pub(crate) step: Option<u32>,
    pub(crate) taken: BoxFrameTaken,
    pub(crate) because: Option<BoxBecause>,
    pub(crate) scores: CallFrame,
}

/// #463 S2-5: the captures stage one stored for step `target` on the way to its screen and before
/// its press, those the step table shows were taken by a recorded press's or a recorded
/// application operation's own captures after it: the press or operation whose `after_frame` names
/// a frame of the step, with its `after_capture` (the loop stored its captures), took that frame
/// and the `after_capture.captures - 1` frames before it in the step, which arrive in one command
/// under the recording lock and so stand together in the step's frames. Of those, the ones
/// superseded now, in the step's order (the live one is the step's own frame). A frame a
/// `capture --record` or a `record mark --frame` replaced by hand, and the first capture of a press
/// sent twice, are told by nothing in the step table from a capture before the press, so they are
/// not among them.
pub(crate) fn stage_one_captures(recording: &LabRecording, target: u32) -> Vec<RecordedFrame> {
    let Some(step) = recording.steps.iter().find(|step| step.index == target) else {
        return Vec::new();
    };
    let mut captured = vec![false; step.frames.len()];
    for other in &recording.steps {
        // #463 KR-1: a key step's press is the one its key replaced.
        let after = match (other.recorded_click(), &other.application) {
            (Some(click), _) => click
                .execution
                .as_ref()
                .map(|execution| (&execution.after_frame, &execution.after_capture)),
            (None, Some(application)) => application
                .executed
                .as_ref()
                .map(|execution| (&execution.after_frame, &execution.after_capture)),
            (None, None) => None,
        };
        let Some((Some(after_frame), Some(record))) = after else {
            continue;
        };
        let Some(last) = step
            .frames
            .iter()
            .position(|frame| &frame.frame_id == after_frame)
        else {
            continue;
        };
        let captures = usize::try_from(record.captures).unwrap_or(usize::MAX);
        let first = (last + 1).saturating_sub(captures);
        captured[first..=last].fill(true);
    }
    step.frames
        .iter()
        .zip(captured)
        .filter(|(frame, by_press)| *by_press && frame.superseded)
        .map(|(frame, _)| frame.clone())
        .collect()
}

/// Each box of the request as step `step` took it: a recognize box whose mark was made with its
/// threshold, its metric and its score on each of `frames` (a frame `elsewhere` only where the box
/// reaches its threshold).
pub(crate) fn box_views(
    boxes: &[BoxSpec],
    step: u32,
    marks: &[BoxMark],
    frames: &[ScoredFrame],
    defaults: &RecordingDefaults,
) -> Vec<BoxView> {
    boxes
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            let rect = box_rect(spec);
            let made = marks.iter().find(|made| made.index == index);
            let scored = frames
                .iter()
                .filter_map(|frame| {
                    let score = frame
                        .scores
                        .scores
                        .iter()
                        .find(|score| score.index == index)?;
                    if frame.taken == BoxFrameTaken::Elsewhere && !score.reaches {
                        return None;
                    }
                    Some(BoxFrameView {
                        frame_id: frame.frame_id.clone(),
                        sample: frame.sample,
                        superseded: frame.superseded.clone(),
                        step: frame.step,
                        score: score.score,
                        matched_rect: score.matched_rect,
                        taken: frame.taken,
                        because: frame.because.clone(),
                    })
                })
                .collect::<Vec<_>>();
            BoxView {
                usage: spec.usage,
                corners: spec.corners,
                rect,
                mark: (spec.usage == BoxUse::Recognize).then(|| box_mark_id(step, rect)),
                threshold: made
                    .map(|made| made.mark.threshold.unwrap_or(defaults.template_threshold)),
                metric: made.map(|made| {
                    made.mark
                        .metric
                        .clone()
                        .unwrap_or_else(|| defaults.match_metric.clone())
                }),
                frames: scored,
            }
        })
        .collect()
}
