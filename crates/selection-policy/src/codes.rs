// SPDX-License-Identifier: AGPL-3.0-only

//! The selection policy's outcome codes (Workflow #378 A2d). Every entry is listed in the
//! fragment `contracts/outcome-codes/selection-policy.json`; the outcome guard (G1) checks both
//! directions. [`crate::SelectionErrorCode`] stays the crate's closed failure type: each of its
//! variants names one of these codes or one of the contract's `SharedCode`s.

actingcommand_contract::outcome_codes! {
    /// Why a selection-policy document, an evaluation input or the evaluation itself is
    /// refused, where no shared code has the meaning.
    pub enum SelectionPolicyCode {
        // Integer arithmetic and the canonical integer range.
        ArithmeticOverflow => "arithmetic_overflow": error / none,
        FloatRejected => "float_rejected": error / none,
        IntegerOutOfRange => "integer_out_of_range": error / none,
        // The offline evaluation shell's input files.
        ReadFailed => "read_failed": error / none,
    }
}
