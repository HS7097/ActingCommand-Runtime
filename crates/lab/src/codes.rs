// SPDX-License-Identifier: AGPL-3.0-only

//! The Lab crate's outcome codes and locations (Workflow #378). Every entry is listed in the
//! fragment `contracts/outcome-codes/lab.json`; the outcome guard (G1) checks both directions.
//! The crate's other result channels stay strings until B4 converts the Lab sites.

actingcommand_contract::outcome_codes! {
    /// Workflow #396 LAB-3 (FCM T-02): why the offline tools cannot evaluate OCR.
    pub enum LabCode {
        /// Error. An OCR evaluation of actinglab or the Lab cannot run because no usable OCR
        /// model was loaded (optional keys `target_id`, `path`: the vision root). Its cause
        /// says why.
        LabOcrUnavailable => "lab_ocr_unavailable": error / none,
        /// Error link. The offline tool has no Runtime configuration with a `vision` section:
        /// it does not run from an installation or a single-location root, or the selected
        /// configuration has none.
        LabOcrVisionUnconfigured => "lab_ocr_vision_unconfigured": error / none,
        /// Error link. The vision root's models folder holds no OCR model folder (key `path`:
        /// the models folder).
        LabOcrModelMissing => "lab_ocr_model_missing": error / none,
        /// Error link. The vision root has no ONNX Runtime library (key `path`: the library the
        /// Runtime would load).
        LabOcrRuntimeMissing => "lab_ocr_runtime_missing": error / none,
        /// Error. Workflow #396 REC-4: a click that follows a template mark (#463 L2: or a
        /// located-text mark) does not hold (keys `mark_id`: the followed mark,
        /// `follow_problem`: why; `stage`). A declaration
        /// that cannot hold refuses `record mark` with it; a frame on which it does not hold
        /// is its reason inside `record_mark_rejected`.
        RecordClickFollowInvalid => "record_click_follow_invalid": error / none,
        /// Error. #411 Lab BD: `do --capture --record` was asked to execute a recording step
        /// whose declared gesture a Lab operation cannot press, a single-touch drag with a
        /// vertical brake; nothing was captured, pressed or recorded (exit 3; actinglab details
        /// `step`, `gesture`).
        RecordGestureExecutionUnsupported => "record_gesture_execution_unsupported": error / none,
        /// Error. Workflow #449 REC-ARCH: `record archive --copy-to` names a directory that exists
        /// and is not empty, or a path that is no directory; nothing was written and the recording
        /// is as it was (exit 3; actinglab details `path`, `reason`).
        RecordArchiveTargetNotEmpty => "record_archive_target_not_empty": error / none,
        /// Error. Workflow #449 REC-ARCH: a file an archive's manifest lists does not hold the
        /// sha256 the manifest gives, or cannot be read: a copy `record archive --copy-to` made, or
        /// a file of the recording's own directory when an archived recording is named again
        /// (exit 5; actinglab details `file`, `path`, `checked`, `reason`, `failures`). Nothing is
        /// deleted and the recording stays archived.
        RecordArchiveVerifyFailed => "record_archive_verify_failed": error / none,
        /// Error. Workflow #449 REC-ARCH: the recording `record archive --record-id` or `record
        /// reopen` names is not an archived recording of the instance; nothing was written (exit 3;
        /// actinglab details `record_id`, `state`).
        RecordNotArchived => "record_not_archived": error / none,
        /// Error. #463 R1: a recording step's `select` (a choice in a list of fixed slots by the
        /// name read in each) cannot be declared or generated (key `select_problem`: why); the
        /// command is refused and nothing is written (exit 3; actinglab details `step`).
        RecordSelectInvalid => "record_select_invalid": error / none,
        /// Error. #463 R2: an ending of a stopped recording (`record mark --ending`, a request's
        /// `ending` or `remove_ending`) cannot be taken, given up or generated (key
        /// `ending_problem`: why); the command is refused and nothing is written (exit 3;
        /// actinglab details `outcome_key`).
        RecordEndingInvalid => "record_ending_invalid": error / none,
        /// Error. #463 R2: `record stop`'s self-check item 9: an ending's page passes on recorded
        /// frames of a screen the run may show instead (key `against`: which), so the run would
        /// end on it there; nothing is written (exit 3; actinglab details `outcome_key`, `page`,
        /// `frames`).
        RecordEndingAmbiguous => "record_ending_ambiguous": error / none,
        /// Error. #463 R4: an optional step a stopped recording inserts after the effect of one of
        /// its steps (`record mark --optional-after`, a request's `optional_after`), or gives up
        /// (`remove_inserted`), cannot be inserted or given up (key `optional_after_problem`:
        /// why); the command is refused and nothing is written (exit 3; actinglab details `step`).
        RecordOptionalAfterInvalid => "record_optional_after_invalid": error / none,
    }
}

// Workflow #396 REC-4: one location registry for the crate.
actingcommand_contract::outcome_locations! {
    /// Where in the Lab a result arose.
    pub enum LabLocation {
        /// Checking a click that follows a template mark or (#463 L2) a located-text mark: its
        /// declaration, and the followed mark and click box on every live frame of the step.
        RecordClickFollowCheck => "record_click_follow_check",
    }
}

// Workflow #396 REC-4; #463 L2: a click may follow a located-text mark too.
actingcommand_contract::outcome_vocabulary! {
    /// Why a click that follows a template mark or a located-text mark does not hold
    /// (`follow_problem`).
    pub enum ClickFollowProblem: "follow_problem" {
        /// The followed mark is neither a template mark nor a located-text mark of the step.
        NotTemplate => "not_template",
        /// The followed mark is only a member of an `any_of` check of the step's page.
        NotRequired => "not_required",
        /// The click guard names another mark than the followed one.
        GuardConflict => "guard_conflict",
        /// On the primary frame the template's best match is not at its crop, or not at factor
        /// 1000.
        PrimaryMismatch => "primary_mismatch",
        /// On a frame the template does not pass in its search area.
        TemplateUnmatched => "template_unmatched",
        /// On a frame the click box that follows the match or the found text leaves the frame.
        ClickOutsideFrame => "click_outside_frame",
        /// #463 L2: on the primary frame the press of the step's executed click lies outside the
        /// box the followed located text gives there.
        PressOutsideFoundBox => "press_outside_found_box",
        /// #463 L2 fix 1: the step's page would need the followed located text to be recognized:
        /// no other mark of the step recognizes it, or a check of the page names the located text.
        PageNeedsFollowedText => "page_needs_followed_text",
    }
}

// #463 R1: why a recording step's select cannot be declared or generated.
actingcommand_contract::outcome_vocabulary! {
    /// Why a Lab recording step's `select` cannot hold (`select_problem`).
    pub enum SelectProblem: "select_problem" {
        /// The request's click follows a mark beside the select: the select's click is the slot it
        /// chooses.
        WithFollow => "with_follow",
        /// The request or the step declares a drag, a long press or another gesture: a select
        /// taps once.
        WithGesture => "with_gesture",
        /// The request declares a click rectangle or a click source beside the select: the
        /// select's click is the slot it chooses.
        WithClick => "with_click",
        /// The request or the step declares a click retry: a select step declares no retry.
        WithRetry => "with_retry",
        /// The step is optional, or the request makes it optional: a select step's page is its
        /// list.
        OnOptionalStep => "on_optional_step",
        /// The request or the step declares an application operation: one step has one effect.
        WithApplication => "with_application",
        /// The stopped recording's last step: the package ends on its page, which then would be
        /// the select's empty end too.
        OnFinalStep => "on_final_step",
        /// A slot's rectangle, click box or name region does not lie inside the frame.
        SlotOutsideFrame => "slot_outside_frame",
        /// A name's `id` breaks the grammar `^[a-z][a-z0-9_]{0,63}$`.
        NameIdInvalid => "name_id_invalid",
        /// Fix 1: the wanted value's `name` breaks the grammar `^[a-z][a-z0-9_]{0,63}$`.
        WantedNameInvalid => "wanted_name_invalid",
        /// Two names of the select have one `id`.
        NameDuplicate => "name_duplicate",
        /// The wanted value's `default` is no `id` of the select's names.
        DefaultNotInNames => "default_not_in_names",
        /// `pressed` is no `id` of the select's names.
        PressedNotInNames => "pressed_not_in_names",
        /// Another select step of the recording names its wanted value alike.
        WantedNameTaken => "wanted_name_taken",
        /// The `on_empty_key` breaks the grammar `^[a-z0-9][a-z0-9_]{0,127}$`.
        OnEmptyKeyInvalid => "on_empty_key_invalid",
        /// Fix 1: the `on_empty` key is `<task_id>_done`, the key of the run's usual end.
        OnEmptyKeyReserved => "on_empty_key_reserved",
        /// The `on_empty` key is another select step's of the recording (fix 1: the usual end's
        /// key is `on_empty_key_reserved`).
        OnEmptyKeyTaken => "on_empty_key_taken",
        /// The step's page has no template, color, color digest or check mark (or a value test
        /// named as its click guard): a select step needs a guard.
        GuardMissing => "guard_missing",
        /// The kernel's admission refuses the select step as `record stop` would generate it.
        KernelRefused => "kernel_refused",
        /// #463 R2: `clear_select` names a step that has no select, so there is nothing to take
        /// off.
        NoSelect => "no_select",
        /// #463 R2 fix 1: the `on_empty` key is the outcome key of an ending of the recording.
        OnEmptyKeyOfEnding => "on_empty_key_of_ending",
    }
}

// #463 R2: why an ending of a stopped recording cannot be taken, given up or generated.
actingcommand_contract::outcome_vocabulary! {
    /// Why an ending of a stopped Lab recording cannot hold (`ending_problem`).
    pub enum EndingProblem: "ending_problem" {
        /// The recording is active: an ending is taken after `record stop`, which records the
        /// usual ending first.
        RecordingActive => "recording_active",
        /// The request names a step change beside `ending` or `remove_ending`, or both of these.
        WithOtherChange => "with_other_change",
        /// The outcome key breaks the grammar `^[a-z0-9][a-z0-9_]{0,127}$`.
        KeyInvalid => "key_invalid",
        /// The outcome key is `<task_id>_done`, the key of the run's usual end.
        KeyReserved => "key_reserved",
        /// The outcome key is the `on_empty` key of a select step of the recording.
        KeyOfSelect => "key_of_select",
        /// Another ending of the recording has the outcome key.
        KeyExists => "key_exists",
        /// `remove_ending` names a key no ending of the recording has.
        KeyUnknown => "key_unknown",
        /// Another ending of the recording has the page name.
        PageExists => "page_exists",
        /// The ending adds and reuses no mark, so its page would recognize nothing.
        NoMarks => "no_marks",
        /// Fix 1: `record stop`'s self-check item 9 could not check the ending: its page rests on
        /// OCR marks alone and the Lab had no OCR model, so whether it passes on a screen the run
        /// may show instead is unknown.
        OcrAloneUnchecked => "ocr_alone_unchecked",
    }
}

// #463 R2: which recorded screen an ending's page passes on (`record stop`'s self-check item 9).
actingcommand_contract::outcome_vocabulary! {
    /// The screen an ending's page must not pass on (`against`).
    pub enum EndingAgainst: "ending_against" {
        /// The frames of a step after whose press the run awaits the endings (its page, or its page
        /// transition's frames when it has one): the result of that press has not shown yet.
        Previous => "previous",
        /// The frames of the last step, the usual ending: the ending would always win there.
        Usual => "usual",
        /// The frames of another ending of the recording.
        OtherEnding => "other_ending",
    }
}

// #463 R4: why an optional step a stopped recording inserts after one of its steps cannot be
// inserted or given up.
actingcommand_contract::outcome_vocabulary! {
    /// Why an optional step inserted after the stop cannot hold (`optional_after_problem`).
    pub enum OptionalAfterProblem: "optional_after_problem" {
        /// The recording is active: a step is inserted after `record stop`, which generated the
        /// package of the recorded path first.
        RecordingActive => "recording_active",
        /// The request names another change beside `optional_after` or `remove_inserted`, or both
        /// of these.
        WithOtherChange => "with_other_change",
        /// `optional_after.step` names no step of the recording (none, a dropped or a converted
        /// one).
        StepUnknown => "step_unknown",
        /// `optional_after.step` names the last step: every run ends on its page, after which
        /// nothing is pressed.
        StepLast => "step_last",
        /// `optional_after.step` names a step whose effect is an application operation.
        StepApplication => "step_application",
        /// The inserted step adds and reuses no mark, so its page would recognize nothing.
        NoMarks => "no_marks",
        /// The inserted step declares no click, so nothing would close its pop-up.
        NoClick => "no_click",
        /// The request clears `optional` on an inserted step: a step never recorded on the path
        /// stays optional.
        ClearsOptional => "clears_optional",
        /// `remove_inserted` names no inserted step of the recording that is not given up yet.
        NotInserted => "not_inserted",
    }
}
