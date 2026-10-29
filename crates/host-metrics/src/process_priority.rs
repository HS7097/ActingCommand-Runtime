// SPDX-License-Identifier: AGPL-3.0-only

//! RB-2 (Alice's ruling of 2026-10-29 10:2x, 「设回正常」): this process's own memory priority
//! and I/O priority, set and read back. The CPU priority class is not here: nothing in this
//! module reads or changes it.
//!
//! - **Memory priority:** `SetProcessInformation` / `GetProcessInformation` with
//!   `ProcessMemoryPriority` (documented, Windows 8 and later).
//! - **I/O priority:** `NtSetInformationProcess` / `NtQueryInformationProcess` with the class
//!   `ProcessIoPriority` (33), resolved from ntdll at run time. Windows documents no
//!   process-wide I/O priority call: `SetPriorityClass` with `PROCESS_MODE_BACKGROUND_END` only
//!   ends a background mode the process entered itself (a process started by a below-normal
//!   scheduled task is not in one, so it fails with `ERROR_PROCESS_NOT_IN_BACKGROUND`), and
//!   `SetFileInformationByHandle(FileIoPriorityHintInfo)` sets one file handle's hint, not the
//!   process's. The native class is the one Task Scheduler's own priority uses and has kept its
//!   number and its `IO_PRIORITY_HINT` values since Windows Vista; raising the process's own
//!   priority to normal needs no privilege. The risk is that an undocumented call may change or
//!   be refused in a later Windows: then the call fails with its NTSTATUS, the caller records
//!   a warning, and the start goes on.
//!
//! On an operating system other than Windows nothing is set and nothing is read: a set is a
//! no-op and a read answers `None`.

use actingcommand_contract::outcome::Outcome;

/// Windows' normal memory priority of a process (`MEMORY_PRIORITY_NORMAL`).
pub const MEMORY_PRIORITY_NORMAL: u32 = 5;
/// Windows' low memory priority (`MEMORY_PRIORITY_LOW`): what a process of a below-normal
/// scheduled task gets.
pub const MEMORY_PRIORITY_LOW: u32 = 2;
/// Windows' normal I/O priority hint (`IoPriorityNormal`).
pub const IO_PRIORITY_NORMAL: u32 = 2;
/// Windows' low I/O priority hint (`IoPriorityLow`): what a process of a below-normal scheduled
/// task gets.
pub const IO_PRIORITY_LOW: u32 = 1;

/// Sets this process's memory priority to `level`. A refusal is `foreign_os_error` with the
/// operation `process_memory_priority_set`.
pub fn set_own_memory_priority(level: u32) -> Result<(), Outcome> {
    #[cfg(windows)]
    {
        os::set_memory_priority(level)
    }
    #[cfg(not(windows))]
    {
        let _ = level;
        Ok(())
    }
}

/// This process's memory priority; `None` on an operating system without one. A refusal is
/// `foreign_os_error` with the operation `process_memory_priority_read`.
pub fn own_memory_priority() -> Result<Option<u32>, Outcome> {
    #[cfg(windows)]
    {
        os::memory_priority().map(Some)
    }
    #[cfg(not(windows))]
    {
        Ok(None)
    }
}

/// Sets this process's I/O priority hint to `level`. A refusal is `foreign_os_error` with the
/// operation `process_io_priority_set`.
pub fn set_own_io_priority(level: u32) -> Result<(), Outcome> {
    #[cfg(windows)]
    {
        os::set_io_priority(level)
    }
    #[cfg(not(windows))]
    {
        let _ = level;
        Ok(())
    }
}

/// This process's I/O priority hint; `None` on an operating system without one. A refusal is
/// `foreign_os_error` with the operation `process_io_priority_read`.
pub fn own_io_priority() -> Result<Option<u32>, Outcome> {
    #[cfg(windows)]
    {
        os::io_priority().map(Some)
    }
    #[cfg(not(windows))]
    {
        Ok(None)
    }
}

#[cfg(windows)]
mod os {
    use crate::codes::HostMetricsLocation;
    use actingcommand_contract::codes::ContractCode;
    use actingcommand_contract::outcome::{Outcome, RawSource};
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows_sys::Win32::Foundation::{GetLastError, HANDLE, NTSTATUS, RtlNtStatusToDosError};
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessInformation, MEMORY_PRIORITY_INFORMATION,
        ProcessMemoryPriority, SetProcessInformation,
    };

    /// `PROCESSINFOCLASS::ProcessIoPriority`.
    const PROCESS_IO_PRIORITY: u32 = 33;

    /// `NtSetInformationProcess(ProcessHandle, ProcessInformationClass, ProcessInformation,
    /// ProcessInformationLength)`.
    type NtSetInformationProcess =
        unsafe extern "system" fn(HANDLE, u32, *const c_void, u32) -> NTSTATUS;
    /// `NtQueryInformationProcess(ProcessHandle, ProcessInformationClass, ProcessInformation,
    /// ProcessInformationLength, ReturnLength)`.
    type NtQueryInformationProcess =
        unsafe extern "system" fn(HANDLE, u32, *mut c_void, u32, *mut u32) -> NTSTATUS;

    /// `foreign_os_error` for a failed Win32 call: the thread's last error, its system text.
    fn last_error(operation: HostMetricsLocation) -> Outcome {
        // SAFETY: reads the calling thread's last error code.
        let os_error = unsafe { GetLastError() };
        Outcome::new(ContractCode::ForeignOsError)
            .raw_source(RawSource::Os)
            .os_error(os_error)
            .raw_text(std::io::Error::from_raw_os_error(os_error as i32).to_string())
            .operation(operation)
    }

    /// `foreign_os_error` for a failed native call: the NTSTATUS as `raw_code`, the Win32 error
    /// it maps to and that error's system text.
    fn status_error(status: NTSTATUS, operation: HostMetricsLocation) -> Outcome {
        // SAFETY: a pure mapping of a status value.
        let os_error = unsafe { RtlNtStatusToDosError(status) };
        Outcome::new(ContractCode::ForeignOsError)
            .raw_source(RawSource::Os)
            .raw_code(format!("0x{:08X}", status as u32))
            .os_error(os_error)
            .raw_text(std::io::Error::from_raw_os_error(os_error as i32).to_string())
            .operation(operation)
    }

    /// The address of ntdll's export `name` (a NUL-terminated ANSI name); `foreign_os_error`
    /// for `operation` when ntdll or the export is not there.
    fn ntdll_export(
        name: &'static std::ffi::CStr,
        operation: HostMetricsLocation,
    ) -> Result<unsafe extern "system" fn() -> isize, Outcome> {
        let library: Vec<u16> = "ntdll.dll".encode_utf16().chain(Some(0)).collect();
        // SAFETY: a NUL-terminated wide name; ntdll is loaded in every process and stays loaded.
        let module = unsafe { GetModuleHandleW(library.as_ptr()) };
        if module.is_null() {
            return Err(last_error(operation));
        }
        // SAFETY: a live module handle and a NUL-terminated ANSI name.
        let address = unsafe { GetProcAddress(module, name.as_ptr().cast()) };
        address.ok_or_else(|| last_error(operation))
    }

    pub(super) fn set_memory_priority(level: u32) -> Result<(), Outcome> {
        let information = MEMORY_PRIORITY_INFORMATION {
            MemoryPriority: level,
        };
        // SAFETY: the pseudo handle of this process, and a valid structure of the class's size.
        let set = unsafe {
            SetProcessInformation(
                GetCurrentProcess(),
                ProcessMemoryPriority,
                (&raw const information).cast::<c_void>(),
                size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
            )
        };
        if set == 0 {
            return Err(last_error(HostMetricsLocation::ProcessMemoryPrioritySet));
        }
        Ok(())
    }

    pub(super) fn memory_priority() -> Result<u32, Outcome> {
        let mut information = MEMORY_PRIORITY_INFORMATION { MemoryPriority: 0 };
        // SAFETY: the pseudo handle of this process, and a writable structure of the class's
        // size.
        let read = unsafe {
            GetProcessInformation(
                GetCurrentProcess(),
                ProcessMemoryPriority,
                (&raw mut information).cast::<c_void>(),
                size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
            )
        };
        if read == 0 {
            return Err(last_error(HostMetricsLocation::ProcessMemoryPriorityRead));
        }
        Ok(information.MemoryPriority)
    }

    pub(super) fn set_io_priority(level: u32) -> Result<(), Outcome> {
        let operation = HostMetricsLocation::ProcessIoPrioritySet;
        let address = ntdll_export(c"NtSetInformationProcess", operation)?;
        // SAFETY: ntdll's `NtSetInformationProcess` has this system-ABI signature.
        let set = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, NtSetInformationProcess>(
                address,
            )
        };
        // SAFETY: the pseudo handle of this process and a valid ULONG `IO_PRIORITY_HINT`.
        let status = unsafe {
            set(
                GetCurrentProcess(),
                PROCESS_IO_PRIORITY,
                (&raw const level).cast::<c_void>(),
                size_of::<u32>() as u32,
            )
        };
        if status < 0 {
            return Err(status_error(status, operation));
        }
        Ok(())
    }

    pub(super) fn io_priority() -> Result<u32, Outcome> {
        let operation = HostMetricsLocation::ProcessIoPriorityRead;
        let address = ntdll_export(c"NtQueryInformationProcess", operation)?;
        // SAFETY: ntdll's `NtQueryInformationProcess` has this system-ABI signature.
        let query = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, NtQueryInformationProcess>(
                address,
            )
        };
        let mut level = 0u32;
        let mut length = 0u32;
        // SAFETY: the pseudo handle of this process, a writable ULONG and its length.
        let status = unsafe {
            query(
                GetCurrentProcess(),
                PROCESS_IO_PRIORITY,
                (&raw mut level).cast::<c_void>(),
                size_of::<u32>() as u32,
                &mut length,
            )
        };
        if status < 0 {
            return Err(status_error(status, operation));
        }
        Ok(level)
    }
}
