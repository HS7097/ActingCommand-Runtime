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
//!
//! Fix round 1 (the coordinator's rulings R1, R2 and R5 of 11-16; the fake's key calls now answer
//! the code a test gives them):
//!
//! - A key down the library does not confirm still sends the key up, once.
//! - A key whose check fails between its key down and its key up stays recorded as possibly down:
//!   the frame's fence refuses every input on it until the next reset, which sends one key up.
//! - The back key on a library without the two key calls is the input capability's refusal
//!   (`input_unsupported`), not a device failure: nothing called, nothing left unconfirmed.

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

/// Fix round 1: an input's check whose deadline has passed by the final up: it answers time left
/// while the input goes on and none at the final up, as a deadline that runs out between a key
/// down and its key up.
struct ExpiresAtFinalUp;

impl InputOperationCheck for ExpiresAtFinalUp {
    fn check(&self, phase: InputCheckPhase) -> DeviceResult<Duration> {
        Ok(match phase {
            InputCheckPhase::Continue => Duration::from_millis(5_000),
            InputCheckPhase::FinalUp => Duration::ZERO,
        })
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
    for key in [
        "home",
        "3",
        "KEYCODE_HOME",
        "BACK",
        "Back",
        "menu",
        "neutral",
    ] {
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
// the error every key had before, and the key is not even looked for on the frame. Adapted by fix
// round 1 (R2: the answer is the capability's denial, nothing performed, as for a key before this
// slice): the refusal is the input capability's (`input_unsupported`), which the Runtime answers
// as a denial; it leaves nothing unconfirmed and records no key.
#[test]
fn kb1_a_library_without_the_key_calls_answers_as_before() {
    let mut device = FakeDevice::new();
    device.key_calls = false;
    let frame = device.frame();
    let error = press_key(&mut device, "back", &context(Some(frame)), &running())
        .expect_err("no key without the library's key calls");
    assert_eq!(error, unsupported());
    assert!(
        error.input_unsupported(),
        "the capability's refusal: {error:?}"
    );
    assert_ne!(
        error.resource_quiescence(),
        Some(DeviceResourceQuiescence::Unconfirmed),
        "{error:?}"
    );
    assert!(device.calls.is_empty(), "{:?}", device.calls);
    assert!(device.touched_displays.is_empty());
    assert_eq!(device.input().unreleased_key(), None);
}

// Fix round 1 (R1, R5): a key down the library answers with another code than 0 fails the key, and
// the key up is still called once with the same code; it is confirmed, so the key is released and
// the next input on a frame is admitted.
#[test]
fn kb1_fix1_a_failed_key_down_still_sends_the_key_up() {
    let mut device = FakeDevice::new();
    device.key_down_code = 7;
    let frame = device.frame();
    let error = press_key(&mut device, "back", &context(Some(frame)), &running())
        .expect_err("a key down the library does not confirm fails the key");
    assert_eq!(error.message(), "Nemu key down failed with code 7");
    assert_eq!(
        device.calls,
        [Call::KeyDown(VENDOR_BACK), Call::KeyUp(VENDOR_BACK)]
    );
    assert_eq!(
        device.input().unreleased_key(),
        None,
        "the key up released it"
    );

    let frame = device.frame();
    perform_events(
        &mut device,
        &long_tap_events((10, 10), 50),
        &context(Some(frame)),
        &running(),
    )
    .expect("the next input is admitted");
    assert_eq!(
        device.calls,
        [
            Call::KeyDown(VENDOR_BACK),
            Call::KeyUp(VENDOR_BACK),
            Call::Down(10, 10),
            Call::Wait(50),
            Call::Up
        ]
    );
}

// Fix round 1 (R1, R5): a key whose check fails between its key down and its key up (its deadline
// gone by the final up) fails unconfirmed, its key up not called, and stays recorded as possibly
// down. While it is, the frame's fence refuses a touch and another key on a current frame, nothing
// called. The next reset, which needs no frame, sends one key up with the key's code and releases
// it, leaving the finger as it was; the next input on a frame is then admitted.
#[test]
fn kb1_fix1_a_key_left_down_is_lifted_by_the_next_reset_and_fences_input_until_then() {
    let mut device = FakeDevice::new();
    let frame = device.frame();
    let expiring = InputExecutionContext {
        geometry: Some(frame),
        check: Arc::new(ExpiresAtFinalUp),
    };
    let error = press_key(&mut device, "back", &expiring, &running())
        .expect_err("the check before the key up fails the key");
    assert_eq!(
        error.resource_quiescence(),
        Some(DeviceResourceQuiescence::Unconfirmed),
        "{error:?}"
    );
    assert_eq!(device.calls, [Call::KeyDown(VENDOR_BACK)], "no key up");
    assert_eq!(
        device.input().unreleased_key(),
        Some(PressedKey {
            vendor_key: VENDOR_BACK,
            state: Contact::MayBeDown,
        })
    );

    let frame = device.frame();
    perform_events(
        &mut device,
        &long_tap_events((10, 10), 50),
        &context(Some(Arc::clone(&frame))),
        &running(),
    )
    .expect_err("the fence refuses a touch while the key may be down");
    press_key(&mut device, "back", &context(Some(frame)), &running())
        .expect_err("the fence refuses a key while the key may be down");
    assert_eq!(device.calls, [Call::KeyDown(VENDOR_BACK)], "nothing called");

    reset_contact(&mut device, &context(None), &running()).expect("the reset lifts the key");
    assert_eq!(
        device.calls,
        [Call::KeyDown(VENDOR_BACK), Call::KeyUp(VENDOR_BACK)],
        "one key up"
    );
    assert_eq!(device.input().unreleased_key(), None);
    assert_eq!(
        device.input().contact,
        Contact::NotStarted,
        "the finger untouched"
    );

    let frame = device.frame();
    perform_events(
        &mut device,
        &long_tap_events((10, 10), 50),
        &context(Some(frame)),
        &running(),
    )
    .expect("the next input is admitted");
    assert_eq!(device.calls.len(), 5, "{:?}", device.calls);
}
