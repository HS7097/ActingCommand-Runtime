// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-3b (`contracts/display-adapter.md` section 10, "The vertical brake"; the
//! coordinator's work order of 11-13 on board #392, route E): a vertical-brake drag under a pure
//! scale leaves as a drag with a pause and a brake, its brake scaled; the interface's bounds do
//! not move. Every expected value is worked out by hand from the rounding table (a point and a
//! length `round_half_up(v · s)`), never by the code under test. The sizes are neutral examples.

use super::*;

fn extent(width: u32, height: u32) -> CaptureExtent {
    CaptureExtent::new(width, height).expect("a test extent has two positive sides")
}

/// The map of a `frame`-sized frame that is its own logical display, at rotation 0, uncropped,
/// with no insets, for a package of `base` size without a declaration.
fn map(base: (u32, u32), frame: (u32, u32)) -> DisplayMap {
    let observation = DisplayObservation {
        frame: extent(frame.0, frame.1),
        display: extent(frame.0, frame.1),
        rotation: Some(CaptureRotation::R0),
        crop: None,
        insets: None,
    };
    DisplayMap::build(extent(base.0, base.1), observation, None)
        .unwrap_or_else(|error| panic!("{base:?} on {frame:?}: {error:?}"))
}

/// The vertical-brake drag the kernel makes from the press (`x1`, `y1`) to the corner (`x2`,
/// `y2`), its brake of 100 straight above the corner, with the interface's fixed times and slopes.
fn vertical(x1: i32, y1: i32, x2: i32, y2: i32) -> InputAction {
    InputAction::SingleTouchDragWithVerticalBrakeV1 {
        x1,
        y1,
        x2,
        y2,
        x3: x2,
        y3: y2 - 100,
        horizontal_duration_ms: 200,
        corner_hold_ms: 150,
        brake_distance_px: 100,
        brake_duration_ms: 200,
        slope_in: 2,
        slope_out: 0,
    }
}

/// Work order test 2 (route E): a 1280x720 package's vertical-brake drag from (201, 601), drawn in
/// (180, 580, 40, 40), to the corner (901, 501), drawn in (880, 480, 40, 40), on a 1920x1080, a
/// 2560x1440, a 3200x1800 and a 3840x2160 device (s = 3/2, 2, 5/2 and 3) is admitted and leaves as
/// a drag with a pause and a brake. Its press and its corner are at `round_half_up(v · s)` (at 3/2
/// 301.5, 901.5, 1351.5 and 751.5 go up to 302, 902, 1352 and 752; at 5/2 502.5, 1502.5, 2252.5
/// and 1252.5 to 503, 1503, 2253 and 1253), its brake `round_half_up(100 · s)` (150, 200, 250,
/// 300) straight above the corner, and its three times are the drag's: drag 200 ms, pause 150 ms,
/// brake 200 ms. The interface accepts the drag sent as it is (`InputAction::validate`), and a
/// backend that takes frame coordinates receives the same points (no crop, no turn). On a
/// 1280x720 device, the identity, the drag leaves as it was made, its kind kept.
#[test]
fn da3b_a_vertical_brake_leaves_a_scaled_device_as_a_drag_with_a_pause_and_a_brake() {
    let drag = vertical(201, 601, 901, 501);
    let boxes = [Rect::new(180, 580, 40, 40), Rect::new(880, 480, 40, 40)];
    for (frame, (x1, y1), (x2, y2), brake) in [
        ((1920, 1080), (302, 902), (1352, 752), 150),
        ((2560, 1440), (402, 1202), (1802, 1002), 200),
        ((3200, 1800), (503, 1503), (2253, 1253), 250),
        ((3840, 2160), (603, 1803), (2703, 1503), 300),
    ] {
        let map = map((1280, 720), frame);
        assert_eq!(map.kind(), DisplayMapKind::Scale, "{frame:?}");
        assert_eq!(map.admit_input(&drag), Ok(()), "{frame:?}");
        let expected = InputAction::SingleTouchDragWithPauseAndBrakeV1 {
            x1,
            y1,
            x2,
            y2,
            drag_duration_ms: 200,
            pause_ms: 150,
            x3: x2,
            y3: y2 - brake,
            brake_duration_ms: 200,
        };
        assert_eq!(
            map.device_input(&drag, &boxes, InputCoordinates::Display),
            Ok(expected.clone()),
            "{frame:?}"
        );
        assert_eq!(
            map.device_input(&drag, &boxes, InputCoordinates::DeliveredFrame),
            Ok(expected.clone()),
            "{frame:?}"
        );
        assert!(
            expected.validate().is_ok(),
            "{frame:?}: the interface accepts the drag sent"
        );
    }
    let identity = map((1280, 720), (1280, 720));
    assert!(identity.is_plain_identity());
    assert_eq!(identity.admit_input(&drag), Ok(()));
    assert_eq!(
        identity.device_input(&drag, &boxes, InputCoordinates::Display),
        Ok(drag.clone())
    );
}

/// Work order test 2, the refusal: a brake whose end would leave the device frame is refused out of
/// bounds (the host's `contained_task_input_out_of_bounds`), never clamped. The kernel makes no
/// such drag (its corner lies at least the brake below the top, and the rounding keeps that
/// order), so the drag here is one the interface refuses: its corner at (900, 99), 99 base pixels
/// below the top. The corner goes to y = 149 (148.5 up), 198, 248 (247.5 up) and 297 at 3/2, 2,
/// 5/2 and 3, the brake to 150, 200, 250 and 300: the end would lie at y = -1, -2, -2 and -3, and
/// the drag is refused rather than sent with its end moved to the frame's edge.
#[test]
fn da3b_a_brake_end_outside_the_device_frame_is_refused_not_clamped() {
    let drag = InputAction::SingleTouchDragWithVerticalBrakeV1 {
        x1: 200,
        y1: 600,
        x2: 900,
        y2: 99,
        x3: 900,
        y3: -1,
        horizontal_duration_ms: 200,
        corner_hold_ms: 150,
        brake_distance_px: 100,
        brake_duration_ms: 200,
        slope_in: 2,
        slope_out: 0,
    };
    for frame in [(1920, 1080), (2560, 1440), (3200, 1800), (3840, 2160)] {
        let map = map((1280, 720), frame);
        assert_eq!(map.admit_input(&drag), Ok(()), "{frame:?}");
        for coordinates in [InputCoordinates::Display, InputCoordinates::DeliveredFrame] {
            assert_eq!(
                map.device_input(&drag, &[], coordinates),
                Err(InputMapError::OutOfBounds),
                "{frame:?} {coordinates:?}"
            );
        }
    }
}
