// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab L3 (exec key 97; register §1.119): the Lab presses gestures as the kernel does.
// `do --capture --record` on a step that declares a long press sends one long tap at the planned
// point over the declared duration, a drag one swipe and a tap a tap, and the Lab operation
// request takes each of them. A route of `ensure` presses a `long_press` edge (the reader's drag
// that does not move) as a long tap at the point it holds, a `drag` as one swipe and a tap as a
// tap. Every expected value comes from the slice's requirement; plans and clicks are synthetic
// and neutral, and no Runtime or device is involved.

use super::*;
use actingcommand_lab::RecordRect;

const STEP_RECT: RecordRect = RecordRect {
    x: 60,
    y: 10,
    width: 8,
    height: 8,
};

// The click `record_plan_click` plans on the open step: at the centre of the step's click
// rectangle; a drag ends at the centre of its destination, a long press where it began.
fn plan(gesture: Option<ClickGesture>, to_point: Option<RecordPoint>) -> ClickPlan {
    ClickPlan {
        record_id: "record_l3".to_string(),
        step: 1,
        rect: STEP_RECT,
        point: RecordPoint { x: 64, y: 14 },
        point_rule: "rect_center".to_string(),
        source: "declared".to_string(),
        declares_click: false,
        gesture,
        to_point,
    }
}

#[test]
fn l3_do_capture_record_presses_a_long_press_step_as_a_long_tap() {
    for (case, plan, action) in [
        (
            "a long press",
            plan(
                Some(ClickGesture::LongPress { duration_ms: 800 }),
                Some(RecordPoint { x: 64, y: 14 }),
            ),
            InputAction::LongTap {
                x: 64,
                y: 14,
                duration_ms: 800,
            },
        ),
        (
            "a drag",
            plan(
                Some(ClickGesture::Drag {
                    to: RecordRect {
                        x: 60,
                        y: 40,
                        width: 8,
                        height: 8,
                    },
                    duration_ms: 300,
                }),
                Some(RecordPoint { x: 64, y: 44 }),
            ),
            InputAction::Swipe {
                x1: 64,
                y1: 14,
                x2: 64,
                y2: 44,
                duration_ms: 300,
            },
        ),
        ("a tap", plan(None, None), InputAction::Tap { x: 64, y: 14 }),
    ] {
        let selection = recorded_click_selection(&plan).expect("the planned click's input");
        assert_eq!(
            selection,
            LabOperationSelection::Coordinates { action },
            "{case}"
        );
        selection
            .validate()
            .expect("the Lab operation request takes the planned input");
    }
}

#[test]
fn l3_ensure_presses_a_long_press_edge_as_a_long_tap() {
    for (case, click, action) in [
        (
            "a long press in a click box",
            json!({"kind": "long_press", "x": 30, "y": 40, "width": 8, "height": 6,
                   "duration_ms": 800}),
            InputAction::LongTap {
                x: 34,
                y: 43,
                duration_ms: 800,
            },
        ),
        (
            "a long press at a point",
            json!({"kind": "long_press", "x": 30, "y": 40, "duration_ms": 800}),
            InputAction::LongTap {
                x: 30,
                y: 40,
                duration_ms: 800,
            },
        ),
        (
            "a drag",
            json!({"kind": "drag",
                   "from": {"x": 240, "y": 120, "width": 40, "height": 20},
                   "to": {"x": 40, "y": 120, "width": 40, "height": 20},
                   "duration_ms": 400}),
            InputAction::Swipe {
                x1: 260,
                y1: 130,
                x2: 60,
                y2: 130,
                duration_ms: 400,
            },
        ),
        (
            "a tap",
            json!({"kind": "rect", "x": 10, "y": 20, "width": 4, "height": 6}),
            InputAction::Tap { x: 12, y: 23 },
        ),
    ] {
        let input = crate::commands::parse_navigation_input(&click).expect("a navigation click");
        assert_eq!(
            runtime_input_action(&input).expect("a Runtime input"),
            action,
            "{case}"
        );
    }
}
