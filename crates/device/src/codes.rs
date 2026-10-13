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

// Workflow #398 RD-5a: the device-state checks of a handset (begin).
actingcommand_contract::outcome_codes! {
    /// Workflow #398 RD-5a (model-398 v1.1 §7, §10.1): the read-only device-state checks of a
    /// handset and its screen check (`crate::device_state`). Every entry is pending.
    pub enum DeviceStateCode {
        /// A device-state read failed or answered a shape the parser does not know (value
        /// `device_state_probe`; link `foreign_adb_error` when adb answered).
        DeviceStateUnreadable => "device_state_unreadable": error / environment_fault,
        /// The screen was still not awake after one wake key and the settle time (value
        /// `device_label`).
        DeviceWakeFailed => "device_wake_failed": error / environment_fault,
        /// A keyguard without a credential still showed after one dismissal and the settle time
        /// (value `device_label`).
        DeviceKeyguardDismissFailed => "device_keyguard_dismiss_failed": error / environment_fault,
        /// A keyguard waits for a person: it asks for a credential, or the policy is `wait` (value
        /// `device_label`).
        DeviceLocked => "device_locked": warning / budget_or_permission,
    }
}
// Workflow #398 RD-5a (end).

actingcommand_contract::outcome_codes! {
    /// #402 RD-3: the scrcpy stream capture backend's results, and the stream freshness result
    /// of the capture interface (#398 RD-3i). Until B5 types the device layer, a device error
    /// carries the spelling as its message, as the frame memory codes do.
    pub enum StreamCaptureCode {
        /// The scrcpy server the executable carries does not match its pinned SHA-256 (or the
        /// build embedded none). Only that instance's scrcpy capture backend is unavailable;
        /// the Runtime goes on (coordinator ruling 10-13, board #403 direction).
        ScrcpyServerDigestMismatch => "scrcpy_server_digest_mismatch": error / none,
        /// Pushing, forwarding or starting the scrcpy server, or its first frame, failed. Only
        /// that instance's run fails and the ladder handles it; the Runtime goes on
        /// (coordinator ruling 10-13, the instance-level rule of board #403).
        ScrcpyServerStartFailed => "scrcpy_server_start_failed": error / environment_fault,
        /// The stream stopped: the server exited, the socket closed, a packet was malformed or
        /// decoding failed. Only that instance's run fails and the ladder handles it; the
        /// Runtime goes on (coordinator ruling 10-13, the instance-level rule of board #403).
        ScrcpyStreamEnded => "scrcpy_stream_ended": error / environment_fault,
        /// The encoder produced another video size than the unscaled crop requested, so a frame
        /// would be scaled; it is refused instead. Only that instance's run fails and the ladder
        /// handles it; the Runtime goes on (coordinator ruling 10-13 09:2x: every scrcpy code is
        /// instance-level).
        ScrcpyVideoSizeMismatch => "scrcpy_video_size_mismatch": error / environment_fault,
        /// No frame encoded after the capture floor arrived within the configured wait.
        CaptureFrameStale => "capture_frame_stale": error / environment_fault,
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
