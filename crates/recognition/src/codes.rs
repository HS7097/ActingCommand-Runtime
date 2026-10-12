// SPDX-License-Identifier: AGPL-3.0-only

//! The recognition crate's outcome codes (Workflow #378 A2d). Every entry is listed in the
//! fragment `contracts/outcome-codes/recognition.json`; the outcome guard (G1) checks both
//! directions.

actingcommand_contract::outcome_codes! {
    /// Why a `color_digest.v1` declaration, digest or comparison is refused
    /// (`contracts/color-digest.md`). A [`crate::color_digest::ColorDigestError`] carries one,
    /// and its `Display` text starts with the code's spelling.
    pub enum ColorDigestErrorCode {
        AlgorithmUnknown => "color_digest_algorithm_unknown": error,
        GridInvalid => "color_digest_grid_invalid": error,
        RegionInvalid => "color_digest_region_invalid": error,
        CellsInvalid => "color_digest_cells_invalid": error,
        ExcludeCellsInvalid => "color_digest_exclude_cells_invalid": error,
        GridMismatch => "color_digest_grid_mismatch": error,
    }
}
