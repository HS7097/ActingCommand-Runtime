// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #406 T4 (exec key 112; register §1.138; brief-T4 Requirement 3): actinglab reads the
// path drag `single_touch_path_drag_v1` (Workflow #406 T3) as it reads the vertical-brake drag, in
// the form source compilation writes it into the navigation (`box_rect`, `turns`,
// `min_segment_px`, `duration_ms`, `corner_hold_ms`), and the Lab does not perform it: no route
// passes it, its page-projection element is not actionable (`gesture_not_projectable`, no input),
// so `do` on it is `lab_element_unavailable`, offline `observe` lists its edge with `input: null`,
// and as a destructive page operation it covers its box. An offline run of a package that holds
// one reports the path the kernel generated in `WouldClick`. Packages and frames are synthetic
// and neutral; no Runtime or device is involved.

use super::*;
use actingcommand_contract::codes::RuntimeContractCode;
use actingcommand_contract::{InputAction, LabOperationSelection};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_lab::ExternallyVerifiedBundle;
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const ENTRY: [u8; 3] = [40, 90, 160];
const PAGE_A: [u8; 3] = [200, 60, 60];
const GUARD: [u8; 3] = [0, 255, 0];
// The box the path drag turns in.
const PATH_BOX: PackRect = PackRect {
    x: 40,
    y: 60,
    width: 200,
    height: 120,
};

fn rect_value(rect: PackRect) -> Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

// A path drag as source compilation writes it.
fn path_drag() -> Value {
    json!({
        "kind": "single_touch_path_drag_v1",
        "box_rect": rect_value(PATH_BOX),
        "turns": 3,
        "min_segment_px": 30,
        "duration_ms": 1200,
        "corner_hold_ms": 50
    })
}

// The generated navigation of the entry page: a path drag to `page_a`, and a page operation
// (`to` null) that is a path drag, which compilation writes into `page_operations` and
// `destructive_actions`.
fn navigation() -> Value {
    let page_operation = json!({
        "task_id": "task",
        "page": "neutral/entry_page",
        "id": "trace_list",
        "purpose": "trace_list",
        "click": path_drag(),
        "expect_after": null,
        "verify_template": null,
        "consumes": [],
        "produces": []
    });
    json!({
        "schema_version": "0.6",
        "game": "neutral",
        "server": "test",
        "navigation": [
            {"id": "trace_to_page_a", "from_page": "neutral/entry_page",
             "to_page": "neutral/page_a", "click": path_drag(), "source": ""}
        ],
        "page_operations": [page_operation.clone()],
        "destructive_actions": [page_operation]
    })
}

fn zip_entries(entries: &[(&str, Value)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(value).expect("JSON"))
            .expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

// The observation package, admitted as pack containment admits it; its pages are told apart by
// the colour of the frame's top-left pixel.
fn bundle() -> ExternallyVerifiedBundle {
    let page_target = |page: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": format!("page/{page}"),
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let package = zip_entries(&[
        (
            "control.json",
            json!({"game": "neutral", "server": "test", "package_id": "neutral.t4.path",
                   "entry_task_id": "task"}),
        ),
        ("resources/manifest.json", json!({"entry_task_id": "task"})),
        (
            "resources/operations/task/task.json",
            json!({"task_id": "task"}),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            json!({
                "schema_version": "0.7",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": WIDTH, "height": HEIGHT},
                "targets": [page_target("entry_page", ENTRY), page_target("page_a", PAGE_A)]
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({
                "schema_version": "0.6",
                "pages": [
                    {"id": "neutral/entry_page", "required": ["page/entry_page"]},
                    {"id": "neutral/page_a", "required": ["page/page_a"]}
                ]
            }),
        ),
        (
            "resources/navigation/neutral.test.navigation.json",
            navigation(),
        ),
    ]);
    ExternallyVerifiedBundle::load_observation("semantic_observe", &package, None)
        .unwrap_or_else(|error| panic!("the package is admitted: {error}"))
}

// A frame of `page`'s colour everywhere, the guard's colour at (1, 0).
fn frame(page: [u8; 3]) -> Frame {
    let mut pixels = page.repeat((WIDTH * HEIGHT) as usize);
    pixels[3..6].copy_from_slice(&GUARD);
    Frame::from_pixels(
        WIDTH,
        HEIGHT,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

// The task package of one operation from the entry page to `page_a` that is the path drag,
// guarded by the colour at (1, 0), in the compiled form.
fn task_package() -> actingcommand_lab::PreparedContainedTask {
    let color = |id: &str, at: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": at, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let bytes = zip_entries(&[
        (
            "control.json",
            json!({
                "schema_version": "Lab-1y.control.v1",
                "package_id": "neutral.t4.path_run",
                "execution_mode": "navigable_route",
                "game": "neutral",
                "server": "test",
                "resolution": {"width": WIDTH, "height": HEIGHT},
                "entry_task_id": "task",
                "capture_interval_ms": 1,
                "step_timeout_ms": 10,
                "timeout_ms": 100,
                "max_steps": 2
            }),
        ),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        (
            "resources/operations/task/task.json",
            json!({
                "schema_version": "0.6",
                "task_id": "task",
                "game": "neutral",
                "server_scope": ["test"],
                "coordinate_space": {"width": WIDTH, "height": HEIGHT},
                "entry_page": "entry_page",
                "target_page": "page_a",
                "operations": [{
                    "id": "trace_to_page_a",
                    "from": "entry_page",
                    "to": "page_a",
                    "click": path_drag(),
                    "guard": {
                        "page_id": "entry_page",
                        "target_id": "guard/ready",
                        "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                        "color_probe": "guard/ready"
                    }
                }]
            }),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            json!({
                "schema_version": "0.3",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": WIDTH, "height": HEIGHT},
                "defaults": {"color_max_distance": 0.0},
                "targets": [
                    color("page/entry_page", 0, ENTRY),
                    color("page/page_a", 0, PAGE_A),
                    color("guard/ready", 1, GUARD)
                ]
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version": "0.3", "pages": [
                {"id": "neutral/entry_page", "required": ["page/entry_page"], "optional": [],
                 "forbidden": []},
                {"id": "neutral/page_a", "required": ["page/page_a"], "optional": [],
                 "forbidden": []}
            ]}),
        ),
    ]);
    actingcommand_lab::PreparedContainedTask::load("neutral.instance", &bytes)
        .unwrap_or_else(|error| panic!("the task package is admitted: {error}"))
}

#[test]
fn t4_the_lab_reads_a_path_drag_and_does_not_perform_it() {
    // The reader reads it with its box; a graph holding it as an edge and as a destructive page
    // operation is read, and no route passes the edge.
    let NavigationClick::PathDrag(drag) =
        parse_navigation_click(&path_drag()).expect("a path drag")
    else {
        panic!("not read as a path drag");
    };
    assert_eq!(drag.box_rect, PATH_BOX);
    let graph = parse_navigation_graph_value(&navigation())
        .unwrap_or_else(|error| panic!("a graph with path drags: {}", error.message));
    assert!(
        find_navigation_route(&graph.edges, "neutral/entry_page", "neutral/page_a").is_none(),
        "no route passes a path drag"
    );
    let mut task_form = path_drag();
    let object = task_form.as_object_mut().expect("object");
    let box_rect = object.remove("box_rect").expect("box_rect");
    object.insert("box".to_string(), box_rect);
    assert!(
        parse_navigation_click(&task_form).is_err(),
        "the task file's `box` is not the navigation's form"
    );

    // Offline `observe`: its element is listed from the page declaration, not actionable, without
    // an input, blocked by `gesture_not_projectable`; `do` on it is `lab_element_unavailable`.
    let bundle = bundle();
    let view = crate::contained_resources::observation_resources(&bundle)
        .unwrap_or_else(|error| panic!("observe admits the package: {}", error.message));
    let (evaluator, detector) = crate::contained_resources::recognition_pipeline(&bundle)
        .unwrap_or_else(|error| panic!("recognition pipeline: {}", error.message));
    let scene =
        Scene::from_rgb8(WIDTH, HEIGHT, &ENTRY.repeat((WIDTH * HEIGHT) as usize)).expect("scene");
    let outcome = detect_current_page(&evaluator, &detector, &scene).expect("page detection");
    assert!(outcome.matched && outcome.page == "neutral/entry_page");
    let (projection, _) = build(
        &view,
        &evaluator,
        &[&scene],
        &outcome,
        &LabOcr::from_installation(None),
        &[],
    )
    .unwrap_or_else(|error| panic!("projection: {}", error.message));
    for resource_id in ["trace_to_page_a", "trace_list"] {
        let element = projection
            .elements
            .iter()
            .chain(&projection.missing)
            .find(|entry| entry["resource_id"] == resource_id)
            .unwrap_or_else(|| panic!("no {resource_id} entry in {projection:?}"));
        assert_eq!(element["actionable"], json!(false), "{element}");
        assert_eq!(element["input"], Value::Null, "{element}");
        assert_eq!(
            element["blocked_reason"],
            json!("gesture_not_projectable"),
            "{element}"
        );
        let id = element["id"].as_str().expect("element id").to_string();
        let refused = LabOperationSelection::Element { id }
            .resolve(&projection)
            .expect_err("do on the path drag's element");
        assert_eq!(
            refused.code().as_str(),
            RuntimeContractCode::LabElementUnavailable.as_str(),
            "{element}"
        );
    }

    // Offline `observe`'s `actions` lists the edge with `input: null`.
    let row = parse_navigation_row(&navigation()["navigation"][0]).expect("the edge's row");
    assert_eq!(
        super::super::navigation_action_json(&row, "neutral/page_a"),
        json!({"id": "trace_to_page_a", "from_page": "neutral/entry_page",
               "to_page": "neutral/page_a", "input": null, "source": ""})
    );

    // An offline run reports the path the kernel generated: one path drag in the box, its
    // `turns + 2` points inside it, its durations summing to `duration_ms`, its corner hold.
    let result = actingcommand_lab::simulate_contained_task(&task_package(), vec![frame(ENTRY)])
        .expect("the offline run");
    let actingcommand_lab::OfflineDecision::WouldClick {
        action:
            InputAction::SingleTouchPathDragV1 {
                box_x,
                box_y,
                box_width,
                box_height,
                points,
                segment_duration_ms,
                corner_hold_ms,
            },
        ..
    } = &result.decision
    else {
        panic!("not a path drag: {:?}", result.decision);
    };
    assert_eq!(
        (*box_x, *box_y, *box_width, *box_height),
        (PATH_BOX.x, PATH_BOX.y, PATH_BOX.width, PATH_BOX.height)
    );
    assert_eq!(points.len(), 5);
    assert!(
        points.iter().all(|point| point.x >= PATH_BOX.x
            && point.y >= PATH_BOX.y
            && point.x < PATH_BOX.x + PATH_BOX.width
            && point.y < PATH_BOX.y + PATH_BOX.height),
        "{points:?}"
    );
    assert_eq!(segment_duration_ms.iter().sum::<u64>(), 1200);
    assert_eq!(*corner_hold_ms, 50);
}
