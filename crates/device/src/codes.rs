// SPDX-License-Identifier: AGPL-3.0-only

//! The device crate's outcome codes and locations (Workflow #378 A2e). Every entry is listed in
//! the fragment `contracts/outcome-codes/device.json`; the outcome guard (G1) checks both
//! directions. A2e is mechanical: it registers the spellings the crate emits that already follow
//! the naming form and changes no emitted string. The dotted device stages, the one-word
//! operations and the device error's other text channels stay strings behind bridge entries
//! (A2-P renames, B5 settles the device layer). Codes and locations several owners share are
//! the contract's (`SharedCode`, `ContractLocation`). Every entry is pending.

actingcommand_contract::outcome_codes! {
    /// Why resolving MuMu's external renderer (Nemu) for capture or input failed. A Nemu
    /// resolution context carries one, and the device error's diagnostic text names it.
    pub enum NemuResolutionReason {
        InstallationAbsent => "installation_absent": error / environment_fault,
        InstallationAmbiguous => "installation_ambiguous": error / environment_fault,
        SharedAdbMultipleDllVersions => "shared_adb_multiple_dll_versions": error / environment_fault,
        ConfiguredAdbIdentityUnrecognized => "configured_adb_identity_unrecognized": error / none,
        RootMismatch => "root_mismatch": error / none,
        DllOutsideRoot => "dll_outside_root": error / environment_fault,
        DllVersionMissing => "dll_version_missing": error / environment_fault,
        VersionMismatch => "version_mismatch": error / environment_fault,
        CandidateAbsent => "candidate_absent": error / environment_fault,
        CandidateOutsideRoot => "candidate_outside_root": error / environment_fault,
        RunningExecutableTopologyInvalid => "running_executable_topology_invalid": error / environment_fault,
        RunningVersionMissing => "running_version_missing": error / environment_fault,
        TargetIdentityUnavailable => "target_identity_unavailable": error / environment_fault,
        TargetIdentityInvalid => "target_identity_invalid": error / environment_fault,
        TargetIdentityMismatch => "target_identity_mismatch": error / environment_fault,
        TargetProcessAbsent => "target_process_absent": error / environment_fault,
        TargetProcessAmbiguous => "target_process_ambiguous": error / environment_fault,
        TargetExecutableMissing => "target_executable_missing": error / environment_fault,
        CaptureIdentityUncoordinated => "capture_identity_uncoordinated": error / environment_fault,
        /// `MuMuManager version` is below `MUMU_MANAGER_MINIMUM_VERSION` (a Runtime policy floor).
        ProviderVersionBelowMinimum => "provider_version_below_minimum": error / environment_fault,
        /// `MuMuManager version` did not produce a four-part numeric version.
        ProviderVersionUnparseable => "provider_version_unparseable": error / environment_fault,
        /// The Windows uninstall registry source cannot be read on this platform.
        RegistrySourceUnavailable => "registry_source_unavailable": error / environment_fault,
        /// A `MuMuPlayer*` uninstall entry carries no usable install location or an unsupported value type.
        RegistryEntryInvalid => "registry_entry_invalid": error / environment_fault,
    }
}

actingcommand_contract::outcome_locations! {
    /// The device layer's own operations and steps that a device error's diagnostic context or
    /// developer text names: the adb shell input backend's connect steps, the Nemu capture
    /// resolution operations and the segmented swipe capability check.
    pub enum DeviceLocation {
        EnsureDevice => "ensure_device",
        ScreenSize => "screen_size",
        BoundsConversion => "bounds_conversion",
        DeviceRotation => "device_rotation",
        BoundsValidate => "bounds_validate",
        InstallationResolve => "installation_resolve",
        CaptureIdentity => "capture_identity",
        TargetResolve => "target_resolve",
        SegmentedSwipe => "segmented_swipe",
        RotationResolve => "rotation_resolve",
    }
}
