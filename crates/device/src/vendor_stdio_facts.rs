// SPDX-License-Identifier: AGPL-3.0-only

/// Fixed owner references; these are roles, not evidence of file-object identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioReference {
    Stdout,
    Stderr,
    SavedStdout,
    SavedStderr,
    CaptureStdout,
    CaptureStderr,
    Win32Stdout,
    Win32Stderr,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioPhase {
    Acquire,
    Install,
    Restore,
    Close,
    AcquisitionCleanup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioApi {
    Dup,
    Dup2,
    Open,
    CreateFile,
    CloseHandle,
    GetStdHandle,
    GetOsfhandle,
    SetStdHandle,
    SetHandleInformation,
    Flush,
    Close,
    Unlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdioNativeError {
    Win32 {
        code: u32,
    },
    Crt {
        errno: Result<i32, i32>,
        dos_errno: Result<u32, i32>,
    },
    Io {
        code: Option<i32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdioUnknown {
    Borrowed,
    Invalid,
    HandleUnavailable(StdioNativeError),
    QueryFailed(StdioNativeError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdioFact<T> {
    Known(T),
    Unknown(StdioUnknown),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioFileIdentity {
    pub volume_serial: u64,
    pub file_id: [u8; 16],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioReferenceFact {
    pub reference: StdioReference,
    pub observed_filetime: u64,
    pub fd: Option<i32>,
    pub handle: StdioFact<u64>,
    /// Metadata belongs to this currently held FD, including when a Win32 table
    /// value was observed to reference it. An unassociated table value is borrowed.
    pub metadata_from: Option<StdioReference>,
    pub flags: StdioFact<u32>,
    pub file_identity: StdioFact<StdioFileIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioStep {
    pub phase: StdioPhase,
    pub api: StdioApi,
    pub target: StdioReference,
    pub source: Option<StdioReference>,
    pub completed_filetime: u64,
    pub returned: i64,
    pub error: Option<StdioNativeError>,
    pub before: Option<StdioReferenceFact>,
    pub after: Option<StdioReferenceFact>,
    pub related: Option<StdioReferenceFact>,
    /// The target was explicitly retired while owned, before restore's _dup2.
    pub target_retirement: Option<StdioTargetRetirement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioTargetRetirement {
    ClosedBeforeReplacement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdioPathRemoval {
    Removed,
    Residual(StdioNativeError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioPathFact {
    pub reference: StdioReference,
    /// The exact Windows path supplied to unlink and Restart Manager, without NUL.
    pub path_utf16: Vec<u16>,
    pub removal: StdioPathRemoval,
}

pub const MAX_STDIO_RM_PROCESSES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioRmApi {
    StartSession,
    RegisterResources,
    GetList,
    EndSession,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioRmCall {
    pub api: StdioRmApi,
    pub status: u32,
    pub completed_filetime: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioRmAvailability {
    Complete,
    Incomplete,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioRmProcess {
    pub process_id: u32,
    /// RM_UNIQUE_PROCESS.ProcessStartTime: UTC 100 ns ticks since 1601-01-01.
    pub created_filetime: u64,
    /// RM_PROCESS_INFO.strAppName, an application/service display name, not an exe name.
    pub rm_app_name_utf16: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioRmFacts {
    pub availability: StdioRmAvailability,
    pub calls: Vec<StdioRmCall>,
    pub needed_processes: u32,
    pub reported_processes: u32,
    pub reboot_reasons: u32,
    pub processes: Vec<StdioRmProcess>,
}

/// Only acquisition and teardown operations append facts, never frame snapshots.
pub const MAX_VENDOR_STDIO_STEPS: usize = 32;

#[derive(Clone, PartialEq, Eq)]
pub struct VendorStdioFacts {
    pub process_id: u32,
    pub process_created_filetime: StdioFact<u64>,
    pub started_filetime: u64,
    pub steps: Vec<StdioStep>,
    pub dropped_count: u16,
    pub paths: Vec<StdioPathFact>,
    pub restart_manager: Option<StdioRmFacts>,
}

impl std::fmt::Debug for VendorStdioFacts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VendorStdioFacts")
            .field("steps", &self.steps.len())
            .field("dropped_count", &self.dropped_count)
            .finish_non_exhaustive()
    }
}
