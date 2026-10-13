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
        // Workflow #308 list selector L1 (begin).
        /// The anchor search of a `repeated_anchor` layout reached its time limit before it had
        /// scored every row of its region; no partial candidate set is returned (keys `stage`,
        /// `timeout_ms`, `count`: the positions found by then).
        CandidateSearchIncomplete => "candidate_search_incomplete": error / budget_or_permission,
        // Workflow #308 list selector L1 (end).
        // Workflow #396 REC-1: text located by OCR. A target that did not pass carries the
        // first two as its `failure`; the last two are evaluation errors.
        TextLocateNotFound => "text_locate_not_found": error / pack_logic_mismatch,
        TextLocateOutsideCells => "text_locate_outside_cells": error / pack_logic_mismatch,
        TextReadBudgetExceeded => "text_read_budget_exceeded": error / budget_or_permission,
        TextReadBlocksTruncated => "text_read_blocks_truncated": error / budget_or_permission,
        // Workflow #396 REC-2: a target that declares `unique`. A second place is the
        // target's `failure`; a search past its limit is an evaluation error.
        TargetMatchNotUnique => "target_match_not_unique": error / pack_logic_mismatch,
        TargetMatchSearchIncomplete => "target_match_search_incomplete": error / budget_or_permission,
    }
}

// Workflow #308 list selector L1 and Workflow #396 REC-1: one location registry for the crate.
actingcommand_contract::outcome_locations! {
    /// Where in the recognition pack a result arose.
    pub enum RecognitionPackLocation {
        // Workflow #308 list selector L1 (begin).
        /// Enumerating the anchor matches of a `repeated_anchor` candidate layout.
        CandidateSearchRepeatedAnchor => "candidate_search_repeated_anchor",
        // Workflow #308 list selector L1 (end).
        // Workflow #396 REC-1 (begin).
        /// Comparing a locate OCR target's blocks with its expected values and declared cells.
        TextLocateEvaluate => "text_locate_evaluate",
        /// Checking one consumer's text read budget before calling the OCR provider.
        TextReadRequest => "text_read_request",
        // Workflow #396 REC-1 (end).
        // Workflow #396 REC-2.
        /// Checking that a target declaring `unique` has no second place within its margin.
        TargetMatchUnique => "target_match_unique",
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

// Workflow #396 REC-2.
actingcommand_contract::outcome_vocabulary! {
    /// Which kind of target a uniqueness check compared (`target_kind`).
    pub enum TargetMatchKind: "target_kind" {
        /// A template target: positions of its match score.
        Template => "template",
        /// A locate OCR target: text blocks and their confidence.
        Ocr => "ocr",
    }
}
