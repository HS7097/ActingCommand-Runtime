// SPDX-License-Identifier: AGPL-3.0-only

//! The contract crate's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/contract.json`; the outcome guard (G1) checks both directions.

crate::outcome_codes! {
    /// The codes the contract crate owns: the fallback code of each kind of third-party
    /// failure (model section 2.5) and the caught-panic link (section 2.4). Any crate emits
    /// them; a crate that recognises a foreign condition registers its own code and attaches
    /// the fallback as its `caused_by` link.
    pub enum ContractCode {
        ForeignOsError => "foreign_os_error": error,
        ForeignSqliteError => "foreign_sqlite_error": error,
        ForeignNemuError => "foreign_nemu_error": error,
        ForeignAdbError => "foreign_adb_error": error,
        ForeignMumuManagerError => "foreign_mumu_manager_error": error,
        ForeignVisionError => "foreign_vision_error": error,
        ForeignDataError => "foreign_data_error": error,
        ForeignHttpError => "foreign_http_error": error,
        ForeignOtherError => "foreign_other_error": error,
        PanicCaught => "panic_caught": warning,
    }
}
