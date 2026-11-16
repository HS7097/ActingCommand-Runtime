// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-5ab (`contracts/display-adapter.md` sections 10 and 11; the coordinator's work
//! order of 11-16 on board #392, test "presses"): the one exit an input in base coordinates takes
//! to the device, a run's and a Lab operation's alike (`DisplayMap::send_input`). For a 1280x720
//! base on a 1920x1080 frame (s = 3/2): every point of a rectangle drawn in it leaves inside the
//! rectangle's inward device box, at `round_half_up(v · 3/2)`; a swipe's two points leave as two
//! points do; an input decided on a frame of another size is stale. On a 1600x900 frame
//! (s = 5/4) every input is refused `input_scale_unsupported` at `display_input_map`, before
//! anything is converted.

use super::*;

fn extent(width: u32, height: u32) -> CaptureExtent {
    CaptureExtent::new(width, height).expect("a positive extent")
}

/// The map of a `frame` the Lab reads back from its picture (it shows itself) for a 1280x720 base.
fn lab_map(frame: (u32, u32)) -> DisplayMap {
    let frame = extent(frame.0, frame.1);
    DisplayMap::build(
        extent(1280, 720),
        DisplayObservation {
            frame,
            display: frame,
            rotation: None,
            crop: None,
            insets: None,
        },
        None,
    )
    .expect("a pure scale")
}

/// `value · 3/2`, halves up.
fn three_halves(value: i32) -> i32 {
    (value * 3 + 1).div_euclid(2)
}

#[test]
fn da5ab_an_input_in_base_coordinates_leaves_at_three_halves_inside_its_device_box() {
    let map = lab_map((1920, 1080));
    let device = Some(extent(1920, 1080));
    assert_eq!(
        map.send_input(&InputAction::Tap { x: 640, y: 360 }, &[], device),
        Ok(InputAction::Tap { x: 960, y: 540 })
    );
    assert_eq!(
        map.send_input(&InputAction::Tap { x: 1279, y: 0 }, &[], device),
        Ok(InputAction::Tap { x: 1919, y: 0 })
    );
    // A rectangle (101, 50, 21, 11): each of its points leaves inside its inward device box,
    // left and top `ceil(v · 3/2)`, right and bottom `floor(v · 3/2)`: (152, 75) to (183, 91).
    let rectangle = Rect::new(101, 50, 21, 11);
    for y in 50..61 {
        for x in 101..122 {
            let sent = map
                .send_input(&InputAction::Tap { x, y }, &[rectangle], device)
                .unwrap_or_else(|error| panic!("({x}, {y}): {error:?}"));
            let InputAction::Tap {
                x: sent_x,
                y: sent_y,
            } = sent
            else {
                panic!("({x}, {y}): a tap leaves as a tap");
            };
            assert_eq!(
                (sent_x, sent_y),
                (three_halves(x), three_halves(y)),
                "({x}, {y})"
            );
            assert!(
                (152..183).contains(&sent_x) && (75..91).contains(&sent_y),
                "({x}, {y}) left at ({sent_x}, {sent_y})"
            );
        }
    }
    assert_eq!(
        map.send_input(
            &InputAction::Swipe {
                x1: 0,
                y1: 0,
                x2: 1279,
                y2: 719,
                duration_ms: 300,
            },
            &[Rect::new(0, 0, 1, 1), Rect::new(1279, 719, 1, 1)],
            device,
        ),
        Ok(InputAction::Swipe {
            x1: 0,
            y1: 0,
            x2: 1919,
            y2: 1079,
            duration_ms: 300,
        })
    );
    // Decided on a frame of the base's size, under the map of a 1920x1080 frame: stale.
    assert_eq!(
        map.send_input(
            &InputAction::Tap { x: 640, y: 360 },
            &[],
            Some(extent(1280, 720))
        ),
        Err(InputSendError::Stale(DisplayMapStale {
            decided: extent(1280, 720),
            current: extent(1920, 1080),
        }))
    );
}

#[test]
fn da5ab_an_input_at_five_quarters_is_refused_before_anything_is_converted() {
    let map = lab_map((1600, 900));
    let refused = Err(InputSendError::Unresolved(DisplayMapUnresolved {
        problem: DisplayProblem::InputScaleUnsupported,
        stage: DisplayLocation::InputMap,
        frame: extent(1600, 900),
        base: extent(1280, 720),
    }));
    for action in [
        InputAction::Tap { x: 640, y: 360 },
        InputAction::Tap { x: 0, y: 0 },
        InputAction::Swipe {
            x1: 0,
            y1: 0,
            x2: 1279,
            y2: 719,
            duration_ms: 300,
        },
    ] {
        assert_eq!(
            map.send_input(&action, &[], Some(extent(1600, 900))),
            refused,
            "{action:?}"
        );
    }
}
