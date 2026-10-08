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
        /// next actingd start completes (keys `pending_evictions`; the catalog's `handling`
        /// says to start actingd once); on a Segment root any recorded eviction.
        MaintenanceArtifactMaterialUnavailable => "maintenance_artifact_material_unavailable": error,
        /// Warning. A material read found no file for an artifact without an eviction proof
        /// (a frame deleted by hand); the read reports `missing` and nothing is recorded
        /// (keys `artifact_id`, `event_id`, carried by the read request).
        MaterialReadMissing => "material_read_missing": warning,
        /// Error. A proposal's report artifact could not be read; the request is refused
        /// and the Runtime keeps running (keys `artifact_id`).
        ProposalReportUnavailable => "proposal_report_unavailable": error,
        /// Error. A strategic report's evidence artifact could not be read; the request is
        /// refused and the Runtime keeps running (keys `artifact_id`).
        StrategicEvidenceUnavailable => "strategic_evidence_unavailable": error,
        /// Warning. The frame cleaner could not delete a frame for another reason than a
        /// held or absent file; the frame stays and is tried again after a restart (keys
        /// `artifact_id`, `io_kind`, `os_error`).
        FrameRetentionRemoveFailed => "frame_retention_remove_failed": warning,
        /// Warning. The frame cleaner could not move a frame into its kept folder for another
        /// reason than a held or absent file, and the frame stays where it is; or the frame
        /// moved and the move counter `kept/.moves` could not be written (keys `artifact_id`,
        /// `entry`, `io_kind`, `os_error`).
        FrameRetentionMoveFailed => "frame_retention_move_failed": warning,
    }
}
