// SPDX-License-Identifier: AGPL-3.0-only

//! actingledger's outcome codes and locations (Workflow #378). Every entry is listed in the
//! fragment `contracts/outcome-codes/actingledger.json`; the outcome guard (G1) checks both
//! directions. Spellings several owners share are the contract's (`SharedCode`, `ContractLocation`,
//! decision D3). Every entry added by A2c is pending (B3 settles them).

actingcommand_contract::outcome_codes! {
    /// actingledger's own codes: its command line and output.
    pub enum ActingledgerCode {
        LedgerViewSourceIncomplete => "ledger_view_source_incomplete": error,
        OutputFailed => "output_failed": error,
        StabilityExportIncomplete => "stability_export_incomplete": error,
        TaskEvidenceExportIncomplete => "task_evidence_export_incomplete": error,
    }
}

actingcommand_contract::outcome_locations! {
    /// actingledger's operations and stages.
    pub enum ActingledgerLocation {
        ExportStability => "export_stability",
        ExportTaskEvidence => "export_task_evidence",
        ParseArguments => "parse_arguments",
        ReplaySignatures => "replay_signatures",
        SerializeForensicReport => "serialize_forensic_report",
        WriteForensicOutput => "write_forensic_output",
    }
}
