// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab L2 and A13's A part (exec key 90; register §1.104). actinglab's navigation reader
// reads a segmented drag, the click `single_touch_drag_with_vertical_brake_v1` that source
// compilation writes into the navigation in the canonical form, and checks its shape as pack
// containment does. A package with such an operation (`to` a page, a page set or null) is
// admitted by offline `observe` and by the navigation that `do --scene`, `ensure --scene` and
// their `--capture` forms read, as a package with a plain `drag` would be; its element is listed
// with `actionable: false`, `input: null` and `blocked_reason: gesture_not_projectable`, and no
// route passes it. A13: offline `observe` projects an operation whose `to` is a page set from
// its generated navigation row, with its declared or resolved input, as the online observation
// does (resource-declarations.md: such an edge is a navigation element like any other).
// Packages and frames are synthetic and neutral.

use super::*;
use actingcommand_contract::codes::SharedCode;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_lab::ExternallyVerifiedBundle;
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const ENTRY: [u8; 3] = [40, 90, 160];
const PAGE_A: [u8; 3] = [200, 60, 60];
const PAGE_B: [u8; 3] = [60, 200, 60];
const ICON: u32 = 16;
const ICON_X: u32 = 40;
const ICON_Y: u32 = 24;

fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
    PackRect {
        x,
        y,
        width,
        height,
    }
}

fn rect_value(rect: PackRect) -> Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

// A list scroll as source compilation writes it: the press near the bottom right, the corner
// near the bottom left, and the brake 100 px up from the corner, with the durations the kernel
// requires (200 / 150 / 200 ms).
fn segmented_drag() -> Value {
    json!({
        "kind": "single_touch_drag_with_vertical_brake_v1",
        "from_rect": rect_value(rect(240, 200, 40, 20)),
        "corner_rect": rect_value(rect(40, 180, 40, 20)),
        "horizontal_duration_ms": 200,
        "corner_hold_ms": 150,
        "brake_distance_px": 100,
        "brake_duration_ms": 200
    })
}

// An operation from `entry_page` whose `to` is the page set `page_a` / `page_b`, as the generated
// navigation keeps it.
fn page_set_row(id: &str, click: Value) -> Value {
    json!({"id": id, "from_page": "neutral/entry_page",
           "to_pages": ["neutral/page_a", "neutral/page_b"], "click": click, "source": ""})
}

// The generated navigation of the entry page: a segmented drag to `page_a` and one to the page
// set, a plain drag to `page_b`, three page-set operations clicking a point, a box and a target,
// and a page operation (`to` null) that is a segmented drag, which compilation writes into
// `page_operations` and `destructive_actions`.
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
            page_set_row("scroll_to_either", segmented_drag()),
            {"id": "swipe_to_page_b", "from_page": "neutral/entry_page",
             "to_page": "neutral/page_b", "source": "",
             "click": {"kind": "drag", "from": rect_value(rect(240, 120, 40, 20)),
                       "to": rect_value(rect(40, 120, 40, 20)), "duration_ms": 400}},
            page_set_row("open_either_at_point", json!({"kind": "point", "point": "1,0"})),
            page_set_row(
                "open_either_in_box",
                json!({"kind": "rect", "x": 200, "y": 40, "width": 60, "height": 30})
            ),
            page_set_row(
                "open_either_on_icon",
                json!({"kind": "target_center", "target_id": "ui/icon"})
            )
        ],
        "page_operations": [page_operation.clone()],
        "destructive_actions": [page_operation]
    })
}

// A deterministic generator of neutral gray noise.
struct Noise(u64);

impl Noise {
    fn sample(&mut self) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        1 + ((self.0 >> 33) % 255) as u8
    }
}

// The icon `ui/icon`, gray noise drawn with equal red, green and blue.
fn icon_pixels() -> Vec<u8> {
    let mut noise = Noise(0x0411_0090);
    (0..ICON * ICON)
        .flat_map(|_| {
            let value = noise.sample();
            [value; 3]
        })
        .collect()
}

fn png(width: u32, height: u32, pixels: Vec<u8>) -> Vec<u8> {
    Frame::from_pixels(
        width,
        height,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

// The entry page: its colour everywhere and, when asked, the icon at (40, 24).
fn entry_frame(with_icon: bool) -> Scene {
    let mut pixels = ENTRY.repeat((WIDTH * HEIGHT) as usize);
    if with_icon {
        let icon = icon_pixels();
        for row in 0..ICON {
            for column in 0..ICON {
                let to = (((ICON_Y + row) * WIDTH + ICON_X + column) * 3) as usize;
                let from = ((row * ICON + column) * 3) as usize;
                pixels[to..to + 3].copy_from_slice(&icon[from..from + 3]);
            }
        }
    }
    Scene::from_rgb8(WIDTH, HEIGHT, &pixels).expect("scene")
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

// The package, admitted as pack containment admits it. Its pages are told apart by the colour
// of the frame's top-left pixel; `ui/icon` is a template that scores `ccoeff_normed`, so a flat
// frame does not match it.
fn bundle() -> ExternallyVerifiedBundle {
    let page_target = |page: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": format!("page/{page}"),
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let package = zip_entries(&[
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
                "defaults": {"match_metric": "ccoeff_normed"},
                "targets": [
                    page_target("entry_page", ENTRY),
                    page_target("page_a", PAGE_A),
                    page_target("page_b", PAGE_B),
                    {"type": "template", "id": "ui/icon",
                     "template_path": "recognition/assets/icon.png",
                     "region": "full_frame", "threshold": 0.9}
                ]
            })),
        ),
        (
            "resources/recognition/assets/icon.png",
            png(ICON, ICON, icon_pixels()),
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
    ]);
    ExternallyVerifiedBundle::load_observation("semantic_observe", &package, None)
        .unwrap_or_else(|error| panic!("the package is admitted: {error}"))
}

// Offline `observe`'s projection of the entry page, built from the package's observation
// resources as `run_observe` builds it.
fn observe_entry_page(with_icon: bool) -> PageProjection {
    let bundle = bundle();
    let view = crate::contained_resources::observation_resources(&bundle)
        .unwrap_or_else(|error| panic!("observe admits the package: {}", error.message));
    let (evaluator, detector) = crate::contained_resources::recognition_pipeline(&bundle)
        .unwrap_or_else(|error| panic!("recognition pipeline: {}", error.message));
    let scene = entry_frame(with_icon);
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
    projection
}

// The projection entry of an action: an element, or a missing one when it is not recognised.
fn entry<'a>(projection: &'a PageProjection, resource_id: &str) -> &'a Value {
    projection
        .elements
        .iter()
        .chain(&projection.missing)
        .find(|entry| entry["resource_id"] == resource_id)
        .unwrap_or_else(|| panic!("no {resource_id} entry in {projection:?}"))
}

fn tap(x: u32, y: u32, width: u32, height: u32, point: (u32, u32)) -> Value {
    json!({"type": "tap", "rect": {"x": x, "y": y, "width": width, "height": height},
           "point": {"x": point.0, "y": point.1}})
}

// The reader reads a segmented drag with its boxes and brake distance; a graph holding one as an
// edge to `page_a` and as a destructive page operation is read, and no route passes the edge,
// while the plain drag to `page_b` stays a route.
#[test]
fn l2_actinglab_reads_a_segmented_drag_and_no_route_passes_it() {
    let NavigationClick::SegmentedDrag(drag) =
        parse_navigation_click(&segmented_drag()).expect("a segmented drag")
    else {
        panic!("not read as a segmented drag");
    };
    assert_eq!(drag.from_rect, rect(240, 200, 40, 20));
    assert_eq!(drag.corner_rect, rect(40, 180, 40, 20));
    assert_eq!(drag.brake_distance_px, 100);

    let graph = parse_navigation_graph_value(&navigation())
        .unwrap_or_else(|error| panic!("a graph with segmented drags: {}", error.message));
    assert!(
        find_navigation_route(&graph.edges, "neutral/entry_page", "neutral/page_a").is_none(),
        "no route passes a segmented drag"
    );
    let route = find_navigation_route(&graph.edges, "neutral/entry_page", "neutral/page_b")
        .expect("the plain drag's route");
    assert_eq!(route.len(), 1);
    assert_eq!(route[0].id, "swipe_to_page_b");
}

// A segmented drag of another shape than the canonical click is refused: the task file's member
// names, a rectangle with a member it does not take, a missing duration, a brake distance that
// is not an integer.
#[test]
fn l2_actinglab_refuses_a_segmented_drag_of_another_shape() {
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
        assert_eq!(error.code, SharedCode::ValidationFailed.as_str(), "{case}");
        let mut navigation = navigation();
        navigation["navigation"][0]["click"] = click;
        assert!(
            parse_navigation_graph_value(&navigation).is_err(),
            "{case}: a graph holding it is refused"
        );
    }
}

// A package whose navigation holds segmented drags to a page, to a page set and as a page
// operation is admitted by offline `observe` and by the navigation `do` and `ensure` read; each
// is listed from the page declaration, not actionable, without an input, blocked by
// `gesture_not_projectable`; the plain drag of the same page keeps its input.
#[test]
fn l2_offline_tools_admit_segmented_drags_and_observe_lists_them_not_actionable() {
    let graph = crate::contained_resources::navigation_graph(&bundle())
        .unwrap_or_else(|error| panic!("do and ensure admit the package: {}", error.message));
    assert!(
        find_navigation_route(&graph.edges, "neutral/entry_page", "neutral/page_a").is_none(),
        "no route passes a segmented drag"
    );

    let projection = observe_entry_page(false);
    for resource_id in ["scroll_to_page_a", "scroll_to_either", "scroll_list"] {
        let element = entry(&projection, resource_id);
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
    let swipe = entry(&projection, "swipe_to_page_b");
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

// A13: an operation whose `to` is a page set with the fixed point `1,0` is a navigation element
// of its page whose input is the tap of the 1x1 box at that point.
#[test]
fn a13_a_page_set_operation_with_a_point_click_is_an_element_with_its_tap() {
    let projection = observe_entry_page(false);
    let element = entry(&projection, "open_either_at_point");
    assert_eq!(element["role"], json!("navigate"), "{element}");
    assert_eq!(element["actionable"], json!(true), "{element}");
    assert_eq!(element["input"], tap(1, 0, 1, 1, (1, 0)), "{element}");
}

// A13: one with a box click projects its box, pressed at its centre.
#[test]
fn a13_a_page_set_operation_with_a_box_click_projects_its_box() {
    let projection = observe_entry_page(false);
    let element = entry(&projection, "open_either_in_box");
    assert_eq!(element["actionable"], json!(true), "{element}");
    assert_eq!(
        element["input"],
        tap(200, 40, 60, 30, (230, 55)),
        "{element}"
    );
}

// A13: one with a target click is resolved on the frame: where the icon is at (40, 24) its input
// is the tap of the box the template matched; where it is not, `target_not_matched` with no
// input.
#[test]
fn a13_a_page_set_operation_with_a_target_click_is_resolved_on_the_frame() {
    let projection = observe_entry_page(true);
    let element = entry(&projection, "open_either_on_icon");
    assert_eq!(element["target_id"], json!("ui/icon"), "{element}");
    assert_eq!(
        element["recognition_basis"],
        json!("target_evaluation"),
        "{element}"
    );
    assert_eq!(element["actionable"], json!(true), "{element}");
    assert_eq!(
        element["input"],
        tap(ICON_X, ICON_Y, ICON, ICON, (ICON_X + 8, ICON_Y + 8)),
        "{element}"
    );

    let projection = observe_entry_page(false);
    let element = entry(&projection, "open_either_on_icon");
    assert_eq!(element["actionable"], json!(false), "{element}");
    assert_eq!(element["input"], Value::Null, "{element}");
    assert_eq!(
        element["blocked_reason"],
        json!("target_not_matched"),
        "{element}"
    );
}
