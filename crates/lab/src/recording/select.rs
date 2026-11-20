// SPDX-License-Identifier: AGPL-3.0-only

//! #463 R1 (`contracts/lab-recording.md`, "Selecting in a list"): a recording step that chooses
//! in a list of fixed slots by the name read in each slot. A request's `select` declares the slots,
//! the OCR read of their names, the identity domain (`names`), the wanted value (`wanted`; #463
//! S2-6 fix round 1: in the package an `ordered` variable of its name, its default `[default]`)
//! and the name the agent presses now (`pressed`); the step's click is then the click box of the
//! slot the decision selects for the pressed name on the primary frame (`source: "select"`), which
//! `do --capture --record` presses as any recorded click.
//!
//! The mark-time self-test is the kernel's own dry run of the select step (`dry_run_select`),
//! called on a probe package that holds the step as `record stop` generates it: on every live
//! frame of the step every slot's name must read a known name (`select_name_unread`), the pressed
//! name must select a slot on the primary frame (`select_pressed_absent`) and the executed press
//! must lie in that slot (`select_press_mismatch`); the default's decision is reported per frame
//! by its kind. Fix 1: the page of another step the mark knows must not pass on those frames
//! (`select_other_page_passes`, checked in `steps.rs`); #463 SC-1: and on the frames of the step
//! before it (fix round 1: the frames its confirmation can meet besides its own), where the select
//! step's page passes, no other page may pass beside it (`select_page_passes_on_other_frame`, also
//! in `steps.rs`). Without an OCR model the select is
//! `not_evaluated` (`lab_ocr_unavailable`), never passed. Every declaration that cannot hold is
//! `record_select_invalid` with its `select_problem`; `clear_select` takes a select off again.
//!
//! #463 S2-6 (`contracts/lab-recording.md`, "Marking a choice first"): a request's `choice: true`
//! only marks that the step holds a choice, its press the demonstration ([`apply_choice`]); the
//! step's select refines it later, on the active or the stopped recording, as a select is declared
//! on any step whose press is recorded. A step marked so keeps a select step's rules from its mark
//! on ([`check_select_rules`]); `record stop` generates no package while one is not refined (the
//! pre-check `select_unrefined`, `generate::survey`). The mark means only that: fix round 1, the
//! wanted value of every select step is the package's `ordered` variable, marked or not, which the
//! self-test reads at its default ([`decide`]); `choice: false` on a step without the mark is
//! refused (`no_choice`).

use super::frames::{LoadedFrame, rect_inside};
use super::generate::{
    SELECT_FEATURE, done_key, render, select_policy_document, select_probe_plan,
};
use super::marks::MarkRejection;
use super::model::{
    ClickSpec, LabRecording, MarkRequest, RecordPoint, RecordRect, RecordedFrame, RecordedMark,
    RecordingStep, SelectClickBefore, SelectDecision, SelectFrameDecision, SelectSelfTest,
    SelectSpec, SelfTestStatus, StepChoice, StepClick, StepSelect,
};
use super::store::{blocked, invalid, now_unix_ms, with_details};
use crate::LabOcr;
use crate::codes::{LabCode, SelectProblem};
use actingcommand_contract::candidate_projection::{
    CandidateFeature, CandidateUnknownReason, ProjectedCandidate,
};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{
    InstanceFactContext, InstanceFactSnapshot, LabError, LabResult, TaskSelectionOutcome,
};
use actingcommand_device::Frame;
use actingcommand_execution_kernel::{PreparedContainedTask, dry_run_select};
use actingcommand_recognition_pack::{
    CandidateIdentityDeclaration, CandidateIdentityEntry, CandidateIdentityRecognition,
    CandidateNameBlock, CandidateNameRead, CandidateUnreadCause,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// The id a select's refusals of `record_mark_rejected` carry, the one select of a step.
pub(crate) const SELECT_REJECTION_ID: &str = "select";
/// A slot's name read no known name of the select on a live frame.
pub(crate) const NAME_UNREAD: &str = "select_name_unread";
/// The pressed name selects no slot on the primary frame.
pub(crate) const PRESSED_ABSENT: &str = "select_pressed_absent";
/// The step's executed press lies outside the slot the pressed name selects on the primary frame.
pub(crate) const PRESS_MISMATCH: &str = "select_press_mismatch";
/// The kernel's dry run of the select failed on a frame (the existing reason of an evaluation
/// error of a mark's self-test).
pub(crate) const SELF_TEST_ERROR: &str = "self_test_error";
/// Fix 1: the page of another effective step passes on a live frame of the select step too, so
/// the run's select would not know its page there (the kernel's `select_page_mismatch` at stop).
pub(crate) const OTHER_PAGE_PASSES: &str = "select_other_page_passes";
/// #463 SC-1: the other way round: the select step's page passes on a live frame its confirmation
/// can meet that is not its own (fix round 1: a frame of the step before it, its page transition's
/// when it has one), and so does another page of the package there. The run's select confirms on
/// a fresh frame that exactly one page of the whole package is recognised
/// (`contained_task_recognition_conflict` otherwise), so on such a screen it would fail; the cause
/// is the select step's own page, which holds on another step's screen. A frame after the select
/// step's press is no such frame: there it is the warning `record_select_page_passes_after_press`.
pub(crate) const PAGE_PASSES_ON_OTHER_FRAME: &str = "select_page_passes_on_other_frame";
/// The instance the probe package is admitted for.
const PROBE_INSTANCE: &str = "lab.record.select";
/// The time the probe's admission may take, as `record stop`'s.
const PROBE_BUDGET: Duration = Duration::from_secs(120);
/// The source of the click a select declares.
pub(crate) const SELECT_SOURCE: &str = "select";

/// `record_select_invalid` on `step` with why, its details carrying the outcome with
/// `select_problem`.
pub(crate) fn select_invalid(
    step: Option<u32>,
    problem: SelectProblem,
    message: String,
) -> LabError {
    with_details(
        blocked(LabCode::RecordSelectInvalid.as_str(), message),
        json!({
            "step": step,
            "select_problem": problem.as_str(),
            "outcome": Outcome::new(LabCode::RecordSelectInvalid)
                .select_problem(problem)
                .envelope(Detail::Full)
        }),
    )
}

/// The `on_empty` key of a select: the declared one, else `<task_id>_not_found`.
pub(crate) fn on_empty_key(task_id: &str, select: &SelectSpec) -> String {
    select
        .on_empty_key
        .clone()
        .unwrap_or_else(|| format!("{task_id}_not_found"))
}

/// #463 S2-9b: the `on_empty` key of a select that ends a run normally when the wanted name is in
/// no slot or row ([`on_empty_key`]); `None` for a scrolling select straight to the wanted item
/// without an end mark, which declares no `on_empty` (`scan::ends_empty`), so that none of the
/// key's rules applies to it.
pub(crate) fn empty_end_key(task_id: &str, select: &SelectSpec) -> Option<String> {
    super::scan::ends_empty(select).then(|| on_empty_key(task_id, select))
}

/// `^[a-z][a-z0-9_]{0,63}$`: the parameter grammar of a name's id and of the wanted value's name.
fn name_id_valid(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

/// `^[a-z0-9][a-z0-9_]{0,127}$`: the grammar of a declared `on_empty` key; #463 R2: and of an
/// ending's outcome key.
pub(crate) fn on_empty_key_valid(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

/// The click members a select takes the place of: a gesture of any kind.
fn click_gesture(click: &ClickSpec) -> bool {
    click.drag_to.is_some()
        || click.long_press.is_some()
        || click.duration_ms.is_some()
        || click.brake_corner.is_some()
        || click.path_drag.is_some()
}

/// The identity declaration the layout of a select declares for its names.
fn identity(select: &SelectSpec) -> CandidateIdentityDeclaration {
    CandidateIdentityDeclaration {
        entries: select
            .names
            .iter()
            .map(|name| CandidateIdentityEntry {
                id: name.id.clone(),
                variant: None,
                aliases: name.aliases.clone(),
            })
            .collect(),
        recognition: CandidateIdentityRecognition::OcrAliases {
            max_distance: select.matching.max_distance,
            minimum_margin: select.matching.minimum_margin,
            minimum_confidence_milli: select.read.minimum_confidence_milli,
            confusions: BTreeMap::new(),
        },
    }
}

/// The rules of a request that declares a select, before any recording is read: nothing beside
/// it that the step's one effect cannot hold (`with_application`, `with_follow`, `with_gesture`,
/// `with_click`, `with_retry`, `on_optional_step`), then its shape. The Lab's own reading of the
/// shape (the slots' count, a rectangle without width or height, an empty value of the read, a
/// zero timeout, a confidence outside 1..=1000, a digest that is not 64 lowercase hex, and the
/// identity domain and match as the identity declaration validates them) is `validation_failed`
/// (exit 2); a name, key or value that breaks its own rule has its word. Fix 1: every other bound
/// of the read (a timeout, the languages, the model reference) is the kernel's: its admission of
/// the probe refuses it at the self-test, `kernel_refused` with the kernel's code.
pub(crate) fn validate_select_request(request: &MarkRequest) -> LabResult<()> {
    if request.clear_select && request.select.is_some() {
        return Err(invalid(
            "validation_failed",
            "give one of select and clear_select",
        ));
    }
    let Some(select) = &request.select else {
        return Ok(());
    };
    let step = request.step;
    let refuse = |problem: SelectProblem, message: &str| {
        select_invalid(step, problem, format!("{message}; nothing was written"))
    };
    if request.application.is_some() {
        return Err(refuse(
            SelectProblem::WithApplication,
            "the request declares a select and an application operation: one step has one effect",
        ));
    }
    if let Some(click) = &request.click {
        if click.follow.is_some() {
            return Err(refuse(
                SelectProblem::WithFollow,
                "the request declares a select and a click that follows a mark: the select's click \
                 is the slot it chooses",
            ));
        }
        if click_gesture(click) {
            return Err(refuse(
                SelectProblem::WithGesture,
                "the request declares a select and a drag or a long press: a select taps once",
            ));
        }
        return Err(refuse(
            SelectProblem::WithClick,
            "the request declares a select and a click: the select's click is the slot it \
             chooses; leave click out",
        ));
    }
    if request.retry.is_some() {
        return Err(refuse(
            SelectProblem::WithRetry,
            "the request declares a select and a click retry: a select step declares no retry",
        ));
    }
    if request.optional == Some(true) {
        return Err(refuse(
            SelectProblem::OnOptionalStep,
            "the request declares a select on an optional step: a select step's page is its list",
        ));
    }
    validate_select_shape(step, select)
}

/// The shape of a select (see [`validate_select_request`]).
fn validate_select_shape(step: Option<u32>, select: &SelectSpec) -> LabResult<()> {
    let shape = |message: String| invalid("validation_failed", format!("select {message}"));
    let refuse = |problem: SelectProblem, message: String| {
        Err(select_invalid(
            step,
            problem,
            format!("{message}; nothing was written"),
        ))
    };
    let slots = super::model::LAB_RECORDING_MAX_SELECT_SLOTS;
    // #463 S2-9a: a scrolling select declares rows and a scan in place of slots.
    if select.rows.is_some() || select.scan.is_some() {
        super::scan::validate_scan_shape(step, select)?;
    } else if !(1..=slots).contains(&select.slots.len()) {
        return Err(shape(format!("needs 1..={slots} slots")));
    }
    for (index, slot) in select.slots.iter().enumerate() {
        for rect in [slot.rect, slot.click_rect(), slot.name] {
            if rect.width < 1 || rect.height < 1 {
                return Err(shape(format!(
                    "slot {index}: a rectangle needs width and height >= 1"
                )));
            }
        }
    }
    let read = &select.read;
    let sha_valid = read.model_sha256.len() == 64
        && read
            .model_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if read.languages.is_empty()
        || read
            .languages
            .iter()
            .any(|language| language.trim().is_empty())
        || read.timeout_ms == 0
        || !(1..=1000).contains(&read.minimum_confidence_milli)
        || read.model_ref.trim().is_empty()
        || !sha_valid
    {
        return Err(shape(
            "read needs non-empty languages, timeout_ms > 0, minimum_confidence_milli in \
             1..=1000, a model_ref and a lowercase 64-hex model_sha256"
                .to_string(),
        ));
    }
    // #463 SEL-2L-a: `blocks` and `line_merge` come together or not at all, as `rows` and `scan`
    // do: a name read by detecting its blocks says how their lines are merged.
    let missing = match (&read.blocks, &read.line_merge) {
        (Some(_), None) => Some("blocks without line_merge"),
        (None, Some(_)) => Some("line_merge without blocks"),
        _ => None,
    };
    if let Some(missing) = missing {
        return Err(shape(format!(
            "read declares {missing}: names read by detecting their blocks need both, \
             \"blocks\": \"detect\" and \"line_merge\" ({{\"gap_factor\": n}} or \"none\"); give \
             both or neither"
        )));
    }
    // #463 SEL-GRID G2: the lines of a name box are those of the select's shared read, which only
    // a read by detecting blocks makes.
    if read.lines.is_some() && read.blocks.is_none() {
        return Err(shape(
            "read declares lines without blocks: the lines of a name box are those of the \
             select's shared read; give \"blocks\": \"detect\" and \"line_merge\" with them"
                .to_string(),
        ));
    }
    let mut ids = BTreeSet::new();
    for name in &select.names {
        if !name_id_valid(&name.id) {
            return refuse(
                SelectProblem::NameIdInvalid,
                format!(
                    "the name id '{}' must match ^[a-z][a-z0-9_]{{0,63}}$",
                    name.id
                ),
            );
        }
        if !ids.insert(name.id.as_str()) {
            return refuse(
                SelectProblem::NameDuplicate,
                format!("the name id '{}' is given twice", name.id),
            );
        }
    }
    // The identity domain's rules as the kernel's layout declares them: 1..=1024 names, 1..=8
    // aliases each that normalize to a text, and the match's distance and margin.
    identity(select)
        .validate()
        .map_err(|message| shape(format!("names or match: {message}")))?;
    if !name_id_valid(&select.wanted.name) {
        return refuse(
            SelectProblem::WantedNameInvalid,
            format!(
                "the wanted name '{}' must match ^[a-z][a-z0-9_]{{0,63}}$",
                select.wanted.name
            ),
        );
    }
    if !ids.contains(select.wanted.default.as_str()) {
        return refuse(
            SelectProblem::DefaultNotInNames,
            format!(
                "the wanted default '{}' is no id of the select's names",
                select.wanted.default
            ),
        );
    }
    if let Some(pressed) = &select.pressed
        && !ids.contains(pressed.as_str())
    {
        return refuse(
            SelectProblem::PressedNotInNames,
            format!("the pressed name '{pressed}' is no id of the select's names"),
        );
    }
    if let Some(key) = &select.on_empty_key
        && !on_empty_key_valid(key)
    {
        return refuse(
            SelectProblem::OnEmptyKeyInvalid,
            format!("the on_empty key '{key}' must match ^[a-z0-9][a-z0-9_]{{0,127}}$"),
        );
    }
    Ok(())
}

/// The request's select becomes the step's effect (before its click guard is applied): the
/// step's click is the select's (`source: "select"`) and its select the declaration, its
/// self-test to be made. A step whose click has an outcome on record keeps its press, the
/// demonstration the self-test checks, whatever declared it (a select declared again after its
/// press, a tap recorded before the select); a gesture, a followed mark or a retry of such a click
/// stays in the way (`record_select_invalid`). A click without an outcome, the select's own
/// among them, is replaced only with `replace_click` (`record_click_exists` otherwise, so that a
/// one-call step sent again presses nothing twice). An application operation of the step stays in
/// the way too. Fix 1: an executed click that was not the select's own keeps its rectangle,
/// source and mark in the select (`click_before`, carried over when the select is declared
/// again), which `clear_select` gives back. Returns whether the step had no press to keep a
/// click box from: without an OCR model such a select cannot find its click.
pub(crate) fn declare_select(step: &mut RecordingStep, request: &MarkRequest) -> LabResult<bool> {
    let Some(spec) = &request.select else {
        return Ok(false);
    };
    let carried = step
        .select
        .as_ref()
        .and_then(|select| select.click_before.clone());
    let index = step.index;
    let refuse =
        |problem: SelectProblem, message: String| select_invalid(Some(index), problem, message);
    if step.application.is_some() {
        return Err(refuse(
            SelectProblem::WithApplication,
            format!(
                "step {index} has an application operation; one step has one effect: a select \
                 needs a step of its own"
            ),
        ));
    }
    let now = now_unix_ms();
    let declared = |attempts| StepClick {
        rect: spec.slots.first().map_or(
            RecordRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            |slot| slot.click_rect(),
        ),
        source: SELECT_SOURCE.to_string(),
        from: None,
        follow: None,
        gesture: None,
        retry: None,
        declared_at_unix_ms: now,
        execution: None,
        attempts,
        needs_review: false,
    };
    let (click, fresh, click_before) = match step.click.take() {
        Some(click) if click.execution.is_some() => {
            let problem = if click.gesture.is_some() {
                Some((
                    SelectProblem::WithGesture,
                    format!("step {index} recorded a drag or a long press; a select taps once"),
                ))
            } else if click.follow.is_some() {
                Some((
                    SelectProblem::WithFollow,
                    format!(
                        "step {index}'s recorded click follows a mark; reopen the step and declare \
                         the select before its press"
                    ),
                ))
            } else if click.retry.is_some() {
                Some((
                    SelectProblem::WithRetry,
                    format!("step {index}'s click declares a retry; a select step declares none"),
                ))
            } else {
                None
            };
            if let Some((problem, message)) = problem {
                step.click = Some(click);
                return Err(refuse(problem, message));
            }
            // Fix 1: the select's own press keeps what an earlier declaration kept; any other
            // press keeps its own rectangle, source and mark.
            let before = if click.source == SELECT_SOURCE {
                carried
            } else {
                Some(SelectClickBefore {
                    rect: click.rect,
                    source: click.source.clone(),
                    from: click.from.clone(),
                })
            };
            (
                StepClick {
                    source: SELECT_SOURCE.to_string(),
                    from: None,
                    ..click
                },
                false,
                before,
            )
        }
        Some(click) if !request.replace_click => {
            step.click = Some(click);
            return Err(with_details(
                blocked(
                    "record_click_exists",
                    format!(
                        "step {index} already declares a click without an outcome; add \
                         --replace-click (replace_click) to declare the select in its place"
                    ),
                ),
                json!({"step": index}),
            ));
        }
        Some(click) => (declared(click.attempts), true, None),
        None => (declared(Vec::new()), true, None),
    };
    step.click = Some(click);
    step.select = Some(StepSelect {
        declaration: spec.clone(),
        marked_at_unix_ms: now,
        self_test: SelectSelfTest {
            status: SelfTestStatus::NotEvaluated,
            reason: None,
            frames: Vec::new(),
        },
        click_before,
        scan: None,
    });
    Ok(fresh)
}

/// Fix 1: a request's `clear_select` takes the step's select off, also on a stopped recording.
/// An executed click the select took over gets back its own rectangle, source and mark
/// (`click_before`), so the step is the one recorded before the select; the select's own click
/// without an outcome goes with it, with its click guard, and the step has no effect again. The
/// select's own press, executed, is no click of its own to give back: `record_click_executed`
/// (reopen the step first). #463 R2: a step without a select has nothing to take off, and is
/// refused with `no_select`, so that the answer never says something was taken off.
pub(crate) fn clear_select(step: &mut RecordingStep) -> LabResult<()> {
    let index = step.index;
    let Some(select) = &step.select else {
        return Err(select_invalid(
            Some(index),
            SelectProblem::NoSelect,
            format!(
                "step {index} has no select, so clear_select has nothing to take off; nothing was \
                 written"
            ),
        ));
    };
    let executed = step
        .click
        .as_ref()
        .is_some_and(|click| click.execution.is_some());
    if select.click_before.is_none() && executed {
        return Err(with_details(
            blocked(
                "record_click_executed",
                format!(
                    "step {index}'s press was the select's own, so there is no click of its own to \
                     give back; run `record mark --reopen-step {index}` first, or declare the \
                     select again"
                ),
            ),
            json!({"step": index}),
        ));
    }
    match step.select.take().and_then(|select| select.click_before) {
        Some(before) => {
            if let Some(click) = step.click.as_mut() {
                click.rect = before.rect;
                click.source = before.source;
                click.from = before.from;
            }
        }
        None => {
            step.click = None;
            step.click_guard = None;
        }
    }
    Ok(())
}

/// The rules a select step keeps after every `record mark` of it, whatever the command changed:
/// it is not optional, has no application operation, and its click declares no retry, gesture or
/// follow. #463 S2-6: a step marked as holding a choice keeps them too, with the same words, since
/// its select refines it.
pub(crate) fn check_select_rules(step: &RecordingStep) -> LabResult<()> {
    let what = match (&step.select, &step.choice) {
        (Some(_), _) => "selects in a list",
        (None, Some(_)) => "holds a choice",
        (None, None) => return Ok(()),
    };
    let index = step.index;
    let refuse = |problem: SelectProblem, message: String| {
        Err(select_invalid(Some(index), problem, message))
    };
    if step.optional.is_some() {
        return refuse(
            SelectProblem::OnOptionalStep,
            format!("step {index} {what} and cannot be optional: its page is its list"),
        );
    }
    if step.application.is_some() {
        return refuse(
            SelectProblem::WithApplication,
            format!("step {index} {what}; one step has one effect"),
        );
    }
    if let Some(click) = &step.click {
        if click.retry.is_some() {
            return refuse(
                SelectProblem::WithRetry,
                format!("step {index} {what} and declares no retry"),
            );
        }
        if click.gesture.is_some() {
            return refuse(
                SelectProblem::WithGesture,
                format!("step {index} {what} and taps once"),
            );
        }
        if click.follow.is_some() {
            return refuse(
                SelectProblem::WithFollow,
                format!("step {index} {what}; its click is the slot it chooses"),
            );
        }
    }
    Ok(())
}

/// #463 S2-6 (`contracts/lab-recording.md`, "Marking a choice first"): the request's `choice` on
/// `step`: `true` marks it (a mark already there keeps its time), `false` takes the mark off,
/// absent leaves it. Fix round 1: `false` on a step without the mark has nothing to take off and
/// is refused (`no_choice`), as `clear_select` on a step without a select is (`no_select`); on a
/// refined step it takes off the mark alone, the select staying, and the package is the same. A
/// step marked so on the last step of a stopped recording (`final_of_stopped`) is refused as a
/// select there is (`on_final_step`): the package ends on that page, which only recognizes. The
/// rules the mark keeps beside the rest of the step are [`check_select_rules`]'s.
pub(crate) fn apply_choice(
    step: &mut RecordingStep,
    request: &MarkRequest,
    final_of_stopped: bool,
) -> LabResult<()> {
    let index = step.index;
    match request.choice {
        None => {}
        Some(false) if step.choice.is_none() => {
            return Err(select_invalid(
                Some(index),
                SelectProblem::NoChoice,
                format!(
                    "step {index} is not marked as holding a choice, so choice false has nothing \
                     to take off; nothing was written"
                ),
            ));
        }
        Some(false) => step.choice = None,
        Some(true) => {
            step.choice.get_or_insert_with(|| StepChoice {
                marked_at_unix_ms: now_unix_ms(),
            });
        }
    }
    if step.choice.is_some() && final_of_stopped {
        return Err(select_invalid(
            Some(index),
            SelectProblem::OnFinalStep,
            format!(
                "step {index} is the last step of the stopped recording: the package ends on its \
                 page, and the last step only recognizes; a choice stands on a step with a next \
                 screen; nothing was written"
            ),
        ));
    }
    Ok(())
}

/// What the self-test of a step's select leaves: the select with its self-test, the click box
/// the primary frame gives the pressed name (`None` without an OCR model, the click kept), and
/// the refusals of the batch.
pub(crate) struct SelectChecked {
    pub(crate) select: StepSelect,
    pub(crate) click_rect: Option<RecordRect>,
    pub(crate) rejections: Vec<MarkRejection>,
}

/// One live frame of a select step: its record, and the frame as loaded.
pub(crate) struct SelectFrame<'a> {
    pub(crate) entry: &'a RecordedFrame,
    pub(crate) loaded: &'a LoadedFrame,
}

/// The step and everything its probe package needs: the step as this command leaves it (its own
/// marks, reused and forbidden marks, anchor, click and select), every mark its page names (its
/// own, reused and forbidden marks and their check members) and the template crops of those.
pub(crate) struct SelectProbe {
    pub(crate) step: RecordingStep,
    pub(crate) marks: Vec<RecordedMark>,
    pub(crate) crops: BTreeMap<String, Vec<u8>>,
}

/// The self-test of the select of `probe.step` on its live `frames`: the rules that need the
/// recording (`slot_outside_frame`, `on_final_step`, `on_empty_key_reserved`,
/// `on_empty_key_taken`, `wanted_name_taken`, #463 R2 fix 1: `on_empty_key_of_ending`), then the
/// probe package admitted by the kernel (`guard_missing`, `kernel_refused`), then, with an OCR
/// model, the kernel's dry run on every frame, each frame's decisions reported by their kind.
/// `fresh` says the step had no press to keep a click box from (see [`declare_select`]); without an
/// OCR model such a select is refused `lab_ocr_unavailable`, every other one is `not_evaluated`.
pub(crate) fn check_step_select(
    recording: &LabRecording,
    stopped: bool,
    probe: SelectProbe,
    frames: &[SelectFrame<'_>],
    fresh: bool,
    ocr: &LabOcr,
) -> LabResult<Option<SelectChecked>> {
    let Some(current) = probe.step.select.clone() else {
        return Ok(None);
    };
    let index = probe.step.index;
    let spec = &current.declaration;
    let refuse = |problem: SelectProblem, message: String| {
        Err(select_invalid(
            Some(index),
            problem,
            format!("{message}; nothing was written"),
        ))
    };
    let size = recording.coordinate_space.ok_or_else(|| {
        blocked(
            "record_step_frame_missing",
            "the recording has no frame, so it has no coordinate space",
        )
    })?;
    // A scrolling select has no slots; its regions are checked with its screens
    // (`scan::check_scan_select`).
    for (slot, declared) in spec.slots.iter().enumerate() {
        if [declared.rect, declared.click_rect(), declared.name]
            .into_iter()
            .any(|rect| !rect_inside(rect, size))
        {
            return refuse(
                SelectProblem::SlotOutsideFrame,
                format!(
                    "slot {slot} of the select of step {index} has a rectangle, click box or name \
                     region outside the {}x{} frame",
                    size.width, size.height
                ),
            );
        }
    }
    if stopped && super::steps::last_effective(recording) == Some(index) {
        return refuse(
            SelectProblem::OnFinalStep,
            format!(
                "step {index} is the last step of the stopped recording: the package ends on its \
                 page, and the last step only recognizes; a select stands on a step with a next \
                 screen"
            ),
        );
    }
    // #463 S2-9b: a select that declares no `on_empty` has no key to keep these rules.
    let key = empty_end_key(&recording.task_id, spec);
    if let Some(key) = &key
        && *key == done_key(&recording.task_id)
    {
        return refuse(
            SelectProblem::OnEmptyKeyReserved,
            format!("the on_empty key '{key}' is the usual ending's key"),
        );
    }
    for other in recording
        .steps
        .iter()
        .filter(|other| other.is_effective() && other.index != index)
    {
        let Some(other_select) = &other.select else {
            continue;
        };
        if let Some(key) = &key
            && empty_end_key(&recording.task_id, &other_select.declaration).as_ref() == Some(key)
        {
            return refuse(
                SelectProblem::OnEmptyKeyTaken,
                format!(
                    "the on_empty key '{key}' is also the key of step {}'s select",
                    other.index
                ),
            );
        }
        if other_select.declaration.wanted.name == spec.wanted.name {
            return refuse(
                SelectProblem::WantedNameTaken,
                format!(
                    "the wanted name '{}' is also the wanted name of step {}'s select",
                    spec.wanted.name, other.index
                ),
            );
        }
    }
    // #463 R2 fix 1: the key is no ending's, also for a select declared after the ending; the
    // ending's own check at record stop (`key_of_select`) would refuse it there otherwise.
    if let Some(key) = &key
        && recording
            .endings
            .iter()
            .any(|ending| ending.outcome_key == *key)
    {
        return refuse(
            SelectProblem::OnEmptyKeyOfEnding,
            format!(
                "the on_empty key '{key}' is the outcome key of an ending of the recording; give \
                 the select an on_empty_key of its own"
            ),
        );
    }

    // #463 S2-9a: a scrolling select reads its list over the screens of the demonstration folded
    // into the step, and is self-tested there (`scan::check_scan_select`).
    if spec.rows.is_some() {
        return super::scan::check_scan_select(recording, probe, frames, ocr, current).map(Some);
    }

    // The probe package: the step as `record stop` generates it, admitted by the kernel.
    let plan = select_probe_plan(recording, &probe.step, probe.marks)?;
    let entries = render(&plan, None, &probe.crops)?;
    let prepared = PreparedContainedTask::load_content_entries(
        PROBE_INSTANCE,
        entries,
        ocr.provider(),
        Instant::now() + PROBE_BUDGET,
    )
    .map_err(|error| {
        let mut refused = select_invalid(
            Some(index),
            SelectProblem::KernelRefused,
            format!(
                "the kernel's admission refuses the select of step {index} as record stop would \
                 generate it: {error}; nothing was written"
            ),
        );
        if let Some(details) = refused.details.as_mut().and_then(Value::as_object_mut) {
            details.insert("loader_code".to_string(), json!(error.code().as_str()));
            details.insert("detail".to_string(), json!(error.detail()));
        }
        refused
    })?;
    let operation = plan.operation_id(&plan.steps[0]);
    let override_document = (spec.pressed_name() != spec.wanted.default)
        .then(|| select_policy_document(&plan, &plan.steps[0], spec, spec.pressed_name()));

    if ocr.ocr_provider().is_none() {
        if fresh {
            return Err(ocr.unavailable_error(None).unwrap_or_else(|| {
                blocked(
                    LabCode::LabOcrUnavailable.as_str(),
                    format!(
                        "the select of step {index} finds the slot of its pressed name with the \
                         OCR model, and no click can be recorded without it"
                    ),
                )
            }));
        }
        return Ok(Some(SelectChecked {
            select: StepSelect {
                self_test: SelectSelfTest {
                    status: SelfTestStatus::NotEvaluated,
                    reason: Some(LabCode::LabOcrUnavailable.as_str().to_string()),
                    frames: Vec::new(),
                },
                ..current
            },
            click_rect: None,
            rejections: Vec::new(),
        }));
    }

    let executed = probe
        .step
        .click
        .as_ref()
        .filter(|click| click.executed())
        .and_then(|click| click.execution.as_ref())
        .map(|execution| execution.point);
    let instant = recording.record_started_at_unix_ms;
    let mut decisions = Vec::new();
    let mut rejections = Vec::new();
    let mut click_rect = None;
    for frame in frames {
        let frame_id = frame.entry.frame_id.as_str();
        let primary = frame.entry.role == "primary";
        let default = match decide(&prepared, &operation, &frame.loaded.frame, None, instant)? {
            Ok(decided) => decided,
            Err(failure) => {
                rejections.push(MarkRejection::new(
                    SELECT_REJECTION_ID,
                    SELF_TEST_ERROR,
                    json!({"frame_id": frame_id, "failure": failure}),
                ));
                continue;
            }
        };
        // Fix 1: the refusal names the default's decision on the frame too (`blocked`: the run
        // would fail there).
        if let Some((slot, cause)) = default.unread.first() {
            // #463 SEL-GRID G2: with what the refused slot's read saw.
            rejections.push(MarkRejection::new(
                SELECT_REJECTION_ID,
                NAME_UNREAD,
                default.with_blocks(
                    json!({
                        "frame_id": frame_id,
                        "slot": slot,
                        "cause": cause,
                        "unread": default
                            .unread
                            .iter()
                            .map(|(slot, cause)| json!({"slot": slot, "cause": cause}))
                            .collect::<Vec<_>>(),
                        "default": default.kind
                    }),
                    *slot,
                ),
            ));
            continue;
        }
        let pressed = match &override_document {
            None => default.kind,
            Some(document) => {
                match decide(
                    &prepared,
                    &operation,
                    &frame.loaded.frame,
                    Some(document),
                    instant,
                )? {
                    Ok(decided) => decided.kind,
                    Err(failure) => {
                        rejections.push(MarkRejection::new(
                            SELECT_REJECTION_ID,
                            SELF_TEST_ERROR,
                            json!({"frame_id": frame_id, "failure": failure}),
                        ));
                        continue;
                    }
                }
            }
        };
        if primary {
            let Some(slot) = pressed.slot() else {
                rejections.push(MarkRejection::new(
                    SELECT_REJECTION_ID,
                    PRESSED_ABSENT,
                    json!({
                        "frame_id": frame_id,
                        "pressed": spec.pressed_name(),
                        "decision": pressed
                    }),
                ));
                continue;
            };
            let rect = default.clicks.get(&slot).copied().ok_or_else(|| {
                invalid(
                    "validation_failed",
                    format!("the dry run of step {index} selected slot {slot}, which it lacks"),
                )
            })?;
            if let Some(point) = executed
                && !point_in(point, rect)
            {
                rejections.push(MarkRejection::new(
                    SELECT_REJECTION_ID,
                    PRESS_MISMATCH,
                    json!({
                        "frame_id": frame_id,
                        "pressed": spec.pressed_name(),
                        "slot": slot,
                        "click_rect": rect,
                        "point": point
                    }),
                ));
                continue;
            }
            click_rect = Some(rect);
        }
        decisions.push(SelectFrameDecision {
            frame_id: frame_id.to_string(),
            pressed,
            default: default.kind,
        });
    }
    if rejections.is_empty() && click_rect.is_none() {
        return Err(invalid(
            "validation_failed",
            format!("step {index} has no live primary frame for its select"),
        ));
    }
    Ok(Some(SelectChecked {
        select: StepSelect {
            self_test: SelectSelfTest {
                status: SelfTestStatus::Passed,
                reason: None,
                frames: decisions,
            },
            ..current
        },
        click_rect,
        rejections,
    }))
}

fn point_in(point: RecordPoint, rect: RecordRect) -> bool {
    i64::from(point.x) >= i64::from(rect.x)
        && i64::from(point.y) >= i64::from(rect.y)
        && i64::from(point.x) < i64::from(rect.x) + i64::from(rect.width)
        && i64::from(point.y) < i64::from(rect.y) + i64::from(rect.height)
}

/// One decision of the kernel's dry run of a select on one frame: its kind (fix 1: `selected`
/// with its slot, `empty`, or the kind of a decision the run fails on), each slot's click box, and
/// the slots whose name read no known name, each with its typed cause.
pub(crate) struct Decided {
    pub(crate) kind: SelectDecision,
    pub(crate) clicks: BTreeMap<u32, RecordRect>,
    pub(crate) unread: Vec<(u32, Value)>,
    /// #463 S2-9a: every instance the projection holds, as a scrolling select reads its rows.
    pub(crate) instances: Vec<DecidedInstance>,
    /// #463 SEL-GRID G2: what each actionable instance's name read saw (the kernel's dry run,
    /// `SelectionDryRun::name_reads`), by its index.
    pub(crate) reads: BTreeMap<u32, CandidateNameRead>,
}

impl Decided {
    /// #463 SEL-GRID G2 (`contracts/lab-recording.md`, the refusal `select_name_unread`): the
    /// refusal `detail` of the name of the instance `index` with what its read saw, so that an
    /// author sets the name box, the lines and the chain from it: `blocks`, the blocks its name box
    /// held, each with its `rect`, `height` and `text` and a cut block with its `strips` (each its
    /// `rect`, `height` and `text`), and, when there are any, `below_floor`, the blocks lower than
    /// half a line that were not read, each with its `rect` and `height`. A detail that is no
    /// object, or an instance whose name was not read from OCR, is left as it is.
    pub(crate) fn with_blocks(&self, mut detail: Value, index: u32) -> Value {
        let Some(read) = self.reads.get(&index) else {
            return detail;
        };
        if let Some(object) = detail.as_object_mut() {
            insert_blocks(object, read);
        }
        detail
    }
}

/// #463 SEL-GRID G2: the members [`Decided::with_blocks`] adds for `read`.
fn insert_blocks(object: &mut serde_json::Map<String, Value>, read: &CandidateNameRead) {
    let block_json = |block: &CandidateNameBlock| {
        let mut value = json!({
            "rect": block.rect,
            "height": block.rect.height,
            "text": block.text
        });
        if !block.strips.is_empty() {
            value["strips"] = Value::Array(
                block
                    .strips
                    .iter()
                    .map(|strip| {
                        json!({
                            "rect": strip.rect,
                            "height": strip.rect.height,
                            "text": strip.text
                        })
                    })
                    .collect(),
            );
        }
        value
    };
    object.insert(
        "blocks".to_string(),
        Value::Array(read.blocks.iter().map(block_json).collect()),
    );
    if !read.below_floor.is_empty() {
        object.insert(
            "below_floor".to_string(),
            Value::Array(
                read.below_floor
                    .iter()
                    .map(|rect| json!({"rect": rect, "height": rect.height}))
                    .collect(),
            ),
        );
    }
}

/// #463 S2-9a: one instance of a dry run's projection: its index, whether it is actionable (its
/// rectangle and click box inside the readable band; a row cut by the list's edge is not), its
/// rectangle, the name its identity feature read and, when it read none, why.
pub(crate) struct DecidedInstance {
    pub(crate) index: u32,
    pub(crate) actionable: bool,
    pub(crate) rect: RecordRect,
    pub(crate) name: Option<String>,
    pub(crate) unread: Option<Value>,
}

/// The kernel's dry run of the select operation `operation` of `prepared` on `frame`
/// (`dry_run_select`, called, never copied), with the package's sealed policy document or with
/// `override_document`, an empty fact snapshot and the instant `now_unix_ms`. #463 S2-6: the run
/// reads what a run without a dispatch reads, every variable the package declares at its default
/// (`default_run_values`; a package that declares none reads its parameters alone, as before), so
/// the policy of every select step (fix round 1) reads its wanted value's default; the overlay is
/// observed at `now_unix_ms`, so the instant decides nothing. A refusal of the dry run (the frame
/// shows another page, a name read fails, …) is its outcome, with the kernel's code and detail.
pub(crate) fn decide(
    prepared: &PreparedContainedTask,
    operation: &str,
    frame: &Frame,
    override_document: Option<&Value>,
    now_unix_ms: u64,
) -> LabResult<Result<Decided, Value>> {
    let snapshot = InstanceFactSnapshot {
        snapshot_id: "lab.record.select".to_string(),
        ledger_position: 0,
        context: InstanceFactContext {
            instance_id: PROBE_INSTANCE.to_string(),
            server_id: String::new(),
            game_id: String::new(),
        },
        records: Vec::new(),
    };
    let parameters = prepared.default_run_values();
    let run = match override_document {
        None => dry_run_select(
            prepared,
            operation,
            frame,
            (&snapshot, now_unix_ms),
            &parameters,
            None,
        ),
        Some(document) => {
            let policy = serde_json::from_value(document.clone()).map_err(|error| {
                invalid(
                    "validation_failed",
                    format!("the select's policy document does not decode: {error}"),
                )
            })?;
            dry_run_select(
                prepared,
                operation,
                frame,
                (&snapshot, now_unix_ms),
                &parameters,
                Some(&policy),
            )
        }
    };
    let dry = match run {
        Ok(dry) => dry,
        Err(error) => {
            return Ok(Err(json!({
                "code": error.code().as_str(),
                "detail": error.detail()
            })));
        }
    };
    let candidates = dry.candidate_projection.candidates();
    // #463 SEL-GRID G2: what each instance's name read saw, the first read of its name.
    let mut reads = BTreeMap::new();
    for read in &dry.name_reads {
        if read.feature == SELECT_FEATURE {
            reads
                .entry(read.instance_index)
                .or_insert_with(|| read.clone());
        }
    }
    let cause_of = |candidate: &ProjectedCandidate| unread_cause(candidate, &reads);
    let unread = candidates
        .iter()
        .filter_map(|candidate| cause_of(candidate).map(|cause| (candidate.instance_index, cause)))
        .collect::<Vec<_>>();
    // Fix 1: the decision by its kind, as the run takes it: an empty or insufficient decision
    // while a slot reads no known name is blocked (S0 §3.2 rule 2, which the run applies). #463
    // S2-9a fix round 1: counted over the actionable candidates, the ones the kernel evaluates and
    // counts unknown (a row the list's edge cuts reads no name and blocks nothing; a fixed slot is
    // always actionable).
    let blocking = candidates
        .iter()
        .any(|candidate| candidate.actionable && cause_of(candidate).is_some());
    let kind = match mirrored_outcome(&dry.decision.outcome)? {
        TaskSelectionOutcome::Selected { .. } => {
            let [id] = dry.decision.selected.as_slice() else {
                return Err(invalid(
                    "validation_failed",
                    format!(
                        "the dry run selected {} candidates of one list",
                        dry.decision.selected.len()
                    ),
                ));
            };
            let slot = candidates
                .iter()
                .find(|candidate| candidate.id == *id)
                .map(|candidate| candidate.instance_index)
                .ok_or_else(|| {
                    invalid(
                        "validation_failed",
                        format!("the dry run selected '{id}', no candidate of its projection"),
                    )
                })?;
            SelectDecision::Selected { slot }
        }
        TaskSelectionOutcome::Empty | TaskSelectionOutcome::Insufficient { .. } if blocking => {
            SelectDecision::Blocked
        }
        TaskSelectionOutcome::Empty => SelectDecision::Empty,
        TaskSelectionOutcome::Insufficient { .. } => SelectDecision::Insufficient,
        TaskSelectionOutcome::Ambiguous { .. } => SelectDecision::Ambiguous,
        TaskSelectionOutcome::Unknown { .. } => SelectDecision::Unknown,
    };
    let instances = candidates
        .iter()
        .map(|candidate| DecidedInstance {
            index: candidate.instance_index,
            actionable: candidate.actionable,
            rect: RecordRect {
                x: candidate.rect.x,
                y: candidate.rect.y,
                width: candidate.rect.width,
                height: candidate.rect.height,
            },
            name: match candidate.features.get(SELECT_FEATURE) {
                Some(CandidateFeature::Identity { value, .. }) => Some(value.clone()),
                _ => None,
            },
            unread: cause_of(candidate),
        })
        .collect();
    Ok(Ok(Decided {
        kind,
        clicks: candidates
            .iter()
            .map(|candidate| (candidate.instance_index, click_of(candidate)))
            .collect(),
        unread,
        instances,
        reads,
    }))
}

/// Fix 1: the kernel's decision outcome as the contract mirrors it (`TaskSelectionOutcome`,
/// through their shared serde shape, as the run's record takes it): the evaluator's `detail`
/// text of an unknown outcome stays out, as the record leaves it out.
pub(crate) fn mirrored_outcome(outcome: &impl Serialize) -> LabResult<TaskSelectionOutcome> {
    let mut value = serde_json::to_value(outcome).map_err(|error| {
        invalid(
            "validation_failed",
            format!("the dry run's outcome does not encode: {error}"),
        )
    })?;
    if let Some(object) = value.as_object_mut() {
        object.remove("detail");
    }
    serde_json::from_value(value).map_err(|error| {
        invalid(
            "validation_failed",
            format!("the dry run's outcome is no outcome the contract mirrors: {error}"),
        )
    })
}

fn click_of(candidate: &ProjectedCandidate) -> RecordRect {
    RecordRect {
        x: candidate.click.x,
        y: candidate.click.y,
        width: candidate.click.width,
        height: candidate.click.height,
    }
}

/// Why a slot's name read no known name: the identity feature's typed reason, or `missing` when
/// the slot has no identity feature at all. #463 SEL-GRID G2: a `low_confidence` whose read passed
/// the confidence and matched no alias within `max_distance` is `beyond_distance`, the cause the
/// kernel's mapping gives that read (`CandidateIdentityDeclaration::map_ocr_with_cause`, called on
/// the read in the dry run, `reads`), where the feature keeps its reason.
fn unread_cause(
    candidate: &ProjectedCandidate,
    reads: &BTreeMap<u32, CandidateNameRead>,
) -> Option<Value> {
    match candidate.features.get(SELECT_FEATURE) {
        Some(CandidateFeature::Identity { .. }) => None,
        Some(CandidateFeature::Unknown { reason, .. }) => {
            let beyond = *reason == CandidateUnknownReason::LowConfidence
                && reads
                    .get(&candidate.instance_index)
                    .is_some_and(|read| read.unread == Some(CandidateUnreadCause::BeyondDistance));
            Some(if beyond {
                json!(CandidateUnreadCause::BeyondDistance)
            } else {
                json!(reason)
            })
        }
        Some(_) | None => Some(json!("missing")),
    }
}

/// The marks the probe of a select step needs besides the step's own: each reused and forbidden
/// mark found by `find`, and the members of every check among them all.
pub(crate) fn probe_marks(
    own: &[RecordedMark],
    reused: &[RecordedMark],
    forbidden: &[String],
    find: impl Fn(&str) -> Option<RecordedMark>,
) -> LabResult<Vec<RecordedMark>> {
    let mut marks = own.iter().chain(reused).cloned().collect::<Vec<_>>();
    for id in forbidden {
        let mark = find(id).ok_or_else(|| {
            invalid(
                "validation_failed",
                format!("the forbidden mark '{id}' is not a mark of an effective step"),
            )
        })?;
        marks.push(mark);
    }
    let mut members = Vec::new();
    for mark in &marks {
        for member in mark.check_members() {
            if !marks.iter().any(|known| &known.id == member)
                && !members
                    .iter()
                    .any(|known: &RecordedMark| &known.id == member)
            {
                let found = find(member).ok_or_else(|| {
                    invalid(
                        "validation_failed",
                        format!("the check '{}' names '{member}', no step mark", mark.id),
                    )
                })?;
                members.push(found);
            }
        }
    }
    marks.extend(members);
    Ok(marks)
}

/// #463 R1: a mark id the generated select targets take: the prefix `select/` is reserved, as
/// `page/` is for anchors.
pub(crate) const SELECT_TARGET_PREFIX: &str = "select/";

/// The id of a select step's entry in `record stop`'s `marks_not_evaluated`, `select/<page>`.
pub(crate) fn not_evaluated_id(page: &str) -> String {
    format!("{SELECT_TARGET_PREFIX}{page}")
}
