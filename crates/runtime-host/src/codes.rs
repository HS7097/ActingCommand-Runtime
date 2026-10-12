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
        MaintenanceArtifactMaterialUnavailable => "maintenance_artifact_material_unavailable": error / none,
        /// Warning. A material read found no file for an artifact without an eviction proof
        /// (a frame deleted by hand); the read reports `missing` and nothing is recorded
        /// (keys `artifact_id`, `event_id`, carried by the read request).
        MaterialReadMissing => "material_read_missing": warning / none,
        /// Error. A proposal's report artifact could not be read; the request is refused
        /// and the Runtime keeps running (keys `artifact_id`).
        ProposalReportUnavailable => "proposal_report_unavailable": error / none,
        /// Error. A strategic report's evidence artifact could not be read; the request is
        /// refused and the Runtime keeps running (keys `artifact_id`).
        StrategicEvidenceUnavailable => "strategic_evidence_unavailable": error / none,
        /// Warning. The frame cleaner could not delete a frame for another reason than a
        /// held or absent file; the frame stays and is tried again after a restart (keys
        /// `artifact_id`, `io_kind`, `os_error`).
        FrameRetentionRemoveFailed => "frame_retention_remove_failed": warning / none,
        /// Warning. The frame cleaner could not move a frame into its kept folder for another
        /// reason than a held or absent file, and the frame stays where it is (keys
        /// `artifact_id`, `entry`, `io_kind`, `os_error`).
        FrameRetentionMoveFailed => "frame_retention_move_failed": warning / none,
        /// Warning. A frame moved into its kept folder, but the move counter `kept/.moves`
        /// could not be written, so other processes may find moved frames only at their next
        /// walk of the kept folders, within 60 s; once per process (keys `entry`, `io_kind`,
        /// `os_error`).
        FrameRetentionCounterFailed => "frame_retention_counter_failed": warning / none,
        /// Info. A policy candidate, or its admission, met an instance that a claim holds or
        /// that an eligible claim waits for: the candidate is deferred, or its intent is
        /// rejected at Info, and the driver wakes when a key returns (keys `instance_id`,
        /// `holder_kind`, `count`).
        DispatchInstanceHeld => "dispatch_instance_held": info / none,
        /// Info. A policy admission found the instance's admission guard taken by a keyless
        /// observe or probe for its whole short retry; the intent is rejected at Info and may
        /// run again from `next_eligible_unix_ms` (keys `instance_id`,
        /// `next_eligible_unix_ms`).
        DispatchInstanceContended => "dispatch_instance_contended": info / none,
        /// Info. A policy candidate met an instance in takeover cooldown after an unclean
        /// previous owner; it is deferred with a wake at the cooldown's end (keys
        /// `instance_id`, `next_eligible_unix_ms`).
        DispatchInstanceCooldown => "dispatch_instance_cooldown": info / none,
        /// Warning. A stuck-recovery ladder no longer holds its instance's key at a holding
        /// check: it cancels its continuation, fails the current rung with this reason and
        /// finishes exhausted at warning (keys `instance_id`, `lease_id`).
        RecoveryLadderKeyLost => "recovery_ladder_key_lost": warning / none,
        /// Error. An instance worker panicked again before it finished one payload after its
        /// rebuild (spec §7); the Runtime is marked fatal and actingd exits with it.
        RuntimeRestartRequired => "runtime_restart_required": error / none,
        /// Error. A held or released start stopped before it finished preparing because the
        /// Runtime latched a failure meanwhile. Operation `install_transition`. Until A2 makes
        /// it a `caused_by` link, the latched failure travels as the tokens
        /// `cause=<code> cause_operation=<operation>` (Workflow #381 A R3′). A restart may
        /// succeed: the watchdog considers a start (R5′).
        InstallStartupStopped => "install_startup_stopped": error / none,
        /// Error. A held start ended because a shutdown request for its owner was accepted
        /// before it was released. Operation `install_transition`. A formal stop: the watchdog
        /// holds it and never restarts it (Workflow #381 A, #672 review M1).
        InstallStartupShutDown => "install_startup_shut_down": error / none,
        /// Error. A held start received no release before its deadline; the transition fails
        /// with admission closed (Workflow #352) and the start ends with it, even when nobody
        /// polls (#381 A R5′). Operation `install_transition`.
        HeldTimeout => "held_timeout": error / none,
        /// Error. A released start did not finish preparing before its deadline; the
        /// transition fails with admission closed (Workflow #352) and the start ends with it
        /// (#381 A R5′). Operation `install_transition`.
        ReleaseTimeout => "release_timeout": error / none,
        /// Info. A start found a scheduled run whose dispatch has no outcome and whose lease
        /// has no run-linked `lease.released` (cut between its task terminal and its release,
        /// mid-run, or before its first task event; its lease not transferred, expired or
        /// released without run links), wrote the missing release (effect
        /// `not_performed`) and settles the run once: from its terminal when it has one,
        /// otherwise as interrupted
        /// (`policy_settlement_interrupted`); recorded once, under the run's links (keys
        /// `instance_id`, `lease_id`, `task_id`; the run id is on the record's links).
        PolicySettlementReleaseRecovered => "policy_settlement_release_recovered": info / none,
        /// Warning. After a start's reconciliation, a scheduled dispatch is still without an
        /// outcome (its run's instance is no longer registered, or its lease ended in more than
        /// one way); recorded once per start, under the run's links (keys `instance_id`,
        /// `task_id`, and `lease_id` when the dispatch has one grant; the run id is on the
        /// record's links).
        PolicySettlementDispatchLeftOpen => "policy_settlement_dispatch_left_open": warning / none,
        /// Error. A start could not settle a scheduled run: the host's recovery or the ledger
        /// refused its settlement (keys `failure_code`, the refusal's code, and `instance_id`,
        /// `task_id`, `lease_id` when the dispatch has one grant; the run id is on the record's
        /// links). The start carries on: the dispatch stays open, out of the active workloads,
        /// and the run's instance is paused with this code as its reason.
        PolicySettlementDispatchUnsettled => "policy_settlement_dispatch_unsettled": error / none,
        /// Error. An operator emulator restart whose Stop phase was not confirmed; after a
        /// Stop whose readiness wait expired its Start still ran (Workflow #381 B F3). The
        /// request's one failure record (keys `restart_phase`, `stop_result`,
        /// `stop_elapsed_ms`, `start_dispatched`, `device_process_present`, and
        /// `start_elapsed_ms`, `process_started`, `android_started`, `adb_port_present` when
        /// known). Until A2 makes it a `caused_by` link, the Stop's control failure travels as
        /// the token `cause=<code>` (`emulator_control_wait_timeout` or
        /// `emulator_control_failed`).
        EmulatorRestartStopFailed => "emulator_restart_stop_failed": error / environment_fault,
        /// Error. An operator emulator restart whose Stop phase was confirmed and whose Start
        /// phase failed (Workflow #381 B F4); same keys, `start_elapsed_ms` always present, and
        /// the Start's control failure as the token `cause=<code>`.
        EmulatorRestartStartFailed => "emulator_restart_start_failed": error / environment_fault,
        /// Warning. A Stop confirmed by `info` alone because its process listing failed
        /// (Workflow #381 B F2, D6): once per Stop under the request's links, before
        /// `command.validated` (keys `instance_id`, `device_process_present`, and
        /// `stop_elapsed_ms`).
        EmulatorStopProcessUnverified => "emulator_stop_process_unverified": warning / none,
        /// Warning. A persistent gate (a budget, a closed activity window, a task paused by
        /// failure suspension, or the configuration) refused a due clock slot; recorded once
        /// per (task, instance, occurrence) as a `goal_missed` planning signal (keys `task_id`,
        /// `alias`, `gate_code`, `occurrence_unix_ms`, and `budget_dimension`,
        /// `window_end_unix_ms`, `next_eligible_unix_ms`, `next_occurrence_unix_ms` when known;
        /// Workflow #372 K2).
        ClockSlotRefused => "clock_slot_refused": warning / none,
        /// Info. A transient gate held a due clock slot back: an operator pause at first sight,
        /// a queue wait or an unavailable instance only once the pair's activity window has
        /// closed; recorded once per (task, instance, occurrence) as a `goal_missed` planning
        /// signal (same keys as `clock_slot_refused`; Workflow #372 K2).
        ClockSlotDeferred => "clock_slot_deferred": info / none,
        /// Info. A selected dispatch serves a clock occurrence from before its activity window
        /// opened; recorded once at the first cycle that selects it, as a `goal_missed`
        /// planning signal (keys `task_id`, `alias`, `occurrence_unix_ms`,
        /// `window_start_unix_ms`, and `next_occurrence_unix_ms`, `first_run` when known;
        /// Workflow #372 K2).
        ClockSlotOwed => "clock_slot_owed": info / none,
    }
}
