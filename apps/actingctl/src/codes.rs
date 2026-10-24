// SPDX-License-Identifier: AGPL-3.0-only

//! actingctl's outcome codes (Workflow #378 A2a). Every entry is listed in the fragment
//! `contracts/outcome-codes/actingctl.json`; the outcome guard (G1) checks both directions.
//! Codes several owners emit with one meaning are the contract's (`SharedCode`, decision D3).

actingcommand_contract::outcome_codes! {
    /// actingctl's own codes: the CLI's refusals, MCP tool errors and warnings, and the watchdog's
    /// decisions, log codes and failures (decision D7).
    pub enum ActingctlCode {
        ActingctlUsageInvalid => "actingctl_usage_invalid": error / none,
        ActingctlPackageResolutionFailed => "actingctl_package_resolution_failed": error / none,
        ActingctlFactRecordInvalid => "actingctl_fact_record_invalid": error / none,
        ActingctlOutputFailed => "actingctl_output_failed": error / none,
        ResourceTargetsFileInvalid => "resource_targets_file_invalid": error / none,
        TaskOffsetScopeAmbiguous => "task_offset_scope_ambiguous": error / none,
        ClockUnavailable => "clock_unavailable": error / none,
        ConfigFactsMissing => "config_facts_missing": error / none,
        ShutdownWaitTimeout => "shutdown_wait_timeout": error / none,
        ShutdownWaitJournalUnreadable => "shutdown_wait_journal_unreadable": error / none,
        ArgumentsInvalid => "arguments_invalid": error / none,
        TierNotEnabled => "tier_not_enabled": error / none,
        OutputBudgetExceeded => "output_budget_exceeded": error / none,
        JobCapacityExhausted => "job_capacity_exhausted": error / none,
        JobStartFailed => "job_start_failed": error / none,
        CallBudgetExhausted => "call_budget_exhausted": error / none,
        HandleUnknown => "handle_unknown": error / none,
        SinceOutOfRange => "since_out_of_range": error / none,
        ErrorsQueryInvalid => "errors_query_invalid": error / none,
        CursorInvalid => "cursor_invalid": error / none,
        CursorEncodeFailed => "cursor_encode_failed": error / none,
        InstanceAmbiguous => "instance_ambiguous": error / none,
        InstallBuildManifestUnreadable => "install_build_manifest_unreadable": warning / none,
        InstallStateRootUnresolved => "install_state_root_unresolved": error / none,
        InstallProcessRootMismatch => "install_process_root_mismatch": error / none,
        InstallProcessStateRootMismatch => "install_process_state_root_mismatch": error / none,
        InstallRootUnresolved => "install_root_unresolved": error / none,
        MaterialRefInvalid => "material_ref_invalid": error / none,
        MaterialRefUnknown => "material_ref_unknown": error / none,
        MaterialQueryInvalid => "material_query_invalid": error / none,
        MaterialNotText => "material_not_text": error / none,
        MaterialTooLarge => "material_too_large": error / none,
        MaterialExportFailed => "material_export_failed": error / none,
        RunUnknown => "run_unknown": error / none,
        ResourceTargetsDocumentTooLarge => "resource_targets_document_too_large": error / none,
        LabToolUnavailable => "lab_tool_unavailable": error / none,
        LabProcessFailed => "lab_process_failed": error / none,
        LabProcessTimeout => "lab_process_timeout": error / none,
        AnswerExported => "answer_exported": warning / none,
        ClientActionWithoutReqId => "client_action_without_req_id": warning / none,
        ClientActionNotRecorded => "client_action_not_recorded": warning / none,
        BindingRequiresUnavailable => "binding_requires_unavailable": warning / none,
        PreflightFailed => "preflight_failed": warning / none,
        CheckConfigUnavailable => "check_config_unavailable": warning / none,
        CheckConfigFailed => "check_config_failed": error / none,
        SuspendedReportReaderPanicked => "suspended_report_reader_panicked": error / none,
        SuspendedReportSpawnFailed => "suspended_report_spawn_failed": error / none,
        SuspendedReportIoFailed => "suspended_report_io_failed": error / none,
        SuspendedReportTimeout => "suspended_report_timeout": error / none,
        SuspendedReportStateRootMismatch => "suspended_report_state_root_mismatch": error / none,
        SuspendedReportUnreadable => "suspended_report_unreadable": error / none,
        McpWorkersUnavailable => "mcp_workers_unavailable": error / none,
        McpToolPanicked => "mcp_tool_panicked": error / none,
        WatchdogAlive => "watchdog_alive": info / none,
        WatchdogFormalClose => "watchdog_formal_close": info / none,
        WatchdogGrace => "watchdog_grace": info / none,
        WatchdogInstallerBusy => "watchdog_installer_busy": info / none,
        WatchdogNeverStarted => "watchdog_never_started": info / none,
        WatchdogOwnerLockHeld => "watchdog_owner_lock_held": info / none,
        WatchdogRunInProgress => "watchdog_run_in_progress": info / none,
        WatchdogRuntimeProcessPresent => "watchdog_runtime_process_present": info / none,
        WatchdogSelectionChanged => "watchdog_selection_changed": info / none,
        WatchdogStartedRuntime => "watchdog_started_runtime": info / none,
        WatchdogMisconfigured => "watchdog_misconfigured": error / none,
        WatchdogStartFailed => "watchdog_start_failed": error / none,
        WatchdogOwnerRetainedUnconfirmed => "watchdog_owner_retained_unconfirmed": error / none,
        WatchdogRuntimeProcessWithoutOwner => "watchdog_runtime_process_without_owner": error / none,
        WatchdogBudgetExhausted => "watchdog_budget_exhausted": error / none,
        WatchdogFatalHold => "watchdog_fatal_hold": fatal / none,
        WatchdogFormalStartObserved => "watchdog_formal_start_observed": info / none,
        WatchdogInstalled => "watchdog_installed": success / none,
        WatchdogUninstalled => "watchdog_uninstalled": success / none,
        WatchdogStateReset => "watchdog_state_reset": warning / none,
        WatchdogConfigUnreadable => "watchdog_config_unreadable": error / none,
        WatchdogDirectoryUnavailable => "watchdog_directory_unavailable": error / none,
        WatchdogJournalUnreadable => "watchdog_journal_unreadable": error / none,
        WatchdogJournalUnrecognised => "watchdog_journal_unrecognised": error / none,
        WatchdogLauncherMissing => "watchdog_launcher_missing": error / none,
        WatchdogLayoutUnknown => "watchdog_layout_unknown": error / none,
        WatchdogLogDirectoryUnreadable => "watchdog_log_directory_unreadable": error / none,
        WatchdogLogUnreadable => "watchdog_log_unreadable": error / none,
        WatchdogLogUnwritable => "watchdog_log_unwritable": error / none,
        WatchdogRootMismatch => "watchdog_root_mismatch": error / none,
        WatchdogRootUnavailable => "watchdog_root_unavailable": error / none,
        WatchdogRunLockFailed => "watchdog_run_lock_failed": error / none,
        WatchdogStateRootInvalid => "watchdog_state_root_invalid": error / none,
        WatchdogStateUnreadable => "watchdog_state_unreadable": error / none,
        WatchdogStateUnwritable => "watchdog_state_unwritable": error / none,
        WatchdogTaskDeleteFailed => "watchdog_task_delete_failed": error / none,
        WatchdogTaskMismatch => "watchdog_task_mismatch": error / none,
        WatchdogTaskQueryFailed => "watchdog_task_query_failed": error / none,
        WatchdogTaskRegisterFailed => "watchdog_task_register_failed": error / none,
        WatchdogUserUnavailable => "watchdog_user_unavailable": error / none,
        WatchdogWmiPathUnsafe => "watchdog_wmi_path_unsafe": error / none,
        WatchdogWriterLockFailed => "watchdog_writer_lock_failed": error / none,
        EntryMissing => "entry_missing": error / none,
        LogCreateFailed => "log_create_failed": error / none,
        SpawnFailed => "spawn_failed": error / none,
        WmiStartFailed => "wmi_start_failed": error / none,
        ExitedDuringStartup => "exited_during_startup": error / none,
        StartTimeout => "start_timeout": error / none,
        ProcessProbeFailed => "process_probe_failed": error / none,
        // ---- #389 U-0 registrations (pending), emitted from U-3
        // (contracts/runtime-interface.md, "Outcome codes").
        LabToolOptionDisabled => "lab_tool_option_disabled": error / none,
        McpTierRetired => "mcp_tier_retired": warning / none,
        // ---- end of #389 U-0 registrations
        // Workflow #389 MCP-10: `actingctl start` and `actingctl restart`.
        StartRuntimeAlreadyRunning => "start_runtime_already_running": error / none,
        StartWaitIdleExpired => "start_wait_idle_expired": error / none,
        // ---- Workflow #389 MCP-11: `packs` / `ac_packs`.
        PackageBindingUnreadable => "package_binding_unreadable": warning / none,
        // ---- end of MCP-11
        // ---- Workflow #389 MCP-9a: `overview` / `ac_overview` (command table §4.10).
        ConfigGenerationMismatch => "config_generation_mismatch": warning / none,
        // ---- end of MCP-9a
        // ---- Workflow #389 MCP-14: `codes` / `ac_outcome_code` (command table §4.15).
        // Its refusals outcome_code_unknown and outcome_catalog_unreadable are the contract's
        // (`SharedCode`, #378 A8b): actingledger codes emits them too.
        OutcomeCodesReported => "outcome_codes_reported": success / none,
        // ---- end of MCP-14
        // ---- Workflow #389 MCP-6: `run evidence` / `ac_run_evidence` (command table §4.7).
        RunEvidenceReported => "run_evidence_reported": success / none,
        // ---- end of MCP-6
        // ---- Workflow #398 RD-1c: `devices list|bind|unbind` / `ac_devices`,
        // `ac_device_bind`, `ac_device_unbind` (model-398 §3.3, §10.1).
        DeviceListReported => "device_list_reported": success / none,
        DeviceBindingChanged => "device_binding_changed": success / none,
        DeviceHandleUnknown => "device_handle_unknown": error / none,
        DeviceListFailed => "device_list_failed": error / none,
        DeviceKindUnsupported => "device_kind_unsupported": error / none,
        DeviceBindingsInvalid => "device_bindings_invalid": error / none,
        DeviceBindingsWriteFailed => "device_bindings_write_failed": error / none,
        // ---- end of RD-1c
        // ---- Workflow #410 item 6: `capability-gap` / `ac_capability_gap`.
        ActingctlCapabilityGapRecorded => "actingctl_capability_gap_recorded": success / none,
        ActingctlCapabilityGapWriteFailed => "actingctl_capability_gap_write_failed": error / none,
        // ---- end of #410 item 6
        // ---- Workflow #378 A5 (begin): the CLI's success codes (model §7.3), the MCP
        // transport and Lab codes, and the watchdog's report v2 and launcher codes (§7.5).
        AgentFactsPublished => "agent_facts_published": success / none,
        ResourceTargetsApplied => "resource_targets_applied": success / none,
        TaskOffsetPublished => "task_offset_published": success / none,
        RuntimeShutdownRequested => "runtime_shutdown_requested": success / none,
        RuntimeShutdownCompleted => "runtime_shutdown_completed": success / none,
        InstallTransitionReported => "install_transition_reported": success / none,
        InstallTransitionFailed => "install_transition_failed": error / none,
        InstallTransitionAborted => "install_transition_aborted": error / none,
        InstanceResetDone => "instance_reset_done": success / none,
        InstanceObserved => "instance_observed": success / none,
        RuntimeStatusReported => "runtime_status_reported": success / none,
        RuntimeConfigReported => "runtime_config_reported": success / none,
        ConfigSettingsChanged => "config_settings_changed": success / none,
        RuntimeFactsReported => "runtime_facts_reported": success / none,
        MonitorStatusReported => "monitor_status_reported": success / none,
        MonitorPolicySet => "monitor_policy_set": success / none,
        MonitorPolicyCleared => "monitor_policy_cleared": success / none,
        InstanceStatusReported => "instance_status_reported": success / none,
        InstanceEmulatorControlled => "instance_emulator_controlled": success / none,
        InstancesDiscovered => "instances_discovered": success / none,
        RuntimeSchedulingPaused => "runtime_scheduling_paused": success / none,
        RuntimeSchedulingResumed => "runtime_scheduling_resumed": success / none,
        InstanceSelfCheckReported => "instance_self_check_reported": success / none,
        InstanceSelfCheckFailed => "instance_self_check_failed": error / none,
        InstanceFramesCaptured => "instance_frames_captured": success / none,
        TaskRunReported => "task_run_reported": success / none,
        RuntimeEventsReported => "runtime_events_reported": success / none,
        InstanceFrameExported => "instance_frame_exported": success / none,
        RunListReported => "run_list_reported": success / none,
        // Workflow #389 MCP-4: `facts --instance`.
        InstanceFactsReported => "instance_facts_reported": success / none,
        RuntimeDiagnosisReported => "runtime_diagnosis_reported": success / none,
        // Workflow #389 MCP-8: `overview`.
        RuntimeOverviewReported => "runtime_overview_reported": success / none,
        // Workflow #390 DRILL-1: `ladder-drill`, a drill of an instance's ladder admitted.
        RecoveryLadderDrillQueued => "recovery_ladder_drill_queued": success / none,
        // Workflow #385 P-m: `perf-status`.
        RuntimePerformanceReported => "runtime_performance_reported": success / none,
        // Workflow #384 D2-S5: `fact-refresh`.
        RuntimeFactRefreshReported => "runtime_fact_refresh_reported": success / none,
        // Workflow #386 SR-6b, MCP-19 (registered by MCP-19a, emitted from MCP-19b on):
        // `schedule`.
        RuntimeScheduleReported => "runtime_schedule_reported": success / none,
        PackageBindingsReported => "package_bindings_reported": success / none,
        StartRuntimeStarted => "start_runtime_started": success / none,
        StartRuntimeRestarted => "start_runtime_restarted": success / none,
        StartRuntimeRefused => "start_runtime_refused": error / none,
        MaterialUnavailable => "material_unavailable": error / none,
        SuspendedReportFailed => "suspended_report_failed": error / none,
        LabToolFailed => "lab_tool_failed": error / none,
        McpParamsInvalid => "mcp_params_invalid": error / none,
        McpMessageMalformed => "mcp_message_malformed": error / none,
        McpMessageTooLarge => "mcp_message_too_large": error / none,
        McpBatchEmpty => "mcp_batch_empty": error / none,
        McpBatchUnsupported => "mcp_batch_unsupported": error / none,
        McpRequestInvalid => "mcp_request_invalid": error / none,
        McpMethodUnknown => "mcp_method_unknown": error / none,
        McpProtocolVersionUnsupported => "mcp_protocol_version_unsupported": error / none,
        McpProtocolVersionMissing => "mcp_protocol_version_missing": error / none,
        McpRequestIdInUse => "mcp_request_id_in_use": error / none,
        McpServerStartFailed => "mcp_server_start_failed": error / none,
        McpInputFailed => "mcp_input_failed": error / none,
        McpFileTooLarge => "mcp_file_too_large": error / none,
        McpConfigProgramUnresolved => "mcp_config_program_unresolved": error / none,
        McpConfigProgramUnsupported => "mcp_config_program_unsupported": error / none,
        McpIdentityCardRefused => "mcp_identity_card_refused": warning / none,
        WatchdogStartDue => "watchdog_start_due": info / none,
        WatchdogStartPending => "watchdog_start_pending": info / none,
        WatchdogOwnerLockUnanswered => "watchdog_owner_lock_unanswered": warning / none,
        WatchdogFormalCloseUnlogged => "watchdog_formal_close_unlogged": warning / none,
        WatchdogLastStartFailed => "watchdog_last_start_failed": warning / none,
        WatchdogRestartsRepeated => "watchdog_restarts_repeated": warning / none,
        WatchdogTaskMissing => "watchdog_task_missing": warning / none,
        WatchdogInstallRefused => "watchdog_install_refused": error / none,
        WatchdogLauncherMisplaced => "watchdog_launcher_misplaced": error / none,
        WatchdogLauncherLogUnavailable => "watchdog_launcher_log_unavailable": error / none,
        WatchdogLauncherWriterLockFailed => "watchdog_launcher_writer_lock_failed": error / none,
        WatchdogLauncherSpawnFailed => "watchdog_launcher_spawn_failed": error / none,
        // ---- Workflow #378 A5 (end)
        // ---- Workflow #421 INST-0 (begin): `actingctl install`, the command-line installer
        // (contracts/install-cli.md, "Outcome codes"). INST-1 emits the release, adopt, update,
        // rollback, journal and watchdog codes; the stop, start, guard, configuration, pack,
        // instance and fresh-root codes are emitted from the slices INST-2 to INST-6.
        InstallReleaseVerified => "install_release_verified": success / none,
        InstallBuildStaged => "install_build_staged": success / none,
        InstallRootAdopted => "install_root_adopted": success / none,
        InstallUpdatePlanned => "install_update_planned": success / none,
        InstallUpdateApplied => "install_update_applied": success / none,
        InstallUpdateUnchanged => "install_update_unchanged": success / none,
        InstallRollbackPlanned => "install_rollback_planned": success / none,
        InstallRollbackApplied => "install_rollback_applied": success / none,
        InstallOperationResumed => "install_operation_resumed": success / none,
        InstallOperationUndone => "install_operation_undone": success / none,
        InstallRuntimeStopped => "install_runtime_stopped": success / none,
        InstallRuntimeStarted => "install_runtime_started": success / none,
        InstallHoldReleased => "install_hold_released": success / none,
        InstallGuardsReported => "install_guards_reported": success / none,
        InstallConfigStaged => "install_config_staged": success / none,
        InstallConfigPlanned => "install_config_planned": success / none,
        InstallConfigCommitted => "install_config_committed": success / none,
        InstallConfigRestored => "install_config_restored": success / none,
        InstallPackPlanned => "install_pack_planned": success / none,
        InstallPackPlaced => "install_pack_placed": success / none,
        InstallPackRemoved => "install_pack_removed": success / none,
        InstallPackRestored => "install_pack_restored": success / none,
        InstallInstanceStaged => "install_instance_staged": success / none,
        InstallRootCreated => "install_root_created": success / none,
        InstallStatusReported => "install_status_reported": success / none,
        InstallLeftoversReported => "install_leftovers_reported": success / none,
        InstallRestartRequired => "install_restart_required": info / none,
        InstallPackFullCheckUnavailable => "install_pack_full_check_unavailable": info / none,
        InstallReleaseVerifiedLocal => "install_release_verified_local": warning / none,
        InstallMcpClientsOnPreviousBuild => "install_mcp_clients_on_previous_build": warning / none,
        InstallPostCheckFailed => "install_post_check_failed": warning / none,
        InstallPackDeclarationUnknown => "install_pack_declaration_unknown": warning / none,
        InstallPackRestartNeeded => "install_pack_restart_needed": warning / none,
        InstallConfigOverriddenByInstruction => "install_config_overridden_by_instruction": warning / none,
        WatchdogInstallerUnfinished => "watchdog_installer_unfinished": warning / none,
        InstallReleaseRefused => "install_release_refused": error / none,
        InstallUpdateRefused => "install_update_refused": error / none,
        InstallRollbackRefused => "install_rollback_refused": error / none,
        InstallRollbackLedgerUnreadable => "install_rollback_ledger_unreadable": error / none,
        InstallPlanChanged => "install_plan_changed": error / none,
        InstallGuardFailed => "install_guard_failed": error / none,
        InstallOperationUnfinished => "install_operation_unfinished": error / none,
        InstallForeignPart => "install_foreign_part": error / none,
        InstallRunningImageBlocks => "install_running_image_blocks": error / none,
        InstallLiveBuildModified => "install_live_build_modified": error / none,
        InstallLayoutAbUnsupported => "install_layout_ab_unsupported": error / none,
        InstallConfigRefused => "install_config_refused": error / none,
        InstallConfigBaseChanged => "install_config_base_changed": error / none,
        InstallConfigLabOptionsLocked => "install_config_lab_options_locked": error / none,
        InstallCatalogPlanUnexpected => "install_catalog_plan_unexpected": error / none,
        InstallPackRefused => "install_pack_refused": error / none,
        InstallPackBindingBroken => "install_pack_binding_broken": error / none,
        InstallPackMaintenanceDrift => "install_pack_maintenance_drift": error / none,
        InstallPackNotIdle => "install_pack_not_idle": error / none,
        InstallInstanceRefused => "install_instance_refused": error / none,
        InstallRuntimeStartFailed => "install_runtime_start_failed": error / none,
        InstallRuntimeStartTimeout => "install_runtime_start_timeout": error / none,
        InstallOperationInterrupted => "install_operation_interrupted": fatal / none,
        // ---- Workflow #421 INST-0 (end)
    }
}

// ---- Workflow #378 A5 (begin): the vocabularies actingctl's outcomes carry. ----

actingcommand_contract::outcome_vocabulary! {
    /// What is wrong with one command-line flag or MCP tool argument (vocabulary
    /// `argument_problem`, the value `argument_problem` of `arguments_invalid` and
    /// `actingctl_usage_invalid`; Workflow #378 A5).
    pub enum ArgumentProblem: "argument_problem" {
        /// Not a flag of the command or an argument of the tool.
        Unknown => "unknown",
        /// A required flag or argument is absent.
        Required => "required",
        /// Given more than once.
        Repeated => "repeated",
        /// A flag without its value.
        ValueMissing => "value_missing",
        /// A command-line word that is not UTF-8.
        NotUtf8 => "not_utf8",
        /// Not of the type it must have (a string, a number, a boolean, an object, a list).
        WrongType => "wrong_type",
        /// A number or a length outside its range (`range_min`, `range_max`).
        OutOfRange => "out_of_range",
        /// Text that is none of the spellings or forms it may take.
        NotRecognised => "not_recognised",
        /// Excludes, or needs, another flag or argument that was given (or not).
        Conflicting => "conflicting",
        /// Names something that cannot be used: unreadable, not of its kind, or refused.
        Unusable => "unusable",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// The state of a Runtime material read (vocabulary `material_state`; the contract's
    /// `RuntimeMaterialReadState`, Workflow #378 A5).
    pub enum MaterialState: "material_state" {
        Verified => "verified",
        NotProvided => "not_provided",
        Missing => "missing",
        IntegrityFailed => "integrity_failed",
        SourceIncomplete => "source_incomplete",
        RequestDenied => "request_denied",
        ReadFailed => "read_failed",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Why a Runtime material read stopped short (vocabulary `material_limit`; the contract's
    /// `RuntimeMaterialReadLimit`, Workflow #378 A5).
    pub enum MaterialLimit: "material_limit" {
        PendingEviction => "pending_eviction",
        Evicted => "evicted",
        EvictionFailed => "eviction_failed",
        BudgetExceeded => "budget_exceeded",
        ReplyBytes => "reply_bytes",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// The state of a contained run as its ledger events show it (vocabulary `run_state`; the
    /// runtime client's `ContainedRunState`, Workflow #378 A5).
    pub enum RunState: "run_state" {
        NotFound => "not_found",
        Admitted => "admitted",
        Running => "running",
        Succeeded => "succeeded",
        Failed => "failed",
        Cancelled => "cancelled",
        InterruptedUnterminated => "interrupted_unterminated",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Why the Runtime refused a resource-targets document (vocabulary
    /// `resource_targets_rejection_reason`; the contract's `ResourceTargetsRejectionReason`,
    /// Workflow #378 A5).
    pub enum ResourceTargetsRejectionReason: "resource_targets_rejection_reason" {
        InvalidJson => "invalid_json",
        DuplicateKey => "duplicate_key",
        UnknownField => "unknown_field",
        MissingField => "missing_field",
        InvalidType => "invalid_type",
        InvalidValue => "invalid_value",
        OutOfRange => "out_of_range",
        UnsupportedSchemaVersion => "unsupported_schema_version",
        DuplicateId => "duplicate_id",
        UnknownInstance => "unknown_instance",
        ValidityOutOfRange => "validity_out_of_range",
        CatalogUnavailable => "catalog_unavailable",
        UnknownResource => "unknown_resource",
        ResourceNotObservable => "resource_not_observable",
        ResourceOutOfScope => "resource_out_of_scope",
        UnknownTask => "unknown_task",
        TaskOutOfScope => "task_out_of_scope",
        TaskDisabled => "task_disabled",
        UnmappedTask => "unmapped_task",
        DuplicateTask => "duplicate_task",
    }
}

impl RunState {
    /// The token of a contained run's state.
    pub fn of(state: actingcommand_runtime_client::ContainedRunState) -> Self {
        use actingcommand_runtime_client::ContainedRunState as State;
        match state {
            State::NotFound => Self::NotFound,
            State::Admitted => Self::Admitted,
            State::Running => Self::Running,
            State::Succeeded => Self::Succeeded,
            State::Failed => Self::Failed,
            State::Cancelled => Self::Cancelled,
            State::InterruptedUnterminated => Self::InterruptedUnterminated,
        }
    }
}

impl ResourceTargetsRejectionReason {
    /// The token of a resource-targets document rejection reason.
    pub fn of(reason: actingcommand_contract::ResourceTargetsRejectionReason) -> Self {
        use actingcommand_contract::ResourceTargetsRejectionReason as Reason;
        match reason {
            Reason::InvalidJson => Self::InvalidJson,
            Reason::DuplicateKey => Self::DuplicateKey,
            Reason::UnknownField => Self::UnknownField,
            Reason::MissingField => Self::MissingField,
            Reason::InvalidType => Self::InvalidType,
            Reason::InvalidValue => Self::InvalidValue,
            Reason::OutOfRange => Self::OutOfRange,
            Reason::UnsupportedSchemaVersion => Self::UnsupportedSchemaVersion,
            Reason::DuplicateId => Self::DuplicateId,
            Reason::UnknownInstance => Self::UnknownInstance,
            Reason::ValidityOutOfRange => Self::ValidityOutOfRange,
            Reason::CatalogUnavailable => Self::CatalogUnavailable,
            Reason::UnknownResource => Self::UnknownResource,
            Reason::ResourceNotObservable => Self::ResourceNotObservable,
            Reason::ResourceOutOfScope => Self::ResourceOutOfScope,
            Reason::UnknownTask => Self::UnknownTask,
            Reason::TaskOutOfScope => Self::TaskOutOfScope,
            Reason::TaskDisabled => Self::TaskDisabled,
            Reason::UnmappedTask => Self::UnmappedTask,
            Reason::DuplicateTask => Self::DuplicateTask,
        }
    }
}

impl MaterialState {
    /// The token of a material read state.
    pub fn of(state: actingcommand_contract::RuntimeMaterialReadState) -> Self {
        use actingcommand_contract::RuntimeMaterialReadState as State;
        match state {
            State::Verified => Self::Verified,
            State::NotProvided => Self::NotProvided,
            State::Missing => Self::Missing,
            State::IntegrityFailed => Self::IntegrityFailed,
            State::SourceIncomplete => Self::SourceIncomplete,
            State::RequestDenied => Self::RequestDenied,
            State::ReadFailed => Self::ReadFailed,
        }
    }
}

impl MaterialLimit {
    /// The token of a material read limit.
    pub fn of(limit: actingcommand_contract::RuntimeMaterialReadLimit) -> Self {
        use actingcommand_contract::RuntimeMaterialReadLimit as Limit;
        match limit {
            Limit::PendingEviction => Self::PendingEviction,
            Limit::Evicted => Self::Evicted,
            Limit::EvictionFailed => Self::EvictionFailed,
            Limit::BudgetExceeded => Self::BudgetExceeded,
            Limit::ReplyBytes => Self::ReplyBytes,
        }
    }
}

// ---- Workflow #378 A5 (end) ----

// ---- Workflow #421 INST-0 (begin): the vocabularies of `actingctl install`. ----

actingcommand_contract::outcome_vocabulary! {
    /// Why an installer update or rollback refused before any change (vocabulary
    /// `install_refusal`, the value of `install_update_refused`; Workflow #421 INST-0).
    pub enum InstallRefusal: "install_refusal" {
        NotSingleLocation => "not_single_location",
        NotAdopted => "not_adopted",
        StateInvalid => "state_invalid",
        LiveBuildInvalid => "live_build_invalid",
        BuildNotKept => "build_not_kept",
        BuildInvalid => "build_invalid",
        RuntimeRunning => "runtime_running",
        OwnerLockHeld => "owner_lock_held",
        ProcessesUnreadable => "processes_unreadable",
        InterfaceEdge => "interface_edge",
        ConfigRefusedByTarget => "config_refused_by_target",
        CheckConfigUnanswered => "check_config_unanswered",
        LedgerVerifyFailed => "ledger_verify_failed",
        LedgerHeadUnreadable => "ledger_head_unreadable",
        WriterLockFailed => "writer_lock_failed",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// A check of a release folder (vocabulary `release_check`, the items of
    /// `release_checks` of `install_release_refused`; Workflow #421 INST-0).
    pub enum ReleaseCheck: "release_check" {
        ZipSet => "zip_set",
        SumsMissing => "sums_missing",
        SumsMismatch => "sums_mismatch",
        FactsMissing => "facts_missing",
        FactsInvalid => "facts_invalid",
        DigestMismatch => "digest_mismatch",
        FactsCommitMismatch => "facts_commit_mismatch",
        TagMismatch => "tag_mismatch",
        CommitPrefixMismatch => "commit_prefix_mismatch",
        ZipUnreadable => "zip_unreadable",
        ManifestInvalid => "manifest_invalid",
        ManifestLayout => "manifest_layout",
        ManifestRepository => "manifest_repository",
        ManifestCommit => "manifest_commit",
        FileSet => "file_set",
        FileMismatch => "file_mismatch",
        InterfacesInvalid => "interfaces_invalid",
        InterfacesDiffer => "interfaces_differ",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// A part of the rollback gate (vocabulary `rollback_gate_part`, the items of
    /// `rollback_gate_parts` of `install_rollback_refused`; Workflow #421 INST-0).
    pub enum RollbackGatePart: "rollback_gate_part" {
        InterfaceEdges => "interface_edges",
        LedgerReadability => "ledger_readability",
        OpenTime => "open_time",
        Config => "config",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// An installer guard (vocabulary `install_guard`, the items of `install_guards`
    /// of `install_guard_failed`; Workflow #421 INST-0; INST-2 evaluates them).
    pub enum InstallGuard: "install_guard" {
        Memory => "memory",
        ScheduleNear => "schedule_near",
        BlockedWindow => "blocked_window",
        ForeignPauses => "foreign_pauses",
        LadderHolding => "ladder_holding",
        Adb => "adb",
    }
}

// ---- Workflow #421 INST-0 (end) ----
