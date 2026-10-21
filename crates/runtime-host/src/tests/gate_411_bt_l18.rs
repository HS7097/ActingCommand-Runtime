// SPDX-License-Identifier: AGPL-3.0-only

// #411 A18 option ④ (slice BT-LAB, plan §2.17, test row BT-L18 T1; host key 128; register §1.146
// L18-1): online `do`'s arrival samples start every 500 ms, start to start; after a sample's
// capture and evaluation the request waits for what is left of the period, never past the
// deadline. On the manual Runtime clock every capture records the instant it starts and then moves
// the clock 300 ms, the time the sample's capture and evaluation take, so the request's wait is
// all that moves it further. The fake instance's 2x1 frames are scripted per capture
// (`FakeState.pixel_script`): pixel 0 shows the page. The package is synthetic and neutral.

use super::*;
use actingcommand_contract::{
    ContainedLabOperationRequest, LabArrivalCondition, LabArrivalStatus, LabOperationSelection,
    LabProjectionHint, ObservationFramePlan,
};
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::sync::Mutex;

const ALIAS: &str = "node.a";
const HOME: [u8; 3] = [255, 0, 0];
const NEXT: [u8; 3] = [0, 0, 255];
/// What a sample's capture and evaluation take on the manual Runtime clock.
const SAMPLE_WORK_MS: u64 = 300;
/// The before frame is capture 1; the arrival samples are captures 2 to 5, the last showing the
/// arrival page.
const FIRST_ARRIVAL_CAPTURE: usize = 2;
const ARRIVAL_SHOWN_ON: usize = 5;

// An observation package (the Lab resource layout) whose pages `home` and `next` are told apart
// by pixel 0, written into `root`.
fn write_package(root: &TempDir) -> PathBuf {
    let color_target = |page: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": format!("page/{page}"),
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let entries: [(&str, Value); 6] = [
        (
            "control.json",
            json!({"game": "neutral", "server": "test", "package_id": "neutral.bt_l18",
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
                "targets": [color_target("home", HOME), color_target("next", NEXT)]
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version": "0.6", "pages": [
                {"id": "neutral/home", "required": ["page/home"]},
                {"id": "neutral/next", "required": ["page/next"]}
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
    let path = root.path().join("bt-l18.zip");
    fs::write(&path, zip.finish().expect("finish zip").into_inner()).expect("write package");
    path
}

// T1: a do on `home` taps once and waits for `next`, which the fourth arrival sample shows. Each
// sample's capture and evaluation take 300 ms on the Runtime clock; the samples start 500 ms
// apart, start to start, and the arrival is reached on the fourth.
#[test]
fn a18_arrival_samples_start_500_ms_apart_when_their_evaluation_takes_300_ms() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let starts = Arc::new(Mutex::new(Vec::<(usize, u64)>::new()));
    let (script_clock, script_starts) = (Arc::clone(&clock), Arc::clone(&starts));
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        move |capture: usize, _inputs: usize| -> Vec<(usize, [u8; 3])> {
            script_starts
                .lock()
                .expect("capture starts")
                .push((capture, script_clock.monotonic_ms()));
            script_clock.advance(SAMPLE_WORK_MS);
            vec![(
                0,
                if capture >= ARRIVAL_SHOWN_ON {
                    NEXT
                } else {
                    HOME
                },
            )]
        },
    ));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    let package = write_package(&root);
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
                selection: LabOperationSelection::Coordinates {
                    action: InputAction::Tap { x: 1, y: 0 },
                },
                projection_hint: LabProjectionHint {
                    sequence: None,
                    content_sha256: None,
                },
                after: Some(LabArrivalCondition {
                    page_id: "neutral/next".to_owned(),
                    timeout_ms: 5_000,
                }),
            },
            ObservationFramePlan::SINGLE,
        )
        .expect("the client verifies the operation");
    let record = &verified.operation().record;
    assert!(record.failure.is_none(), "{:?}", record.failure);
    let arrival = record.arrival.as_ref().expect("the arrival");
    assert_eq!(arrival.status, LabArrivalStatus::Reached);
    assert_eq!(arrival.samples.len(), 4);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);

    let starts = starts.lock().expect("capture starts").clone();
    let arrival_starts = starts
        .iter()
        .filter(|(capture, _)| *capture >= FIRST_ARRIVAL_CAPTURE)
        .map(|(_, started_ms)| *started_ms)
        .collect::<Vec<_>>();
    assert_eq!(arrival_starts.len(), 4, "{starts:?}");
    let apart = arrival_starts
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect::<Vec<_>>();
    assert_eq!(
        apart,
        vec![500, 500, 500],
        "the arrival samples start 500 ms apart, start to start: {starts:?}"
    );
    drop(client);
    host.close().expect("close host");
}
