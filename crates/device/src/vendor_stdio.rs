// SPDX-License-Identifier: AGPL-3.0-only

use crate::DeviceResult;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct VendorStdioCapture {
    pub stdout: String,
    pub stderr: String,
}

impl VendorStdioCapture {
    pub(crate) fn is_empty(&self) -> bool {
        self.stdout.is_empty() && self.stderr.is_empty()
    }
}

#[cfg(windows)]
pub(crate) fn capture_vendor_stdio<T>(
    operation: impl FnOnce() -> DeviceResult<T>,
) -> DeviceResult<(T, VendorStdioCapture)> {
    let _lock = stdio_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut guard = imp::RedirectGuard::new()?;
    let result = operation();
    let captured = guard.finish()?;
    result.map(|value| (value, captured))
}

#[cfg(not(windows))]
pub(crate) fn capture_vendor_stdio<T>(
    operation: impl FnOnce() -> DeviceResult<T>,
) -> DeviceResult<(T, VendorStdioCapture)> {
    operation().map(|value| (value, VendorStdioCapture::default()))
}

#[cfg(windows)]
fn stdio_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(windows)]
mod imp {
    use super::VendorStdioCapture;
    use crate::{DeviceError, DeviceResult};
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
        restored: bool,
    }

    impl RedirectGuard {
        pub(super) fn new() -> DeviceResult<Self> {
            let saved_stdout = dup_fd(STDOUT_FD, "stdout")?;
            let saved_stderr = match dup_fd(STDERR_FD, "stderr") {
                Ok(fd) => fd,
                Err(err) => {
                    close_fd(saved_stdout);
                    return Err(err);
                }
            };
            let stdout_path = capture_path("stdout");
            let capture_stdout = match open_capture_file(&stdout_path) {
                Ok(fd) => fd,
                Err(err) => {
                    close_fd(saved_stdout);
                    close_fd(saved_stderr);
                    return Err(err);
                }
            };
            let stderr_path = capture_path("stderr");
            let capture_stderr = match open_capture_file(&stderr_path) {
                Ok(fd) => fd,
                Err(err) => {
                    close_fd(saved_stdout);
                    close_fd(saved_stderr);
                    close_fd(capture_stdout);
                    let _ = std::fs::remove_file(&stdout_path);
                    return Err(err);
                }
            };

            flush_all();
            if let Err(err) = dup2_fd(capture_stdout, STDOUT_FD, "stdout") {
                close_fd(saved_stdout);
                close_fd(saved_stderr);
                close_fd(capture_stdout);
                close_fd(capture_stderr);
                let _ = std::fs::remove_file(&stdout_path);
                let _ = std::fs::remove_file(&stderr_path);
                return Err(err);
            }
            if let Err(err) = dup2_fd(capture_stderr, STDERR_FD, "stderr") {
                let _ = dup2_fd(saved_stdout, STDOUT_FD, "stdout");
                close_fd(saved_stdout);
                close_fd(saved_stderr);
                close_fd(capture_stdout);
                close_fd(capture_stderr);
                let _ = std::fs::remove_file(&stdout_path);
                let _ = std::fs::remove_file(&stderr_path);
                return Err(err);
            }
            let saved_stdout_handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
            let saved_stderr_handle = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
            if let Err(err) = set_std_handle(STD_OUTPUT_HANDLE, capture_stdout, "stdout") {
                let _ = dup2_fd(saved_stdout, STDOUT_FD, "stdout");
                let _ = dup2_fd(saved_stderr, STDERR_FD, "stderr");
                close_fd(saved_stdout);
                close_fd(saved_stderr);
                close_fd(capture_stdout);
                close_fd(capture_stderr);
                let _ = std::fs::remove_file(&stdout_path);
                let _ = std::fs::remove_file(&stderr_path);
                return Err(err);
            }
            if let Err(err) = set_std_handle(STD_ERROR_HANDLE, capture_stderr, "stderr") {
                let _ = unsafe { SetStdHandle(STD_OUTPUT_HANDLE, saved_stdout_handle) };
                let _ = dup2_fd(saved_stdout, STDOUT_FD, "stdout");
                let _ = dup2_fd(saved_stderr, STDERR_FD, "stderr");
                close_fd(saved_stdout);
                close_fd(saved_stderr);
                close_fd(capture_stdout);
                close_fd(capture_stderr);
                let _ = std::fs::remove_file(&stdout_path);
                let _ = std::fs::remove_file(&stderr_path);
                return Err(err);
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
                restored: false,
            })
        }

        pub(super) fn finish(&mut self) -> DeviceResult<VendorStdioCapture> {
            self.restore()?;
            let stdout = read_capture_fd(self.capture_stdout, "stdout")?;
            let stderr = read_capture_fd(self.capture_stderr, "stderr")?;
            close_fd(self.capture_stdout);
            close_fd(self.capture_stderr);
            let _ = std::fs::remove_file(&self.stdout_path);
            let _ = std::fs::remove_file(&self.stderr_path);
            Ok(VendorStdioCapture {
                stdout: String::from_utf8_lossy(&stdout).to_string(),
                stderr: String::from_utf8_lossy(&stderr).to_string(),
            })
        }

        fn restore(&mut self) -> DeviceResult<()> {
            if self.restored {
                return Ok(());
            }
            flush_all();
            if unsafe { SetStdHandle(STD_OUTPUT_HANDLE, self.saved_stdout_handle) } == 0 {
                return Err(DeviceError::fatal(
                    "failed to restore vendor stdout Win32 handle",
                ));
            }
            if unsafe { SetStdHandle(STD_ERROR_HANDLE, self.saved_stderr_handle) } == 0 {
                return Err(DeviceError::fatal(
                    "failed to restore vendor stderr Win32 handle",
                ));
            }
            dup2_fd(self.saved_stdout, STDOUT_FD, "stdout")?;
            dup2_fd(self.saved_stderr, STDERR_FD, "stderr")?;
            close_fd(self.saved_stdout);
            close_fd(self.saved_stderr);
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

    fn close_fd(fd: i32) {
        if fd >= 0 {
            let _ = unsafe { _close(fd) };
        }
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

    fn flush_all() {
        let _ = unsafe { fflush(std::ptr::null_mut()) };
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

    fn read_capture_fd(fd: i32, name: &str) -> DeviceResult<Vec<u8>> {
        if unsafe { _lseek(fd, 0, SEEK_SET) } < 0 {
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

    #[cfg(test)]
    pub(super) fn write_win32_handle_for_test(std_handle: u32, bytes: &[u8]) -> DeviceResult<()> {
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
        let handle = unsafe { GetStdHandle(std_handle) };
        let mut written = 0u32;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn captures_crt_stdout_and_stderr_noise() {
        let (value, capture) = capture_vendor_stdio(|| {
            imp::write_fd_for_test(1, b"vendor stdout noise\n")?;
            imp::write_fd_for_test(2, b"vendor stderr noise\n")?;
            Ok(7)
        })
        .expect("capture vendor stdio");

        assert_eq!(value, 7);
        assert_eq!(capture.stdout, "vendor stdout noise\n");
        assert_eq!(capture.stderr, "vendor stderr noise\n");
    }

    #[cfg(windows)]
    #[test]
    fn captures_win32_stdout_and_stderr_noise() {
        let (value, capture) = capture_vendor_stdio(|| {
            imp::write_win32_handle_for_test(imp::STD_OUTPUT_HANDLE, b"win32 stdout noise\n")?;
            imp::write_win32_handle_for_test(imp::STD_ERROR_HANDLE, b"win32 stderr noise\n")?;
            Ok(7)
        })
        .expect("capture vendor Win32 stdio");

        assert_eq!(value, 7);
        assert_eq!(capture.stdout, "win32 stdout noise\n");
        assert_eq!(capture.stderr, "win32 stderr noise\n");
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_capture_is_noop() {
        let (value, capture) = capture_vendor_stdio(|| Ok(7)).expect("capture vendor stdio");

        assert_eq!(value, 7);
        assert!(capture.is_empty());
    }
}
