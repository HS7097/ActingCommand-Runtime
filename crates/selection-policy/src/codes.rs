// SPDX-License-Identifier: AGPL-3.0-only

//! The selection policy's outcome codes (Workflow #378 A2d). Every entry is listed in the
//! fragment `contracts/outcome-codes/selection-policy.json`; the outcome guard (G1) checks both
//! directions. [`crate::SelectionErrorCode`] stays the crate's closed failure type: each of its
//! variants names one of these codes or one of the contract's `SharedCode`s.

actingcommand_contract::outcome_codes! {
    /// Why a selection-policy document, an evaluation input or the evaluation itself is
    /// refused, where no shared code has the meaning, and the steps of a decision's reason
    /// chain.
    pub enum SelectionPolicyCode {
        // Integer arithmetic and the canonical integer range.
        ArithmeticOverflow => "arithmetic_overflow": error / none,
        FloatRejected => "float_rejected": error / none,
        IntegerOutOfRange => "integer_out_of_range": error / none,
        // The offline evaluation shell's input files.
        ReadFailed => "read_failed": error / none,
        /// Info. Workflow #378 A2-P.
        SelectionPolicyIdentity => "selection_policy_identity": info / none,
        /// Info. Workflow #378 A2-P.
        SelectionInputsIdentity => "selection_inputs_identity": info / none,
        /// Info. Workflow #378 A2-P.
        SelectionRequirementDeclared => "selection_requirement_declared": info / none,
        /// Info. Workflow #378 A2-P (keys `unknown_reason`).
        SelectionUnknown => "selection_unknown": info / none,
        /// Info. Workflow #378 A2-P.
        CandidateRanked => "candidate_ranked": info / none,
        /// Info. Workflow #378 A2-P.
        SelectionSelected => "selection_selected": info / none,
        /// Info. Workflow #378 A2-P.
        SelectionEmpty => "selection_empty": info / none,
        /// Info. Workflow #378 A2-P.
        SelectionInsufficient => "selection_insufficient": info / none,
        /// Info. Workflow #378 A2-P.
        SelectionAmbiguous => "selection_ambiguous": info / none,
        /// Info. Workflow #378 A2-P.
        CandidateSelected => "candidate_selected": info / none,
        /// Info. Workflow #378 A2-P.
        SelectionOutcomeKeyChosen => "selection_outcome_key_chosen": info / none,
        /// Info. Workflow #378 A2-P (keys `rule_id`).
        GateUnknownAborted => "gate_unknown_aborted": info / none,
        /// Info. Workflow #378 A2-P (keys `rule_id`).
        GateUnknownSubstituted => "gate_unknown_substituted": info / none,
        /// Info. Workflow #378 A2-P (keys `rule_id`).
        GateUnknownDropped => "gate_unknown_dropped": info / none,
        /// Info. Workflow #378 A2-P (keys `rule_id`).
        GateRejected => "gate_rejected": info / none,
        /// Info. Workflow #378 A2-P (keys `rule_id`).
        TermUnknownAborted => "term_unknown_aborted": info / none,
        /// Info. Workflow #378 A2-P (keys `rule_id`).
        TermUnknownSubstituted => "term_unknown_substituted": info / none,
        /// Info. Workflow #378 A2-P (keys `rule_id`).
        TermUnknownDropped => "term_unknown_dropped": info / none,
        /// Info. Workflow #378 A2-P.
        CandidateScored => "candidate_scored": info / none,
        /// Info. Workflow #378 A2-P.
        TieBreakUnknown => "tie_break_unknown": info / none,
    }
}
