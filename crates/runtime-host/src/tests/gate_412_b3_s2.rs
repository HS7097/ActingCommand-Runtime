// SPDX-License-Identifier: AGPL-3.0-only

// #412 Lab B-3 S2 (frozen design rd-1021/train-0129/lab-b3-design.md §2.3, §3, §4 and §5 "S2",
// tests T2-1 to T2-4, and T2-8 of the coordinator's rulings of 10-22; register §1.162; host keys
// 152 to 155; ledger revision 4 row LR-10): the host runs one operation a package declares as the
// kernel runs it, over the Lab lease, and writes its LR-10 records.
//
// - T2-1: the prepared record is persisted and verified before attempt 1's `InputIntent`; an
//   operation of two attempts ends `confirmed`, its record passes its own validation and the
//   shared verifier (the Lab client runs it).
// - T2-2: an instance whose input is `nemu_ipc` takes the brake drag the kernel generates, on the
//   frame the kernel bound the input to.
// - T2-3: the capture after the 256th is refused, `lab_operation_frame_limit_reached`
//   (`count`, `limit_count` 256), and no input follows it.
// - T2-4: two operation requests on one box land on different candidates: the press chooses
//   through the instance's landing history, which runs share (decision Q1).
// - T2-8: a keyed end before any input (#411 B22) records no attempt and the guard
//   `not_evaluated`, and its records verify (host key 152).
//
// Every expected value comes from the design and its decisions. The fake instance's frames are
// scripted per capture (`FakeState.pixel_script`): pixel 0 shows the page. The package is
// synthetic and neutral (`page_a`, `page_b`, `op_a`). The Runtime clock is manual: every wait of
// the kernel moves it and returns at once.

use super::*;
use actingcommand_contract::{
    ContainedLabOperationRequest, EffectiveDeviceConfiguration, LabGuardOutcome, LabOperationEnd,
    LabOperationSelection, LabOperationStage, LabProjectionHint, ObservationFramePlan,
};
use actingcommand_execution_kernel::ResolvedExecutionInstance;
use actingcommand_runtime_client::{
    RuntimeChannel, RuntimeClient, RuntimeClientConfig, VerifiedLabOperation,
};
use serde_json::{Value, json};

const ALIAS: &str = "node.a";
const PAGE_A: [u8; 3] = [255, 0, 0];
const PAGE_B: [u8; 3] = [0, 0, 255];
const OPERATION: &str = "op_a";
/// The package's task timeout (`control.json` `timeout_ms`), which bounds the receipt wait.
const TASK_TIMEOUT_MS: u64 = 30_000;
/// LR-10: the most frames a declared operation's record lists.
const FRAME_LIMIT: usize = 256;
/// The seed both requests of T2-4 give.
const SEED: u64 = 23;
/// T2-8: the key `op_a`'s keyed guard refusal ends the run with.
const KEY: &str = "key_a";
/// The colour of the ready flag (pixel 1) a guard of `op_a` may require.
const READY: [u8; 3] = [0, 255, 0];

/// A neutral package of `width` × `height` frames whose pages `neutral/page_a` and
/// `neutral/page_b` are told apart by pixel 0, and whose entry task declares `operation`,
/// written into `root`.
fn write_package(root: &TempDir, size: (u32, u32), operation: Value) -> PathBuf {
    write_task_package(root, size, operation, json!({}))
}

/// [`write_package`] whose task also declares the members of `task_extra`; the pack has a ready
/// flag on pixel 1 (`flag/ready`) a guard may require.
fn write_task_package(
    root: &TempDir,
    (width, height): (u32, u32),
    operation: Value,
    task_extra: Value,
) -> PathBuf {
    let space = json!({"width": width, "height": height});
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": space.clone(),
        "target_page": "page_b",
        "operations": [operation]
    });
    for (member, value) in task_extra.as_object().expect("task members") {
        task[member] = value.clone();
    }
    let page_target = |page: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": format!("page/{page}"),
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let entries: [(&str, Value); 6] = [
        (
            "control.json",
            json!({
                "schema_version": "Lab-1y.control.v2",
                "package_id": "neutral.lab_b3_s2",
                "execution_mode": "navigable_route",
                "game": "neutral",
                "server": "test",
                "resolution": space.clone(),
                "entry_task_id": "task",
                "capture_interval_ms": 1,
                "step_timeout_ms": 1_000,
                "timeout_ms": TASK_TIMEOUT_MS,
                "max_steps": 2
            }),
        ),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        (
            "resources/recognition/neutral.test.pack.json",
            json!({
                "schema_version": "0.7",
                "game": "neutral",
                "server": "test",
                "coordinate_space": space,
                "defaults": {"color_max_distance": 0.0},
                "targets": [
                    page_target("page_a", PAGE_A),
                    page_target("page_b", PAGE_B),
                    json!({"type": "color", "id": "flag/ready",
                           "region": {"x": 1, "y": 0, "width": 1, "height": 1},
                           "expected": READY})
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
            json!({"navigation": []}),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in &entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(value).expect("JSON"))
            .expect("zip bytes");
    }
    let path = root.path().join("lab-b3-s2.zip");
    fs::write(&path, zip.finish().expect("finish zip").into_inner()).expect("write package");
    path
}

/// The device shows `page_b` once it took `inputs` inputs, `page_a` before.
fn page_b_after(state: &FakeState, inputs: usize) {
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        move |_capture: usize, taken: usize| -> Vec<(usize, [u8; 3])> {
            vec![(0, if taken >= inputs { PAGE_B } else { PAGE_A })]
        },
    ));
}

/// A host on `root` over `provider`, on a manual Runtime clock.
fn start(root: &TempDir, provider: FakeProvider) -> RuntimeHost {
    RuntimeHost::start(
        config(root).with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0))),
        Arc::new(provider),
    )
    .expect("runtime host")
}

fn lab_client(root: &TempDir) -> RuntimeClient {
    RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Lab client")
}

/// One Lab operation running `op_a` of `package`, with `seed`, verified by the Lab client (the
/// shared evidence verifier included).
fn run_operation(
    client: &RuntimeClient,
    package: &Path,
    seed: Option<u64>,
) -> VerifiedLabOperation {
    client
        .begin_debug_session()
        .expect("debug session")
        .run_contained_lab_declared_operation(
            ALIAS,
            ContainedLabOperationRequest {
                package_path: package.to_str().expect("path").to_owned(),
                selection: LabOperationSelection::Operation {
                    operation_id: OPERATION.to_owned(),
                    seed,
                },
                projection_hint: LabProjectionHint::default(),
                after: None,
            },
            ObservationFramePlan::SINGLE,
            Duration::from_millis(TASK_TIMEOUT_MS),
        )
        .expect("the client verifies the operation")
}

// T2-1 (host key 152; design §2.3 "Traces", §3; §5 S2 T2-1): `op_a` taps from `page_a` to
// `page_b`, retried once; the device shows `page_b` only after the second tap. The operation ends
// `confirmed` on `page_b` after two attempts; the prepared record was persisted and verified
// before attempt 1's `InputIntent`, and the record passes its own validation and the shared
// verifier, which the Lab client ran.
#[test]
fn b3_s2_t2_1_the_prepared_record_is_verified_before_attempt_1_and_two_attempts_verify() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    page_b_after(&state, 2);
    let host = start(
        &root,
        FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state)),
    );
    let package = write_package(
        &root,
        (2, 1),
        json!({
            "id": OPERATION,
            "from": "page_a",
            "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": true,
            "max_attempts": 2,
            "retry_interval_ms": 100,
            "expect_after": {"page_id": "page_b", "timeout_ms": 300, "interval_ms": 100}
        }),
    );
    let client = lab_client(&root);

    let verified = run_operation(&client, &package, None);

    let operation = verified.operation();
    operation
        .validate()
        .expect("the record passes its own validation");
    let record = &operation.record;
    assert!(record.failure.is_none(), "{:?}", record.failure);
    let result = record
        .operation_result
        .as_ref()
        .expect("the declared operation's result");
    assert_eq!(result.end, LabOperationEnd::Confirmed);
    assert_eq!(result.final_page.as_deref(), Some("neutral/page_b"));
    assert_eq!(
        result
            .attempts
            .iter()
            .map(|attempt| attempt.attempt)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 2);
    let first_intent = result.attempts[0]
        .input_intent
        .expect("attempt 1's input intent");
    assert!(
        record.prepared_artifact.verified.sequence < first_intent.sequence,
        "the prepared record (verified at {}) precedes attempt 1's InputIntent ({})",
        record.prepared_artifact.verified.sequence,
        first_intent.sequence
    );
    assert!(
        record.prepared.operation.is_some(),
        "the prepared record states the declared operation"
    );
    drop(client);
    host.close().expect("close host");
}

// T2-2 (host key 153; design §2.2 "gesture", §2.3 "input"; §5 S2 T2-2): on an instance whose
// input is `nemu_ipc`, `op_a`'s vertical-brake drag is taken as one segmented plan, and its input
// names the frame the kernel bound it to: attempt 1's input frame.
#[test]
fn b3_s2_t2_2_the_nemu_input_takes_the_brake_drag_with_the_kernels_input_frame() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((320, 240));
    page_b_after(&state, 1);
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
    let host = start(
        &root,
        FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))
            .with_resolved_for(ALIAS, resolved),
    );
    let package = write_package(
        &root,
        (320, 240),
        json!({
            "id": OPERATION,
            "from": "page_a",
            "to": "page_b",
            "click": {"kind": "single_touch_drag_with_vertical_brake_v1",
                      "from_rect": {"x": 20, "y": 150, "width": 40, "height": 40},
                      "corner_rect": {"x": 200, "y": 150, "width": 40, "height": 40},
                      "horizontal_duration_ms": 200, "corner_hold_ms": 150,
                      "brake_distance_px": 100, "brake_duration_ms": 200},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }),
    );
    let client = lab_client(&root);

    let verified = run_operation(&client, &package, None);

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
    assert!(
        matches!(
            attempt.action,
            InputAction::SingleTouchDragWithVerticalBrakeV1 { .. }
        ),
        "{:?}",
        attempt.action
    );
    let plans = state
        .segmented_swipe_plans
        .lock()
        .expect("fake segmented swipe plans lock")
        .clone();
    assert_eq!(plans.len(), 1, "one segmented plan: {plans:?}");
    let input_frame = result
        .frames
        .get(usize::try_from(attempt.input_frame_index).expect("index"))
        .expect("the attempt's input frame")
        .frame
        .observation
        .artifact()
        .frame_id;
    let intents = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(record.prepared.correlation_id),
            event_type: Some(EventType::InputIntent),
            ..EventQuery::default()
        })
        .expect("query the input intent");
    let [intent] = intents.as_slice() else {
        panic!("one input intent, got {}", intents.len());
    };
    let EventPayload::Input(InputPayload::Intent(intent)) = intent.payload() else {
        panic!("typed input intent");
    };
    let provenance = intent.provenance().expect("the input intent's provenance");
    assert_eq!(provenance.before_frame_id, input_frame);
    assert!(input_frame.is_some(), "the Nemu input names its frame");
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}

// T2-3 (host key 154; design §2.3 "capture", §4 "New codes"; §5 S2 T2-3): `op_a` taps once and
// waits for `page_b`, which never shows, one frame a millisecond, with a retry declared. The
// capture after the 256th is refused: the operation fails at `after_frame` with
// `lab_operation_frame_limit_reached` (`count` 257, `limit_count` 256), lists 256 frames, and no
// input follows its first; the record still verifies.
#[test]
fn b3_s2_t2_3_the_capture_after_the_frame_limit_ends_the_operation_without_another_input() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    page_b_after(&state, usize::MAX);
    let host = start(
        &root,
        FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state)),
    );
    let package = write_package(
        &root,
        (2, 1),
        json!({
            "id": OPERATION,
            "from": "page_a",
            "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": true,
            "max_attempts": 2,
            "retry_interval_ms": 100,
            "expect_after": {"page_id": "page_b", "timeout_ms": 1_000, "interval_ms": 1}
        }),
    );
    let client = lab_client(&root);

    let verified = run_operation(&client, &package, None);

    let record = &verified.operation().record;
    let failure = record.failure.as_ref().expect("the operation failed");
    assert_eq!(failure.code, "lab_operation_frame_limit_reached");
    assert_eq!(failure.stage, LabOperationStage::AfterFrame);
    let outcome = verified.receipt().outcome();
    assert_eq!(outcome.code().as_str(), "lab_operation_frame_limit_reached");
    assert_eq!(outcome.value("count"), Some(&json!(FRAME_LIMIT + 1)));
    assert_eq!(outcome.value("limit_count"), Some(&json!(FRAME_LIMIT)));
    let result = record
        .operation_result
        .as_ref()
        .expect("the declared operation's result");
    assert_eq!(result.end, LabOperationEnd::Failed);
    assert_eq!(result.frames.len(), FRAME_LIMIT);
    assert_eq!(result.attempts.len(), 1);
    assert_eq!(
        state.input_count.load(Ordering::Acquire),
        1,
        "no input after the refused capture"
    );
    drop(client);
    host.close().expect("close host");
}

// T2-4 (host key 155; design §2.3 "touch_sampling, choose_touch_candidate", decision Q1; §5 S2
// T2-4): two requests run `op_a`, which presses a 40 × 20 box, with the same seed, so attempt 1 of
// each draws the same candidates (decision Q9). The first lands on the first candidate; the
// instance's landing history, shared with runs, then holds that point, so the second lands on the
// first candidate elsewhere (the least-recent rule, Alice 2026-10-14). Their points differ.
#[test]
fn b3_s2_t2_4_two_operation_requests_on_one_box_land_on_different_candidates() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((64, 36));
    page_b_after(&state, usize::MAX);
    let host = start(
        &root,
        FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state)),
    );
    let rect = (8, 8, 40, 20);
    let package = write_package(
        &root,
        (64, 36),
        json!({
            "id": OPERATION,
            "from": "page_a",
            "click": {"kind": "rect", "x": rect.0, "y": rect.1, "width": rect.2, "height": rect.3},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }),
    );
    let client = lab_client(&root);

    let presses = (0..2)
        .map(|_| {
            let verified = run_operation(&client, &package, Some(SEED));
            let record = &verified.operation().record;
            assert!(record.failure.is_none(), "{:?}", record.failure);
            let result = record
                .operation_result
                .as_ref()
                .expect("the declared operation's result");
            let [attempt] = result.attempts.as_slice() else {
                panic!("one attempt, got {:?}", result.attempts);
            };
            let sampling = attempt.sampling.clone().expect("the press's draw");
            assert_eq!(
                sampling.action_seed(),
                SEED,
                "attempt 1 draws from the seed"
            );
            (attempt.action.clone(), sampling)
        })
        .collect::<Vec<_>>();

    // The defaults of the touch keys: 250 ‰, 8 redraws, 4 candidates (Workflow #406 T12).
    let candidates = super::gate_406_t12::reference_candidates(SEED, rect, 250, 8, 4);
    let point = |action: &InputAction| match action {
        InputAction::Tap { x, y } => (*x, *y),
        other => panic!("a tap, got {other:?}"),
    };
    let first = point(&presses[0].0);
    assert_eq!(
        first, candidates[0],
        "the first lands on the first candidate"
    );
    let elsewhere = candidates
        .iter()
        .position(|candidate| *candidate != first)
        .expect("a candidate elsewhere");
    let second = point(&presses[1].0);
    assert_eq!(second, candidates[elsewhere]);
    assert_ne!(
        second, first,
        "the two requests land on different candidates"
    );
    assert_eq!(
        presses[1]
            .1
            .parameters()
            .and_then(|parameters| parameters.chosen_candidate),
        Some(u32::try_from(elsewhere).expect("index"))
    );
    drop(client);
    host.close().expect("close host");
}

// T2-8 (host key 152; coordinator rulings 1 and 2 of 10-22 on design §3, §3.3 item 2 and §5 S2:
// LR-10's guard `not_evaluated` and the shared verifier amended before LR-10's first writer):
// `op_a`'s guard requires the ready flag, which never shows; its keyed `on_guard_refused` ends the
// run on `page_a` with `key_a` before any input (#411 B22). The operation ends `run_completed` on
// `page_a` with `key_a`; its records state no attempt, no before frame, no action and the guard
// `not_evaluated`, list the frames the kernel captured (the after frame the last), and pass the
// shared verifier, which the Lab client ran; nothing is input.
#[test]
fn b3_s2_t2_8_a_keyed_end_before_any_input_records_no_attempt_and_verifies() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    // Pixel 1 never shows the ready flag.
    state.refuse_guard_capture.store(true, Ordering::Release);
    page_b_after(&state, usize::MAX);
    let host = start(
        &root,
        FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state)),
    );
    let mapping = |key: &str, page: &str| json!({"outcome_key": key, "effect": "no_designated_effect", "terminal_pages": [page]});
    let package = write_task_package(
        &root,
        (2, 1),
        json!({
            "id": OPERATION,
            "from": "page_a",
            "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "guard": {"page_id": "page_a", "target_id": "flag/ready",
                      "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                      "color_probe": "flag/ready"},
            "on_guard_refused": {"outcome_key": KEY},
            "retryable": false
        }),
        json!({"scheduling_outcome": {"mappings": [
            mapping(KEY, "page_a"),
            mapping("done", "page_b")
        ]}}),
    );
    let client = lab_client(&root);

    let verified = run_operation(&client, &package, None);

    let record = &verified.operation().record;
    assert!(record.failure.is_none(), "{:?}", record.failure);
    let result = record
        .operation_result
        .as_ref()
        .expect("the declared operation's result");
    assert_eq!(result.end, LabOperationEnd::RunCompleted);
    assert_eq!(result.final_page.as_deref(), Some("neutral/page_a"));
    assert_eq!(result.outcome_key.as_deref(), Some(KEY));
    assert!(result.attempts.is_empty(), "{:?}", result.attempts);
    assert!(!result.frames.is_empty(), "the frames the kernel captured");
    assert_eq!(
        record.after_frame.as_ref(),
        result.frames.last().map(|entry| &entry.frame)
    );
    let prepared = &record.prepared;
    assert_eq!(
        prepared
            .operation
            .as_ref()
            .map(|operation| &operation.guard),
        Some(&LabGuardOutcome::NotEvaluated)
    );
    assert!(prepared.before_frame.is_none(), "no before frame");
    assert!(prepared.action.is_none(), "no action");
    assert_eq!(
        state.input_count.load(Ordering::Acquire),
        0,
        "nothing input"
    );
    drop(client);
    host.close().expect("close host");
}
