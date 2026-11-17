// SPDX-License-Identifier: AGPL-3.0-only

//! #463 KR-1 (`contracts/lab-recording.md`, "Key steps"; the owner's ruling on board #463 that the
//! back key is not recorded while playing and that an agent adds such a step afterwards): a
//! recording's step may press the back key in place of a click, declared on the recording after the
//! fact and never sent while recording. A request's `key` (`record mark --key back`, `--no-key`)
//! declares it or takes it back; an optional step inserted after the stop may be closed by it
//! (`optional_after.key`). The key is the step's one effect, as an application operation is: a
//! click it replaces (only with `replace_click`; executed or declared, of any form) is kept whole
//! inside it as what it replaced and given back exactly when the key is taken back, and the step's
//! frames stand. Its guard is the request's click guard, named by the agent: `record stop` writes a
//! key step only with it (`generate.rs`, `key_guards`). Every refusal of a key is
//! `record_key_invalid` with its `key_problem`, but for a second effect without `replace_click`,
//! the existing `record_step_effect_exists`.

use super::boxes::with_press_click;
use super::model::{
    KeyRequest, LabRecording, MarkRequest, OptionalAfterSpec, RecordedKey, RecordingStep,
};
use super::steps::{effect_exists, holds_choice, step_ref};
use super::store::{blocked, invalid, now_unix_ms, with_details};
use crate::codes::{KeyProblem, LabCode};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{LabError, LabResult};
use actingcommand_execution_kernel::StepKey;
use serde_json::{Value, json};

/// What a step whose effect is a key needs before another effect is declared on it.
const TAKE_THE_KEY_BACK: &str = "take the key back first with key false (--no-key), which gives \
                                 back the click it replaced";

/// `record_key_invalid` with why, for the step `step` (for an inserted step, the step it is
/// inserted after), its details carrying the outcome with `key_problem` and the members of `extra`.
pub(crate) fn key_invalid(
    problem: KeyProblem,
    step: Option<u32>,
    message: String,
    extra: Value,
) -> LabError {
    let mut details = json!({
        "step": step,
        "key_problem": problem.as_str(),
        "outcome": Outcome::new(LabCode::RecordKeyInvalid)
            .key_problem(problem)
            .envelope(Detail::Full)
    });
    if let (Some(object), Some(extra)) = (details.as_object_mut(), extra.as_object()) {
        for (name, value) in extra {
            object.insert(name.clone(), value.clone());
        }
    }
    with_details(
        blocked(LabCode::RecordKeyInvalid.as_str(), message),
        details,
    )
}

/// The key `word` names in the package's closed vocabulary, the kernel's `StepKey` (`back`
/// alone); any other word is `word_unknown`, its word in the details.
fn known_word(word: &str, step: Option<u32>) -> LabResult<()> {
    if StepKey::from_word(word).is_some() {
        return Ok(());
    }
    Err(key_invalid(
        KeyProblem::WordUnknown,
        step,
        format!(
            "'{word}' is no key of the vocabulary: a step presses the back key alone (back); \
             nothing was written"
        ),
        json!({"key": word}),
    ))
}

/// The refusal of a key and a choice on one step.
fn key_with_choice(step: Option<u32>) -> LabError {
    key_invalid(
        KeyProblem::HoldsChoice,
        step,
        "a step that presses a key holds no choice, and a step that holds a choice (a select or a \
         choice mark) presses no key: its press chooses an item; withdraw the choice first \
         (choice false, clear_select) or take the key back (key false); nothing was written"
            .to_string(),
        json!({}),
    )
}

/// #463 KR-1: the rules of a request's `key`, before any recording is read. `true` is no key
/// (`validation_failed`); a key is the effect of a step, never beside a step operation or a
/// transition (`validation_failed`). A key declared names a word of the vocabulary
/// (`word_unknown`) and stands beside no click (`with_click`: a rectangle, a source mark, a
/// followed mark, a gesture or a press box), no application operation (`with_application`), no
/// click retry (`with_retry`, a key step declares none) and no select or choice (`holds_choice`).
pub(crate) fn validate_key_request(request: &MarkRequest) -> LabResult<()> {
    let Some(change) = &request.key else {
        return Ok(());
    };
    if request.step_action.is_some() || request.transition.is_some() || request.replace_transition {
        return Err(invalid(
            "validation_failed",
            "a key is the effect of a step: it is not given beside a step operation or a \
             transition",
        ));
    }
    let spec = match change {
        KeyRequest::TakeBack(false) => return Ok(()),
        KeyRequest::TakeBack(true) => {
            return Err(invalid(
                "validation_failed",
                "key takes {\"key\": \"back\"}, which declares the back key, or false, which takes \
                 it back",
            ));
        }
        KeyRequest::Declare(spec) => spec,
    };
    let step = request.step;
    known_word(&spec.key, step)?;
    let beside = |problem: KeyProblem, what: &str| {
        key_invalid(
            problem,
            step,
            format!(
                "a key is the step's one effect, and the request also declares {what}; nothing \
                 was written"
            ),
            json!({}),
        )
    };
    if with_press_click(request).click.is_some() {
        return Err(beside(KeyProblem::WithClick, "a click"));
    }
    if request.application.is_some() {
        return Err(beside(
            KeyProblem::WithApplication,
            "an application operation",
        ));
    }
    if request.retry.is_some() {
        return Err(beside(
            KeyProblem::WithRetry,
            "a click retry (a key step declares no retry: its one press ends the run where its next \
             page does not come)",
        ));
    }
    if request.select.is_some() || request.choice == Some(true) {
        return Err(key_with_choice(step));
    }
    Ok(())
}

/// #463 KR-1: an inserted step's key, in place of its click: never both (`with_click`, the refusal
/// of a key with a click), and a word of the vocabulary (`word_unknown`). An inserted step with
/// neither is `record_optional_after_invalid` with `no_effect` (`inserted.rs`).
pub(crate) fn validate_inserted_key(spec: &OptionalAfterSpec) -> LabResult<()> {
    let Some(key) = &spec.key else {
        return Ok(());
    };
    if spec.click.is_some() {
        return Err(key_invalid(
            KeyProblem::WithClick,
            Some(spec.step),
            "the inserted step declares a click and a key; its one effect is one of them; nothing \
             was written"
                .to_string(),
            json!({}),
        ));
    }
    known_word(&key.key, Some(spec.step))
}

/// #463 KR-1: a key declared on a step the recording holds dropped or converted into a page
/// transition: `step_not_effective`. A step it does not hold stays `record_step_not_found`, as for
/// every request that names one.
pub(crate) fn key_on_dropped(recording: &LabRecording, request: &MarkRequest) -> LabResult<()> {
    if let (Some(KeyRequest::Declare(_)), Some(index)) = (&request.key, request.step)
        && step_ref(recording, index).is_some_and(|step| !step.is_effective())
    {
        return Err(key_invalid(
            KeyProblem::StepNotEffective,
            Some(index),
            format!(
                "step {index} is dropped or converted into a page transition, so it presses \
                 nothing; nothing was written"
            ),
            json!({}),
        ));
    }
    Ok(())
}

/// #463 KR-1: a key declared on the application entry step (`target`), which has no frame and so
/// no page to guard a key on: `application_entry`.
pub(crate) fn key_on_entry(request: &MarkRequest, target: u32) -> LabResult<()> {
    if matches!(request.key, Some(KeyRequest::Declare(_))) {
        return Err(key_invalid(
            KeyProblem::ApplicationEntry,
            Some(target),
            format!(
                "step {target} is the application entry step: it has no frame and no page to \
                 guard a key on; nothing was written"
            ),
            json!({}),
        ));
    }
    Ok(())
}

/// #463 KR-1: the request's `key` on `step`. `false` takes the step's key back and gives back the
/// click it replaced exactly as it was (none when it replaced none); on a step without a key it
/// changes nothing. A key declared becomes the step's one effect: on a step without an effect,
/// declared; on a step with a click of any form, executed or declared, only with `replace_click`
/// (otherwise the refusal of a second effect, `record_step_effect_exists`), the click kept whole
/// inside the key as what it replaced; a key declared again with `replace_click` keeps the click
/// the first replaced. A step whose effect is an application operation (`application_step`) or
/// that holds a choice (`holds_choice`) is refused. Nothing is sent.
pub(crate) fn apply_key(step: &mut RecordingStep, request: &MarkRequest) -> LabResult<()> {
    let spec = match &request.key {
        None => return Ok(()),
        Some(KeyRequest::TakeBack(_)) => {
            if let Some(key) = step.key.take() {
                step.click = key.replaced;
            }
            return Ok(());
        }
        Some(KeyRequest::Declare(spec)) => spec,
    };
    let index = step.index;
    if let Some(application) = &step.application {
        return Err(key_invalid(
            KeyProblem::ApplicationStep,
            Some(index),
            format!(
                "step {index}'s effect is the application operation {}; one step has one effect, \
                 and an application step presses no key; nothing was written",
                application.action
            ),
            json!({"application": application.action}),
        ));
    }
    if holds_choice(step) {
        return Err(key_with_choice(Some(index)));
    }
    if !request.replace_click {
        if step.click.is_some() {
            return Err(effect_exists(
                index,
                "click",
                "add --replace-click to replace it with the key, which keeps it whole and gives it \
                 back when the key is taken back",
            ));
        }
        if step.key.is_some() {
            return Err(effect_exists(
                index,
                "key",
                "add --replace-click to declare it again",
            ));
        }
    }
    let replaced = match step.key.take() {
        Some(earlier) => earlier.replaced,
        None => step.click.take(),
    };
    step.key = Some(RecordedKey {
        key: spec.key.clone(),
        declared_at_unix_ms: now_unix_ms(),
        replaced,
    });
    Ok(())
}

/// #463 KR-1: a step whose effect is a key keeps it as its one effect whatever the request
/// changed: a choice given to it is `holds_choice`, and a click or an application operation
/// declared on it the refusal of a second effect, `record_step_effect_exists` with the effect
/// `key` (the key is taken back first).
pub(crate) fn check_key_step(step: &RecordingStep) -> LabResult<()> {
    if step.key.is_none() {
        return Ok(());
    }
    if holds_choice(step) {
        return Err(key_with_choice(Some(step.index)));
    }
    if step.click.is_some() || step.application.is_some() {
        return Err(key_effect_exists(step.index));
    }
    Ok(())
}

/// #463 KR-1: step `index` presses a key, so no other effect lands on it, by a mark or by a device
/// command recording a press or an application operation: `record_step_effect_exists`, its effect
/// `key`.
pub(crate) fn key_effect_exists(index: u32) -> LabError {
    effect_exists(index, "key", TAKE_THE_KEY_BACK)
}
