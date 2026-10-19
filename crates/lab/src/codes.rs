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
        /// Error. Workflow #396 REC-4: a click that follows a template mark does not hold
        /// (keys `mark_id`: the followed mark, `follow_problem`: why; `stage`). A declaration
        /// that cannot hold refuses `record mark` with it; a frame on which it does not hold
        /// is its reason inside `record_mark_rejected`.
        RecordClickFollowInvalid => "record_click_follow_invalid": error / none,
    }
}

// Workflow #396 REC-4: one location registry for the crate.
actingcommand_contract::outcome_locations! {
    /// Where in the Lab a result arose.
    pub enum LabLocation {
        /// Checking a click that follows a template mark: its declaration, and the followed
        /// template and click box on every live frame of the step.
        RecordClickFollowCheck => "record_click_follow_check",
    }
}

// Workflow #396 REC-4.
actingcommand_contract::outcome_vocabulary! {
    /// Why a click that follows a template mark does not hold (`follow_problem`).
    pub enum ClickFollowProblem: "follow_problem" {
        /// The followed mark is not a template mark of the step.
        NotTemplate => "not_template",
        /// The followed template is only a member of an `any_of` check, not a required mark of
        /// the step's page.
        NotRequired => "not_required",
        /// The click guard names another mark than the followed template.
        GuardConflict => "guard_conflict",
        /// On the primary frame the template's best match is not at its crop, or not at factor
        /// 1000.
        PrimaryMismatch => "primary_mismatch",
        /// On a frame the template does not pass in its search area.
        TemplateUnmatched => "template_unmatched",
        /// On a frame the click box that follows the match leaves the frame.
        ClickOutsideFrame => "click_outside_frame",
    }
}
