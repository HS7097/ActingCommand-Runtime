// SPDX-License-Identifier: AGPL-3.0-only

//! The codes of `actinglab-mcp` (`contracts/outcome-codes/actinglab-mcp.json`; Workflow #434
//! LAB-5G2a). The JSON-RPC errors and the usage refusals it answers are actingctl's, as today's MCP
//! server answers them (`actingcommand_lab_ops::codes`).

actingcommand_contract::outcome_codes! {
    /// The codes only this server emits.
    pub enum McpCode {
        OperationUnsupported => "mcp_operation_unsupported": error / none,
        // Workflow #434 LAB-5G2b: the Runtime refused the identity card declared before a write;
        // the write went ahead (where today's server answers mcp_identity_card_refused).
        IdentityDeclarationRefused => "mcp_identity_declaration_refused": warning / none,
        // Workflow #434 LAB-5G3: the call goes on as a job of this server, named by its handle.
        JobStarted => "mcp_job_started": info / none,
    }
}
