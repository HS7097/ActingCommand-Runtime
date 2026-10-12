// SPDX-License-Identifier: AGPL-3.0-only

//! vision-ffi's outcome codes (Workflow #378). A2a registers the three refusals of a vision
//! root's model listing here, ahead of A2d (decision D10): actingd's startup listing and its
//! `check-config`, and the vision-provider-check tool, refuse a models root with them. The
//! crate's closed `VisionFfiErrorCode` registers nothing: it stays inside the process and the
//! execution kernel maps it onto the recognition pack's `VisionProviderErrorCode` (A2d). Every
//! entry is listed in the fragment `contracts/outcome-codes/vision-ffi.json`; the outcome guard
//! (G1) checks both directions.

actingcommand_contract::outcome_codes! {
    /// Why the model listing of a vision root cannot be used.
    pub enum VisionListingCode {
        VisionRootUnavailable => "vision_root_unavailable": error,
        VisionModelsEmpty => "vision_models_empty": error,
        VisionModelFolderInvalid => "vision_model_folder_invalid": error,
    }
}
