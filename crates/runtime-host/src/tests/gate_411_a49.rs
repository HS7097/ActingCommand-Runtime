// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A49 (0.12.9; production host 10-21 11:28, v0.12.7): a read-only observe that lands on an
//! instance between a run's recognition and its input must not displace the frame the run
//! decided on. There, the observe (MCP `ac_observe`) committed its own frame as the instance's
//! one input frame, and the run's tap was then refused `input_frame_unavailable` when it resolved
//! the frame its recognition used. Requirements of the slice (coordinator 10-21):
//!
//! - T1: recognition, then an observe, then the run's input: the input resolves against the
//!   frame its own recognition used, the click is issued and the run completes;
//! - T2: that observe still captures a fresh frame of its own.
//!
//! The interleaving is made deterministic by the step's pre-input wait (#385 P-k), which sits
//! after the step's recognition and before its input: the instance's pacing factor is pinned at
//! 2 000 and `pre_input_wait_base_ms` set, so the step waits `PRE_INPUT_WAIT`, and the Runtime
//! clock holds that one wait until the observe has returned. The fake instance shows `home`
//! until the first input and `next` after it on pixel 0, and each capture's number on pixel 1.
//! Existing tests missed this because no test observed an instance while a run on it was between
//! its recognition and its input: the observe tests run on an idle instance or under their own
//! lease, and the run tests have no second client.

use super::*;
use crate::{RuntimeClock, RuntimeClockSample, SystemRuntimeClock};
use serde_json::json;
use std::sync::Mutex;

// Bounds only a broken run: the held wait and the observing thread meet on channels.
const GATE_WAIT: Duration = Duration::from_secs(30);
const ALIAS: &str = "neutral.instance";
const HOME: [u8; 3] = [255, 0, 0];
const NEXT: [u8; 3] = [0, 0, 255];
const PACED_FACTOR_MILLI: u32 = 2_000;
const PRE_INPUT_WAIT_BASE_MS: u64 = 1_234;
// The base × (2 000 − 1 000) / 1 000.
const PRE_INPUT_WAIT: Duration = Duration::from_millis(PRE_INPUT_WAIT_BASE_MS);

// A Runtime clock that holds the first wait of `PRE_INPUT_WAIT` once it is armed: it tells the
// test it is waiting, waits until the test lets it go on and then returns at once. Every other
// wait is the system clock's.
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
        let held = if duration == PRE_INPUT_WAIT {
            self.held.lock().expect("held wait").take()
        } else {
            None
        };
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

/// A configuration that sets `performance.pacing.pre_input_wait_base_ms` and nothing else.
fn pre_input_wait_base(base_ms: u64) -> actingcommand_contract::RuntimeConfigManifest {
    let mut parameters = vec![actingcommand_contract::ConfigParameter {
        key: "performance.pacing.pre_input_wait_base_ms".to_owned(),
        value: actingcommand_contract::FactScalar::DurationMs(base_ms),
        source: actingcommand_contract::ConfigParameterSource::Explicit,
        settable: false,
    }];
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    actingcommand_contract::RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// A page-graph package from `home` to `next` (told apart by the frame's first pixel) whose one
/// operation taps once, a frame every millisecond.
fn one_tap_package() -> Vec<u8> {
    let task = json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "next",
        "operations": [{
            "id": "open_next",
            "from": "home",
            "to": "next",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    let target = |id: &str, expected: [u8; 3]| {
        json!({
            "type": "color",
            "id": id,
            "region": {"x": 0, "y": 0, "width": 1, "height": 1},
            "expected": expected
        })
    };
    let recognition = json!({
        "schema_version": "0.3",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [target("page/home", HOME), target("page/next", NEXT)]
    });
    let page = |id: &str, required: &str| json!({"id": id, "required": [required], "optional": [], "forbidden": []});
    let pages = json!({
        "schema_version": "0.3",
        "pages": [page("neutral/home", "page/home"), page("neutral/next", "page/next")]
    });
    let control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.a49.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 5_000,
        "timeout_ms": 60_000,
        "max_steps": 2
    });
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in [
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", recognition),
        ("resources/recognition/neutral.test.pages.json", pages),
    ] {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(&value).expect("package JSON"))
            .expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// The run, the observe that landed between its recognition and its input, and what the fake
/// instance counted around the observe.
struct Interleaving {
    root: TempDir,
    state: Arc<FakeState>,
    host: RuntimeHost,
    run: RuntimeReceipt,
    run_events: Vec<PersistedEvent>,
    observed: RuntimeReceipt,
    captures_before_observe: usize,
    captures_after_observe: usize,
}

/// A direct run of the one-tap package, held in its step's pre-input wait while a keyless
/// read-only observe of the same instance runs to its receipt; then the run goes on.
fn observe_between_recognition_and_input() -> Interleaving {
    let root = TempDir::new().expect("tempdir");
    let path = root.path().join("one-tap.zip");
    fs::write(&path, one_tap_package()).expect("write package");
    let state = Arc::new(FakeState::default());
    // Workflow #392 DA-A: a direct run needs an observed display.
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        |capture: usize, inputs: usize| -> Vec<(usize, [u8; 3])> {
            vec![
                (0, if inputs == 0 { HOME } else { NEXT }),
                (1, [u8::try_from(capture).unwrap_or(u8::MAX), 0, 0]),
            ]
        },
    ));
    let clock = Arc::new(HeldClock {
        system: SystemRuntimeClock::new(),
        held: Mutex::new(None),
    });
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_config_manifest(pre_input_wait_base(PRE_INPUT_WAIT_BASE_MS)),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    host.pin_pacing_factor_for_test(ALIAS, PACED_FACTOR_MILLI)
        .expect("pin the factor");
    let (waiting, held) = mpsc::channel();
    let (go_on, released) = mpsc::channel();
    *clock.held.lock().expect("held wait") = Some((waiting, released));

    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run_request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(path.display().to_string())
                .expect("contained task request")
                .with_response_deadline_ms(60_000)
                .expect("bounded response deadline"),
        ),
    );
    let observe_request = runtime_request(
        &ids,
        RuntimeOperation::ObserveReadonly {
            instance_alias: ALIAS.to_owned(),
        },
    );
    let (run, observed, captures_before_observe, captures_after_observe) = thread::scope(|scope| {
        // Dropped if this side fails, which ends the held wait at once.
        let go_on = go_on;
        let runner = scope.spawn(|| {
            host.process_request_for_test(
                &run_request,
                ConnectionId::new(4_901).expect("connection"),
            )
            .expect("the run's receipt")
        });
        held.recv_timeout(GATE_WAIT)
            .expect("the run waits after its recognition, before its input");
        let before = state.capture_count.load(Ordering::Acquire);
        let observed = host
            .process_request_for_test(
                &observe_request,
                ConnectionId::new(4_902).expect("connection"),
            )
            .expect("the observe's receipt");
        let after = state.capture_count.load(Ordering::Acquire);
        go_on.send(()).expect("let the run go on");
        (runner.join().expect("the run"), observed, before, after)
    });
    let run_events = host
        .query_persisted_events_for_test(EventQuery {
            request_id: Some(run_request.request_id()),
            ..EventQuery::default()
        })
        .expect("the run's records");
    Interleaving {
        root,
        state,
        host,
        run,
        run_events,
        observed,
        captures_before_observe,
        captures_after_observe,
    }
}

/// The frame ID the observe's receipt names.
fn observed_frame_id(observed: &RuntimeReceipt) -> actingcommand_contract::FrameId {
    let Some(RuntimeResult::ReadonlyObservationCompleted { observation }) = observed.result()
    else {
        panic!("the observation: {observed:?}");
    };
    observation
        .artifact()
        .frame_id
        .expect("an observed frame has its ID")
}

// T1: the run recognised `home`, an observe of the instance took its own frame, and then the
// run's input resolves against the frame its own recognition used: the run completes, its one
// click is issued, and its input names the frame its step was decided on, not the observe's.
#[test]
fn a49_t1_a_runs_input_resolves_its_own_frame_after_an_observe() {
    let interleaving = observe_between_recognition_and_input();
    assert_eq!(
        interleaving.run.state(),
        RuntimeReceiptState::Completed,
        "the run goes on after the observe: {:?}",
        interleaving.run
    );
    assert_eq!(
        interleaving.state.input_count.load(Ordering::Acquire),
        1,
        "the run's click is issued"
    );
    let decided_on = interleaving
        .run_events
        .iter()
        .filter(|event| event.event_type() == EventType::TaskEffectIntent)
        .map(|event| event.links().frame_id().copied())
        .collect::<Vec<_>>();
    let input_frames = interleaving
        .run_events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Input(InputPayload::Intent(intent)) => Some(
                intent
                    .provenance()
                    .and_then(|provenance| provenance.before_frame_id),
            ),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [Some(decided_on)] = decided_on.as_slice() else {
        panic!("one step decided on one frame: {decided_on:?}");
    };
    assert_eq!(
        input_frames,
        vec![Some(*decided_on)],
        "the run's input is made in the frame its step was decided on"
    );
    assert_ne!(
        *decided_on,
        observed_frame_id(&interleaving.observed),
        "the run's frame is not the observe's"
    );
    interleaving.host.close().expect("close host");
}

// T2: the observe that lands while the run waits before its input captures a fresh frame of its
// own: the instance captures once more for it, and its frame shows that capture, not the frame
// the run decided on.
#[test]
fn a49_t2_the_observe_still_takes_a_fresh_frame_of_its_own() {
    let interleaving = observe_between_recognition_and_input();
    assert_eq!(
        interleaving.observed.state(),
        RuntimeReceiptState::Completed,
        "the observe: {:?}",
        interleaving.observed
    );
    assert_eq!(
        interleaving.captures_after_observe,
        interleaving.captures_before_observe + 1,
        "the observe captures one frame of its own"
    );
    let Some(RuntimeResult::ReadonlyObservationCompleted { observation }) =
        interleaving.observed.result()
    else {
        panic!("the observation: {:?}", interleaving.observed);
    };
    let png = read_projected_verified(interleaving.root.path(), observation.artifact())
        .expect("the observed frame's bytes");
    let frame = Frame::from_png(png, CaptureBackendName::AdbScreencap).expect("decoded frame");
    // RGBA: pixel 1's red channel is the number of the capture that took the frame.
    assert_eq!(
        frame.pixels.get(4).copied().map(usize::from),
        Some(interleaving.captures_after_observe),
        "the observed frame is the observe's own capture"
    );
    interleaving.host.close().expect("close host");
}
