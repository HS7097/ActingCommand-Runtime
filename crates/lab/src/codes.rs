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
        /// A name's `id` or the wanted value's `name` breaks the grammar
        /// `^[a-z][a-z0-9_]{0,63}$`.
        NameIdInvalid => "name_id_invalid",
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
        /// The `on_empty_key` is the usual ending's key or another select step's.
        OnEmptyKeyTaken => "on_empty_key_taken",
        /// The step's page has no template, color, color digest or check mark (or a value test
        /// named as its click guard): a select step needs a guard.
        GuardMissing => "guard_missing",
        /// The kernel's admission refuses the select step as `record stop` would generate it.
        KernelRefused => "kernel_refused",
    }
}
