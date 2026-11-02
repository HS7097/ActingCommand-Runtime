// SPDX-License-Identifier: AGPL-3.0-only

//! An overlapped client of a local named pipe, for the transports of this crate (Workflow #432;
//! `contracts/agent-forward.md`). The deadline and the stop are enforced here, in the calling
//! thread, with no extra thread and no process.
//!
//! There is no flush. `FlushFileBuffers` waits for the reader and has no form that can be cancelled,
//! so it would hold the calling thread past any deadline. A finished write is in the pipe, and
//! closing a client's handle leaves it there for the server to read.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE,
    INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

/// How long one wait lasts before the stop and the deadline are looked at again.
const WAIT_SLICE: Duration = Duration::from_millis(200);

/// Why a pipe could not be opened.
#[derive(Debug)]
pub(crate) enum OpenFailure {
    /// No server has this name, or every instance is busy (errors 2 and 231).
    Unavailable,
    /// Anything else the operating system said.
    Failed(io::Error),
}

/// Why a write did not finish. In all three cases part of the bytes may be in the pipe.
#[derive(Debug)]
pub(crate) enum WriteFailure {
    /// The deadline passed and the write was cancelled.
    TimedOut,
    /// The caller asked to stop and the write was cancelled.
    Stopped,
    /// The operating system refused the write.
    Failed(io::Error),
}

/// One open connection to a named pipe's server.
pub(crate) struct PipeClient {
    handle: OwnedHandle,
    /// The manual-reset event of this client's overlapped operations.
    event: OwnedHandle,
}

impl PipeClient {
    /// Opens the pipe `name`. The server may not impersonate this process beyond identifying it.
    pub(crate) fn open(name: &str) -> Result<Self, OpenFailure> {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `wide` is a NUL-terminated string that outlives the call; the security attributes
        // and the template are optional and null.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_NONE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let error = io::Error::last_os_error();
            let code = error.raw_os_error();
            return Err(
                if code == Some(ERROR_FILE_NOT_FOUND as i32) || code == Some(ERROR_PIPE_BUSY as i32)
                {
                    OpenFailure::Unavailable
                } else {
                    OpenFailure::Failed(error)
                },
            );
        }
        // SAFETY: the handle was just opened, is valid and has no other owner.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        // SAFETY: both optional pointers are null; the event is manual-reset and starts unset.
        let event = unsafe { CreateEventW(null(), 1, 0, null()) };
        if event.is_null() {
            return Err(OpenFailure::Failed(io::Error::last_os_error()));
        }
        // SAFETY: the event was just created, is valid and has no other owner.
        let event = unsafe { OwnedHandle::from_raw_handle(event) };
        Ok(Self { handle, event })
    }

    /// The process id of the server this client is connected to.
    pub(crate) fn server_process_id(&self) -> io::Result<u32> {
        let mut pid = 0_u32;
        // SAFETY: the handle lives as long as `self` and the out pointer is valid for the call.
        if unsafe { GetNamedPipeServerProcessId(self.handle.as_raw_handle(), &mut pid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(pid)
    }

    /// Writes all of `bytes`, until `deadline` or until `stopped` answers true.
    pub(crate) fn write_all(
        &self,
        bytes: &[u8],
        deadline: Instant,
        stopped: &dyn Fn() -> bool,
    ) -> Result<(), WriteFailure> {
        let mut sent = 0;
        while sent < bytes.len() {
            let written = self.write_once(&bytes[sent..], deadline, stopped)?;
            if written == 0 {
                return Err(WriteFailure::Failed(io::ErrorKind::WriteZero.into()));
            }
            sent += written;
        }
        Ok(())
    }

    /// One overlapped write. `overlapped` and `bytes` belong to the kernel from `WriteFile` until
    /// the operation has finished, so no path below returns before `GetOverlappedResult` says it
    /// has: an interrupted write is cancelled and then waited for.
    fn write_once(
        &self,
        bytes: &[u8],
        deadline: Instant,
        stopped: &dyn Fn() -> bool,
    ) -> Result<usize, WriteFailure> {
        let handle = self.handle.as_raw_handle();
        let event = self.event.as_raw_handle();
        let length = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        let mut overlapped = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };
        // SAFETY: the handle is valid; `bytes` holds at least `length` bytes and, like
        // `overlapped`, outlives the operation (see above). The count pointer is null, as an
        // overlapped write allows.
        if unsafe { WriteFile(handle, bytes.as_ptr(), length, null_mut(), &mut overlapped) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                // Nothing was queued, so nothing holds `overlapped`.
                return Err(WriteFailure::Failed(error));
            }
        }
        let interrupted = loop {
            let slice = deadline
                .saturating_duration_since(Instant::now())
                .min(WAIT_SLICE);
            let milliseconds = u32::try_from(slice.as_millis()).unwrap_or(u32::MAX);
            // SAFETY: the event lives as long as `self`.
            match unsafe { WaitForSingleObject(event, milliseconds) } {
                WAIT_OBJECT_0 => break None,
                WAIT_TIMEOUT => {
                    if stopped() {
                        break Some(WriteFailure::Stopped);
                    }
                    if Instant::now() >= deadline {
                        break Some(WriteFailure::TimedOut);
                    }
                }
                _ => break Some(WriteFailure::Failed(io::Error::last_os_error())),
            }
        };
        if interrupted.is_some() {
            // SAFETY: the handle and `overlapped` are valid. Its one expected failure says the
            // operation has already finished, which the wait below sees as well.
            unsafe { CancelIoEx(handle, &overlapped) };
        }
        let mut transferred = 0_u32;
        // SAFETY: the handle, `overlapped` and the out pointer are valid. After a cancel this
        // waits, so the operation is over before `overlapped` and `bytes` go out of use.
        let finished = unsafe {
            GetOverlappedResult(
                handle,
                &overlapped,
                &mut transferred,
                i32::from(interrupted.is_some()),
            )
        };
        if finished != 0 {
            return Ok(transferred as usize);
        }
        let error = io::Error::last_os_error();
        Err(interrupted.unwrap_or(WriteFailure::Failed(error)))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows_sys::Win32::Foundation::ERROR_BROKEN_PIPE;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, ReadFile,
    };
    use windows_sys::Win32::System::Pipes::{
        CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    /// A local pipe server of this test process, under an invented name that no session has. A
    /// client may connect from the moment it exists. It reads only when asked.
    pub(crate) struct TestServer {
        handle: OwnedHandle,
        pub(crate) name: String,
    }

    impl TestServer {
        /// A server with `buffer` bytes of buffer each way. It claims the first instance of its
        /// name, so it can only ever be this test's own pipe.
        pub(crate) fn new(buffer: u32) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let name = format!(
                r"\\.\pipe\cc-msg-ac7e57-{:08x}-{:08x}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            // SAFETY: `wide` is NUL-terminated and outlives the call; the attributes are optional.
            let handle = unsafe {
                CreateNamedPipeW(
                    wide.as_ptr(),
                    PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    1,
                    buffer,
                    buffer,
                    0,
                    null(),
                )
            };
            assert!(
                handle != INVALID_HANDLE_VALUE,
                "create the test pipe: {}",
                io::Error::last_os_error()
            );
            // SAFETY: the handle was just created, is valid and has no other owner.
            let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
            Self { handle, name }
        }

        /// Everything the client wrote, once the client has closed.
        pub(crate) fn read_to_end(&self) -> Vec<u8> {
            let mut all = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                let mut read = 0_u32;
                // SAFETY: the handle is valid and not overlapped; the buffer and the count outlive
                // this synchronous call.
                let ok = unsafe {
                    ReadFile(
                        self.handle.as_raw_handle(),
                        chunk.as_mut_ptr(),
                        chunk.len() as u32,
                        &mut read,
                        null_mut(),
                    )
                };
                if ok == 0 {
                    let error = io::Error::last_os_error();
                    assert_eq!(
                        error.raw_os_error(),
                        Some(ERROR_BROKEN_PIPE as i32),
                        "read the test pipe: {error}"
                    );
                    return all;
                }
                all.extend_from_slice(&chunk[..read as usize]);
            }
        }
    }

    #[test]
    fn pipe_write_past_its_deadline_is_cancelled() {
        // The server never reads and its buffer is far smaller than the write.
        let server = TestServer::new(16);
        let client = PipeClient::open(&server.name).expect("open the test pipe");
        let bytes = vec![b'x'; 1 << 20];
        let deadline = Instant::now() + Duration::from_millis(300);
        let result = client.write_all(&bytes, deadline, &|| false);
        assert!(
            matches!(result, Err(WriteFailure::TimedOut)),
            "the write ends at its deadline: {result:?}"
        );
        assert!(Instant::now() >= deadline);
    }

    #[test]
    fn pipe_stops_when_asked() {
        let server = TestServer::new(16);
        let client = PipeClient::open(&server.name).expect("open the test pipe");
        let bytes = vec![b'x'; 1 << 20];
        let deadline = Instant::now() + Duration::from_secs(3600);
        let result = client.write_all(&bytes, deadline, &|| true);
        assert!(
            matches!(result, Err(WriteFailure::Stopped)),
            "the write ends when asked, long before its deadline: {result:?}"
        );
    }

    #[test]
    fn pipe_reports_the_server_pid() {
        let server = TestServer::new(4096);
        let client = PipeClient::open(&server.name).expect("open the test pipe");
        assert_eq!(
            client.server_process_id().expect("the server's process id"),
            std::process::id()
        );
    }
}
