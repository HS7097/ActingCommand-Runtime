// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 / #401, slice DA-3 in the host (model 392 v1.2 §6, §13; work order tests;
//! `contracts/display-adapter.md` section 10): every input of a run leaves for the device at one
//! place, mapped by the run's display map. On a device two and three times the package's size a
//! tap drawn in a box, a swipe and a path drag reach the fake device at their base points times the
//! scale, their durations kept; the effect intent keeps the base input and its base boxes, the
//! input intent records the device input. On a device of the package's size (the identity) the
//! records and the inputs the device receives are exactly the base ones, as before this slice.
//!
//! A synthetic `neutral` game, server `test`, with a 16x9 package. The device's frames show the
//! page in their 3x3 top-left block: `home` before any input, `second` after one, `third` after
//! two, `terminal` after three, so the scale frame's first pixel keeps the page's colour.

use super::gate_392_da2a::{number, text};
use super::*;
use actingcommand_contract::{
    CaptureExtent, DisplayMapKind, InputPathPoint, LabOperationEnd, LabOperationStage,
    TaskGeometryObservation, TaskGeometryPhase,
};
use actingcommand_device::{PathDragAction, SegmentedDragAction};
use actingcommand_ledger_forensics::{
    ForensicEventFilter, ForensicEventsRequest, ForensicOutput, ForensicReport, ForensicRequest,
    MAX_FORENSIC_EVENTS,
};
use serde_json::json;

const ALIAS: &str = "neutral.instance";
const GAME: &str = "neutral";
const HOME: [u8; 3] = [255, 0, 0];
const SECOND: [u8; 3] = [0, 255, 0];
const THIRD: [u8; 3] = [255, 0, 255];
const TERMINAL: [u8; 3] = [0, 0, 255];

/// A host whose one instance is an emulator's endpoint with frames and display of
/// `width`x`height` (rotation `r0`); every frame shows the page of its input count in its 3x3
/// top-left block.
fn device_host(temp: &TempDir, (width, height): (u32, u32)) -> (RuntimeHost, Arc<FakeState>) {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((width, height));
    let row = width as usize;
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        move |_capture: usize, inputs: usize| -> Vec<(usize, [u8; 3])> {
            let page = match inputs {
                0 => HOME,
                1 => SECOND,
                2 => THIRD,
                _ => TERMINAL,
            };
            (0..3)
                .flat_map(move |line| (0..3).map(move |column| (line * row + column, page)))
                .collect()
        },
    ));
    let registered = instance_id();
    let resolved = actingcommand_execution_kernel::ResolvedExecutionInstance::new(
        registered,
        "127.0.0.1:16384",
    )
    .with_adb_endpoint(actingcommand_execution_kernel::ResolvedAdbEndpoint::new(
        "127.0.0.1",
        16384,
        false,
    ));
    let provider = FakeProvider::one(ALIAS, registered, Arc::clone(&state))
        .with_resolved_override(Arc::new(std::sync::Mutex::new(resolved)));
    let host = RuntimeHost::start(config(temp), Arc::new(provider))
        .expect("runtime host with a device instance");
    (host, state)
}

/// The 16x9 route package: from `home` a tap drawn in the box (4, 2, 4, 3), from `second` a swipe
/// from the box (1, 5, 3, 3) to the box (10, 4, 4, 3) over 300 ms, from `third` a path drag in
/// the box (2, 2, 12, 6) with two turns, segments of at least 3, 600 ms and a hold of 40 ms.
fn three_inputs_package() -> Vec<u8> {
    let space = json!({"width": 16, "height": 9});
    let control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.test.task",
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": "test",
        "resolution": space,
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 20_000,
        "timeout_ms": 40_000,
        "max_steps": 4
    });
    let task = json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": GAME,
        "server_scope": ["test"],
        "coordinate_space": space,
        "entry_page": "home",
        "target_page": "terminal",
        "operations": [
            {
                "id": "press",
                "from": "home",
                "to": "second",
                "click": {"kind": "rect", "x": 4, "y": 2, "width": 4, "height": 3},
                "unguarded_trusted_coordinate": true,
                "retryable": false
            },
            {
                "id": "swipe",
                "from": "second",
                "to": "third",
                "click": {
                    "kind": "drag",
                    "from_rect": {"x": 1, "y": 5, "width": 3, "height": 3},
                    "to_rect": {"x": 10, "y": 4, "width": 4, "height": 3},
                    "duration_ms": 300
                },
                "unguarded_trusted_coordinate": true,
                "retryable": false
            },
            {
                "id": "trace",
                "from": "third",
                "to": "terminal",
                "click": {
                    "kind": "single_touch_path_drag_v1",
                    "box_rect": {"x": 2, "y": 2, "width": 12, "height": 6},
                    "turns": 2,
                    "min_segment_px": 3,
                    "duration_ms": 600,
                    "corner_hold_ms": 40
                },
                "unguarded_trusted_coordinate": true,
                "retryable": false
            }
        ]
    });
    let target = |id: &str, colour: [u8; 3]| json!({"type": "color", "id": id, "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": colour});
    let recognition = json!({
        "schema_version": "0.3",
        "game": GAME,
        "server": "test",
        "coordinate_space": space,
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            target("page/home", HOME),
            target("page/second", SECOND),
            target("page/third", THIRD),
            target("page/terminal", TERMINAL)
        ]
    });
    let page = |id: &str, target: &str| json!({"id": id, "required": [target], "optional": [], "forbidden": []});
    let pages = json!({
        "schema_version": "0.3",
        "pages": [
            page("neutral/home", "page/home"),
            page("neutral/second", "page/second"),
            page("neutral/third", "page/third"),
            page("neutral/terminal", "page/terminal")
        ]
    });
    let manifest = json!({"schema_version": "0.3", "entry_task_id": "task"});
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, contents) in [
        ("control.json", control),
        ("resources/manifest.json", manifest),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", recognition),
        ("resources/recognition/neutral.test.pages.json", pages),
    ] {
        zip.start_file(path, options).expect("package entry");
        zip.write_all(&serde_json::to_vec(&contents).expect("package JSON"))
            .expect("package content");
    }
    zip.finish().expect("finish package").into_inner()
}

/// One effect intent as the ledger holds it: the base input, its base boxes and the device
/// frame's size.
#[derive(Debug, PartialEq)]
pub(super) struct EffectRecord {
    pub(super) action: InputAction,
    pub(super) boxes: Option<Vec<(i32, i32, i32, i32)>>,
    pub(super) frame_extent: Option<CaptureExtent>,
}

/// What a run recorded: its receipt, its effect intents and the inputs its input intents name,
/// in order, and its task geometry records; the sequence of its first record.
pub(super) struct InputRun {
    pub(super) receipt: RuntimeReceipt,
    pub(super) effects: Vec<EffectRecord>,
    pub(super) inputs: Vec<InputAction>,
    pub(super) geometry: Vec<TaskGeometryObservation>,
    pub(super) first_sequence: u64,
}

/// The effect intents, the input intents' inputs and the geometry records of `events`.
pub(super) fn input_records(
    events: &[PersistedEvent],
) -> (
    Vec<EffectRecord>,
    Vec<InputAction>,
    Vec<TaskGeometryObservation>,
) {
    let mut effects = Vec::new();
    let mut inputs = Vec::new();
    let mut geometry = Vec::new();
    for event in events {
        match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::EffectIntent {
                    action,
                    frame_extent,
                    ..
                } => effects.push(EffectRecord {
                    action: action.clone(),
                    boxes: payload.sampling().map(|sampling| {
                        sampling
                            .source_regions()
                            .iter()
                            .map(|region| (region.x(), region.y(), region.width(), region.height()))
                            .collect()
                    }),
                    frame_extent: *frame_extent,
                }),
                TaskSemanticFact::GeometryObserved { observation } => {
                    geometry.push(observation.as_ref().clone());
                }
                _ => {}
            },
            EventPayload::Input(InputPayload::Intent(intent)) => inputs.push(
                intent
                    .provenance()
                    .expect("a run's input intent names its input")
                    .input_action
                    .clone(),
            ),
            _ => {}
        }
    }
    (effects, inputs, geometry)
}

/// Runs `package` (written as `<name>.zip`) on the instance and reads its records back.
fn run(host: &RuntimeHost, temp: &TempDir, name: &str, package: &[u8]) -> InputRun {
    let path = temp.path().join(format!("{name}.zip"));
    fs::write(&path, package).expect("write run package");
    let task_request = ContainedTaskRequest::new(path.display().to_string()).expect("task request");
    let mut client = TestClient::connect(host);
    client
        .stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .expect("receipt timeout");
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            task_request,
        ),
    );
    let receipt = client.send(&request);
    drop(client);
    let events = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        })
        .expect("run events");
    let (effects, inputs, geometry) = input_records(&events);
    let first_sequence = events
        .iter()
        .map(PersistedEvent::sequence)
        .min()
        .expect("the run's records");
    InputRun {
        receipt,
        effects,
        inputs,
        geometry,
        first_sequence,
    }
}

/// `action` at an integer scale `k`, by the rounding table's hand rule: `round_half_up(v · k)` is
/// `v · k`, and a box `(x, y, w, h)` inward is `(k·x, k·y, k·w, k·h)`; durations and the hold are
/// kept.
fn times(action: &InputAction, k: i32) -> InputAction {
    match action {
        InputAction::Tap { x, y } => InputAction::Tap { x: x * k, y: y * k },
        InputAction::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        } => InputAction::Swipe {
            x1: x1 * k,
            y1: y1 * k,
            x2: x2 * k,
            y2: y2 * k,
            duration_ms: *duration_ms,
        },
        InputAction::SingleTouchPathDragV1 {
            box_x,
            box_y,
            box_width,
            box_height,
            points,
            segment_duration_ms,
            corner_hold_ms,
        } => InputAction::SingleTouchPathDragV1 {
            box_x: box_x * k,
            box_y: box_y * k,
            box_width: box_width * k,
            box_height: box_height * k,
            points: points
                .iter()
                .map(|point| InputPathPoint {
                    x: point.x * k,
                    y: point.y * k,
                })
                .collect(),
            segment_duration_ms: segment_duration_ms.clone(),
            corner_hold_ms: *corner_hold_ms,
        },
        other => panic!("the package makes taps, swipes and path drags only, got {other:?}"),
    }
}

/// Work order tests (model 392 v1.2 §6.1, §6.2; DA3-T9's one exit): the 16x9 package on a 32x18
/// and a 48x27 device (two and three times its size, a pure scale of 2 and 3 without a
/// declaration) and, for the identity, on a 16x9 device. The run completes on its target page.
/// Its three effect intents keep the base inputs: a tap inside (4, 2, 4, 3), a swipe from inside
/// (1, 5, 3, 3) to inside (10, 4, 4, 3) over 300 ms, a path drag in (2, 2, 12, 6) of four points,
/// 600 ms over its segments and a hold of 40; their boxes are the base boxes and their frame size
/// the device's. The input intents and the device hold the base inputs times the scale: the tap
/// and the swipe as taps and swipes, the path drag as one plan through its points. The first
/// geometry record names the pure scale `k/1` (the identity names none).
#[test]
fn da3_a_tap_a_swipe_and_a_drag_reach_the_device_at_two_and_three_times_their_base_points() {
    for k in [1_u32, 2, 3] {
        let temp = TempDir::new().expect("tempdir");
        let (host, state) = device_host(&temp, (16 * k, 9 * k));
        let run = run(&host, &temp, "three-inputs", &three_inputs_package());
        host.close().expect("close runtime host");
        assert_eq!(
            run.receipt.state(),
            RuntimeReceiptState::Completed,
            "{k}: {:?}",
            run.receipt
        );
        assert!(
            matches!(
                run.receipt.result(),
                Some(RuntimeResult::ContainedTaskCompleted {
                    task_result: TaskOutcome::Success,
                    final_page: Some(page),
                    ..
                }) if page == "neutral/terminal"
            ),
            "{k}: {:?}",
            run.receipt
        );
        let [press, swipe, trace] = run.effects.as_slice() else {
            panic!("{k}: three effect intents, got {:?}", run.effects);
        };
        let device = CaptureExtent::new(16 * k, 9 * k).expect("the device frame");
        let InputAction::Tap { x, y } = press.action else {
            panic!("{k}: the first input is a tap, got {:?}", press.action);
        };
        assert!(
            (4..8).contains(&x) && (2..5).contains(&y),
            "{k}: the tap inside its base box"
        );
        assert_eq!(press.boxes, Some(vec![(4, 2, 4, 3)]), "{k}");
        assert_eq!(press.frame_extent, Some(device), "{k}");
        let InputAction::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        } = swipe.action
        else {
            panic!("{k}: the second input is a swipe, got {:?}", swipe.action);
        };
        assert!(
            (1..4).contains(&x1) && (5..8).contains(&y1),
            "{k}: the swipe's press inside its base box"
        );
        assert!(
            (10..14).contains(&x2) && (4..7).contains(&y2),
            "{k}: the swipe's end inside its base box"
        );
        assert_eq!(duration_ms, 300);
        assert_eq!(swipe.boxes, Some(vec![(1, 5, 3, 3), (10, 4, 4, 3)]), "{k}");
        assert_eq!(swipe.frame_extent, Some(device), "{k}");
        let InputAction::SingleTouchPathDragV1 {
            box_x,
            box_y,
            box_width,
            box_height,
            points,
            segment_duration_ms,
            corner_hold_ms,
        } = &trace.action
        else {
            panic!(
                "{k}: the third input is a path drag, got {:?}",
                trace.action
            );
        };
        assert_eq!((*box_x, *box_y, *box_width, *box_height), (2, 2, 12, 6));
        assert_eq!(points.len(), 4, "{k}: two turns make four points");
        assert_eq!(segment_duration_ms.iter().sum::<u64>(), 600);
        assert_eq!(*corner_hold_ms, 40);
        assert_eq!(trace.boxes, None, "{k}: a path drag records no sampling");
        assert_eq!(trace.frame_extent, Some(device), "{k}");

        let scale = i32::try_from(k).expect("a small scale");
        let expected: Vec<InputAction> = run
            .effects
            .iter()
            .map(|effect| times(&effect.action, scale))
            .collect();
        assert_eq!(
            run.inputs, expected,
            "{k}: the input intents name the device inputs"
        );
        assert_eq!(
            *state.input_actions.lock().expect("fake input actions"),
            expected[..2],
            "{k}: the tap and the swipe the device received"
        );
        let InputAction::SingleTouchPathDragV1 {
            points,
            segment_duration_ms,
            corner_hold_ms,
            ..
        } = &expected[2]
        else {
            panic!("{k}: a path drag");
        };
        let plans = state
            .segmented_swipe_plans
            .lock()
            .expect("fake segmented swipe plans")
            .clone();
        let [plan] = plans.as_slice() else {
            panic!("{k}: one performed plan, got {plans:?}");
        };
        assert_eq!(
            plan.action(),
            SegmentedDragAction::PathDrag(PathDragAction {
                points: points.iter().map(|point| (point.x, point.y)).collect(),
                segment_duration_ms: segment_duration_ms.clone(),
                corner_hold_ms: *corner_hold_ms,
            }),
            "{k}: the path drag the device performed"
        );
        let Some(first) = run.geometry.first() else {
            panic!("{k}: a geometry record");
        };
        assert_eq!(first.phase, TaskGeometryPhase::Initial);
        assert_eq!(
            first
                .display_map
                .map(|map| (map.kind, map.scale_num, map.scale_den, map.declared)),
            (k > 1).then_some((DisplayMapKind::Scale, k, 1, false)),
            "{k}"
        );
    }
}

/// Work order point 5 (model 392 v1.2 §0.2 C16, R10; the owner's ruling of 10-26, merge is
/// deploy): with the literals of the existing gates (DA-2a's 1280x720 package, whose one operation
/// taps the point (1, 0)), a run on a 1280x720 device records and sends exactly what it did before
/// this slice: the effect intent's input is the tap (1, 0) with no sampling and the frame size
/// 1280x720, the input intent's input is the same tap, the device received that one tap, and the
/// one geometry record is `initial` without a display map.
#[test]
fn da3_identity_a_1280x720_package_on_a_1280x720_device_records_and_sends_as_before() {
    let temp = TempDir::new().expect("tempdir");
    let (host, state) = device_host(&temp, (1280, 720));
    // DA-2a's package reads its page at (0, 0): `home` before the tap, `terminal` after it.
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        |_capture: usize, inputs: usize| -> Vec<(usize, [u8; 3])> {
            let page = if inputs > 0 { TERMINAL } else { HOME };
            vec![(0, page)]
        },
    ));
    let run = run(
        &host,
        &temp,
        "identity",
        &super::gate_392_da2a::package_at(1280, 720),
    );
    host.close().expect("close runtime host");
    assert_eq!(
        run.receipt.state(),
        RuntimeReceiptState::Completed,
        "{:?}",
        run.receipt
    );
    assert_eq!(
        run.effects,
        [EffectRecord {
            action: InputAction::Tap { x: 1, y: 0 },
            boxes: None,
            frame_extent: CaptureExtent::new(1280, 720),
        }]
    );
    assert_eq!(run.inputs, [InputAction::Tap { x: 1, y: 0 }]);
    assert_eq!(
        *state.input_actions.lock().expect("fake input actions"),
        [InputAction::Tap { x: 1, y: 0 }]
    );
    assert_eq!(
        run.geometry
            .iter()
            .map(|record| (record.phase, record.display_map))
            .collect::<Vec<_>>(),
        [(TaskGeometryPhase::Initial, None)]
    );
}

/// Fix 1, work order point 3 (`contracts/display-adapter.md` section 10): a pure scale whose
/// double is not a whole number is refused up front, whatever the point. DA-2a's package at
/// 16x12 on a 20x15 device (s = 5/4, which the capture side admits) recognises its home page; its
/// first input, the point (1, 0), which `round_half_up(1 · 5/4)` would put at (1, 0) inside the
/// frame, is not sent: the run ends `display_map_unresolved` with `input_scale_unsupported` at
/// `display_input_map`, the frame 20x15 and the base 16x12. The effect intent keeps the tap
/// (1, 0), no input intent is written, the device took no input, and the first geometry record
/// names the pure scale 5/4.
#[test]
fn da3_fix1_a_scale_whose_double_is_not_whole_refuses_the_first_input_up_front() {
    let temp = TempDir::new().expect("tempdir");
    let (host, state) = device_host(&temp, (20, 15));
    let run = run(
        &host,
        &temp,
        "five-quarters",
        &super::gate_392_da2a::package_at(16, 12),
    );
    host.close().expect("close runtime host");
    assert_eq!(
        run.receipt.state(),
        RuntimeReceiptState::Failed,
        "{:?}",
        run.receipt
    );
    let outcome = run.receipt.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "display_map_unresolved",
        "{outcome:?}"
    );
    assert_eq!(
        text(outcome, "display_problem"),
        Some("input_scale_unsupported")
    );
    assert_eq!(text(outcome, "stage"), Some("display_input_map"));
    assert_eq!(number(outcome, "frame_width"), Some(20));
    assert_eq!(number(outcome, "frame_height"), Some(15));
    assert_eq!(number(outcome, "base_width"), Some(16));
    assert_eq!(number(outcome, "base_height"), Some(12));
    assert_eq!(
        run.effects
            .iter()
            .map(|effect| effect.action.clone())
            .collect::<Vec<_>>(),
        [InputAction::Tap { x: 1, y: 0 }]
    );
    assert!(run.inputs.is_empty(), "{:?}", run.inputs);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0, "no input");
    assert_eq!(
        run.geometry
            .first()
            .and_then(|record| record.display_map.map(|map| (
                map.kind,
                map.scale_num,
                map.scale_den
            ))),
        Some((DisplayMapKind::Scale, 5, 4))
    );
}

/// Fix 1, work order point 1 (`contracts/display-adapter.md` section 10): a Lab declared
/// operation, whose runtime reads no game declaration and whose frames carry no producer
/// geometry, sends its input only on a frame of its package's size. `op_a` of a 16x9 package taps
/// the point (1, 0) from `page_a` to `page_b` (the page in the frame's 3x3 top-left block). On a
/// 16x9 device, the identity, the device receives the literal tap (1, 0) and the operation is
/// confirmed. On a 32x18 device, a pure scale of 2 that the kernel's frame gate admits, the
/// operation fails at its input with `display_map_unresolved` (`input_scale_unsupported` at
/// `display_input_map`, the frame 32x18 and the base 16x9): its one attempt keeps the base tap
/// (1, 0) and never reached its input, and the device received nothing.
#[test]
fn da3_fix1_a_lab_declared_operation_sends_its_base_point_only_on_a_frame_of_its_package() {
    use super::gate_412_b3_s2 as lab;
    for k in [1_u32, 2] {
        let root = TempDir::new().expect("tempdir");
        let state = Arc::new(FakeState::default());
        state.physical_task_geometry.store(true, Ordering::Release);
        *state
            .physical_task_extent
            .lock()
            .expect("fake physical task extent") = Some((16 * k, 9 * k));
        let row = (16 * k) as usize;
        *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
            move |_capture: usize, inputs: usize| -> Vec<(usize, [u8; 3])> {
                let page = if inputs > 0 { lab::PAGE_B } else { lab::PAGE_A };
                (0..3)
                    .flat_map(move |line| (0..3).map(move |column| (line * row + column, page)))
                    .collect()
            },
        ));
        let host = lab::start(
            &root,
            FakeProvider::one(lab::ALIAS, instance_id(), Arc::clone(&state)),
        );
        let package = lab::write_package(
            &root,
            (16, 9),
            json!({
                "id": lab::OPERATION,
                "from": "page_a",
                "to": "page_b",
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true,
                "retryable": false
            }),
        );
        let client = lab::lab_client(&root);

        let verified = lab::run_operation(&client, &package, None);

        let record = &verified.operation().record;
        let result = record
            .operation_result
            .as_ref()
            .expect("the declared operation's result");
        let [attempt] = result.attempts.as_slice() else {
            panic!("{k}: one attempt, got {:?}", result.attempts);
        };
        assert_eq!(attempt.action, InputAction::Tap { x: 1, y: 0 }, "{k}");
        let received = state
            .input_actions
            .lock()
            .expect("fake input actions")
            .clone();
        if k == 1 {
            assert!(record.failure.is_none(), "{:?}", record.failure);
            assert_eq!(result.end, LabOperationEnd::Confirmed);
            assert_eq!(received, [InputAction::Tap { x: 1, y: 0 }]);
        } else {
            let failure = record.failure.as_ref().expect("the operation failed");
            assert_eq!(failure.code, "display_map_unresolved", "{failure:?}");
            assert_eq!(failure.stage, LabOperationStage::Input, "{failure:?}");
            assert_eq!(result.end, LabOperationEnd::Failed);
            let outcome = verified.receipt().outcome();
            assert_eq!(outcome.code().as_str(), "display_map_unresolved");
            assert_eq!(
                text(outcome, "display_problem"),
                Some("input_scale_unsupported")
            );
            assert_eq!(text(outcome, "stage"), Some("display_input_map"));
            assert_eq!(number(outcome, "frame_width"), Some(32));
            assert_eq!(number(outcome, "frame_height"), Some(18));
            assert_eq!(number(outcome, "base_width"), Some(16));
            assert_eq!(number(outcome, "base_height"), Some(9));
            assert!(!attempt.input_returned, "the tap never reached its input");
            assert!(received.is_empty(), "{received:?}");
            assert_eq!(state.input_count.load(Ordering::Acquire), 0, "no input");
        }
        drop(client);
        host.close().expect("close host");
    }
}

/// Fix 1, work order point 2 (`contracts/display-adapter.md` section 10): the task evidence
/// pairs every input of a scaled run with its effect intent. The three-input package on a 16x9,
/// a 32x18 and a 48x27 device, read back as `actingledger task-evidence` reads it from the run's
/// first record: each of the tap, the swipe and the path drag relates `linked` to its step's
/// effect intent (under a scale the effect intent's base input, mapped by the run's map, is the
/// input sent; under the identity the two are the same bytes), and no `source_mismatch` is
/// reported.
#[test]
fn da3_fix1_the_task_evidence_pairs_every_scaled_input_with_its_effect_intent() {
    for k in [1_u32, 2, 3] {
        let temp = TempDir::new().expect("tempdir");
        let (host, _state) = device_host(&temp, (16 * k, 9 * k));
        let run = run(&host, &temp, "three-inputs", &three_inputs_package());
        host.close().expect("close runtime host");
        assert_eq!(
            run.receipt.state(),
            RuntimeReceiptState::Completed,
            "{k}: {:?}",
            run.receipt
        );
        let request = ForensicRequest::task_evidence(
            temp.path(),
            ForensicEventsRequest::new(
                ForensicEventFilter::default(),
                run.first_sequence - 1,
                None,
                MAX_FORENSIC_EVENTS,
            )
            .expect("a task evidence page"),
        );
        let ForensicOutput::Machine(ForensicReport::TaskEvidence(report)) =
            actingcommand_ledger_forensics::run(request).expect("the task evidence")
        else {
            panic!("{k}: a task evidence report");
        };
        assert_eq!(
            report
                .inputs
                .iter()
                .map(|input| input.source_step.state)
                .collect::<Vec<_>>(),
            ["linked", "linked", "linked"],
            "{k}: {:?}",
            report.gaps
        );
        assert!(
            !report.gaps.contains(&"source_mismatch"),
            "{k}: {:?}",
            report.gaps
        );
    }
}
