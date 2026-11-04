// SPDX-License-Identifier: AGPL-3.0-only

//! #402 HS-5a (design `rd-1031/backend-switch/model-hs5.md` §4.1, §4.2, §5.1; register §1.276):
//! a paired Nemu session whose application does not run behaves as an adb one.
//!
//! - D1–D3: the paired session looks its package's display up before every capture and touch.
//!   Without one (the application does not run) it takes the main display, the one a
//!   capture-only session captures, and fails nothing; a display that appears, goes or changes
//!   (also from the main display 0 to the package's display 0) invalidates every earlier frame's
//!   geometry, so a touch decided on such a frame is refused before anything is pressed. A touch
//!   on a frame of the main display presses the main display.
//! - R1–R3: an open that fails before it hands its backend over releases the connection it made
//!   itself, nothing pressed on it, under `LocalOnly`: the disconnect is called once, the handle
//!   is retired and the close is confirmed. A backend handed over is closed under `LocalOnly`
//!   as before: no disconnect, unconfirmed.
//!
//! The paired session's device is HS-4's fake (`gate_hs4::FakeDevice`), which now answers the
//! package's display too; the close runs the worker's own close with an in-process callback in
//! place of `nemu_disconnect`, as the owned-close test does. Nothing loads the vendor library,
//! sleeps or reads a clock.

use super::gate_hs4::{Call, DISPLAY, FakeDevice, HEIGHT, MAIN_DISPLAY, WIDTH, context, running};
use super::*;
use std::sync::atomic::{AtomicI32, AtomicUsize};

/// What Nemu answers for a package that has no display (the application does not run).
const NO_DISPLAY: i32 = -1;

/// A device whose package has the display `package` (`NO_DISPLAY`: none), resolved once as the
/// open's resolution probe does.
fn opened(package: i32) -> FakeDevice {
    let mut device = FakeDevice::new();
    device.package_display = package;
    if let Err(error) = resolve_display(&mut device, None) {
        panic!("the paired session opens with package display {package}: {error:?}");
    }
    device
}

/// A tap's events on the fake: the press, the hold and the release.
fn tap() -> Vec<SegmentedSwipeEvent> {
    long_tap_events((10, 10), 50)
}

// D2: no display for the package: the session addresses the main display, its resolution probe
// and its frames succeed, and no `display_missing` is reported.
#[test]
fn hs5_no_package_display_uses_main_display() {
    assert_eq!(
        nemu_paired_display(NO_DISPLAY, MAIN_DISPLAY).ok(),
        Some(MAIN_DISPLAY),
        "the paired display without a package display is the main display"
    );
    let mut device = FakeDevice::new();
    device.package_display = NO_DISPLAY;
    let resolved = resolve_display(&mut device, None);
    assert!(
        resolved.is_ok(),
        "no display_missing: the session takes the main display: {resolved:?}"
    );
    assert_eq!(device.display, MAIN_DISPLAY);
    assert_eq!(device.input().display, Some(MAIN_DISPLAY));
    let probed = device.touch_resolution(&context(None), &running());
    assert!(
        matches!(probed, Ok((WIDTH, HEIGHT))),
        "the resolution probe succeeds: {probed:?}"
    );
    assert_eq!(device.frame().display_id, MAIN_DISPLAY);
    assert!(
        device.calls.is_empty(),
        "nothing was pressed: {:?}",
        device.calls
    );
}

// D3: the application comes up: the package's display appears, with the main display's number
// (0) or another. The display generation moves on, a touch decided on a frame of the main
// display is refused before any native call, and a frame of the package's display is touched
// there.
#[test]
fn hs5_package_display_appearing_bumps_generation() {
    for package in [MAIN_DISPLAY, DISPLAY] {
        let mut device = opened(NO_DISPLAY);
        let before = device.frame();
        let generation = device.input().generation;
        device.package_display = package;
        perform_events(&mut device, &tap(), &context(Some(before)), &running())
            .expect_err("a frame of the main display no longer admits a touch");
        assert!(
            device.calls.is_empty(),
            "package {package}: nothing pressed: {:?}",
            device.calls
        );
        assert!(
            device.input().generation > generation,
            "package {package}: the generation moved on"
        );
        assert_eq!(device.display, package);

        let after = device.frame();
        if let Err(error) = perform_events(&mut device, &tap(), &context(Some(after)), &running()) {
            panic!("package {package}: a frame of the package's display is touched: {error:?}");
        }
        assert_eq!(device.calls, [Call::Down(10, 10), Call::Wait(50), Call::Up]);
        assert_eq!(device.touched_displays, [package, package]);
    }
}

// D3: the application crashes: its display goes and the session addresses the main display
// again. The generation moves on and a frame of the package's display admits no touch.
#[test]
fn hs5_package_display_lost_bumps_generation() {
    for package in [MAIN_DISPLAY, DISPLAY] {
        let mut device = opened(package);
        let before = device.frame();
        let generation = device.input().generation;
        device.package_display = NO_DISPLAY;
        if let Err(error) = resolve_display(&mut device, None) {
            panic!("package {package}: the session takes the main display: {error:?}");
        }
        assert!(
            device.input().generation > generation,
            "package {package}: the generation moved on"
        );
        assert_eq!(device.display, MAIN_DISPLAY);
        perform_events(&mut device, &tap(), &context(Some(before)), &running())
            .expect_err("a frame of the lost display admits no touch");
        assert!(
            device.calls.is_empty(),
            "package {package}: nothing pressed: {:?}",
            device.calls
        );
    }
}

// D1–D3: a tap on a frame of the main display presses and lifts on the main display.
#[test]
fn hs5_main_display_frame_presses_main_display() {
    let mut device = opened(NO_DISPLAY);
    let frame = device.frame();
    assert_eq!(frame.display_id, MAIN_DISPLAY);
    if let Err(error) = perform_events(&mut device, &tap(), &context(Some(frame)), &running()) {
        panic!("a tap on a frame of the main display: {error:?}");
    }
    assert_eq!(device.calls, [Call::Down(10, 10), Call::Wait(50), Call::Up]);
    assert_eq!(device.touched_displays, [MAIN_DISPLAY, MAIN_DISPLAY]);
}

/// A connection the open made (`connect_id`), its vendor stdio session started, no library
/// loaded (the in-process callback stands for `nemu_disconnect`), with the paired input of HS-4's
/// fake device or none (a capture-only open).
fn connection(connect_id: i32, paired: bool) -> NemuIpcWorkerState {
    NemuIpcWorkerState {
        library: None,
        stdio_session: Some(VendorStdioSession::start().expect("the vendor stdio session")),
        nemu_folder: Vec::new(),
        instance_id: 1,
        display_id: MAIN_DISPLAY,
        connect_id,
        raw_buffer: Vec::new(),
        frame_width: 0,
        frame_height: 0,
        vendor_stdio: VendorStdioLog::default(),
        input: if paired {
            FakeDevice::new().input
        } else {
            None
        },
    }
}

// R1: the open's resolution probe or first frame failed after it connected. The opener closes
// what it made itself, nothing pressed on it, under `LocalOnly`: one disconnect of its handle,
// the handle retired, the vendor stdio session closed, the close confirmed. A paired open and a
// capture-only one alike.
#[test]
fn hs5_failed_open_releases_its_connection() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static LAST_ID: AtomicI32 = AtomicI32::new(0);
    unsafe extern "C" fn released(id: i32) {
        LAST_ID.store(id, Ordering::SeqCst);
        CALLS.fetch_add(1, Ordering::SeqCst);
    }
    for (connect_id, paired) in [(40, true), (41, false)] {
        let before = CALLS.load(Ordering::SeqCst);
        let mut state = connection(connect_id, paired);
        // The worker's close of an opener's Shutdown: the contact first (never pressed), then
        // the connection.
        state
            .close_input_contact(DeviceCloseAuthority::LocalOnly, None, &running())
            .expect("no contact to lift");
        let closed = state.close_with(NemuCloser::Opener, |_| Ok(released as NemuDisconnect));
        let outcome = match closed {
            Ok(outcome) => outcome,
            Err(error) => panic!(
                "paired {paired}: the opener releases its own connection: {error:?}, quiescence {:?}",
                error.resource_quiescence()
            ),
        };
        assert_eq!(outcome.quiescence(), DeviceResourceQuiescence::Confirmed);
        assert_eq!(CALLS.load(Ordering::SeqCst) - before, 1, "one disconnect");
        assert_eq!(LAST_ID.load(Ordering::SeqCst), connect_id);
        assert_eq!(state.connect_id, 0, "the handle is retired");
        assert!(state.stdio_session.is_none());
    }
}

// R2: a backend handed over and closed under `LocalOnly` keeps today's rule: no disconnect, the
// handle kept, unconfirmed (`ProviderConnection`, `DisconnectCall`). R1 holds only for a
// connection nothing was pressed on: an opener's close after a touch is refused the same way.
#[test]
fn hs5_returned_backend_local_close_still_refused() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn released(_id: i32) {
        CALLS.fetch_add(1, Ordering::SeqCst);
    }
    let pressed = || {
        let mut state = connection(43, true);
        state.input.as_mut().expect("the paired input").contact = Contact::Released;
        (NemuCloser::Opener, state)
    };
    let closes = [
        (
            NemuCloser::Holder(DeviceCloseAuthority::LocalOnly),
            connection(42, true),
        ),
        pressed(),
    ];
    for (closer, mut state) in closes {
        let connect_id = state.connect_id;
        let error = state
            .close_with(closer, |_| Ok(released as NemuDisconnect))
            .expect_err("the disconnect needs the ResourceClose witness");
        assert_eq!(
            error.resource_quiescence(),
            Some(DeviceResourceQuiescence::Unconfirmed),
            "{error:?}"
        );
        assert!(
            error
                .resource_close_causes()
                .iter()
                .any(|cause| cause.phase() == DeviceResourceClosePhase::DisconnectCall),
            "{error:?}"
        );
        assert_eq!(CALLS.load(Ordering::SeqCst), 0, "no disconnect");
        assert_eq!(state.connect_id, connect_id, "the handle is kept");
        // Release the in-test opaque handle (it never belonged to a native provider) and close
        // the vendor stdio session.
        state.connect_id = 0;
        state
            .close(DeviceCloseAuthority::LocalOnly)
            .expect("the test's own cleanup");
    }
}
