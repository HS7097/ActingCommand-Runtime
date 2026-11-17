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
        /// Error. Workflow #392 DA-5cd: an offline scene, a stored image, cannot be brought to its
        /// package's `coordinate_space` by the frame gate the Lab's live observation uses (the
        /// train of DA-5; keys `display_problem`: the gate's word for why, `frame_width`,
        /// `frame_height`: the scene's size, `base_width`, `base_height`: the package's); nothing
        /// is evaluated (exit 2).
        LabSceneSizeUnsupported => "lab_scene_size_unsupported": error / none,
        /// Error. Workflow #392 DA-5cd: this instance's display is not the base size: `record
        /// start` on an instance whose display the Runtime last observed at another size than the
        /// base size, or (fix round 1) the first frame of a recording whose header names a base
        /// size, of another size (keys `frame_width`, `frame_height`: the display's size,
        /// `base_width`, `base_height`: the base size); nothing was recorded, nothing is written,
        /// and a refused start holds nothing (exit 3; actinglab's own line exits 1).
        RecordDisplayNotBase => "record_display_not_base": error / none,
        /// Error. Workflow #392 DA-5cd fix round 1: this recording holds frames that are not the
        /// base size (a header that names a base size: frames of any other size; one that names
        /// none: frames of a larger display of the base size's ratio): `record stop` makes no
        /// package of it, `record ready` lists it (`size_not_base`) and a frame added to it is not
        /// stored (keys `frame_width`, `frame_height`: the frames' size, `base_width`,
        /// `base_height`: the base size); nothing is written (exit 3; actinglab's own line exits
        /// 1).
        RecordSizeNotBase => "record_size_not_base": error / none,
        /// Error. #463 S2-7: a page transition names by its frame id (`frame_id`,
        /// `sample_frame_ids`) a frame this recording does not hold; nothing is written (exit 3;
        /// actinglab details `step`, `frame_id`).
        RecordFrameUnknown => "record_frame_unknown": error / none,
        /// Error. #463 S2-7: `drop_step` names a step before the last whose stored frames show a
        /// change at its press or do not hold what the comparison needs (key `drop_problem`: why;
        /// fix round 1, key `drop_comparison`: which comparison showed the change); nothing is
        /// written (exit 3; actinglab details `step`).
        RecordDropUnproven => "record_drop_unproven": error / none,
        /// Warning. #463 RC-1: a regression case's `case.json` declares another `schema_version`
        /// than `actingcommand.resource-regression-case.v1`, or none; it is not run (verdict
        /// `not_run`) and fails nothing (optional key `schema_version`: the one it declares).
        CaseSchemaUnsupported => "case_schema_unsupported": warning / none,
        /// Warning. #463 RC-1: pixels of a regression case's scene outside every rectangle of its
        /// `scene_construction` are not the canvas colour (key `count`: how many); the case still
        /// runs.
        ResourceRegressionSceneOutsideRects => "resource_regression_scene_outside_rects": warning / none,
        /// Warning. #463 RC-1 fix round 1: a member of a regression case's top level that the
        /// contract does not name is read past, never a failure (optional key `member`: its name);
        /// the case still runs.
        CaseMemberUnknown => "case_member_unknown": warning / none,
        /// Error. #463 S2-10: a regression case `record stop` made from the recording for the
        /// package it generated misses on that package when the one runner of the case contract
        /// runs it: its page is not recognised on its scene, a target it expects fails, or another
        /// check fails, never only because another page matches too (key `regression_problem`:
        /// the word of its first failed check); the case is not written and `record stop` refuses
        /// as at any self-check's failure, nothing written (exit 3; actinglab details `case`,
        /// `page`, `frame_id`, `step` and `artifact_step` or `outcome_key`, `verdict`, `checks`).
        RecordCaseFailed => "record_case_failed": error / none,
        /// Warning. #463 S2-10, the coordinator's ruling after the first round (R2's ruling 4 as
        /// precedent): a regression case's page is recognised on its scene, but so is another page
        /// of the package, which a press step of a linear run does not ask (#463 SC-1: a select
        /// step's confirmation does, and `record stop` refuses that before the cases); the case is
        /// written without `expect.observe`, every target its page requires passing and (fix round
        /// 3) every target it forbids failing. `record stop` and `record ready` name it, with the
        /// other pages.
        RecordCasePageAmbiguous => "record_case_page_ambiguous": warning / none,
        /// Warning. #463 S2-10: the page a regression case `record stop` wrote expects is also a
        /// page the bare canvas answers, the canvas with no patch at all, so the case proves
        /// nothing of its page; it is written and named.
        RecordCaseBareCanvas => "record_case_bare_canvas": warning / none,
        /// Warning. #463 S2-10 fix round 3 (one cause per word): a regression case's page is
        /// recognised on its scene beside another page of the package, and the page has no
        /// recognition target to pin, so without `expect.observe` the case would declare nothing;
        /// it is not written. `record stop` and `record ready` name it, with the other pages.
        RecordCaseAmbiguousNotWritten => "record_case_ambiguous_not_written": warning / none,
        /// Warning. #463 S2-10 fix round 3: a recognition target of the package reads the whole
        /// frame (its region or its search area is the whole frame), so the scene of the case
        /// `record stop` wrote is the whole frame; it is written and named, with those targets.
        RecordCaseWholeFrame => "record_case_whole_frame": warning / none,
        /// Warning. #463 S2-10 fix round 3: the Lab has no text-recognition model and the package's
        /// page detection reads a text target, so the runner judged no regression case's page; the
        /// cases are written without their page judgement, and `record stop` says so once.
        RecordCasesOcrUnavailable => "record_cases_ocr_unavailable": warning / none,
        /// Error. #463 S2-8': the swipes recorded before a press cannot be folded into one
        /// scrolling search of that press (`fold_scan`), the fold cannot be undone (`unfold_scan`),
        /// a step action or a mark names a step of a fold, or `record stop` meets a fold no
        /// scrolling select reads yet (key `scan_problem`: why); nothing is written (exit 3;
        /// actinglab details `step`).
        RecordScanInvalid => "record_scan_invalid": error / none,
        /// Error. #463 KR-1: a recording step's key (the back key a step presses in place of a
        /// click, declared after the fact and never sent while recording: a request's `key`,
        /// `record mark --key back`, or an inserted optional step's) cannot be declared or
        /// generated (key `key_problem`: why); the command is refused and nothing is written (exit
        /// 3; actinglab details `step`).
        RecordKeyInvalid => "record_key_invalid": error / none,
        /// Warning. #463 RM-1: a template mark's score lies nearer its threshold than the bound
        /// (`--template-margin-bound`, default 0.03), on the side a self-check of `record stop`
        /// needs it on (`side`, a word of `template_threshold_side`): it passes where it must pass,
        /// or misses where it must not pass, by less than the bound. `lab.warnings`, `record ready`
        /// and `record review` name it with the mark, its step, its score, its threshold, the
        /// margin and the bound; it refuses nothing, proposes nothing and is not written into the
        /// package.
        RecordTemplateNearThreshold => "record_template_near_threshold": warning / none,
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
        /// #463 S2-6: `record stop` meets a step marked as holding a choice that no select refines
        /// yet: it knows no names, so no package is generated.
        ChoiceNotRefined => "choice_not_refined",
        /// #463 S2-6 fix round 1: a request's `choice: false` names a step that is not marked as
        /// holding a choice, so there is nothing to take off.
        NoChoice => "no_choice",
        /// #463 stage two, train, fix round: `drop_step` names a step that holds a choice (marked
        /// as holding one, or refined by a select), the last step too: its press may have chosen
        /// the item already selected, which changes nothing on the screen and still matters to a
        /// run handed another value; the choice is withdrawn first.
        DropOfChoice => "drop_of_choice",
        /// #463 stage two, train, fix round: `to_transition` names a step that holds a choice: a
        /// page transition holds none, so the conversion would lose it; the choice is withdrawn
        /// first.
        TransitionOfChoice => "transition_of_choice",
        /// #463 S2-9a: a scrolling select (`rows` and `scan`) on a step that holds no fold of the
        /// swipes recorded before it: it has no screens of the list to read.
        ScanWithoutFold => "scan_without_fold",
        /// #463 S2-9a: the request declares `rows` and `slots`, which exclude each other.
        RowsWithSlots => "rows_with_slots",
        /// #463 S2-9a: the request declares `confirm` beside `rows`: the kernel confirms a
        /// scrolled choice itself.
        ConfirmWithRows => "confirm_with_rows",
        /// #463 S2-9a: the request's `scan` states no `stop`, the kind of the scrolling choice.
        ScanStopMissing => "scan_stop_missing",
        /// #463 S2-9a: a region of a scrolling select does not lie inside the frame.
        ScanRegionOutsideFrame => "scan_region_outside_frame",
        /// #463 S2-9a: the row of a scrolling select does not lie inside the list.
        ScanRegionOutsideList => "scan_region_outside_list",
        /// #463 S2-9a: the anchor, the name region or the click box of a scrolling select does not
        /// lie inside the row.
        ScanRegionOutsideRow => "scan_region_outside_row",
        /// #463 S2-9a: a number the Lab measures from the demonstration cannot be measured there
        /// (a swipe recorded without its after loop's record) and the request does not give it.
        ScanNumberNotMeasured => "scan_number_not_measured",
        /// #463 S2-9b: a scrolling select straight to the wanted item that declares no end mark
        /// gives an `on_empty_key`: such a scan never reads its list to an end, so a run cannot end
        /// normally there; where the item is not found it fails with the kernel's code.
        OnEmptyWithoutEnd => "on_empty_without_end",
        /// #463 S2-9a fix round 1: the first forward swipe of the fold, the package's advance, moves
        /// the finger further along the list than the kernel admits (its length less a row's, less
        /// one), which no given number changes: the scroll is recorded again with shorter swipes.
        ScanSwipeTooLong => "scan_swipe_too_long",
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
        /// #463 KR-1 (was `no_click`): the inserted step declares no effect, neither a click nor a
        /// key, so nothing would close its pop-up.
        NoEffect => "no_effect",
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
        /// the Lab's instance configuration nor (#463 S2-1b) from the configuration of the
        /// installation the program runs from (`record_locale_missing`).
        Game => "game",
        /// The package's server is known neither from `record stop` nor from the recording nor
        /// from the Lab's instance configuration nor (#463 S2-1b) from the configuration of the
        /// installation the program runs from (`record_locale_missing`).
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
        /// Workflow #392 DA-5cd fix round 1: the recording holds frames that are not the base size
        /// (`record_size_not_base`).
        SizeNotBase => "size_not_base",
        /// #463 S2-6: a step marked as holding a choice that no select refines yet
        /// (`record_select_invalid`, `choice_not_refined`).
        SelectUnrefined => "select_unrefined",
        /// #463 S2-8': a press step whose recorded swipes are folded into it declares no scrolling
        /// select yet (`record_scan_invalid`, `scan_not_selected`).
        ScanWithoutSelect => "scan_without_select",
        /// #463 S2-9a: a scrolling select that reads its list to its end declares no end mark:
        /// the demonstration never reached the list's end (`record_scan_invalid`,
        /// `end_not_recorded`).
        ScanEndNotRecorded => "scan_end_not_recorded",
        /// #463 KR-1: a step that presses a key gets no guard (`record_key_invalid`,
        /// `guard_missing`; fix round 1: by a click step's rule, its page gives none).
        KeyGuardMissing => "key_guard_missing",
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
        /// #463 S2-10: the regression cases made from the recording, each run on the final
        /// package by the one runner of the case contract before it is written; the last check.
        Cases => "cases",
    }
}

// #463 S2-10: what the regression cases `record stop` writes say of themselves (`limits`).
actingcommand_contract::outcome_vocabulary! {
    /// A limit of the regression cases written with a package (`case_limit`).
    pub enum CaseLimit: "case_limit" {
        /// The cases were made from the frames the package was built from: that they pass shows
        /// the package recognises the screens it was recorded on and says nothing about screens
        /// it did not meet.
        SelfAgreement => "self_agreement",
    }
}

// #463 S2-1c: why a self-check `record ready` ran stopped at a failure short of its end, so that the
// failures it lists may not be all it has (`checks[].reason`).
actingcommand_contract::outcome_vocabulary! {
    /// Why a self-check stopped short of its end (`ready_check_stop`).
    pub enum ReadyCheckStop: "ready_check_stop" {
        /// The check judges the generated package as one, in stages each built on the one before
        /// (`admission`, `final_package`): its first refusal leaves no stage after it to judge.
        WholePackage => "whole_package",
        /// The check met an error that is none of its verdicts (a target the evaluator cannot
        /// evaluate, a stored frame it cannot decode): what it would judge after it is not judged.
        CheckError => "check_error",
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

// #463 RM-1: the side of its threshold a self-check of `record stop` needs a template mark on, which
// the warning `record_template_near_threshold` names (`side`).
actingcommand_contract::outcome_vocabulary! {
    /// The side of its threshold a template mark lies on, nearer than the bound, where a self-check
    /// needs it (`template_threshold_side`).
    pub enum TemplateThresholdSide: "template_threshold_side" {
        /// The check needs the mark to pass, and its score lies at or above its threshold by less
        /// than the bound.
        Passes => "passes",
        /// The check needs the mark not to pass, and its score lies below its threshold by less
        /// than the bound.
        Misses => "misses",
    }
}

// #463 RC-1: the verdict of a regression case (`contracts/resource-regression-case.md`).
actingcommand_contract::outcome_vocabulary! {
    /// The verdict of one regression case (`regression_verdict`).
    pub enum RegressionVerdict: "regression_verdict" {
        /// Every check of the case ran and passed.
        Passed => "passed",
        /// A check of the case failed: the reader's, the scene's, the package's or an
        /// expectation's.
        Failed => "failed",
        /// What ran passed, and a check did not run.
        Partial => "partial",
        /// Nothing ran: the case is of another schema, or no expectation of it could be evaluated.
        NotRun => "not_run",
    }
}

// #463 RC-1: the result of one check of a regression case.
actingcommand_contract::outcome_vocabulary! {
    /// The result of one check of a regression case (`regression_result`).
    pub enum RegressionResult: "regression_result" {
        /// The check held.
        Passed => "passed",
        /// The check did not hold; its `problem` says why.
        Failed => "failed",
        /// The check was not run; its `reason` says why.
        NotRun => "not_run",
    }
}

// #463 RC-1: why a check of a regression case failed, one cause per word.
actingcommand_contract::outcome_vocabulary! {
    /// Why a check of a regression case failed (`regression_problem`).
    pub enum RegressionProblem: "regression_problem" {
        /// The case's directory holds no `case.json` that can be read as a file.
        CaseMissing => "case_missing",
        /// `case.json` is no JSON object.
        CaseUnreadable => "case_unreadable",
        /// A member the contract requires is absent.
        MemberMissing => "member_missing",
        /// A member the contract does not list for this object.
        MemberUnknown => "member_unknown",
        /// A member of the wrong type, or out of its form.
        MemberInvalid => "member_invalid",
        /// `expect` declares no expectation.
        ExpectEmpty => "expect_empty",
        /// The scene file the case names is not there or cannot be read.
        SceneMissing => "scene_missing",
        /// The scene's bytes are no PNG image that decodes.
        SceneUnreadable => "scene_unreadable",
        /// The SHA-256 of the scene's bytes is not `scene_construction.scene_sha256`.
        SceneSha256Mismatch => "scene_sha256_mismatch",
        /// The scene's size is not the canvas's.
        CanvasSizeMismatch => "canvas_size_mismatch",
        /// The scene cannot be brought to its package's coordinate space as the offline commands
        /// bring a stored image there (they refuse it `lab_scene_size_unsupported`).
        SceneSizeUnsupported => "scene_size_unsupported",
        /// Fix round 1: the kernel's frame gate for a Lab frame could not read the scene at all
        /// (no size of it is refused: the offline commands relay it as a device error).
        SceneGateUnreadable => "scene_gate_unreadable",
        /// A rectangle of `scene_construction.rects` does not lie inside the canvas.
        RectOutsideCanvas => "rect_outside_canvas",
        /// The case's package is no directory.
        PackageMissing => "package_missing",
        /// The kernel's admission refused the package, or it gives no evaluator or page set.
        PackageRefused => "package_refused",
        /// Fix round 1: the case's `package_id` is not the one its package's control declares.
        PackageIdMismatch => "package_id_mismatch",
        /// Several pages of the package matched the scene.
        PageConflict => "page_conflict",
        /// The kernel recognised another page, or none, than the case expects.
        PageMismatch => "page_mismatch",
        /// The kernel's page detection refused the scene for another cause than a conflict.
        PageDetectionRefused => "page_detection_refused",
        /// The kernel's evaluation of a target is not the `passed` the case expects.
        TargetMismatch => "target_mismatch",
        /// The target is click-only, which is never evaluated on a scene.
        TargetClickOnly => "target_click_only",
        /// The kernel refused evaluating the target (one the package does not declare, for
        /// one).
        TargetRefused => "target_refused",
    }
}

// #463 RC-1: why a check of a regression case did not run.
actingcommand_contract::outcome_vocabulary! {
    /// Why a check of a regression case did not run (`regression_not_run`).
    pub enum RegressionNotRun: "regression_not_run" {
        /// This version of the runner reads the expectation and does not evaluate it.
        NotEvaluated => "not_evaluated",
        /// Evaluating it needs text recognition, and the Lab has no OCR model.
        OcrUnavailable => "ocr_unavailable",
        /// The package's target consensus reads more frames than the case's one scene.
        FramesRequired => "frames_required",
    }
}

// #463 S2-1b: where the package's game, server and locale `record ready` answers with came from
// (`sources`), the place `record stop` takes each from when it builds.
actingcommand_contract::outcome_vocabulary! {
    /// Where a part of the package's identity came from (`ready_source`).
    pub enum ReadySource: "ready_source" {
        /// The command line of `record ready` or `record stop`: `--game`, `--server` (actinglab's
        /// global options) or `--locale`.
        CommandLine => "command_line",
        /// What the recording holds: given at `record start`, or, on a stopped recording marked
        /// again, what its package was generated with.
        Recording => "recording",
        /// The Lab's instance configuration: the instance's `game` or `server` in actinglab's own
        /// configuration (game and server only).
        LabConfig => "lab_config",
        /// The configuration of the installation the program runs from: the instance's row of
        /// `policy.facts.instances` (game and server only).
        InstallationConfig => "installation_config",
    }
}

// #463 S2-7: why `drop_step` does not drop a step before the last: the stored frames do not hold
// what the comparison needs, or they show a change (fix round 1: they contradict that its press
// changed nothing; the tool refuses only that).
actingcommand_contract::outcome_vocabulary! {
    /// Why a step before the last cannot be dropped (`drop_problem`).
    pub enum DropProblem: "drop_problem" {
        /// The step has no press on record with the stored frame it was sent on and the stored
        /// frame after it (no click performed by `tap`, `swipe` or `long-tap --record`, a press
        /// sent on a capture that was not stored, an application operation), or a frame the
        /// comparison needs is not on record.
        FramesNotRecorded => "frames_not_recorded",
        /// The stored frames show a change: the frame after the press differs from the frame
        /// before it by more than the frame-change bound, by one of the two comparisons
        /// (`drop_comparison`).
        ScreenChanged => "screen_changed",
        /// The stored frames show a change: the next step's frame before its press differs from
        /// the frame before this press by more than the frame-change bound, by one of the two
        /// comparisons (`drop_comparison`).
        NextScreenDiffers => "next_screen_differs",
        /// #463 KR-1 fix round 1: the step's effect is a declared key: the frames on record are
        /// those of the press the key replaced, and nothing shows what the key does; the key is
        /// taken back first.
        KeyDeclared => "key_declared",
    }
}

// #463 S2-7 fix round 1 (the coordinator's ruling R1): which of the two comparisons of two stored
// frames showed the change that `drop_step` refuses a step before the last for.
actingcommand_contract::outcome_vocabulary! {
    /// Which comparison showed a change (`drop_comparison`).
    pub enum DropComparison: "drop_comparison" {
        /// The whole frame, by the frame-change measure.
        Frame => "frame",
        /// The neighbourhood of the press (its rectangle grown by one cell of the whole-frame
        /// measure's grid on every side, clamped to the frame), by the same measure.
        PressRegion => "press_region",
    }
}

// #463 S2-8': why the swipes recorded before a press cannot be folded into one scrolling search of
// that press, or the fold undone, or a step of a fold acted on, or the fold generated.
actingcommand_contract::outcome_vocabulary! {
    /// Why a fold of recorded swipes cannot hold (`scan_problem`).
    pub enum ScanProblem: "scan_problem" {
        /// `fold_scan` names no fold: its first step is not below its press step; a step of the
        /// range is unknown, dropped, converted into a transition, or folded or holding a fold
        /// already; the press step's effect is no executed tap; or a step between them is no
        /// executed swipe.
        FoldRangeInvalid => "fold_range_invalid",
        /// Fix round 1: a step of the range, the press step included, is an optional step a
        /// stopped recording inserted after the stop, no step recorded on the path.
        InsertedInRange => "inserted_in_range",
        /// A step the fold would fold has marks of its own, which no page of the package would
        /// read any more.
        WouldLoseMarks => "would_lose_marks",
        /// A step the fold would fold names its page.
        WouldLosePage => "would_lose_page",
        /// A step the fold would fold is optional.
        WouldLoseOptional => "would_lose_optional",
        /// A step the fold would fold holds a choice (marked as holding one, or refined by a
        /// select).
        WouldLoseChoice => "would_lose_choice",
        /// A step the fold would fold has a transition after its effect.
        WouldLoseTransition => "would_lose_transition",
        /// Fix round 1: a step the fold would fold has an optional step a stopped recording
        /// inserted after it after the stop, and not given up, which would run after no step of
        /// the package.
        WouldLoseInserted => "would_lose_inserted",
        /// A swipe of the range runs clearly along neither axis (its move along one axis is not
        /// at least twice its move across it), or along another axis than the first swipe.
        MoveAxisUnclear => "move_axis_unclear",
        /// A swipe in the first swipe's direction after one against it: the demonstration goes
        /// forward as often as needed, then back as often as needed, then presses.
        MoveAfterBack => "move_after_back",
        /// `unfold_scan` names a step that holds no fold.
        NotFolded => "not_folded",
        /// A step action or a mark names a step of a fold: a swipe folded into a press step, or (a
        /// step action) the press step that holds the fold; the fold is undone first.
        StepInFold => "step_in_fold",
        /// `record stop` meets a press step whose swipes are folded into it and which declares no
        /// scrolling select yet, so its package would press without scrolling.
        ScanNotSelected => "scan_not_selected",
        /// #463 S2-9a: `unfold_scan` names a press step whose fold a scrolling select reads; the
        /// select is taken off first (`clear_select`).
        ScanSelected => "scan_selected",
        /// #463 S2-9a: `record stop` meets a scrolling select that reads its list to its end and
        /// declares no end mark: the demonstration never reached the list's end.
        EndNotRecorded => "end_not_recorded",
    }
}

// #463 KR-1: why a recording step cannot press a key, the back key a step presses in place of a
// click, declared after the fact and never sent while recording.
actingcommand_contract::outcome_vocabulary! {
    /// Why a Lab recording step's key cannot be declared or generated (`key_problem`).
    pub enum KeyProblem: "key_problem" {
        /// The key's word is none of the vocabulary's, which is `back` alone.
        WordUnknown => "word_unknown",
        /// The request declares a click beside the key (a rectangle, a source mark, a followed
        /// mark, a gesture or a press box), or an inserted step declares both a click and a key.
        WithClick => "with_click",
        /// The request declares an application operation beside the key.
        WithApplication => "with_application",
        /// The request declares a click retry beside the key: a key step declares no retry.
        WithRetry => "with_retry",
        /// The step is the application entry step, which has no frame and no page to guard a key
        /// on.
        ApplicationEntry => "application_entry",
        /// The step's effect is an application operation, declared or executed.
        ApplicationStep => "application_step",
        /// A key and a choice on one step: the step holds a choice (a select or a choice mark), or
        /// the request gives one to a step with a key or beside a key.
        HoldsChoice => "holds_choice",
        /// The step is dropped or converted into a page transition.
        StepNotEffective => "step_not_effective",
        /// `record stop` meets a step that presses a key and gets no guard (fix round 1: by a
        /// click step's rule, it names none its page requires and its page requires no template,
        /// colour, colour digest or check mark): a key has no coordinate to trust.
        GuardMissing => "guard_missing",
        /// The train of 0.12.70 (#463 S2-8' joined): the step holds a fold, the swipes recorded
        /// before it folded into its scrolling search, so its press is the press of a scrolling
        /// search; the fold is undone first.
        HoldsFold => "holds_fold",
    }
}
