// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 A36a (0.12.9): the stdio redirect around the vendor capture library.
//!
//! - T1 runs the session's own install and restore orders (`redirect_order`) in an in-memory
//!   process: its OS handle table, its CRT descriptors and its two Win32 standard-handle slots,
//!   with the UCRT behaviour the redirect meets. Another file takes every number the install
//!   closed, and another thread writes a log line through the Win32 slots after every step.
//! - T2 and T3 drive the real session against this test process's own standard streams and the
//!   process-wide redirect gate (Windows).

use super::redirect_order::{
    INSTALL_ORDER, RESTORE_ORDER, StdStream, StdTarget, TableSource, table_source,
};
#[cfg(windows)]
use super::{VENDOR_STDIO_REDIRECT_WAIT, VendorStdioSession, stdio_gate};
use std::collections::BTreeMap;
#[cfg(windows)]
use std::sync::mpsc;
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::time::Duration;

/// A file object of the in-memory process.
type FileObject = &'static str;

const ORIGINAL_STDOUT: FileObject = "original stdout";
const ORIGINAL_STDERR: FileObject = "original stderr";
const STDOUT_CAPTURE: FileObject = "stdout capture file";
const STDERR_CAPTURE: FileObject = "stderr capture file";
const OTHER_FILE: FileObject = "another file of the process";

/// An in-memory process with the UCRT behaviour the redirect meets: `_close` of FD1 or FD2
/// keeps a handle the other one also owns open, and a console-subsystem CRT keeps the Win32
/// slots of FD1/2 in step (closing the descriptor clears its slot, `_dup2` onto it writes the new
/// handle into it).
struct FakeProcess {
    crt_keeps_slots: bool,
    next_handle: isize,
    /// Open OS handles and the file object each refers to.
    handles: BTreeMap<isize, FileObject>,
    /// CRT descriptors and the OS handle each owns.
    descriptors: BTreeMap<i32, isize>,
    /// The Win32 STD_OUTPUT_HANDLE and STD_ERROR_HANDLE slots; 0 is NULL.
    slots: [isize; 2],
    /// Log lines each file object received through the Win32 slots.
    received: BTreeMap<FileObject, usize>,
}

impl FakeProcess {
    /// A process whose FD1/2 own the handles its Win32 slots hold, as the CRT sets them up at
    /// start. `shared`: the launcher passed one handle for both streams.
    fn start(crt_keeps_slots: bool, shared: bool) -> Self {
        let mut process = Self {
            crt_keeps_slots,
            next_handle: 0x100,
            handles: BTreeMap::new(),
            descriptors: BTreeMap::new(),
            slots: [0, 0],
            received: BTreeMap::new(),
        };
        let stdout = process.open(ORIGINAL_STDOUT);
        let stderr = if shared {
            stdout
        } else {
            process.open(ORIGINAL_STDERR)
        };
        process.descriptors.insert(1, stdout);
        process.descriptors.insert(2, stderr);
        process.slots = [stdout, stderr];
        process
    }

    fn open(&mut self, object: FileObject) -> isize {
        let handle = self.next_handle;
        self.next_handle += 4;
        self.handles.insert(handle, object);
        handle
    }

    /// Another file of the process gets a number that is free now.
    fn open_reusing(&mut self, number: isize, object: FileObject) {
        assert!(
            self.handles.insert(number, object).is_none(),
            "handle {number:#x} is still open"
        );
    }

    /// `_get_osfhandle`
    fn owned(&self, descriptor: i32) -> isize {
        *self
            .descriptors
            .get(&descriptor)
            .unwrap_or_else(|| panic!("descriptor {descriptor} is not open"))
    }

    /// `_open_osfhandle`
    fn attach(&mut self, handle: isize) -> i32 {
        let descriptor = (3..)
            .find(|candidate| !self.descriptors.contains_key(candidate))
            .expect("a free descriptor");
        self.descriptors.insert(descriptor, handle);
        descriptor
    }

    /// `_dup`
    fn crt_dup(&mut self, descriptor: i32) -> i32 {
        let object = self.handles[&self.owned(descriptor)];
        let handle = self.open(object);
        self.attach(handle)
    }

    /// `_close`
    fn crt_close(&mut self, descriptor: i32) {
        let handle = self
            .descriptors
            .remove(&descriptor)
            .unwrap_or_else(|| panic!("descriptor {descriptor} is not open"));
        let other = match descriptor {
            1 => Some(2),
            2 => Some(1),
            _ => None,
        };
        let shared = other.is_some_and(|other| self.descriptors.get(&other) == Some(&handle));
        if !shared {
            assert!(
                self.handles.remove(&handle).is_some(),
                "handle {handle:#x} was already closed"
            );
        }
        if self.crt_keeps_slots && matches!(descriptor, 1 | 2) {
            self.slots[descriptor as usize - 1] = 0;
        }
    }

    /// `_dup2`
    fn crt_dup2(&mut self, source: i32, target: i32) {
        if self.descriptors.contains_key(&target) {
            self.crt_close(target);
        }
        let object = self.handles[&self.owned(source)];
        let handle = self.open(object);
        self.descriptors.insert(target, handle);
        if self.crt_keeps_slots && matches!(target, 1 | 2) {
            self.slots[target as usize - 1] = handle;
        }
    }

    /// `SetStdHandle`
    fn set_slot(&mut self, stream: StdStream, handle: isize) {
        self.slots[stream.index()] = handle;
    }

    /// Another thread writes one log line to each Win32 standard handle. A closed number writes
    /// nowhere; a number another file took writes into that file.
    fn log_from_another_thread(&mut self) {
        for slot in self.slots {
            if let Some(&object) = self.handles.get(&slot) {
                *self.received.entry(object).or_default() += 1;
            }
        }
    }

    fn lines_received(&self, object: FileObject) -> usize {
        self.received.get(object).copied().unwrap_or_default()
    }

    fn slot_object(&self, stream: StdStream) -> Option<FileObject> {
        self.handles.get(&self.slots[stream.index()]).copied()
    }

    fn descriptor_object(&self, descriptor: i32) -> Option<FileObject> {
        self.descriptors
            .get(&descriptor)
            .and_then(|handle| self.handles.get(handle))
            .copied()
    }
}

/// The session's descriptors and slot sources, step by step as `imp::RedirectGuard` runs them.
struct FakeRedirect {
    saved: [i32; 2],
    capture: [i32; 2],
    table_sources: [TableSource; 2],
}

impl FakeRedirect {
    /// `RedirectGuard::new`: `_dup` of FD1 and FD2, the two capture files, then both slots read
    /// beside the handles FD1 and FD2 own.
    fn acquire(process: &mut FakeProcess) -> Self {
        let saved = [process.crt_dup(1), process.crt_dup(2)];
        let capture = [STDOUT_CAPTURE, STDERR_CAPTURE].map(|object| {
            let handle = process.open(object);
            process.attach(handle)
        });
        let (stdout_owned, stderr_owned) = (process.owned(1), process.owned(2));
        let table_sources = [StdStream::Stdout, StdStream::Stderr].map(|stream| {
            table_source(
                stream,
                process.slots[stream.index()],
                stdout_owned,
                stderr_owned,
            )
        });
        Self {
            saved,
            capture,
            table_sources,
        }
    }

    /// One step of `RedirectGuard::install`.
    fn install(&self, process: &mut FakeProcess, target: StdTarget) {
        match target {
            StdTarget::Descriptor(stream) => {
                process.crt_dup2(self.capture[stream.index()], stream.fd());
            }
            StdTarget::Table(stream) => {
                let handle = process.owned(self.capture[stream.index()]);
                process.set_slot(stream, handle);
            }
        }
    }

    /// One step of `RedirectGuard::restore_targets`.
    fn restore(&self, process: &mut FakeProcess, target: StdTarget) {
        match target {
            StdTarget::Descriptor(stream) => {
                process.crt_close(stream.fd());
                process.crt_dup2(self.saved[stream.index()], stream.fd());
            }
            StdTarget::Table(stream) => {
                let handle = match self.table_sources[stream.index()] {
                    TableSource::Descriptor(owner) => process.owned(owner.fd()),
                    TableSource::Unowned(value) => value,
                };
                process.set_slot(stream, handle);
            }
        }
    }
}

/// T1 (requirement 1): install, then another file takes every number the install closed, then
/// restore. After restore the process standard handles are the original objects, and the other
/// file never receives a line written through them, at any step. Both CRT kinds, with one
/// handle per stream and with one handle shared by both streams.
#[test]
fn a36a_t1_restore_returns_the_original_objects_and_never_writes_a_file_that_took_a_closed_number()
{
    for crt_keeps_slots in [true, false] {
        for shared in [false, true] {
            let case = format!(
                "CRT keeps the Win32 slots in step: {crt_keeps_slots}; one handle for both streams: {shared}"
            );
            let mut process = FakeProcess::start(crt_keeps_slots, shared);
            let original_numbers = process.slots;
            let stderr_object = if shared {
                ORIGINAL_STDOUT
            } else {
                ORIGINAL_STDERR
            };
            let redirect = FakeRedirect::acquire(&mut process);
            for target in INSTALL_ORDER {
                redirect.install(&mut process, target);
                process.log_from_another_thread();
            }
            let mut reused = 0;
            for number in original_numbers {
                if !process.handles.contains_key(&number) {
                    process.open_reusing(number, OTHER_FILE);
                    reused += 1;
                }
            }
            assert!(
                reused > 0,
                "{case}: the install closes an original standard handle"
            );
            for target in RESTORE_ORDER {
                redirect.restore(&mut process, target);
                process.log_from_another_thread();
            }

            assert_eq!(
                process.lines_received(OTHER_FILE),
                0,
                "{case}: a line written through a process standard handle reached the file that \
                 took a number the install closed"
            );
            assert_eq!(
                [
                    process.slot_object(StdStream::Stdout),
                    process.slot_object(StdStream::Stderr),
                ],
                [Some(ORIGINAL_STDOUT), Some(stderr_object)],
                "{case}: the Win32 standard handles after restore"
            );
            assert_eq!(
                [process.descriptor_object(1), process.descriptor_object(2)],
                [Some(ORIGINAL_STDOUT), Some(stderr_object)],
                "{case}: FD1 and FD2 after restore"
            );
            process.log_from_another_thread();
            assert_eq!(
                process.lines_received(OTHER_FILE),
                0,
                "{case}: a line written after restore reached the other file"
            );
        }
    }
}

/// How long the test waits for a caller to arrive at the gate or to come back from it. Each wait
/// ends as soon as its condition holds; `GATE_WAIT` only bounds a broken run.
#[cfg(windows)]
const GATE_WAIT: Duration = Duration::from_secs(10);

/// T3's bound.
#[cfg(windows)]
const SHORT_BOUND: Duration = Duration::from_millis(25);

/// T2 (requirement 2): two redirect callers never overlap. (a) While a session's redirect is
/// installed (at start and after each resume) the session holds the process-wide gate, and it
/// leaves it when it restores. (b) While another caller holds the gate, a second caller's start
/// waits at it and installs only after the first left.
#[cfg(windows)]
#[test]
fn a36a_t2_two_redirect_callers_are_strictly_serialized_by_the_process_wide_gate() {
    let mut session = VendorStdioSession::start().expect("first caller's redirect");
    assert!(
        stdio_gate().is_held(),
        "an installed redirect does not hold the process-wide gate"
    );
    session
        .suspend()
        .expect("first caller's restore after its call");
    assert!(
        !stdio_gate().is_held(),
        "a restored (suspended) redirect still holds the gate"
    );
    session.resume().expect("first caller's next call");
    assert!(
        stdio_gate().is_held(),
        "a resumed redirect does not hold the process-wide gate"
    );
    session.finish().expect("first caller's close");
    assert!(
        !stdio_gate().is_held(),
        "a closed redirect still holds the gate"
    );

    let first = stdio_gate()
        .enter(VENDOR_STDIO_REDIRECT_WAIT)
        .expect("the first caller enters the free gate");
    let (installed_tx, installed_rx) = mpsc::channel();
    let second = thread::spawn(move || {
        let started = VendorStdioSession::start();
        let held = stdio_gate().is_held();
        let _ = installed_tx.send(());
        let mut session = started.expect("second caller's redirect after the first left");
        session.finish().expect("second caller's restore");
        held
    });
    assert!(
        stdio_gate().wait_for_waiters(1, GATE_WAIT),
        "the second caller never waited at the gate"
    );
    assert!(
        installed_rx.try_recv().is_err(),
        "the second caller installed while the first held the gate"
    );
    drop(first);
    installed_rx
        .recv_timeout(GATE_WAIT)
        .expect("the second caller installs once the first left");
    assert!(
        second.join().expect("second caller"),
        "the second caller's redirect does not hold the gate"
    );
}

/// T3 (requirement 2): a wait past its bound fails with the registered code and redirects
/// nothing: neither a session's start nor a suspended session's resume (one native call) runs
/// unredirected.
#[cfg(windows)]
#[test]
fn a36a_t3_a_redirect_wait_past_its_bound_fails_with_the_registered_code_and_redirects_nothing() {
    let busy = crate::codes::VendorStdioCode::VendorStdioRedirectBusy.as_str();

    let holder = stdio_gate()
        .enter(VENDOR_STDIO_REDIRECT_WAIT)
        .expect("another caller holds the gate");
    let (started_tx, started_rx) = mpsc::channel();
    let starter = thread::spawn(move || {
        let started = VendorStdioSession::start_within(SHORT_BOUND).map(drop);
        let _ = started_tx.send(started);
    });
    let started = started_rx
        .recv_timeout(GATE_WAIT)
        .expect("a start waiting at a held gate ends at its bound");
    drop(holder);
    starter.join().expect("starter");
    let error = started.expect_err("a start past its bound fails instead of redirecting");
    assert!(error.message().starts_with(busy), "{error:?}");

    let (ready_tx, ready_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let (resumed_tx, resumed_rx) = mpsc::channel();
    let caller = thread::spawn(move || {
        let mut session = VendorStdioSession::start().expect("session");
        session
            .suspend()
            .expect("restore after the session's first call");
        let _ = ready_tx.send(());
        go_rx.recv().expect("go");
        let resumed = session.resume_within(SHORT_BOUND);
        let suspended = session
            .guard
            .as_ref()
            .is_some_and(|guard| guard.is_suspended());
        let _ = resumed_tx.send((resumed, suspended));
        session.finish().expect("close the session");
    });
    ready_rx
        .recv_timeout(GATE_WAIT)
        .expect("the session restored after its first call");
    let holder = stdio_gate()
        .enter(VENDOR_STDIO_REDIRECT_WAIT)
        .expect("another caller holds the gate");
    go_tx.send(()).expect("go");
    let (resumed, suspended) = resumed_rx
        .recv_timeout(GATE_WAIT)
        .expect("a resume waiting at a held gate ends at its bound");
    drop(holder);
    caller.join().expect("caller");
    let error = resumed.expect_err("a resume past its bound fails instead of redirecting");
    assert!(error.message().starts_with(busy), "{error:?}");
    assert!(suspended, "a resume past its bound redirected the session");
}
