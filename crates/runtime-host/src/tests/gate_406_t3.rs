// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #406 T3 (host gate key 124; register §1.137, §1.18 H1; plan Q18, Q19; brief T3
//! requirements 3 and 5): a package run performs the path drag the kernel generates from a
//! `single_touch_path_drag_v1` click as one segmented plan on a backend that performs segmented
//! swipes, the Nemu input included, which presses only on a frame the Runtime captured. The run
//! records the generated points in the effect intent's action and the input intent's provenance
//! (event action `input.swipe`), no sampling evidence and no execution plan record. Packages and
//! frames are neutral.

use super::*;
use actingcommand_contract::{EffectiveDeviceConfiguration, EventAction, InputSamplingEvidence};
use actingcommand_device::{PathDragAction, SegmentedDragAction};
use actingcommand_execution_kernel::ResolvedExecutionInstance;

const ALIAS: &str = "neutral.instance";

/// The neutral package whose one operation from `home` drags along a path in the whole 16 × 9
/// frame: two turns, segments of at least 5 px, 600 ms and a hold of 40 ms at each corner.
fn path_drag_package() -> Vec<u8> {
    neutral_contained_task_package_with_task(
        true,
        br#"{
            "schema_version":"0.6",
            "task_id":"task",
            "game":"neutral",
            "server_scope":["test"],
            "coordinate_space":{"width":16,"height":9},
            "entry_page":"home",
            "target_page":"terminal",
            "operations":[{
                "id":"open_terminal",
                "from":"home",
                "click":{
                    "kind":"single_touch_path_drag_v1",
                    "box_rect":{"x":0,"y":0,"width":16,"height":9},
                    "turns":2,
                    "min_segment_px":5,
                    "duration_ms":600,
                    "corner_hold_ms":40
                },
                "unguarded_trusted_coordinate":true,
                "retryable":false
            }]
        }"#,
    )
}

/// The instance's fake device: it shows the entry page until an input, then the target page.
fn device() -> Arc<FakeState> {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state
}

/// What one run of the path drag package recorded: the effect intent's action and sampling, and
/// the input intent's event action, provenance action, frame and execution plan, read back from
/// the ledger.
struct PathDragRun {
    effect_action: InputAction,
    effect_sampling: Option<InputSamplingEvidence>,
    input_event_action: EventAction,
    input_action: InputAction,
    input_before_frame: bool,
    input_execution_plan: bool,
}

fn run_path_drag(host: &RuntimeHost, root: &TempDir) -> PathDragRun {
    let mut client = TestClient::connect(host);
    let package_path = root.path().join("path-drag-task.zip");
    fs::write(&package_path, path_drag_package()).expect("write package");
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
                .expect("contained task request"),
        ),
    );
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    drop(client);

    let effects = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            event_type: Some(EventType::TaskEffectIntent),
            ..EventQuery::default()
        })
        .expect("query the effect intent");
    let [effect] = effects.as_slice() else {
        panic!("one effect intent, got {}", effects.len());
    };
    let EventPayload::Task(TaskPayload::Semantic(effect)) = effect.payload() else {
        panic!("the effect intent is a typed task payload");
    };
    let TaskSemanticFact::EffectIntent { action, .. } = effect.fact() else {
        panic!("the effect intent holds its action");
    };
    let effect_action = action.clone();
    let effect_sampling = effect.sampling().cloned();

    let inputs = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            event_type: Some(EventType::InputIntent),
            ..EventQuery::default()
        })
        .expect("query the input intent");
    let [input] = inputs.as_slice() else {
        panic!("one input intent, got {}", inputs.len());
    };
    let EventPayload::Input(InputPayload::Intent(intent)) = input.payload() else {
        panic!("typed input intent");
    };
    let provenance = intent.provenance().expect("the input intent's provenance");
    PathDragRun {
        effect_action,
        effect_sampling,
        input_event_action: intent.action(),
        input_action: provenance.input_action.clone(),
        input_before_frame: provenance.before_frame_id.is_some(),
        input_execution_plan: intent.execution_plan().is_some(),
    }
}

/// The generated drag's points, durations and corner hold, checked against the click: two turns
/// make four points in the 16 × 9 box, each segment at least 5 px, the durations summing to 600.
fn checked_path(action: &InputAction) -> PathDragAction {
    let InputAction::SingleTouchPathDragV1 {
        box_x,
        box_y,
        box_width,
        box_height,
        points,
        segment_duration_ms,
        corner_hold_ms,
    } = action
    else {
        panic!("the run generates a path drag, got {action:?}");
    };
    assert_eq!((*box_x, *box_y, *box_width, *box_height), (0, 0, 16, 9));
    assert_eq!(points.len(), 4, "{points:?}");
    for point in points {
        assert!(
            (0..16).contains(&point.x) && (0..9).contains(&point.y),
            "{point:?} outside the box"
        );
    }
    for segment in points.windows(2) {
        let (dx, dy) = (segment[1].x - segment[0].x, segment[1].y - segment[0].y);
        assert!(
            dx * dx + dy * dy >= 25,
            "segment {segment:?} shorter than 5 px"
        );
    }
    assert_eq!(segment_duration_ms.iter().sum::<u64>(), 600);
    assert_eq!(*corner_hold_ms, 40);
    PathDragAction {
        points: points.iter().map(|point| (point.x, point.y)).collect(),
        segment_duration_ms: segment_duration_ms.clone(),
        corner_hold_ms: *corner_hold_ms,
    }
}

/// The plans the fake backend performed.
fn performed_plans(device: &FakeState) -> Vec<PreparedSegmentedSwipePlan> {
    device
        .segmented_swipe_plans
        .lock()
        .expect("fake segmented swipe plans lock")
        .clone()
}

// Requirements 3 and 5 (plan Q19): a run on a backend that performs segmented swipes performs the
// generated path drag as one plan through its points; the effect intent's action and the input
// intent's provenance hold the same generated points (event action `input.swipe`), with no
// sampling evidence and no execution plan record, read back from the ledger.
#[test]
fn t3_a_run_performs_a_path_drag_on_a_segmented_backend_and_records_its_points() {
    let root = TempDir::new().expect("tempdir");
    let device = device();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(&device))),
    )
    .expect("runtime host");

    let run = run_path_drag(&host, &root);
    let path = checked_path(&run.effect_action);
    assert!(
        run.effect_sampling.is_none(),
        "a path drag carries no sampling evidence"
    );
    assert_eq!(run.input_action, run.effect_action);
    assert_eq!(run.input_event_action, EventAction::InputSwipe);
    assert!(!run.input_execution_plan, "a path drag records no plan");
    let plans = performed_plans(&device);
    let [plan] = plans.as_slice() else {
        panic!("one performed plan, got {plans:?}");
    };
    assert_eq!(plan.action(), SegmentedDragAction::PathDrag(path));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close runtime host");
}

// Requirement 5: an instance whose input is `nemu_ipc` admits the path drag as it admits the two
// brake drags: the run's input names the Runtime frame it was decided on, and the drag is
// performed as one plan.
#[test]
fn t3_the_nemu_input_admits_a_path_drag_with_a_frame() {
    let root = TempDir::new().expect("tempdir");
    let device = device();
    // An instance configured as a Nemu one (neutral test values).
    let resolved = ResolvedExecutionInstance::new(instance_id(), "127.0.0.1:16384")
        .with_configuration(EffectiveDeviceConfiguration {
            input_backend: "nemu_ipc".to_owned(),
            capture_backend: "nemu_ipc".to_owned(),
            input_adb: "<sealed-test>".to_owned(),
            capture_adb: "<sealed-test>".to_owned(),
            configured_serial: None,
            resolved_serial: "TESTSERIAL0001".to_owned(),
            input_command_timeout_ms: 5_000,
            capture_command_timeout_ms: 5_000,
            capture_timeout_ms: 5_000,
            configured_mumu_root: None,
            configured_capture_dll: None,
        });
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(ALIAS, instance_id(), Arc::clone(&device))
                .with_resolved_for(ALIAS, resolved),
        ),
    )
    .expect("runtime host");

    let run = run_path_drag(&host, &root);
    let path = checked_path(&run.effect_action);
    assert!(run.input_before_frame, "the Nemu input names its frame");
    assert_eq!(run.input_action, run.effect_action);
    let plans = performed_plans(&device);
    let [plan] = plans.as_slice() else {
        panic!("one performed plan, got {plans:?}");
    };
    assert_eq!(plan.action(), SegmentedDragAction::PathDrag(path));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close runtime host");
}
