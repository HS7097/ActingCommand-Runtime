// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-3b in the host (`contracts/display-adapter.md` section 10, "The vertical
//! brake"; the coordinator's work order of 11-13 on board #392, route E): under a scale a
//! vertical-brake drag leaves for the device as a drag with a pause and a brake, which the device
//! performs with the same events as the vertical-brake drag of the same points. The effect intent
//! keeps the package's drag in base pixels; the input intent records the drag sent, in device
//! pixels. Under the identity the drag is sent and recorded as before this slice.
//!
//! A synthetic `neutral` game, server `test`, with a 320x180 package whose one operation drags
//! from `home` to `terminal`. The device's frames show the page in their 3x3 top-left block,
//! `home` before any input and `terminal` after one, so the scale frame's first pixel keeps the
//! page's colour.

use super::gate_392_da3 as da3;
use super::*;
use actingcommand_contract::{CaptureExtent, DisplayMapKind, TaskGeometryPhase};
use actingcommand_device::{
    DragWithPauseAndBrakeAction, SegmentedDragAction, SegmentedSwipeAction, SegmentedSwipeEvent,
};
use actingcommand_execution_kernel::PreparedInputAction;
use actingcommand_ledger_forensics::{
    ForensicEventFilter, ForensicEventsRequest, ForensicOutput, ForensicReport, ForensicRequest,
    MAX_FORENSIC_EVENTS,
};
use serde_json::json;

const GAME: &str = "neutral";
const HOME: [u8; 3] = [255, 0, 0];
const TERMINAL: [u8; 3] = [0, 0, 255];

/// The time the moves of `events` take.
fn move_time(events: &[SegmentedSwipeEvent]) -> u64 {
    events
        .iter()
        .map(|event| match event {
            SegmentedSwipeEvent::Move {
                delay_before_ms, ..
            } => *delay_before_ms,
            _ => 0,
        })
        .sum()
}

/// Work order test 1 (the survey's claim, which this slice checked member by member before it
/// was built): for the same points the kernel and the device layer make one plan of both kinds,
/// event for event. A vertical-brake drag from (300, 150) to the corner (20, 150), its brake to
/// (20, 50), and a drag with a pause and a brake through the same three points with drag 200 ms,
/// pause 150 ms and brake 200 ms; the same for (1095, 355), (105, 357) and (105, 257). Both
/// plans are the press at the first point, 13 moves (200 / 16 rounded up) whose waits sum to
/// 200 ms, the last at the corner, the hold of 150 ms, 13 moves summing to 200 ms, the last at the
/// brake's end, and the release. The plans name their own kinds and hold the same points.
#[test]
fn da3b_the_two_drags_are_one_device_plan_for_the_same_points() {
    let plan = |action: InputAction| match PreparedInputAction::try_from(action) {
        Ok(PreparedInputAction::SegmentedSwipe(plan)) => plan,
        Ok(other) => panic!("a segmented plan, got {other:?}"),
        Err(_) => panic!("the kernel prepares the drag"),
    };
    for ((x1, y1), (x2, y2)) in [((300, 150), (20, 150)), ((1095, 355), (105, 357))] {
        let vertical = plan(InputAction::SingleTouchDragWithVerticalBrakeV1 {
            x1,
            y1,
            x2,
            y2,
            x3: x2,
            y3: y2 - 100,
            horizontal_duration_ms: 200,
            corner_hold_ms: 150,
            brake_distance_px: 100,
            brake_duration_ms: 200,
            slope_in: 2,
            slope_out: 0,
        });
        let paused = plan(InputAction::SingleTouchDragWithPauseAndBrakeV1 {
            x1,
            y1,
            x2,
            y2,
            drag_duration_ms: 200,
            pause_ms: 150,
            x3: x2,
            y3: y2 - 100,
            brake_duration_ms: 200,
        });
        assert_eq!(vertical.events(), paused.events(), "({x1}, {y1})");
        let events = paused.events();
        assert_eq!(events.len(), 29, "{events:?}");
        assert_eq!(events[0], SegmentedSwipeEvent::Down((x1, y1)));
        assert!(
            events[1..14]
                .iter()
                .chain(&events[15..28])
                .all(|event| matches!(event, SegmentedSwipeEvent::Move { .. })),
            "{events:?}"
        );
        assert_eq!(move_time(&events[1..14]), 200);
        assert!(
            matches!(events[13], SegmentedSwipeEvent::Move { point, .. } if point == (x2, y2)),
            "{events:?}"
        );
        assert_eq!(events[14], SegmentedSwipeEvent::Hold(150));
        assert_eq!(move_time(&events[15..28]), 200);
        assert!(
            matches!(events[27], SegmentedSwipeEvent::Move { point, .. } if point == (x2, y2 - 100)),
            "{events:?}"
        );
        assert_eq!(events[28], SegmentedSwipeEvent::Up);
        assert_eq!(vertical.action().points(), paused.action().points());
        assert_eq!(
            vertical.action().name(),
            "single_touch_drag_with_vertical_brake_v1"
        );
        assert_eq!(
            paused.action().name(),
            "single_touch_drag_with_pause_and_brake_v1"
        );
    }
}

/// The 320x180 route package: from `home` a vertical-brake drag pressed in the one-pixel box
/// (300, 150, 1, 1), to the corner in the one-pixel box (20, 150, 1, 1), with the interface's
/// fixed times and its brake of 100, to `terminal`.
fn brake_package() -> Vec<u8> {
    let space = json!({"width": 320, "height": 180});
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.test.brake",
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": "test",
        "resolution": space,
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 20_000,
        "timeout_ms": 40_000,
        "max_steps": 2
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": ["test"],
        "coordinate_space": space,
        "entry_page": "home",
        "target_page": "terminal",
        "operations": [{
            "id": "brake",
            "from": "home",
            "to": "terminal",
            "click": {
                "kind": "single_touch_drag_with_vertical_brake_v1",
                "from_rect": {"x": 300, "y": 150, "width": 1, "height": 1},
                "corner_rect": {"x": 20, "y": 150, "width": 1, "height": 1},
                "horizontal_duration_ms": 200,
                "corner_hold_ms": 150,
                "brake_distance_px": 100,
                "brake_duration_ms": 200
            },
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    let target = |id: &str, colour: [u8; 3]| json!({"type": "color", "id": id, "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": colour});
    let recognition = json!({
        "schema_version": "0.3",
        "game": GAME,
        "server": "test",
        "coordinate_space": space,
        "defaults": {"color_max_distance": 0.0},
        "targets": [target("page/home", HOME), target("page/terminal", TERMINAL)]
    });
    let page = |id: &str, target: &str| json!({"id": id, "required": [target], "optional": [], "forbidden": []});
    let pages = json!({
        "schema_version": "0.3",
        "pages": [
            page("neutral/home", "page/home"),
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

/// The device shows `home` in its frames' 3x3 top-left block before any input and `terminal`
/// after one; `width` is the length of a frame's row.
fn terminal_after_one_input(state: &FakeState, width: u32) {
    let row = width as usize;
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        move |_capture: usize, inputs: usize| -> Vec<(usize, [u8; 3])> {
            let page = if inputs > 0 { TERMINAL } else { HOME };
            (0..3)
                .flat_map(move |line| (0..3).map(move |column| (line * row + column, page)))
                .collect()
        },
    ));
}

/// The package's drag as the kernel makes it, in base pixels: pressed at (300, 150), to the
/// corner (20, 150), its brake of 100 to (20, 50).
fn base_drag() -> InputAction {
    InputAction::SingleTouchDragWithVerticalBrakeV1 {
        x1: 300,
        y1: 150,
        x2: 20,
        y2: 150,
        x3: 20,
        y3: 50,
        horizontal_duration_ms: 200,
        corner_hold_ms: 150,
        brake_distance_px: 100,
        brake_duration_ms: 200,
        slope_in: 2,
        slope_out: 0,
    }
}

/// Work order tests 3 and 4 (route E; `contracts/display-adapter.md` section 10, "The vertical
/// brake" and "The task evidence"): the package on a 640x360 device (a pure scale of 2 without a
/// declaration) and on a 320x180 one (the identity). The run completes on `terminal` either way.
///
/// - The effect intent holds the package's drag in base pixels, its two one-pixel boxes and the
///   device frame's size.
/// - Under 2 the input intent names, and the fake device performs, the drag with a pause and a
///   brake from (600, 300) to the corner (40, 300), held 150 ms, braked to (40, 100): the press
///   and the corner twice the base's, the brake 200 (`round_half_up(100 · 2)`), drag 200 ms and
///   brake 200 ms. Like every such drag its input intent records no execution plan.
/// - Under the identity, the literals of today: the input intent names the package's drag, the
///   device performs the vertical-brake drag through (300, 150), (20, 150) and (20, 50), and the
///   input intent records its execution plan.
/// - The device takes no other input, the first geometry record names the pure scale 2 (none under
///   the identity), and the task evidence, read from the run's first record, relates the input
///   `linked` to its step's effect intent, with no `source_mismatch`.
#[test]
fn da3b_a_vertical_brake_drag_reaches_a_scaled_device_as_a_drag_with_a_pause_and_a_brake() {
    for k in [1_u32, 2] {
        let temp = TempDir::new().expect("tempdir");
        let (host, state) = da3::device_host(&temp, (320 * k, 180 * k));
        terminal_after_one_input(&state, 320 * k);
        let run = da3::run(&host, &temp, "vertical-brake", &brake_package());
        let intents = host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::InputIntent),
                ..EventQuery::default()
            })
            .expect("the input intents");
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
        assert_eq!(
            run.effects,
            [da3::EffectRecord {
                action: base_drag(),
                boxes: Some(vec![(300, 150, 1, 1), (20, 150, 1, 1)]),
                frame_extent: CaptureExtent::new(320 * k, 180 * k),
            }],
            "{k}"
        );
        let (sent, performed) = if k == 1 {
            (
                base_drag(),
                SegmentedDragAction::VerticalBrake(SegmentedSwipeAction {
                    points: [(300, 150), (20, 150), (20, 50)],
                    horizontal_duration_ms: 200,
                    corner_hold_ms: 150,
                    brake_distance_px: 100,
                    brake_duration_ms: 200,
                    slope_in: 2,
                    slope_out: 0,
                }),
            )
        } else {
            (
                InputAction::SingleTouchDragWithPauseAndBrakeV1 {
                    x1: 600,
                    y1: 300,
                    x2: 40,
                    y2: 300,
                    drag_duration_ms: 200,
                    pause_ms: 150,
                    x3: 40,
                    y3: 100,
                    brake_duration_ms: 200,
                },
                SegmentedDragAction::PauseAndBrake(DragWithPauseAndBrakeAction {
                    points: [(600, 300), (40, 300), (40, 100)],
                    drag_duration_ms: 200,
                    pause_ms: 150,
                    brake_duration_ms: 200,
                }),
            )
        };
        assert_eq!(
            run.inputs,
            [sent],
            "{k}: the input intent names the drag sent"
        );
        let plans = state
            .segmented_swipe_plans
            .lock()
            .expect("fake segmented swipe plans")
            .iter()
            .map(|plan| plan.action())
            .collect::<Vec<_>>();
        assert_eq!(plans, [performed], "{k}: the drag the device performed");
        assert!(
            state
                .input_actions
                .lock()
                .expect("fake input actions")
                .is_empty(),
            "{k}: no other input"
        );
        assert_eq!(state.input_count.load(Ordering::Acquire), 1, "{k}");
        let execution_plans = intents
            .iter()
            .map(|event| match event.payload() {
                EventPayload::Input(InputPayload::Intent(intent)) => {
                    intent.execution_plan().is_some()
                }
                _ => panic!("{k}: an input intent"),
            })
            .collect::<Vec<_>>();
        assert_eq!(execution_plans, [k == 1], "{k}: the execution plan record");
        assert_eq!(
            run.geometry.first().map(|record| (
                record.phase,
                record
                    .display_map
                    .map(|map| (map.kind, map.scale_num, map.scale_den))
            )),
            Some((
                TaskGeometryPhase::Initial,
                (k > 1).then_some((DisplayMapKind::Scale, k, 1))
            )),
            "{k}"
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
            ["linked"],
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
