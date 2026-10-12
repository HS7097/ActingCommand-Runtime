// SPDX-License-Identifier: AGPL-3.0-only

//! The page detector's outcome codes (Workflow #378 A2d). Every entry is listed in the fragment
//! `contracts/outcome-codes/page-detector.json`; the outcome guard (G1) checks both directions.
//! [`crate::PageUnexecutedReason`] stays the closed type of an unexecuted page's reason; each of
//! its variants names one of these codes, and its `Display` text is the code's spelling.

actingcommand_contract::outcome_codes! {
    /// Why a page of an evaluation batch was not evaluated.
    pub enum PageDetectorCode {
        BatchValidationFailed => "batch_validation_failed": warning / none,
        BatchTerminated => "batch_terminated": warning / none,
    }
}
