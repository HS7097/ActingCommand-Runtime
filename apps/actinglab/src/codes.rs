// SPDX-License-Identifier: AGPL-3.0-only

//! actinglab's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/actinglab.json`; the outcome guard (G1) checks both directions.

actingcommand_contract::outcome_codes! {
    /// actinglab's registered codes; the #389 U-0 block is registered ahead of the slice that
    /// emits it.
    // ---- #389 U-0 registrations (pending), emitted from U-3
    // (contracts/runtime-interface.md, "Outcome codes"). Nothing emits them yet.
    #[allow(dead_code)]
    pub(crate) enum ActinglabCode {
        /// Error. An offline package-tool command while Lab option one (authoring) is not
        /// installed (keys `operation`, `lab_option`).
        LabCommandOptionDisabled => "lab_command_option_disabled": error / none,
        // ---- end of #389 U-0 registrations
        // ---- #391 MCP-391b (pending)
        /// Error. `observe`'s Min output still exceeds its hard byte limit with every fact row
        /// and every projection entry omitted, so only the identity is left and it does not
        /// fit (keys `actual_bytes`, `limit_bytes`).
        ObserveIdentityLimitExceeded => "observe_identity_limit_exceeded": error / none,
        // ---- end of #391 MCP-391b
    }
}
