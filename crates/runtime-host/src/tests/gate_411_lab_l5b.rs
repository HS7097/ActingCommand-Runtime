// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab L5b (host key 121; register §1.130 L5-6 to L5-8; coordinator decisions Q2 and Q5 of
// 2026-10-19): `observe --capture` projects a candidate layout that needs more frames over its
// own series. When the observation is complete and recognises one page, each such layout of the
// page takes F_l - 1 further page transactions, the layout's interval apart, each on the same
// page and frame size, and is projected over the current frame and each further transaction's
// current frame, as a select step does; the observation's frame and every other row stay its own
// transaction's; an observation takes at most 25 frames; the input check covers the whole
// observation. The fake instance's 2x1 frames are scripted per capture (`FakeState.pixel_script`)
// where the test needs it: pixel 0 shows the page, pixel 1 the capture's number. Packages are
// synthetic and neutral; every interval is 1 ms.

use super::*;
use crate::{RuntimeClock, RuntimeClockSample, SystemRuntimeClock};
use actingcommand_contract::{
    ContainedObservationEvidence, ContainedObservationRequest, ObservationFacts,
    ObservationSeriesPlan, PageObservationStatus,
};
use actingcommand_execution_kernel::PreparedPageObservation;
use actingcommand_runtime_client::{
    RuntimeChannel, RuntimeClient, RuntimeClientConfig, RuntimeClientResult,
    VerifiedPageObservation,
};
use serde_json::{Value, json};
use std::sync::Mutex;

// Bounds only a broken run: the held wait and the tapping thread meet on channels.
const GATE_WAIT: Duration = Duration::from_secs(30);
const ALIAS: &str = "node.a";
const HOME: [u8; 3] = [255, 0, 0];
const OTHER: [u8; 3] = [0, 0, 255];
// The fake instance's pixel 1 when no script sets it.
const READY: [u8; 3] = [0, 255, 0];

fn json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON")
}

// An observation package (the Lab resource layout) with `pack` and `pages`, written into `root`.
fn write_package(root: &TempDir, pack: Value, pages: Value) -> PathBuf {
    let entries = [
        (
            "control.json",
            json!({"game": "neutral", "server": "test", "package_id": "neutral.lab_l5b",
                   "entry_task_id": "task"}),
        ),
        ("resources/manifest.json", json!({"entry_task_id": "task"})),
        (
            "resources/operations/task/task.json",
            json!({"task_id": "task"}),
        ),
        ("resources/recognition/neutral.test.pack.json", pack),
        ("resources/recognition/neutral.test.pages.json", pages),
        (
            "resources/navigation/neutral.test.navigation.json",
            json!({"navigation": []}),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in &entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(&json_bytes(value)).expect("zip bytes");
    }
    let path = root.path().join("lab-l5b.zip");
    fs::write(&path, zip.finish().expect("finish zip").into_inner()).expect("write package");
    path
}

fn color_target(id: &str, x: u32, expected: [u8; 3]) -> Value {
    json!({"type": "color", "id": id,
           "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
}

// The recognition pack: one page target per page colour of pixel 0, plus `targets`, and
// `layouts`; with `frames` above one, each page target needs its mark on two of `frames`
// frames, 1 ms apart.
fn pack(pages: &[(&str, [u8; 3])], frames: u8, targets: Vec<Value>, layouts: Vec<Value>) -> Value {
    let mut all = pages
        .iter()
        .map(|(page, color)| color_target(&format!("page/{page}"), 0, *color))
        .collect::<Vec<_>>();
    all.extend(targets);
    let mut pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": all,
        "candidate_layouts": layouts
    });
    if frames > 1 {
        let samples = (0..frames)
            .map(|frame| json!({"frame": frame}))
            .collect::<Vec<_>>();
        pack["target_consensus"] = pages
            .iter()
            .map(|(page, _)| {
                (
                    format!("page/{page}"),
                    json!({"samples": samples, "k": 2, "sample_interval_ms": 1}),
                )
            })
            .collect::<serde_json::Map<_, _>>()
            .into();
    }
    pack
}

fn pages(pages: &[&str]) -> Value {
    json!({
        "schema_version": "0.6",
        "pages": pages
            .iter()
            .map(|page| json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")]}))
            .collect::<Vec<_>>()
    })
}

// A one-slot layout of `neutral/home`, 1 ms apart: each feature `(name, target, frames)` reads
// its passed verdict of `target` on each of `frames`, all of which must pass.
fn layout(id: &str, features: &[(&str, &str, &[u8])]) -> Value {
    let declared = features
        .iter()
        .map(|(name, _, frames)| {
            let samples = frames
                .iter()
                .map(|frame| json!({"frame": frame}))
                .collect::<Vec<_>>();
            json!({
                "name": name,
                "value": "passed",
                "consensus": {
                    "samples": samples,
                    "aggregate": {"kind": "k_of_n", "k": frames.len()}
                }
            })
        })
        .collect::<Vec<_>>();
    let targets = features
        .iter()
        .map(|(name, target, _)| ((*name).to_owned(), json!(target)))
        .collect::<serde_json::Map<String, Value>>();
    json!({
        "id": id,
        "page_id": "neutral/home",
        "kind": "fixed_slots",
        "sample_interval_ms": 1,
        "features": declared,
        "slots": [{
            "rect": {"x": 0, "y": 0, "width": 2, "height": 1},
            "click": {"x": 0, "y": 0, "width": 2, "height": 1},
            "targets": targets
        }]
    })
}

// The capture-number targets of pixel 1 and the three-frame layout that reads capture 1 on its
// first frame, capture 2 on its second and capture 3 on its third.
fn capture_targets() -> Vec<Value> {
    (1..=3)
        .map(|capture| color_target(&format!("capture/{capture}"), 1, [capture, 0, 0]))
        .collect()
}

fn three_capture_layout() -> Value {
    layout(
        "series",
        &[
            ("first", "capture/1", &[0]),
            ("second", "capture/2", &[1]),
            ("third", "capture/3", &[2]),
        ],
    )
}

// Pixel 0 shows `page(capture)`, pixel 1 the capture's number.
fn script(state: &FakeState, page: fn(usize) -> [u8; 3]) {
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        move |capture: usize, _inputs: usize| -> Vec<(usize, [u8; 3])> {
            vec![
                (0, page(capture)),
                (1, [u8::try_from(capture).expect("capture number"), 0, 0]),
            ]
        },
    ));
}

fn always_home(_capture: usize) -> [u8; 3] {
    HOME
}

// The other page on capture 2, home otherwise.
fn other_on_second(capture: usize) -> [u8; 3] {
    if capture == 2 { OTHER } else { HOME }
}

// The RGB8 hash of the frame capture `capture` shows `page` on.
fn frame_rgb8(page: [u8; 3], capture: u8) -> String {
    format!("{:x}", Sha256::digest([page, [capture, 0, 0]].concat()))
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

// What actinglab hands the client for `package`: the kernel's series plan of its admission.
fn series_plan(package: &Path) -> ObservationSeriesPlan {
    PreparedPageObservation::load(ALIAS, &fs::read(package).expect("package bytes"), &[], None)
        .expect("the package is admitted for observation")
        .series_plan()
}

fn observe(root: &TempDir, package: &Path) -> RuntimeClientResult<VerifiedPageObservation> {
    let client = lab_client(root);
    let session = client.begin_debug_session().expect("debug session");
    session.observe_contained_page(
        ALIAS,
        ContainedObservationRequest::new(package.to_str().expect("path"), Vec::new())
            .expect("observation request"),
        series_plan(package),
    )
}

fn rows<'a>(facts: &'a ObservationFacts, kind: &str) -> Vec<&'a Value> {
    facts
        .rows
        .iter()
        .filter(|row| row["kind"] == kind)
        .collect()
}

// Each layout row of `facts`: its kind, layout, code (a failure's) and coverage.
fn layout_rows(facts: &ObservationFacts) -> Vec<Value> {
    facts
        .rows
        .iter()
        .filter(|row| {
            row["kind"] == "candidate_projection" || row["kind"] == "candidate_projection_failure"
        })
        .map(|row| {
            let layout = if row["kind"] == "candidate_projection" {
                &row["projection"]["layout_id"]
            } else {
                &row["layout_id"]
            };
            json!([row["kind"], layout, row["code"], row["coverage"]])
        })
        .collect()
}

fn observation_evidence(
    root: &TempDir,
    artifact: &ProjectedArtifactReference,
) -> ContainedObservationEvidence {
    serde_json::from_slice(&read_projected_verified(root.path(), artifact).expect("evidence bytes"))
        .expect("observation evidence")
}

// No target consensus (one frame per transaction) and a layout that needs three frames, 1 ms
// apart, reading captures 1, 2 and 3 on its frames: the observation takes captures 2 and 3 after
// its own, projects the layout over the three with coverage {3, 3, production_owner} and every
// feature holding, is recognised, and its frame is capture 1.
#[test]
fn l5b_observe_projects_a_multi_frame_layout_over_its_own_series() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    script(&state, always_home);
    let host = host_with_state(&root, ALIAS, Arc::clone(&state));
    let package = write_package(
        &root,
        pack(
            &[("home", HOME)],
            1,
            capture_targets(),
            vec![three_capture_layout()],
        ),
        pages(&["home"]),
    );
    let verified = observe(&root, &package).expect("observation");
    let observation = verified.observation();
    assert_eq!(observation.status, PageObservationStatus::Recognized);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 3);
    assert_eq!(
        layout_rows(&observation.facts),
        vec![json!(["candidate_projection", "series", null,
                    {"provided_frames": 3, "required_frames": 3,
                     "aggregation": "production_owner"}])]
    );
    let evidence = observation_evidence(&root, &observation.artifact);
    let projection = rows(&evidence.private_facts, "candidate_projection");
    let [projection] = projection.as_slice() else {
        panic!("one projection: {projection:?}");
    };
    let features = &projection["projection"]["candidates"][0]["features"];
    for feature in ["first", "second", "third"] {
        assert_eq!(
            features[feature]["value"],
            json!(true),
            "{feature} reads its own capture: {features}"
        );
    }
    assert_eq!(
        evidence.rgb8_sha256,
        frame_rgb8(HOME, 1),
        "the observation's frame is its own capture"
    );
    host.close().expect("close host");
}

// The layout's series meets the other page on capture 2: the layout's row is a projection
// failure with candidate_sample_page_or_geometry_changed, the observation is partial, and its
// frame is still capture 1.
#[test]
fn l5b_a_series_frame_on_another_page_fails_that_layout() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    script(&state, other_on_second);
    let host = host_with_state(&root, ALIAS, Arc::clone(&state));
    let package = write_package(
        &root,
        pack(
            &[("home", HOME), ("other", OTHER)],
            1,
            capture_targets(),
            vec![three_capture_layout()],
        ),
        pages(&["home", "other"]),
    );
    let verified = observe(&root, &package).expect("observation");
    let observation = verified.observation();
    assert_eq!(observation.status, PageObservationStatus::Partial);
    let failures = rows(&observation.facts, "candidate_projection_failure");
    let [failure] = failures.as_slice() else {
        panic!("one layout failure: {:?}", observation.facts.rows);
    };
    assert_eq!(failure["layout_id"], "series");
    assert_eq!(failure["code"], "candidate_sample_page_or_geometry_changed");
    assert!(rows(&observation.facts, "candidate_projection").is_empty());
    assert_eq!(
        observation_evidence(&root, &observation.artifact).rgb8_sha256,
        frame_rgb8(HOME, 1),
        "the observation's frame is still capture 1"
    );
    host.close().expect("close host");
}

// The five-frame layout `id`, its one feature holding on all five frames.
fn five_frame_layout(id: &str) -> Value {
    layout(id, &[("ready", "guard/ready", &[0, 1, 2, 3, 4])])
}

// Two frames per transaction and three layouts of five frames: the first two series take
// 2 + 8 + 8 = 18 frames; the third would take 26, so it is projected over the current frame
// alone and the recognition pack refuses it; the observation is partial.
#[test]
fn l5b_layouts_beyond_25_frames_are_refused_unavailable() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, ALIAS, Arc::clone(&state));
    let package = write_package(
        &root,
        pack(
            &[("home", HOME)],
            2,
            vec![color_target("guard/ready", 1, READY)],
            vec![
                five_frame_layout("first_layout"),
                five_frame_layout("second_layout"),
                five_frame_layout("third_layout"),
            ],
        ),
        pages(&["home"]),
    );
    let verified = observe(&root, &package).expect("observation");
    let observation = verified.observation();
    assert_eq!(state.capture_count.load(Ordering::Acquire), 18);
    let projected = json!({"provided_frames": 5, "required_frames": 5,
                           "aggregation": "production_owner"});
    assert_eq!(
        layout_rows(&observation.facts),
        vec![
            json!(["candidate_projection", "first_layout", null, projected]),
            json!(["candidate_projection", "second_layout", null, projected]),
            json!(["candidate_projection_failure", "third_layout",
                   "candidate_samples_unavailable",
                   {"provided_frames": 1, "required_frames": 5,
                    "aggregation": "not_completed"}]),
        ]
    );
    assert_eq!(observation.status, PageObservationStatus::Partial);
    host.close().expect("close host");
}

// A Runtime clock whose first armed wait tells the test it is waiting and waits until the
// test lets it go on; every other wait is the system clock's.
struct HeldClock {
    system: SystemRuntimeClock,
    held: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
}

impl RuntimeClock for HeldClock {
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        self.system.sample()
    }

    fn monotonic_instant(&self, origin: Instant, origin_ms: u64) -> RuntimeHostResult<Instant> {
        self.system.monotonic_instant(origin, origin_ms)
    }

    fn pause(&self, duration: Duration) {
        let held = self.held.lock().expect("held wait").take();
        match held {
            Some((waiting, go_on)) => {
                waiting.send(()).expect("announce the held wait");
                // A test that fails before letting it go on ends the wait at `GATE_WAIT`.
                let _ = go_on.recv_timeout(GATE_WAIT);
            }
            None => self.system.pause(duration),
        }
    }
}

// One frame per transaction and a two-frame layout: the observation is held in the layout's wait
// after its own frame; meanwhile another session takes a lease and taps the instance. The series
// does not show the observation's screen: the observation fails with
// `observation_samples_interrupted`, its receipt `failed`.
#[test]
fn l5b_an_input_during_the_series_fails_the_observation() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(HeldClock {
        system: SystemRuntimeClock::new(),
        held: Mutex::new(None),
    });
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    // Armed once the host has started, so the layout's wait is the first it holds.
    let (waiting, held) = mpsc::channel();
    let (go_on, released) = mpsc::channel();
    *clock.held.lock().expect("held wait") = Some((waiting, released));
    let package = write_package(
        &root,
        pack(
            &[("home", HOME)],
            1,
            vec![color_target("guard/ready", 1, READY)],
            vec![layout("choices", &[("ready", "guard/ready", &[0, 1])])],
        ),
        pages(&["home"]),
    );
    let address = host.runtime_info().socket_addr().expect("runtime address");
    let tapper = thread::spawn(move || {
        held.recv_timeout(GATE_WAIT)
            .expect("the observation waits before its series");
        let mut client = TestClient::connect_address(address);
        let (_, token) = client.acquire(ALIAS);
        let tap = client.request(RuntimeOperation::Input {
            frame: None,
            token: token.clone(),
            action: InputAction::Tap { x: 0, y: 0 },
        });
        let tapped = client.send(&tap).state();
        let release = client.request(RuntimeOperation::ReleaseLease { token });
        client.send(&release);
        go_on.send(()).expect("let the observation go on");
        tapped
    });
    let result = observe(&root, &package);
    assert_eq!(
        tapper.join().expect("the tapping session"),
        RuntimeReceiptState::Completed
    );
    let Err(error) = result else {
        panic!("a series that straddles an input does not show the observation's screen");
    };
    assert_eq!(
        error.outcome().code().as_str(),
        "observation_samples_interrupted"
    );
    assert_eq!(
        error.received_receipt().map(RuntimeReceipt::state),
        Some(RuntimeReceiptState::Failed)
    );
    assert_eq!(state.capture_count.load(Ordering::Acquire), 2);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    host.close().expect("close host");
}
