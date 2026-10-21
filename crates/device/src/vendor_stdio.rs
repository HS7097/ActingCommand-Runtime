// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    DeviceResourceCloseOutcome, DeviceResourceClosePhase, DeviceResourceKind,
    DeviceResourceQuiescence, DeviceResult,
};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod gate_411_a36a;
#[cfg(windows)]
mod native_facts;
#[cfg(any(windows, test))]
mod redirect_order;

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
    lock: Option<RedirectGateTicket>,
    guard: Option<imp::RedirectGuard>,
    close_result: Option<DeviceResult<DeviceResourceCloseOutcome>>,
}

#[cfg(windows)]
impl VendorStdioSession {
    pub(crate) fn start() -> DeviceResult<Self> {
        Self::start_within(VENDOR_STDIO_REDIRECT_WAIT)
    }

    /// Workflow #411 A36a: takes the process-wide redirect gate, waiting at most `bound`, before
    /// anything is redirected; a longer wait fails with `vendor_stdio_redirect_busy`.
    fn start_within(bound: std::time::Duration) -> DeviceResult<Self> {
        // A36a red proof 2: the redirect runs without holding the process-wide gate.
        drop(stdio_gate().enter(bound)?);
        let guard = imp::RedirectGuard::new()?;
        Ok(Self {
            lock: None,
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

    /// Workflow #191 H: ends one native call's capture. The process standard streams are
    /// restored and the process-level stdio lock is released; the capture files stay open for
    /// the next `resume`. A failed restore keeps the lock and is the session's cached failure.
    pub(crate) fn suspend(&mut self) -> DeviceResult<()> {
        self.guard
            .as_mut()
            .ok_or_else(|| crate::DeviceError::fatal("vendor stdio session is closed"))?
            .suspend()?;
        self.lock.take();
        Ok(())
    }

    /// Workflow #191 H: starts one native call's capture: takes the process-level stdio lock and
    /// redirects the standard streams to this session's capture files again. A resumed session
    /// resumes nothing. A redirection that fails is undone at once; the lock is released unless
    /// that undo failed too.
    pub(crate) fn resume(&mut self) -> DeviceResult<()> {
        self.resume_within(VENDOR_STDIO_REDIRECT_WAIT)
    }

    /// Workflow #411 A36a: the lock is the process-wide redirect gate; a wait longer than `bound`
    /// fails with `vendor_stdio_redirect_busy` and leaves the session suspended.
    fn resume_within(&mut self, bound: std::time::Duration) -> DeviceResult<()> {
        let guard = self
            .guard
            .as_mut()
            .ok_or_else(|| crate::DeviceError::fatal("vendor stdio session is closed"))?;
        if self.lock.is_some() {
            return guard.resume();
        }
        // A36a red proof 2: the redirect runs without holding the process-wide gate.
        drop(stdio_gate().enter(bound)?);
        guard.resume()
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
            .map(|_| {
                DeviceResourceCloseOutcome::confirmed(6)
                    .with_vendor_stdio(std::sync::Arc::new(guard.facts().clone()))
            })
            .map_err(|error| {
                let error = if error.resource_close_causes().is_empty() {
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
                };
                error.with_vendor_stdio_facts(std::sync::Arc::new(guard.facts().clone()))
            });
        if result.as_ref().is_err_and(|error| {
            error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed)
        }) {
            std::mem::forget(guard);
            if let Some(lock) = self.lock.take() {
                std::mem::forget(lock);
            }
        } else {
            drop(guard);
            self.lock.take();
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

    pub(crate) fn suspend(&mut self) -> DeviceResult<()> {
        Ok(())
    }

    pub(crate) fn resume(&mut self) -> DeviceResult<()> {
        Ok(())
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

/// Workflow #411 A36a: how long a vendor library call waits for the process's stdio redirect
/// while another call holds it. Longer than one vendor call holds it, shorter than the Nemu
/// worker's default request timeout (`DEFAULT_CAPTURE_TIMEOUT`, 5 s), so the waiting call
/// answers `vendor_stdio_redirect_busy` before its owner stops waiting for it.
#[cfg(windows)]
const VENDOR_STDIO_REDIRECT_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Workflow #411 A36a: at most one vendor stdio redirect is active in the process, across all
/// threads and sessions. A caller waits for the holder to leave, at most for its bound; a longer
/// wait fails with `vendor_stdio_redirect_busy` and the caller redirects nothing.
#[cfg(windows)]
struct RedirectGate {
    state: std::sync::Mutex<RedirectGateState>,
    changed: std::sync::Condvar,
}

#[cfg(windows)]
struct RedirectGateState {
    held: bool,
    /// Callers waiting at the gate now.
    waiting: usize,
}

/// The gate held by one redirect; dropping it lets the next caller in. A session whose restore
/// is unconfirmed forgets it, so the gate stays held and later callers fail at their bound.
#[cfg(windows)]
struct RedirectGateTicket(&'static RedirectGate);

#[cfg(windows)]
impl Drop for RedirectGateTicket {
    fn drop(&mut self) {
        let mut state = self.0.lock_state();
        state.held = false;
        drop(state);
        self.0.changed.notify_all();
    }
}

#[cfg(windows)]
impl RedirectGate {
    const fn new() -> Self {
        Self {
            state: std::sync::Mutex::new(RedirectGateState {
                held: false,
                waiting: 0,
            }),
            changed: std::sync::Condvar::new(),
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, RedirectGateState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn enter(&'static self, bound: std::time::Duration) -> DeviceResult<RedirectGateTicket> {
        let mut state = self.lock_state();
        state.waiting += 1;
        self.changed.notify_all();
        let (mut state, _) = self
            .changed
            .wait_timeout_while(state, bound, |state| state.held)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.waiting -= 1;
        if state.held {
            return Err(crate::DeviceError::transient(format!(
                "{}: another vendor library call held the process stdio redirect for more than {} ms ({} other callers waiting)",
                crate::codes::VendorStdioCode::VendorStdioRedirectBusy.as_str(),
                bound.as_millis(),
                state.waiting
            )));
        }
        state.held = true;
        Ok(RedirectGateTicket(self))
    }

    #[cfg(test)]
    fn is_held(&self) -> bool {
        self.lock_state().held
    }

    /// Waits at most `bound` until `count` callers wait at the gate.
    #[cfg(test)]
    fn wait_for_waiters(&self, count: usize, bound: std::time::Duration) -> bool {
        let state = self.lock_state();
        let (state, _) = self
            .changed
            .wait_timeout_while(state, bound, |state| state.waiting < count)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.waiting >= count
    }
}

#[cfg(windows)]
fn stdio_gate() -> &'static RedirectGate {
    static GATE: RedirectGate = RedirectGate::new();
    &GATE
}

#[cfg(windows)]
mod imp {
    use super::VendorStdioCapture;
    use super::native_facts;
    use super::redirect_order::{self, StdStream, StdTarget, TableSource};
    use crate::{
        DeviceError, DeviceResourceClosePhase, DeviceResourceKind, DeviceResourceQuiescence,
        DeviceResult,
    };
    use crate::{StdioApi, StdioPhase, StdioReference, VendorStdioFacts};
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    const STDOUT_FD: i32 = 1;
    const STDERR_FD: i32 = 2;
    pub(super) const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    pub(super) const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const O_NOINHERIT: i32 = 0x0080;
    const O_BINARY: i32 = 0x8000;
    const SEEK_SET: i32 = 0;

    #[link(name = "ucrt")]
    unsafe extern "C" {
        fn _dup(fd: i32) -> i32;
        fn _dup2(source_fd: i32, target_fd: i32) -> i32;
        fn _close(fd: i32) -> i32;
        fn _open_osfhandle(handle: isize, flags: i32) -> i32;
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
        facts: VendorStdioFacts,
        saved_stdout: i32,
        saved_stderr: i32,
        /// Workflow #411 A36a: where the restore takes each Win32 standard handle from, by
        /// `StdStream::index`.
        table_sources: [TableSource; 2],
        capture_stdout: i32,
        capture_stderr: i32,
        stdout_path: PathBuf,
        stderr_path: PathBuf,
        stdout_offset: i32,
        stderr_offset: i32,
        restored: bool,
        stdout_crt_redirected: bool,
        stderr_crt_redirected: bool,
        stdout_win32_redirected: bool,
        stderr_win32_redirected: bool,
        restore_result: Option<DeviceResult<()>>,
        finish_result: Option<DeviceResult<VendorStdioCapture>>,
        /// Workflow #191 H: the standard targets are restored between two native calls.
        suspended: bool,
        /// Workflow #191 H: while a later resume/suspend cycle records into its own context,
        /// the owner context waits here. A failed cycle keeps its own context as `facts`.
        owner_facts: Option<VendorStdioFacts>,
    }

    impl RedirectGuard {
        pub(super) fn facts(&self) -> &VendorStdioFacts {
            &self.facts
        }

        pub(super) const fn is_suspended(&self) -> bool {
            self.suspended
        }

        /// Workflow #191 H: restores the standard targets after one native call; the saved and
        /// capture descriptors stay open. The first cycle records into the owner context (the
        /// complete successful path keeps its 32 steps); a later cycle's steps are dropped with
        /// its context once it completes.
        pub(super) fn suspend(&mut self) -> DeviceResult<()> {
            if self.suspended {
                return Ok(());
            }
            self.restore_targets()?;
            self.suspended = true;
            if let Some(owner) = self.owner_facts.take() {
                self.facts = owner;
            }
            Ok(())
        }

        /// Workflow #191 H: installs the capture targets again before one native call. The cycle
        /// records into its own context, not into the bounded owner context. A failed install
        /// is restored at once, as at acquisition; its steps travel with the error.
        pub(super) fn resume(&mut self) -> DeviceResult<()> {
            if let Some(Err(error)) = &self.restore_result {
                return Err(error.clone());
            }
            if self.restored || self.finish_result.is_some() {
                return Err(DeviceError::fatal("vendor stdio session is closed"));
            }
            if !self.suspended {
                return Ok(());
            }
            self.owner_facts = Some(std::mem::replace(&mut self.facts, VendorStdioFacts::new()));
            if let Err(error) = self.install() {
                let error = match self.restore_targets() {
                    Ok(()) => {
                        let error =
                            error.with_vendor_stdio_facts(std::sync::Arc::new(self.facts.clone()));
                        if let Some(owner) = self.owner_facts.take() {
                            self.facts = owner;
                        }
                        error
                    }
                    Err(cleanup) => error
                        .merge_resource_cleanup(cleanup)
                        .with_vendor_stdio_facts(std::sync::Arc::new(self.facts.clone())),
                };
                return Err(error);
            }
            self.suspended = false;
            Ok(())
        }

        pub(super) fn new() -> DeviceResult<Self> {
            let mut facts = VendorStdioFacts::new();
            let saved_stdout = dup_fd(
                STDOUT_FD,
                "stdout",
                StdioReference::Stdout,
                StdioReference::SavedStdout,
                &mut facts,
            )?;
            let saved_stderr = match dup_fd(
                STDERR_FD,
                "stderr",
                StdioReference::Stderr,
                StdioReference::SavedStderr,
                &mut facts,
            ) {
                Ok(fd) => fd,
                Err(error) => {
                    return Err(cleanup_acquisition(
                        error,
                        &[(saved_stdout, "saved stdout", StdioReference::SavedStdout)],
                        &[],
                        &mut facts,
                    ));
                }
            };
            let stdout_path = capture_path("stdout");
            let capture_stdout =
                match open_capture_file(&stdout_path, StdioReference::CaptureStdout, &mut facts) {
                    Ok(fd) => fd,
                    Err(error) => {
                        return Err(cleanup_acquisition(
                            error,
                            &[
                                (saved_stdout, "saved stdout", StdioReference::SavedStdout),
                                (saved_stderr, "saved stderr", StdioReference::SavedStderr),
                            ],
                            &[],
                            &mut facts,
                        ));
                    }
                };
            let stderr_path = capture_path("stderr");
            let capture_stderr =
                match open_capture_file(&stderr_path, StdioReference::CaptureStderr, &mut facts) {
                    Ok(fd) => fd,
                    Err(error) => {
                        return Err(cleanup_acquisition(
                            error,
                            &[
                                (saved_stdout, "saved stdout", StdioReference::SavedStdout),
                                (saved_stderr, "saved stderr", StdioReference::SavedStderr),
                                (
                                    capture_stdout,
                                    "capture stdout",
                                    StdioReference::CaptureStdout,
                                ),
                            ],
                            &[(&stdout_path, StdioReference::CaptureStdout)],
                            &mut facts,
                        ));
                    }
                };

            let stdout_slot = get_std_handle(
                STD_OUTPUT_HANDLE,
                STDOUT_FD,
                StdioReference::Stdout,
                StdioReference::Win32Stdout,
                &mut facts,
            );
            let stderr_slot = get_std_handle(
                STD_ERROR_HANDLE,
                STDERR_FD,
                StdioReference::Stderr,
                StdioReference::Win32Stderr,
                &mut facts,
            );
            // Workflow #411 A36a: a slot that holds a standard descriptor's own handle is
            // restored from that descriptor; the install closes the number it holds now.
            let stdout_owned = unsafe { _get_osfhandle(STDOUT_FD) };
            let stderr_owned = unsafe { _get_osfhandle(STDERR_FD) };
            let table_sources = [
                redirect_order::table_source(
                    StdStream::Stdout,
                    stdout_slot as isize,
                    stdout_owned,
                    stderr_owned,
                ),
                redirect_order::table_source(
                    StdStream::Stderr,
                    stderr_slot as isize,
                    stdout_owned,
                    stderr_owned,
                ),
            ];
            let mut guard = Self {
                saved_stdout,
                saved_stderr,
                table_sources,
                facts,
                capture_stdout,
                capture_stderr,
                stdout_path,
                stderr_path,
                stdout_offset: 0,
                stderr_offset: 0,
                restored: false,
                stdout_crt_redirected: false,
                stderr_crt_redirected: false,
                stdout_win32_redirected: false,
                stderr_win32_redirected: false,
                restore_result: None,
                finish_result: None,
                suspended: false,
                owner_facts: None,
            };
            if let Err(mut error) = guard.install() {
                if let Err(cleanup) = guard.finish() {
                    error = error.merge_resource_cleanup(cleanup);
                }
                error = error.with_vendor_stdio_facts(std::sync::Arc::new(guard.facts.clone()));
                if error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed) {
                    std::mem::forget(guard);
                }
                return Err(error);
            }
            Ok(guard)
        }

        /// Points the standard targets at the capture files, in `redirect_order::INSTALL_ORDER`.
        fn install(&mut self) -> DeviceResult<()> {
            flush_recorded(StdioPhase::Install, &mut self.facts)?;
            for target in redirect_order::INSTALL_ORDER {
                match target {
                    StdTarget::Descriptor(stream) => {
                        *self.descriptor_redirected(stream) = true;
                        dup2_fd(
                            self.capture_fd(stream),
                            stream.fd(),
                            stream_name(stream),
                            capture_reference(stream),
                            standard_reference(stream),
                            StdioPhase::Install,
                            &mut self.facts,
                        )?;
                    }
                    StdTarget::Table(stream) => {
                        *self.table_redirected(stream) = true;
                        set_std_handle(
                            table_slot(stream),
                            self.capture_fd(stream),
                            stream_name(stream),
                            capture_reference(stream),
                            &mut self.facts,
                        )?;
                    }
                }
            }
            Ok(())
        }

        fn capture_fd(&self, stream: StdStream) -> i32 {
            match stream {
                StdStream::Stdout => self.capture_stdout,
                StdStream::Stderr => self.capture_stderr,
            }
        }

        fn saved_fd(&self, stream: StdStream) -> i32 {
            match stream {
                StdStream::Stdout => self.saved_stdout,
                StdStream::Stderr => self.saved_stderr,
            }
        }

        fn descriptor_redirected(&mut self, stream: StdStream) -> &mut bool {
            match stream {
                StdStream::Stdout => &mut self.stdout_crt_redirected,
                StdStream::Stderr => &mut self.stderr_crt_redirected,
            }
        }

        fn table_redirected(&mut self, stream: StdStream) -> &mut bool {
            match stream {
                StdStream::Stdout => &mut self.stdout_win32_redirected,
                StdStream::Stderr => &mut self.stderr_win32_redirected,
            }
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
            if let Some(result) = &self.finish_result {
                return result.clone();
            }
            if let Err(error) = self.restore() {
                let error = error.with_vendor_stdio_facts(std::sync::Arc::new(self.facts.clone()));
                self.finish_result = Some(Err(error.clone()));
                return Err(error);
            }
            let mut failure = None;
            let mut resources_confirmed = true;
            let captured = match self.snapshot() {
                Ok(captured) => Some(captured),
                Err(error) => {
                    merge_close_failure(&mut failure, error);
                    None
                }
            };
            for result in [
                close_fd(
                    self.capture_stdout,
                    "capture stdout",
                    StdioReference::CaptureStdout,
                    StdioPhase::Close,
                    &mut self.facts,
                ),
                close_fd(
                    self.capture_stderr,
                    "capture stderr",
                    StdioReference::CaptureStderr,
                    StdioPhase::Close,
                    &mut self.facts,
                ),
            ] {
                if let Err(error) = result {
                    resources_confirmed = false;
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
            for (path, reference) in [
                (&self.stdout_path, StdioReference::CaptureStdout),
                (&self.stderr_path, StdioReference::CaptureStderr),
            ] {
                unlink(path, reference, StdioPhase::Close, &mut self.facts);
            }
            native_facts::probe_residue(&mut self.facts);
            let result = match (failure, captured) {
                (Some(error), _) => Err(error.with_resource_summary(
                    if resources_confirmed {
                        DeviceResourceQuiescence::Confirmed
                    } else {
                        DeviceResourceQuiescence::Unconfirmed
                    },
                    6,
                )),
                (None, Some(captured)) => Ok(captured),
                (None, None) => Err(DeviceError::fatal(
                    "vendor stdio snapshot was unavailable without a close error",
                )),
            };
            let result = result.map_err(|error| {
                error.with_vendor_stdio_facts(std::sync::Arc::new(self.facts.clone()))
            });
            self.finish_result = Some(result.clone());
            result
        }

        /// The final restore: the standard targets (unless a suspend restored them already),
        /// then the saved descriptors.
        fn restore(&mut self) -> DeviceResult<()> {
            if let Some(result) = &self.restore_result {
                return result.clone();
            }
            self.suspend()?;
            let mut failure = None;
            for result in [
                close_fd(
                    self.saved_stdout,
                    "saved stdout",
                    StdioReference::SavedStdout,
                    StdioPhase::Restore,
                    &mut self.facts,
                ),
                close_fd(
                    self.saved_stderr,
                    "saved stderr",
                    StdioReference::SavedStderr,
                    StdioPhase::Restore,
                    &mut self.facts,
                ),
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
                self.restore_result = Some(Err(error.clone()));
                return Err(error);
            }
            self.restored = true;
            self.restore_result = Some(Ok(()));
            Ok(())
        }

        /// Restores the standard targets this session redirected. Workflow #191 H: it runs after
        /// every native call (`suspend`); a failure is cached as the session's restore result.
        /// Workflow #411 A36a: in `redirect_order::RESTORE_ORDER`, the descriptors first, then
        /// the Win32 standard handles from their `TableSource`.
        fn restore_targets(&mut self) -> DeviceResult<()> {
            if let Some(result) = &self.restore_result {
                return result.clone();
            }
            let mut failure = flush_recorded(StdioPhase::Restore, &mut self.facts)
                .map_err(|error| {
                    resource_close_error(
                        error,
                        DeviceResourceKind::VendorStdio,
                        DeviceResourceClosePhase::RestoreFlush,
                    )
                })
                .err();
            for target in redirect_order::RESTORE_ORDER {
                let restored = match target {
                    StdTarget::Descriptor(stream) => self.restore_descriptor(stream),
                    StdTarget::Table(stream) => self.restore_table(stream),
                };
                if let Err(error) = restored {
                    merge_close_failure(&mut failure, error);
                }
            }
            if let Some(error) = failure {
                self.restore_result = Some(Err(error.clone()));
                return Err(error);
            }
            Ok(())
        }

        /// Retires the standard descriptor's capture duplicate and duplicates the saved original
        /// back onto it (`dup2_fd`).
        fn restore_descriptor(&mut self, stream: StdStream) -> DeviceResult<()> {
            if !*self.descriptor_redirected(stream) {
                return Ok(());
            }
            dup2_fd(
                self.saved_fd(stream),
                stream.fd(),
                stream_name(stream),
                saved_reference(stream),
                standard_reference(stream),
                StdioPhase::Restore,
                &mut self.facts,
            )
            .map_err(|error| {
                resource_close_error(
                    error,
                    DeviceResourceKind::FileDescriptor,
                    DeviceResourceClosePhase::RestoreCrt,
                )
            })?;
            *self.descriptor_redirected(stream) = false;
            Ok(())
        }

        /// Workflow #411 A36a: points the Win32 standard handle back at its original object. A
        /// slot that held a standard descriptor's own handle takes the handle that descriptor
        /// owns after its restore, never the saved number, which the install closed and another
        /// file may hold by now. While that descriptor is still redirected the slot keeps this
        /// session's capture handle, which an unconfirmed session keeps open.
        fn restore_table(&mut self, stream: StdStream) -> DeviceResult<()> {
            if !*self.table_redirected(stream) {
                return Ok(());
            }
            let which = table_slot(stream);
            let target = table_reference(stream);
            let name = stream_name(stream);
            let source = self.table_sources[stream.index()];
            let (handle, source_reference, restored_owner) = match source {
                TableSource::Descriptor(owner) => {
                    if *self.descriptor_redirected(owner) {
                        return Err(resource_close_error(
                            DeviceError::fatal(format!(
                                "vendor {name} Win32 handle stays redirected: its {} descriptor was not restored",
                                stream_name(owner)
                            )),
                            DeviceResourceKind::VendorStdio,
                            DeviceResourceClosePhase::RestoreWin32,
                        ));
                    }
                    let restored = native_facts::owned_fd(owner.fd(), standard_reference(owner));
                    let handle = match restored.handle {
                        crate::StdioFact::Known(handle) => handle as isize,
                        crate::StdioFact::Unknown(_) => {
                            return Err(resource_close_error(
                                DeviceError::fatal(format!(
                                    "restored vendor {} descriptor handle is unavailable",
                                    stream_name(owner)
                                )),
                                DeviceResourceKind::VendorStdio,
                                DeviceResourceClosePhase::RestoreWin32,
                            ));
                        }
                    };
                    (handle, standard_reference(owner), Some(restored))
                }
                TableSource::Unowned(handle) => (handle, target, None),
            };
            let capture =
                native_facts::owned_fd(self.capture_fd(stream), capture_reference(stream));
            let before = native_facts::table(which, target, Some(&capture));
            let returned = unsafe { SetStdHandle(which, handle as *mut c_void) };
            let native_error = (returned == 0).then(native_facts::win32_error);
            let mut observation = native_facts::step(
                StdioPhase::Restore,
                StdioApi::SetStdHandle,
                target,
                Some(source_reference),
                i64::from(returned),
                native_error,
            );
            observation.before = Some(before);
            observation.after = Some(native_facts::table(which, target, restored_owner.as_ref()));
            observation.related = restored_owner;
            self.facts.push(observation);
            if returned == 0 {
                return Err(resource_close_error(
                    DeviceError::fatal(format!("failed to restore vendor {name} Win32 handle")),
                    DeviceResourceKind::VendorStdio,
                    DeviceResourceClosePhase::RestoreWin32,
                ));
            }
            *self.table_redirected(stream) = false;
            Ok(())
        }
    }

    const fn table_slot(stream: StdStream) -> u32 {
        match stream {
            StdStream::Stdout => STD_OUTPUT_HANDLE,
            StdStream::Stderr => STD_ERROR_HANDLE,
        }
    }

    const fn stream_name(stream: StdStream) -> &'static str {
        match stream {
            StdStream::Stdout => "stdout",
            StdStream::Stderr => "stderr",
        }
    }

    const fn standard_reference(stream: StdStream) -> StdioReference {
        match stream {
            StdStream::Stdout => StdioReference::Stdout,
            StdStream::Stderr => StdioReference::Stderr,
        }
    }

    const fn saved_reference(stream: StdStream) -> StdioReference {
        match stream {
            StdStream::Stdout => StdioReference::SavedStdout,
            StdStream::Stderr => StdioReference::SavedStderr,
        }
    }

    const fn capture_reference(stream: StdStream) -> StdioReference {
        match stream {
            StdStream::Stdout => StdioReference::CaptureStdout,
            StdStream::Stderr => StdioReference::CaptureStderr,
        }
    }

    const fn table_reference(stream: StdStream) -> StdioReference {
        match stream {
            StdStream::Stdout => StdioReference::Win32Stdout,
            StdStream::Stderr => StdioReference::Win32Stderr,
        }
    }

    impl Drop for RedirectGuard {
        fn drop(&mut self) {
            if !self.restored {
                let _ = self.restore();
            }
        }
    }

    fn dup_fd(
        fd: i32,
        name: &str,
        source: StdioReference,
        target: StdioReference,
        facts: &mut VendorStdioFacts,
    ) -> DeviceResult<i32> {
        let duplicated = unsafe { _dup(fd) };
        let error = (duplicated < 0).then(native_facts::crt_error);
        let mut observation = native_facts::step(
            StdioPhase::Acquire,
            StdioApi::Dup,
            target,
            Some(source),
            i64::from(duplicated),
            error,
        );
        if duplicated >= 0 {
            observation.after = Some(native_facts::owned_fd(duplicated, target));
            // A successful _dup establishes that the original descriptor was live.
            observation.related = Some(native_facts::owned_fd(fd, source));
        }
        facts.push(observation);
        if duplicated < 0 {
            return Err(DeviceError::fatal(format!(
                "failed to duplicate vendor {name} fd"
            )));
        }
        Ok(duplicated)
    }

    fn dup2_fd(
        source_fd: i32,
        target_fd: i32,
        name: &str,
        source: StdioReference,
        target: StdioReference,
        phase: StdioPhase,
        facts: &mut VendorStdioFacts,
    ) -> DeviceResult<()> {
        let target_handle = unsafe { _get_osfhandle(target_fd) };
        let target_handle_error = (target_handle == -1).then(native_facts::crt_error);
        let before = native_facts::owned_handle(
            target_fd,
            target,
            target_handle,
            target_handle_error.clone(),
        );
        let mut restore_flags = None;
        if phase == StdioPhase::Restore {
            // _dup2 ignores its internal target close error. Retire this live,
            // owned CRT target explicitly and never query or close its old HANDLE again.
            let installed = facts.steps.iter().rev().find(|step| {
                step.phase == StdioPhase::Install
                    && step.api == StdioApi::Dup2
                    && step.target == target
            });
            let owned = installed
                .and_then(|step| step.after.as_ref())
                .is_some_and(|expected| {
                    matches!(before.handle, crate::StdioFact::Known(_))
                        && matches!(before.file_identity, crate::StdioFact::Known(_))
                        && before.handle == expected.handle
                        && before.file_identity == expected.file_identity
                });
            restore_flags = installed
                .and_then(|step| step.before.as_ref())
                .and_then(|original| match original.flags {
                    crate::StdioFact::Known(flags) => Some(flags),
                    _ => None,
                });
            if !owned || restore_flags.is_none() {
                let mut observation = native_facts::step(
                    phase,
                    StdioApi::GetOsfhandle,
                    target,
                    None,
                    target_handle as i64,
                    target_handle_error,
                );
                observation.after = Some(before);
                facts.push(observation);
                return Err(DeviceError::fatal(
                    "restore target handle ownership is unconfirmed",
                ));
            }
            close_fd(target_fd, name, target, phase, facts)?;
        }
        let returned = unsafe { _dup2(source_fd, target_fd) };
        let error = (returned != 0).then(native_facts::crt_error);
        let mut observation = native_facts::step(
            phase,
            StdioApi::Dup2,
            target,
            Some(source),
            i64::from(returned),
            error,
        );
        observation.before = Some(before);
        if phase == StdioPhase::Restore {
            observation.target_retirement =
                Some(crate::StdioTargetRetirement::ClosedBeforeReplacement);
        }
        if returned == 0 {
            let current = native_facts::owned_fd(target_fd, target);
            let (which, reference) = if target_fd == STDOUT_FD {
                (STD_OUTPUT_HANDLE, StdioReference::Win32Stdout)
            } else {
                (STD_ERROR_HANDLE, StdioReference::Win32Stderr)
            };
            observation.related = Some(native_facts::table(which, reference, Some(&current)));
            observation.after = Some(current);
        }
        facts.push(observation);
        if returned != 0 {
            return Err(DeviceError::fatal(format!(
                "failed to redirect vendor {name} fd"
            )));
        }
        if matches!(phase, StdioPhase::Install | StdioPhase::Restore) {
            use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
            let before = native_facts::owned_fd(target_fd, target);
            let handle = match before.handle {
                crate::StdioFact::Known(handle) => handle,
                crate::StdioFact::Unknown(_) => {
                    return Err(DeviceError::fatal(
                        "redirected vendor stdio handle is unavailable",
                    ));
                }
            };
            let flags = restore_flags.unwrap_or(0) & HANDLE_FLAG_INHERIT;
            let returned =
                unsafe { SetHandleInformation(handle as *mut c_void, HANDLE_FLAG_INHERIT, flags) };
            let error = (returned == 0).then(native_facts::win32_error);
            let mut observation = native_facts::step(
                phase,
                StdioApi::SetHandleInformation,
                target,
                None,
                i64::from(returned),
                error,
            );
            observation.before = Some(before);
            observation.after = Some(native_facts::owned_fd(target_fd, target));
            facts.push(observation);
            if returned == 0 {
                return Err(DeviceError::fatal(
                    "failed to set vendor stdio handle inheritance",
                ));
            }
        }
        Ok(())
    }

    fn close_fd(
        fd: i32,
        name: &str,
        reference: StdioReference,
        phase: StdioPhase,
        facts: &mut VendorStdioFacts,
    ) -> DeviceResult<()> {
        if fd < 0 {
            return Ok(());
        }
        let before = native_facts::owned_fd(fd, reference);
        let returned = unsafe { _close(fd) };
        let error = (returned != 0).then(native_facts::crt_error);
        let mut observation = native_facts::step(
            phase,
            StdioApi::Close,
            reference,
            None,
            i64::from(returned),
            error,
        );
        observation.before = Some(before);
        // _close retires the CRT slot even on an OS close failure. No after-query.
        let (which, table) = match reference {
            StdioReference::Stdout
            | StdioReference::SavedStdout
            | StdioReference::CaptureStdout => (STD_OUTPUT_HANDLE, StdioReference::Win32Stdout),
            _ => (STD_ERROR_HANDLE, StdioReference::Win32Stderr),
        };
        // The borrowed table may still contain a retired handle number. Read only
        // its value; do not query metadata through that number or through the FD.
        observation.related = Some(native_facts::table(which, table, None));
        facts.push(observation);
        if returned != 0 {
            return Err(DeviceError::fatal(format!(
                "failed to close vendor {name} fd"
            )));
        }
        Ok(())
    }

    fn set_std_handle(
        std_handle: u32,
        fd: i32,
        name: &str,
        source: StdioReference,
        facts: &mut VendorStdioFacts,
    ) -> DeviceResult<()> {
        let handle = unsafe { _get_osfhandle(fd) };
        let error = (handle == -1).then(native_facts::crt_error);
        let mut observation = native_facts::step(
            StdioPhase::Install,
            StdioApi::GetOsfhandle,
            source,
            None,
            handle as i64,
            error.clone(),
        );
        let owner = native_facts::owned_handle(fd, source, handle, error);
        observation.after = Some(owner.clone());
        facts.push(observation);
        if handle == -1 {
            return Err(DeviceError::fatal(format!(
                "failed to get vendor {name} OS handle"
            )));
        }
        let returned = unsafe { SetStdHandle(std_handle, handle as *mut c_void) };
        let error = (returned == 0).then(native_facts::win32_error);
        let target = if std_handle == STD_OUTPUT_HANDLE {
            StdioReference::Win32Stdout
        } else {
            StdioReference::Win32Stderr
        };
        let mut observation = native_facts::step(
            StdioPhase::Install,
            StdioApi::SetStdHandle,
            target,
            Some(source),
            i64::from(returned),
            error,
        );
        observation.after = Some(native_facts::table(std_handle, target, Some(&owner)));
        facts.push(observation);
        if returned == 0 {
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

    fn flush_recorded(phase: StdioPhase, facts: &mut VendorStdioFacts) -> DeviceResult<()> {
        let returned = unsafe { fflush(std::ptr::null_mut()) };
        let error = (returned != 0).then(native_facts::crt_error);
        facts.push(native_facts::step(
            phase,
            StdioApi::Flush,
            StdioReference::All,
            None,
            i64::from(returned),
            error,
        ));
        if returned != 0 {
            return Err(DeviceError::fatal("failed to flush vendor stdio"));
        }
        Ok(())
    }

    fn get_std_handle(
        which: u32,
        fd: i32,
        owner: StdioReference,
        target: StdioReference,
        facts: &mut VendorStdioFacts,
    ) -> *mut c_void {
        let handle = unsafe { GetStdHandle(which) };
        let error = (handle as isize == -1).then(native_facts::win32_error);
        let mut observation = native_facts::step(
            StdioPhase::Acquire,
            StdioApi::GetStdHandle,
            target,
            None,
            handle as i64,
            error.clone(),
        );
        let owner = native_facts::owned_fd(fd, owner);
        observation.after = Some(native_facts::table_value(
            handle as isize,
            error,
            target,
            Some(&owner),
        ));
        observation.related = Some(owner);
        facts.push(observation);
        handle
    }

    fn unlink(
        path: &Path,
        reference: StdioReference,
        phase: StdioPhase,
        facts: &mut VendorStdioFacts,
    ) {
        let result = std::fs::remove_file(path);
        let error = result
            .as_ref()
            .err()
            .map(|error| crate::StdioNativeError::Io {
                code: error.raw_os_error(),
            });
        facts.push(native_facts::step(
            phase,
            StdioApi::Unlink,
            reference,
            None,
            if result.is_ok() { 0 } else { -1 },
            error.clone(),
        ));
        facts.paths.push(crate::StdioPathFact {
            reference,
            path_utf16: path.as_os_str().encode_wide().collect(),
            removal: match error {
                Some(error) => crate::StdioPathRemoval::Residual(error),
                None => crate::StdioPathRemoval::Removed,
            },
        });
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
        descriptors: &[(i32, &str, StdioReference)],
        paths: &[(&PathBuf, StdioReference)],
        facts: &mut VendorStdioFacts,
    ) -> DeviceError {
        for (descriptor, name, reference) in descriptors {
            merge_close_result(
                &mut primary,
                close_fd(
                    *descriptor,
                    name,
                    *reference,
                    StdioPhase::AcquisitionCleanup,
                    facts,
                )
                .map_err(|error| {
                    resource_close_error(
                        error,
                        DeviceResourceKind::FileDescriptor,
                        DeviceResourceClosePhase::AcquisitionCleanup,
                    )
                }),
            );
        }
        for (path, reference) in paths {
            unlink(path, *reference, StdioPhase::AcquisitionCleanup, facts);
        }
        native_facts::probe_residue(facts);
        primary.with_vendor_stdio_facts(std::sync::Arc::new(facts.clone()))
    }

    fn open_capture_file(
        path: &Path,
        reference: StdioReference,
        facts: &mut VendorStdioFacts,
    ) -> DeviceResult<i32> {
        use windows_sys::Win32::Foundation::{
            CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
        };
        use windows_sys::Win32::Storage::FileSystem::{
            CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        };
        let wide = wide_path(path);
        // Null security attributes create a non-inheritable handle. Preserve
        // read/write access and sharing, including deletion by this owner.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                CREATE_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        let error = (handle == INVALID_HANDLE_VALUE).then(native_facts::win32_error);
        facts.push(native_facts::step(
            StdioPhase::Acquire,
            StdioApi::CreateFile,
            reference,
            None,
            handle as i64,
            error,
        ));
        if handle == INVALID_HANDLE_VALUE {
            return Err(DeviceError::fatal(format!(
                "failed to create vendor stdio capture file {}",
                path.display()
            )));
        }
        let fd = unsafe { _open_osfhandle(handle as isize, O_BINARY | O_NOINHERIT) };
        let error = (fd < 0).then(native_facts::crt_error);
        let mut observation = native_facts::step(
            StdioPhase::Acquire,
            StdioApi::Open,
            reference,
            None,
            i64::from(fd),
            error,
        );
        if fd >= 0 {
            observation.after = Some(native_facts::owned_fd(fd, reference));
        }
        facts.push(observation);
        if fd < 0 {
            // CRT did not acquire this newly created handle. It is still ours;
            // this is not a lookup or close through a retired handle value.
            let returned = unsafe { CloseHandle(handle) };
            let error = (returned == 0).then(native_facts::win32_error);
            facts.push(native_facts::step(
                StdioPhase::AcquisitionCleanup,
                StdioApi::CloseHandle,
                reference,
                None,
                i64::from(returned),
                error,
            ));
            let mut failure = DeviceError::fatal(format!(
                "failed to open vendor stdio capture file {}",
                path.display()
            ));
            if returned == 0 {
                failure = resource_close_error(
                    failure,
                    DeviceResourceKind::FileDescriptor,
                    DeviceResourceClosePhase::AcquisitionCleanup,
                );
            }
            // The creation succeeded even when CRT attachment failed.
            unlink(path, reference, StdioPhase::AcquisitionCleanup, facts);
            return Err(failure);
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

        // Workflow #269 NEMU-STDIO-FACTS-v1: existing owner operations supply facts.
        let guard = session.guard.as_mut().expect("active owner");
        guard.finish().expect("original close path");
        let facts = guard.facts().clone();
        assert_eq!(facts.process_id, std::process::id());
        assert!(
            matches!(facts.process_created_filetime, crate::StdioFact::Known(value) if value > 0)
        );
        assert_eq!(facts.steps.len(), 32);
        assert_eq!(facts.dropped_count, 0);
        for reference in [
            crate::StdioReference::CaptureStdout,
            crate::StdioReference::CaptureStderr,
        ] {
            let opened = facts
                .steps
                .iter()
                .find(|step| step.api == crate::StdioApi::Open && step.target == reference)
                .and_then(|step| step.after.as_ref())
                .expect("opened capture FD");
            assert!(matches!(opened.file_identity, crate::StdioFact::Known(_)));
            assert!(matches!(opened.flags, crate::StdioFact::Known(flags) if flags & 1 == 0));
            let redirected = facts
                .steps
                .iter()
                .find(|step| step.api == crate::StdioApi::Dup2 && step.source == Some(reference))
                .and_then(|step| step.after.as_ref())
                .expect("actual standard FD duplicate");
            assert_eq!(opened.file_identity, redirected.file_identity);
        }
        let closes = facts
            .steps
            .iter()
            .filter(|step| step.api == crate::StdioApi::Close)
            .collect::<Vec<_>>();
        assert_eq!(closes.len(), 6);
        for close in closes {
            assert_eq!(close.returned, 0);
            assert!(close.error.is_none());
            assert!(
                close
                    .before
                    .as_ref()
                    .is_some_and(|value| value.fd.is_some())
            );
            assert!(close.after.is_none(), "a retired FD is never queried");
            assert!(
                close
                    .related
                    .as_ref()
                    .is_some_and(|value| value.fd.is_none())
            );
        }
        guard.finish().expect("cached guard finish");
        assert_eq!(&facts, guard.facts());
        let first = session.finish().expect("session finish");
        assert_eq!(first, session.finish().expect("cached session finish"));
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
