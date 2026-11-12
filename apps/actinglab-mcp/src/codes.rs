// SPDX-License-Identifier: AGPL-3.0-only

//! The codes of `actinglab-mcp` (`contracts/outcome-codes/actinglab-mcp.json`; Workflow #434
//! LAB-5G2a). The JSON-RPC errors and the usage refusals it answers are actingctl's, as today's MCP
//! server answers them (`actingcommand_lab_ops::codes`).

actingcommand_contract::outcome_codes! {
    /// The codes only this server emits.
    pub enum McpCode {
        OperationUnsupported => "mcp_operation_unsupported": error / none,
    }
}
