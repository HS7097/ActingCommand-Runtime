// SPDX-License-Identifier: AGPL-3.0-only

//! The artifact store's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/artifact-store.json`; the outcome guard (G1) checks both directions.
//! Until slice A2 converts this crate, a code travels as its spelling (`StoreCode::as_str`)
//! through `ArtifactStoreError`, with only `key=value` tokens in its detail.

actingcommand_contract::outcome_codes! {
    /// Codes the artifact store registered ahead of the A2 sweep (Workflow #375).
    pub(crate) enum StoreCode {
        /// Error. A read found no file at a frame's object key and could not list the kept
        /// folders below `<state root>\kept` to look for it there: a listing failed, or `kept`
        /// itself is a reparse point, which is never followed (keys `entry`, `io_kind`,
        /// `os_error`). The read fails; the lookup map of the previous walk stays.
        ArtifactKeptWalkFailed => "artifact_kept_walk_failed": error,
    }
}
