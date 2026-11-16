// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 KB-1 (0.12.69; register §1.447; the owner's ruling on board #463 that a package
//! may press the back key, the coordinator's rulings of 11-16): the Nemu input sends the back key
//! through the library's key calls, on the step's frame as a tap.
//!
//! The device is HS-4's fake (`gate_hs4::FakeDevice`), which now records the key calls too and
//! says whether the library offers them. What the worker runs for a key is [`press_key`]
//! (`NemuIpcWorkerState::input_key`); nothing here loads the vendor library, sleeps or reads a
//! clock: the input's check answers a fixed remaining time.
//!
//! - The back key (its word and the two Android spellings a client sends) is one key down and
//!   one key up with the vendor's key code, on the display of the frame, once the frame's fence
//!   and the deadline passed; without a frame, on a frame of an earlier display generation, or
//!   with a deadline that cannot cover the final up's reserve, nothing is called.
//! - Every other key is refused before any call with the error every key had before, and so is
//!   the back key on a library without the two key calls; `text` stays unsupported.

use super::gate_hs4::{Call, DISPLAY, FakeDevice, context, running};
use super::*;

/// The vendor's key code of the back key, as the library's header names it (Linux `KEY_BACK`).
const VENDOR_BACK: i32 = 158;

/// An input's check that always answers `remaining`.
struct Left(Duration);

impl InputOperationCheck for Left {
    fn check(&self, _phase: InputCheckPhase) -> DeviceResult<Duration> {
        Ok(self.0)
    }
}

// The back key, by its word and by the two Android spellings a client sends today (actinglab's
// `key back` sends `4`), is one key down and one key up with the vendor's key code, on the
// display the frame was captured on; the finger's contact is not touched.
#[test]
fn kb1_back_is_one_key_down_and_one_key_up_with_the_vendors_code_on_the_frame() {
    assert_eq!(nemu_key_code(DeviceKey::Back), VENDOR_BACK);
    for key in ["back", "4", "KEYCODE_BACK"] {
        assert_eq!(
            NemuInputSupport::of(&InputAction::Key {
                key: key.to_owned()
            }),
            NemuInputSupport::InFrame,
            "{key}: on the step's frame"
        );
        let mut device = FakeDevice::new();
        let frame = device.frame();
        if let Err(error) = press_key(&mut device, key, &context(Some(frame)), &running()) {
            panic!("{key}: the back key is sent: {error:?}");
        }
        assert_eq!(
            device.calls,
            [Call::KeyDown(VENDOR_BACK), Call::KeyUp(VENDOR_BACK)],
            "{key}"
        );
        assert_eq!(device.touched_displays, [DISPLAY, DISPLAY], "{key}");
        assert_eq!(device.input().contact, Contact::NotStarted, "{key}");
    }
}

// The key is bound to its step's frame as a tap is: without a frame, on a frame of an earlier
// display generation, or with a deadline that cannot cover the final up's reserve (the shutdown
// timeout, 1 s) nothing is called; a deadline one millisecond longer than the reserve is enough.
#[test]
fn kb1_back_is_refused_before_any_call_off_its_frame_or_its_deadline() {
    let mut device = FakeDevice::new();
    press_key(&mut device, "back", &context(None), &running())
        .expect_err("a key needs its step's frame");
    assert!(device.calls.is_empty(), "{:?}", device.calls);

    let mut device = FakeDevice::new();
    let frame = device.frame();
    device
        .input
        .as_mut()
        .expect("the paired input")
        .invalidate()
        .expect("a new display generation");
    press_key(&mut device, "back", &context(Some(frame)), &running())
        .expect_err("a frame of an earlier display generation is refused");
    assert!(device.calls.is_empty(), "{:?}", device.calls);

    for (remaining_ms, sent) in [(1_000, false), (1_001, true)] {
        let mut device = FakeDevice::new();
        let frame = device.frame();
        let context = InputExecutionContext {
            geometry: Some(frame),
            check: Arc::new(Left(Duration::from_millis(remaining_ms))),
        };
        let result = press_key(&mut device, "back", &context, &running());
        assert_eq!(result.is_ok(), sent, "{remaining_ms} ms left: {result:?}");
        if sent {
            assert_eq!(
                device.calls,
                [Call::KeyDown(VENDOR_BACK), Call::KeyUp(VENDOR_BACK)]
            );
        } else {
            assert!(device.calls.is_empty(), "{:?}", device.calls);
        }
    }
}

// Every other key is refused before anything is called, with the error the input answered every
// key with before (`unsupported`), and the host's gate reads it unsupported; `text` stays
// unsupported.
#[test]
fn kb1_another_key_and_text_are_refused_before_any_call() {
    for key in ["home", "3", "KEYCODE_HOME", "BACK", "Back", "menu", "neutral"] {
        assert_eq!(
            NemuInputSupport::of(&InputAction::Key {
                key: key.to_owned()
            }),
            NemuInputSupport::Unsupported,
            "{key}"
        );
        let mut device = FakeDevice::new();
        let frame = device.frame();
        let error = press_key(&mut device, key, &context(Some(frame)), &running())
            .expect_err("another key is refused");
        assert_eq!(error, unsupported(), "{key}");
        assert!(device.calls.is_empty(), "{key}: {:?}", device.calls);
    }
    assert_eq!(
        NemuInputSupport::of(&InputAction::Text {
            text: "neutral".to_owned()
        }),
        NemuInputSupport::Unsupported
    );
}

// A library without the two key calls sends no key: the back key is refused before any call with
// the error every key had before, and the key is not even looked for on the frame.
#[test]
fn kb1_a_library_without_the_key_calls_answers_as_before() {
    let mut device = FakeDevice::new();
    device.key_calls = false;
    let frame = device.frame();
    let error = press_key(&mut device, "back", &context(Some(frame)), &running())
        .expect_err("no key without the library's key calls");
    assert_eq!(error, unsupported());
    assert!(device.calls.is_empty(), "{:?}", device.calls);
    assert!(device.touched_displays.is_empty());
}
