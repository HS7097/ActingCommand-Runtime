// SPDX-License-Identifier: AGPL-3.0-only

//! actinglab's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/actinglab.json`; the outcome guard (G1) checks both directions.

actingcommand_contract::outcome_codes! {
    /// Codes registered ahead of the slices that emit them.
    // ---- #389 U-0 registrations (pending), emitted from U-3
    // (contracts/runtime-interface.md, "Outcome codes"). Nothing emits them yet.
    #[allow(dead_code)]
    pub(crate) enum ActinglabCode {
        /// Error. An offline package-tool command while Lab option one (authoring) is not
        /// installed (keys `operation`, `lab_option`).
        LabCommandOptionDisabled => "lab_command_option_disabled": error / none,
        // ---- end of #389 U-0 registrations
    }
}
