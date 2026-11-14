// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #396 REC-4 (REC-M v1.2.1 §6): a click that follows a template mark. The package
//! clicks the box the template matches on the frame it runs on (`target`), or that box's
//! top-left corner plus the declared rectangle's offset from the crop (`offset`, scaled by the
//! factor a template with `scales` matched at), instead of the fixed rectangle. `record mark`
//! checks the declaration (`not_template`, `not_required`, `guard_conflict`; the command is
//! refused) and every live frame of the step (`primary_mismatch`, `template_unmatched`,
//! `click_outside_frame`; a reason inside `record_mark_rejected`), each under
//! `record_click_follow_invalid`.
//!
//! #463 L2: a click may follow a located-text mark instead (an OCR mark that declares `locate`):
//! the package clicks the block the mark's names are found in, or the declared cell holding it
//! (`target`), or that box's corner plus the declared rectangle's offset from it on the primary
//! frame (`offset`). Its frames are checked with the Lab's OCR provider: the click box lies inside
//! every frame (`click_outside_frame`) and, once the step's click was executed, its press lies in
//! the box on the primary frame (`press_outside_found_box`). A frame on which the names are not
//! found is the mark's own self-test refusal. #463 L2 fix 1: the followed located-text mark is the
//! click's guard and no mark of the step's page, which another mark recognizes
//! (`page_needs_followed_text` otherwise); a command that changes only its names is not held to
//! the recorded press.

use super::marks::{EvalFrame, MarkRejection, ocr_frame_evaluations, template_frame_evaluations};
use super::model::{
    ClickFollow, ClickFollowForm, MarkFamily, RecordPoint, RecordRect, RecordSize, RecordedFrame,
    RecordedMark, RecordingDefaults, RecordingStep, StepClick,
};
use super::store::{blocked, with_details};
use crate::codes::{ClickFollowProblem, LabCode, LabLocation};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{LabError, LabResult};
use actingcommand_recognition::resample::scale_signed;
use actingcommand_recognition_pack::{
    OcrLocatedEvidence, TargetEvaluation, TemplateEvaluation, VisionProvider,
};
use serde_json::{Value, json};
use std::sync::Arc;

/// The factor a template without `scales` matches at.
const NATIVE_SCALE_MILLI: u32 = 1000;

/// `record_click_follow_invalid` with the followed mark and why.
fn follow_outcome(mark_id: &str, problem: ClickFollowProblem) -> Outcome {
    Outcome::new(LabCode::RecordClickFollowInvalid)
        .mark_id(mark_id)
        .follow_problem(problem)
        .stage(LabLocation::RecordClickFollowCheck)
}

/// A declaration that cannot hold: `record mark` is refused and nothing is written.
pub(crate) fn follow_invalid(
    step: u32,
    mark_id: &str,
    problem: ClickFollowProblem,
    message: String,
) -> LabError {
    with_details(
        blocked(LabCode::RecordClickFollowInvalid.as_str(), message),
        json!({
            "step": step,
            "outcome": follow_outcome(mark_id, problem).envelope(Detail::Full)
        }),
    )
}

/// A frame on which the click does not hold: a refused mark of the batch, under the followed
/// mark's id.
fn follow_rejection(
    mark_id: &str,
    problem: ClickFollowProblem,
    mut detail: Value,
) -> MarkRejection {
    if let Some(object) = detail.as_object_mut() {
        object.insert(
            "outcome".to_string(),
            json!(follow_outcome(mark_id, problem).envelope(Detail::Full)),
        );
    }
    MarkRejection::new(mark_id, LabCode::RecordClickFollowInvalid.as_str(), detail)
}

/// The followed mark: a template mark of the step (own or reused) or (#463 L2) a located-text
/// mark of the step, else `not_template`.
pub(crate) fn followed_template(
    step: u32,
    mark_id: &str,
    mark: Option<RecordedMark>,
) -> LabResult<RecordedMark> {
    match mark {
        Some(mark) if mark.family == MarkFamily::Template || mark.locates_text() => Ok(mark),
        Some(mark) => Err(follow_invalid(
            step,
            mark_id,
            ClickFollowProblem::NotTemplate,
            format!(
                "the click follows '{mark_id}', a {} mark; a click follows a template mark or a \
                 located-text mark (an OCR mark that declares locate) of the step",
                mark.family.as_str()
            ),
        )),
        None => Err(follow_invalid(
            step,
            mark_id,
            ClickFollowProblem::NotTemplate,
            format!(
                "the click follows '{mark_id}', which is not a template mark or a located-text \
                 mark of this step; add or reuse the mark first"
            ),
        )),
    }
}

/// The declaration rules a step with a following click keeps after every `record mark`: the
/// followed mark is not only a member of an `any_of` check among `marks`, the step's own and
/// reused marks (a followed template is a required mark of the page), and the click guard is
/// absent or the followed mark. #463 L2 fix 1: a followed located-text mark is the click's guard
/// and no mark of the page, so the page stands without it ([`located_page_problem`]).
pub(crate) fn check_follow_declaration(
    step: &RecordingStep,
    follow: &ClickFollow,
    marks: &[RecordedMark],
) -> LabResult<()> {
    let id = follow.mark_id.as_str();
    let any_of_member = marks
        .iter()
        .filter_map(|mark| mark.any_of.as_ref())
        .flatten()
        .any(|member| member == id);
    if any_of_member {
        return Err(follow_invalid(
            step.index,
            id,
            ClickFollowProblem::NotRequired,
            format!(
                "the click follows '{id}', which only an any_of check of the step names; the \
                 package's guard is the followed mark, and a followed template must be a required \
                 mark of the page"
            ),
        ));
    }
    if marks
        .iter()
        .any(|mark| mark.id == id && mark.locates_text())
    {
        let step_marks = marks.iter().collect::<Vec<_>>();
        if let Some(message) = located_page_problem(id, &step_marks) {
            return Err(follow_invalid(
                step.index,
                id,
                ClickFollowProblem::PageNeedsFollowedText,
                message,
            ));
        }
    }
    if let Some(guard) = step.click_guard.as_deref()
        && guard != id
    {
        return Err(follow_invalid(
            step.index,
            id,
            ClickFollowProblem::GuardConflict,
            format!(
                "the click follows '{id}' but its guard is '{guard}'; the guard of a following \
                 click is the followed mark: leave --click-guard out or name '{id}'"
            ),
        ));
    }
    Ok(())
}

/// #463 L2 fix 1: why the page of a step whose click follows the located-text mark `id` would
/// need that mark to be recognized, among `marks` (every own and reused mark of the step, the
/// followed one too): a check of the page names it, or no other mark recognizes the page (a
/// member of an `any_of` check recognizes nothing on its own; the check does). `None` when the
/// page stands without it: a screen on which the name is missing is then still the step's page,
/// and the guard, not the page, refuses it.
pub(crate) fn located_page_problem(id: &str, marks: &[&RecordedMark]) -> Option<String> {
    if let Some(check) = marks
        .iter()
        .find(|mark| mark.id != id && mark.check_members().iter().any(|member| member == id))
    {
        return Some(format!(
            "the click follows the located-text mark '{id}', which is its guard and no mark of the \
             step's page, but the check '{}' of the page names it: the page would not be \
             recognized on a screen without the name; take '{id}' out of the check",
            check.id
        ));
    }
    let any_of_members = marks
        .iter()
        .filter_map(|mark| mark.any_of.as_ref())
        .flatten()
        .collect::<Vec<_>>();
    let recognized = marks
        .iter()
        .any(|mark| mark.id != id && !any_of_members.contains(&&mark.id));
    (!recognized).then(|| {
        format!(
            "the click follows the located-text mark '{id}', which is its guard and no mark of the \
             step's page, and the step has no other mark that recognizes its page: add a \
             template, color, color digest, check or another OCR mark that holds on the step's \
             frames"
        )
    })
}

/// The box the package clicks when the followed template matched at `template`: the match box,
/// or its top-left corner plus the offset, each of the four numbers scaled by the factor of a
/// template with `scales` as the kernel scales a click offset (`(|v| * f + 500) div 1000` with
/// the sign of `v`, a width or height at least 1). `None` when a number leaves the `i32` range.
pub(crate) fn followed_box(
    template: &TemplateEvaluation,
    follow: &ClickFollow,
) -> Option<RecordRect> {
    let matched = RecordRect {
        x: template.x,
        y: template.y,
        width: template.width,
        height: template.height,
    };
    let offset = match (follow.form, follow.offset) {
        (ClickFollowForm::Target, _) => return Some(matched),
        (ClickFollowForm::Offset, offset) => offset?,
    };
    let offset = match template.scale_milli {
        None => offset,
        Some(factor) => RecordRect {
            x: scale_signed(offset.x, factor)?,
            y: scale_signed(offset.y, factor)?,
            width: scale_signed(offset.width, factor)?.max(1),
            height: scale_signed(offset.height, factor)?.max(1),
        },
    };
    Some(RecordRect {
        x: matched.x.checked_add(offset.x)?,
        y: matched.y.checked_add(offset.y)?,
        width: offset.width,
        height: offset.height,
    })
}

/// #463 L2: the box a located text gives: the declared cell holding the found block, else the
/// block.
fn located_rect(located: &OcrLocatedEvidence) -> RecordRect {
    let rect = located.cell_rect.unwrap_or(located.rect);
    RecordRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

/// #463 L2: the box the package clicks when the followed located text gives `found`: that box, or
/// its top-left corner plus the offset, which the kernel never scales for a located text. `None`
/// when a number leaves the `i32` range or an offset click has no offset.
fn located_box(found: RecordRect, follow: &ClickFollow) -> Option<RecordRect> {
    match follow.form {
        ClickFollowForm::Target => Some(found),
        ClickFollowForm::Offset => {
            let offset = follow.offset?;
            Some(RecordRect {
                x: found.x.checked_add(offset.x)?,
                y: found.y.checked_add(offset.y)?,
                width: offset.width,
                height: offset.height,
            })
        }
    }
}

/// Workflow #396 REC-4, #463 L2: the box a following click clicks on a frame on which the followed
/// mark evaluated as `evaluation`: a passing template's [`followed_box`], or a passing located
/// text's box; `None` when the mark did not pass.
pub(crate) fn followed_evaluation_box(
    evaluation: &TargetEvaluation,
    follow: &ClickFollow,
) -> Option<RecordRect> {
    if !evaluation.passed {
        return None;
    }
    if let Some(template) = &evaluation.template {
        return followed_box(template, follow);
    }
    let located = evaluation.ocr.as_ref()?.located.as_ref()?;
    located_box(located_rect(located), follow)
}

fn inside(rect: RecordRect, size: RecordSize) -> bool {
    rect.x >= 0
        && rect.y >= 0
        && rect.width >= 1
        && rect.height >= 1
        && i64::from(rect.x) + i64::from(rect.width) <= i64::from(size.width)
        && i64::from(rect.y) + i64::from(rect.height) <= i64::from(size.height)
}

fn point_in(point: RecordPoint, rect: RecordRect) -> bool {
    i64::from(point.x) >= i64::from(rect.x)
        && i64::from(point.y) >= i64::from(rect.y)
        && i64::from(point.x) < i64::from(rect.x) + i64::from(rect.width)
        && i64::from(point.y) < i64::from(rect.y) + i64::from(rect.height)
}

/// #463 L2: what the live frames of a step say about its click: the refusal of the first frame on
/// which it does not hold and, for a click that follows a located-text mark, the click as the
/// primary frame gives it, which the step then records.
#[derive(Default)]
pub(crate) struct FollowFrames {
    pub(crate) rejection: Option<MarkRejection>,
    pub(crate) located: Option<LocatedClick>,
}

/// #463 L2: the click of a step that follows a located-text mark, as the primary frame gives it.
pub(crate) struct LocatedClick {
    /// The box the package clicks on the primary frame: what `do --capture --record` presses and
    /// `record status` shows.
    pub(crate) rect: RecordRect,
    /// An `offset` click's offset from the found box's top-left corner, fixed when the click is
    /// declared; `None` for a `target` click.
    pub(crate) offset: Option<RecordRect>,
    /// #463 L2 fix 1: the follow's [`ClickFollow::names_changed_after_press`] after this command;
    /// the caller sets it, from how the command changed the followed mark.
    pub(crate) names_changed_after_press: bool,
}

/// #463 L2: every live frame of a step whose click follows the located-text mark `mark`,
/// evaluated with the Lab's OCR provider as the Runtime's guard evaluates the target. A frame on
/// which the names are not found (or lie in no cell, or not uniquely) is already the mark's own
/// self-test refusal, with the kernel's typed reason, and gives nothing here. Otherwise the click
/// box on each frame lies inside it (`click_outside_frame`) and, when the step's click was
/// executed and `press_check` holds, the press lies in the box on the primary frame
/// (`press_outside_found_box`); the first frame that does not hold gives the rejection. #463 L2
/// fix 1: `press_check` is false for a command that changes only the mark's names, and for one
/// that leaves the mark as it is after its names were changed after the press. An `offset` click
/// declared by this command takes its offset from the declared rectangle and the found box on the
/// primary frame.
pub(crate) fn check_located_follow_frames(
    follow: &ClickFollow,
    click: &StepClick,
    mark: &RecordedMark,
    live: &[RecordedFrame],
    frames: &[EvalFrame<'_>],
    provider: Arc<dyn VisionProvider>,
    press_check: bool,
) -> LabResult<FollowFrames> {
    let id = follow.mark_id.as_str();
    let evaluations = ocr_frame_evaluations(mark, frames, provider)?;
    let mut found = Vec::with_capacity(evaluations.len());
    for frame in &evaluations {
        let located = frame
            .evaluation
            .as_ref()
            .ok()
            .filter(|evaluation| evaluation.passed)
            .and_then(|evaluation| evaluation.ocr.as_ref())
            .and_then(|ocr| ocr.located);
        let Some(located) = located else {
            return Ok(FollowFrames::default());
        };
        found.push(located_rect(&located));
    }
    let primary = live
        .iter()
        .position(|entry| entry.role == "primary")
        .and_then(|index| found.get(index).copied())
        .ok_or_else(|| {
            blocked(
                LabCode::RecordClickFollowInvalid.as_str(),
                format!("the step whose click follows '{id}' has no primary frame"),
            )
        })?;
    let offset = match (follow.form, follow.offset) {
        (ClickFollowForm::Target, _) => None,
        (ClickFollowForm::Offset, Some(offset)) => Some(offset),
        // Declared by this command: the declared rectangle minus the found box's corner.
        (ClickFollowForm::Offset, None) => Some(RecordRect {
            x: click.rect.x.saturating_sub(primary.x),
            y: click.rect.y.saturating_sub(primary.y),
            width: click.rect.width,
            height: click.rect.height,
        }),
    };
    let fixed = ClickFollow {
        offset,
        ..follow.clone()
    };
    let executed = click
        .execution
        .as_ref()
        .filter(|_| press_check && click.executed())
        .map(|execution| execution.point);
    let mut primary_box = None;
    for ((entry, frame), base) in live.iter().zip(&evaluations).zip(&found) {
        let clicked = located_box(*base, &fixed);
        if !clicked.is_some_and(|clicked| inside(clicked, frame.size)) {
            return Ok(FollowFrames {
                rejection: Some(follow_rejection(
                    id,
                    ClickFollowProblem::ClickOutsideFrame,
                    json!({
                        "frame_id": frame.frame_id,
                        "found_rect": base,
                        "click_rect": clicked,
                        "frame_size": frame.size
                    }),
                )),
                located: None,
            });
        }
        if entry.role != "primary" {
            continue;
        }
        if let (Some(point), Some(clicked)) = (executed, clicked)
            && !point_in(point, clicked)
        {
            return Ok(FollowFrames {
                rejection: Some(follow_rejection(
                    id,
                    ClickFollowProblem::PressOutsideFoundBox,
                    json!({
                        "frame_id": frame.frame_id,
                        "found_rect": base,
                        "click_rect": clicked,
                        "point": point
                    }),
                )),
                located: None,
            });
        }
        primary_box = clicked;
    }
    Ok(FollowFrames {
        rejection: None,
        located: primary_box.map(|rect| LocatedClick {
            rect,
            offset,
            names_changed_after_press: follow.names_changed_after_press,
        }),
    })
}

/// Every live frame of the step, primary frame first as the frames are recorded: on the primary
/// frame the template's best match is its crop at factor 1000 (`primary_mismatch`); on every
/// frame the template passes in its search area (`template_unmatched`) and the followed click
/// box lies inside the frame (`click_outside_frame`). The first frame that does not hold gives
/// the rejection, naming the frame.
pub(crate) fn check_follow_frames(
    follow: &ClickFollow,
    mark: &RecordedMark,
    template_png: &[u8],
    live: &[RecordedFrame],
    frames: &[EvalFrame<'_>],
    defaults: &RecordingDefaults,
) -> LabResult<Option<MarkRejection>> {
    let id = follow.mark_id.as_str();
    let crop = mark.region.ok_or_else(|| {
        blocked(
            LabCode::RecordClickFollowInvalid.as_str(),
            format!("the followed template '{id}' has no crop"),
        )
    })?;
    let evaluations = template_frame_evaluations(mark, template_png, frames, defaults)?;
    for (entry, frame) in live.iter().zip(&evaluations) {
        let template = match &frame.evaluation {
            Ok(evaluation) if evaluation.passed => evaluation.template,
            Ok(_) => None,
            Err(error) => {
                return Ok(Some(follow_rejection(
                    id,
                    ClickFollowProblem::TemplateUnmatched,
                    json!({"frame_id": frame.frame_id, "error": error}),
                )));
            }
        };
        let Some(template) = template else {
            let evaluation = frame.evaluation.as_ref().ok();
            return Ok(Some(follow_rejection(
                id,
                ClickFollowProblem::TemplateUnmatched,
                json!({
                    "frame_id": frame.frame_id,
                    "template": evaluation.and_then(|evaluation| evaluation.template)
                }),
            )));
        };
        let matched = RecordRect {
            x: template.x,
            y: template.y,
            width: template.width,
            height: template.height,
        };
        if entry.role == "primary"
            && (matched.x != crop.x
                || matched.y != crop.y
                || template.scale_milli.unwrap_or(NATIVE_SCALE_MILLI) != NATIVE_SCALE_MILLI)
        {
            return Ok(Some(follow_rejection(
                id,
                ClickFollowProblem::PrimaryMismatch,
                json!({
                    "frame_id": frame.frame_id,
                    "crop": crop,
                    "matched_rect": matched,
                    "scale_milli": template.scale_milli
                }),
            )));
        }
        let clicked = followed_box(&template, follow);
        if !clicked.is_some_and(|clicked| inside(clicked, frame.size)) {
            return Ok(Some(follow_rejection(
                id,
                ClickFollowProblem::ClickOutsideFrame,
                json!({
                    "frame_id": frame.frame_id,
                    "matched_rect": matched,
                    "scale_milli": template.scale_milli,
                    "click_rect": clicked,
                    "frame_size": frame.size
                }),
            )));
        }
    }
    Ok(None)
}
