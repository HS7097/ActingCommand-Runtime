// SPDX-License-Identifier: AGPL-3.0-only

// #411 A13 B part (slice BT-LAB, plan §2.16, test row BT-L13 T1; exec key 117; register §1.147
// L13-1): offline `observe` lists each operation of the observed page whose `to` is a page set in
// `actions` with `input` next to its declared `click`, built from its generated navigation row as
// an edge's `input` is (`null` for a segmented drag, as an edge's), with no coordinate resolved on
// the frame. Every click below is in the generated navigation twice, on an edge to one page and on
// a page-set row, in the form source compilation writes there; the task file declares the
// page-set operation in its compiled form. The package and its frame are synthetic and neutral,
// and no Runtime or device is involved.

use super::*;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const ENTRY: [u8; 3] = [40, 90, 160];
const PAGE_A: [u8; 3] = [200, 60, 60];
const PAGE_B: [u8; 3] = [60, 200, 60];
const ENTRY_PAGE: &str = "neutral/entry_page";

/// One click, declared on the edge `edge` to `page_a` and on the page-set operation `page_set`.
struct Case {
    what: &'static str,
    edge: &'static str,
    page_set: &'static str,
    /// The click as the generated navigation holds it, on both rows.
    navigation_click: Value,
    /// The click as the task file declares the page-set operation.
    task_click: Value,
    /// An edge's `input` for the click: `semantic_input_json`, or `null` for a segmented drag.
    input: Value,
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> Value {
    json!({"x": x, "y": y, "width": width, "height": height})
}

fn point(x: i32, y: i32) -> Value {
    json!({"x": x, "y": y})
}

fn segmented_drag() -> Value {
    json!({
        "kind": "single_touch_drag_with_vertical_brake_v1",
        "from_rect": rect(240, 200, 40, 20),
        "corner_rect": rect(40, 180, 40, 20),
        "horizontal_duration_ms": 200,
        "corner_hold_ms": 150,
        "brake_distance_px": 100,
        "brake_duration_ms": 200
    })
}

fn cases() -> Vec<Case> {
    let box_click = json!({"kind": "rect", "x": 200, "y": 40, "width": 60, "height": 30});
    vec![
        Case {
            what: "a box",
            edge: "tap_to_page_a",
            page_set: "tap_to_either",
            navigation_click: box_click.clone(),
            task_click: box_click,
            input: json!({"type": "tap", "rect": rect(200, 40, 60, 30), "point": point(230, 55)}),
        },
        Case {
            what: "a point",
            edge: "point_to_page_a",
            page_set: "point_to_either",
            navigation_click: json!({"kind": "point", "point": "1,0"}),
            task_click: json!({"kind": "point", "x": 1, "y": 0}),
            input: json!({"type": "tap", "rect": rect(1, 0, 1, 1), "point": point(1, 0)}),
        },
        Case {
            what: "a drag",
            edge: "swipe_to_page_a",
            page_set: "swipe_to_either",
            navigation_click: json!({"kind": "drag", "from": rect(240, 120, 40, 20),
                                     "to": rect(40, 120, 40, 20), "duration_ms": 400}),
            task_click: json!({"kind": "drag", "from_rect": rect(240, 120, 40, 20),
                               "to_rect": rect(40, 120, 40, 20), "duration_ms": 400}),
            input: json!({"type": "drag", "from_rect": rect(240, 120, 40, 20),
                          "to_rect": rect(40, 120, 40, 20), "from": point(260, 130),
                          "to": point(60, 130), "duration_ms": 400}),
        },
        Case {
            what: "a segmented drag",
            edge: "scroll_to_page_a",
            page_set: "scroll_to_either",
            navigation_click: segmented_drag(),
            task_click: segmented_drag(),
            input: Value::Null,
        },
    ]
}

/// A context with no configuration, no recording session and an empty Runtime state root, and a
/// directory for the package and its frame. The fields drop in order, the environment lock last.
struct Isolated {
    files: TempDir,
    _runtime_env: RuntimeStateEnvGuard,
    _runtime: TempDir,
    _app_env: TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Isolated {
    fn new() -> Self {
        let guard = env_lock();
        let app_env = set_isolated_app_env();
        set_missing_config_env();
        unsafe {
            env::remove_var(SESSION_STATE_ENV);
        }
        let runtime = TempDir::new().unwrap();
        let runtime_env = use_runtime_state_root(runtime.path());
        Self {
            _guard: guard,
            _app_env: app_env,
            _runtime_env: runtime_env,
            _runtime: runtime,
            files: TempDir::new().unwrap(),
        }
    }

    /// The entry page: its colour everywhere.
    fn entry_scene(&self) -> String {
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
        let path = self.files.path().join("entry.png");
        fs::write(&path, png).unwrap();
        path.display().to_string()
    }

    /// The package: its pages told apart by the colour of the frame's top-left pixel; each case's
    /// edge and page-set row in the generated navigation, and its page-set operation in the task
    /// file.
    fn package(&self, cases: &[Case]) -> String {
        let page_target = |page: &str, expected: [u8; 3]| {
            json!({"type": "color", "id": format!("page/{page}"),
                   "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
        };
        let mut navigation = Vec::new();
        let mut operations = Vec::new();
        for case in cases {
            navigation.push(json!({"id": case.edge, "from_page": ENTRY_PAGE,
                                   "to_page": "neutral/page_a",
                                   "click": case.navigation_click, "source": ""}));
            navigation.push(json!({"id": case.page_set, "from_page": ENTRY_PAGE,
                                   "to_pages": ["neutral/page_a", "neutral/page_b"],
                                   "click": case.navigation_click, "source": ""}));
            operations.push(json!({"id": case.page_set, "from": "entry_page",
                                   "to": ["page_a", "page_b"], "click": case.task_click}));
        }
        let bytes = zip_entries(&[
            (
                "control.json",
                json!({"game": "neutral", "server": "test", "package_id": "neutral.bt_l13",
                       "entry_task_id": "task"}),
            ),
            ("resources/manifest.json", json!({"entry_task_id": "task"})),
            (
                "resources/operations/task/task.json",
                json!({"task_id": "task", "game": "neutral", "operations": operations}),
            ),
            (
                "resources/recognition/neutral.test.pack.json",
                json!({
                    "schema_version": "0.7",
                    "game": "neutral",
                    "server": "test",
                    "coordinate_space": {"width": WIDTH, "height": HEIGHT},
                    "defaults": {"color_max_distance": 0.0},
                    "targets": [
                        page_target("entry_page", ENTRY),
                        page_target("page_a", PAGE_A),
                        page_target("page_b", PAGE_B)
                    ]
                }),
            ),
            (
                "resources/recognition/neutral.test.pages.json",
                json!({"schema_version": "0.6", "pages": [
                    {"id": ENTRY_PAGE, "required": ["page/entry_page"]},
                    {"id": "neutral/page_a", "required": ["page/page_a"]},
                    {"id": "neutral/page_b", "required": ["page/page_b"]}
                ]}),
            ),
            (
                "resources/navigation/neutral.test.navigation.json",
                json!({"schema_version": "0.6", "game": "neutral", "server": "test",
                       "navigation": navigation, "destructive_actions": []}),
            ),
        ]);
        let path = self.files.path().join("bt_l13.zip");
        fs::write(&path, bytes).unwrap();
        path.display().to_string()
    }
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

// T1: offline `observe` of the entry page lists each page-set operation in `actions` with `input`
// next to its declared `click`, equal to the `input` of the edge with the same click: a box and a
// point are a tap of the box at its centre, a drag its two boxes and their centres, a segmented
// drag `null`.
#[test]
fn a13b_offline_observe_gives_a_page_set_action_the_input_of_an_edge_with_the_same_click() {
    let context = Isolated::new();
    let cases = cases();
    let package = context.package(&cases);
    let scene = context.entry_scene();
    let result = run_cli(
        [
            "--json",
            "observe",
            "--zip",
            package.as_str(),
            "--scene",
            scene.as_str(),
            "--verbose",
        ],
        true,
    );
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 0, "{shown}");
    let data = result
        .envelope
        .data
        .clone()
        .unwrap_or_else(|| panic!("no data: {shown}"));
    assert_eq!(data["page"], ENTRY_PAGE, "{shown}");
    let actions = data["actions"]
        .as_array()
        .unwrap_or_else(|| panic!("no actions: {shown}"));
    let action = |id: &str| {
        actions
            .iter()
            .find(|action| action["id"] == id)
            .unwrap_or_else(|| panic!("no action {id} in {actions:?}"))
    };
    for case in &cases {
        let what = case.what;
        let page_set = action(case.page_set);
        let edge = action(case.edge);
        assert!(
            page_set.get("input").is_some(),
            "{what}: the page-set action carries input: {page_set}"
        );
        assert_eq!(
            page_set["input"], edge["input"],
            "{what}: the page-set action's input is the edge's: {page_set} / {edge}"
        );
        assert_eq!(
            page_set["input"], case.input,
            "{what}: the input of the click: {page_set}"
        );
        assert_eq!(
            page_set["click"], case.task_click,
            "{what}: the declared click stays next to it: {page_set}"
        );
    }
}
