// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #396 REC-4 (REC-M v1.2.1 §6): a click that follows a template mark. The package
//! clicks the box the template matches on the frame it runs on (`target`), or that box's
//! top-left corner plus the declared rectangle's offset from the crop (`offset`, scaled by the
//! factor a template with `scales` matched at), instead of the fixed rectangle. `record mark`
//! checks the declaration (`not_template`, `not_required`, `guard_conflict`; the command is
//! refused) and every live frame of the step (`primary_mismatch`, `template_unmatched`,
//! `click_outside_frame`; a reason inside `record_mark_rejected`), each under
//! `record_click_follow_invalid`.

use super::marks::{EvalFrame, MarkRejection, template_frame_evaluations};
use super::model::{
    ClickFollow, ClickFollowForm, MarkFamily, RecordRect, RecordSize, RecordedFrame, RecordedMark,
    RecordingDefaults, RecordingStep,
};
use super::store::{blocked, with_details};
use crate::codes::{ClickFollowProblem, LabCode, LabLocation};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{LabError, LabResult};
use actingcommand_recognition::resample::scale_signed;
use actingcommand_recognition_pack::TemplateEvaluation;
use serde_json::{Value, json};

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

/// The followed mark: a template mark of the step (own or reused), else `not_template`.
pub(crate) fn followed_template(
    step: u32,
    mark_id: &str,
    mark: Option<RecordedMark>,
) -> LabResult<RecordedMark> {
    match mark {
        Some(mark) if mark.family == MarkFamily::Template => Ok(mark),
        Some(mark) => Err(follow_invalid(
            step,
            mark_id,
            ClickFollowProblem::NotTemplate,
            format!(
                "the click follows '{mark_id}', a {} mark; a click follows a template mark of \
                 the step",
                mark.family.as_str()
            ),
        )),
        None => Err(follow_invalid(
            step,
            mark_id,
            ClickFollowProblem::NotTemplate,
            format!(
                "the click follows '{mark_id}', which is not a template mark of this step; add \
                 or reuse the template first"
            ),
        )),
    }
}

/// The declaration rules a step with a following click keeps after every `record mark`: the
/// followed template is a required mark of the page (not only a member of an `any_of` check
/// among `marks`, the step's own and reused marks), and the click guard is absent or the
/// followed template.
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
                 package's guard is the followed template and must be a required mark of the \
                 page"
            ),
        ));
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
                 click is the followed template: leave --click-guard out or name '{id}'"
            ),
        ));
    }
    Ok(())
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

fn inside(rect: RecordRect, size: RecordSize) -> bool {
    rect.x >= 0
        && rect.y >= 0
        && rect.width >= 1
        && rect.height >= 1
        && i64::from(rect.x) + i64::from(rect.width) <= i64::from(size.width)
        && i64::from(rect.y) + i64::from(rect.height) <= i64::from(size.height)
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
