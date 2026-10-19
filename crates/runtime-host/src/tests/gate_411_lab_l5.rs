// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab L5a (host key 120; register §1.130 L5-1 to L5-5; coordinator decisions Q2-Q5 of
// 2026-10-19): every online Lab observation takes the frames its package's target consensus
// needs as one transaction. `observe --capture` and each observation of `do --capture` capture
// the earlier frames and then the current frame, the declared interval apart, and the kernel
// evaluates the set as a task's observation does; the last frame is the observation's frame.
// A keyless observe whose frames straddle an input fails with `observation_samples_interrupted`;
// a do's observations are page observations, which skip a layout that needs more frames; the
// shared evidence verifier accounts for every earlier frame. The fake instance's 2x1 frames are
// scripted per capture (`FakeState.pixel_script`): pixel 0 shows the page, pixel 1 the capture's
// number. Packages are synthetic and neutral; every interval is 1 ms.

use super::*;
use crate::{RuntimeClock, RuntimeClockSample, SystemRuntimeClock};
use actingcommand_contract::page_projection::ElementRole;
use actingcommand_contract::{
    ContainedLabOperationRequest, ContainedObservationEvidence, ContainedObservationRequest,
    LabArrivalCondition, LabArrivalStatus, LabOperationEvidence, LabOperationSelection,
    LabProjectionHint, ObservationFacts, ObservationFramePlan, PageObservationStatus,
    verify_lab_operation_evidence,
};
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::sync::Mutex;

// Bounds only a broken run: the held transaction and the tapping thread meet on channels.
const GATE_WAIT: Duration = Duration::from_secs(30);
const ALIAS: &str = "node.a";
const HOME: [u8; 3] = [255, 0, 0];
const NEXT: [u8; 3] = [0, 0, 255];
const ABSENT: [u8; 3] = [0, 255, 255];

fn json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON")
}

// An observation package (the Lab resource layout) with `pack`, `pages` and `navigation`,
// written into `root`.
fn write_package(root: &TempDir, pack: Value, pages: Value, navigation: Value) -> PathBuf {
    let entries = [
        (
            "control.json",
            json!({"game": "neutral", "server": "test", "package_id": "neutral.lab_l5",
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
            navigation,
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in &entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(&json_bytes(value)).expect("zip bytes");
    }
    let path = root.path().join("lab-l5.zip");
    fs::write(&path, zip.finish().expect("finish zip").into_inner()).expect("write package");
    path
}

fn color_target(id: &str, x: u32, expected: [u8; 3]) -> Value {
    json!({"type": "color", "id": id,
           "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
}

// The recognition pack: one page target per page colour of pixel 0; with `frames` above one,
// each page target needs its mark on two of `frames` frames, 1 ms apart.
fn pack(pages: &[(&str, [u8; 3])], frames: u8) -> Value {
    let mut pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": pages
            .iter()
            .map(|(page, color)| color_target(&format!("page/{page}"), 0, *color))
            .collect::<Vec<_>>()
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

fn no_navigation() -> Value {
    json!({"navigation": []})
}

// Pixel 0 shows `page(capture, inputs)`, pixel 1 the capture's number.
fn script(state: &FakeState, page: fn(usize, usize) -> [u8; 3]) {
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        move |capture: usize, inputs: usize| -> Vec<(usize, [u8; 3])> {
            vec![
                (0, page(capture, inputs)),
                (1, [u8::try_from(capture).expect("capture number"), 0, 0]),
            ]
        },
    ));
}

// The page shown on captures 1 and 3, not on capture 2.
fn shown_on_first_and_third(capture: usize, _inputs: usize) -> [u8; 3] {
    if capture == 2 { ABSENT } else { HOME }
}

// The page shown on capture 3 only.
fn shown_on_third(capture: usize, _inputs: usize) -> [u8; 3] {
    if capture == 3 { HOME } else { ABSENT }
}

// `home` until the first input, `next` after it.
fn next_after_input(_capture: usize, inputs: usize) -> [u8; 3] {
    if inputs > 0 { NEXT } else { HOME }
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

fn rows<'a>(facts: &'a ObservationFacts, kind: &str) -> Vec<&'a Value> {
    facts
        .rows
        .iter()
        .filter(|row| row["kind"] == kind)
        .collect()
}

fn coverage(facts: &ObservationFacts) -> Vec<Value> {
    rows(facts, "recognition_coverage")
        .into_iter()
        .map(|row| {
            json!([
                row["provided_frames"],
                row["required_frames"],
                row["aggregation"]
            ])
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

// Target consensus of two of three frames: the page shown on captures 1 and 3 is recognised
// over the three frames the observation takes, with its coverage, and its frame is capture 3;
// the page shown on capture 3 alone is not.
#[test]
fn l5_observe_takes_the_consensus_frames_and_recognizes_on_their_agreement() {
    for (case, page, expected) in [
        (
            "on captures 1 and 3",
            shown_on_first_and_third as fn(usize, usize) -> [u8; 3],
            PageObservationStatus::Recognized,
        ),
        (
            "on capture 3 only",
            shown_on_third as fn(usize, usize) -> [u8; 3],
            PageObservationStatus::NoMatch,
        ),
    ] {
        let root = TempDir::new().expect("tempdir");
        let state = Arc::new(FakeState::default());
        script(&state, page);
        let host = host_with_state(&root, ALIAS, Arc::clone(&state));
        let package = write_package(
            &root,
            pack(&[("home", HOME)], 3),
            pages(&["home"]),
            no_navigation(),
        );
        let client = lab_client(&root);
        let session = client.begin_debug_session().expect("debug session");
        let verified = session
            .observe_contained_page(
                ALIAS,
                ContainedObservationRequest::new(package.to_str().expect("path"), Vec::new())
                    .expect("observation request"),
                ObservationFramePlan::new(3, 1).expect("three frames"),
            )
            .unwrap_or_else(|error| panic!("{case}: {error:?}"));
        let observation = verified.observation();
        assert_eq!(observation.status, expected, "{case}");
        assert_eq!(state.capture_count.load(Ordering::Acquire), 3, "{case}");
        assert_eq!(
            coverage(&observation.facts),
            vec![json!([3, 3, "production_owner"])],
            "{case}"
        );
        let evidence = observation_evidence(&root, &observation.artifact);
        assert_eq!(
            evidence.rgb8_sha256,
            frame_rgb8(HOME, 3),
            "{case}: the observation's frame is the third capture"
        );
        drop(session);
        drop(client);
        host.close().expect("close host");
    }
}

// A package without target consensus takes one frame: one capture, no coverage row, and the
// records of one single-frame observation.
#[test]
fn l5_a_package_without_consensus_observes_one_frame_as_before() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, ALIAS, Arc::clone(&state));
    let package = write_package(
        &root,
        pack(&[("home", HOME)], 1),
        pages(&["home"]),
        no_navigation(),
    );
    let client = lab_client(&root);
    let session = client.begin_debug_session().expect("debug session");
    let verified = session
        .observe_contained_page(
            ALIAS,
            ContainedObservationRequest::new(package.to_str().expect("path"), Vec::new())
                .expect("observation request"),
            ObservationFramePlan::SINGLE,
        )
        .expect("observation");
    assert_eq!(
        verified.observation().status,
        PageObservationStatus::Recognized
    );
    assert_eq!(state.capture_count.load(Ordering::Acquire), 1);
    assert!(coverage(&verified.observation().facts).is_empty());
    let events = session
        .query_events(ProjectionProfile::Forensic)
        .expect("observation events");
    let types = events
        .iter()
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
    assert_events_in_order(
        &types,
        &[
            EventType::SchedulerAdmitted,
            EventType::CaptureRequested,
            EventType::RecognitionRequested,
            EventType::ArtifactCreated,
            EventType::ArtifactVerified,
            EventType::CaptureCompleted,
            EventType::RecognitionCompleted,
            EventType::RecognitionRequested,
            EventType::ArtifactCreated,
            EventType::ArtifactVerified,
            EventType::RecognitionCompleted,
        ],
    );
    for kind in [EventType::CaptureRequested, EventType::CaptureCompleted] {
        assert_eq!(
            types.iter().filter(|recorded| **recorded == kind).count(),
            1,
            "{kind:?} in {types:?}"
        );
    }
    drop(session);
    drop(client);
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

// The observation is held in its wait after the first of its two frames; meanwhile another
// session takes a lease and taps the instance. The frames are not one screen: the observation
// fails with `observation_samples_interrupted`, its receipt `failed`, and nothing is recognised.
#[test]
fn l5_an_input_between_the_sample_frames_fails_the_observation() {
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
    // Armed once the host has started, so the observation's wait is the first it holds.
    let (waiting, held) = mpsc::channel();
    let (go_on, released) = mpsc::channel();
    *clock.held.lock().expect("held wait") = Some((waiting, released));
    let package = write_package(
        &root,
        pack(&[("home", HOME)], 2),
        pages(&["home"]),
        no_navigation(),
    );
    let address = host.runtime_info().socket_addr().expect("runtime address");
    let tapper = thread::spawn(move || {
        held.recv_timeout(GATE_WAIT)
            .expect("the observation waits after its first frame");
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
    let client = lab_client(&root);
    let session = client.begin_debug_session().expect("debug session");
    let result = session.observe_contained_page(
        ALIAS,
        ContainedObservationRequest::new(package.to_str().expect("path"), Vec::new())
            .expect("observation request"),
        ObservationFramePlan::new(2, 1).expect("two frames"),
    );
    assert_eq!(
        tapper.join().expect("the tapping session"),
        RuntimeReceiptState::Completed
    );
    let Err(error) = result else {
        panic!("frames that straddle an input are not one screen");
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
    let events = session
        .query_events(ProjectionProfile::Forensic)
        .expect("observation events");
    assert!(
        !events.iter().any(|event| event
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == ArtifactKind::DiagnosticJson)),
        "nothing is recognised from the frames"
    );
    drop(session);
    drop(client);
    host.close().expect("close host");
}

// A do on `home` whose arrival page is `next`, both needing two of three frames: the before
// observation and the arrival sample each take three frames.
struct ConsensusDo {
    root: TempDir,
    state: Arc<FakeState>,
    host: RuntimeHost,
    evidence: LabOperationEvidence,
}

fn consensus_do() -> ConsensusDo {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    script(&state, next_after_input);
    let host = host_with_state(&root, ALIAS, Arc::clone(&state));
    let package = write_package(
        &root,
        pack(&[("home", HOME), ("next", NEXT)], 3),
        pages(&["home", "next"]),
        no_navigation(),
    );
    let client = lab_client(&root);
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
            ObservationFramePlan::new(3, 1).expect("three frames"),
        )
        .expect("the client verifies the operation");
    let operation = verified.operation().clone();
    let prepared = &operation.record.prepared;
    let query = |query: EventQuery| {
        client
            .query_events(query, ProjectionProfile::Forensic)
            .expect("operation events")
    };
    let events = query(EventQuery {
        request_id: Some(prepared.request_id),
        correlation_id: Some(prepared.correlation_id),
        ..EventQuery::default()
    });
    let lease_events = query(EventQuery {
        lease_id: prepared.lease_id,
        ..EventQuery::default()
    });
    let action_events = query(EventQuery {
        action_id: Some(operation.record.input_action_id.expect("the tap's action")),
        ..EventQuery::default()
    });
    let artifacts = events
        .iter()
        .filter(|event| event.event_type == EventType::ArtifactVerified)
        .flat_map(|event| &event.artifacts)
        .map(|reference| {
            (
                reference.artifact_id,
                read_projected_verified(root.path(), reference).expect("verified artifact"),
            )
        })
        .collect();
    let evidence = LabOperationEvidence {
        operation,
        terminal: verified.receipt().terminal().expect("terminal"),
        events,
        lease_events,
        action_events,
        artifacts,
    };
    drop(client);
    ConsensusDo {
        root,
        state,
        host,
        evidence,
    }
}

// The before observation takes three frames and its frame is the third capture; after the tap
// the arrival page is reached on the next transaction's frames; the evidence of the request's
// events verifies.
#[test]
fn l5_do_takes_the_transaction_for_its_before_and_arrival_observations_and_its_evidence_verifies() {
    let run = consensus_do();
    let record = &run.evidence.operation.record;
    assert!(record.failure.is_none(), "{:?}", record.failure);
    let before = record
        .prepared
        .before_projection
        .as_ref()
        .expect("the before projection");
    assert_eq!(before.status, PageObservationStatus::Recognized);
    assert_eq!(before.projection.page, "neutral/home");
    assert_eq!(
        coverage(&before.facts),
        vec![json!([3, 3, "production_owner"])]
    );
    assert_eq!(
        observation_evidence(&run.root, &before.artifact).rgb8_sha256,
        frame_rgb8(HOME, 3),
        "the before frame is the third capture"
    );
    let arrival = record.arrival.as_ref().expect("the arrival");
    assert_eq!(arrival.status, LabArrivalStatus::Reached);
    assert_eq!(arrival.samples.len(), 1);
    assert_eq!(run.state.capture_count.load(Ordering::Acquire), 6);
    verify_lab_operation_evidence(&run.evidence).expect("the operation's evidence verifies");
    run.host.close().expect("close host");
}

// `evidence` with one more frame of the request: a copy, under a new artifact ID, of the first
// frame no observation references (an earlier frame of the before observation), recorded at
// `sequences` (its created and verified records), or where the copied frame was.
fn with_copied_frame(
    evidence: &LabOperationEvidence,
    sequences: Option<(u64, u64)>,
) -> LabOperationEvidence {
    let record = &evidence.operation.record;
    let mut referenced = BTreeSet::new();
    for frame in [&record.prepared.before_frame, &record.after_frame]
        .into_iter()
        .flatten()
    {
        referenced.insert(frame.observation.artifact().artifact_id);
    }
    for sample in &record.arrival.as_ref().expect("the arrival").samples {
        referenced.insert(sample.frame.observation.artifact().artifact_id);
    }
    let verified = evidence
        .events
        .iter()
        .filter(|event| {
            event.event_type == EventType::ArtifactVerified
                && event.artifacts.len() == 1
                && event.artifacts[0].kind == ArtifactKind::CaptureFrame
                && !referenced.contains(&event.artifacts[0].artifact_id)
        })
        .min_by_key(|event| event.sequence)
        .expect("an earlier frame");
    let original = verified.artifacts[0].artifact_id;
    let created = evidence
        .events
        .iter()
        .find(|event| {
            event.event_type == EventType::ArtifactCreated
                && event.artifacts.len() == 1
                && event.artifacts[0].artifact_id == original
        })
        .expect("its created record");
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let copy = *issuer.mint_artifact_id().expect("artifact id").transport();
    let mut changed = evidence.clone();
    for (index, event) in [created, verified].into_iter().enumerate() {
        let mut event = event.clone();
        event.artifacts[0].artifact_id = copy;
        event.event_id = *issuer.mint_event_id().expect("event id").transport();
        if let Some((created_at, verified_at)) = sequences {
            event.sequence = if index == 0 { created_at } else { verified_at };
        }
        changed.events.push(event);
    }
    changed
        .artifacts
        .insert(copy, evidence.artifacts[&original].clone());
    changed
}

// A frame beyond `provided_frames - 1` in the before observation's transaction, and one recorded
// before the lease grant, are each a frame no observation accounts for.
#[test]
fn l5_the_verifier_refuses_a_frame_no_observation_accounts_for() {
    let run = consensus_do();
    verify_lab_operation_evidence(&run.evidence).expect("the operation's evidence verifies");
    let grant = run
        .evidence
        .events
        .iter()
        .find(|event| event.event_type == EventType::LeaseGranted)
        .expect("the lease grant")
        .sequence;
    for (case, sequences) in [
        ("one frame too many in the before transaction", None),
        (
            "a frame before the lease grant",
            Some((grant - 2, grant - 1)),
        ),
    ] {
        let changed = with_copied_frame(&run.evidence, sequences);
        let Err(error) = verify_lab_operation_evidence(&changed) else {
            panic!("{case}: the frame is accounted for");
        };
        assert_eq!(
            error.code().as_str(),
            "runtime_lab_frame_budget_mismatch",
            "{case}"
        );
    }
    run.host.close().expect("close host");
}

// A page with a candidate layout that needs two frames: do's before observation is a page
// observation, so the layout is not projected; the observation is recognised, the page's
// element is actionable and pressed, and the layout's row says it was skipped.
#[test]
fn l5_do_observations_skip_multi_frame_layouts_and_stay_complete() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, ALIAS, Arc::clone(&state));
    let mut recognition = pack(&[("home", HOME), ("next", NEXT)], 1);
    recognition["targets"]
        .as_array_mut()
        .expect("targets")
        .push(color_target("guard/ready", 1, [0, 255, 0]));
    recognition["candidate_layouts"] = json!([{
        "id": "choices",
        "page_id": "neutral/home",
        "kind": "fixed_slots",
        "sample_interval_ms": 1,
        "features": [{
            "name": "ready",
            "value": "passed",
            "consensus": {"samples": [{"frame": 0}, {"frame": 1}],
                          "aggregate": {"kind": "k_of_n", "k": 2}}
        }],
        "slots": [{
            "rect": {"x": 0, "y": 0, "width": 2, "height": 1},
            "click": {"x": 0, "y": 0, "width": 2, "height": 1},
            "targets": {"ready": "guard/ready"}
        }]
    }]);
    let package = write_package(
        &root,
        recognition,
        pages(&["home", "next"]),
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
    );
    let element = serde_json::to_string(&(ElementRole::Navigate, "", "neutral/spec", "to_next"))
        .expect("element ID");
    let client = lab_client(&root);
    let verified = client
        .begin_debug_session()
        .expect("debug session")
        .run_contained_lab_operation(
            ALIAS,
            ContainedLabOperationRequest {
                package_path: package.to_str().expect("path").to_owned(),
                selection: LabOperationSelection::Element { id: element },
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
    assert!(record.failure.is_none(), "{:?}", record.failure);
    let before = record
        .prepared
        .before_projection
        .as_ref()
        .expect("the before projection");
    assert_eq!(before.status, PageObservationStatus::Recognized);
    assert_eq!(before.projection.page, "neutral/home");
    assert_eq!(
        rows(&before.facts, "candidate_projection_skipped"),
        vec![
            &json!({"kind": "candidate_projection_skipped", "layout_id": "choices",
                     "coverage": {"provided_frames": 1, "required_frames": 2,
                                  "aggregation": "not_requested"}})
        ]
    );
    assert!(rows(&before.facts, "candidate_projection_failure").is_empty());
    assert_eq!(record.effect, EffectDisposition::Performed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    drop(client);
    host.close().expect("close host");
}
