// SPDX-License-Identifier: AGPL-3.0-only

//! HS-4 (#402; design `rd-1031/backend-switch/model-backend-switch.md` §4.2, §4.4; register
//! §1.268): the Nemu input's reset, long tap and swipe, on a fake device.
//!
//! The fake device is the connection a Nemu touch drives (`NemuTouchPort`): it answers the
//! display and its size, records every native contact call (a finger down at a point, a finger
//! up) and every wait between events, and lifts with the code a test gives it. Nothing here loads
//! the vendor library, sleeps or reads a clock: the input's check answers a fixed remaining time.
//!
//! - The reset lifts a contact whose lift was not confirmed, once, with no frame; a lift the
//!   device does not confirm fails the reset and leaves the contact unconfirmed.
//! - The reset of a released (or never pressed) contact calls nothing.
//! - The long tap is the press, the hold of its duration and the release, on its step's frame,
//!   bound by the geometry fence and by the deadline (§4.2 items 4 and 5) as the tap is.
//! - The swipe is the press at its start, evenly timed moves along the line to its end and the
//!   release, bound the same way.

use super::*;

pub(super) const WIDTH: u32 = 400;
pub(super) const HEIGHT: u32 = 300;
/// The application package's display, as Nemu answers it.
pub(super) const DISPLAY: i32 = 2;
/// #402 HS-5a: the connection's main display.
pub(super) const MAIN_DISPLAY: i32 = 0;
const COMMAND_TIMEOUT_MS: u64 = 12_000;
// The reserve the deadline keeps for the final lift: min(shutdown, command) timeout.
const SHUTDOWN_TIMEOUT_MS: u64 = 1_000;
// What the step's check says is left of its deadline.
const REMAINING_MS: u64 = 5_000;

/// A native call or a wait the fake device saw, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Call {
    Down(i32, i32),
    Up,
    Wait(u64),
}

/// The input's check: it always answers the same remaining time.
struct Remaining(Duration);

impl InputOperationCheck for Remaining {
    fn check(&self, _phase: InputCheckPhase) -> DeviceResult<Duration> {
        Ok(self.0)
    }
}

pub(super) struct FakeDevice {
    pub(super) input: Option<NemuInputState>,
    /// The display the native calls address.
    pub(super) display: i32,
    /// #402 HS-5a: what Nemu answers for the application package's display (negative: none),
    /// and the connection's main display.
    pub(super) package_display: i32,
    pub(super) main_display: i32,
    pub(super) size: (u32, u32),
    // What the native finger up returns; 0 confirms the lift.
    up_code: i32,
    pub(super) calls: Vec<Call>,
    /// #402 HS-5a: the display each native contact call addressed, in order.
    pub(super) touched_displays: Vec<i32>,
}

impl FakeDevice {
    pub(super) fn new() -> Self {
        let application = NemuApplicationTarget::new(
            "neutral.application",
            NemuAppIndex::try_from(0_u32).expect("application index"),
        )
        .expect("application");
        let config = NemuInputConfig {
            command_timeout: Duration::from_millis(COMMAND_TIMEOUT_MS),
            shutdown_timeout: Duration::from_millis(SHUTDOWN_TIMEOUT_MS),
            tap_hold: Duration::from_millis(50),
        };
        Self {
            input: Some(NemuInputState::new(application, config).with_main_display(MAIN_DISPLAY)),
            display: DISPLAY,
            package_display: DISPLAY,
            main_display: MAIN_DISPLAY,
            size: (WIDTH, HEIGHT),
            up_code: 0,
            calls: Vec::new(),
            touched_displays: Vec::new(),
        }
    }

    pub(super) fn input(&self) -> &NemuInputState {
        self.input.as_ref().expect("the paired input")
    }

    fn set_contact(&mut self, contact: Contact) {
        self.input.as_mut().expect("the paired input").contact = contact;
    }

    /// The geometry of a frame the connection captures now, as the worker records it with the
    /// frame (`input_geometry`).
    pub(super) fn frame(&self) -> Arc<NemuFrameGeometry> {
        let input = self.input();
        Arc::new(NemuFrameGeometry {
            owner: Arc::clone(&input.owner),
            generation: input.generation,
            display_id: self.display,
            width: self.size.0,
            height: self.size.1,
        })
    }
}

impl NemuTouchPort for FakeDevice {
    fn touch_input(&mut self) -> Option<&mut NemuInputState> {
        self.input.as_mut()
    }

    fn touch_display(&self) -> i32 {
        self.display
    }

    fn touch_set_display(&mut self, display: i32) {
        self.display = display;
    }

    fn touch_main_display(&self) -> i32 {
        self.main_display
    }

    fn touch_package_display(&mut self) -> DeviceResult<i32> {
        Ok(self.package_display)
    }

    fn touch_display_missing(&self) -> crate::CaptureOpenFailure {
        crate::CaptureOpenFailure {
            cause: CaptureOpenCause::DisplayMissing,
            application_id: None,
            instance_index: None,
        }
    }

    /// #402 HS-5a: the display is resolved as the worker resolves it, then the frame's size.
    fn touch_resolution(
        &mut self,
        context: &InputExecutionContext,
        stopped: &AtomicBool,
    ) -> DeviceResult<(u32, u32)> {
        resolve_display(self, Some((context, stopped)))?;
        Ok(self.size)
    }

    fn touch_require(&self, _call: NemuTouchCall) -> DeviceResult<()> {
        Ok(())
    }

    fn touch_resume(&mut self) -> DeviceResult<()> {
        Ok(())
    }

    fn touch_native(&mut self, call: NemuTouchCall) -> DeviceResult<i32> {
        self.touched_displays.push(self.display);
        Ok(match call {
            NemuTouchCall::Down(x, y) => {
                self.calls.push(Call::Down(x, y));
                0
            }
            NemuTouchCall::Up => {
                self.calls.push(Call::Up);
                self.up_code
            }
        })
    }

    fn touch_snapshot(&mut self) -> DeviceResult<()> {
        Ok(())
    }

    fn touch_wait(
        &mut self,
        duration: Duration,
        _check: &dyn InputOperationCheck,
        _stopped: &AtomicBool,
    ) -> DeviceResult<()> {
        self.calls.push(Call::Wait(
            u64::try_from(duration.as_millis()).expect("a wait in milliseconds"),
        ));
        Ok(())
    }
}

/// The context of a step's input: its frame's geometry, if any, and its check.
pub(super) fn context(geometry: Option<Arc<NemuFrameGeometry>>) -> InputExecutionContext {
    InputExecutionContext {
        geometry,
        check: Arc::new(Remaining(Duration::from_millis(REMAINING_MS))),
    }
}

pub(super) fn running() -> AtomicBool {
    AtomicBool::new(false)
}

// §4.4: a contact pressed, or lifted without a confirmed answer, is lifted once by the reset,
// which needs no frame; the contact is then released and the next gesture is admitted. A lift the
// device does not confirm fails the reset as unconfirmed and leaves the contact so.
#[test]
fn hs4_nemu_reset_releases_unconfirmed_contact() {
    for contact in [Contact::MayBeDown, Contact::UpAttempted] {
        let mut device = FakeDevice::new();
        device.set_contact(contact);
        if let Err(error) = reset_contact(&mut device, &context(None), &running()) {
            panic!("{contact:?}: the reset lifts the contact: {error:?}");
        }
        assert_eq!(device.calls, [Call::Up], "{contact:?}: one lift");
        assert_eq!(device.input().contact, Contact::Released, "{contact:?}");

        let frame = device.frame();
        if let Err(error) = perform_events(
            &mut device,
            &long_tap_events((10, 10), 100),
            &context(Some(frame)),
            &running(),
        ) {
            panic!("{contact:?}: a gesture after the reset is admitted: {error:?}");
        }
        assert_eq!(
            device.calls,
            [Call::Up, Call::Down(10, 10), Call::Wait(100), Call::Up],
            "{contact:?}"
        );
    }

    let mut device = FakeDevice::new();
    device.set_contact(Contact::MayBeDown);
    device.up_code = 7;
    let error = reset_contact(&mut device, &context(None), &running())
        .expect_err("a lift the device does not confirm fails the reset");
    assert!(
        error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed),
        "{error:?}"
    );
    assert_eq!(device.calls, [Call::Up]);
    assert_eq!(device.input().contact, Contact::UpAttempted);
}

// §4.4: the reset of a contact never pressed, or one whose lift was confirmed (also after a
// gesture), calls nothing and leaves the contact as it is.
#[test]
fn hs4_nemu_reset_noop_when_released() {
    for contact in [Contact::NotStarted, Contact::Released] {
        let mut device = FakeDevice::new();
        device.set_contact(contact);
        if let Err(error) = reset_contact(&mut device, &context(None), &running()) {
            panic!("{contact:?}: the reset succeeds: {error:?}");
        }
        assert!(device.calls.is_empty(), "{contact:?}: {:?}", device.calls);
        assert_eq!(device.input().contact, contact);
    }

    let mut device = FakeDevice::new();
    let frame = device.frame();
    perform_events(
        &mut device,
        &long_tap_events((5, 5), 50),
        &context(Some(frame)),
        &running(),
    )
    .expect("a long tap");
    let pressed = device.calls.clone();
    reset_contact(&mut device, &context(None), &running()).expect("the reset after it");
    assert_eq!(device.calls, pressed, "the reset called nothing");
    assert_eq!(device.input().contact, Contact::Released);
}

// §4.2 items 4 and 5, §4.4: the long tap presses its point, holds for its duration and lifts, on
// its step's frame. Without a frame, on a frame of another size or an earlier display generation,
// at a point outside the frame, or with a hold the deadline cannot cover together with the final
// lift's reserve, it is refused before anything is pressed.
#[test]
fn hs4_nemu_long_tap_in_frame() {
    let mut device = FakeDevice::new();
    let frame = device.frame();
    perform_events(
        &mut device,
        &long_tap_events((120, 80), 800),
        &context(Some(frame)),
        &running(),
    )
    .expect("the long tap");
    assert_eq!(
        device.calls,
        [Call::Down(120, 80), Call::Wait(800), Call::Up]
    );
    assert_eq!(device.input().contact, Contact::Released);

    let mut device = FakeDevice::new();
    perform_events(
        &mut device,
        &long_tap_events((120, 80), 800),
        &context(None),
        &running(),
    )
    .expect_err("a long tap needs its step's frame");
    assert!(device.calls.is_empty(), "{:?}", device.calls);

    let mut device = FakeDevice::new();
    let frame = device.frame();
    device.size = (WIDTH, HEIGHT + 1);
    perform_events(
        &mut device,
        &long_tap_events((120, 80), 800),
        &context(Some(frame)),
        &running(),
    )
    .expect_err("a frame of another size is refused");
    assert!(device.calls.is_empty(), "{:?}", device.calls);

    let mut device = FakeDevice::new();
    let frame = device.frame();
    device
        .input
        .as_mut()
        .expect("the paired input")
        .invalidate()
        .expect("a new display generation");
    perform_events(
        &mut device,
        &long_tap_events((120, 80), 800),
        &context(Some(frame)),
        &running(),
    )
    .expect_err("a frame of an earlier display generation is refused");
    assert!(device.calls.is_empty(), "{:?}", device.calls);

    let mut device = FakeDevice::new();
    let frame = device.frame();
    perform_events(
        &mut device,
        &long_tap_events((120, HEIGHT as i32), 800),
        &context(Some(frame)),
        &running(),
    )
    .expect_err("a point outside the frame is refused");
    assert!(device.calls.is_empty(), "{:?}", device.calls);

    let uncovered = REMAINING_MS - SHUTDOWN_TIMEOUT_MS;
    let mut device = FakeDevice::new();
    let frame = device.frame();
    perform_events(
        &mut device,
        &long_tap_events((120, 80), uncovered),
        &context(Some(frame)),
        &running(),
    )
    .expect_err("a hold the deadline cannot cover with the final lift is refused");
    assert!(device.calls.is_empty(), "{:?}", device.calls);

    let mut device = FakeDevice::new();
    let frame = device.frame();
    perform_events(
        &mut device,
        &long_tap_events((120, 80), uncovered - 1),
        &context(Some(frame)),
        &running(),
    )
    .expect("a hold one millisecond shorter fits");
    assert_eq!(
        device.calls,
        [Call::Down(120, 80), Call::Wait(uncovered - 1), Call::Up]
    );
}

// §4.2 items 4 and 5, §4.4: the swipe presses its start, moves along the straight line to its
// end in evenly timed moves (one per 16 ms; each a wait, then the finger at its point) that take
// its whole duration, and lifts once, on its step's frame. Without a frame, or with an end
// outside the frame, nothing is pressed.
#[test]
fn hs4_nemu_swipe_in_frame() {
    let mut device = FakeDevice::new();
    let frame = device.frame();
    perform_events(
        &mut device,
        &crate::input::linear_swipe_events((40, 200), (340, 120), 300),
        &context(Some(frame)),
        &running(),
    )
    .expect("the swipe");
    let calls = &device.calls;
    assert_eq!(calls.first(), Some(&Call::Down(40, 200)));
    assert_eq!(calls.last(), Some(&Call::Up));
    let mut waits = Vec::new();
    let mut points = Vec::new();
    for (index, call) in calls[1..calls.len() - 1].iter().enumerate() {
        match (index % 2, call) {
            (0, Call::Wait(ms)) => waits.push(*ms),
            (1, Call::Down(x, y)) => points.push((*x, *y)),
            other => panic!("a move is a wait, then the finger at its point: {other:?}"),
        }
    }
    // 300 ms in moves of 16 ms: 19 moves.
    assert_eq!(waits.len(), 19, "{calls:?}");
    assert_eq!(points.len(), 19, "{calls:?}");
    assert_eq!(waits.iter().sum::<u64>(), 300);
    let shortest = waits.iter().min().copied().expect("a wait");
    let longest = waits.iter().max().copied().expect("a wait");
    assert!(longest - shortest <= 1, "evenly timed: {waits:?}");
    assert_eq!(points.last(), Some(&(340, 120)));
    assert!(
        points
            .windows(2)
            .all(|pair| pair[0].0 < pair[1].0 && pair[0].1 >= pair[1].1),
        "along the line: {points:?}"
    );
    assert_eq!(device.input().contact, Contact::Released);

    let mut device = FakeDevice::new();
    perform_events(
        &mut device,
        &crate::input::linear_swipe_events((40, 200), (340, 120), 300),
        &context(None),
        &running(),
    )
    .expect_err("a swipe needs its step's frame");
    assert!(device.calls.is_empty(), "{:?}", device.calls);

    let mut device = FakeDevice::new();
    let frame = device.frame();
    perform_events(
        &mut device,
        &crate::input::linear_swipe_events((40, 200), (WIDTH as i32, 120), 300),
        &context(Some(frame)),
        &running(),
    )
    .expect_err("an end outside the frame is refused");
    assert!(device.calls.is_empty(), "{:?}", device.calls);
}
