// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 lpbox (exec key 76; coordinator ruling 6 of wave 16, from Alice's rule of 10-09
// that pack taps use click boxes, never fixed points): a `long_press` (or `long_tap`) click takes
// a click box `x, y, width, height`, and the kernel holds a point picked inside the box for the
// click's duration, drawn with the action seed as a `rect` tap's point is and recorded with the
// box as its one sampling region. A point long press, which the contract already allowed, still
// loads and holds its point. A click with only one of `width` and `height`, or a box that leaves
// the frame, is refused.

use super::*;
use serde_json::json;

const RESOLUTION: Resolution = Resolution {
    width: 100,
    height: 100,
};

fn click(value: serde_json::Value) -> TaskClick {
    serde_json::from_value(value).expect("task click")
}

// The box 10,20 8x6 held for 800 ms: every seed presses a point inside it, the evidence names the
// seed and the box, and the points differ between seeds; without a seed the box's centre is held.
#[test]
fn lpbox_a_long_press_box_holds_a_seeded_point_of_its_box_for_its_duration() {
    for kind in ["long_press", "long_tap"] {
        let press = click(json!({
            "kind": kind, "x": 10, "y": 20, "width": 8, "height": 6, "duration_ms": 800
        }));
        if let Err(error) = press.validate(&RESOLUTION, None, "0.9") {
            panic!("{kind}: a box inside the frame is valid: {error}");
        }
        let mut points = BTreeSet::new();
        for seed in 1..=16 {
            let (action, sampling) = press
                .input_action(&RESOLUTION, None, Some(seed))
                .expect("a sampled long press");
            let InputAction::LongTap { x, y, duration_ms } = action else {
                panic!("{kind}: a long press is a long tap, got {action:?}");
            };
            assert!((10..18).contains(&x), "{kind} seed {seed}: x {x}");
            assert!((20..26).contains(&y), "{kind} seed {seed}: y {y}");
            assert_eq!(duration_ms, 800, "{kind} seed {seed}");
            let sampling = sampling.expect("sampling evidence");
            assert_eq!(sampling.action_seed(), seed);
            assert_eq!(
                sampling.source_regions(),
                [InputSamplingRegion::new(10, 20, 8, 6).expect("region")]
            );
            points.insert((x, y));
        }
        assert!(
            points.len() > 1,
            "{kind}: the pressed point varies inside the box"
        );

        assert_eq!(
            press
                .input_action(&RESOLUTION, None, None)
                .expect("an unseeded long press"),
            (
                InputAction::LongTap {
                    x: 14,
                    y: 23,
                    duration_ms: 800
                },
                None
            )
        );
    }
}

// The point long press the contract allowed before the box keeps loading and holds its point; a
// box needs both `width` and `height`, at least one pixel each, and lies inside the frame.
#[test]
fn lpbox_a_long_press_takes_a_whole_box_inside_the_frame_or_its_allowed_point() {
    let point = click(json!({"kind": "long_press", "x": 8, "y": 10, "duration_ms": 500}));
    if let Err(error) = point.validate(&RESOLUTION, None, "0.9") {
        panic!("a point long press still loads: {error}");
    }
    assert_eq!(
        point
            .input_action(&RESOLUTION, None, Some(78))
            .expect("a point long press"),
        (
            InputAction::LongTap {
                x: 8,
                y: 10,
                duration_ms: 500
            },
            None
        )
    );

    let operation_invalid = ExecutionKernelCode::ContainedTaskOperationInvalid.as_str();
    let out_of_bounds = ExecutionKernelCode::ContainedTaskInputOutOfBounds.as_str();
    for (case, value, code) in [
        (
            "width without height",
            json!({"kind": "long_press", "x": 10, "y": 20, "width": 8, "duration_ms": 800}),
            operation_invalid,
        ),
        (
            "height without width",
            json!({"kind": "long_tap", "x": 10, "y": 20, "height": 6, "duration_ms": 800}),
            operation_invalid,
        ),
        (
            "an empty box",
            json!({"kind": "long_press", "x": 10, "y": 20, "width": 0, "height": 6, "duration_ms": 800}),
            operation_invalid,
        ),
        (
            "a box past the right edge",
            json!({"kind": "long_press", "x": 95, "y": 20, "width": 8, "height": 6, "duration_ms": 800}),
            out_of_bounds,
        ),
    ] {
        let press = click(value);
        let Err(error) = press.validate(&RESOLUTION, None, "0.9") else {
            panic!("{case}: refused at admission");
        };
        assert_eq!(error.code().as_str(), code, "{case}");
        let Err(error) = press.input_action(&RESOLUTION, None, Some(1)) else {
            panic!("{case}: no input is built");
        };
        assert_eq!(error.code().as_str(), code, "{case}");
    }
}
