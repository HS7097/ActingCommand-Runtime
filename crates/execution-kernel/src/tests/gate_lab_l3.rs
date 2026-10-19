// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab L3 (exec key 97; register §1.119): the Lab presses gestures as the kernel does. The
// drive reader's input for a navigation click, which the Lab's `navigate` presses through the
// Runtime, is a long tap for a `long_press` click (the reader's drag that does not move, held at
// the box's centre or at its point over `duration_ms`) and one swipe for a `drag`, as the kernel
// presses a pack's long press and drag. Clicks are synthetic and neutral.

use crate::drive::parse_navigation_input;
use actingcommand_contract::InputAction;
use serde_json::json;

#[test]
fn l3_navigate_presses_a_long_press_as_a_long_tap_and_a_drag_as_a_swipe() {
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
    ] {
        let input = parse_navigation_input(&click).expect("a navigation click");
        assert_eq!(
            input.resolved_input_action().expect("a Runtime input"),
            action,
            "{case}"
        );
    }
}
