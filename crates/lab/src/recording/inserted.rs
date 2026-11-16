// SPDX-License-Identifier: AGPL-3.0-only

//! #463 R4 (`contracts/lab-recording.md`, "Inserting an optional step after stop"): a pop-up a run
//! met after the effect of a step, after which the path goes on, taken by a stopped recording as
//! an optional step from a frame, without recording again. A request's `optional_after` (`record
//! mark --optional-after <k>`) gives its frame, samples, page name, marks, click and settle; the
//! step takes the next serial number and `inserted_after`, the recorded step it runs after
//! (`steps.rs`, `apply_optional_after`, and `path_order`, the order the steps run in).
//! `remove_inserted` (`record mark --remove-inserted <n>`) gives one up again. `record stop`
//! generates and checks it as every optional step. Every insertion that cannot hold is
//! `record_optional_after_invalid` with its `optional_after_problem`.

use super::model::{LAB_RECORDING_MAX_SETTLE_MS, MarkRequest, OptionalAfterSpec};
use super::store::{blocked, invalid, page_name_valid, with_details};
use crate::codes::{LabCode, OptionalAfterProblem};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{LabError, LabResult};
use serde_json::{Value, json};

/// `record_optional_after_invalid` with why, for the step `step` (the step named after or the
/// inserted step given up), its details carrying the outcome with `optional_after_problem` and the
/// members of `extra`.
pub(crate) fn optional_after_invalid(
    problem: OptionalAfterProblem,
    step: Option<u32>,
    message: String,
    extra: Value,
) -> LabError {
    let mut details = json!({
        "step": step,
        "optional_after_problem": problem.as_str(),
        "outcome": Outcome::new(LabCode::RecordOptionalAfterInvalid)
            .optional_after_problem(problem)
            .envelope(Detail::Full)
    });
    if let (Some(object), Some(extra)) = (details.as_object_mut(), extra.as_object()) {
        for (name, value) in extra {
            object.insert(name.clone(), value.clone());
        }
    }
    with_details(
        blocked(LabCode::RecordOptionalAfterInvalid.as_str(), message),
        details,
    )
}

/// The members of `request` beside `optional_after` and `remove_inserted`, each by its name in the
/// request form: an inserted step stands alone, so a request that names one of them with it, or
/// both of these, is refused, never cut down.
fn other_members(request: &MarkRequest) -> Vec<&'static str> {
    [
        ("step", request.step.is_some()),
        ("frame", request.frame.is_some()),
        ("samples", !request.samples.is_empty()),
        ("page", request.page.is_some()),
        ("add", !request.add.is_empty()),
        ("reuse", !request.reuse.is_empty()),
        ("remove", !request.remove.is_empty()),
        ("click", request.click.is_some()),
        ("click_guard", request.click_guard.is_some()),
        ("retry", request.retry.is_some()),
        ("replace_click", request.replace_click),
        ("transition", request.transition.is_some()),
        ("replace_transition", request.replace_transition),
        ("step_action", request.step_action.is_some()),
        ("application", request.application.is_some()),
        ("optional", request.optional.is_some()),
        ("optional_settle_ms", request.optional_settle_ms.is_some()),
        ("forbid", !request.forbid.is_empty()),
        ("anchor", request.anchor.is_some()),
        ("clear_anchor", request.clear_anchor),
        ("select", request.select.is_some()),
        ("clear_select", request.clear_select),
        // #463 S2-5: a box marks a recorded step or presses its click; an inserted step takes
        // its marks and click in `optional_after`.
        ("boxes", !request.boxes.is_empty()),
        // #463 S2-6: a choice marks a recorded step; an inserted step is optional, and a step
        // marked as a choice is not.
        ("choice", request.choice.is_some()),
        ("ending", request.ending.is_some()),
        ("remove_ending", request.remove_ending.is_some()),
        (
            "remove_inserted",
            request.optional_after.is_some() && request.remove_inserted.is_some(),
        ),
    ]
    .into_iter()
    .filter(|(_, present)| *present)
    .map(|(name, _)| name)
    .collect()
}

/// The rules of a request that inserts an optional step or gives one up, before any recording is
/// read: it stands alone (`with_other_change`), its page name keeps the grammar of a step's page
/// (`validation_failed`, exit 2, as a step's `--page`), its settle lies in 0..=60000
/// (`record_optional_settle_invalid`, exit 2, as `--settle-ms`), and it names a mark (`no_marks`)
/// and a click (`no_click`).
pub(crate) fn validate_optional_after_request(request: &MarkRequest) -> LabResult<()> {
    let step = request
        .optional_after
        .as_ref()
        .map(|spec| spec.step)
        .or(request.remove_inserted);
    let members = other_members(request);
    if !members.is_empty() {
        return Err(optional_after_invalid(
            OptionalAfterProblem::WithOtherChange,
            step,
            format!(
                "an inserted optional step stands alone in its request; the request also names \
                 {}; nothing was written",
                members.join(", ")
            ),
            json!({"members": members}),
        ));
    }
    let Some(spec) = &request.optional_after else {
        return Ok(());
    };
    validate_spec(spec)
}

fn validate_spec(spec: &OptionalAfterSpec) -> LabResult<()> {
    if let Some(page) = &spec.page
        && !page_name_valid(page)
    {
        return Err(invalid(
            "validation_failed",
            format!("the inserted step's page '{page}' must match ^[a-z0-9][a-z0-9_]{{0,47}}$"),
        ));
    }
    if let Some(settle_ms) = spec.settle_ms
        && settle_ms > LAB_RECORDING_MAX_SETTLE_MS
    {
        return Err(with_details(
            invalid(
                "record_optional_settle_invalid",
                format!(
                    "the inserted step's settle_ms must be in 0..={LAB_RECORDING_MAX_SETTLE_MS}, \
                     got {settle_ms}"
                ),
            ),
            json!({"settle_ms": settle_ms, "max_settle_ms": LAB_RECORDING_MAX_SETTLE_MS}),
        ));
    }
    if spec.add.is_empty() && spec.reuse.is_empty() {
        return Err(optional_after_invalid(
            OptionalAfterProblem::NoMarks,
            Some(spec.step),
            "the inserted step adds and reuses no mark, so its page would recognize nothing; add \
             a mark of the pop-up, such as a template of its title; nothing was written"
                .to_string(),
            json!({}),
        ));
    }
    if spec.click.is_none() {
        return Err(optional_after_invalid(
            OptionalAfterProblem::NoClick,
            Some(spec.step),
            "the inserted step declares no click, so nothing would close its pop-up; give the \
             click that closes it; nothing was written"
                .to_string(),
            json!({}),
        ));
    }
    Ok(())
}

/// An insertion on an active recording, or a step given up there: `recording_active`.
pub(crate) fn recording_active(step: u32) -> LabError {
    optional_after_invalid(
        OptionalAfterProblem::RecordingActive,
        Some(step),
        "the recording is active: an optional step is inserted or given up on a stopped \
         recording, after record stop has generated the package of the recorded path; nothing was \
         written"
            .to_string(),
        json!({}),
    )
}
