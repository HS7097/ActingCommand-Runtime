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
        // Workflow #396 REC-6: a value test that did not pass carries one of these as its
        // `failure`.
        ValueTestUnresolved => "value_test_unresolved": error / pack_logic_mismatch,
        ValueTestNotMet => "value_test_not_met": error / pack_logic_mismatch,
        /// Error. A recognition pack refused its package or a target declaration
        /// (`RecognitionPackErrorCode::InvalidPackage`).
        /// Workflow #378 A2-P.
        RecognitionPackInvalid => "recognition_pack_invalid": error / declaration_error,
        /// Error. A recognition target uses semantics this build cannot evaluate
        /// (`RecognitionPackErrorCode::UnsupportedTarget`).
        /// Workflow #378 A2-P.
        RecognitionPackTargetUnsupported => "recognition_pack_target_unsupported": error / declaration_error,
        /// Error. A recognition target needs a vision provider that is not configured
        /// (`RecognitionPackErrorCode::VisionProviderMissing`).
        /// Workflow #378 A2-P.
        RecognitionPackProviderMissing => "recognition_pack_provider_missing": error / declaration_error,
        /// Error. The vision provider failed while evaluating a recognition target
        /// (`RecognitionPackErrorCode::VisionProviderFailure`).
        /// Workflow #378 A2-P.
        RecognitionPackProviderFailed => "recognition_pack_provider_failed": error / environment_fault,
        /// Error. The vision provider answered with a response the recognition pack refuses
        /// (`RecognitionPackErrorCode::VisionProviderInvalidResponse`).
        /// Workflow #378 A2-P.
        RecognitionPackProviderResponseInvalid => "recognition_pack_provider_response_invalid": error / environment_fault,
        /// Error. An OCR target's region could not be resolved on the frame: its anchor did not
        /// match or the region left the frame (`RecognitionPackErrorCode::RegionUnresolved`).
        /// Workflow #378 A2-P.
        OcrRegionUnresolved => "ocr_region_unresolved": error / pack_logic_mismatch,
        /// Error. A text read refused to conclude; its own outcome says why
        /// (`RecognitionPackErrorCode::TextReadRefused`, Workflow #396 REC-1).
        /// Workflow #378 A2-P.
        RecognitionPackTextReadRefused => "recognition_pack_text_read_refused": error / budget_or_permission,
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
        // Workflow #396 REC-7.
        /// Scoring a masked template target that declares no `unique` on its exact score map.
        TargetMatchExact => "target_match_exact",
        // Workflow #396 REC-6.
        /// Reading, parsing and comparing the sides of a value test.
        ValueTestEvaluate => "value_test_evaluate",
        // Workflow #396 REC-8.
        /// Reading a candidate layout's shared OCR request for the targets it serves.
        TextReadShared => "text_read_shared",
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

// Workflow #396 REC-6.
actingcommand_contract::outcome_vocabulary! {
    /// Which side of a value test could not be read (`value_side`).
    pub enum ValueSide: "value_side" {
        /// The left side.
        Left => "left",
        /// The right side, another read value.
        Right => "right",
    }
}

// Workflow #396 REC-6.
actingcommand_contract::outcome_vocabulary! {
    /// Why one side of a value test could not be read (`value_problem`).
    pub enum ValueProblem: "value_problem" {
        /// The trimmed text is empty.
        Empty => "empty",
        /// The text is not in the declared format.
        Invalid => "invalid",
        /// The number does not fit 64 bits.
        Overflow => "overflow",
        /// The value lies outside the declared `min..=max`.
        OutOfRange => "out_of_range",
        /// The OCR target's region could not be resolved on the frame.
        RegionUnresolved => "region_unresolved",
        /// The read's confidence is missing or below the OCR target's `minimum_confidence`.
        LowConfidence => "low_confidence",
    }
}

// Workflow #396 REC-6.
actingcommand_contract::outcome_vocabulary! {
    /// The comparison of a value test (`value_op`).
    pub enum ValueOp: "value_op" {
        /// Equal.
        Eq => "eq",
        /// Not equal.
        Ne => "ne",
        /// Less than.
        Lt => "lt",
        /// Less than or equal.
        Le => "le",
        /// Greater than.
        Gt => "gt",
        /// Greater than or equal.
        Ge => "ge",
        /// The left side reads in its format.
        Parses => "parses",
    }
}
