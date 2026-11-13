// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-3 (model 392 v1.2 §13, rows DA3-T1, T2, T5 to T8, T10 to T12, T14 and T15, as
//! the coordinator's work order narrows the slice; `contracts/display-adapter.md` section 10): the
//! input side's pure functions. Every expected value is worked out by hand from the rounding table
//! (a point `round_half_up(v · s)`, an input box inward), never by the code under test. The sizes
//! are neutral examples.

use super::*;
use actingcommand_contract::outcome::{Category, LadderClass};

const BY_HEIGHT: DisplayRule = DisplayRule {
    scale_by: DisplayScaleBy::Height,
    scale_area: DisplayArea::Screen,
    anchor_area: DisplayArea::Screen,
};

fn extent(width: u32, height: u32) -> CaptureExtent {
    CaptureExtent::new(width, height).expect("a test extent has two positive sides")
}

/// A frame that is its own logical display, at rotation 0, uncropped, with no insets read.
fn plain(width: u32, height: u32) -> DisplayObservation {
    DisplayObservation {
        frame: extent(width, height),
        display: extent(width, height),
        rotation: Some(CaptureRotation::R0),
        crop: None,
        insets: None,
    }
}

fn map(base: (u32, u32), observation: DisplayObservation, rule: Option<DisplayRule>) -> DisplayMap {
    DisplayMap::build(extent(base.0, base.1), observation, rule)
        .unwrap_or_else(|error| panic!("{base:?} on {observation:?} under {rule:?}: {error:?}"))
}

fn rect(x: i64, y: i64, width: i64, height: i64) -> Rect {
    Rect::new(x, y, width, height)
}

fn tap(x: i32, y: i32) -> InputAction {
    InputAction::Tap { x, y }
}

/// `action` on the device under `map`, for a backend that takes display coordinates.
fn sent(
    map: &DisplayMap,
    action: &InputAction,
    boxes: &[Rect],
) -> Result<InputAction, InputMapError> {
    map.device_input(action, boxes, InputCoordinates::Display)
}

/// The `display_map_unresolved` refusal of an input, or a panic naming what came instead.
fn unresolved(result: Result<InputAction, InputMapError>) -> DisplayMapUnresolved {
    match result {
        Err(InputMapError::Unresolved(unresolved)) => unresolved,
        other => panic!("expected display_map_unresolved, got {other:?}"),
    }
}

/// A frame cropped 2 off the top of its 1080x2340 display (DA1-T15's stream).
fn cropped() -> DisplayObservation {
    DisplayObservation {
        frame: extent(1080, 2336),
        display: extent(1080, 2340),
        rotation: Some(CaptureRotation::R0),
        crop: Some(CaptureFrameCrop {
            x: 0,
            y: 2,
            width: 1080,
            height: 2336,
        }),
        insets: None,
    }
}

/// `round_half_up(v · num / den)` for `v ≥ 0`, by hand: `⌊(2 · v · num + den) / (2 · den)⌋`.
fn nearest(value: i64, num: i64, den: i64) -> i64 {
    (2 * value * num + den) / (2 * den)
}

/// `⌈v · num / den⌉` for `v ≥ 0`, by hand.
fn up(value: i64, num: i64, den: i64) -> i64 {
    (value * num + den - 1) / den
}

/// `⌊v · num / den⌋` for `v ≥ 0`, by hand.
fn down(value: i64, num: i64, den: i64) -> i64 {
    value * num / den
}

/// Every box of a `width` x `height` base, each with every point in it.
fn every_box(width: i64, height: i64) -> Vec<(Rect, Vec<(i64, i64)>)> {
    let mut boxes = Vec::new();
    for left in 0..width {
        for right in left + 1..=width {
            for top in 0..height {
                for bottom in top + 1..=height {
                    let points = (left..right)
                        .flat_map(|x| (top..bottom).map(move |y| (x, y)))
                        .collect();
                    boxes.push((rect(left, top, right - left, bottom - top), points));
                }
            }
        }
    }
    boxes
}

/// A base or device coordinate of a small test frame as an input's `i32`.
fn coordinate(value: i64) -> i32 {
    i32::try_from(value).expect("a small test coordinate")
}

/// A 720x1280 frame of a 1280x720 display at 90°, for a 1280x720 package: turned upright.
fn turned() -> DisplayObservation {
    DisplayObservation {
        frame: extent(720, 1280),
        display: extent(1280, 720),
        rotation: Some(CaptureRotation::R90),
        crop: None,
        insets: None,
    }
}

/// Work order point 4 (model 392 v1.2 §4.6): a point goes to the nearest device pixel and lands
/// in the device frame and in the device box its base box maps to inward, at the frame's last
/// column and row and in a box one base pixel wide, at the scales 3/2, 2 and 3; where it would
/// not (a scale of 4/3), it is refused, never clamped.
#[test]
fn da3_points_land_in_the_device_frame_and_in_their_device_box_at_the_edges() {
    // s = 3/2: 1279 · 3/2 = 1918.5 → 1919, 719 · 3/2 = 1078.5 → 1079; the box (1279, 719, 1, 1)
    // spans 1918.5 .. 1920 by 1078.5 .. 1080, inward (1919, 1079, 1, 1).
    // s = 2: (2558, 1438) in (2558, 1438, 2, 2); s = 3: (3837, 2157) in (3837, 2157, 3, 3).
    for (frame, last, last_box) in [
        ((1920, 1080), (1919, 1079), rect(1919, 1079, 1, 1)),
        ((2560, 1440), (2558, 1438), rect(2558, 1438, 2, 2)),
        ((3840, 2160), (3837, 2157), rect(3837, 2157, 3, 3)),
    ] {
        let map = map((1280, 720), plain(frame.0, frame.1), None);
        assert_eq!(map.kind(), DisplayMapKind::Scale, "{frame:?}");
        assert_eq!(map.admit_input(&tap(1279, 719)), Ok(()), "{frame:?}");
        assert_eq!(
            map.map_input_box(rect(1279, 719, 1, 1), None),
            Ok(last_box),
            "{frame:?}"
        );
        assert_eq!(
            sent(&map, &tap(1279, 719), &[rect(1279, 719, 1, 1)]),
            Ok(tap(last.0, last.1)),
            "{frame:?}"
        );
        assert_eq!(
            sent(&map, &tap(0, 0), &[rect(0, 0, 1, 1)]),
            Ok(tap(0, 0)),
            "{frame:?}"
        );
    }
    // s = 3/2: (1, 1) → round_half_up(1.5) = 2, inside the box (1, 1, 1, 1) → (2, 2, 1, 1).
    let half = map((1280, 720), plain(1920, 1080), None);
    assert_eq!(
        half.map_input_box(rect(1, 1, 1, 1), None),
        Ok(rect(2, 2, 1, 1))
    );
    assert_eq!(sent(&half, &tap(1, 1), &[rect(1, 1, 1, 1)]), Ok(tap(2, 2)));
    // A recognised element's box (DA3-T2): the point drawn in it, (640, 360) in (600, 340, 80, 40),
    // lands at (960, 540) in its device box (900, 510, 120, 60).
    assert_eq!(
        sent(&half, &tap(640, 360), &[rect(600, 340, 80, 40)]),
        Ok(tap(960, 540))
    );
    // s = 4/3 (1280x720 for 960x540): (1, 1) → round_half_up(4/3) = 1, but the box (1, 1, 2, 2)
    // spans 4/3 .. 4 and keeps 2 .. 4 inward: the point is outside its device box.
    let thirds = map((960, 540), plain(1280, 720), None);
    assert_eq!(
        sent(&thirds, &tap(1, 1), &[rect(1, 1, 2, 2)]),
        Err(InputMapError::OutOfBounds)
    );
    // Without a box the same point is sent where it maps.
    assert_eq!(sent(&thirds, &tap(1, 1), &[]), Ok(tap(1, 1)));
    // A box one base pixel wide spans 4/3 .. 8/3: no whole device pixel, `collapsed`.
    let collapsed = unresolved(sent(&thirds, &tap(1, 1), &[rect(1, 1, 1, 1)]));
    assert_eq!(collapsed.problem, DisplayProblem::Collapsed);
    assert_eq!(collapsed.stage, DisplayLocation::InputMap);
    // A point outside the base is outside the scale frame, refused, never clamped.
    let outside = unresolved(sent(&half, &tap(1280, 0), &[]));
    assert_eq!(outside.problem, DisplayProblem::OutsideFrame);
    assert_eq!(outside.stage, DisplayLocation::InputMap);
}

/// Fix 1, work order point 3 (`contracts/display-adapter.md` section 10): the input side sends
/// under a pure scale `s = num / den` (reduced) only when `2 · s` is a whole number, `den` 1 or
/// 2, because exactly then every base point of every base box lands inside that box's inward
/// device box.
///
/// Why (a box spans `a ≤ v < b`, `s ≥ 1`): the point goes to `⌊v · s + ½⌋`, the box to
/// `⌈a · s⌉ .. ⌊b · s⌋`. The low edge holds for every `v ≥ a` exactly when `⌊a · s + ½⌋ ≥
/// ⌈a · s⌉`, that is when the fraction of `a · s` is 0 or at least ½; as `a` runs over the base
/// (`den` divides its sides), `a · num` takes every residue modulo `den`, so the fraction takes
/// every `k / den`, and none lies strictly between 0 and ½ exactly when `den ≤ 2`. The high edge,
/// `⌊(b − 1) · s + ½⌋ < ⌊b · s⌋`, holds for every `b` once `s ≥ 3/2`, which every such scale
/// above 1 is; the frame's bound is the whole base box's.
///
/// Over an 8x4 base, for the scales 1, 3/2, 2, 5/2, 3, 7/2 and 4, every point of every box is
/// admitted and sent at `⌊(2 · v · num + den) / (2 · den)⌋`, which lies in `⌈x · s⌉ ..
/// ⌊(x + w) · s⌋` (both worked out here, not by the code under test). For 4/3, 5/4, 8/5 and 9/4
/// the same enumeration finds a point its box cannot keep. At 5/4 the input side refuses every
/// point of the base up front, `input_scale_unsupported` at `display_input_map` with the frame's
/// and the base's sizes, whatever the point.
#[test]
fn da3_fix1_the_input_side_sends_under_a_scale_only_when_its_double_is_whole() {
    for (num, den) in [
        (1_u32, 1_u32),
        (3, 2),
        (2, 1),
        (5, 2),
        (3, 1),
        (7, 2),
        (4, 1),
    ] {
        let map = map((8, 4), plain(8 * num / den, 4 * num / den), None);
        assert!(map.scale().doubled_is_whole(), "{num}/{den}");
        let (n, d) = (i64::from(num), i64::from(den));
        for (input_box, points) in every_box(8, 4) {
            let (left, top) = (up(input_box.x, n, d), up(input_box.y, n, d));
            let (right, bottom) = (
                down(input_box.right(), n, d),
                down(input_box.bottom(), n, d),
            );
            for (x, y) in points {
                let base = tap(coordinate(x), coordinate(y));
                let (device_x, device_y) = (nearest(x, n, d), nearest(y, n, d));
                assert!(
                    (left..right).contains(&device_x) && (top..bottom).contains(&device_y),
                    "{num}/{den}: ({x}, {y}) of {input_box:?} by hand"
                );
                assert_eq!(map.admit_input(&base), Ok(()), "{num}/{den}");
                assert_eq!(
                    sent(&map, &base, &[input_box]),
                    Ok(tap(coordinate(device_x), coordinate(device_y))),
                    "{num}/{den}: ({x}, {y}) of {input_box:?}"
                );
            }
        }
    }
    for (base, num, den) in [
        ((6_u32, 3_u32), 4_u32, 3_u32),
        ((8, 4), 5, 4),
        ((10, 5), 8, 5),
        ((8, 4), 9, 4),
    ] {
        let map = map(base, plain(base.0 * num / den, base.1 * num / den), None);
        assert_eq!(map.kind(), DisplayMapKind::Scale, "{num}/{den}");
        assert!(!map.scale().doubled_is_whole(), "{num}/{den}");
        let kept = every_box(i64::from(base.0), i64::from(base.1))
            .into_iter()
            .all(|(input_box, points)| {
                points.into_iter().all(|(x, y)| {
                    sent(&map, &tap(coordinate(x), coordinate(y)), &[input_box]).is_ok()
                })
            });
        assert!(!kept, "{num}/{den}: some point lies outside its device box");
    }
    let quarters = map((8, 4), plain(10, 5), None);
    for x in 0..8 {
        for y in 0..4 {
            let refused = quarters
                .admit_input(&tap(x, y))
                .expect_err("a scale of 5/4 is refused up front");
            assert_eq!(refused.problem, DisplayProblem::InputScaleUnsupported);
            assert_eq!(refused.stage, DisplayLocation::InputMap);
            assert_eq!((refused.frame, refused.base), (extent(10, 5), extent(8, 4)));
        }
    }
}

/// Work order point 5 at the pure level: under the identity every kind of input, with or without
/// its boxes, leaves exactly as the kernel made it.
#[test]
fn da3_identity_sends_every_input_as_it_was_made() {
    let identity = map((1280, 720), plain(1280, 720), None);
    assert!(identity.is_plain_identity());
    let rows: [(InputAction, Vec<Rect>); 9] = [
        (tap(1279, 719), vec![rect(1279, 719, 1, 1)]),
        (tap(1, 0), vec![]),
        (
            InputAction::LongTap {
                x: 640,
                y: 360,
                duration_ms: 800,
            },
            vec![rect(620, 340, 40, 40)],
        ),
        (
            InputAction::Swipe {
                x1: 10,
                y1: 20,
                x2: 1200,
                y2: 700,
                duration_ms: 300,
            },
            vec![rect(0, 0, 40, 40), rect(1180, 680, 40, 40)],
        ),
        (
            InputAction::SingleTouchDragWithVerticalBrakeV1 {
                x1: 200,
                y1: 600,
                x2: 900,
                y2: 500,
                x3: 900,
                y3: 400,
                horizontal_duration_ms: 200,
                corner_hold_ms: 150,
                brake_distance_px: 100,
                brake_duration_ms: 200,
                slope_in: 2,
                slope_out: 0,
            },
            vec![rect(180, 580, 40, 40), rect(880, 480, 40, 40)],
        ),
        (
            InputAction::SingleTouchDragWithPauseAndBrakeV1 {
                x1: 100,
                y1: 600,
                x2: 100,
                y2: 200,
                drag_duration_ms: 300,
                pause_ms: 120,
                x3: 120,
                y3: 200,
                brake_duration_ms: 80,
            },
            vec![rect(90, 590, 20, 20), rect(90, 190, 20, 20)],
        ),
        (
            InputAction::SingleTouchPathDragV1 {
                box_x: 2,
                box_y: 2,
                box_width: 12,
                box_height: 6,
                points: vec![
                    InputPathPoint { x: 3, y: 3 },
                    InputPathPoint { x: 13, y: 7 },
                ],
                segment_duration_ms: vec![600],
                corner_hold_ms: 40,
            },
            vec![],
        ),
        (
            InputAction::Key {
                key: "neutral".to_owned(),
            },
            vec![],
        ),
        (InputAction::Reset, vec![]),
    ];
    for (action, boxes) in rows {
        assert_eq!(identity.admit_input(&action), Ok(()));
        assert_eq!(sent(&identity, &action, &boxes), Ok(action.clone()));
        assert_eq!(
            identity.device_input(&action, &boxes, InputCoordinates::DeliveredFrame),
            Ok(action)
        );
    }
}

/// DA3-T1 (model 392 v1.2 §6.1, at the pure level; the host refuses a translation until DA-2b):
/// on 2340x1080 by height a centred box (590, 600, 100, 40) moves by half the room, 140, into
/// (730, 600, 100, 40) of the 1560x720 scale space, which is (1095, 900, 150, 60) inward on the
/// device; the point (600, 610) drawn in it lands at (740, 610) · 3/2 = (1110, 915), inside.
#[test]
fn da3_t1_a_box_under_a_translation_lands_by_the_hand_table() {
    let map = map((1280, 720), plain(2340, 1080), Some(BY_HEIGHT));
    assert_eq!(map.kind(), DisplayMapKind::Translate);
    assert_eq!(
        map.map_input_box(rect(590, 600, 100, 40), None),
        Ok(rect(1095, 900, 150, 60))
    );
    assert_eq!(
        sent(&map, &tap(600, 610), &[rect(590, 600, 100, 40)]),
        Ok(tap(1110, 915))
    );
}

/// DA3-T5 (model 392 v1.2 §6.4): at s = 3/2 the vertical brake of 100 base pixels is 150 on the
/// device, and the brake's end stays straight above the corner: (200, 600) → (300, 900), the
/// corner (900, 500) → (1350, 750), the end (1350, 600). The host holds that drag back under a
/// scale (`input_scale_unsupported`): the contract fixes its brake at 100.
#[test]
fn da3_t5_a_vertical_brake_scales_with_the_map_and_waits_for_its_bound() {
    let map = map((1280, 720), plain(1920, 1080), None);
    let drag = |x1, y1, x2, y2, x3, y3, brake| InputAction::SingleTouchDragWithVerticalBrakeV1 {
        x1,
        y1,
        x2,
        y2,
        x3,
        y3,
        horizontal_duration_ms: 200,
        corner_hold_ms: 150,
        brake_distance_px: brake,
        brake_duration_ms: 200,
        slope_in: 2,
        slope_out: 0,
    };
    let boxes = [rect(180, 580, 40, 40), rect(880, 480, 40, 40)];
    assert_eq!(
        sent(&map, &drag(200, 600, 900, 500, 900, 400, 100), &boxes),
        Ok(drag(300, 900, 1350, 750, 1350, 600, 150))
    );
    let held = map
        .admit_input(&drag(200, 600, 900, 500, 900, 400, 100))
        .expect_err("a scaled brake waits for its bound");
    assert_eq!(held.problem, DisplayProblem::InputScaleUnsupported);
    assert_eq!(held.stage, DisplayLocation::InputMap);
    assert_eq!(map.admit_input(&tap(1, 1)), Ok(()));
}

/// DA3-T6 and DA3-T15 (model 392 v1.2 §6.6): a map built for a 1920x1080 frame is current for an
/// input decided on a frame of that size and stale for one decided on a 1280x720 frame:
/// `display_map_stale`, a warning of the ladder's stability class, at `display_input_map`.
#[test]
fn da3_t6_a_map_of_another_frame_size_is_stale() {
    let map = map((1280, 720), plain(1920, 1080), None);
    assert_eq!(map.current_for(extent(1920, 1080)), Ok(()));
    let stale = map
        .current_for(extent(1280, 720))
        .expect_err("the geometry changed after the frame");
    assert_eq!(
        stale,
        DisplayMapStale {
            decided: extent(1280, 720),
            current: extent(1920, 1080),
        }
    );
    let outcome = stale.outcome();
    assert!(outcome.is(DisplayCode::MapStale));
    assert_eq!(outcome.code().as_str(), "display_map_stale");
    assert_eq!(outcome.category(), Category::Warning);
    assert_eq!(
        DisplayCode::MapStale.ladder_class(),
        LadderClass::StabilityWarning
    );
    assert_eq!(
        outcome.value("stage").and_then(|value| value.as_str()),
        Some("display_input_map")
    );
}

/// DA3-T7 (model 392 v1.2 §6.7, at the pure level; the host refuses a turned frame until DA-2b):
/// a 720x1280 frame of a 1280x720 display at 90° is turned clockwise; the tap at (100, 200) is
/// (100, 200) in display coordinates and (200, 1179) with the turn undone for a backend that takes
/// frame coordinates; a vertical brake cannot be kept across a turn for such a backend.
#[test]
fn da3_t7_a_turned_frame_undoes_its_turn_for_frame_coordinates() {
    let map = map((1280, 720), turned(), None);
    assert_eq!(map.turn(), Some(QuarterTurn::Clockwise));
    assert_eq!(sent(&map, &tap(100, 200), &[]), Ok(tap(100, 200)));
    assert_eq!(
        map.device_input(&tap(100, 200), &[], InputCoordinates::DeliveredFrame),
        Ok(tap(200, 1179))
    );
    let brake = InputAction::SingleTouchDragWithVerticalBrakeV1 {
        x1: 200,
        y1: 600,
        x2: 900,
        y2: 500,
        x3: 900,
        y3: 400,
        horizontal_duration_ms: 200,
        corner_hold_ms: 150,
        brake_distance_px: 100,
        brake_duration_ms: 200,
        slope_in: 2,
        slope_out: 0,
    };
    let refused = unresolved(map.device_input(&brake, &[], InputCoordinates::DeliveredFrame));
    assert_eq!(refused.problem, DisplayProblem::OrientationMismatch);
}

/// DA3-T10, DA3-T11 and a swipe (model 392 v1.2 §6.1, §6.2): at s = 2 a long press's point and
/// its box double and its hold is kept; a swipe's two points double, its duration kept. At s = 3/2
/// a drag with a pause and a brake maps its two points, its brake's cross of 20 becomes 30 (and
/// one of -1 becomes -2, the size to the nearest with its sign), and its three durations are kept.
#[test]
fn da3_t10_t11_long_presses_swipes_and_braked_drags_keep_their_durations() {
    let double = map((1280, 720), plain(2560, 1440), None);
    assert_eq!(
        sent(
            &double,
            &InputAction::LongTap {
                x: 640,
                y: 360,
                duration_ms: 800
            },
            &[rect(620, 340, 40, 40)]
        ),
        Ok(InputAction::LongTap {
            x: 1280,
            y: 720,
            duration_ms: 800
        })
    );
    assert_eq!(
        sent(
            &double,
            &InputAction::Swipe {
                x1: 1,
                y1: 2,
                x2: 10,
                y2: 5,
                duration_ms: 300
            },
            &[rect(0, 0, 4, 4), rect(8, 4, 4, 4)]
        ),
        Ok(InputAction::Swipe {
            x1: 2,
            y1: 4,
            x2: 20,
            y2: 10,
            duration_ms: 300
        })
    );
    let half = map((1280, 720), plain(1920, 1080), None);
    let braked = |x3, y3| InputAction::SingleTouchDragWithPauseAndBrakeV1 {
        x1: 100,
        y1: 600,
        x2: 100,
        y2: 200,
        drag_duration_ms: 300,
        pause_ms: 120,
        x3,
        y3,
        brake_duration_ms: 80,
    };
    let device = |x3, y3| InputAction::SingleTouchDragWithPauseAndBrakeV1 {
        x1: 150,
        y1: 900,
        x2: 150,
        y2: 300,
        drag_duration_ms: 300,
        pause_ms: 120,
        x3,
        y3,
        brake_duration_ms: 80,
    };
    let boxes = [rect(90, 590, 20, 20), rect(90, 190, 20, 20)];
    assert_eq!(sent(&half, &braked(120, 200), &boxes), Ok(device(180, 300)));
    assert_eq!(sent(&half, &braked(99, 200), &boxes), Ok(device(148, 300)));
}

/// DA3-T12 (model 392 v1.2 §6.2): at s = 3 a path drag's box (2, 2, 12, 6) is (6, 6, 36, 18) on
/// the device and holds every point, each three times its base point; the durations and the
/// corner hold are kept, and the same drag maps the same way twice.
#[test]
fn da3_t12_a_path_drag_maps_its_box_and_every_point() {
    let triple = map((16, 9), plain(48, 27), None);
    let path = |scale: i32, box_rect: (i32, i32, i32, i32)| InputAction::SingleTouchPathDragV1 {
        box_x: box_rect.0,
        box_y: box_rect.1,
        box_width: box_rect.2,
        box_height: box_rect.3,
        points: [(3, 3), (13, 7), (4, 6), (12, 3)]
            .into_iter()
            .map(|(x, y)| InputPathPoint {
                x: x * scale,
                y: y * scale,
            })
            .collect(),
        segment_duration_ms: vec![200, 200, 200],
        corner_hold_ms: 40,
    };
    let base = path(1, (2, 2, 12, 6));
    let expected = path(3, (6, 6, 36, 18));
    assert_eq!(sent(&triple, &base, &[]), Ok(expected.clone()));
    assert_eq!(sent(&triple, &base, &[]), Ok(expected));
}

/// DA3-T14 (model 392 v1.2 §4.7, §6.7, at the pure level; the host refuses a cropped frame until
/// DA-2b): a frame cropped 2 off the top of its display, for a package of the display's size, is
/// the identity; the tap at (150, 550) is (150, 548) in the frame and (150, 550) again in display
/// coordinates, inside its device box (100, 498, 200, 100).
#[test]
fn da3_t14_a_cropped_frame_adds_its_offset_back_for_display_coordinates() {
    let map = map((1080, 2340), cropped(), None);
    assert_eq!(
        map.map_input_box(rect(100, 500, 200, 100), None),
        Ok(rect(100, 498, 200, 100))
    );
    assert_eq!(
        sent(&map, &tap(150, 550), &[rect(100, 500, 200, 100)]),
        Ok(tap(150, 550))
    );
    assert_eq!(
        map.device_input(
            &tap(150, 550),
            &[rect(100, 500, 200, 100)],
            InputCoordinates::DeliveredFrame
        ),
        Ok(tap(150, 548))
    );
}

/// Work order point 2: the input side sends under the identity and a pure scale only; a
/// translation and a cropped frame are `translation_unsupported`, a turned frame
/// `orientation_mismatch`, each `display_map_unresolved` at `display_input_map` with the frame's and
/// the base's sizes.
#[test]
fn da3_the_input_side_refuses_every_map_the_capture_side_refuses() {
    for (base, observation, rule) in [
        ((1280, 720), plain(1280, 720), None),
        ((1280, 720), plain(1920, 1080), None),
        ((1280, 720), plain(2560, 1440), None),
    ] {
        assert_eq!(
            map(base, observation, rule).admit_input(&tap(1, 0)),
            Ok(()),
            "{observation:?}"
        );
    }
    for (base, observation, rule, problem, frame) in [
        (
            (1280, 720),
            plain(2340, 1080),
            Some(BY_HEIGHT),
            DisplayProblem::TranslationUnsupported,
            (2340, 1080),
        ),
        (
            (1080, 2340),
            cropped(),
            None,
            DisplayProblem::TranslationUnsupported,
            (1080, 2336),
        ),
        (
            (1280, 720),
            turned(),
            None,
            DisplayProblem::OrientationMismatch,
            (720, 1280),
        ),
    ] {
        let refused = map(base, observation, rule)
            .admit_input(&tap(1, 0))
            .expect_err("the input side waits for DA-2b");
        assert_eq!(refused.problem, problem, "{observation:?}");
        assert_eq!(refused.stage, DisplayLocation::InputMap);
        let outcome = refused.outcome();
        assert_eq!(outcome.code().as_str(), "display_map_unresolved");
        assert_eq!(
            outcome
                .value("display_problem")
                .and_then(|value| value.as_str()),
            Some(problem.as_str())
        );
        assert_eq!(
            outcome.value("stage").and_then(|value| value.as_str()),
            Some("display_input_map")
        );
        for (key, number) in [
            ("frame_width", frame.0),
            ("frame_height", frame.1),
            ("base_width", base.0),
            ("base_height", base.1),
        ] {
            assert_eq!(
                outcome.value(key).and_then(|value| value.as_u64()),
                Some(u64::from(number)),
                "{problem:?} {key}"
            );
        }
    }
}
