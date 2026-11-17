// SPDX-License-Identifier: AGPL-3.0-only

//! #463 R2 (`contracts/lab-recording.md`, "Endings"): a screen that ends the business another way
//! after the last press (too little balance, nothing to collect), taken by a stopped recording from
//! a frame without recording again. A request's `ending` (`record mark --ending`) gives its frame,
//! samples, page name, outcome key and marks; its marks are self-tested on its frames as any mark
//! is (`steps.rs`, `apply_ending`), and the recording keeps it in `endings[]`. `remove_ending`
//! (`record mark --remove-ending`) gives one up again. `record stop` generates its page and its
//! `scheduling_outcome` mapping (`generate.rs`) and checks it with self-check item 9
//! (`crosscheck.rs`). Every ending that cannot hold is `record_ending_invalid` with its
//! `ending_problem`.

use super::generate::done_key;
use super::model::{LabRecording, MarkRequest, RecordingEnding};
use super::select::{on_empty_key, on_empty_key_valid};
use super::steps::Session;
use super::store::{blocked, invalid, page_name_valid, with_details};
use crate::codes::{EndingProblem, LabCode, ReadyMissing};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{LabError, LabResult};
use serde_json::{Value, json};

/// The `step` an ending's mark carries: the number of no step (steps are numbered from 1).
pub(crate) const ENDING_MARK_STEP: u32 = 0;

/// `record_ending_invalid` with why, for the ending of `outcome_key`, its details carrying the
/// outcome with `ending_problem` and the members of `extra`.
pub(crate) fn ending_invalid(
    problem: EndingProblem,
    outcome_key: Option<&str>,
    message: String,
    extra: Value,
) -> LabError {
    let mut details = json!({
        "outcome_key": outcome_key,
        "ending_problem": problem.as_str(),
        "outcome": Outcome::new(LabCode::RecordEndingInvalid)
            .ending_problem(problem)
            .envelope(Detail::Full)
    });
    if let (Some(object), Some(extra)) = (details.as_object_mut(), extra.as_object()) {
        for (name, value) in extra {
            object.insert(name.clone(), value.clone());
        }
    }
    with_details(
        blocked(LabCode::RecordEndingInvalid.as_str(), message),
        details,
    )
}

/// The page an ending generates: `ending_<number>` or `ending_<number>_<page>`, `number` in the
/// width of the package's other numbers.
pub(crate) fn ending_page_id(number: &str, page: Option<&str>) -> String {
    match page {
        Some(name) => format!("ending_{number}_{name}"),
        None => format!("ending_{number}"),
    }
}

/// The members of `request` beside `ending` and `remove_ending`, each by its name in the request
/// form: an ending is no step and changes none, so a request that names one of them with an
/// ending, or both of these, is refused, never cut down.
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
        // #463 S2-5: a box marks a step or presses its click.
        ("boxes", !request.boxes.is_empty()),
        // #463 S2-6: a choice marks a step.
        ("choice", request.choice.is_some()),
        // #463 KR-1: a key is a step's effect.
        ("key", request.key.is_some()),
        // #463 R4: an inserted optional step is no ending either.
        ("optional_after", request.optional_after.is_some()),
        ("remove_inserted", request.remove_inserted.is_some()),
        (
            "remove_ending",
            request.ending.is_some() && request.remove_ending.is_some(),
        ),
    ]
    .into_iter()
    .filter(|(_, present)| *present)
    .map(|(name, _)| name)
    .collect()
}

/// The rules of a request that names an ending, before any recording is read: it stands alone
/// (`with_other_change`), its outcome key keeps its grammar (`key_invalid`), its page name the
/// grammar of a step's page (`validation_failed`, exit 2, as a step's `--page`), and it names a
/// mark (`no_marks`). `remove_ending` stands alone and keeps the key's grammar too.
pub(crate) fn validate_ending_request(request: &MarkRequest) -> LabResult<()> {
    let key = request
        .ending
        .as_ref()
        .map(|ending| ending.outcome_key.as_str())
        .or(request.remove_ending.as_deref());
    let members = other_members(request);
    if !members.is_empty() {
        return Err(ending_invalid(
            EndingProblem::WithOtherChange,
            key,
            format!(
                "an ending is no step and stands alone in its request; the request also names {}; \
                 nothing was written",
                members.join(", ")
            ),
            json!({"members": members}),
        ));
    }
    if let Some(key) = key
        && !on_empty_key_valid(key)
    {
        return Err(ending_invalid(
            EndingProblem::KeyInvalid,
            Some(key),
            format!(
                "the outcome key '{key}' must match ^[a-z0-9][a-z0-9_]{{0,127}}$; nothing was \
                 written"
            ),
            json!({}),
        ));
    }
    let Some(ending) = &request.ending else {
        return Ok(());
    };
    if let Some(page) = &ending.page
        && !page_name_valid(page)
    {
        return Err(invalid(
            "validation_failed",
            format!("the ending's page '{page}' must match ^[a-z0-9][a-z0-9_]{{0,47}}$"),
        ));
    }
    if ending.add.is_empty() && ending.reuse.is_empty() {
        return Err(ending_invalid(
            EndingProblem::NoMarks,
            Some(ending.outcome_key.as_str()),
            format!(
                "the ending '{}' adds and reuses no mark, so its page would recognize nothing; \
                 add a mark of its screen, such as a template of its message; nothing was written",
                ending.outcome_key
            ),
            json!({}),
        ));
    }
    Ok(())
}

/// Why `key` cannot be the outcome key of an ending of `recording` (the ending at `own` itself
/// excepted): the usual ending's `<task_id>_done` (`key_reserved`), the `on_empty` key of a select
/// step (`key_of_select`), or another ending's (`key_exists`); `None` when it can. #463 S2-1: with
/// the word `record ready` lists the cause with.
pub(crate) fn ending_key_problem(
    recording: &LabRecording,
    key: &str,
    own: Option<usize>,
) -> Option<(EndingProblem, ReadyMissing, String)> {
    if key == done_key(&recording.task_id) {
        return Some((
            EndingProblem::KeyReserved,
            ReadyMissing::EndingKeyReserved,
            format!("the outcome key '{key}' is the key of the run's usual end"),
        ));
    }
    if let Some(step) = recording
        .steps
        .iter()
        .filter(|step| step.is_effective())
        .find(|step| {
            step.select
                .as_ref()
                .is_some_and(|select| on_empty_key(&recording.task_id, &select.declaration) == key)
        })
    {
        return Some((
            EndingProblem::KeyOfSelect,
            ReadyMissing::EndingKeyOfSelect,
            format!(
                "the outcome key '{key}' is the on_empty key of the select of step {}",
                step.index
            ),
        ));
    }
    recording
        .endings
        .iter()
        .enumerate()
        .find(|(position, ending)| Some(*position) != own && ending.outcome_key == key)
        .map(|_| {
            (
                EndingProblem::KeyExists,
                ReadyMissing::EndingKeyTaken,
                format!("the outcome key '{key}' is another ending's of the recording"),
            )
        })
}

/// The rules every ending of `recording` keeps where `record stop` generates the package, as
/// `record mark` checks them when it takes one: its key is the usual ending's, a select step's
/// (also a select declared after the ending) or another ending's, or its page name another
/// ending's. Each is `record_ending_invalid` with its word. #463 S2-1: every ending that breaks
/// one, in the order of the endings and for each in the order above, with the word `record ready`
/// lists it with; `record stop` answers the first.
pub(crate) fn ending_rule_refusals(recording: &LabRecording) -> Vec<(ReadyMissing, LabError)> {
    let mut refusals = Vec::new();
    for (position, ending) in recording.endings.iter().enumerate() {
        let key = ending.outcome_key.as_str();
        if let Some((problem, what, message)) = ending_key_problem(recording, key, Some(position)) {
            refusals.push((
                what,
                ending_invalid(
                    problem,
                    Some(key),
                    format!("{message}; give up the ending with record mark --remove-ending {key}"),
                    json!({}),
                ),
            ));
        }
        if let Some(page) = &ending.page
            && recording.endings[..position]
                .iter()
                .any(|earlier| earlier.page.as_ref() == Some(page))
        {
            refusals.push((
                ReadyMissing::EndingPageTaken,
                ending_invalid(
                    EndingProblem::PageExists,
                    Some(key),
                    format!("the page name '{page}' of the ending '{key}' is another ending's"),
                    json!({"page": page}),
                ),
            ));
        }
    }
    refusals
}

/// The checks of a stopped recording's `session` that need no frame before it takes the ending
/// `key` with the page name `page`: the recording is stopped (`recording_active`), and the key and
/// the page name are its own.
pub(crate) fn check_new_ending(session: &Session, key: &str, page: Option<&str>) -> LabResult<()> {
    if !session.stopped {
        return Err(recording_active(key));
    }
    if let Some((problem, _, message)) = ending_key_problem(&session.recording, key, None) {
        return Err(ending_invalid(
            problem,
            Some(key),
            format!("{message}; nothing was written"),
            json!({}),
        ));
    }
    if let Some(page) = page
        && session
            .recording
            .endings
            .iter()
            .any(|ending| ending.page.as_deref() == Some(page))
    {
        return Err(ending_invalid(
            EndingProblem::PageExists,
            Some(key),
            format!(
                "the page name '{page}' is another ending's of the recording; nothing was written"
            ),
            json!({"page": page}),
        ));
    }
    Ok(())
}

/// An ending on an active recording: `recording_active`.
fn recording_active(key: &str) -> LabError {
    ending_invalid(
        EndingProblem::RecordingActive,
        Some(key),
        "the recording is active: an ending is taken or given up on a stopped recording, after \
         record stop has generated its package with the usual ending; nothing was written"
            .to_string(),
        json!({}),
    )
}

/// `record mark --remove-ending <key>`: the stopped recording gives up its ending `key`, which is
/// returned as the recording held it. Its frames and crops stay in the recording's directory, as
/// every stored file does; the next `record stop` generates the package without it.
pub(crate) fn remove_ending(session: &mut Session, key: &str) -> LabResult<RecordingEnding> {
    if !session.stopped {
        return Err(recording_active(key));
    }
    let Some(position) = session
        .recording
        .endings
        .iter()
        .position(|ending| ending.outcome_key == key)
    else {
        let keys = session
            .recording
            .endings
            .iter()
            .map(|ending| ending.outcome_key.clone())
            .collect::<Vec<_>>();
        return Err(ending_invalid(
            EndingProblem::KeyUnknown,
            Some(key),
            format!("no ending of the recording has the outcome key '{key}'; nothing was written"),
            json!({"endings": keys}),
        ));
    };
    Ok(session.recording.endings.remove(position))
}
