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
        InstallationAbsent => "installation_absent": error,
        InstallationAmbiguous => "installation_ambiguous": error,
        SharedAdbMultipleDllVersions => "shared_adb_multiple_dll_versions": error,
        ConfiguredAdbIdentityUnrecognized => "configured_adb_identity_unrecognized": error,
        RootMismatch => "root_mismatch": error,
        DllOutsideRoot => "dll_outside_root": error,
        DllVersionMissing => "dll_version_missing": error,
        VersionMismatch => "version_mismatch": error,
        CandidateAbsent => "candidate_absent": error,
        CandidateOutsideRoot => "candidate_outside_root": error,
        RunningExecutableTopologyInvalid => "running_executable_topology_invalid": error,
        RunningVersionMissing => "running_version_missing": error,
        TargetIdentityUnavailable => "target_identity_unavailable": error,
        TargetIdentityInvalid => "target_identity_invalid": error,
        TargetIdentityMismatch => "target_identity_mismatch": error,
        TargetProcessAbsent => "target_process_absent": error,
        TargetProcessAmbiguous => "target_process_ambiguous": error,
        TargetExecutableMissing => "target_executable_missing": error,
        CaptureIdentityUncoordinated => "capture_identity_uncoordinated": error,
        /// `MuMuManager version` is below `MUMU_MANAGER_MINIMUM_VERSION` (a Runtime policy floor).
        ProviderVersionBelowMinimum => "provider_version_below_minimum": error,
        /// `MuMuManager version` did not produce a four-part numeric version.
        ProviderVersionUnparseable => "provider_version_unparseable": error,
        /// The Windows uninstall registry source cannot be read on this platform.
        RegistrySourceUnavailable => "registry_source_unavailable": error,
        /// A `MuMuPlayer*` uninstall entry carries no usable install location or an unsupported value type.
        RegistryEntryInvalid => "registry_entry_invalid": error,
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
