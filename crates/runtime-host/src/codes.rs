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
        /// Warning. A stuck-recovery ladder no longer holds its instance's key at a holding
        /// check: it cancels its continuation, fails the current rung with this reason and
        /// finishes exhausted at warning (keys `instance_id`, `lease_id`).
        RecoveryLadderKeyLost => "recovery_ladder_key_lost": warning,
        /// Error. A lease used, renewed or released after its expiry; since #369 S2+S3b also
        /// a Runtime-held key whose renewal came after it lapsed (keys `lease_id`). The
        /// scheduler spells it too until A2b moves its spellings into a registry.
        LeaseExpired => "lease_expired": error,
        /// Error. A lease that is no longer the instance's lease; since #369 S2+S3b also a
        /// Runtime-held key whose renewal found it gone (keys `lease_id`).
        LeaseMissing => "lease_missing": error,
        /// Error. An instance worker panicked again before it finished one payload after its
        /// rebuild (spec §7); the Runtime is marked fatal and actingd exits with it.
        RuntimeRestartRequired => "runtime_restart_required": error,
        /// Warning. A restart found a scheduled run with its task terminal but no
        /// `lease.released` under its run links (cut between the two) and skipped the run's
        /// settlement (model C1); recorded once per restart (keys `instance_id`).
        PolicySettlementSkippedNoRelease => "policy_settlement_skipped_no_release": warning,
    }
}
