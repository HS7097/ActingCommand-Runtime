// SPDX-License-Identifier: AGPL-3.0-only

//! WD-MT (Workflow #421, INV-1119): this process's own standard output and standard error
//! handles, made not inheritable.
//!
//! Windows creates every child with `bInheritHandles` TRUE when it is given standard handles
//! (Rust's `Command` always is), so the child receives a copy of every inheritable handle in its
//! parent, not only the three it is given. A child that starts a long-lived process of its own
//! hands them on again: on 2026-10-29 an adb client started by actingd started the adb server,
//! and the server kept actingd's stdout and stderr log files open for its whole life. Clearing
//! `HANDLE_FLAG_INHERIT` on the process's own handles once, before any child is started, keeps
//! them out of every child, whatever starts it (Rust's `Command`, a vendor library's own
//! `CreateProcess`). A handle a child is explicitly given is duplicated for it by its starter
//! and is not affected.
//!
//! On an operating system other than Windows nothing is changed.

use actingcommand_contract::outcome::Outcome;

/// One of this process's standard handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardStream {
    Output,
    Error,
}

/// Marks this process's `stream` handle not inheritable (`SetHandleInformation` with
/// `HANDLE_FLAG_INHERIT` 0). `Ok(false)` when the process has no such handle (or, on another
/// operating system, nothing to change). A refusal is `foreign_os_error` with the operation
/// `process_stdout_inheritance_clear` or `process_stderr_inheritance_clear`.
pub fn clear_own_inheritance(stream: StandardStream) -> Result<bool, Outcome> {
    #[cfg(windows)]
    {
        os::clear_inheritance(stream)
    }
    #[cfg(not(windows))]
    {
        let _ = stream;
        Ok(false)
    }
}

#[cfg(windows)]
mod os {
    use super::StandardStream;
    use crate::codes::HostMetricsLocation;
    use actingcommand_contract::codes::ContractCode;
    use actingcommand_contract::outcome::{Outcome, RawSource};
    use windows_sys::Win32::Foundation::{
        GetLastError, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    };
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};

    pub(super) fn clear_inheritance(stream: StandardStream) -> Result<bool, Outcome> {
        // The handle is read straight from the process's standard handle table; this crate
        // never opens a std stream object of its own.
        let (which, operation) = match stream {
            StandardStream::Output => (
                STD_OUTPUT_HANDLE,
                HostMetricsLocation::ProcessStdoutInheritanceClear,
            ),
            StandardStream::Error => (
                STD_ERROR_HANDLE,
                HostMetricsLocation::ProcessStderrInheritanceClear,
            ),
        };
        // SAFETY: reads one entry of this process's standard handle table; nothing is opened.
        let handle = unsafe { GetStdHandle(which) };
        // NULL: the process has no such handle. INVALID_HANDLE_VALUE: none could be read.
        // Either way there is no handle whose flag could be inherited.
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Ok(false);
        }
        // SAFETY: the process's own standard handle, live for the process; only its inherit
        // flag changes.
        let cleared = unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) };
        if cleared == 0 {
            // SAFETY: reads the calling thread's last error code.
            let os_error = unsafe { GetLastError() };
            return Err(Outcome::new(ContractCode::ForeignOsError)
                .raw_source(RawSource::Os)
                .os_error(os_error)
                .raw_text(std::io::Error::from_raw_os_error(os_error as i32).to_string())
                .operation(operation));
        }
        Ok(true)
    }
}
