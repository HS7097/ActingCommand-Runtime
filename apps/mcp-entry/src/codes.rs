// SPDX-License-Identifier: AGPL-3.0-only

//! The entry's outcome codes (`contracts/outcome-codes/mcp-entry.json`). The entry answers a
//! client request itself only when no child can answer it: the child ended with the request in
//! flight (its outcome is unknown and it is never sent again), or no child could be started for
//! it (it was not sent).

actingcommand_contract::outcome_codes! {
    /// The codes of `actingcommand-mcp-entry` (Workflow #421 MCP-ENTRY).
    pub enum EntryCode {
        OutcomeUnknown => "mcp_entry_outcome_unknown": error / none,
        ChildUnavailable => "mcp_entry_child_unavailable": error / none,
        UsageInvalid => "mcp_entry_usage_invalid": error / none,
    }
}
