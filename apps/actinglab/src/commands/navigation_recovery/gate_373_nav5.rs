// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #373 NAV-5 (exec key 78; wave-17 ruling 21): actinglab's navigation reader takes a
// destructive page operation in every click form a navigation edge takes (a `point` or `rect`
// click alone was read before, so a package holding another could not be routed). Its rectangle,
// long press box and drag boxes are its region without a frame; a `target` or `offset` click
// covers the box its target matches on the frame the route is resolved on.

use super::*;

const DESTRUCTIVE_GRAPH: &str = r#"{
    "game": "fixture01",
    "navigation": [
        {"id": "home_to_terminal", "from_page": "fixture01/home", "to_page": "fixture01/terminal",
         "click": {"kind": "rect", "x": 0, "y": 0, "width": 4, "height": 4}}
    ],
    "destructive_actions": [
        {"page": "fixture01/home", "click": {"x": 100, "y": 100, "width": 10, "height": 10}},
        {"page": "fixture01/home", "click": {"kind": "drag",
            "from": {"kind": "rect", "x": 200, "y": 200, "width": 10, "height": 10},
            "to": {"kind": "rect", "x": 300, "y": 300, "width": 10, "height": 10},
            "duration_ms": 400}},
        {"page": "fixture01/home", "click": {"kind": "long_press", "x": 400, "y": 400,
            "width": 10, "height": 10, "duration_ms": 800}},
        {"page": "fixture01/home", "click": {"kind": "target", "target_id": "buy_button"}},
        {"page": "fixture01/home", "click": {"kind": "offset", "target_id": "buy_icon",
            "offset": {"x": 5, "y": 6, "width": 7, "height": 8}}},
        {"page": "fixture01/other", "click": {"kind": "target", "target_id": "elsewhere"}}
    ]
}"#;

fn tap(x: i32, y: i32) -> NavigationEdge {
    NavigationEdge {
        id: "tap".to_string(),
        from_page: "fixture01/home".to_string(),
        to_page: "fixture01/terminal".to_string(),
        input: SemanticInput::Tap {
            rect: PackRect {
                x,
                y,
                width: 1,
                height: 1,
            },
            point: SemanticPoint { x, y },
        },
        source: None,
    }
}

#[test]
fn nav5_a_destructive_action_takes_every_navigation_click_form() {
    let graph =
        parse_navigation_graph_value(&serde_json::from_str(DESTRUCTIVE_GRAPH).expect("graph JSON"))
            .expect("a graph whose destructive actions take every click form");
    let destructive = &graph.destructive_clicks;
    assert_eq!(destructive.len(), 6);
    for (x, y) in [(105, 105), (205, 205), (305, 305), (405, 405)] {
        assert!(
            reject_destructive_overlap(&tap(x, y), destructive).is_err(),
            "({x},{y}) lies in a destructive region"
        );
    }
    assert!(reject_destructive_overlap(&tap(600, 600), destructive).is_ok());
    assert!(has_frame_destructive_regions("fixture01/home", destructive));
    assert!(!has_frame_destructive_regions(
        "fixture01/terminal",
        destructive
    ));
    // The frame resolved a target's box at (600,600): a tap there is blocked on it.
    let edge = tap(600, 600);
    let resolved = [PackRect {
        x: 595,
        y: 595,
        width: 10,
        height: 10,
    }];
    assert!(
        reject_destructive_overlap_on_frame(&edge, &edge.input, destructive, &resolved).is_err()
    );
    assert!(reject_destructive_overlap_on_frame(&edge, &edge.input, destructive, &[]).is_ok());
}
