// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    MAX_VENDOR_STDIO_STEPS, StdioApi, StdioFact, StdioFileIdentity, StdioNativeError, StdioPhase,
    StdioReference, StdioReferenceFact, StdioStep, StdioUnknown, VendorStdioFacts,
};
use std::ffi::c_void;

#[repr(C)]
#[derive(Default)]
struct FileTime {
    low: u32,
    high: u32,
}

impl FileTime {
    fn value(&self) -> u64 {
        u64::from(self.low) | (u64::from(self.high) << 32)
    }
}

#[repr(C)]
#[derive(Default)]
struct FileIdInfo {
    volume_serial: u64,
    file_id: [u8; 16],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn GetProcessTimes(
        process: *mut c_void,
        created: *mut FileTime,
        exited: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    fn GetSystemTimeAsFileTime(time: *mut FileTime);
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
    fn GetHandleInformation(handle: *mut c_void, flags: *mut u32) -> i32;
    fn GetFileInformationByHandleEx(
        handle: *mut c_void,
        class: i32,
        info: *mut c_void,
        size: u32,
    ) -> i32;
    fn GetStdHandle(which: u32) -> *mut c_void;
}

#[link(name = "ucrt")]
unsafe extern "C" {
    fn _get_osfhandle(fd: i32) -> isize;
    fn _get_errno(value: *mut i32) -> i32;
    fn _get_doserrno(value: *mut u32) -> i32;
    fn _set_errno(value: i32) -> i32;
    fn _set_doserrno(value: u32) -> i32;
}

// Queries must not replace the calling operation's CRT or Win32 error state.
struct NativeErrorState {
    win32: u32,
    errno: Result<i32, i32>,
    dos_errno: Result<u32, i32>,
}
impl NativeErrorState {
    fn save() -> Self {
        let win32 = unsafe { GetLastError() };
        let (errno, dos_errno) = crt_codes();
        Self {
            win32,
            errno,
            dos_errno,
        }
    }
}
impl Drop for NativeErrorState {
    fn drop(&mut self) {
        unsafe {
            if let Ok(value) = self.errno {
                _set_errno(value);
            }
            if let Ok(value) = self.dos_errno {
                _set_doserrno(value);
            }
            SetLastError(self.win32);
        }
    }
}

fn crt_codes() -> (Result<i32, i32>, Result<u32, i32>) {
    let mut errno = 0;
    let mut dos_errno = 0;
    let errno_status = unsafe { _get_errno(&mut errno) };
    let dos_status = unsafe { _get_doserrno(&mut dos_errno) };
    (
        if errno_status == 0 {
            Ok(errno)
        } else {
            Err(errno_status)
        },
        if dos_status == 0 {
            Ok(dos_errno)
        } else {
            Err(dos_status)
        },
    )
}

pub(super) fn crt_error() -> StdioNativeError {
    let (errno, dos_errno) = crt_codes();
    StdioNativeError::Crt { errno, dos_errno }
}

pub(super) fn win32_error() -> StdioNativeError {
    StdioNativeError::Win32 {
        code: unsafe { GetLastError() },
    }
}

pub(super) fn filetime() -> u64 {
    let mut time = FileTime::default();
    unsafe { GetSystemTimeAsFileTime(&mut time) };
    time.value()
}

impl VendorStdioFacts {
    pub(super) fn new() -> Self {
        let _error_state = NativeErrorState::save();
        let mut created = FileTime::default();
        let mut exited = FileTime::default();
        let mut kernel = FileTime::default();
        let mut user = FileTime::default();
        let ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        };
        let process_created_filetime = if ok == 0 {
            StdioFact::Unknown(StdioUnknown::QueryFailed(win32_error()))
        } else {
            StdioFact::Known(created.value())
        };
        Self {
            process_id: std::process::id(),
            process_created_filetime,
            started_filetime: filetime(),
            steps: Vec::with_capacity(MAX_VENDOR_STDIO_STEPS),
            dropped_count: 0,
            paths: Vec::with_capacity(2),
            restart_manager: None,
        }
    }

    pub(super) fn push(&mut self, step: StdioStep) {
        if self.steps.len() < MAX_VENDOR_STDIO_STEPS {
            self.steps.push(step);
        } else {
            self.dropped_count = self.dropped_count.saturating_add(1);
        }
    }
}

pub(super) fn step(
    phase: StdioPhase,
    api: StdioApi,
    target: StdioReference,
    source: Option<StdioReference>,
    returned: i64,
    error: Option<StdioNativeError>,
) -> StdioStep {
    StdioStep {
        phase,
        api,
        target,
        source,
        returned,
        error,
        completed_filetime: filetime(),
        before: None,
        after: None,
        related: None,
        target_retirement: None,
    }
}

/// One observation of this session's paths; size/read is bounded to two GetList calls.
pub(super) fn probe_residue(facts: &mut VendorStdioFacts) {
    use crate::{
        MAX_STDIO_RM_PROCESSES, StdioPathRemoval, StdioRmApi, StdioRmAvailability, StdioRmCall,
        StdioRmFacts, StdioRmProcess,
    };
    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::RestartManager::{
        CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmGetList,
        RmRebootReasonPermissionDenied, RmRebootReasonSessionMismatch, RmRegisterResources,
        RmStartSession,
    };

    if facts.restart_manager.is_some()
        || !facts
            .paths
            .iter()
            .any(|path| matches!(path.removal, StdioPathRemoval::Residual(_)))
    {
        return;
    }
    let _error_state = NativeErrorState::save();
    let mut report = StdioRmFacts {
        availability: StdioRmAvailability::Unavailable,
        calls: Vec::with_capacity(5),
        needed_processes: 0,
        reported_processes: 0,
        reboot_reasons: 0,
        processes: Vec::new(),
    };
    let mut session = 0;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    let status = unsafe { RmStartSession(&mut session, 0, key.as_mut_ptr()) };
    report.calls.push(StdioRmCall {
        api: StdioRmApi::StartSession,
        status,
        completed_filetime: filetime(),
    });
    if status == ERROR_SUCCESS {
        let paths = facts
            .paths
            .iter()
            .map(|path| {
                path.path_utf16
                    .iter()
                    .copied()
                    .chain(std::iter::once(0))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let pointers = paths.iter().map(|path| path.as_ptr()).collect::<Vec<_>>();
        let status = unsafe {
            RmRegisterResources(
                session,
                pointers.len() as u32,
                pointers.as_ptr(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
            )
        };
        report.calls.push(StdioRmCall {
            api: StdioRmApi::RegisterResources,
            status,
            completed_filetime: filetime(),
        });
        if status == ERROR_SUCCESS {
            let status = unsafe {
                RmGetList(
                    session,
                    &mut report.needed_processes,
                    &mut report.reported_processes,
                    std::ptr::null_mut(),
                    &mut report.reboot_reasons,
                )
            };
            report.calls.push(StdioRmCall {
                api: StdioRmApi::GetList,
                status,
                completed_filetime: filetime(),
            });
            if status == ERROR_SUCCESS
                && report.needed_processes == 0
                && report.reported_processes == 0
            {
                report.availability = StdioRmAvailability::Complete;
            } else if status == ERROR_MORE_DATA {
                report.availability = StdioRmAvailability::Incomplete;
                if report.needed_processes <= MAX_STDIO_RM_PROCESSES as u32 {
                    let mut processes = [RM_PROCESS_INFO::default(); MAX_STDIO_RM_PROCESSES];
                    report.reported_processes = MAX_STDIO_RM_PROCESSES as u32;
                    let status = unsafe {
                        RmGetList(
                            session,
                            &mut report.needed_processes,
                            &mut report.reported_processes,
                            processes.as_mut_ptr(),
                            &mut report.reboot_reasons,
                        )
                    };
                    report.calls.push(StdioRmCall {
                        api: StdioRmApi::GetList,
                        status,
                        completed_filetime: filetime(),
                    });
                    if status == ERROR_SUCCESS
                        && report.reported_processes <= MAX_STDIO_RM_PROCESSES as u32
                        && report.needed_processes <= report.reported_processes
                    {
                        report.availability = StdioRmAvailability::Complete;
                        report.processes = processes[..report.reported_processes as usize]
                            .iter()
                            .map(|process| StdioRmProcess {
                                process_id: process.Process.dwProcessId,
                                created_filetime: u64::from(
                                    process.Process.ProcessStartTime.dwLowDateTime,
                                ) | (u64::from(
                                    process.Process.ProcessStartTime.dwHighDateTime,
                                ) << 32),
                                rm_app_name_utf16: process
                                    .strAppName
                                    .iter()
                                    .copied()
                                    .take_while(|unit| *unit != 0)
                                    .collect(),
                            })
                            .collect();
                    } else if status != ERROR_MORE_DATA && status != ERROR_SUCCESS {
                        report.availability = StdioRmAvailability::Unavailable;
                    }
                }
            } else if status == ERROR_SUCCESS {
                report.availability = StdioRmAvailability::Incomplete;
            }
        }
        let status = unsafe { RmEndSession(session) };
        report.calls.push(StdioRmCall {
            api: StdioRmApi::EndSession,
            status,
            completed_filetime: filetime(),
        });
        if status != ERROR_SUCCESS && report.availability == StdioRmAvailability::Complete {
            report.availability = StdioRmAvailability::Incomplete;
        }
        if report.reboot_reasons
            & (RmRebootReasonPermissionDenied | RmRebootReasonSessionMismatch) as u32
            != 0
            && report.availability == StdioRmAvailability::Complete
        {
            report.availability = StdioRmAvailability::Incomplete;
        }
    }
    facts.restart_manager = Some(report);
}

/// Caller must hold a live CRT descriptor. Never call following any _close attempt.
pub(super) fn owned_fd(fd: i32, reference: StdioReference) -> StdioReferenceFact {
    let _error_state = NativeErrorState::save();
    let handle = unsafe { _get_osfhandle(fd) };
    let error = (handle == -1).then(crt_error);
    owned_handle(fd, reference, handle, error)
}

/// Also accepts the result of an already executed _get_osfhandle operation.
pub(super) fn owned_handle(
    fd: i32,
    reference: StdioReference,
    handle: isize,
    error: Option<StdioNativeError>,
) -> StdioReferenceFact {
    let _error_state = NativeErrorState::save();
    let mut value = StdioReferenceFact {
        reference,
        observed_filetime: filetime(),
        fd: Some(fd),
        handle: StdioFact::Known(handle as u64),
        metadata_from: Some(reference),
        flags: StdioFact::Unknown(StdioUnknown::Invalid),
        file_identity: StdioFact::Unknown(StdioUnknown::Invalid),
    };
    if handle == -1 || handle == -2 || handle == 0 {
        let reason = error
            .map(StdioUnknown::QueryFailed)
            .unwrap_or(StdioUnknown::Invalid);
        value.handle = StdioFact::Unknown(reason.clone());
        value.flags = StdioFact::Unknown(reason.clone());
        value.file_identity = StdioFact::Unknown(reason);
        value.metadata_from = None;
        return value;
    }
    let mut flags = 0;
    if unsafe { GetHandleInformation(handle as *mut c_void, &mut flags) } == 0 {
        let error = win32_error();
        value.flags = StdioFact::Unknown(StdioUnknown::QueryFailed(error.clone()));
        value.file_identity = StdioFact::Unknown(StdioUnknown::HandleUnavailable(error));
        return value;
    }
    value.flags = StdioFact::Known(flags);
    let mut info = FileIdInfo::default();
    value.file_identity = if unsafe {
        GetFileInformationByHandleEx(
            handle as *mut c_void,
            18,
            (&mut info as *mut FileIdInfo).cast(),
            std::mem::size_of::<FileIdInfo>() as u32,
        )
    } == 0
    {
        StdioFact::Unknown(StdioUnknown::QueryFailed(win32_error()))
    } else {
        StdioFact::Known(StdioFileIdentity {
            volume_serial: info.volume_serial,
            file_id: info.file_id,
        })
    };
    value
}

/// Read only the fixed Win32 table slot. Metadata is copied from a simultaneous
/// live owner observation, never queried through an unowned borrowed table value.
pub(super) fn table(
    which: u32,
    reference: StdioReference,
    owner: Option<&StdioReferenceFact>,
) -> StdioReferenceFact {
    let _error_state = NativeErrorState::save();
    let handle = unsafe { GetStdHandle(which) } as isize;
    let error = (handle == -1).then(win32_error);
    table_value(handle, error, reference, owner)
}

pub(super) fn table_value(
    handle: isize,
    error: Option<StdioNativeError>,
    reference: StdioReference,
    owner: Option<&StdioReferenceFact>,
) -> StdioReferenceFact {
    let reason = if handle == 0 {
        StdioUnknown::Invalid
    } else {
        StdioUnknown::Borrowed
    };
    let mut value = StdioReferenceFact {
        reference,
        observed_filetime: filetime(),
        fd: None,
        handle: if let Some(error) = error {
            StdioFact::Unknown(StdioUnknown::QueryFailed(error))
        } else if handle == 0 {
            StdioFact::Unknown(StdioUnknown::Invalid)
        } else {
            StdioFact::Known(handle as u64)
        },
        metadata_from: None,
        flags: StdioFact::Unknown(reason.clone()),
        file_identity: StdioFact::Unknown(reason),
    };
    if let Some(owner) = owner
        && matches!(&owner.handle, StdioFact::Known(raw) if *raw == handle as u64)
        && matches!(owner.flags, StdioFact::Known(_))
    {
        value.metadata_from = owner.metadata_from;
        value.flags = owner.flags.clone();
        value.file_identity = owner.file_identity.clone();
    }
    value
}
