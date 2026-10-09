// SPDX-License-Identifier: AGPL-3.0-only

//! The ledger's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/ledger.json`; the outcome guard (G1) checks both directions.
//! Until slice A2c converts this crate, a code travels as its spelling (`LedgerCode::as_str`)
//! through `GlobalLedgerError`, with no detail.

actingcommand_contract::outcome_codes! {
    /// Codes the ledger registered ahead of the A2c sweep (Workflow #375).
    pub(crate) enum LedgerCode {
        /// Error. A query on a selected ledger read (`GlobalLedger::open_selected`) named no
        /// event type, a type the read did not select, or a view (no keys).
        SelectionQueryUnsupported => "ledger_selection_query_unsupported": error,
    }
}

actingcommand_contract::outcome_locations! {
    /// Locations the ledger registered ahead of the A2c sweep (Workflow #375).
    pub(crate) enum LedgerLocation {
        /// Answering a query on a selected ledger read.
        QuerySelection => "ledger_query_selection",
    }
}
