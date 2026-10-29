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
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{GetLastError, HANDLE_FLAG_INHERIT, SetHandleInformation};

    pub(super) fn clear_inheritance(stream: StandardStream) -> Result<bool, Outcome> {
        // std answers null for a process without the handle (also when Windows answers
        // INVALID_HANDLE_VALUE).
        let (handle, operation) = match stream {
            StandardStream::Output => (
                std::io::stdout().as_raw_handle(),
                HostMetricsLocation::ProcessStdoutInheritanceClear,
            ),
            StandardStream::Error => (
                std::io::stderr().as_raw_handle(),
                HostMetricsLocation::ProcessStderrInheritanceClear,
            ),
        };
        if handle.is_null() {
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
