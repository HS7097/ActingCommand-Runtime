// SPDX-License-Identifier: AGPL-3.0-only

/// Fixed owner references; these are roles, not evidence of file-object identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum StdioPhase {
    Acquire,
    Install,
    Restore,
    Close,
    AcquisitionCleanup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum StdioUnknown {
    Borrowed,
    Invalid,
    HandleUnavailable(StdioNativeError),
    QueryFailed(StdioNativeError),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum StdioFact<T> {
    Known(T),
    Unknown(StdioUnknown),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StdioFileIdentity {
    pub volume_serial: u64,
    pub file_id: [u8; 16],
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_retirement: Option<StdioTargetRetirement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum StdioTargetRetirement {
    ClosedBeforeReplacement,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum StdioPathRemoval {
    Removed,
    Residual(StdioNativeError),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StdioPathFact {
    pub reference: StdioReference,
    /// Exact Windows unlink/Restart Manager path, without NUL.
    pub path_utf16: Vec<u16>,
    pub removal: StdioPathRemoval,
}

pub const MAX_STDIO_RM_PROCESSES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum StdioRmApi {
    StartSession,
    RegisterResources,
    GetList,
    EndSession,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StdioRmCall {
    pub api: StdioRmApi,
    pub status: u32,
    pub completed_filetime: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum StdioRmAvailability {
    Complete,
    Incomplete,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StdioRmProcess {
    pub process_id: u32,
    /// RM_UNIQUE_PROCESS.ProcessStartTime: UTC 100 ns ticks since 1601-01-01.
    pub created_filetime: u64,
    /// RM_PROCESS_INFO.strAppName: application/service display name, not exe identity.
    pub rm_app_name_utf16: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
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

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VendorStdioFacts {
    pub process_id: u32,
    pub process_created_filetime: StdioFact<u64>,
    pub started_filetime: u64,
    pub steps: Vec<StdioStep>,
    pub dropped_count: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<StdioPathFact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

impl VendorStdioFacts {
    pub(crate) fn validate(&self) -> Result<(), super::SanitizationError> {
        if self.steps.len() > MAX_VENDOR_STDIO_STEPS {
            return Err(super::SanitizationError::new(
                "vendor_stdio_steps_exceeded",
                "vendor_stdio",
            ));
        }
        if self.paths.len() > 2
            || self.paths.iter().any(|path| path.path_utf16.len() > 32_767)
            || self.restart_manager.as_ref().is_some_and(|facts| {
                facts.calls.len() > 5
                    || facts.processes.len() > MAX_STDIO_RM_PROCESSES
                    || facts
                        .processes
                        .iter()
                        .any(|process| process.rm_app_name_utf16.len() > 255)
            })
        {
            return Err(super::SanitizationError::new(
                "vendor_stdio_residue_facts_exceeded",
                "vendor_stdio",
            ));
        }
        Ok(())
    }
}
