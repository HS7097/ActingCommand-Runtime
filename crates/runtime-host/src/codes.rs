// SPDX-License-Identifier: AGPL-3.0-only

//! The Runtime host's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/runtime-host.json`; the outcome guard (G1) checks both directions.
//! Until slice A2 converts this crate, a code travels as its spelling (`HostCode::as_str`)
//! through the existing error types, with only `key=value` tokens in their detail fields.

actingcommand_contract::outcome_codes! {
    /// Codes the Runtime host registered ahead of the A2 sweep (Workflow #375, #369).
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
        /// reason than a held or absent file, and the frame stays where it is (keys
        /// `artifact_id`, `entry`, `io_kind`, `os_error`).
        FrameRetentionMoveFailed => "frame_retention_move_failed": warning,
        /// Warning. A frame moved into its kept folder, but the move counter `kept/.moves`
        /// could not be written, so other processes may find moved frames only at their next
        /// walk of the kept folders, within 60 s; once per process (keys `entry`, `io_kind`,
        /// `os_error`).
        FrameRetentionCounterFailed => "frame_retention_counter_failed": warning,
        /// Info. A policy candidate, or its admission, met an instance that a claim holds or
        /// that an eligible claim waits for: the candidate is deferred, or its intent is
        /// rejected at Info, and the driver wakes when a key returns (keys `instance_id`,
        /// `holder_kind`, `count`).
        DispatchInstanceHeld => "dispatch_instance_held": info,
        /// Info. A policy admission found the instance's admission guard taken by a keyless
        /// observe or probe for its whole short retry; the intent is rejected at Info and may
        /// run again from `next_eligible_unix_ms` (keys `instance_id`,
        /// `next_eligible_unix_ms`).
        DispatchInstanceContended => "dispatch_instance_contended": info,
        /// Info. A policy candidate met an instance in takeover cooldown after an unclean
        /// previous owner; it is deferred with a wake at the cooldown's end (keys
        /// `instance_id`, `next_eligible_unix_ms`).
        DispatchInstanceCooldown => "dispatch_instance_cooldown": info,
        /// Error. A held or released start stopped before it finished preparing because the
        /// Runtime latched a failure meanwhile. Operation `install_transition`. Until A2 makes
        /// it a `caused_by` link, the latched failure travels as the tokens
        /// `cause=<code> cause_operation=<operation>` (Workflow #381 A R3′). A restart may
        /// succeed: the watchdog considers a start (R5′).
        InstallStartupStopped => "install_startup_stopped": error,
        /// Error. A held start ended because a shutdown request for its owner was accepted
        /// before it was released. Operation `install_transition`. A formal stop: the watchdog
        /// holds it and never restarts it (Workflow #381 A, #672 review M1).
        InstallStartupShutDown => "install_startup_shut_down": error,
        /// Error. A held start received no release before its deadline; the transition fails
        /// with admission closed (Workflow #352). Operation `install_transition`.
        HeldTimeout => "held_timeout": error,
        /// Error. A released start did not finish preparing before its deadline; the
        /// transition fails with admission closed (Workflow #352). Operation
        /// `install_transition`.
        ReleaseTimeout => "release_timeout": error,
    }
}
