// SPDX-License-Identifier: AGPL-3.0-only

//! The Runtime host's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/runtime-host.json`; the outcome guard (G1) checks both directions.
//! Until slice A2 converts this crate, a code travels as its spelling (`HostCode::as_str`)
//! through the existing error types, with only `key=value` tokens in their detail fields.

actingcommand_contract::outcome_codes! {
    /// Codes the Runtime host registered ahead of the A2 sweep (Workflow #375).
    pub(crate) enum HostCode {
        /// Error. `ledger-maintenance` refuses a root whose artifact retention is not
        /// settled: on a formal root an eviction intent without its outcome, which only the
        /// next actingd start completes (keys `pending_evictions`; the detail also names
        /// `remedy=start_actingd_once`); on a Segment root any recorded eviction.
        MaintenanceArtifactMaterialUnavailable => "maintenance_artifact_material_unavailable": error,
    }
}
