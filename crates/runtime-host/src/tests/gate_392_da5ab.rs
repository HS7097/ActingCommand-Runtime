// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-5ab (`contracts/display-adapter.md` section 11; the coordinator's work order of
//! 11-16 on board #392, test "presses"): on a display that maps to its package's base, a Lab
//! operation takes its coordinates in the base, observes on the frame brought to the base, and
//! sends its input to the device through the run's own exit.
//!
//! - On a 1920x1080 device a 1280x720 package's base point (640, 360) leaves as (960, 540), the base
//!   column 1279 as 1919, and a swipe from (0, 0) to (1279, 719) as one from (0, 0) to
//!   (1919, 1079), each with its duration; the records keep the base inputs, and the operation's
//!   projections are of the base's size.
//! - On a 1600x900 device (s = 5/4, which the capture side admits and the input side does not) the
//!   operation observes its page and fails at its input with `display_map_unresolved`
//!   (`input_scale_unsupported` at `display_input_map`, the frame 1600x900 and the base 1280x720),
//!   and nothing is sent.
//!
//! Fix round 1 (the coordinator's work order of 11-16 after the review of pull request #1780,
//! point 5), on the same devices:
//! - at 3/2 an element, the navigation edge of `page_a` drawn in the box (101, 50, 21, 11), is
//!   pressed at its projection's point (111, 55), in the base, and the device receives (167, 83),
//!   inside the box's inward device box (152, 75) to (183, 91), exclusive;
//! - at 3/2 a declared operation whose click is that box draws its point in it from a seed, and the
//!   device receives that point at `round_half_up(v · 3/2)`, inside the same device box;
//! - at 5/4 a declared operation of the 1280x720 package recognises its page on the reduced frame
//!   and fails at its input with `display_map_unresolved` (`input_scale_unsupported` at
//!   `display_input_map`, the frame 1600x900 and the base 1280x720); its attempt keeps the base tap
//!   and never reached its input, and nothing is sent.
//!
//! The device's frames show `page_a` (before any input) or `page_b` (after one) in their 3x3
//! top-left block, so the base frame's first pixel keeps the page's colour. The package is
//! #412 Lab B-3 S2's synthetic and neutral one; the element's is of the same pages.

use super::gate_392_da2a::{number, text};
use super::gate_412_b3_s2 as lab;
use super::*;
use actingcommand_contract::page_projection::{
    ElementRole, Geometry, Point as ProjectionPoint, Rect as ProjectionRect,
};
use actingcommand_contract::{
    ContainedLabOperationRequest, LabOperationEnd, LabOperationSelection, LabOperationStage,
    LabProjectionHint, ObservationFramePlan,
};
use actingcommand_runtime_client::{RuntimeClient, VerifiedLabOperation};
use serde_json::{Value, json};

/// Fix round 1: the box the element and the declared operation press in (base pixels), and the
/// inward device box of it at 3/2: left and top `ceil(v · 3/2)`, right and bottom `floor(v · 3/2)`,
/// exclusive.
const BOX: (u32, u32, u32, u32) = (101, 50, 21, 11);
const DEVICE_BOX_X: std::ops::Range<i32> = 152..183;
const DEVICE_BOX_Y: std::ops::Range<i32> = 75..91;
/// Fix round 1: the seed the declared operation's draw takes.
const SEED: u64 = 23;

/// `value · 3/2`, halves up, as the display adapter maps a base point at 3/2.
fn three_halves(value: i32) -> i32 {
    (value * 3 + 1).div_euclid(2)
}

/// A Lab host whose one instance's frames and display are `width`x`height`.
fn device_host(root: &TempDir, (width, height): (u32, u32)) -> (RuntimeHost, Arc<FakeState>) {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((width, height));
    let row = width as usize;
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        move |_capture: usize, inputs: usize| -> Vec<(usize, [u8; 3])> {
            let page = if inputs > 0 { lab::PAGE_B } else { lab::PAGE_A };
            (0..3)
                .flat_map(move |line| (0..3).map(move |column| (line * row + column, page)))
                .collect()
        },
    ));
    let host = lab::start(
        root,
        FakeProvider::one(lab::ALIAS, instance_id(), Arc::clone(&state)),
    );
    (host, state)
}

/// The 1280x720 package of #412 Lab B-3 S2, whose declared operation `op_a` taps (1, 0) from
/// `page_a` to `page_b` (run only at 5/4, fix round 1).
fn package(root: &TempDir) -> PathBuf {
    lab::write_package(
        root,
        (1280, 720),
        json!({
            "id": lab::OPERATION,
            "from": "page_a",
            "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }),
    )
}

/// One Lab operation pressing `action`, given in the package's base, verified by the Lab client.
fn press(client: &RuntimeClient, package: &Path, action: InputAction) -> VerifiedLabOperation {
    client
        .begin_debug_session()
        .expect("debug session")
        .run_contained_lab_operation(
            lab::ALIAS,
            ContainedLabOperationRequest {
                package_path: package.to_str().expect("path").to_owned(),
                selection: LabOperationSelection::Coordinates { action },
                projection_hint: LabProjectionHint::default(),
                after: None,
            },
            ObservationFramePlan::SINGLE,
        )
        .expect("the client verifies the operation")
}

#[test]
fn da5ab_a_lab_press_on_a_device_three_halves_the_base_leaves_at_its_device_point() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = device_host(&root, (1920, 1080));
    let package = package(&root);
    let client = lab::lab_client(&root);
    let presses = [
        (
            InputAction::Tap { x: 640, y: 360 },
            InputAction::Tap { x: 960, y: 540 },
        ),
        (
            InputAction::Tap { x: 1279, y: 0 },
            InputAction::Tap { x: 1919, y: 0 },
        ),
        (
            InputAction::Swipe {
                x1: 0,
                y1: 0,
                x2: 1279,
                y2: 719,
                duration_ms: 300,
            },
            InputAction::Swipe {
                x1: 0,
                y1: 0,
                x2: 1919,
                y2: 1079,
                duration_ms: 300,
            },
        ),
    ];
    for (base, _) in &presses {
        let verified = press(&client, &package, base.clone());
        let record = &verified.operation().record;
        assert!(record.failure.is_none(), "{base:?}: {:?}", record.failure);
        assert_eq!(
            record.prepared.action.as_ref(),
            Some(base),
            "the base input"
        );
        for (frame, projection) in [
            (
                &record.prepared.before_frame,
                &record.prepared.before_projection,
            ),
            (&record.after_frame, &record.after_projection),
        ] {
            let frame = frame.as_ref().expect("the operation's frame");
            let projection = projection.as_ref().expect("the frame's projection");
            assert_eq!(
                (frame.observation.width(), frame.observation.height()),
                (1920, 1080),
                "the device frame"
            );
            assert_eq!(
                (
                    projection.projection.frame.width,
                    projection.projection.frame.height
                ),
                (1280, 720),
                "the projection on the base"
            );
        }
    }
    drop(client);
    host.close().expect("close host");
    assert_eq!(
        *state.input_actions.lock().expect("fake input actions"),
        presses
            .iter()
            .map(|(_, device)| device.clone())
            .collect::<Vec<_>>(),
        "the device received each base input at its device point"
    );
}

#[test]
fn da5ab_a_lab_press_on_a_device_five_quarters_the_base_sends_nothing() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = device_host(&root, (1600, 900));
    let package = package(&root);
    let client = lab::lab_client(&root);
    let verified = press(&client, &package, InputAction::Tap { x: 640, y: 360 });
    let record = &verified.operation().record;
    let projection = record
        .prepared
        .before_projection
        .as_ref()
        .expect("the page was observed");
    assert_eq!(projection.projection.page, "neutral/page_a");
    let failure = record.failure.as_ref().expect("the operation failed");
    assert_eq!(failure.code, "display_map_unresolved", "{failure:?}");
    assert_eq!(failure.stage, LabOperationStage::Input, "{failure:?}");
    assert!(!record.input_returned, "the input never reached its input");
    assert_eq!(record.effect, EffectDisposition::NotPerformed);
    let outcome = verified.receipt().outcome();
    assert_eq!(outcome.code().as_str(), "display_map_unresolved");
    assert_eq!(
        text(outcome, "display_problem"),
        Some("input_scale_unsupported")
    );
    assert_eq!(text(outcome, "stage"), Some("display_input_map"));
    assert_eq!(number(outcome, "frame_width"), Some(1600));
    assert_eq!(number(outcome, "frame_height"), Some(900));
    assert_eq!(number(outcome, "base_width"), Some(1280));
    assert_eq!(number(outcome, "base_height"), Some(720));
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.input_count.load(Ordering::Acquire), 0, "no input");
    assert!(
        state
            .input_actions
            .lock()
            .expect("fake input actions")
            .is_empty()
    );
}

/// Fix round 1: a 1280x720 observation package of the pages `neutral/page_a` and `neutral/page_b`
/// (their colour on the first pixel, as #412 Lab B-3 S2's), whose one navigation edge `a_to_b`,
/// the element of `page_a`, is pressed in [`BOX`].
fn element_package(root: &TempDir) -> PathBuf {
    let page_target = |page: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": format!("page/{page}"),
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let entries: [(&str, Value); 6] = [
        (
            "control.json",
            json!({"game": "neutral", "server": "test", "package_id": "neutral.da5ab_element",
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
                "coordinate_space": {"width": 1280, "height": 720},
                "defaults": {"color_max_distance": 0.0},
                "targets": [
                    page_target("page_a", lab::PAGE_A),
                    page_target("page_b", lab::PAGE_B)
                ]
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version": "0.6", "pages": [
                {"id": "neutral/page_a", "required": ["page/page_a"]},
                {"id": "neutral/page_b", "required": ["page/page_b"]}
            ]}),
        ),
        (
            "resources/navigation/neutral.test.navigation.json",
            json!({
                "schema_version": "0.6",
                "game": "neutral",
                "server": "test",
                "navigation": [{
                    "id": "a_to_b", "from_page": "neutral/page_a", "to_page": "neutral/page_b",
                    "click": {"kind": "rect", "x": BOX.0, "y": BOX.1,
                              "width": BOX.2, "height": BOX.3},
                    "source": "neutral/spec"
                }]
            }),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in &entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(value).expect("JSON"))
            .expect("zip bytes");
    }
    let path = root.path().join("da5ab-element.zip");
    fs::write(&path, zip.finish().expect("finish zip").into_inner()).expect("write package");
    path
}

#[test]
fn da5ab_fix1_an_element_at_three_halves_leaves_inside_its_device_box() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = device_host(&root, (1920, 1080));
    let package = element_package(&root);
    let client = lab::lab_client(&root);
    let element = serde_json::to_string(&(ElementRole::Navigate, "", "neutral/spec", "a_to_b"))
        .expect("element ID");
    let verified = client
        .begin_debug_session()
        .expect("debug session")
        .run_contained_lab_operation(
            lab::ALIAS,
            ContainedLabOperationRequest {
                package_path: package.to_str().expect("path").to_owned(),
                selection: LabOperationSelection::Element { id: element },
                projection_hint: LabProjectionHint::default(),
                after: None,
            },
            ObservationFramePlan::SINGLE,
        )
        .expect("the client verifies the operation");
    let record = &verified.operation().record;
    assert!(record.failure.is_none(), "{:?}", record.failure);
    // The element's rectangle and point, and the input, in the base.
    assert_eq!(
        record.prepared.geometry,
        Some(Geometry::Tap {
            rect: ProjectionRect {
                x: BOX.0,
                y: BOX.1,
                width: BOX.2,
                height: BOX.3,
            },
            point: ProjectionPoint { x: 111, y: 55 },
        })
    );
    assert_eq!(
        record.prepared.action,
        Some(InputAction::Tap { x: 111, y: 55 }),
        "the base input"
    );
    let before = record
        .prepared
        .before_projection
        .as_ref()
        .expect("the before projection");
    assert_eq!(before.projection.page, "neutral/page_a");
    assert_eq!(
        (
            before.projection.frame.width,
            before.projection.frame.height
        ),
        (1280, 720),
        "the projection on the base"
    );
    drop(client);
    host.close().expect("close host");
    let device = (three_halves(111), three_halves(55));
    assert_eq!(device, (167, 83));
    assert!(
        DEVICE_BOX_X.contains(&device.0) && DEVICE_BOX_Y.contains(&device.1),
        "{device:?} inside the element's device box"
    );
    assert_eq!(
        *state.input_actions.lock().expect("fake input actions"),
        [InputAction::Tap {
            x: device.0,
            y: device.1
        }],
        "the device received the element's point at 3/2"
    );
}

#[test]
fn da5ab_fix1_a_declared_operations_drawn_box_at_three_halves_leaves_inside_its_device_box() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = device_host(&root, (1920, 1080));
    let package = lab::write_package(
        &root,
        (1280, 720),
        json!({
            "id": lab::OPERATION,
            "from": "page_a",
            "to": "page_b",
            "click": {"kind": "rect", "x": BOX.0, "y": BOX.1, "width": BOX.2, "height": BOX.3},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }),
    );
    let client = lab::lab_client(&root);
    let verified = lab::run_operation(&client, &package, Some(SEED));
    let record = &verified.operation().record;
    assert!(record.failure.is_none(), "{:?}", record.failure);
    let result = record
        .operation_result
        .as_ref()
        .expect("the declared operation's result");
    assert_eq!(result.end, LabOperationEnd::Confirmed);
    let [attempt] = result.attempts.as_slice() else {
        panic!("one attempt, got {:?}", result.attempts);
    };
    let sampling = attempt.sampling.as_ref().expect("the press's draw");
    assert_eq!(
        sampling
            .source_regions()
            .iter()
            .map(|region| (
                i64::from(region.x()),
                i64::from(region.y()),
                i64::from(region.width()),
                i64::from(region.height())
            ))
            .collect::<Vec<_>>(),
        [(101, 50, 21, 11)],
        "the box the point was drawn in, in the base"
    );
    let &InputAction::Tap { x, y } = &attempt.action else {
        panic!("a tap, got {:?}", attempt.action);
    };
    assert!(
        (101..122).contains(&x) && (50..61).contains(&y),
        "the drawn point ({x}, {y}) in the base box"
    );
    drop(client);
    host.close().expect("close host");
    let device = (three_halves(x), three_halves(y));
    assert!(
        DEVICE_BOX_X.contains(&device.0) && DEVICE_BOX_Y.contains(&device.1),
        "{device:?} inside the box's device box"
    );
    assert_eq!(
        *state.input_actions.lock().expect("fake input actions"),
        [InputAction::Tap {
            x: device.0,
            y: device.1
        }],
        "the device received the drawn point at 3/2"
    );
}

#[test]
fn da5ab_fix1_a_declared_operation_on_a_device_five_quarters_the_base_sends_nothing() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = device_host(&root, (1600, 900));
    let package = package(&root);
    let client = lab::lab_client(&root);
    let verified = lab::run_operation(&client, &package, None);
    let record = &verified.operation().record;
    let failure = record.failure.as_ref().expect("the operation failed");
    assert_eq!(failure.code, "display_map_unresolved", "{failure:?}");
    assert_eq!(failure.stage, LabOperationStage::Input, "{failure:?}");
    let result = record
        .operation_result
        .as_ref()
        .expect("the declared operation's result");
    assert_eq!(result.end, LabOperationEnd::Failed);
    let [attempt] = result.attempts.as_slice() else {
        panic!("one attempt, got {:?}", result.attempts);
    };
    assert_eq!(
        attempt.action,
        InputAction::Tap { x: 1, y: 0 },
        "the attempt keeps the base tap"
    );
    assert!(!attempt.input_returned, "the tap never reached its input");
    let outcome = verified.receipt().outcome();
    assert_eq!(outcome.code().as_str(), "display_map_unresolved");
    assert_eq!(
        text(outcome, "display_problem"),
        Some("input_scale_unsupported")
    );
    assert_eq!(text(outcome, "stage"), Some("display_input_map"));
    assert_eq!(number(outcome, "frame_width"), Some(1600));
    assert_eq!(number(outcome, "frame_height"), Some(900));
    assert_eq!(number(outcome, "base_width"), Some(1280));
    assert_eq!(number(outcome, "base_height"), Some(720));
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.input_count.load(Ordering::Acquire), 0, "no input");
    assert!(
        state
            .input_actions
            .lock()
            .expect("fake input actions")
            .is_empty()
    );
}
