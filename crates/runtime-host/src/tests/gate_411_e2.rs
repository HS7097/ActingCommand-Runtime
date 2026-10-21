// SPDX-License-Identifier: AGPL-3.0-only

// #411 E2 (0.12.9): the Runtime's diagnostics say what happened. Every expected value comes from
// the slice brief's requirement lines.
//
// T2 (A30): a run whose operation's guard refuses carries, on its terminal and on its receipt,
// the page the guard judged on (`page_id`) and the frame it judged on (`artifact_id`, the frame
// of the run's `GuardRejection` pin that `capture.summary_committed` records), as typed values
// next to `operation`. The neutral package's one guarded step is on `home`; the fake device's
// guard pixel never passes, so the guard refuses there.
//
// T3 (A37), the Runtime's part: a Lab operation (`ac_lab_do` with capture) whose element id is
// one of the package's recognition target ids fails with `lab_target_type_mismatch`, whose
// outcome names the expected type (`element_id`) and the received one (`target_id`), not with
// `lab_element_unavailable`. The neutral observation package shows `home` and declares one
// navigation element on it.

use super::*;
use actingcommand_contract::{
    ContainedLabOperationRequest, LabOperationSelection, LabOperationStage, LabProjectionHint,
    ObservationFramePlan,
};
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};

const ALIAS: &str = "neutral.instance";
const GUARD_REFUSED: &str = "contained_task_guard_refused";
const TARGET_TYPE_MISMATCH: &str = "lab_target_type_mismatch";

// T2: the terminal and the receipt of a guard refusal name the page and the frame the guard
// judged on.
#[test]
fn e2_t2_a_guard_refusal_names_the_page_and_the_frame_it_judged_on() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("guarded.zip");
    fs::write(&package, neutral_retrying_contained_task_package()).expect("write package");
    let state = Arc::new(FakeState::default());
    state.refuse_guard_capture.store(true, Ordering::Release);
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string()).expect("task request"),
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
    host.close().expect("close runtime host");

    assert_eq!(receipt.state(), RuntimeReceiptState::Failed);
    let judged_frames = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Capture(CapturePayload::SummaryCommitted(summary)) => Some(summary),
            _ => None,
        })
        .flat_map(|summary| summary.summary().pinned())
        .filter(|pin| pin.reason() == PinnedFrameReason::GuardRejection)
        .filter_map(|pin| pin.artifact())
        .map(|artifact| serde_json::to_value(artifact.artifact_id).expect("artifact id"))
        .collect::<Vec<_>>();
    let [judged_frame] = judged_frames.as_slice() else {
        panic!("one GuardRejection pin with its frame: {judged_frames:?}");
    };
    let terminals = events
        .iter()
        .filter(|event| event.event_type() == EventType::TaskFailed)
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::TerminalCommitted {
                    outcome: Some(outcome),
                    ..
                } => Some(outcome.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    let [terminal] = terminals.as_slice() else {
        panic!("one failed terminal with its outcome: {terminals:?}");
    };

    for (carrier, outcome) in [("terminal", terminal), ("receipt", receipt.outcome())] {
        assert_eq!(outcome.code().as_str(), GUARD_REFUSED, "{carrier}");
        let value = |key: &str| outcome.value(key).cloned();
        assert_eq!(value("page_id"), Some(json!("neutral/home")), "{carrier}");
        assert_eq!(
            value("artifact_id").as_ref(),
            Some(judged_frame),
            "{carrier}"
        );
    }
}

// The observation package of T3, written into `root`: `home` and `next` told apart by pixel 0,
// and one navigation element from `home` to `next`.
fn write_observation_package(root: &TempDir) -> PathBuf {
    let colour = |id: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let entries: [(&str, Value); 6] = [
        (
            "control.json",
            json!({"game": "neutral", "server": "test", "package_id": "neutral.lab_e2",
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
                "coordinate_space": {"width": 2, "height": 1},
                "defaults": {"color_max_distance": 0.0},
                "targets": [colour("page/home", [255, 0, 0]), colour("page/next", [0, 0, 255])]
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({
                "schema_version": "0.6",
                "pages": [
                    {"id": "neutral/home", "required": ["page/home"]},
                    {"id": "neutral/next", "required": ["page/next"]}
                ]
            }),
        ),
        (
            "resources/navigation/neutral.test.navigation.json",
            json!({
                "schema_version": "0.6",
                "game": "neutral",
                "server": "test",
                "navigation": [{
                    "id": "to_next", "from_page": "neutral/home", "to_page": "neutral/next",
                    "click": {"kind": "point", "x": 1, "y": 0},
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
    let path = root.path().join("lab-e2.zip");
    fs::write(&path, zip.finish().expect("finish zip").into_inner()).expect("write package");
    path
}

// T3 (Runtime): `do` with a recognition target id where the element id belongs fails with the
// type mismatch code, which names the expected and the received type.
#[test]
fn e2_t3_a_lab_operation_naming_a_target_id_fails_with_the_type_mismatch() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, ALIAS, Arc::clone(&state));
    let package = write_observation_package(&root);
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Lab client");
    let verified = client
        .begin_debug_session()
        .expect("debug session")
        .run_contained_lab_operation(
            ALIAS,
            ContainedLabOperationRequest {
                package_path: package.to_str().expect("path").to_owned(),
                selection: LabOperationSelection::Element {
                    id: "page/home".to_owned(),
                },
                projection_hint: LabProjectionHint {
                    sequence: None,
                    content_sha256: None,
                },
                after: None,
            },
            ObservationFramePlan::SINGLE,
        )
        .expect("the client verifies the operation");
    let record = &verified.operation().record;
    let failure = record.failure.as_ref().expect("the operation's failure");
    assert_eq!(failure.stage, LabOperationStage::Selection);
    assert_eq!(failure.code, TARGET_TYPE_MISMATCH);
    let outcome = verified.receipt().outcome();
    assert_eq!(outcome.code().as_str(), TARGET_TYPE_MISMATCH);
    assert_eq!(
        outcome.value("expected_target_type"),
        Some(&json!("element_id"))
    );
    assert_eq!(
        outcome.value("received_target_type"),
        Some(&json!("target_id"))
    );
    drop(client);
    host.close().expect("close host");
}
