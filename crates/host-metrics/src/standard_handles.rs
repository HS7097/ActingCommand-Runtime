// SPDX-License-Identifier: AGPL-3.0-only

//! WD-MT (Workflow #421, INV-1119): this process's own standard output and standard error
//! handles, made not inheritable; #398 DB-3: its standard input too.
//!
//! Windows creates every child with `bInheritHandles` TRUE when it is given standard handles
//! (Rust's `Command` always is), so the child receives a copy of every inheritable handle in its
//! parent, not only the three it is given. A child that starts a long-lived process of its own
//! hands them on again: on 2026-10-29 an adb client started by actingd started the adb server,
//! and the server kept actingd's stdout and stderr log files open for its whole life. Clearing
//! `HANDLE_FLAG_INHERIT` on the process's own handles once, before any child is started, keeps
//! them out of every child, whatever starts it (Rust's `Command`, a vendor library's own
//! `CreateProcess`). A handle a child is explicitly given is duplicated for it by its starter
//! and is not affected: Rust's `Command` gives a child an inheritable duplicate of what it is
//! given (a pipe or a file it made, or with `Stdio::inherit()`, the default of `spawn` and
//! `status`, this process's own handle), whatever this process's own flag says. Such a child
//! holds what it was given as its own standard handle, and what a long-lived process it starts
//! may inherit from it is that child's to clear, at its own start.
//!
//! #398 DB-3: the same held a program of the workspace other than actingd. `actinglab devices`,
//! run with its output through a pipe on a computer where no adb server ran, started adb, whose
//! client started the server; the server kept the pipe its caller read actinglab's answer from,
//! so the caller saw the pipe's end only when that server ended, long after actinglab had
//! written its answer and ended. Standard input is cleared too: an MCP server's and an MCP
//! entry's standard input is the pipe its caller writes its requests into, and a long-lived
//! process that holds a copy of the read end keeps a write into it from failing once the server
//! has ended and, when the pipe's buffer is full, from returning at all. (The end of input the
//! server itself reads is not delayed by it: that comes when every write end is closed.)
//!
//! Who calls it now, once, as the first thing it does, before any child is started and before
//! any thread that could start one:
//! - actingd ([`clear_own_inheritance`] of its stdout and stderr, WD-MT): the daemon's start, and
//!   `discover` and `vision` before they do anything;
//! - [`clear_all_own_inheritance`], its stdin, stdout and stderr (#398 DB-3): actinglab (right
//!   after the completion scripts' callback, which starts nothing and writes nothing on stderr),
//!   actinglab-mcp, actingctl, actingcommand-mcp-entry and actingcommand-mcp-entry-g2 at the top
//!   of `main`, and actingwatch at the start of the task's tick and of a subcommand (after it
//!   attached to its parent's console, whose handles are then its own).
//!
//! actingledger, acib, vision-provider-check, selection-eval and outcome-guard start no child and
//! call neither. A refused call is one developer line, on stderr (the tick's: its log), and the
//! program goes on, as actingd's start does: the clearing only keeps the handle out of a
//! long-lived descendant, and what the program answers does not depend on it.
//!
//! On an operating system other than Windows nothing is changed.
//!
//! Workflow #434 LAB-4 fix 1: [`attach_parent_console`], for a GUI-subsystem program run with a
//! subcommand from a console (`actingwatch status|install|uninstall`), so that what it prints
//! appears there. That program forbids unsafe code; the one call it needs lives here.

use actingcommand_contract::outcome::Outcome;

/// Attaches this process to its parent's console (`AttachConsole(ATTACH_PARENT_PROCESS)`), so a
/// GUI-subsystem program started from a console writes its standard output and standard error
/// there: Windows sets the standard handles the process was not given to that console's. `true`
/// when it attached. `false` when there was nothing to attach to (the parent has no console, or
/// has exited, or this process has a console already) or the call was refused, and on an
/// operating system other than Windows: the standard handles stay as they were, so a handle the
/// process was given (a redirection, a pipe) still receives what it writes. Nothing is opened,
/// and a console window is never created.
pub fn attach_parent_console() -> bool {
    #[cfg(windows)]
    {
        os::attach_parent_console()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// One of this process's standard handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardStream {
    /// #398 DB-3: standard input.
    Input,
    Output,
    Error,
}

/// #398 DB-3: the three standard handles, in the order [`clear_all_own_inheritance`] clears them.
pub const ALL_STREAMS: [StandardStream; 3] = [
    StandardStream::Input,
    StandardStream::Output,
    StandardStream::Error,
];

/// #398 DB-3: the developer text a program writes beside a refusal of
/// [`clear_all_own_inheritance`] before it goes on.
pub const NOT_CLEARED_TEXT: &str = "this standard handle stays inheritable, so a long-lived process a child starts may keep it open; the program goes on";

/// Marks this process's `stream` handle not inheritable (`SetHandleInformation` with
/// `HANDLE_FLAG_INHERIT` 0). `Ok(false)` when the process has no such handle (or, on another
/// operating system, nothing to change). A refusal is `foreign_os_error` with the operation
/// `process_stdin_inheritance_clear`, `process_stdout_inheritance_clear` or
/// `process_stderr_inheritance_clear`.
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

/// #398 DB-3: [`clear_own_inheritance`] of each of [`ALL_STREAMS`], each whatever the others
/// answered: the refusals, in that order; none when every handle was cleared or is absent.
pub fn clear_all_own_inheritance() -> Vec<Outcome> {
    ALL_STREAMS
        .into_iter()
        .filter_map(|stream| clear_own_inheritance(stream).err())
        .collect()
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
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE,
    };

    pub(super) fn attach_parent_console() -> bool {
        // SAFETY: attaches this process to the console of its parent, if it has one; no memory
        // is passed and no handle is returned.
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) != 0 }
    }

    pub(super) fn clear_inheritance(stream: StandardStream) -> Result<bool, Outcome> {
        // The handle is read straight from the process's standard handle table; this crate
        // never opens a std stream object of its own.
        let (which, operation) = match stream {
            StandardStream::Input => (
                STD_INPUT_HANDLE,
                HostMetricsLocation::ProcessStdinInheritanceClear,
            ),
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
