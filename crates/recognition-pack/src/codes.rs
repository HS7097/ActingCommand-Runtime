// SPDX-License-Identifier: AGPL-3.0-only

//! The recognition pack's outcome codes (Workflow #378 A2d). Every entry is listed in the
//! fragment `contracts/outcome-codes/recognition-pack.json`; the outcome guard (G1) checks both
//! directions. A candidate projection failure ([`crate::CandidateProjectionFailure`]) carries
//! one of these, or one of the contract's `CandidateProjectionCode`s.

actingcommand_contract::outcome_codes! {
    /// Why no candidate projection was produced.
    pub enum RecognitionPackCode {
        CandidateLayoutUnknown => "candidate_layout_unknown": error / declaration_error,
        CandidateFeatureFailed => "candidate_feature_failed": error / pack_logic_mismatch,
        CandidateFeatureProviderMissing => "candidate_feature_provider_missing": error / declaration_error,
        CandidateSamplesUnavailable => "candidate_samples_unavailable": error / none,
        CandidateSampleGeometryChanged => "candidate_sample_geometry_changed": error / environment_fault,
        CandidateSampleRecordFailed => "candidate_sample_record_failed": error / none,
        // Workflow #396 REC-1: text located by OCR. A target that did not pass carries the
        // first two as its `failure`; the last two are evaluation errors.
        TextLocateNotFound => "text_locate_not_found": error,
        TextLocateOutsideCells => "text_locate_outside_cells": error,
        TextReadBudgetExceeded => "text_read_budget_exceeded": error,
        TextReadBlocksTruncated => "text_read_blocks_truncated": error,
    }
}

// Workflow #396 REC-1.
actingcommand_contract::outcome_locations! {
    /// Where a text read or a text location happened.
    pub enum RecognitionPackLocation {
        TextLocateEvaluate => "text_locate_evaluate",
        TextReadRequest => "text_read_request",
    }
}

// Workflow #396 REC-1.
actingcommand_contract::outcome_vocabulary! {
    /// Which per-consumer text read budget ran out (`text_read_budget`).
    pub enum TextReadBudget: "text_read_budget" {
        /// The provider calls of one consumer.
        Calls => "calls",
        /// The summed region area of one consumer, in pixels.
        Area => "area",
    }
}
