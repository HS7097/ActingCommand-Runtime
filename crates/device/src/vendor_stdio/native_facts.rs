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
    }
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
    value.flags = if unsafe { GetHandleInformation(handle as *mut c_void, &mut flags) } == 0 {
        StdioFact::Unknown(StdioUnknown::QueryFailed(win32_error()))
    } else {
        StdioFact::Known(flags)
    };
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
    {
        value.metadata_from = owner.metadata_from;
        value.flags = owner.flags.clone();
        value.file_identity = owner.file_identity.clone();
    }
    value
}
