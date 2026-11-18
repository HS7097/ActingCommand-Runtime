// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #479 F-3b (`contracts/display-adapter.md` section 13; register §1.487): without a
//! declared rule a display within a pixel of its package's base's shape is the base's display
//! scaled, a pure scale with each axis's own factor; every other display maps as before. Every
//! expected value is worked out by hand here, never by the code under test. Against the base
//! 1280x720 (the two products width x base height and height x base width; scaled by its width,
//! its rows; scaled by its height, its columns; each rounded halves up):
//!
//! | Display | Products | Rows | Columns | |
//! |---|---|---|---|---|
//! | 1334x750 | 960480, 960000 | 719.64: 720 | 1280.64: 1281 | within a pixel |
//! | 1366x768 | 983520, 983040 | 719.65: 720 | 1280.63: 1281 | within a pixel |
//! | 1335x750 | 961200, 960000 | 719.10: 719 | 1281.60: 1282 | neither |
//! | 1280x719 | 921600, 920320 | 719 | 1281.78: 1282 | neither |
//! | 2400x1080 (20:9) | 1728000, 1382400 | 576 | 1600 | neither |

use super::*;

const BASE: (u32, u32) = (1280, 720);

const BY_WIDTH: DisplayRule = DisplayRule {
    scale_by: DisplayScaleBy::Width,
    scale_area: DisplayArea::Screen,
    anchor_area: DisplayArea::Screen,
};

const BY_HEIGHT: DisplayRule = DisplayRule {
    scale_by: DisplayScaleBy::Height,
    scale_area: DisplayArea::Screen,
    anchor_area: DisplayArea::Screen,
};

fn extent(width: u32, height: u32) -> CaptureExtent {
    CaptureExtent::new(width, height).expect("a test extent has two positive sides")
}

/// A frame that shows itself, as the Lab reads a frame back from its picture and as a producer
/// that records no geometry delivers one: its own display, no rotation, crop or insets.
fn shown(width: u32, height: u32) -> DisplayObservation {
    DisplayObservation {
        frame: extent(width, height),
        display: extent(width, height),
        rotation: None,
        crop: None,
        insets: None,
    }
}

fn build(frame: (u32, u32), rule: Option<DisplayRule>) -> Result<DisplayMap, DisplayMapError> {
    DisplayMap::build(extent(BASE.0, BASE.1), shown(frame.0, frame.1), rule)
}

fn map(frame: (u32, u32), rule: Option<DisplayRule>) -> DisplayMap {
    build(frame, rule).unwrap_or_else(|error| panic!("{frame:?} under {rule:?}: {error:?}"))
}

/// `round_half_up(value · num / den)` for `value ≥ 0`, by hand: `⌊(2 · value · num + den) / (2 ·
/// den)⌋`.
fn nearest(value: i64, num: i64, den: i64) -> i64 {
    (2 * value * num + den) / (2 * den)
}

/// A factor as (numerator, denominator).
fn factor(scale: Scale) -> (u32, u32) {
    (scale.num(), scale.den())
}

/// The `display_map` a geometry record names for an undeclared pure scale `num / den` of the
/// 1280x720 base: what an exactly proportional display's map records.
fn scale_record(num: u32, den: u32) -> TaskGeometryDisplayMap {
    TaskGeometryDisplayMap {
        kind: DisplayMapKind::Scale,
        scale_num: num,
        scale_den: den,
        scale_width: 1280,
        scale_height: 720,
        base_width: 1280,
        base_height: 720,
        scale_by: DisplayScaleBy::Width,
        scale_area: DisplayArea::Screen,
        anchor_area: DisplayArea::Screen,
        insets_source: None,
        declared: false,
    }
}

/// Test 1: 1334x750 and 1366x768 against 1280x720, with no rule, are a pure scale with the two
/// factors (667/640 across and 25/24 down; 683/640 and 16/15): the scale space, the scale frame
/// and the anchor area are the base, and the record is an exactly proportional display's, the
/// width's factor its `s`. Every base column and every base row goes to the device by its own
/// axis's factor to the nearest, inside the device frame, and back to itself; against the other
/// axis's factor (a game that draws the base at one factor) it lies within the contract's bound:
/// `|W·Hb − H·Wb| / Hb` device pixels across and `/ Wb` down, beyond the half pixel of rounding
/// (480 / 720 = 0.667 and 480 / 1280 = 0.375 for both sizes). A press maps by the same two
/// factors (the base's centre (640, 360) to (667, 375) and (683, 384)), and the input side,
/// under factors whose doubles are not whole, sends none: `input_scale_unsupported` at
/// `display_input_map`.
#[test]
fn f3b_a_display_within_a_pixel_of_the_base_is_its_display_scaled_by_two_factors() {
    for (frame, across, down, press) in [
        ((1334, 750), (667, 640), (25, 24), (667, 375)),
        ((1366, 768), (683, 640), (16, 15), (683, 384)),
    ] {
        assert!(
            extent(frame.0, frame.1).has_shape_of(extent(BASE.0, BASE.1)),
            "{frame:?}"
        );
        let map = map(frame, None);
        assert_eq!(map.kind(), DisplayMapKind::Scale, "{frame:?}");
        assert_eq!(factor(map.scale()), across, "{frame:?}");
        assert_eq!(factor(map.height_scale()), down, "{frame:?}");
        assert_eq!(map.scale_extent(), extent(1280, 720), "{frame:?}");
        assert_eq!(map.scale_frame_extent(), extent(1280, 720), "{frame:?}");
        assert_eq!(map.anchor_area(), Rect::new(0, 0, 1280, 720), "{frame:?}");
        assert_eq!(map.frame_area(), Rect::new(0, 0, 1280, 720), "{frame:?}");
        assert!(!map.is_plain_identity(), "{frame:?}");
        assert_eq!(map.record(), scale_record(across.0, across.1), "{frame:?}");
        let (width, height) = (i64::from(frame.0), i64::from(frame.1));
        let (base_width, base_height) = (i64::from(BASE.0), i64::from(BASE.1));
        let off = (width * base_height - height * base_width).abs();
        for x in 0..base_width {
            let device = map.device_point(Point::new(x, 0));
            assert_eq!(
                device,
                Point::new(nearest(x, width, base_width), 0),
                "{frame:?}: column {x}"
            );
            assert!(device.x < width, "{frame:?}: column {x}");
            // |X − x · H / Hb| ≤ 1/2 + off / Hb, times 2 · Hb.
            assert!(
                (2 * device.x * base_height - 2 * x * height).abs() <= base_height + 2 * off,
                "{frame:?}: column {x} at {}",
                device.x
            );
            assert_eq!(
                map.scale_frame_point(device),
                Point::new(x, 0),
                "{frame:?}: column {x}"
            );
        }
        for y in 0..base_height {
            let device = map.device_point(Point::new(0, y));
            assert_eq!(
                device,
                Point::new(0, nearest(y, height, base_height)),
                "{frame:?}: row {y}"
            );
            assert!(device.y < height, "{frame:?}: row {y}");
            // |Y − y · W / Wb| ≤ 1/2 + off / Wb, times 2 · Wb.
            assert!(
                (2 * device.y * base_width - 2 * y * width).abs() <= base_width + 2 * off,
                "{frame:?}: row {y} at {}",
                device.y
            );
            assert_eq!(
                map.scale_frame_point(device),
                Point::new(0, y),
                "{frame:?}: row {y}"
            );
        }
        let tap = InputAction::Tap { x: 640, y: 360 };
        assert_eq!(
            map.device_input(&tap, &[], InputCoordinates::Display),
            Ok(InputAction::Tap {
                x: press.0,
                y: press.1
            }),
            "{frame:?}"
        );
        let refused = map
            .admit_input(&tap)
            .expect_err("the input side sends under neither factor");
        assert_eq!(
            refused,
            DisplayMapUnresolved {
                problem: DisplayProblem::InputScaleUnsupported,
                stage: DisplayLocation::InputMap,
                frame: extent(frame.0, frame.1),
                base: extent(1280, 720),
            },
            "{frame:?}"
        );
    }
}

/// Test 2: 1335x750 (a pixel too wide), 1280x719 (a row short) and 2400x1080 (20:9) against
/// 1280x720, with no rule, are not of the base's shape and stay `layout_undeclared` at
/// `display_map_build`, with the frame's and the base's sizes, as before this slice.
#[test]
fn f3b_a_display_beyond_a_pixel_of_the_base_shape_is_layout_undeclared() {
    for frame in [(1335, 750), (1280, 719), (2400, 1080)] {
        assert!(
            !extent(frame.0, frame.1).has_shape_of(extent(BASE.0, BASE.1)),
            "{frame:?}"
        );
        match build(frame, None) {
            Err(DisplayMapError::Unresolved(refused)) => assert_eq!(
                refused,
                DisplayMapUnresolved {
                    problem: DisplayProblem::LayoutUndeclared,
                    stage: DisplayLocation::MapBuild,
                    frame: extent(frame.0, frame.1),
                    base: extent(1280, 720),
                },
                "{frame:?}"
            ),
            other => panic!("{frame:?}: expected layout_undeclared, got {other:?}"),
        }
    }
}

/// Test 3: 1280x720, 1920x1080 and 2560x1440 against 1280x720, with no rule, map as before this
/// slice, member for member: the plain identity and the pure scales 3/2 and 2, one factor on both
/// axes, the base's scale space, anchor area and scale frame, no offset, turn, crop or rule; each
/// point goes to `round_half_up(v · s)` on both axes; the two scales record what they recorded.
#[test]
fn f3b_an_exactly_proportional_display_maps_as_before_member_for_member() {
    for (frame, kind, s) in [
        (
            (1280, 720),
            DisplayMapKind::Identity,
            Scale { num: 1, den: 1 },
        ),
        (
            (1920, 1080),
            DisplayMapKind::Scale,
            Scale { num: 3, den: 2 },
        ),
        (
            (2560, 1440),
            DisplayMapKind::Scale,
            Scale { num: 2, den: 1 },
        ),
    ] {
        let built = map(frame, None);
        assert_eq!(
            built,
            DisplayMap {
                kind,
                base: extent(1280, 720),
                frame: extent(frame.0, frame.1),
                turn: None,
                scale: s,
                height_scale: s,
                scale_extent: extent(1280, 720),
                anchor_area: Rect::new(0, 0, 1280, 720),
                frame_offset: Point::new(0, 0),
                scale_frame: extent(1280, 720),
                crop: None,
                rule: None,
            },
            "{frame:?}"
        );
        assert_eq!(
            built.is_plain_identity(),
            kind == DisplayMapKind::Identity,
            "{frame:?}"
        );
        let (num, den) = (i64::from(s.num), i64::from(s.den));
        for v in (0..720).step_by(7) {
            assert_eq!(
                built.device_point(Point::new(v, v)),
                Point::new(nearest(v, num, den), nearest(v, num, den)),
                "{frame:?}: ({v}, {v})"
            );
        }
    }
    assert_eq!(map((1920, 1080), None).record(), scale_record(3, 2));
    assert_eq!(map((2560, 1440), None).record(), scale_record(2, 1));
}

/// Test 4: a declared rule for 1334x750 still decides, as before this slice. By width it is the
/// rule's pure scale with one factor, 667/640, on both axes (750 · 640 / 667 = 719.64 rows,
/// 720), so the base row 83 goes to 83 · 667 / 640 = 86.50 (87) and the record names the
/// declaration; without a rule the same row goes by the height's 25/24 to 86.46 (86). By height it
/// is the rule's translation, 25/24 on a scale space of 1334 · 24 / 25 = 1280.64 (1281) columns,
/// under which nothing is sent, `translation_unsupported`.
#[test]
fn f3b_a_declared_rule_for_1334x750_still_decides() {
    let by_width = map((1334, 750), Some(BY_WIDTH));
    assert_eq!(by_width.kind(), DisplayMapKind::Scale);
    assert_eq!(factor(by_width.scale()), (667, 640));
    assert_eq!(factor(by_width.height_scale()), (667, 640));
    assert_eq!(by_width.scale_extent(), extent(1280, 720));
    assert_eq!(by_width.rule(), Some(BY_WIDTH));
    assert_eq!(by_width.device_point(Point::new(0, 83)), Point::new(0, 87));
    assert_eq!(
        by_width.record(),
        TaskGeometryDisplayMap {
            declared: true,
            ..scale_record(667, 640)
        }
    );
    assert_eq!(
        map((1334, 750), None).device_point(Point::new(0, 83)),
        Point::new(0, 86)
    );
    let by_height = map((1334, 750), Some(BY_HEIGHT));
    assert_eq!(by_height.kind(), DisplayMapKind::Translate);
    assert_eq!(factor(by_height.scale()), (25, 24));
    assert_eq!(factor(by_height.height_scale()), (25, 24));
    assert_eq!(by_height.scale_extent(), extent(1281, 720));
    let refused = by_height
        .admit_input(&InputAction::Tap { x: 640, y: 360 })
        .expect_err("a translation sends nothing");
    assert_eq!(refused.problem, DisplayProblem::TranslationUnsupported);
    assert_eq!(refused.stage, DisplayLocation::InputMap);
}
