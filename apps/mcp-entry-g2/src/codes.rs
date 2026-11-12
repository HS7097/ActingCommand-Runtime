// SPDX-License-Identifier: AGPL-3.0-only

//! The second-generation entry's outcome codes (`contracts/outcome-codes/mcp-entry-g2.json`).
//! The entry answers a client request itself only when no child can answer it: the child ended
//! with the request in flight (its outcome is unknown and it is never sent again), or no child
//! could be started for it (it was not sent). They are the first generation's three, under this
//! program's own names: a code is registered once, by the crate that emits it.

actingcommand_contract::outcome_codes! {
    /// The codes of `actingcommand-mcp-entry-g2` (Workflow #434 LAB-5G5a).
    pub enum EntryCode {
        OutcomeUnknown => "mcp_entry_g2_outcome_unknown": error / none,
        ChildUnavailable => "mcp_entry_g2_child_unavailable": error / none,
        UsageInvalid => "mcp_entry_g2_usage_invalid": error / none,
    }
}
