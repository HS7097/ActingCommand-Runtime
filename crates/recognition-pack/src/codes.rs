// SPDX-License-Identifier: AGPL-3.0-only

//! The recognition pack's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/recognition-pack.json`; the outcome guard (G1) checks both
//! directions. Until slice A2d converts this crate, a code travels as its spelling
//! (`RecognitionPackCode::as_str`) through [`crate::CandidateProjectionFailure`], with only
//! `key=value` tokens in its detail.

actingcommand_contract::outcome_codes! {
    /// Codes the recognition pack registered ahead of the A2d sweep (Workflow #308, list
    /// selector slice L1).
    pub(crate) enum RecognitionPackCode {
        /// Error. The anchor search of a `repeated_anchor` layout reached its time limit before
        /// it had scored every row of its region; no partial candidate set is returned (keys
        /// `stage`, `timeout_ms`, `count`: the positions found by then).
        CandidateSearchIncomplete => "candidate_search_incomplete": error,
    }
}

actingcommand_contract::outcome_locations! {
    /// Locations the recognition pack registered ahead of the A2d sweep (Workflow #308).
    pub(crate) enum RecognitionPackLocation {
        /// Enumerating the anchor matches of a `repeated_anchor` candidate layout.
        CandidateSearchRepeatedAnchor => "candidate_search_repeated_anchor",
    }
}
