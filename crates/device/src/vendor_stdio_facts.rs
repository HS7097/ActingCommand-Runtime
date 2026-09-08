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
    GetStdHandle,
    GetOsfhandle,
    SetStdHandle,
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
