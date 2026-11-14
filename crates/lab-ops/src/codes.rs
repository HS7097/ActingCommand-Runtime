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
        // ---- Workflow #432 AGT-T1: `agent-channel`, one code for its results and one for its
        // failures; the text of a failure is the developer line's.
        AgentChannelReported => "agent_channel_reported": success / none,
        AgentChannelFailed => "agent_channel_failed": error / none,
        // ---- end of AGT-T1
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
        // Workflow #432 AGT-1b: `agent-wake-test`, a test wake queued for the worker.
        AgentWakeTestQueued => "agent_wake_test_queued": success / none,
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
        // `watchdog_launcher_spawn_failed` is retired in 0.12.47 (Workflow #434 LAB-4): the
        // launcher runs the tick itself, so no fixed entry can fail to run.
        // ---- Workflow #378 A5 (end)
        // ---- Workflow #421 INST-0 (begin): `actingctl install`, the command-line installer
        // (contracts/install-cli.md, "Outcome codes"). Folded by ruling 54 (#420 S4): one domain
        // code per kind of outcome, the value `operation` naming the command (`InstallLocation`).
        // Kept apart: the watchdog decision row 7a″ and the fatal interrupted operation.
        InstallCommandDone => "install_command_done": success / none,
        InstallCaveatNoted => "install_caveat_noted": warning / none,
        WatchdogInstallerUnfinished => "watchdog_installer_unfinished": warning / none,
        InstallCommandRefused => "install_command_refused": error / none,
        InstallOperationInterrupted => "install_operation_interrupted": fatal / none,
        // ---- Workflow #421 INST-0 (end)
        // ---- Workflow #434 LAB-2 (pending; registered here since LAB-5G1b, actinglab's before):
        // the success of an operation of the Lab whose source has no success code of its own,
        // whichever program serves it (the core rows' serving code, `core_ops`, and actinglab's
        // own commands).
        LabOperationCompleted => "lab_operation_completed": success / none,
        // ---- end of LAB-2
        // ---- Workflow #467 RPT-1 (contracts/problem-report.md): `report export`, the archive
        // of the errors view written, its output refused inside the installation root or the
        // state root, its write failed, a part of it that could not be read, and the parts its
        // size bound left out.
        ReportExported => "report_exported": success / none,
        ReportOutputRefused => "report_output_refused": error / none,
        ReportWriteFailed => "report_write_failed": error / none,
        ReportPartMissing => "report_part_missing": warning / none,
        ReportTrimmed => "report_trimmed": warning / none,
        // ---- end of RPT-1
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
        /// Workflow #434 LAB-5G2b: a path that is not absolute where an absolute one is required
        /// (the Lab's MCP server, whose current directory means nothing to its caller).
        NotAbsolute => "not_absolute",
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

// ---- Workflow #421 INST-0 (begin): the locations and vocabularies of `actingctl install`. ----

actingcommand_contract::outcome_locations! {
    /// The commands of `actingctl install` (contracts/install-cli.md), the value `operation` of
    /// `install_command_done`, `install_command_refused`, `install_caveat_noted` and
    /// `install_operation_interrupted` (Workflow #421 INST-0, ruling 54). INST-1 runs the release,
    /// adopt, update, rollback, undo, resume, status and leftovers commands; the later slices
    /// run the rest.
    pub enum InstallLocation {
        ReleaseVerify => "install_release_verify",
        ReleaseStage => "install_release_stage",
        Adopt => "install_adopt",
        UpdatePlan => "install_update_plan",
        UpdateApply => "install_update_apply",
        RollbackPlan => "install_rollback_plan",
        RollbackApply => "install_rollback_apply",
        Undo => "install_undo",
        Resume => "install_resume",
        Status => "install_status",
        Leftovers => "install_leftovers",
        // Workflow #421 MCP-ENTRY: `actingctl mcp-entry plan` and `list`.
        McpEntryPlan => "install_mcp_entry_plan",
        McpEntryList => "install_mcp_entry_list",
        Stop => "install_stop",
        Start => "install_start",
        ReleaseHold => "install_release_hold",
        GuardsReport => "install_guards_report",
        ConfigShow => "install_config_show",
        ConfigDiff => "install_config_diff",
        ConfigSet => "install_config_set",
        ConfigBind => "install_config_bind",
        ConfigPolicy => "install_config_policy",
        ConfigLabOptions => "install_config_lab_options",
        ConfigCommitPlan => "install_config_commit_plan",
        ConfigCommitApply => "install_config_commit_apply",
        ConfigDiscard => "install_config_discard",
        ConfigRestore => "install_config_restore",
        InstanceAdd => "install_instance_add",
        InstanceRemove => "install_instance_remove",
        PackPlan => "install_pack_plan",
        PackPlace => "install_pack_place",
        PackRemovePlan => "install_pack_remove_plan",
        PackRemoveApply => "install_pack_remove_apply",
        PackRestore => "install_pack_restore",
        New => "install_new",
        // Workflow #429 R-15b (since 0.12.35): the A/B commands frozen in the contract, which
        // R-15c (prepare), R-15d (clean, restart) and R-15e (convert) build; until then an
        // earlier slice's build refuses them as unknown commands.
        Prepare => "install_prepare",
        ConvertPlan => "install_convert_plan",
        ConvertApply => "install_convert_apply",
        Clean => "install_clean",
        Restart => "install_restart",
        // Workflow #434 LAB-3 (since 0.12.48; contracts/lab-two-entries.md §7): the operations of
        // the Lab's rescue module that no `actingctl install` command has: the composites `deploy`
        // and `config replace`, `alive`, and `unlock`, `check-config` and `ledger verify`, which run
        // actingd's commands.
        Deploy => "install_deploy",
        ConfigReplace => "install_config_replace",
        Alive => "install_alive",
        Unlock => "install_unlock",
        CheckConfig => "install_check_config",
        LedgerVerify => "install_ledger_verify",
        // Workflow #434 LAB-5G5b (since 0.12.57): the rescue module's `mcp config`, the
        // registration snippet of the MCP entry's second generation (its `mcp plan` and `mcp
        // entries` answer as `install_mcp_entry_plan` and `install_mcp_entry_list`).
        McpConfig => "install_mcp_config",
        // Workflow #453 S10 (since 0.12.59): the rescue module's `emulators`, the live build's
        // `actingd discover` of the root's configuration, read only.
        Emulators => "install_emulators",
        // Workflow #453 S7 (since 0.12.60): the rescue module's `vision manifest` and `vision
        // check`, the live build's `actingd vision manifest|check`, read only.
        VisionManifest => "install_vision_manifest",
        VisionCheck => "install_vision_check",
        // Workflow #453 S9 (since 0.12.60): the rescue module's `vision place`, the files of the
        // configured vision root's manifest placed from a folder a person gives.
        VisionPlace => "install_vision_place",
        // Workflow #453 S12 (since 0.12.60): the rescue module's `state-root claim`, the live
        // build's `actingd claim-state-root` of the root's configuration while the Runtime is
        // stopped.
        StateRootClaim => "install_state_root_claim",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Why an installer command refused or failed without leaving an unfinished operation
    /// (vocabulary `install_refusal`, the value `install_refusal` of `install_command_refused`;
    /// Workflow #421 INST-0, folded by ruling 54).
    pub enum InstallRefusal: "install_refusal" {
        NotSingleLocation => "not_single_location",
        LayoutAbUnsupported => "layout_ab_unsupported",
        NotAdopted => "not_adopted",
        StateInvalid => "state_invalid",
        OperationUnfinished => "operation_unfinished",
        ForeignPart => "foreign_part",
        WriterLockFailed => "writer_lock_failed",
        ReleaseCheckFailed => "release_check_failed",
        LiveBuildInvalid => "live_build_invalid",
        LiveBuildModified => "live_build_modified",
        BuildNotKept => "build_not_kept",
        BuildInvalid => "build_invalid",
        ConfigUnreadable => "config_unreadable",
        InstallSectionInvalid => "install_section_invalid",
        ConfigRefusedByTarget => "config_refused_by_target",
        CheckConfigUnanswered => "check_config_unanswered",
        ConfigBaseChanged => "config_base_changed",
        LabOptionsLocked => "lab_options_locked",
        CatalogPlanUnexpected => "catalog_plan_unexpected",
        RuntimeRunning => "runtime_running",
        OwnerLockHeld => "owner_lock_held",
        ProcessesUnreadable => "processes_unreadable",
        RunningImageBlocks => "running_image_blocks",
        InterfaceEdge => "interface_edge",
        LedgerVerifyFailed => "ledger_verify_failed",
        LedgerHeadUnreadable => "ledger_head_unreadable",
        PlanChanged => "plan_changed",
        RollbackGateFailed => "rollback_gate_failed",
        GuardFailed => "guard_failed",
        RuntimeStartFailed => "runtime_start_failed",
        PackInvalid => "pack_invalid",
        PackBindingBroken => "pack_binding_broken",
        PackMaintenanceDrift => "pack_maintenance_drift",
        InstancesNotIdle => "instances_not_idle",
        InstanceInvalid => "instance_invalid",
        // Workflow #421 INST-2 (#421 deploy design §4, §8): no Runtime answers where the command
        // needs one (`--stop`, `release-hold`); the formal close did not complete; a fail-class
        // post-start check failed and no rollback was possible; the composite rolled back.
        RuntimeNotRunning => "runtime_not_running",
        RuntimeStopFailed => "runtime_stop_failed",
        PostCheckFailed => "post_check_failed",
        DeployRolledBack => "deploy_rolled_back",
        // Workflow #429 R-15b (since 0.12.35; design v4.1 §2.9 to §3.15, §4.3, §5.2, §5.7 and
        // §6.2): the refusals of the A/B layout, the operation attempts, the clean and the
        // conversion, registered before R-15c to R-15e emit them.
        SlotOccupied => "slot_occupied",
        SlotIsActive => "slot_is_active",
        SlotUnrecognised => "slot_unrecognised",
        SlotNotClean => "slot_not_clean",
        PreviousSlotUnconfirmed => "previous_slot_unconfirmed",
        PlatformToolsChanged => "platform_tools_changed",
        StateSelectionMismatch => "state_selection_mismatch",
        SelectionChangedExternally => "selection_changed_externally",
        SelectionCommitLost => "selection_commit_lost",
        SelectionCommitFailed => "selection_commit_failed",
        NotSameVolume => "not_same_volume",
        ReparsePoint => "reparse_point",
        RollbackMaterialUnavailable => "rollback_material_unavailable",
        OperationAttemptRunning => "operation_attempt_running",
        OperationIdUnavailable => "operation_id_unavailable",
        AttemptRecordExists => "attempt_record_exists",
        DetachSpawnFailed => "detach_spawn_failed",
        DetachIdentityMismatch => "detach_identity_mismatch",
        ClientRefUnknown => "client_ref_unknown",
        UiConfirmationStale => "ui_confirmation_stale",
        CleanIrreversible => "clean_irreversible",
        EntryModified => "entry_modified",
        ConfigCandidatePending => "config_candidate_pending",
        ConvertLayoutUnrecognised => "convert_layout_unrecognised",
        ConvertRolledBack => "convert_rolled_back",
        StateRootMismatch => "state_root_mismatch",
        StateRootPlaceholder => "state_root_placeholder",
        LayoutSingleConvertFirst => "layout_single_convert_first",
        // Workflow #434 LAB-3 fix 1 (since 0.12.48; contracts/lab-two-entries.md §5): the Lab's
        // rescue module hands back only its own hold; a reason no journal of the module recorded
        // in its pause step, a `down` reason an instance shows already, or a pause that no longer
        // shows the revision the module recorded is not its own (`hold_reason`, `aliases`).
        HoldNotOwned => "hold_not_owned",
        // Workflow #434 LAB-5G5b (since 0.12.57): the rescue module's `mcp plan` finds no
        // `actinglab-mcp.exe` in the manifests of the installed build (`commit_sha`, `path`).
        McpServerNotInBuild => "mcp_server_not_in_build",
        // Workflow #453 S10 (since 0.12.59): the rescue module's `emulators` got no completed
        // discovery from the live build's `actingd discover` (its outcome, or the child's failure,
        // is the cause; fix 1: a child ended at the row's bound has `timeout_ms` and no cause).
        EmulatorDiscoveryFailed => "emulator_discovery_failed",
        // Workflow #453 S7 (since 0.12.60; fix 1: only where a root really is not resolved): the
        // rescue module's `vision check` got the live build's `actingd vision check` refusal of
        // the configuration or of the vision root it names (actingd's outcome is the cause).
        VisionRootUnresolved => "vision_root_unresolved",
        // Workflow #453 S7 fix 1 (since 0.12.60): the rescue module's `vision manifest` or
        // `vision check` got no usable report from the live build's `actingd vision`: it did not
        // run, printed none (a build without the subcommand), refused its command line, could not
        // read its own registry, or was ended at the row's bound (`timeout_ms`, no cause).
        VisionReportUnanswered => "vision_report_unanswered",
        // Workflow #453 S4 (since 0.12.60): the rescue module's `new` builds only into an absent or
        // empty folder: its new root (`path`), and the state root it is to create (`path`), each
        // refused before anything is written when it exists and is not empty.
        RootNotEmpty => "root_not_empty",
        StateRootNotEmpty => "state_root_not_empty",
        // Workflow #453 S9 (since 0.12.60): a file the rescue module's `vision place` would place
        // does not hold its pin: below `--from` before anything is written (missing, another size
        // or another SHA-256, or unreadable), or its copy or the placed file during the operation
        // (`path` the file).
        VisionSourceInvalid => "vision_source_invalid",
        // Workflow #453 S12 (since 0.12.60): the rescue module's `state-root claim` got no report
        // from the live build's `actingd claim-state-root`: it did not run, printed none, or was
        // left running at its deadline.
        StateRootClaimUnanswered => "state_root_claim_unanswered",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// What an installer command noted while its work went on (vocabulary `install_caveat`, the
    /// value `install_caveat` of `install_caveat_noted`; Workflow #421 INST-0, folded by
    /// ruling 54).
    pub enum InstallCaveat: "install_caveat" {
        ReleaseUnconfirmed => "release_unconfirmed",
        McpClientsOnPreviousBuild => "mcp_clients_on_previous_build",
        PackDeclarationUnknown => "pack_declaration_unknown",
        PostCheckMismatch => "post_check_mismatch",
        RestartRequired => "restart_required",
        ConfigOverriddenByInstruction => "config_overridden_by_instruction",
        PackRestartNeeded => "pack_restart_needed",
        PackFullCheckSkipped => "pack_full_check_skipped",
        // Workflow #421 INST-1b-F (#421 deploy design §4, §8 S1): the rollback gate's open-time
        // part found no open of the target build on this root and used another build's.
        OpenTimeFromOtherBuild => "open_time_from_other_build",
        // Workflow #421 INST-1b-F fix round 1 (review S1 F1): the target's check-config does not
        // take `--install-root` (a build before `actingd-config` revision 17) and judged the
        // configuration from its kept folder.
        CheckConfigWithoutInstallRoot => "check_config_without_install_root",
        // Workflow #421 INST-2 (#421 deploy design §4, §7; decision 75 of 10-27 14:2x): the
        // start looks stalled (no log growth, or reads past the key); a close whose terminal
        // outcome was not `runtime_stopped` or was not found; runs the hold's drain stopped at a
        // checkpoint, whose tasks the Runtime evaluates again from their start.
        StartStalled => "start_stalled",
        StopTerminalUnconfirmed => "stop_terminal_unconfirmed",
        RunsNotContinued => "runs_not_continued",
        // Workflow #421 INST-2 fix round 1 (review S2 M6; decision 75 of 10-27): the owed idle
        // wakes of the held instances could not be read and are derived again by the Runtime;
        // (decision 75, `actingd-config` revision 18) the configuration names a retired key of
        // the `install` section, which has no effect.
        IdleWakesRederived => "idle_wakes_rederived",
        ConfigKeyRetired => "config_key_retired",
        // Workflow #429 R-15b (since 0.12.35; design v4.1 §3.3 to §3.14, §5.3 and §6.2): the
        // caveats of the A/B layout, the attempts and the clean, registered before R-15c to
        // R-15e note them.
        UiRestartRequired => "ui_restart_required",
        RuntimeOnPreviousSlot => "runtime_on_previous_slot",
        StateBehindSelection => "state_behind_selection",
        ForeignPart => "foreign_part",
        EntryModified => "entry_modified",
        InstallerNotNewest => "installer_not_newest",
        LegacyAutostart => "legacy_autostart",
        SlotLeftPrepared => "slot_left_prepared",
        OperationAttemptUnconfirmed => "operation_attempt_unconfirmed",
        OperationAttemptLost => "operation_attempt_lost",
        OperationAttemptMismatch => "operation_attempt_mismatch",
        KeptHoldMismatch => "kept_hold_mismatch",
        CleanupPending => "cleanup_pending",
        CleanupFailed => "cleanup_failed",
        PlayerHoldPersistenceOff => "player_hold_persistence_off",
        CleanupWaitingHealth => "cleanup_waiting_health",
        CleanupDeferred => "cleanup_deferred",
        PlayerHoldOfflineReadFailed => "player_hold_offline_read_failed",
        McpEntryGenerationStale => "mcp_entry_generation_stale",
        // Workflow #434 LAB-3 (since 0.12.48; contracts/lab-two-entries.md §6): in the Lab's
        // rescue module a guard that fails or is not evaluated is noted, never refused; the guards
        // as `install_guards`.
        GuardFailed => "guard_failed",
        // Workflow #434 LAB-3 fix 3 (since 0.12.48; contracts/lab-two-entries.md §5): the rescue
        // module could not read whether a hold is its own (the reason does not parse, the root or
        // its operations folder cannot be read), so its `next` leaves the `release-hold` out and
        // the answer says so (`hold_reason` when the reason parses, the failure as the cause).
        HoldOwnershipUnconfirmed => "hold_ownership_unconfirmed",
        // Workflow #434 LAB-5G5b (since 0.12.57): the rescue module's `mcp config` gave the
        // snippet of an entry program that is not placed in `mcp-entry\g2\` (`path`).
        McpEntryNotPlaced => "mcp_entry_not_placed",
        // Workflow #453 S7 (since 0.12.60): the rescue module's `vision check` found files of the
        // manifest or models that do not match (`count`).
        VisionIncomplete => "vision_incomplete",
        // Workflow #453 S12 (since 0.12.60): the rescue module's `state-root claim` replaced a
        // record of another installation, or one that could not be read: the ledger's records
        // that carry fingerprints were made with the salt of the configuration that wrote them.
        FingerprintsDiscontinuous => "fingerprints_discontinuous",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// A check of a release folder (vocabulary `release_check`, the items of `release_checks` of
    /// `install_command_refused` with `install_refusal` `release_check_failed`; Workflow #421
    /// INST-0).
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
        // Workflow #429 R-15b (since 0.12.35, A.4 b): a UI build's manifest whose
        // `installation_selection_schema` disagrees with its `install-selection` range.
        SelectionSchemaMismatch => "selection_schema_mismatch",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// A part of the rollback gate (vocabulary `rollback_gate_part`, the items of
    /// `rollback_gate_parts` of `install_command_refused` with `install_refusal`
    /// `rollback_gate_failed`; Workflow #421 INST-0).
    pub enum RollbackGatePart: "rollback_gate_part" {
        InterfaceEdges => "interface_edges",
        LedgerReadability => "ledger_readability",
        OpenTime => "open_time",
        Config => "config",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// An installer guard (vocabulary `install_guard`, the items of `install_guards` of
    /// `install_command_refused` with `install_refusal` `guard_failed`; Workflow #421 INST-0;
    /// INST-2 evaluates them).
    pub enum InstallGuard: "install_guard" {
        Memory => "memory",
        // Informational since Alice's decision 75 of 10-27: listed, never refused.
        ScheduleNear => "schedule_near",
        // Retired with `install.guard.blocked_windows` (decision 75, `actingd-config` revision
        // 18): kept registered, never evaluated.
        BlockedWindow => "blocked_window",
        ForeignPauses => "foreign_pauses",
        LadderHolding => "ladder_holding",
        Adb => "adb",
        // Workflow #429 R-15b (since 0.12.35; design v4.1 §3.6, §3.10, §3.12, §4.3, §5.2,
        // §5.3): registered before R-15d and R-15e evaluate them; `install guards` and the
        // `--ack-guard` list of this build do not take them yet.
        Disk => "disk",
        RootInventory => "root_inventory",
        InstructionsDropped => "instructions_dropped",
        SelectionExternal => "selection_external",
        PlayerHolds => "player_holds",
    }
}

// ---- Workflow #421 INST-0 (end) ----
