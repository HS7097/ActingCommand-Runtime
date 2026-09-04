// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    DeviceResourceCloseOutcome, DeviceResourceClosePhase, DeviceResourceKind,
    DeviceResourceQuiescence, DeviceResult,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorStdioCapture {
    pub stdout: String,
    pub stderr: String,
}

impl VendorStdioCapture {
    pub fn is_empty(&self) -> bool {
        self.stdout.is_empty() && self.stderr.is_empty()
    }
}

pub fn vendor_stdio_session_diagnostic() -> DeviceResult<VendorStdioCapture> {
    let mut session = VendorStdioSession::start()?;
    write_vendor_stdio_diagnostic_noise()?;
    let capture = session.snapshot()?;
    session.finish()?;
    Ok(capture)
}

#[cfg(windows)]
pub(crate) struct VendorStdioSession {
    _lock: std::sync::MutexGuard<'static, ()>,
    guard: Option<imp::RedirectGuard>,
    close_result: Option<DeviceResult<DeviceResourceCloseOutcome>>,
}

#[cfg(windows)]
impl VendorStdioSession {
    pub(crate) fn start() -> DeviceResult<Self> {
        let lock = stdio_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let guard = imp::RedirectGuard::new()?;
        Ok(Self {
            _lock: lock,
            guard: Some(guard),
            close_result: None,
        })
    }

    pub(crate) fn snapshot(&mut self) -> DeviceResult<VendorStdioCapture> {
        self.guard
            .as_mut()
            .ok_or_else(|| crate::DeviceError::fatal("vendor stdio session is closed"))?
            .snapshot()
    }

    pub(crate) fn finish(&mut self) -> DeviceResult<DeviceResourceCloseOutcome> {
        if let Some(result) = &self.close_result {
            return result.clone();
        }
        let Some(mut guard) = self.guard.take() else {
            let result = Ok(DeviceResourceCloseOutcome::confirmed(0));
            self.close_result = Some(result.clone());
            return result;
        };
        let result = guard
            .finish()
            .map(|_| DeviceResourceCloseOutcome::confirmed(6))
            .map_err(|error| {
                if error.resource_close_causes().is_empty() {
                    error.with_resource_close_cause(
                        DeviceResourceKind::VendorStdio,
                        DeviceResourceClosePhase::Close,
                        "nemu_vendor_stdio",
                        None,
                        None,
                        DeviceResourceQuiescence::Unconfirmed,
                        6,
                    )
                } else {
                    error
                }
            });
        if result.as_ref().is_err_and(|error| {
            error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed)
        }) {
            std::mem::forget(guard);
        }
        self.close_result = Some(result.clone());
        result
    }
}

#[cfg(windows)]
fn write_vendor_stdio_diagnostic_noise() -> DeviceResult<()> {
    imp::write_win32_handle(imp::STD_OUTPUT_HANDLE, b"nemu dll init stdout diagnostic\n")?;
    imp::write_win32_handle(imp::STD_ERROR_HANDLE, b"nemu dll init stderr diagnostic\n")
}

#[cfg(windows)]
impl Drop for VendorStdioSession {
    fn drop(&mut self) {
        if self.close_result.is_none()
            && let Err(error) = self.finish()
            && !std::thread::panicking()
        {
            panic!("{error}");
        }
    }
}

#[cfg(not(windows))]
pub(crate) struct VendorStdioSession {
    close_result: Option<DeviceResult<DeviceResourceCloseOutcome>>,
}

#[cfg(not(windows))]
impl VendorStdioSession {
    pub(crate) fn start() -> DeviceResult<Self> {
        Ok(Self { close_result: None })
    }

    pub(crate) fn snapshot(&mut self) -> DeviceResult<VendorStdioCapture> {
        Ok(VendorStdioCapture::default())
    }

    pub(crate) fn finish(&mut self) -> DeviceResult<DeviceResourceCloseOutcome> {
        if let Some(result) = &self.close_result {
            return result.clone();
        }
        let result = Ok(DeviceResourceCloseOutcome::confirmed(0));
        self.close_result = Some(result.clone());
        result
    }
}

#[cfg(not(windows))]
fn write_vendor_stdio_diagnostic_noise() -> DeviceResult<()> {
    Ok(())
}

#[cfg(windows)]
fn stdio_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(windows)]
mod imp {
    use super::VendorStdioCapture;
    use crate::{
        DeviceError, DeviceResourceClosePhase, DeviceResourceKind, DeviceResourceQuiescence,
        DeviceResult,
    };
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    const STDOUT_FD: i32 = 1;
    const STDERR_FD: i32 = 2;
    pub(super) const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    pub(super) const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const O_CREAT: i32 = 0x0100;
    const O_TRUNC: i32 = 0x0200;
    const O_RDWR: i32 = 0x0002;
    const O_BINARY: i32 = 0x8000;
    const S_IREAD: i32 = 0x0100;
    const S_IWRITE: i32 = 0x0080;
    const SEEK_SET: i32 = 0;

    #[link(name = "ucrt")]
    unsafe extern "C" {
        fn _dup(fd: i32) -> i32;
        fn _dup2(source_fd: i32, target_fd: i32) -> i32;
        fn _close(fd: i32) -> i32;
        fn _wopen(path: *const u16, flags: i32, mode: i32) -> i32;
        fn _read(fd: i32, buffer: *mut c_void, count: u32) -> i32;
        fn _lseek(fd: i32, offset: i32, origin: i32) -> i32;
        fn _get_osfhandle(fd: i32) -> isize;
        fn fflush(stream: *mut c_void) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(std_handle: u32) -> *mut c_void;
        fn SetStdHandle(std_handle: u32, handle: *mut c_void) -> i32;
    }

    pub(super) struct RedirectGuard {
        saved_stdout: i32,
        saved_stderr: i32,
        saved_stdout_handle: *mut c_void,
        saved_stderr_handle: *mut c_void,
        capture_stdout: i32,
        capture_stderr: i32,
        stdout_path: PathBuf,
        stderr_path: PathBuf,
        stdout_offset: i32,
        stderr_offset: i32,
        restored: bool,
    }

    impl RedirectGuard {
        pub(super) fn new() -> DeviceResult<Self> {
            let saved_stdout = dup_fd(STDOUT_FD, "stdout")?;
            let saved_stderr = match dup_fd(STDERR_FD, "stderr") {
                Ok(fd) => fd,
                Err(error) => {
                    return Err(cleanup_acquisition(
                        error,
                        &[(saved_stdout, "saved stdout")],
                        &[],
                    ));
                }
            };
            let stdout_path = capture_path("stdout");
            let capture_stdout = match open_capture_file(&stdout_path) {
                Ok(fd) => fd,
                Err(error) => {
                    return Err(cleanup_acquisition(
                        error,
                        &[
                            (saved_stdout, "saved stdout"),
                            (saved_stderr, "saved stderr"),
                        ],
                        &[],
                    ));
                }
            };
            let stderr_path = capture_path("stderr");
            let capture_stderr = match open_capture_file(&stderr_path) {
                Ok(fd) => fd,
                Err(error) => {
                    return Err(cleanup_acquisition(
                        error,
                        &[
                            (saved_stdout, "saved stdout"),
                            (saved_stderr, "saved stderr"),
                            (capture_stdout, "capture stdout"),
                        ],
                        &[&stdout_path],
                    ));
                }
            };

            if let Err(error) = flush_all() {
                return Err(cleanup_acquisition(
                    error,
                    &[
                        (saved_stdout, "saved stdout"),
                        (saved_stderr, "saved stderr"),
                        (capture_stdout, "capture stdout"),
                        (capture_stderr, "capture stderr"),
                    ],
                    &[&stdout_path, &stderr_path],
                ));
            }
            if let Err(error) = dup2_fd(capture_stdout, STDOUT_FD, "stdout") {
                return Err(cleanup_acquisition(
                    error,
                    &[
                        (saved_stdout, "saved stdout"),
                        (saved_stderr, "saved stderr"),
                        (capture_stdout, "capture stdout"),
                        (capture_stderr, "capture stderr"),
                    ],
                    &[&stdout_path, &stderr_path],
                ));
            }
            if let Err(error) = dup2_fd(capture_stderr, STDERR_FD, "stderr") {
                let mut error = error;
                merge_close_result(
                    &mut error,
                    dup2_fd(saved_stdout, STDOUT_FD, "stdout").map_err(|cleanup| {
                        resource_close_error(
                            cleanup,
                            DeviceResourceKind::FileDescriptor,
                            DeviceResourceClosePhase::RestoreCrt,
                        )
                    }),
                );
                return Err(cleanup_acquisition(
                    error,
                    &[
                        (saved_stdout, "saved stdout"),
                        (saved_stderr, "saved stderr"),
                        (capture_stdout, "capture stdout"),
                        (capture_stderr, "capture stderr"),
                    ],
                    &[&stdout_path, &stderr_path],
                ));
            }
            let saved_stdout_handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
            let saved_stderr_handle = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
            if let Err(error) = set_std_handle(STD_OUTPUT_HANDLE, capture_stdout, "stdout") {
                let mut error = error;
                for result in [
                    dup2_fd(saved_stdout, STDOUT_FD, "stdout"),
                    dup2_fd(saved_stderr, STDERR_FD, "stderr"),
                ] {
                    merge_close_result(
                        &mut error,
                        result.map_err(|cleanup| {
                            resource_close_error(
                                cleanup,
                                DeviceResourceKind::FileDescriptor,
                                DeviceResourceClosePhase::RestoreCrt,
                            )
                        }),
                    );
                }
                return Err(cleanup_acquisition(
                    error,
                    &[
                        (saved_stdout, "saved stdout"),
                        (saved_stderr, "saved stderr"),
                        (capture_stdout, "capture stdout"),
                        (capture_stderr, "capture stderr"),
                    ],
                    &[&stdout_path, &stderr_path],
                ));
            }
            if let Err(error) = set_std_handle(STD_ERROR_HANDLE, capture_stderr, "stderr") {
                let mut error = error;
                if unsafe { SetStdHandle(STD_OUTPUT_HANDLE, saved_stdout_handle) } == 0 {
                    merge_close_result(
                        &mut error,
                        Err(resource_close_error(
                            DeviceError::fatal("failed to restore vendor stdout Win32 handle"),
                            DeviceResourceKind::VendorStdio,
                            DeviceResourceClosePhase::RestoreWin32,
                        )),
                    );
                }
                for result in [
                    dup2_fd(saved_stdout, STDOUT_FD, "stdout"),
                    dup2_fd(saved_stderr, STDERR_FD, "stderr"),
                ] {
                    merge_close_result(
                        &mut error,
                        result.map_err(|cleanup| {
                            resource_close_error(
                                cleanup,
                                DeviceResourceKind::FileDescriptor,
                                DeviceResourceClosePhase::RestoreCrt,
                            )
                        }),
                    );
                }
                return Err(cleanup_acquisition(
                    error,
                    &[
                        (saved_stdout, "saved stdout"),
                        (saved_stderr, "saved stderr"),
                        (capture_stdout, "capture stdout"),
                        (capture_stderr, "capture stderr"),
                    ],
                    &[&stdout_path, &stderr_path],
                ));
            }

            Ok(Self {
                saved_stdout,
                saved_stderr,
                saved_stdout_handle,
                saved_stderr_handle,
                capture_stdout,
                capture_stderr,
                stdout_path,
                stderr_path,
                stdout_offset: 0,
                stderr_offset: 0,
                restored: false,
            })
        }

        pub(super) fn snapshot(&mut self) -> DeviceResult<VendorStdioCapture> {
            flush_all().map_err(|error| {
                resource_close_error(
                    error,
                    DeviceResourceKind::VendorStdio,
                    DeviceResourceClosePhase::SnapshotFlush,
                )
            })?;
            let stdout = read_capture_fd(self.capture_stdout, &mut self.stdout_offset, "stdout")?;
            let stderr = read_capture_fd(self.capture_stderr, &mut self.stderr_offset, "stderr")?;
            Ok(VendorStdioCapture {
                stdout: String::from_utf8_lossy(&stdout).to_string(),
                stderr: String::from_utf8_lossy(&stderr).to_string(),
            })
        }

        pub(super) fn finish(&mut self) -> DeviceResult<VendorStdioCapture> {
            let mut failure = self.restore().err();
            let captured = match self.snapshot() {
                Ok(captured) => Some(captured),
                Err(error) => {
                    merge_close_failure(&mut failure, error);
                    None
                }
            };
            for result in [
                close_fd(self.capture_stdout, "capture stdout"),
                close_fd(self.capture_stderr, "capture stderr"),
            ] {
                if let Err(error) = result {
                    merge_close_failure(
                        &mut failure,
                        resource_close_error(
                            error,
                            DeviceResourceKind::FileDescriptor,
                            DeviceResourceClosePhase::FileDescriptorClose,
                        ),
                    );
                }
            }
            for path in [&self.stdout_path, &self.stderr_path] {
                if let Err(error) = std::fs::remove_file(path) {
                    merge_close_failure(
                        &mut failure,
                        resource_close_error(
                            DeviceError::fatal(format!(
                                "failed to remove vendor stdio capture path {}: {error}",
                                path.display()
                            )),
                            DeviceResourceKind::TemporaryPath,
                            DeviceResourceClosePhase::Unlink,
                        ),
                    );
                }
            }
            match (failure, captured) {
                (Some(error), _) => Err(error),
                (None, Some(captured)) => Ok(captured),
                (None, None) => Err(DeviceError::fatal(
                    "vendor stdio snapshot was unavailable without a close error",
                )),
            }
        }

        fn restore(&mut self) -> DeviceResult<()> {
            if self.restored {
                return Ok(());
            }
            let mut failure = flush_all()
                .map_err(|error| {
                    resource_close_error(
                        error,
                        DeviceResourceKind::VendorStdio,
                        DeviceResourceClosePhase::RestoreFlush,
                    )
                })
                .err();
            for (handle, name) in [
                (self.saved_stdout_handle, "stdout"),
                (self.saved_stderr_handle, "stderr"),
            ] {
                if unsafe {
                    SetStdHandle(
                        if name == "stdout" {
                            STD_OUTPUT_HANDLE
                        } else {
                            STD_ERROR_HANDLE
                        },
                        handle,
                    )
                } == 0
                {
                    merge_close_failure(
                        &mut failure,
                        resource_close_error(
                            DeviceError::fatal(format!(
                                "failed to restore vendor {name} Win32 handle"
                            )),
                            DeviceResourceKind::VendorStdio,
                            DeviceResourceClosePhase::RestoreWin32,
                        ),
                    );
                }
            }
            for result in [
                dup2_fd(self.saved_stdout, STDOUT_FD, "stdout"),
                dup2_fd(self.saved_stderr, STDERR_FD, "stderr"),
            ] {
                if let Err(error) = result {
                    merge_close_failure(
                        &mut failure,
                        resource_close_error(
                            error,
                            DeviceResourceKind::FileDescriptor,
                            DeviceResourceClosePhase::RestoreCrt,
                        ),
                    );
                }
            }
            for result in [
                close_fd(self.saved_stdout, "saved stdout"),
                close_fd(self.saved_stderr, "saved stderr"),
            ] {
                if let Err(error) = result {
                    merge_close_failure(
                        &mut failure,
                        resource_close_error(
                            error,
                            DeviceResourceKind::FileDescriptor,
                            DeviceResourceClosePhase::FileDescriptorClose,
                        ),
                    );
                }
            }
            if let Some(error) = failure {
                return Err(error);
            }
            self.restored = true;
            Ok(())
        }
    }

    impl Drop for RedirectGuard {
        fn drop(&mut self) {
            if !self.restored {
                let _ = self.restore();
            }
        }
    }

    fn dup_fd(fd: i32, name: &str) -> DeviceResult<i32> {
        let duplicated = unsafe { _dup(fd) };
        if duplicated < 0 {
            return Err(DeviceError::fatal(format!(
                "failed to duplicate vendor {name} fd"
            )));
        }
        Ok(duplicated)
    }

    fn dup2_fd(source_fd: i32, target_fd: i32, name: &str) -> DeviceResult<()> {
        if unsafe { _dup2(source_fd, target_fd) } != 0 {
            return Err(DeviceError::fatal(format!(
                "failed to redirect vendor {name} fd"
            )));
        }
        Ok(())
    }

    fn close_fd(fd: i32, name: &str) -> DeviceResult<()> {
        if fd >= 0 && unsafe { _close(fd) } != 0 {
            return Err(DeviceError::fatal(format!(
                "failed to close vendor {name} fd"
            )));
        }
        Ok(())
    }

    fn set_std_handle(std_handle: u32, fd: i32, name: &str) -> DeviceResult<()> {
        let handle = unsafe { _get_osfhandle(fd) };
        if handle == -1 {
            return Err(DeviceError::fatal(format!(
                "failed to get vendor {name} OS handle"
            )));
        }
        if unsafe { SetStdHandle(std_handle, handle as *mut c_void) } == 0 {
            return Err(DeviceError::fatal(format!(
                "failed to redirect vendor {name} Win32 handle"
            )));
        }
        Ok(())
    }

    fn flush_all() -> DeviceResult<()> {
        if unsafe { fflush(std::ptr::null_mut()) } != 0 {
            return Err(DeviceError::fatal("failed to flush vendor stdio"));
        }
        Ok(())
    }

    fn resource_close_error(
        error: DeviceError,
        resource: DeviceResourceKind,
        phase: DeviceResourceClosePhase,
    ) -> DeviceError {
        error.with_resource_close_cause(
            resource,
            phase,
            "nemu_vendor_stdio",
            None,
            None,
            DeviceResourceQuiescence::Unconfirmed,
            1,
        )
    }

    fn merge_close_failure(failure: &mut Option<DeviceError>, error: DeviceError) {
        *failure = Some(match failure.take() {
            Some(primary) => primary.merge_resource_cleanup(error),
            None => error,
        });
    }

    fn merge_close_result(primary: &mut DeviceError, result: DeviceResult<()>) {
        if let Err(cleanup) = result {
            *primary = primary.clone().merge_resource_cleanup(cleanup);
        }
    }

    fn cleanup_acquisition(
        mut primary: DeviceError,
        descriptors: &[(i32, &str)],
        paths: &[&PathBuf],
    ) -> DeviceError {
        for (descriptor, name) in descriptors {
            merge_close_result(
                &mut primary,
                close_fd(*descriptor, name).map_err(|error| {
                    resource_close_error(
                        error,
                        DeviceResourceKind::FileDescriptor,
                        DeviceResourceClosePhase::AcquisitionCleanup,
                    )
                }),
            );
        }
        for path in paths {
            merge_close_result(
                &mut primary,
                std::fs::remove_file(path).map_err(|error| {
                    resource_close_error(
                        DeviceError::fatal(format!(
                            "failed to remove partial vendor stdio capture path {}: {error}",
                            path.display()
                        )),
                        DeviceResourceKind::TemporaryPath,
                        DeviceResourceClosePhase::AcquisitionCleanup,
                    )
                }),
            );
        }
        primary
    }

    fn open_capture_file(path: &Path) -> DeviceResult<i32> {
        let wide = wide_path(path);
        let fd = unsafe {
            _wopen(
                wide.as_ptr(),
                O_CREAT | O_TRUNC | O_RDWR | O_BINARY,
                S_IREAD | S_IWRITE,
            )
        };
        if fd < 0 {
            return Err(DeviceError::fatal(format!(
                "failed to open vendor stdio capture file {}",
                path.display()
            )));
        }
        Ok(fd)
    }

    fn read_capture_fd(fd: i32, offset: &mut i32, name: &str) -> DeviceResult<Vec<u8>> {
        if unsafe { _lseek(fd, *offset, SEEK_SET) } < 0 {
            return Err(DeviceError::fatal(format!(
                "failed to rewind vendor {name} capture fd"
            )));
        }
        let mut output = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let read = unsafe { _read(fd, buffer.as_mut_ptr().cast::<c_void>(), 4096) };
            if read < 0 {
                return Err(DeviceError::fatal(format!(
                    "failed to read vendor {name} capture fd"
                )));
            }
            if read == 0 {
                *offset += i32::try_from(output.len()).map_err(|_| {
                    DeviceError::fatal(format!("vendor {name} capture offset exceeded i32"))
                })?;
                return Ok(output);
            }
            output.extend_from_slice(&buffer[..read as usize]);
        }
    }

    fn capture_path(label: &str) -> PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "actingcommand-vendor-stdio-{}-{seq}-{label}.log",
            std::process::id()
        ))
    }

    fn wide_path(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    #[cfg(test)]
    pub(super) fn write_fd_for_test(fd: i32, bytes: &[u8]) -> DeviceResult<()> {
        #[link(name = "ucrt")]
        unsafe extern "C" {
            fn _write(fd: i32, buffer: *const c_void, count: u32) -> i32;
        }
        let written = unsafe { _write(fd, bytes.as_ptr().cast::<c_void>(), bytes.len() as u32) };
        if written < 0 || written as usize != bytes.len() {
            return Err(DeviceError::fatal("failed to write test vendor fd noise"));
        }
        Ok(())
    }

    pub(super) fn write_win32_handle(std_handle: u32, bytes: &[u8]) -> DeviceResult<()> {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn WriteFile(
                handle: *mut c_void,
                buffer: *const c_void,
                bytes_to_write: u32,
                bytes_written: *mut u32,
                overlapped: *mut c_void,
            ) -> i32;
        }
        // SAFETY: std_handle is one of the Win32 standard handle constants supplied by
        // this module's callers; the returned handle is checked by the following write.
        let handle = unsafe { GetStdHandle(std_handle) };
        let mut written = 0u32;
        // SAFETY: bytes points to a valid immutable buffer for bytes.len(), and the
        // stack-local written pointer remains valid for the duration of WriteFile.
        let ok = unsafe {
            WriteFile(
                handle,
                bytes.as_ptr().cast::<c_void>(),
                bytes.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || written as usize != bytes.len() {
            return Err(DeviceError::fatal(
                "failed to write test vendor Win32 noise",
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn write_win32_handle_for_test(std_handle: u32, bytes: &[u8]) -> DeviceResult<()> {
        write_win32_handle(std_handle, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn captures_crt_stdout_and_stderr_noise() {
        let mut session = VendorStdioSession::start().expect("start vendor stdio session");
        imp::write_fd_for_test(1, b"vendor stdout noise\n").expect("write stdout noise");
        imp::write_fd_for_test(2, b"vendor stderr noise\n").expect("write stderr noise");
        let value = 7;
        let capture = session.snapshot().expect("capture vendor stdio");

        assert_eq!(value, 7);
        assert!(capture.stdout.contains("vendor stdout noise\n"));
        assert!(capture.stderr.contains("vendor stderr noise\n"));
    }

    #[cfg(windows)]
    #[test]
    fn captures_win32_stdout_and_stderr_noise() {
        let mut session = VendorStdioSession::start().expect("start vendor stdio session");
        imp::write_win32_handle_for_test(imp::STD_OUTPUT_HANDLE, b"win32 stdout noise\n")
            .expect("write stdout noise");
        imp::write_win32_handle_for_test(imp::STD_ERROR_HANDLE, b"win32 stderr noise\n")
            .expect("write stderr noise");
        let value = 7;
        let capture = session.snapshot().expect("capture vendor Win32 stdio");

        assert_eq!(value, 7);
        assert!(capture.stdout.contains("win32 stdout noise\n"));
        assert!(capture.stderr.contains("win32 stderr noise\n"));
    }

    #[cfg(windows)]
    #[test]
    fn session_captures_win32_noise_across_snapshots() {
        let mut session = VendorStdioSession::start().expect("start vendor stdio session");
        imp::write_win32_handle_for_test(imp::STD_OUTPUT_HANDLE, b"first stdout\n")
            .expect("write first stdout noise");
        let first = session.snapshot().expect("first snapshot");
        imp::write_win32_handle_for_test(imp::STD_OUTPUT_HANDLE, b"second stdout\n")
            .expect("write second stdout noise");
        let second = session.snapshot().expect("second snapshot");

        assert!(first.stdout.contains("first stdout\n"));
        assert!(!first.stdout.contains("second stdout\n"));
        assert!(second.stdout.contains("second stdout\n"));
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_capture_is_noop() {
        let mut session = VendorStdioSession::start().expect("start vendor stdio session");
        let value = 7;
        let capture = session.snapshot().expect("capture vendor stdio");

        assert_eq!(value, 7);
        assert!(capture.is_empty());
    }
}
