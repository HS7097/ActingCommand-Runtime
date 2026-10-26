// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-1 (model 392 v1.2 §13, rows DA1-T1 to DA1-T12 and DA1-T14 to DA1-T16): the
//! pure mapping crate's required states, table driven. Every expected value is worked out by
//! hand from the model's rules (§4.2 to §4.7, §7), never by the code under test. The sizes are
//! neutral examples.

use super::*;
use actingcommand_contract::outcome::{Category, LadderClass};
use std::collections::BTreeSet;

const BY_HEIGHT: DisplayRule = DisplayRule {
    scale_by: DisplayScaleBy::Height,
    scale_area: DisplayArea::Screen,
    anchor_area: DisplayArea::Screen,
};

const BY_WIDTH: DisplayRule = DisplayRule {
    scale_by: DisplayScaleBy::Width,
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

fn with_insets(mut observation: DisplayObservation, insets: CaptureInsets) -> DisplayObservation {
    observation.insets = Some(insets);
    observation
}

fn insets(left: u32, top: u32, right: u32, bottom: u32) -> CaptureInsets {
    CaptureInsets {
        left,
        top,
        right,
        bottom,
    }
}

fn build(
    base: (u32, u32),
    observation: DisplayObservation,
    rule: Option<DisplayRule>,
) -> Result<DisplayMap, DisplayMapError> {
    DisplayMap::build(extent(base.0, base.1), observation, rule)
}

fn map(base: (u32, u32), observation: DisplayObservation, rule: Option<DisplayRule>) -> DisplayMap {
    build(base, observation, rule)
        .unwrap_or_else(|error| panic!("{base:?} on {observation:?} under {rule:?}: {error:?}"))
}

/// The `display_map_unresolved` refusal of a build, or a panic naming what came instead.
fn refusal(result: Result<DisplayMap, DisplayMapError>) -> DisplayMapUnresolved {
    match result {
        Err(DisplayMapError::Unresolved(unresolved)) => unresolved,
        other => panic!("expected display_map_unresolved, got {other:?}"),
    }
}

/// The problem a refused mapping carries, or `None` when it maps.
fn problem<T>(result: Result<T, DisplayMapUnresolved>) -> Option<DisplayProblem> {
    result.err().map(|unresolved| unresolved.problem)
}

fn rect(x: i64, y: i64, width: i64, height: i64) -> Rect {
    Rect::new(x, y, width, height)
}

fn point(x: i64, y: i64) -> Point {
    Point::new(x, y)
}

/// DA1-T1: a 1280×720 package on a 1280×720 frame without a declaration is the identity: every
/// grid point and box maps to itself in every direction, whatever the class or anchor, and a
/// declaration changes nothing.
#[test]
fn da1_t1_identity_maps_every_grid_point_and_box_to_itself() {
    for rule in [None, Some(BY_HEIGHT), Some(BY_WIDTH)] {
        let map = map((1280, 720), plain(1280, 720), rule);
        assert_eq!(map.kind(), DisplayMapKind::Identity, "{rule:?}");
        assert_eq!(map.scale(), Scale::ONE);
        assert_eq!(map.scale_extent(), extent(1280, 720));
        assert_eq!(map.scale_frame_extent(), extent(1280, 720));
        assert_eq!(map.anchor_area(), rect(0, 0, 1280, 720));
        assert_eq!(map.frame_area(), rect(0, 0, 1280, 720));
        assert_eq!(map.full_frame(), rect(0, 0, 1280, 720));
        assert_eq!(map.turn(), None);
        assert_eq!(map.declared(), rule.is_some());
        for x in (0..1280).step_by(40) {
            for y in (0..720).step_by(40) {
                let grid_point = point(x, y);
                assert_eq!(map.map_point(grid_point, None), Ok(grid_point));
                assert_eq!(map.device_point(grid_point), grid_point);
                assert_eq!(map.scale_frame_point(grid_point), grid_point);
                assert_eq!(map.display_point(grid_point), grid_point);
                assert_eq!(map.delivered_point(grid_point), grid_point);
                let grid_box = rect(x, y, 40, 40);
                for class in [RegionClass::SearchHint, RegionClass::Positional] {
                    assert_eq!(map.map_region(grid_box, None, class), Ok(grid_box));
                }
                for anchor in Anchor::ALL {
                    assert_eq!(
                        map.map_region(grid_box, Some(anchor), RegionClass::Positional),
                        Ok(grid_box)
                    );
                    assert_eq!(map.map_point(grid_point, Some(anchor)), Ok(grid_point));
                }
                assert_eq!(map.map_input_box(grid_box, None), Ok(grid_box));
                assert_eq!(map.device_input_box(grid_box), Ok(grid_box));
                assert_eq!(map.device_evidence_box(grid_box), grid_box);
                assert_eq!(map.scale_frame_box(grid_box), grid_box);
            }
        }
        // A box across all three columns is never refused or stretched by the identity.
        let bar = rect(0, 664, 1280, 56);
        assert_eq!(map.map_region(bar, None, RegionClass::Positional), Ok(bar));
        assert_eq!(map.device_length(100), 100);
    }
}

/// DA1-T2: a 1920×1080 frame for a 1280×720 package without a declaration is a pure scale:
/// `s` = 3/2, the scale frame 1280×720, no translation; points and boxes go to the device frame
/// and back without drifting.
#[test]
fn da1_t2_pure_scale_has_no_translation_and_round_trips_without_drift() {
    let map = map((1280, 720), plain(1920, 1080), None);
    assert_eq!(map.kind(), DisplayMapKind::Scale);
    assert_eq!((map.scale().num(), map.scale().den()), (3, 2));
    assert_eq!(map.scale_extent(), extent(1280, 720));
    assert_eq!(map.scale_frame_extent(), extent(1280, 720));
    assert_eq!(map.anchor_area(), rect(0, 0, 1280, 720));
    // Odd and even coordinates alike; every 37×37 box stays inside the 1280×720 base.
    for x in (0..1243).step_by(37) {
        for y in (0..683).step_by(37) {
            let grid_point = point(x, y);
            assert_eq!(map.map_point(grid_point, None), Ok(grid_point));
            assert_eq!(
                map.scale_frame_point(map.device_point(grid_point)),
                grid_point
            );
            let grid_box = rect(x, y, 37, 37);
            assert_eq!(
                map.map_region(grid_box, None, RegionClass::Positional),
                Ok(grid_box)
            );
            let input = map
                .device_input_box(grid_box)
                .unwrap_or_else(|error| panic!("{grid_box:?}: {error:?}"));
            assert_eq!(map.scale_frame_box(input), grid_box);
            assert_eq!(
                map.scale_frame_box(map.device_evidence_box(grid_box)),
                grid_box
            );
        }
    }
    assert_eq!(map.device_point(point(640, 360)), point(960, 540));
    assert_eq!(map.device_point(point(1, 1)), point(2, 2));
    assert_eq!(map.scale_frame_point(point(1919, 1079)), point(1279, 719));
}

/// DA1-T3: a 2340×1080 frame for a 1280×720 package, by height: the scale space is 1560×720
/// and the five boxes of model 392 v1.1 §4.4 land where the table says, corner by corner.
#[test]
fn da1_t3_landscape_translation_maps_the_five_reference_boxes() {
    let map = map((1280, 720), plain(2340, 1080), Some(BY_HEIGHT));
    assert_eq!(map.kind(), DisplayMapKind::Translate);
    assert_eq!((map.scale().num(), map.scale().den()), (3, 2));
    assert_eq!(map.scale_extent(), extent(1560, 720));
    assert_eq!(map.anchor_area(), rect(0, 0, 1560, 720));
    let rows = [
        // A bottom bar, left to right: stretched across.
        (rect(0, 664, 1280, 56), None, rect(0, 664, 1560, 56)),
        // A centred button: moved by half the room, 140.
        (rect(590, 600, 100, 40), None, rect(730, 600, 100, 40)),
        // An icon at the top right: moved by all the room, 280.
        (rect(1200, 20, 60, 60), None, rect(1480, 20, 60, 60)),
        // A centred dialog, left to right: stretched, or moved whole with `center`.
        (rect(340, 160, 600, 400), None, rect(340, 160, 880, 400)),
        (
            rect(340, 160, 600, 400),
            Some(Anchor::Center),
            rect(480, 160, 600, 400),
        ),
        // A small box across the left and middle columns: stretched.
        (rect(400, 300, 60, 40), None, rect(400, 300, 200, 40)),
    ];
    for (region, anchor, expected) in rows {
        assert_eq!(
            map.map_region(region, anchor, RegionClass::SearchHint),
            Ok(expected),
            "{region:?} with {anchor:?}"
        );
    }
}

/// DA1-T4: a 1080×2340 frame for a 720×1280 package, by width: the scale space is 720×1560 and
/// the top, middle and bottom thirds move by 0, 140 and 280.
#[test]
fn da1_t4_portrait_translation_moves_the_three_rows_by_0_140_and_280() {
    let map = map((720, 1280), plain(1080, 2340), Some(BY_WIDTH));
    assert_eq!(map.kind(), DisplayMapKind::Translate);
    assert_eq!((map.scale().num(), map.scale().den()), (3, 2));
    assert_eq!(map.scale_extent(), extent(720, 1560));
    assert_eq!(map.anchor_area(), rect(0, 0, 720, 1560));
    for (base_point, expected) in [
        (point(360, 100), point(360, 100)),
        (point(360, 640), point(360, 780)),
        (point(360, 1200), point(360, 1480)),
        (point(10, 1200), point(10, 1480)),
        (point(700, 100), point(700, 100)),
    ] {
        assert_eq!(
            map.map_point(base_point, None),
            Ok(expected),
            "{base_point:?}"
        );
    }
    assert_eq!(
        map.map_region(rect(300, 1200, 120, 60), None, RegionClass::Positional),
        Ok(rect(300, 1480, 120, 60))
    );
}

/// DA1-T5: each of the nine anchors moves a box that crosses both third lines as one piece,
/// by its own share of the room, and keeps its size; without an anchor the same box stretches
/// as a search hint and is refused as a positional box.
#[test]
fn da1_t5_each_explicit_anchor_moves_a_crossing_box_rigidly() {
    // 2400×1200 for a 1280×720 package, by height on the safe area (120 inset at the top): s =
    // 1080 / 720 = 3/2, scale space 1600×800, anchors on the screen: room 320 across, 80 down.
    let rule = DisplayRule {
        scale_by: DisplayScaleBy::Height,
        scale_area: DisplayArea::SafeArea,
        anchor_area: DisplayArea::Screen,
    };
    let observation = with_insets(plain(2400, 1200), insets(0, 120, 0, 0));
    let map = map((1280, 720), observation, Some(rule));
    assert_eq!(map.scale_extent(), extent(1600, 800));
    assert_eq!(map.anchor_area(), rect(0, 0, 1600, 800));
    let crossing = rect(340, 160, 600, 400);
    for (anchor, dx, dy) in [
        (Anchor::TopLeft, 0, 0),
        (Anchor::Top, 160, 0),
        (Anchor::TopRight, 320, 0),
        (Anchor::Left, 0, 40),
        (Anchor::Center, 160, 40),
        (Anchor::Right, 320, 40),
        (Anchor::BottomLeft, 0, 80),
        (Anchor::Bottom, 160, 80),
        (Anchor::BottomRight, 320, 80),
    ] {
        let expected = rect(340 + dx, 160 + dy, 600, 400);
        for class in [RegionClass::SearchHint, RegionClass::Positional] {
            assert_eq!(
                map.map_region(crossing, Some(anchor), class),
                Ok(expected),
                "{anchor:?}"
            );
        }
    }
    assert_eq!(
        map.map_region(crossing, None, RegionClass::SearchHint),
        Ok(rect(340, 160, 920, 480))
    );
    assert_eq!(
        problem(map.map_region(crossing, None, RegionClass::Positional)),
        Some(DisplayProblem::StraddlesZones)
    );
}

/// DA1-T6: on the third lines (`3x = Wb`, `3x = 2·Wb`) a coordinate is in the middle; a box's
/// right and bottom edges are judged at their open ends.
#[test]
fn da1_t6_third_lines_belong_to_the_middle_and_open_ends_decide_the_far_corner() {
    for (value, base, zone) in [
        (399, 1200, Zone::Start),
        (400, 1200, Zone::Middle),
        (800, 1200, Zone::Middle),
        (801, 1200, Zone::End),
        (239, 720, Zone::Start),
        (240, 720, Zone::Middle),
        (480, 720, Zone::Middle),
        (481, 720, Zone::End),
    ] {
        assert_eq!(Zone::of(value, base), zone, "{value} of {base}");
    }
    // 2400×1080 for a 1200×720 package, by height: s = 3/2, scale space 1600×720, room 400.
    let map = map((1200, 720), plain(2400, 1080), Some(BY_HEIGHT));
    assert_eq!(map.scale_extent(), extent(1600, 720));
    for (base_point, expected) in [
        (point(399, 100), point(399, 100)),
        (point(400, 100), point(600, 100)),
        (point(800, 100), point(1000, 100)),
        (point(801, 100), point(1201, 100)),
    ] {
        assert_eq!(
            map.map_point(base_point, None),
            Ok(expected),
            "{base_point:?}"
        );
    }
    for (region, expected) in [
        (rect(0, 0, 399, 100), rect(0, 0, 399, 100)),
        (rect(0, 0, 400, 100), rect(0, 0, 600, 100)),
        (rect(801, 0, 399, 100), rect(1201, 0, 399, 100)),
    ] {
        assert_eq!(
            map.map_region(region, None, RegionClass::SearchHint),
            Ok(expected),
            "{region:?}"
        );
    }
    assert_eq!(
        map.map_region(rect(0, 0, 399, 100), None, RegionClass::Positional),
        Ok(rect(0, 0, 399, 100))
    );
    assert_eq!(
        problem(map.map_region(rect(0, 0, 400, 100), None, RegionClass::Positional)),
        Some(DisplayProblem::StraddlesZones)
    );
}

/// One row of the safe-area table: the rule's areas, then the expected scale, scale space,
/// anchor area and the moves of the start, middle and end zones across and down.
struct SafeAreaRow {
    scale_area: DisplayArea,
    anchor_area: DisplayArea,
    scale: (u32, u32),
    scale_extent: (u32, u32),
    anchor_area_rect: Rect,
    across: [i64; 3],
    down: [i64; 3],
}

/// DA1-T7: with a 90 inset on the left, the four combinations of `scale.area` and
/// `anchor_area` give the hand-worked scale, scale space, anchor area and moves; with insets of
/// 0 the safe area is the screen.
#[test]
fn da1_t7_safe_area_arithmetic_matches_the_hand_table() {
    use DisplayArea::{SafeArea, Screen};
    // By height on 2340×1080: the left inset leaves the height, so s stays 3/2; anchors on the
    // safe area start at 90 · 2/3 = 60 and leave 1500 − 1280 = 220 of room.
    let by_height = [
        (Screen, Screen, (0, 0, 1560, 720), [0, 140, 280]),
        (Screen, SafeArea, (60, 0, 1500, 720), [60, 170, 280]),
        (SafeArea, Screen, (0, 0, 1560, 720), [0, 140, 280]),
        (SafeArea, SafeArea, (60, 0, 1500, 720), [60, 170, 280]),
    ]
    .map(|(scale_area, anchor_area, area, across)| SafeAreaRow {
        scale_area,
        anchor_area,
        scale: (3, 2),
        scale_extent: (1560, 720),
        anchor_area_rect: rect(area.0, area.1, area.2, area.3),
        across,
        down: [0, 0, 0],
    });
    // By width on 2010×1080: the screen gives s = 2010 / 1280 = 201/128 and a scale space of
    // 1280 × round(1080 · 128 / 201) = 1280×688 (32 short, so -16 and -32 down); the safe area
    // gives s = 1920 / 1280 = 3/2 and 1340×720. Anchors on the safe area start at
    // round(90 · 128 / 201) = 57 (room -57: 57, 57 − 29, 0) or 90 · 2/3 = 60 (room 0).
    let by_width = [
        SafeAreaRow {
            scale_area: Screen,
            anchor_area: Screen,
            scale: (201, 128),
            scale_extent: (1280, 688),
            anchor_area_rect: rect(0, 0, 1280, 688),
            across: [0, 0, 0],
            down: [0, -16, -32],
        },
        SafeAreaRow {
            scale_area: Screen,
            anchor_area: SafeArea,
            scale: (201, 128),
            scale_extent: (1280, 688),
            anchor_area_rect: rect(57, 0, 1223, 688),
            across: [57, 28, 0],
            down: [0, -16, -32],
        },
        SafeAreaRow {
            scale_area: SafeArea,
            anchor_area: Screen,
            scale: (3, 2),
            scale_extent: (1340, 720),
            anchor_area_rect: rect(0, 0, 1340, 720),
            across: [0, 30, 60],
            down: [0, 0, 0],
        },
        SafeAreaRow {
            scale_area: SafeArea,
            anchor_area: SafeArea,
            scale: (3, 2),
            scale_extent: (1340, 720),
            anchor_area_rect: rect(60, 0, 1280, 720),
            across: [60, 60, 60],
            down: [0, 0, 0],
        },
    ];
    let left_inset = insets(90, 0, 0, 0);
    for (frame, scale_by, rows) in [
        ((2340, 1080), DisplayScaleBy::Height, by_height.as_slice()),
        ((2010, 1080), DisplayScaleBy::Width, by_width.as_slice()),
    ] {
        for row in rows {
            let rule = DisplayRule {
                scale_by,
                scale_area: row.scale_area,
                anchor_area: row.anchor_area,
            };
            let observation = with_insets(plain(frame.0, frame.1), left_inset);
            let map = map((1280, 720), observation, Some(rule));
            assert_eq!(
                (map.scale().num(), map.scale().den()),
                row.scale,
                "{rule:?}"
            );
            assert_eq!(
                map.scale_extent(),
                extent(row.scale_extent.0, row.scale_extent.1),
                "{rule:?}"
            );
            assert_eq!(map.anchor_area(), row.anchor_area_rect, "{rule:?}");
            // Start, middle and end of the 1280×720 base: x 100, 640, 1200; y 100, 360, 700.
            for (column, x) in [100, 640, 1200].into_iter().enumerate() {
                for (line, y) in [100, 360, 700].into_iter().enumerate() {
                    assert_eq!(
                        map.map_point(point(x, y), None),
                        Ok(point(x + row.across[column], y + row.down[line])),
                        "{rule:?} at ({x}, {y})"
                    );
                }
            }
        }
    }
    // Insets of 0: the safe area is the screen.
    let screen = map((1280, 720), plain(2340, 1080), Some(BY_HEIGHT));
    let safe = DisplayRule {
        scale_by: DisplayScaleBy::Height,
        scale_area: SafeArea,
        anchor_area: SafeArea,
    };
    let zero = map(
        (1280, 720),
        with_insets(plain(2340, 1080), insets(0, 0, 0, 0)),
        Some(safe),
    );
    assert_eq!(zero.kind(), screen.kind());
    assert_eq!(zero.scale(), screen.scale());
    assert_eq!(zero.scale_extent(), screen.scale_extent());
    assert_eq!(zero.anchor_area(), screen.anchor_area());
    assert_eq!(zero.frame_area(), screen.frame_area());
}

/// DA1-T8: a device smaller than the base (`s` below 1) is refused as `scale_below_base`, with
/// or without a declaration; the base's own size passes.
#[test]
fn da1_t8_a_device_smaller_than_the_base_is_refused() {
    for rule in [None, Some(BY_HEIGHT), Some(BY_WIDTH)] {
        let refused = refusal(build((1280, 720), plain(960, 540), rule));
        assert_eq!(refused.problem, DisplayProblem::ScaleBelowBase, "{rule:?}");
        assert_eq!(refused.stage, DisplayLocation::MapBuild);
        assert!(
            build((1280, 720), plain(1280, 720), rule).is_ok(),
            "{rule:?}"
        );
    }
}

/// DA1-T9: without a declaration only a pure scale with no insets in use maps; another ratio,
/// or the same ratio with insets in use, is `layout_undeclared`.
#[test]
fn da1_t9_an_undeclared_layout_maps_only_a_pure_scale() {
    assert_eq!(
        refusal(build((1280, 720), plain(2340, 1080), None)).problem,
        DisplayProblem::LayoutUndeclared
    );
    assert_eq!(
        map((1280, 720), plain(1920, 1080), None).kind(),
        DisplayMapKind::Scale
    );
    let in_use = with_insets(plain(1920, 1080), insets(90, 0, 0, 0));
    assert_eq!(
        refusal(build((1280, 720), in_use, None)).problem,
        DisplayProblem::LayoutUndeclared
    );
    let none_in_use = with_insets(plain(1920, 1080), insets(0, 0, 0, 0));
    assert_eq!(
        map((1280, 720), none_in_use, None).kind(),
        DisplayMapKind::Scale
    );
}

/// DA1-T10: the ratio is never refused: seven ratios in both orientations all build under a
/// declaration, and the only problems any of them can give are the lower bound and the
/// orientation.
#[test]
fn da1_t10_no_ratio_is_ever_refused() {
    // 4:3, 16:10, 16:9, 18:9, 19.5:9, 20:9 and 21:9, each 1080 high (and half of that).
    let long_sides = [1440, 1728, 1920, 2160, 2340, 2400, 2520];
    let allowed = [
        DisplayProblem::ScaleBelowBase,
        DisplayProblem::OrientationMismatch,
    ];
    for long in long_sides {
        for (base, frame, rules) in [
            ((1280, 720), (long, 1080), [BY_HEIGHT, BY_WIDTH]),
            ((720, 1280), (1080, long), [BY_WIDTH, BY_HEIGHT]),
        ] {
            for rule in rules {
                let built = build(base, plain(frame.0, frame.1), Some(rule));
                assert!(
                    built.is_ok(),
                    "{frame:?} for {base:?} under {rule:?}: {built:?}"
                );
                let others = [
                    plain(frame.0 / 2, frame.1 / 2),
                    plain(frame.1, frame.0),
                    plain(frame.1 / 2, frame.0 / 2),
                ];
                for observation in others {
                    match build(base, observation, Some(rule)) {
                        Ok(_) => {}
                        Err(DisplayMapError::Unresolved(unresolved)) => assert!(
                            allowed.contains(&unresolved.problem),
                            "{observation:?} for {base:?}: {unresolved:?}"
                        ),
                        Err(error) => panic!("{observation:?} for {base:?}: {error:?}"),
                    }
                }
            }
        }
    }
}

/// DA1-T11: the four rows of the rotation table, the 9:16 frame of a landscape package at a
/// known rotation turned upright among them.
#[test]
fn da1_t11_the_rotation_table_has_four_rows() {
    let base = extent(1280, 720);
    let landscape = extent(1280, 720);
    let portrait = extent(720, 1280);
    // Row 1: the frame agrees with the package.
    assert_eq!(
        orientation_disposition(base, landscape, landscape, Some(CaptureRotation::R0)),
        OrientationDisposition::AsDelivered
    );
    let as_delivered = map((1280, 720), plain(1280, 720), None);
    assert_eq!(as_delivered.turn(), None);
    assert_eq!(as_delivered.kind(), DisplayMapKind::Identity);
    // Row 2: a natural (portrait) frame of a landscape display at a known rotation is turned.
    for (rotation, turn, delivered) in [
        (
            CaptureRotation::R90,
            QuarterTurn::Clockwise,
            point(200, 1179),
        ),
        (
            CaptureRotation::R270,
            QuarterTurn::Counterclockwise,
            point(519, 100),
        ),
    ] {
        assert_eq!(
            orientation_disposition(base, portrait, landscape, Some(rotation)),
            OrientationDisposition::Turn(turn)
        );
        let observation = DisplayObservation {
            frame: portrait,
            display: landscape,
            rotation: Some(rotation),
            crop: None,
            insets: None,
        };
        let turned = map((1280, 720), observation, None);
        assert_eq!(turned.turn(), Some(turn));
        assert_eq!(turned.kind(), DisplayMapKind::Identity);
        assert_eq!(turned.scale_frame_extent(), extent(1280, 720));
        assert_eq!(turned.delivered_point(point(100, 200)), delivered);
        assert_eq!(turn.turn_point(delivered, portrait), point(100, 200));
    }
    // Row 3: the display disagrees too: this frame cannot be mapped.
    assert_eq!(
        orientation_disposition(base, portrait, portrait, Some(CaptureRotation::R0)),
        OrientationDisposition::Mismatch
    );
    let mismatch = refusal(build((1280, 720), plain(720, 1280), None));
    assert_eq!(mismatch.problem, DisplayProblem::OrientationMismatch);
    assert_eq!(mismatch.stage, DisplayLocation::MapBuild);
    // Row 4: the rotation is unknown.
    assert_eq!(
        orientation_disposition(base, portrait, landscape, None),
        OrientationDisposition::RotationUnknown
    );
    let unknown = DisplayObservation {
        frame: portrait,
        display: landscape,
        rotation: None,
        crop: None,
        insets: None,
    };
    assert_eq!(
        build((1280, 720), unknown, None),
        Err(DisplayMapError::RotationUnknown)
    );
}

/// DA1-T12: the rounding table: points and lengths to the nearest, input boxes inward,
/// evidence boxes outward; an input box with less than one pixel left, and a narrow crossing
/// box on a narrower scale space, are `collapsed`.
#[test]
fn da1_t12_the_rounding_table_holds() {
    // s = 3/2 (1920×1080 for 1280×720).
    let map = map((1280, 720), plain(1920, 1080), None);
    for (scale_point, device) in [
        (point(0, 0), point(0, 0)),
        (point(1, 1), point(2, 2)),
        (point(2, 3), point(3, 5)),
        (point(5, 7), point(8, 11)),
        (point(1279, 719), point(1919, 1079)),
    ] {
        assert_eq!(map.device_point(scale_point), device, "{scale_point:?}");
    }
    for (scale_box, inward, outward) in [
        (rect(0, 0, 1, 1), rect(0, 0, 1, 1), rect(0, 0, 2, 2)),
        (rect(1, 1, 1, 1), rect(2, 2, 1, 1), rect(1, 1, 2, 2)),
        (rect(1, 1, 2, 2), rect(2, 2, 2, 2), rect(1, 1, 4, 4)),
        (
            rect(10, 20, 30, 40),
            rect(15, 30, 45, 60),
            rect(15, 30, 45, 60),
        ),
    ] {
        assert_eq!(map.device_input_box(scale_box), Ok(inward), "{scale_box:?}");
        assert_eq!(map.device_evidence_box(scale_box), outward, "{scale_box:?}");
    }
    for (length, device) in [(0, 0), (1, 2), (7, 11), (100, 150), (101, 152)] {
        assert_eq!(map.device_length(length), device, "{length}");
    }
    // s = 4/3 (1280×720 for 960×540): (1, 1, 1, 1) spans 4/3 .. 8/3, no whole pixel inward.
    let thirds = self::map((960, 540), plain(1280, 720), None);
    let collapsed = thirds
        .device_input_box(rect(1, 1, 1, 1))
        .expect_err("no whole pixel inside");
    assert_eq!(collapsed.problem, DisplayProblem::Collapsed);
    assert_eq!(collapsed.stage, DisplayLocation::InputMap);
    // 1440×1080 by height: scale space 960×720, room -320; a box from the left column (400)
    // into the middle one (460 - 160 = 300) has no width left.
    let narrow = self::map((1280, 720), plain(1440, 1080), Some(BY_HEIGHT));
    assert_eq!(narrow.scale_extent(), extent(960, 720));
    let collapsed = narrow
        .map_region(rect(400, 300, 60, 40), None, RegionClass::SearchHint)
        .expect_err("the crossing box collapses");
    assert_eq!(collapsed.problem, DisplayProblem::Collapsed);
    assert_eq!(collapsed.stage, DisplayLocation::RegionMap);
}

/// The observation of a stream that crops a 1080×2340 display to 1080×2336 (2 off the top).
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

/// DA1-T14: building the same map from the same inputs again gives the same map and the same
/// mappings, bit for bit.
#[test]
fn da1_t14_the_same_inputs_build_the_same_map_bit_for_bit() {
    let safe = DisplayRule {
        scale_by: DisplayScaleBy::Width,
        scale_area: DisplayArea::SafeArea,
        anchor_area: DisplayArea::SafeArea,
    };
    let turned = DisplayObservation {
        frame: extent(720, 1280),
        display: extent(1280, 720),
        rotation: Some(CaptureRotation::R90),
        crop: None,
        insets: None,
    };
    let cases = [
        ((1280, 720), plain(1280, 720), None),
        ((1280, 720), plain(1920, 1080), None),
        ((1280, 720), plain(2340, 1080), Some(BY_HEIGHT)),
        ((720, 1280), plain(1080, 2340), Some(BY_WIDTH)),
        (
            (1280, 720),
            with_insets(plain(2010, 1080), insets(90, 0, 0, 0)),
            Some(safe),
        ),
        ((1080, 2340), cropped(), None),
        ((1280, 720), turned, None),
    ];
    let regions = [
        rect(0, 0, 1, 1),
        rect(13, 17, 101, 59),
        rect(340, 160, 600, 400),
        rect(590, 600, 100, 40),
    ];
    for (base, observation, rule) in cases {
        let first = build(base, observation, rule);
        let second = build(base, observation, rule);
        assert_eq!(first, second, "{base:?} on {observation:?}");
        let (Ok(first), Ok(second)) = (first, second) else {
            panic!("{base:?} on {observation:?} does not build");
        };
        for region in regions {
            for class in [RegionClass::SearchHint, RegionClass::Positional] {
                assert_eq!(
                    first.map_region(region, None, class),
                    second.map_region(region, None, class)
                );
            }
            assert_eq!(
                first.map_input_box(region, Some(Anchor::Center)),
                second.map_input_box(region, Some(Anchor::Center))
            );
            assert_eq!(
                first.device_evidence_box(region),
                second.device_evidence_box(region)
            );
            assert_eq!(
                first.scale_frame_box(region),
                second.scale_frame_box(region)
            );
        }
    }
}

/// DA1-T15: a frame cropped 2 off the top of its 1080×2340 display, for a 1080×2340 package, is
/// the identity on the display: regions lose 2 and are cut to the frame, inputs gain 2, and a
/// region wholly in the cropped rows is `outside_frame`.
#[test]
fn da1_t15_a_cropped_frame_moves_regions_up_and_inputs_down_by_its_offset() {
    let map = map((1080, 2340), cropped(), None);
    assert_eq!(map.kind(), DisplayMapKind::Identity);
    assert_eq!(map.scale(), Scale::ONE);
    assert_eq!(map.scale_extent(), extent(1080, 2340));
    assert_eq!(map.scale_frame_extent(), extent(1080, 2336));
    assert_eq!(map.frame_area(), rect(0, 2, 1080, 2336));
    for class in [RegionClass::SearchHint, RegionClass::Positional] {
        assert_eq!(
            map.map_region(rect(100, 500, 200, 100), None, class),
            Ok(rect(100, 498, 200, 100))
        );
        assert_eq!(
            map.map_region(rect(0, 0, 100, 100), None, class),
            Ok(rect(0, 0, 100, 98))
        );
    }
    let input = map
        .map_input_box(rect(100, 500, 200, 100), None)
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(input, rect(100, 498, 200, 100));
    let frame_point = map
        .map_point(point(150, 550), None)
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(frame_point, point(150, 548));
    assert_eq!(
        map.display_point(map.device_point(frame_point)),
        point(150, 550)
    );
    for outside in [rect(0, 2338, 100, 2), rect(0, 0, 100, 2)] {
        let refused = map
            .map_region(outside, None, RegionClass::SearchHint)
            .expect_err("the cropped rows are outside the frame");
        assert_eq!(refused.problem, DisplayProblem::OutsideFrame, "{outside:?}");
        assert_eq!(refused.stage, DisplayLocation::RegionMap);
    }
    assert_eq!(
        problem(map.map_point(point(150, 1), None)),
        Some(DisplayProblem::OutsideFrame)
    );
}

/// DA1-T16: each of the six `display_problem` tokens, reached through its own path, is the code
/// `display_map_unresolved` (category error, ladder class `declaration_error`) with that token,
/// its stage and the frame's and the base's sizes.
#[test]
fn da1_t16_every_display_problem_is_display_map_unresolved() {
    let translated = map((1280, 720), plain(2340, 1080), Some(BY_HEIGHT));
    let narrow = map((1280, 720), plain(1440, 1080), Some(BY_HEIGHT));
    let cropped_map = map((1080, 2340), cropped(), None);
    let rows = [
        refusal(build((1280, 720), plain(2340, 1080), None)),
        refusal(build((1280, 720), plain(960, 540), None)),
        narrow
            .map_region(rect(400, 300, 60, 40), None, RegionClass::SearchHint)
            .expect_err("collapsed"),
        cropped_map
            .map_region(rect(0, 2338, 100, 2), None, RegionClass::Positional)
            .expect_err("outside the frame"),
        translated
            .map_region(rect(400, 300, 60, 40), None, RegionClass::Positional)
            .expect_err("crosses a third line"),
        refusal(build((1280, 720), plain(720, 1280), None)),
    ];
    let expected = [
        (
            DisplayProblem::LayoutUndeclared,
            DisplayLocation::MapBuild,
            (2340, 1080),
            (1280, 720),
        ),
        (
            DisplayProblem::ScaleBelowBase,
            DisplayLocation::MapBuild,
            (960, 540),
            (1280, 720),
        ),
        (
            DisplayProblem::Collapsed,
            DisplayLocation::RegionMap,
            (1440, 1080),
            (1280, 720),
        ),
        (
            DisplayProblem::OutsideFrame,
            DisplayLocation::RegionMap,
            (1080, 2336),
            (1080, 2340),
        ),
        (
            DisplayProblem::StraddlesZones,
            DisplayLocation::RegionMap,
            (2340, 1080),
            (1280, 720),
        ),
        (
            DisplayProblem::OrientationMismatch,
            DisplayLocation::MapBuild,
            (720, 1280),
            (1280, 720),
        ),
    ];
    assert_eq!(
        DisplayCode::MapUnresolved.ladder_class(),
        LadderClass::DeclarationError
    );
    let mut seen = BTreeSet::new();
    for (unresolved, (problem, stage, frame, base)) in rows.into_iter().zip(expected) {
        assert_eq!(unresolved.problem, problem);
        assert_eq!(unresolved.stage, stage, "{problem:?}");
        assert!(seen.insert(problem), "{problem:?} twice");
        let outcome = unresolved.outcome();
        assert!(outcome.is(DisplayCode::MapUnresolved), "{problem:?}");
        assert_eq!(outcome.code().as_str(), "display_map_unresolved");
        assert_eq!(outcome.category(), Category::Error);
        assert_eq!(
            outcome
                .value("display_problem")
                .and_then(|value| value.as_str()),
            Some(problem.as_str())
        );
        assert_eq!(
            outcome.value("stage").and_then(|value| value.as_str()),
            Some(stage.as_str())
        );
        for (key, number) in [
            ("frame_width", frame.0),
            ("frame_height", frame.1),
            ("base_width", base.0),
            ("base_height", base.1),
        ] {
            assert_eq!(
                outcome.value(key).and_then(|value| value.as_u64()),
                Some(number),
                "{problem:?} {key}"
            );
        }
    }
    // Workflow #392 DA-2a adds three tokens the map itself never gives (the frame gate's
    // `translation_unsupported`, the input side's `input_scale_unsupported` and the host's
    // `safe_area_unavailable`); every other token of the vocabulary has its row here.
    let outside_the_map = [
        DisplayProblem::TranslationUnsupported,
        DisplayProblem::InputScaleUnsupported,
        DisplayProblem::SafeAreaUnavailable,
    ];
    assert_eq!(
        seen,
        DisplayProblem::ALL
            .iter()
            .copied()
            .filter(|problem| !outside_the_map.contains(problem))
            .collect::<BTreeSet<_>>()
    );
}
