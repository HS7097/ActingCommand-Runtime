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
        /// Error. Workflow #392 DA-5cd: an offline scene is neither of its package's
        /// `coordinate_space` nor a pure scale of it the display adapter admits for a run (keys
        /// `display_problem`: why, `frame_width`, `frame_height`: the scene's size, `base_width`,
        /// `base_height`: the package's); nothing is evaluated (exit 2).
        LabSceneSizeUnsupported => "lab_scene_size_unsupported": error / none,
        /// Error. Workflow #392 DA-5cd: `record start` on an instance whose display the Runtime
        /// knows at another size than the base size, or `record stop` of a recording whose frames
        /// show a larger display of the base size's ratio (keys `frame_width`, `frame_height`: the
        /// display's size, `base_width`, `base_height`: the base size); nothing is written, and a
        /// refused start holds nothing (exit 3).
        RecordDisplayNotBase => "record_display_not_base": error / none,
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

// #463 S2-1: what a recording still lacks before `record stop` can make its package, one word per
// pre-check of the generator and one cause per word (`record ready`'s `missing[].what`).
actingcommand_contract::outcome_vocabulary! {
    /// One pre-check of `record stop` that a recording fails (`ready_missing`).
    pub enum ReadyMissing: "ready_missing" {
        /// The recording has fewer than two effective steps (`record_no_steps`).
        TooFewSteps => "too_few_steps",
        /// A step after the first is an application entry step, which has no frame
        /// (`record_application_entry_not_first`).
        ApplicationEntryNotFirst => "application_entry_not_first",
        /// A step before the last has no effect: no click, select or application operation
        /// (`record_step_click_missing`).
        EffectMissing => "effect_missing",
        /// The last step has an effect; it only recognizes the screen the package arrives at
        /// (`record_final_step_has_click`).
        FinalHasEffect => "final_has_effect",
        /// A step but the application entry step has no own or reused mark
        /// (`record_step_without_recognition`).
        PageRecognition => "page_recognition",
        /// A step's page transition has no own or reused mark (`record_step_without_recognition`
        /// with `transition: "page"`).
        TransitionRecognition => "transition_recognition",
        /// More steps have an effect than a package holds (`record_too_many_steps`).
        TooManyEffects => "too_many_effects",
        /// The package's game is known neither from `record stop` nor from the recording nor from
        /// the instance configuration (`record_locale_missing`).
        Game => "game",
        /// The package's server is known neither from `record stop` nor from the recording nor
        /// from the instance configuration (`record_locale_missing`).
        Server => "server",
        /// The package's locale is known neither from `record stop` nor from the recording
        /// (`record_locale_missing`).
        Locale => "locale",
        /// After a step that stops the application the next effect is no launch or restart
        /// (`record_step_after_stop_invalid`).
        EffectAfterStop => "effect_after_stop",
        /// A step's page or its page transition reuses a mark that is no mark of an effective step
        /// (`record_mark_rejected`, `reuse_source_missing`).
        ReusedMarkMissing => "reused_mark_missing",
        /// A step forbids a mark that is no mark of an effective step (`record_mark_rejected`,
        /// `forbidden_source_missing`).
        ForbiddenMarkMissing => "forbidden_mark_missing",
        /// A step's anchor is no own template mark of it (`record_mark_rejected`,
        /// `anchor_invalid`).
        AnchorNotOwnTemplate => "anchor_not_own_template",
        /// The last launch or restart is followed by no step that is the main interface
        /// (`record_application_without_home`).
        ApplicationWithoutHome => "application_without_home",
        /// The first step is optional (`record_optional_first_step`).
        OptionalFirstStep => "optional_first_step",
        /// The last step is optional (`record_optional_final_step`).
        OptionalFinalStep => "optional_final_step",
        /// An application step is optional (`record_optional_application`).
        OptionalApplication => "optional_application",
        /// The main interface after the last launch or restart is optional
        /// (`record_optional_restart_segment_end`).
        OptionalRestartHome => "optional_restart_home",
        /// A select step is optional (`record_select_invalid`, `on_optional_step`).
        SelectOptional => "select_optional",
        /// A select step's click declares a retry (`record_select_invalid`, `with_retry`).
        SelectRetry => "select_retry",
        /// A select step's click declares a gesture (`record_select_invalid`, `with_gesture`).
        SelectGesture => "select_gesture",
        /// A select step's click follows a mark (`record_select_invalid`, `with_follow`).
        SelectFollow => "select_follow",
        /// A select step's `on_empty` key is the usual ending's (`record_select_invalid`,
        /// `on_empty_key_reserved`).
        SelectKeyReserved => "select_key_reserved",
        /// A select step's `on_empty` key is another select step's (`record_select_invalid`,
        /// `on_empty_key_taken`).
        SelectKeyTaken => "select_key_taken",
        /// A select step's wanted name is another select step's (`record_select_invalid`,
        /// `wanted_name_taken`).
        SelectWantedTaken => "select_wanted_taken",
        /// An ending's outcome key is the usual ending's (`record_ending_invalid`,
        /// `key_reserved`).
        EndingKeyReserved => "ending_key_reserved",
        /// An ending's outcome key is a select step's `on_empty` key (`record_ending_invalid`,
        /// `key_of_select`).
        EndingKeyOfSelect => "ending_key_of_select",
        /// An ending's outcome key is another ending's (`record_ending_invalid`, `key_exists`).
        EndingKeyTaken => "ending_key_taken",
        /// An ending's page name is another ending's (`record_ending_invalid`, `page_exists`).
        EndingPageTaken => "ending_page_taken",
        /// An ending's page reuses a mark that is no mark of an effective step
        /// (`record_mark_rejected`, `reuse_source_missing`).
        EndingMarkMissing => "ending_mark_missing",
        /// Workflow #392 DA-5cd: the recording's frames show a larger display of the base size's
        /// ratio, a recording made on a display that is not the base size
        /// (`record_display_not_base`).
        DisplayNotBase => "display_not_base",
    }
}

// #463 S2-1: the self-checks `record stop` runs once its pre-checks pass, in the order they run
// (`record ready`'s `checks_after`, and the `what` of a self-check's failure).
actingcommand_contract::outcome_vocabulary! {
    /// One self-check of `record stop` (`ready_check`).
    pub enum ReadyCheck: "ready_check" {
        /// Every live frame and template crop is read back with its recorded sha256 and the
        /// recording's size.
        Frames => "frames",
        /// The generated package is admitted by the kernel and gives its evaluator.
        Admission => "admission",
        /// The offline simulation of the package's first step on its recorded frames.
        FirstDecision => "first_decision",
        /// Every page on its own frames, the guards on located text, the optional steps, the
        /// arrival gates and the entry overlay.
        CrossCheck => "cross_check",
        /// Every ending's page on its own frames and on the screens a run may show instead.
        Endings => "endings",
        /// Every select step's decisions on its own frames.
        Selects => "selects",
        /// The final package: encoded, expanded again, admitted again, and the same first decision.
        FinalPackage => "final_package",
    }
}

// #463 S2-1: what `record ready` warns of, never blocking (`warnings[].what`).
actingcommand_contract::outcome_vocabulary! {
    /// A warning of `record ready` (`ready_warning`).
    pub enum ReadyWarning: "ready_warning" {
        /// A step's press is a single recorded point with no press region set: the package would
        /// press that exact pixel.
        PressPointOnly => "press_point_only",
    }
}
