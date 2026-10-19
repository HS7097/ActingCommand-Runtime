// SPDX-License-Identifier: AGPL-3.0-only

//! The device crate's outcome codes and locations (Workflow #378 A2e). Every entry is listed in
//! the fragment `contracts/outcome-codes/device.json`; the outcome guard (G1) checks both
//! directions. A2e is mechanical: it registers the spellings the crate emits that already follow
//! the naming form and changes no emitted string. A2-P registers the device stages
//! (`DeviceStage`, the former dotted tokens) and the input operations; the device error's other
//! text channels stay strings behind bridge entries (B5 settles the device layer). Codes and
//! locations several owners share are the contract's (`SharedCode`, `ContractLocation`). Every
//! entry is pending.

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
    /// resolution operations, the segmented swipe capability check and the input backends'
    /// operations (`input_*`, Workflow #378 A2-P; they were one-word tokens).
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
        InputTap => "input_tap",
        InputSwipe => "input_swipe",
        InputLongTap => "input_long_tap",
        InputKey => "input_key",
        InputText => "input_text",
        InputReset => "input_reset",
        InputClose => "input_close",
    }
}

actingcommand_contract::outcome_locations! {
    /// Workflow #378 A2-P: the device layer's diagnostic stages, the step where a device error
    /// arose (`DeviceErrorDiagnostic::stage`). They were dotted tokens (`adb.*`, `capture.*`,
    /// `input.*`, `maatouch.*`, `minitouch.*`, `mumu_manager.*`, `nemu.*`); each is the former
    /// spelling with every `.` replaced by `_`.
    pub enum DeviceStage {
        AdbEnsureDeviceGetState => "adb_ensure_device_get_state",
        AdbInputBoundsValidate => "adb_input_bounds_validate",
        AdbInputRotationResolve => "adb_input_rotation_resolve",
        CaptureFrameLayout => "capture_frame_layout",
        CaptureScrcpyConfig => "capture_scrcpy_config",
        CaptureScrcpyConnect => "capture_scrcpy_connect",
        CaptureScrcpyCrop => "capture_scrcpy_crop",
        CaptureScrcpyFirstFrame => "capture_scrcpy_first_frame",
        CaptureScrcpyForward => "capture_scrcpy_forward",
        CaptureScrcpyFreshWait => "capture_scrcpy_fresh_wait",
        CaptureScrcpyHeader => "capture_scrcpy_header",
        CaptureScrcpyServerDigest => "capture_scrcpy_server_digest",
        CaptureScrcpyServerExited => "capture_scrcpy_server_exited",
        CaptureScrcpyServerLocalCopy => "capture_scrcpy_server_local_copy",
        CaptureScrcpyServerPush => "capture_scrcpy_server_push",
        CaptureScrcpyServerSpawn => "capture_scrcpy_server_spawn",
        CaptureScrcpySocket => "capture_scrcpy_socket",
        CaptureScrcpyStreamDecode => "capture_scrcpy_stream_decode",
        CaptureScrcpyStreamDecoderOpen => "capture_scrcpy_stream_decoder_open",
        CaptureScrcpyStreamProtocol => "capture_scrcpy_stream_protocol",
        CaptureScrcpyStreamServerExited => "capture_scrcpy_stream_server_exited",
        CaptureScrcpyStreamSocketClosed => "capture_scrcpy_stream_socket_closed",
        CaptureScrcpyStreamSpawn => "capture_scrcpy_stream_spawn",
        CaptureScrcpyStreamState => "capture_scrcpy_stream_state",
        CaptureScrcpyStreamVideoSize => "capture_scrcpy_stream_video_size",
        InputNemuCapability => "input_nemu_capability",
        /// Workflow #406 T3: the adb shell input backend's connect step reading the device's
        /// Android SDK level, which decides whether segmented plans go as `input motionevent`.
        InputSdkLevel => "input_sdk_level",
        InputSegmentedSwipeCapability => "input_segmented_swipe_capability",
        MaatouchHandshakeAborted => "maatouch_handshake_aborted",
        MaatouchHandshakeChannelReceive => "maatouch_handshake_channel_receive",
        MaatouchHandshakePidParse => "maatouch_handshake_pid_parse",
        MaatouchHandshakePidRead => "maatouch_handshake_pid_read",
        MaatouchHandshakePressureValidate => "maatouch_handshake_pressure_validate",
        MaatouchHandshakeStdoutEof => "maatouch_handshake_stdout_eof",
        MaatouchHandshakeTimeout => "maatouch_handshake_timeout",
        MaatouchHandshakeVersionParse => "maatouch_handshake_version_parse",
        MaatouchHandshakeVersionRead => "maatouch_handshake_version_read",
        MaatouchHandshakeVersionShape => "maatouch_handshake_version_shape",
        MaatouchHandshakeVersionValueValidate => "maatouch_handshake_version_value_validate",
        MaatouchInstallChmod => "maatouch_install_chmod",
        MaatouchInstallLocalFileMetadata => "maatouch_install_local_file_metadata",
        MaatouchInstallLocalFileValidate => "maatouch_install_local_file_validate",
        MaatouchInstallPush => "maatouch_install_push",
        MaatouchOperationInactive => "maatouch_operation_inactive",
        MaatouchProcessExited => "maatouch_process_exited",
        MaatouchProcessMissing => "maatouch_process_missing",
        MaatouchProcessPoll => "maatouch_process_poll",
        MaatouchProcessSpawn => "maatouch_process_spawn",
        MaatouchStdinFlush => "maatouch_stdin_flush",
        MaatouchStdinMissing => "maatouch_stdin_missing",
        MaatouchStdinWrite => "maatouch_stdin_write",
        MinitouchHandshakeChannelReceive => "minitouch_handshake_channel_receive",
        MinitouchHandshakeMaxParse => "minitouch_handshake_max_parse",
        MinitouchHandshakeMaxRead => "minitouch_handshake_max_read",
        MinitouchHandshakeMaxShape => "minitouch_handshake_max_shape",
        MinitouchHandshakeMaxValueParse => "minitouch_handshake_max_value_parse",
        MinitouchHandshakeMaxValueValidate => "minitouch_handshake_max_value_validate",
        MinitouchHandshakePidParse => "minitouch_handshake_pid_parse",
        MinitouchHandshakePidRead => "minitouch_handshake_pid_read",
        MinitouchHandshakePidValidate => "minitouch_handshake_pid_validate",
        MinitouchHandshakePressureValidate => "minitouch_handshake_pressure_validate",
        MinitouchHandshakeTimeout => "minitouch_handshake_timeout",
        MinitouchHandshakeVersionParse => "minitouch_handshake_version_parse",
        MinitouchHandshakeVersionRead => "minitouch_handshake_version_read",
        MinitouchInstallChmod => "minitouch_install_chmod",
        MinitouchInstallLocalFileMetadata => "minitouch_install_local_file_metadata",
        MinitouchInstallLocalFileValidate => "minitouch_install_local_file_validate",
        MinitouchInstallPush => "minitouch_install_push",
        MinitouchOperationInactive => "minitouch_operation_inactive",
        MinitouchProcessExited => "minitouch_process_exited",
        MinitouchProcessMissing => "minitouch_process_missing",
        MinitouchProcessPoll => "minitouch_process_poll",
        MinitouchProcessSpawn => "minitouch_process_spawn",
        MinitouchStdinFlush => "minitouch_stdin_flush",
        MinitouchStdinMissing => "minitouch_stdin_missing",
        MinitouchStdinWrite => "minitouch_stdin_write",
        MumuManagerControlErrcode => "mumu_manager_control_errcode",
        MumuManagerControlExit => "mumu_manager_control_exit",
        MumuManagerDecode => "mumu_manager_decode",
        MumuManagerDiscover => "mumu_manager_discover",
        MumuManagerEntry => "mumu_manager_entry",
        MumuManagerErrcode => "mumu_manager_errcode",
        MumuManagerExit => "mumu_manager_exit",
        MumuManagerJson => "mumu_manager_json",
        MumuManagerLaunchError => "mumu_manager_launch_error",
        MumuManagerOutputBound => "mumu_manager_output_bound",
        MumuManagerPath => "mumu_manager_path",
        MumuManagerRegistry => "mumu_manager_registry",
        MumuManagerRestartRefused => "mumu_manager_restart_refused",
        MumuManagerRun => "mumu_manager_run",
        MumuManagerShape => "mumu_manager_shape",
        MumuManagerTimeout => "mumu_manager_timeout",
        MumuManagerVersion => "mumu_manager_version",
        MumuManagerVersionFloor => "mumu_manager_version_floor",
        MumuManagerWaitTimeout => "mumu_manager_wait_timeout",
        NemuCaptureIdentity => "nemu_capture_identity",
        NemuInstallationResolve => "nemu_installation_resolve",
        NemuTargetResolve => "nemu_target_resolve",
    }
}
