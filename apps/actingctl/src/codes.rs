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
        OutcomeCodesReported => "outcome_codes_reported": success / none,
        OutcomeCodeUnknown => "outcome_code_unknown": error / none,
        OutcomeCatalogUnreadable => "outcome_catalog_unreadable": error / none,
        // ---- end of MCP-14
        // ---- Workflow #389 MCP-6: `run evidence` / `ac_run_evidence` (command table §4.7).
        RunEvidenceReported => "run_evidence_reported": success / none,
        // ---- end of MCP-6
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
