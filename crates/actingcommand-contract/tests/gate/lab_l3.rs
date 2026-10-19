// SPDX-License-Identifier: AGPL-3.0-only

//! #411 Lab L3 (exec key 97; register §1.119): the Lab presses gestures as the kernel does. An
//! explicit coordinate selection of a Lab operation may be a long tap, which resolves to the
//! projection's long press (a drag that does not move: one point in one rectangle) and to the
//! long tap itself; an element whose projected input is such a drag resolves to a long tap at its
//! point over its duration, any other drag to one swipe. The Runtime's preparation and the
//! evidence verifier resolve by the same rule, so resolving the same selection again gives the
//! same geometry and action. Projections are synthetic and neutral.

use actingcommand_contract::page_projection::{
    FrameIdentity, FrameKind, Geometry, PageProjection, Point, ProjectionMetrics, Rect,
    WindowCompleteness,
};
use actingcommand_contract::{InputAction, LabOperationSelection};
use serde_json::{Value, json};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;

// A fresh projection of a 320x240 frame with `elements`; `matched` says whether its page was
// recognised (explicit coordinates need no recognised page).
fn projection(matched: bool, elements: Vec<Value>) -> PageProjection {
    PageProjection {
        schema_version: "actingcommand.page-projection.v1".to_string(),
        page: if matched {
            "neutral/entry_page"
        } else {
            "unknown"
        }
        .to_string(),
        state: if matched { "recognized" } else { "unknown" }.to_string(),
        matched,
        standby: false,
        frame: FrameIdentity {
            kind: FrameKind::Rgb8,
            sha256: "0".repeat(64),
            width: WIDTH,
            height: HEIGHT,
        },
        elements,
        unscoped_controls: Vec::new(),
        missing: Vec::new(),
        fields: Vec::new(),
        truncated: false,
        output_truncated: false,
        omitted_count: 0,
        page_window_completeness: WindowCompleteness::Complete,
        window: None,
        metrics: ProjectionMetrics::default(),
        content_sha256: "0".repeat(64),
    }
}

fn rect(x: u32, y: u32, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

// An actionable element of the entry page whose projected input is `input`.
fn element(id: &str, input: &Geometry) -> Value {
    json!({"id": id, "resource_id": id, "actionable": true, "blocked_reason": null,
           "input": input})
}

// A long tap given as explicit coordinates is accepted and resolves to the long press it is, a
// drag that does not move (its point as a 1x1 rectangle at both ends), pressed as that long tap;
// resolving the recorded selection again, as the evidence verifier does, gives the same geometry
// and action.
#[test]
fn l3_explicit_coordinates_take_a_long_tap_and_resolve_it_as_one() {
    let selection = LabOperationSelection::Coordinates {
        action: InputAction::LongTap {
            x: 30,
            y: 40,
            duration_ms: 800,
        },
    };
    selection
        .validate()
        .expect("a long tap is an explicit coordinate selection");

    let point = Point { x: 30, y: 40 };
    let expected = (
        None,
        Geometry::Drag {
            from_rect: rect(30, 40, 1, 1),
            to_rect: rect(30, 40, 1, 1),
            from: point,
            to: point,
            duration_ms: 800,
        },
        InputAction::LongTap {
            x: 30,
            y: 40,
            duration_ms: 800,
        },
    );
    let prepared = selection
        .resolve(&projection(false, Vec::new()))
        .expect("the long tap resolves");
    assert_eq!(prepared, expected);
    let verified = selection
        .resolve(&projection(false, Vec::new()))
        .expect("the verifier resolves it again");
    assert_eq!(verified, expected);
}

// An element whose projected input is a drag that does not move (a long press: one point in one
// rectangle, the readers' form of a `long_press` click box) resolves to a long tap at that point
// over its duration; a drag that moves stays one swipe.
#[test]
fn l3_an_element_drag_that_does_not_move_resolves_to_a_long_tap() {
    let hold_box = rect(30, 40, 8, 6);
    let hold_point = Point { x: 34, y: 43 };
    let hold = Geometry::Drag {
        from_rect: hold_box,
        to_rect: hold_box,
        from: hold_point,
        to: hold_point,
        duration_ms: 800,
    };
    let swipe = Geometry::Drag {
        from_rect: rect(240, 120, 40, 20),
        to_rect: rect(40, 120, 40, 20),
        from: Point { x: 260, y: 130 },
        to: Point { x: 60, y: 130 },
        duration_ms: 400,
    };
    let projection = projection(
        true,
        vec![
            element("hold_to_page_a", &hold),
            element("swipe_to_page_b", &swipe),
        ],
    );

    for (id, geometry, action) in [
        (
            "hold_to_page_a",
            hold,
            InputAction::LongTap {
                x: 34,
                y: 43,
                duration_ms: 800,
            },
        ),
        (
            "swipe_to_page_b",
            swipe,
            InputAction::Swipe {
                x1: 260,
                y1: 130,
                x2: 60,
                y2: 130,
                duration_ms: 400,
            },
        ),
    ] {
        let selection = LabOperationSelection::Element { id: id.to_string() };
        let (selected, resolved_geometry, resolved_action) = selection
            .resolve(&projection)
            .expect("the element resolves");
        assert_eq!(
            selected.as_ref().map(|element| &element["id"]),
            Some(&json!(id))
        );
        assert_eq!(resolved_geometry, geometry, "{id}");
        assert_eq!(resolved_action, action, "{id}");
    }
}
