// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab L2 (exec key 90; register §1.104): the kernel's drive reader reads a segmented drag,
// the click `single_touch_drag_with_vertical_brake_v1` that source compilation writes into the
// navigation in the canonical form (`from_rect`, `corner_rect`, `horizontal_duration_ms`,
// `corner_hold_ms`, `brake_distance_px`, `brake_duration_ms`), and checks its shape as pack
// containment does. A package with such an operation (`to` a page, a page set or null) is
// admitted for online observation as a package with a plain `drag` would be, so it can be the
// carrier of `observe --capture` and `do --capture`; its element is listed with
// `actionable: false`, `input: null` and `blocked_reason: gesture_not_projectable`, `do` on it
// answers `lab_element_unavailable`, and no route passes it. Packages and frames are synthetic
// and neutral.

use crate::codes::ExecutionKernelCode;
use crate::drive::parse_navigation_click;
use crate::{
    DriveNavigationClick, DriveNavigationGraph, DriveSegmentedDrag, PreparedPageObservation,
};
use actingcommand_contract::LabOperationSelection;
use actingcommand_contract::codes::RuntimeContractCode;
use actingcommand_contract::page_projection::{FrameIdentity, FrameKind, PageProjection};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_recognition_pack::PackRect;
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const ENTRY: [u8; 3] = [40, 90, 160];
const PAGE_A: [u8; 3] = [200, 60, 60];
const PAGE_B: [u8; 3] = [60, 200, 60];

fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
    PackRect {
        x,
        y,
        width,
        height,
    }
}

fn rect_json(rect: PackRect) -> Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

// A list scroll as source compilation writes it: the press near the bottom right, the corner
// near the bottom left, and the brake 100 px up from the corner, with the durations the kernel
// requires (200 / 150 / 200 ms).
fn segmented_drag() -> Value {
    json!({
        "kind": "single_touch_drag_with_vertical_brake_v1",
        "from_rect": rect_json(rect(240, 200, 40, 20)),
        "corner_rect": rect_json(rect(40, 180, 40, 20)),
        "horizontal_duration_ms": 200,
        "corner_hold_ms": 150,
        "brake_distance_px": 100,
        "brake_duration_ms": 200
    })
}

fn plain_drag() -> Value {
    json!({
        "kind": "drag",
        "from": rect_json(rect(240, 120, 40, 20)),
        "to": rect_json(rect(40, 120, 40, 20)),
        "duration_ms": 400
    })
}

// The generated navigation: from `entry_page` a segmented drag to `page_a`, one to the page set
// `page_a` / `page_b`, a plain drag to `page_b`, and a page operation (`to` null) that is a
// segmented drag, which compilation writes into `page_operations` and `destructive_actions`.
fn navigation() -> Value {
    let page_operation = json!({
        "task_id": "task",
        "page": "neutral/entry_page",
        "id": "scroll_list",
        "purpose": "scroll_list",
        "click": segmented_drag(),
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
            {"id": "scroll_to_page_a", "from_page": "neutral/entry_page",
             "to_page": "neutral/page_a", "click": segmented_drag(), "source": ""},
            {"id": "scroll_to_either", "from_page": "neutral/entry_page",
             "to_pages": ["neutral/page_a", "neutral/page_b"], "click": segmented_drag(),
             "source": ""},
            {"id": "swipe_to_page_b", "from_page": "neutral/entry_page",
             "to_page": "neutral/page_b", "click": plain_drag(), "source": ""}
        ],
        "page_operations": [page_operation.clone()],
        "destructive_actions": [page_operation]
    })
}

fn zip_entries(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(bytes).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn json_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).expect("JSON")
}

// An observation package (the Lab resource layout) whose pages are told apart by the colour of
// the frame's top-left pixel.
fn package() -> Vec<u8> {
    let page_target = |page: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": format!("page/{page}"),
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    zip_entries(&[
        (
            "control.json",
            json_bytes(json!({
                "game": "neutral",
                "server": "test",
                "package_id": "neutral.lab_l2",
                "entry_task_id": "task"
            })),
        ),
        (
            "resources/manifest.json",
            json_bytes(json!({"entry_task_id": "task"})),
        ),
        (
            "resources/operations/task/task.json",
            json_bytes(json!({"task_id": "task"})),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            json_bytes(json!({
                "schema_version": "0.7",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": WIDTH, "height": HEIGHT},
                "targets": [
                    page_target("entry_page", ENTRY),
                    page_target("page_a", PAGE_A),
                    page_target("page_b", PAGE_B)
                ]
            })),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json_bytes(json!({
                "schema_version": "0.6",
                "pages": [
                    {"id": "neutral/entry_page", "required": ["page/entry_page"]},
                    {"id": "neutral/page_a", "required": ["page/page_a"]},
                    {"id": "neutral/page_b", "required": ["page/page_b"]}
                ]
            })),
        ),
        (
            "resources/navigation/neutral.test.navigation.json",
            json_bytes(navigation()),
        ),
    ])
}

// The projection of the online observation of the entry page.
fn observe_entry_page() -> PageProjection {
    let observation = PreparedPageObservation::load("node.a", &package(), &[], None)
        .unwrap_or_else(|error| {
            panic!(
                "the package is admitted for online observation: {error} ({})",
                error.cause()
            )
        });
    let png = Frame::from_pixels(
        WIDTH,
        HEIGHT,
        ENTRY.repeat((WIDTH * HEIGHT) as usize),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG");
    let evaluated = observation
        .evaluate(
            &png,
            FrameIdentity {
                kind: FrameKind::Rgb8,
                sha256: "0".repeat(64),
                width: WIDTH,
                height: HEIGHT,
            },
        )
        .unwrap_or_else(|error| panic!("observation: {error} ({})", error.cause()));
    assert_eq!(evaluated.projection.page, "neutral/entry_page");
    evaluated.projection
}

fn element<'a>(projection: &'a PageProjection, resource_id: &str) -> &'a Value {
    projection
        .elements
        .iter()
        .find(|element| element["resource_id"] == resource_id)
        .unwrap_or_else(|| panic!("no {resource_id} element in {projection:?}"))
}

// The drive reader reads a segmented drag with its boxes and brake distance; a graph holding one
// as an edge to `page_a` and as a destructive page operation is read, and no route passes the
// edge, while the plain drag to `page_b` stays a route.
#[test]
fn lab_l2_the_drive_reader_reads_a_segmented_drag_and_no_route_passes_it() {
    assert_eq!(
        parse_navigation_click(&segmented_drag()).expect("a segmented drag"),
        DriveNavigationClick::SegmentedDrag(DriveSegmentedDrag {
            from_rect: rect(240, 200, 40, 20),
            corner_rect: rect(40, 180, 40, 20),
            brake_distance_px: 100,
        })
    );

    let graph = DriveNavigationGraph::parse_json(&navigation().to_string())
        .expect("a navigation graph with segmented drags");
    assert!(
        graph
            .find_route("neutral/entry_page", "neutral/page_a")
            .is_none(),
        "no route passes a segmented drag"
    );
    let route = graph
        .find_route("neutral/entry_page", "neutral/page_b")
        .expect("the plain drag's route");
    assert_eq!(route.len(), 1);
    assert_eq!(route[0].id(), "swipe_to_page_b");
}

// A segmented drag of another shape than the canonical click is refused: the task file's
// member names, a rectangle with a member it does not take, a missing duration, a brake
// distance that is not an integer.
#[test]
fn lab_l2_the_drive_reader_refuses_a_segmented_drag_of_another_shape() {
    let mut task_file_names = segmented_drag();
    let object = task_file_names.as_object_mut().expect("object");
    let from = object.remove("from_rect").expect("from_rect");
    let corner = object.remove("corner_rect").expect("corner_rect");
    object.insert("from".to_string(), from);
    object.insert("corner".to_string(), corner);
    let mut rect_with_kind = segmented_drag();
    rect_with_kind["from_rect"]["kind"] = json!("rect");
    let mut without_brake_duration = segmented_drag();
    without_brake_duration
        .as_object_mut()
        .expect("object")
        .remove("brake_duration_ms");
    let mut brake_as_text = segmented_drag();
    brake_as_text["brake_distance_px"] = json!("100");

    for (case, click) in [
        ("task-file member names", task_file_names),
        ("a rectangle with a kind", rect_with_kind),
        ("no brake duration", without_brake_duration),
        ("a brake distance as text", brake_as_text),
    ] {
        let Err(error) = parse_navigation_click(&click) else {
            panic!("{case}: the click is refused");
        };
        assert_eq!(
            error.code(),
            ExecutionKernelCode::DrivePlanInvalid.code(),
            "{case}"
        );
        let mut navigation = navigation();
        navigation["navigation"][0]["click"] = click;
        assert!(
            DriveNavigationGraph::parse_json(&navigation.to_string()).is_err(),
            "{case}: a graph holding it is refused"
        );
    }
}

// A package whose navigation holds segmented drags to a page, to a page set and as a page
// operation is admitted for online observation, and each is listed from the page declaration,
// not actionable, without an input, blocked by `gesture_not_projectable`; the plain drag of the
// same page keeps its input.
#[test]
fn lab_l2_a_package_with_segmented_drags_is_observed_online_with_them_not_actionable() {
    let projection = observe_entry_page();

    for resource_id in ["scroll_to_page_a", "scroll_to_either", "scroll_list"] {
        let element = element(&projection, resource_id);
        assert_eq!(element["recognized"], json!(true), "{element}");
        assert_eq!(
            element["recognition_basis"],
            json!("matched_page_declaration"),
            "{element}"
        );
        assert_eq!(element["actionable"], json!(false), "{element}");
        assert_eq!(element["input"], Value::Null, "{element}");
        assert_eq!(
            element["blocked_reason"],
            json!("gesture_not_projectable"),
            "{element}"
        );
    }
    let swipe = element(&projection, "swipe_to_page_b");
    assert_eq!(swipe["actionable"], json!(true), "{swipe}");
    assert_eq!(
        swipe["input"],
        json!({"type": "drag",
               "from_rect": {"x": 240, "y": 120, "width": 40, "height": 20},
               "to_rect": {"x": 40, "y": 120, "width": 40, "height": 20},
               "from": {"x": 260, "y": 130}, "to": {"x": 60, "y": 130},
               "duration_ms": 400}),
        "{swipe}"
    );
}

// `do <element-id>` on a segmented drag's element answers `lab_element_unavailable`, as for any
// element that is not actionable.
#[test]
fn lab_l2_do_on_a_segmented_drag_element_is_unavailable() {
    let projection = observe_entry_page();

    for resource_id in ["scroll_to_page_a", "scroll_to_either", "scroll_list"] {
        let id = element(&projection, resource_id)["id"]
            .as_str()
            .expect("element id")
            .to_string();
        let selection = LabOperationSelection::Element { id };
        let Err(error) = selection.resolve(&projection) else {
            panic!("{resource_id}: do on a segmented drag is refused");
        };
        assert_eq!(
            error.code(),
            RuntimeContractCode::LabElementUnavailable.code(),
            "{resource_id}"
        );
    }
}
