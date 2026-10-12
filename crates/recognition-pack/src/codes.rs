// SPDX-License-Identifier: AGPL-3.0-only

//! The recognition pack's outcome codes (Workflow #378 A2d). Every entry is listed in the
//! fragment `contracts/outcome-codes/recognition-pack.json`; the outcome guard (G1) checks both
//! directions. A candidate projection failure ([`crate::CandidateProjectionFailure`]) carries
//! one of these, or one of the contract's `CandidateProjectionCode`s.

actingcommand_contract::outcome_codes! {
    /// Why no candidate projection was produced.
    pub enum RecognitionPackCode {
        CandidateLayoutUnknown => "candidate_layout_unknown": error,
        CandidateFeatureFailed => "candidate_feature_failed": error,
        CandidateFeatureProviderMissing => "candidate_feature_provider_missing": error,
        CandidateSamplesUnavailable => "candidate_samples_unavailable": error,
        CandidateSampleGeometryChanged => "candidate_sample_geometry_changed": error,
        CandidateSampleRecordFailed => "candidate_sample_record_failed": error,
        // Workflow #308 list selector L1 (begin).
        /// The anchor search of a `repeated_anchor` layout reached its time limit before it had
        /// scored every row of its region; no partial candidate set is returned (keys `stage`,
        /// `timeout_ms`, `count`: the positions found by then).
        CandidateSearchIncomplete => "candidate_search_incomplete": error,
        // Workflow #308 list selector L1 (end).
    }
}

// Workflow #308 list selector L1 (begin).
actingcommand_contract::outcome_locations! {
    /// Where in the recognition pack a result arose.
    pub enum RecognitionPackLocation {
        /// Enumerating the anchor matches of a `repeated_anchor` candidate layout.
        CandidateSearchRepeatedAnchor => "candidate_search_repeated_anchor",
    }
}
// Workflow #308 list selector L1 (end).
