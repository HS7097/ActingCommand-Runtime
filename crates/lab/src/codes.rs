// SPDX-License-Identifier: AGPL-3.0-only

//! The Lab crate's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/lab.json`; the outcome guard (G1) checks both directions. The
//! crate's other result channels stay strings until B4 converts the Lab sites.

actingcommand_contract::outcome_codes! {
    /// Workflow #396 LAB-3 (FCM T-02): why the offline tools cannot evaluate OCR.
    pub enum LabCode {
        /// Error. An OCR evaluation of actinglab or the Lab cannot run because no usable OCR
        /// model was loaded (optional keys `target_id`, `path`: the vision root). Its cause
        /// says why.
        LabOcrUnavailable => "lab_ocr_unavailable": error / none,
        /// Error link. The offline tool has no Runtime configuration with a `vision` section:
        /// it does not run from an installation, or the selected configuration has none.
        LabOcrVisionUnconfigured => "lab_ocr_vision_unconfigured": error / none,
        /// Error link. The vision root's models folder holds no OCR model folder (key `path`:
        /// the models folder).
        LabOcrModelMissing => "lab_ocr_model_missing": error / none,
        /// Error link. The vision root has no ONNX Runtime library (key `path`: the library the
        /// Runtime would load).
        LabOcrRuntimeMissing => "lab_ocr_runtime_missing": error / none,
    }
}
