// SPDX-License-Identifier: AGPL-3.0-only

//! The vocabularies the outcome module itself needs, and the vocabulary tokens of existing
//! contract enums. The outcome guard (G7) checks each against the catalog's `vocabularies`.

use super::VocabularyToken;
use crate::{InstallSlot, InstallTransitionPhase, RuntimeReceiptState, Sensitivity};

crate::outcome_vocabulary! {
    /// How a link relates to the outcome it is attached to (vocabulary `cause_relation`).
    pub enum CauseRelation: "cause_relation" {
        CausedBy => "caused_by",
        Cleanup => "cleanup",
        Secondary => "secondary",
        Recording => "recording",
        Related => "related",
        AfterCommit => "after_commit",
        Note => "note",
        DiagnosticSummary => "diagnostic_summary",
        CapacityStartupCleanup => "capacity_startup_cleanup",
        CaptureFailureRecord => "capture_failure_record",
        CaptureFrameContext => "capture_frame_context",
        CloseUnlockLedger => "close_unlock_ledger",
        CommittedPlanningFact => "committed_planning_fact",
        DiagnosticCleanup => "diagnostic_cleanup",
        ExportFailure => "export_failure",
        FatalMark => "fatal_mark",
        GeometryObservation => "geometry_observation",
        GeometryRecheck => "geometry_recheck",
        InstallFailureRecording => "install_failure_recording",
        PackageAdmission => "package_admission",
        PlanningAttempt => "planning_attempt",
        PriorCapacityAdmission => "prior_capacity_admission",
        PriorEpochStartupCleanup => "prior_epoch_startup_cleanup",
        PriorTask => "prior_task",
        RetentionStartupCleanup => "retention_startup_cleanup",
        SelectionRecord => "selection_record",
        SelectionRecordLedger => "selection_record_ledger",
        SideFileCleanup => "side_file_cleanup",
        StartupCleanup => "startup_cleanup",
        AdmissionRecord => "admission_record",
        DiagnosticArchive => "diagnostic_archive",
        LifecycleRecord => "lifecycle_record",
    }
}

crate::outcome_vocabulary! {
    /// The third party whose text `raw_text` carries (vocabulary `raw_source`).
    pub enum RawSource: "raw_source" {
        Os => "os",
        Sqlite => "sqlite",
        Adb => "adb",
        MumuManager => "mumu_manager",
        Nemu => "nemu",
        Vision => "vision",
        Json => "json",
        Zip => "zip",
        Toml => "toml",
        Http => "http",
        Process => "process",
        Panic => "panic",
        Other => "other",
    }
}

crate::outcome_vocabulary! {
    /// The Rust `io::ErrorKind` of an I/O failure in snake_case (vocabulary `io_kind`).
    pub enum IoKind: "io_kind" {
        NotFound => "not_found",
        PermissionDenied => "permission_denied",
        ConnectionRefused => "connection_refused",
        ConnectionReset => "connection_reset",
        HostUnreachable => "host_unreachable",
        NetworkUnreachable => "network_unreachable",
        ConnectionAborted => "connection_aborted",
        NotConnected => "not_connected",
        AddrInUse => "addr_in_use",
        AddrNotAvailable => "addr_not_available",
        NetworkDown => "network_down",
        BrokenPipe => "broken_pipe",
        AlreadyExists => "already_exists",
        WouldBlock => "would_block",
        NotADirectory => "not_a_directory",
        IsADirectory => "is_a_directory",
        DirectoryNotEmpty => "directory_not_empty",
        ReadOnlyFilesystem => "read_only_filesystem",
        StorageFull => "storage_full",
        FileTooLarge => "file_too_large",
        ResourceBusy => "resource_busy",
        InvalidFilename => "invalid_filename",
        CrossesDevices => "crosses_devices",
        InvalidInput => "invalid_input",
        InvalidData => "invalid_data",
        TimedOut => "timed_out",
        WriteZero => "write_zero",
        Interrupted => "interrupted",
        Unsupported => "unsupported",
        UnexpectedEof => "unexpected_eof",
        OutOfMemory => "out_of_memory",
        Other => "other",
        /// A kind this vocabulary does not list; `os_error` and `raw_text` tell more.
        Uncategorized => "uncategorized",
    }
}

crate::outcome_vocabulary! {
    /// The I/O verb that failed (vocabulary `io_op`).
    pub enum IoOp: "io_op" {
        Open => "open",
        Read => "read",
        Write => "write",
        Create => "create",
        CreateDir => "create_dir",
        Rename => "rename",
        Remove => "remove",
        Inspect => "inspect",
        Metadata => "metadata",
        Exists => "exists",
        ReadDir => "read_dir",
        Canonicalize => "canonicalize",
        Absolute => "absolute",
        Locate => "locate",
        Seek => "seek",
        Sync => "sync",
        CloneHandle => "clone_handle",
        Hash => "hash",
        Lock => "lock",
        Spawn => "spawn",
        Wait => "wait",
    }
}

impl IoKind {
    /// The token of a Rust error kind; a kind this vocabulary does not list is `uncategorized`.
    pub fn from_error_kind(kind: std::io::ErrorKind) -> Self {
        use std::io::ErrorKind as Kind;
        match kind {
            Kind::NotFound => Self::NotFound,
            Kind::PermissionDenied => Self::PermissionDenied,
            Kind::ConnectionRefused => Self::ConnectionRefused,
            Kind::ConnectionReset => Self::ConnectionReset,
            Kind::HostUnreachable => Self::HostUnreachable,
            Kind::NetworkUnreachable => Self::NetworkUnreachable,
            Kind::ConnectionAborted => Self::ConnectionAborted,
            Kind::NotConnected => Self::NotConnected,
            Kind::AddrInUse => Self::AddrInUse,
            Kind::AddrNotAvailable => Self::AddrNotAvailable,
            Kind::NetworkDown => Self::NetworkDown,
            Kind::BrokenPipe => Self::BrokenPipe,
            Kind::AlreadyExists => Self::AlreadyExists,
            Kind::WouldBlock => Self::WouldBlock,
            Kind::NotADirectory => Self::NotADirectory,
            Kind::IsADirectory => Self::IsADirectory,
            Kind::DirectoryNotEmpty => Self::DirectoryNotEmpty,
            Kind::ReadOnlyFilesystem => Self::ReadOnlyFilesystem,
            Kind::StorageFull => Self::StorageFull,
            Kind::FileTooLarge => Self::FileTooLarge,
            Kind::ResourceBusy => Self::ResourceBusy,
            Kind::InvalidFilename => Self::InvalidFilename,
            Kind::CrossesDevices => Self::CrossesDevices,
            Kind::InvalidInput => Self::InvalidInput,
            Kind::InvalidData => Self::InvalidData,
            Kind::TimedOut => Self::TimedOut,
            Kind::WriteZero => Self::WriteZero,
            Kind::Interrupted => Self::Interrupted,
            Kind::Unsupported => Self::Unsupported,
            Kind::UnexpectedEof => Self::UnexpectedEof,
            Kind::OutOfMemory => Self::OutOfMemory,
            Kind::Other => Self::Other,
            _ => Self::Uncategorized,
        }
    }
}

impl VocabularyToken for Sensitivity {
    const VOCABULARY: &'static str = "sensitivity";

    fn token(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Sensitive => "sensitive",
            Self::Secret => "secret",
        }
    }
}

impl VocabularyToken for InstallSlot {
    const VOCABULARY: &'static str = "slot";

    fn token(self) -> &'static str {
        self.as_str()
    }
}

impl VocabularyToken for InstallTransitionPhase {
    const VOCABULARY: &'static str = "install_phase";

    fn token(self) -> &'static str {
        self.as_str()
    }
}

impl VocabularyToken for RuntimeReceiptState {
    const VOCABULARY: &'static str = "receipt_state";

    fn token(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Observed => "observed",
            Self::Queued => "queued",
            Self::Denied => "denied",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}
