// SPDX-License-Identifier: AGPL-3.0-only

//! actingd's outcome codes and locations (Workflow #378 A2a). Every entry is listed in the
//! fragment `contracts/outcome-codes/actingd.json`; the outcome guard (G1) checks both directions.
//! Codes several owners emit with one meaning are the contract's (`SharedCode`, decision D3); the
//! vision listing codes are vision-ffi's (`VisionListingCode`, decision D10).

actingcommand_contract::outcome_codes! {
    /// actingd's own codes: configuration refusals, provider startup, its subcommands and the
    /// policy driver.
    pub enum ActingdCode {
        ConfigUnavailable => "config_unavailable": error / none,
        ConfigSizeInvalid => "config_size_invalid": error / none,
        InstallProviderRetired => "install_provider_retired": error / none,
        MaintenanceConfigInvalid => "maintenance_config_invalid": error / none,
        GovernanceCapabilityRetired => "governance_capability_retired": error / none,
        VisionProviderManifestRetired => "vision_provider_manifest_retired": error / none,
        ConfigInvalid => "config_invalid": error / none,
        BindHostInvalid => "bind_host_invalid": error / none,
        BindHostNotLoopback => "bind_host_not_loopback": error / none,
        MumuRootInvalid => "mumu_root_invalid": error / none,
        InvalidPressureSamples => "invalid_pressure_samples": error / none,
        StuckRecoveryCooldownInvalid => "stuck_recovery_cooldown_invalid": error / none,
        InstanceConfigInvalid => "instance_config_invalid": error / none,
        DevicePathInvalid => "device_path_invalid": error / none,
        GovernanceAllowedClientsInvalid => "governance_allowed_clients_invalid": error / none,
        VisionConfigInvalid => "vision_config_invalid": error / none,
        VisionRootInProgramSlot => "vision_root_in_program_slot": error / none,
        VisionRuntimeUnavailable => "vision_runtime_unavailable": error / none,
        CatalogTransitionKindUnknown => "catalog_transition_kind_unknown": error / none,
        CatalogTransitionInvalid => "catalog_transition_invalid": error / none,
        StartupPackagePathInvalid => "startup_package_path_invalid": error / none,
        StartupPackageInvalid => "startup_package_invalid": error / none,
        ResourcePackageMissing => "resource_package_missing": error / none,
        ResourcePackageInvalid => "resource_package_invalid": error / none,
        PrerequisitePackagesSizeInvalid => "prerequisite_packages_size_invalid": error / none,
        PrerequisitePackageIdInvalid => "prerequisite_package_id_invalid": error / none,
        PrerequisitePackageUnavailable => "prerequisite_package_unavailable": error / none,
        PrerequisitePackageNotRegular => "prerequisite_package_not_regular": error / none,
        PrerequisitePackageContainerUnsupported => "prerequisite_package_container_unsupported": error / none,
        PrerequisitePackageRequestInvalid => "prerequisite_package_request_invalid": error / none,
        PrerequisitePackageDuplicate => "prerequisite_package_duplicate": error / none,
        ReturnHomePackagesSizeInvalid => "return_home_packages_size_invalid": error / none,
        ReturnHomePackageKeyInvalid => "return_home_package_key_invalid": error / none,
        ReturnHomePackageDuplicate => "return_home_package_duplicate": error / none,
        ProcedureManifestSizeInvalid => "procedure_manifest_size_invalid": error / none,
        ProcedureTaskDuplicate => "procedure_task_duplicate": error / none,
        ProcedureManifestInvalid => "procedure_manifest_invalid": error / none,
        ProcedureBindingSizeInvalid => "procedure_binding_size_invalid": error / none,
        ProcedureBindingInvalid => "procedure_binding_invalid": error / none,
        ProcedurePackagePathMissing => "procedure_package_path_missing": error / none,
        ProcedurePackageUnavailable => "procedure_package_unavailable": error / none,
        ProcedurePackageNotRegular => "procedure_package_not_regular": error / none,
        ProcedurePackageContainerUnsupported => "procedure_package_container_unsupported": error / none,
        ProcedureTaskRequestInvalid => "procedure_task_request_invalid": error / none,
        PolicyCatalogApprovalMismatch => "policy_catalog_approval_mismatch": error / none,
        PolicyCatalogSizeInvalid => "policy_catalog_size_invalid": error / none,
        PolicyCatalogPathInvalid => "policy_catalog_path_invalid": error / none,
        PolicyCatalogDocumentSizeInvalid => "policy_catalog_document_size_invalid": error / none,
        PolicyCatalogReadFailed => "policy_catalog_read_failed": error / none,
        PolicyInstanceSetMismatch => "policy_instance_set_mismatch": error / none,
        PolicyInstanceHostUnknown => "policy_instance_host_unknown": error / none,
        ScheduledExecutionBindingMissing => "scheduled_execution_binding_missing": error / none,
        ScheduledExecutionInstanceUnknown => "scheduled_execution_instance_unknown": error / none,
        ScheduledExecutionBackendModeMismatch => "scheduled_execution_backend_mode_mismatch": error / none,
        ScheduledExecutionBackendModeUnavailable => "scheduled_execution_backend_mode_unavailable": error / none,
        InstanceBindingKeyInvalid => "instance_binding_key_invalid": error / none,
        ApplicationIdentityMissing => "application_identity_missing": error / none,
        TouchBackendInvalid => "touch_backend_invalid": error / none,
        CaptureBackendInvalid => "capture_backend_invalid": error / none,
        TouchBackendMustBeExplicit => "touch_backend_must_be_explicit": error / none,
        CaptureBackendMustBeExplicit => "capture_backend_must_be_explicit": error / none,
        /// #398 R1 §5.1 (RD-3i), §3.1 (RD-1b): an instance's device fields do not fit its device
        /// kind, for example `capture_backend: scrcpy` on an emulator or `host` on a handset
        /// (`alias`, `device_config_problem`).
        DeviceConfigInvalid => "device_config_invalid": error / none,
        /// #398 R1 §3.6 (RD-1b): the private binding file binds no device to a handset instance
        /// (`alias`). check-config refuses; a start registers the instance unavailable.
        DeviceBindingMissing => "device_binding_missing": error / none,
        /// #398 R1 §3.6 (RD-1b): the private binding file cannot be read or breaks its rules
        /// (`path`, `device_binding_problem`); the start is refused.
        DeviceBindingFileInvalid => "device_binding_file_invalid": error / none,
        /// #398 R1 §3.2 (RD-1b): the private binding file binds an alias no handset instance of
        /// the configuration has (`alias`); a check-config note, not a failure.
        DeviceBindingUnused => "device_binding_unused": info / none,
        /// Workflow #369 S6a: an instance names `start_emulator: true` but has no emulator the
        /// daemon can start: a fixture, or an instance not bound by discovery (`alias`).
        StartEmulatorRequiresDiscoveryBinding => "start_emulator_requires_discovery_binding": error / none,
        NemuAppIndexInvalid => "nemu_app_index_invalid": error / none,
        NemuPairedInputConfigurationMissing => "nemu_paired_input_configuration_missing": error / none,
        InstanceRegistrationInvalid => "instance_registration_invalid": error / none,
        FixtureDeviceFieldsForbidden => "fixture_device_fields_forbidden": error / none,
        FixtureBackendMissing => "fixture_backend_missing": error / none,
        FixtureBackendInvalid => "fixture_backend_invalid": error / none,
        FixtureFrameSizeInvalid => "fixture_frame_size_invalid": error / none,
        FixtureResidentSizeInvalid => "fixture_resident_size_invalid": error / none,
        FixtureFrameInvalid => "fixture_frame_invalid": error / none,
        TimeoutInvalid => "timeout_invalid": error / none,
        ConfigManifestIncomplete => "config_manifest_incomplete": error / none,
        ConfigManifestInvalid => "config_manifest_invalid": error / none,
        ConfigManifestValueOutOfRange => "config_manifest_value_out_of_range": error / none,
        EnvOverrideIgnored => "env_override_ignored": warning / none,
        // Workflow #378 A6a (model section 2.3): a Tools file of the installation missing or
        // mismatched is a damaged installation, which only an external fix repairs: fatal.
        AdbInstallMissing => "adb_install_missing": fatal / none,
        AdbInstallMismatch => "adb_install_mismatch": fatal / none,
        /// Workflow #378 A6a: the platform-tools pin this build carries cannot be read (keys
        /// `path`); a build defect, fatal.
        AdbInstallPinInvalid => "adb_install_pin_invalid": fatal / none,
        EmulatorCapabilityAdmissionRefused => "emulator_capability_admission_refused": error / none,
        InstanceDiscoveryNoMatch => "instance_discovery_no_match": error / none,
        InstanceDiscoveryAmbiguous => "instance_discovery_ambiguous": error / none,
        InstanceDiscoveryConflict => "instance_discovery_conflict": error / none,
        VisionProviderUnavailable => "vision_provider_unavailable": error / none,
        VisionProviderPathEncodingInvalid => "vision_provider_path_encoding_invalid": error / none,
        AdbPathConflict => "adb_path_conflict": error / none,
        HostConflict => "host_conflict": error / none,
        PortConflict => "port_conflict": error / none,
        PortUnverifiable => "port_unverifiable": error / none,
        AdbPathEncodingInvalid => "adb_path_encoding_invalid": error / none,
        PathEncodingInvalid => "path_encoding_invalid": error / none,
        CheckConfigUsageInvalid => "check_config_usage_invalid": error / none,
        CheckConfigOptionInvalid => "check_config_option_invalid": error / none,
        CheckConfigConfigMissing => "check_config_config_missing": error / none,
        CheckConfigReportEncodeFailed => "check_config_report_encode_failed": error / none,
        MaintenanceUsageInvalid => "maintenance_usage_invalid": error / none,
        MaintenanceOperationInvalid => "maintenance_operation_invalid": error / none,
        MaintenanceOptionInvalid => "maintenance_option_invalid": error / none,
        MaintenanceConfigMissing => "maintenance_config_missing": error / none,
        MaintenanceReceiptEncodeFailed => "maintenance_receipt_encode_failed": error / none,
        LedgerMaintenanceFailed => "ledger_maintenance_failed": error / none,
        // ---- Workflow #378 A6a (model sections 7.4, 8.2): actingd's daemon lines and the
        // success of each subcommand report.
        /// Success. The Runtime started and is ready (keys `pid`).
        RuntimeReady => "runtime_ready": success / none,
        /// Info. The Runtime stopped; on a requested stop this is the terminal outcome, else
        /// the stopping condition follows it.
        RuntimeStopped => "runtime_stopped": info / none,
        /// Warning. A start refusal could not be appended to the fallback log (keys `path`,
        /// `variable` when `LOCALAPPDATA` is not set); a developer line only.
        FallbackLogFailed => "actingd_fallback_log_failed": warning / none,
        /// Success. check-config passed; what it noted are `note` links.
        CheckConfigPassed => "check_config_passed": success / none,
        /// Success. A ledger-maintenance pass completed.
        LedgerMaintenanceCompleted => "ledger_maintenance_completed": success / none,
        /// Success. unlock-owner recorded the confirmation.
        UnlockOwnerCompleted => "unlock_owner_completed": success / none,
        /// Success. The suspended report was produced.
        SuspendedReported => "suspended_reported": success / none,
        // ---- Workflow #378 A6a: end
        UnlockOwnerUsageInvalid => "unlock_owner_usage_invalid": error / none,
        UnlockOwnerOptionInvalid => "unlock_owner_option_invalid": error / none,
        UnlockOwnerConfigMissing => "unlock_owner_config_missing": error / none,
        UnlockOwnerActorMissing => "unlock_owner_actor_missing": error / none,
        UnlockOwnerActorInvalid => "unlock_owner_actor_invalid": error / none,
        UnlockOwnerReportEncodeFailed => "unlock_owner_report_encode_failed": error / none,
        SuspendedUsageInvalid => "suspended_usage_invalid": error / none,
        SuspendedOptionInvalid => "suspended_option_invalid": error / none,
        SuspendedConfigMissing => "suspended_config_missing": error / none,
        SuspendedReportEncodeFailed => "suspended_report_encode_failed": error / none,
        InstallHeldInputInvalid => "install_held_input_invalid": error / none,
        UsageInvalid => "usage_invalid": error / none,
        PolicyBootstrapStateInvalid => "policy_bootstrap_state_invalid": error / none,
        PolicyCatalogPlanChanged => "policy_catalog_plan_changed": error / none,
        PolicyPendingDispatchBudgetExceeded => "policy_pending_dispatch_budget_exceeded": error / none,
        PolicyPendingDispatchWithoutEvaluation => "policy_pending_dispatch_without_evaluation": error / none,
        PolicyCycleYieldReportFailed => "policy_cycle_yield_report_failed": error / none,
        PolicyRecomputeWakeBudgetExceeded => "policy_recompute_wake_budget_exceeded": error / none,
        PolicyRecomputeTerminalMissing => "policy_recompute_terminal_missing": error / none,
        PolicyRecomputeWakeIdentityConflict => "policy_recompute_wake_identity_conflict": error / none,
        PolicySubscriptionCursorInvalid => "policy_subscription_cursor_invalid": error / none,
        PolicyDriverSpawnFailed => "policy_driver_spawn_failed": error / none,
        PolicyDriverStopped => "policy_driver_stopped": error / none,
        PolicySubscriptionRequestInvalid => "policy_subscription_request_invalid": error / none,
        PolicyDriverPanicked => "policy_driver_panicked": error / none,
        PolicyDriverReferenceLeaked => "policy_driver_reference_leaked": error / none,
        PolicyDriverSignalPoisoned => "policy_driver_signal_poisoned": error / none,
        PolicyDriverWakeMissing => "policy_driver_wake_missing": error / none,
        PolicyRecomputeTriggerMissing => "policy_recompute_trigger_missing": error / none,
        PolicyDriverTimeOverflow => "policy_driver_time_overflow": error / none,
        // Workflow #397: the policy monitor's own requests to its Runtime.
        PolicyMonitorRequestRetried => "policy_monitor_request_retried": warning / none,
        PolicyMonitorRetriesExhausted => "policy_monitor_retries_exhausted": error / none,
        PolicyMonitorRuntimeIdentityChanged => "policy_monitor_runtime_identity_changed": error / none,
        PolicyMonitorConfigInvalid => "policy_monitor_config_invalid": error / none,
        // Workflow #378 A3 (ledger revision 3): a provider startup failure of the device layer
        // that names no registered code of its own (was the dynamic `<category>.<stage>`
        // spelling and its `device_error` fallback).
        ProviderNativeFailed => "provider_native_failed": error / none,
        // Workflow #378 A3: why the configuration manifest reports a subsystem on or off (the
        // `outcome` of a `config.subsystems` entry; was the prose `reason`).
        ConfigSubsystemConfigured => "config_subsystem_configured": info / none,
        ConfigSubsystemConfiguredOff => "config_subsystem_configured_off": info / none,
        ConfigSubsystemFlagAbsent => "config_subsystem_flag_absent": info / none,
        ConfigSubsystemSectionAbsent => "config_subsystem_section_absent": info / none,
        ConfigSubsystemAlwaysOn => "config_subsystem_always_on": info / none,
        ConfigSubsystemDiscoveryBound => "config_subsystem_discovery_bound": info / none,
        ConfigSubsystemDiscoveryUnbound => "config_subsystem_discovery_unbound": info / none,
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// What is wrong with an instance's device declaration (vocabulary `device_config_problem`,
    /// Workflow #398 RD-1b, model-398 §10.1).
    pub enum DeviceConfigProblem: "device_config_problem" {
        /// `device.kind` is neither `emulator` nor `handset`.
        KindUnknown => "kind_unknown",
        /// `device.transport` is not `usb` on a handset (`tcp` is reserved), or is named on an
        /// emulator.
        TransportUnsupported => "transport_unsupported",
        /// A handset names `host`, `port` or `connect`.
        EndpointWithHandset => "endpoint_with_handset",
        /// A handset names `serial`: its serial lives only in the private binding file.
        SerialInConfig => "serial_in_config",
        /// A handset names `instance_index`, `instance_name` or `nemu_app_index`.
        DiscoveryKeyWithHandset => "discovery_key_with_handset",
        /// The instance selects a backend its device kind does not allow.
        BackendKindUnsupported => "backend_kind_unsupported",
        /// A handset's alias is longer than 248 bytes, so its label would not fit a name.
        AliasTooLong => "alias_too_long",
    }
}

actingcommand_contract::outcome_locations! {
    /// actingd's stages: the check-config stages (decision D12: the four one-word v1 stages are
    /// registered as `config_*`, and check-config v1 prints their old tokens) and the stages of an
    /// actingd error's FATAL line.
    pub enum ActingdLocation {
        ConfigLoad => "config_load",
        ConfigAssemble => "config_assemble",
        ConfigVision => "config_vision",
        ConfigValidate => "config_validate",
        /// check-config stage: admitting the resource packages (Workflow #378 A6a: was
        /// `resource_package`, which is also the report's member name).
        ResourcePackage => "config_resource_package",
        /// Workflow #384 D2-S2: checking the recognition maintenance package bindings.
        RecognitionBinding => "config_recognition_binding",
        PolicyState => "policy_state",
        PolicyInitialization => "policy_initialization",
        PolicyBootstrap => "policy_bootstrap",
        PolicyMonitorSetup => "policy_monitor_setup",
        PolicyCloseoutDriver => "policy_closeout_driver",
        PolicyCloseoutMonitor => "policy_closeout_monitor",
        PolicyCloseoutShutdown => "policy_closeout_shutdown",
        PolicyCloseoutHostClose => "policy_closeout_host_close",
    }
}
