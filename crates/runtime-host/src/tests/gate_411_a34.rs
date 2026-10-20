// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A34 (0.12.x): a late capacity sample must not fail a healthy run. On a fully loaded
//! host (10-20) a long run's frame write was refused `capacity_admission_refused` because the
//! committed capacity sample was 4.2 s old, past its freshness (twice the 2 s sampling
//! interval), on a volume with plenty of space; the run then stayed `running` in the run views
//! although its dispatch had completed. Requirements of the slice brief:
//!
//! - T1 (requirement 1): an admission that finds the committed sample outside freshness takes
//!   one fresh sample, records it as the periodic sampler does (`perf.summary`), and decides on
//!   it, for the business admission and the Artifact byte admission alike;
//! - T2 (requirement 1): when that fresh sample cannot be taken, the admission refuses with the
//!   sample's cause recorded and nothing acts on the refused bytes;
//! - T3 (requirement 2): a run whose capture evidence meets a capacity refusal ends with a
//!   terminal that the run status read (`get_run`) reports, never `running`.
//!
//! The host here has neither the frame cleaner nor the performance counters, so it starts no
//! periodic sampler: only an admission takes a capacity sample once the test has replaced the
//! sampler. The Runtime clock is `ManualRuntimeClock`; a frame capture that takes 4.2 s of it is
//! the late sampler of the incident. Existing capacity tests missed this because none ran a
//! capture whose clock moved past the freshness window without the test sampling capacity itself
//! (`gate_0122_pk1b.rs` `WaitClock` and `gate_382_h4.rs` `CapacitySamplingClock` sample at every
//! move, for this very refusal).

use super::*;
use actingcommand_contract::{
    CapacityAdmissionOutcome, CapacityAdmissionReason, CapacityState, PerformanceCapacitySample,
    PerformancePayload,
};
use actingcommand_host_metrics::{
    CapacitySample, CapacityTarget, CapacityUnavailable, HostMetricsLocation, HostSample,
    HostSampler, ProcessLoadThresholds,
};
use actingcommand_runtime_client::{
    ContainedRunState, RunKey, RunStatusMode, RuntimeChannel, RuntimeClient, RuntimeClientConfig,
};
use std::sync::Mutex;

const ALIAS: &str = "neutral.instance";
const T0: u64 = 1_760_000_000_000;
/// A committed sample is fresh for twice the default 2 s sampling interval.
const FRESHNESS_MS: u64 = 4_000;
/// How old the incident's committed sample was when the frame write was decided.
const LATE_MS: u64 = 4_200;
/// Free bytes far above the default soft threshold (2 GiB).
const SUFFICIENT: u64 = 64 * 1024 * 1024 * 1024;
/// The OS error a free-space query returns for a volume that is not ready.
const NOT_READY: i32 = 21;

/// What the scripted sampler reports for every volume it finds.
#[derive(Debug, Clone, Copy)]
enum Volume {
    Free(u64),
    Unavailable,
}

/// The host's capacity sampler: the actual volumes of the host's targets, each with the free
/// bytes the test scripts, or the OS query failing.
struct ScriptedCapacity {
    volume: Arc<Mutex<Volume>>,
}

impl HostSampler for ScriptedCapacity {
    fn sample_capacity(&mut self, targets: &[CapacityTarget]) -> Vec<CapacitySample> {
        let volume = *self.volume.lock().expect("scripted volume");
        actingcommand_host_metrics::sample_capacity(targets)
            .into_iter()
            .map(|mut sample| {
                if sample.available_bytes.is_ok() {
                    sample.available_bytes = match volume {
                        Volume::Free(bytes) => Ok(bytes),
                        Volume::Unavailable => Err(CapacityUnavailable {
                            operation: HostMetricsLocation::IdentifyCapacityVolume.location(),
                            raw_os_error: Some(NOT_READY),
                            detail: "The device is not ready.".to_owned(),
                        }),
                    };
                }
                sample
            })
            .collect()
    }

    fn sample(
        &mut self,
        _observed_at_unix_ms: u64,
        _owned_processes: &BTreeMap<u32, String>,
        _top_process_count: usize,
        _thresholds: ProcessLoadThresholds,
    ) -> Result<HostSample, actingcommand_contract::outcome::Code> {
        panic!("a capacity-only host enables no performance counters")
    }
}

struct Fixture {
    root: TempDir,
    clock: Arc<ManualRuntimeClock>,
    state: Arc<FakeState>,
    volume: Arc<Mutex<Volume>>,
    host: RuntimeHost,
    package: PathBuf,
}

/// A host at `T0` without a periodic sampler whose committed sample, taken at `T0` by the
/// scripted sampler, finds the volumes sufficient; and the run's package on disk.
fn fixture() -> Fixture {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(T0, 0));
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root)
            .with_frame_retention_enabled(false)
            .with_runtime_clock(clock.clone())
            // The run's lease outlives the 4.2 s its first capture takes on the Runtime clock.
            .with_scheduler(SchedulerConfig {
                maximum_client_heartbeat_interval_ms: 20,
                takeover_cooldown_ms: 40,
                lease_ttl_ms: 60_000,
                ..SchedulerConfig::default()
            }),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    let volume = Arc::new(Mutex::new(Volume::Free(SUFFICIENT)));
    host.replace_capacity_sampler_for_test(Box::new(ScriptedCapacity {
        volume: Arc::clone(&volume),
    }))
    .expect("a sufficient committed sample at T0");
    let package = root.path().join("a34-run.zip");
    fs::write(&package, package_bytes()).expect("write package");
    Fixture {
        root,
        clock,
        state,
        volume,
        host,
        package,
    }
}

/// The neutral route from `home` to `terminal` (one trusted tap), 16x9, whose step and task
/// timeouts leave room for a 4.2 s capture on the Runtime clock.
fn package_bytes() -> Vec<u8> {
    let space = serde_json::json!({"width": 16, "height": 9});
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.semantic.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": space,
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 20_000,
        "timeout_ms": 40_000,
        "max_steps": 2
    });
    let task = serde_json::json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": space,
        "entry_page": "home",
        "target_page": "terminal",
        "operations": [{
            "id": "open_terminal",
            "from": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    let recognition = serde_json::json!({
        "schema_version": "0.3",
        "game": "neutral",
        "server": "test",
        "coordinate_space": space,
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/terminal", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
            {"type": "color", "id": "page/error", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 255, 0]}
        ]
    });
    let pages = serde_json::json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
            {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [], "forbidden": []},
            {"id": "neutral/error", "required": ["page/error"], "optional": [], "forbidden": []}
        ]
    });
    let manifest = serde_json::json!({"schema_version": "0.3", "entry_task_id": "task"});
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

/// The run's first frame capture takes `LATE_MS` of the Runtime clock (a loaded host), after
/// which the scripted volume reads `then` when given: the frame's bytes are admitted on a sample
/// that is no longer fresh.
fn late_first_capture(fixture: &Fixture, then: Option<Volume>) {
    let clock = Arc::clone(&fixture.clock);
    let volume = Arc::clone(&fixture.volume);
    let moved = Arc::new(AtomicBool::new(false));
    *fixture.state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        move |capture: usize, _inputs: usize| -> Vec<(usize, [u8; 3])> {
            if capture == 1 && !moved.swap(true, Ordering::AcqRel) {
                if let Some(then) = then {
                    *volume.lock().expect("scripted volume") = then;
                }
                clock.advance(LATE_MS);
            }
            Vec::new()
        },
    ));
}

/// One direct run of the fixture's package: its request, its receipt and every record.
fn run(fixture: &Fixture) -> (RuntimeRequest, RuntimeReceipt, Vec<PersistedEvent>) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(fixture.package.display().to_string())
                .expect("contained task request"),
        ),
    );
    let receipt = fixture
        .host
        .process_request_for_test(&request, ConnectionId::new(134).expect("connection"))
        .expect("run receipt");
    let events = fixture
        .host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("every record");
    (request, receipt, events)
}

/// The capacity samples recorded (`perf.summary`) that were observed at or after `unix_ms`,
/// with their records' sequences.
fn samples_recorded_from(
    events: &[PersistedEvent],
    unix_ms: u64,
) -> Vec<(u64, PerformanceCapacitySample)> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Performance(PerformancePayload::Summary(payload)) => payload
                .capacity()
                .filter(|sample| sample.observed_at_unix_ms >= unix_ms)
                .map(|sample| (event.sequence(), sample.clone())),
            _ => None,
        })
        .collect()
}

/// T1: a committed sample older than freshness and a sufficient volume, met first by the run's
/// business admission (the clock moved before the run) and by its first frame's Artifact byte
/// admission (the clock moved while that frame was captured). Each admission takes one fresh
/// sample, records it as a new `perf.summary`, and allows: the run completes.
#[test]
fn a34_t1_a_late_sample_is_resampled_recorded_and_the_admission_allows() {
    for at_frame in [false, true] {
        let fixture = fixture();
        if at_frame {
            late_first_capture(&fixture, None);
        } else {
            fixture.clock.advance(LATE_MS);
        }
        let (_, receipt, events) = run(&fixture);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "at_frame={at_frame}: {receipt:?}"
        );
        assert!(
            matches!(
                receipt.result(),
                Some(RuntimeResult::ContainedTaskCompleted {
                    task_result: TaskOutcome::Success,
                    ..
                })
            ),
            "at_frame={at_frame}: {receipt:?}"
        );
        // The one sample taken after the clock moved past freshness was recorded.
        let recorded = samples_recorded_from(&events, T0 + LATE_MS);
        assert_eq!(recorded.len(), 1, "at_frame={at_frame}: {recorded:?}");
        let (_, sample) = &recorded[0];
        assert_eq!(sample.freshness_ms, FRESHNESS_MS, "at_frame={at_frame}");
        assert!(
            !sample.volumes.is_empty()
                && sample.volumes.iter().all(|volume| {
                    volume.available_bytes == Some(SUFFICIENT)
                        && volume.state == CapacityState::Sufficient
                        && volume.cause.is_none()
                }),
            "at_frame={at_frame}: {sample:?}"
        );
        assert!(
            events
                .iter()
                .all(|event| event.event_type() != EventType::ArtifactStoreFailed),
            "at_frame={at_frame}: no Artifact write was refused"
        );
        fixture.host.close().expect("close host");
    }
}

/// T2: a committed sample older than freshness when the first frame's bytes are admitted, and
/// the fresh sample cannot be taken (the volume's free-space query fails). The admission
/// refuses: the run acts on nothing after that frame, and the refusal names the sample it took,
/// recorded with the volume's cause.
#[test]
fn a34_t2_a_resample_that_cannot_be_taken_refuses_with_its_cause() {
    let fixture = fixture();
    late_first_capture(&fixture, Some(Volume::Unavailable));
    let (_, receipt, events) = run(&fixture);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(
        receipt.outcome().code().as_str(),
        "capacity_admission_refused",
        "{receipt:?}"
    );
    assert_eq!(
        fixture.state.input_count.load(Ordering::Acquire),
        0,
        "nothing acts on a frame whose bytes were refused"
    );
    let refusals = events
        .iter()
        .filter(|event| event.event_type() == EventType::ArtifactStoreFailed)
        .collect::<Vec<_>>();
    assert_eq!(refusals.len(), 1, "{refusals:?}");
    let decision = refusals[0]
        .payload()
        .artifact_failure()
        .and_then(|failure| failure.capacity.clone())
        .expect("the refused write's capacity decision");
    assert_eq!(decision.admission_result, CapacityAdmissionOutcome::Unknown);
    assert_eq!(decision.reason, CapacityAdmissionReason::SampleUnavailable);
    let fact = decision
        .fact
        .expect("the decision names the sample it took");
    let recorded = samples_recorded_from(&events, T0 + LATE_MS);
    let (_, sample) = recorded
        .iter()
        .find(|(sequence, _)| *sequence == fact.sequence)
        .expect("the named sample is the one recorded after the clock moved");
    assert_eq!(sample.observed_at_unix_ms, fact.observed_at_unix_ms);
    assert!(
        !sample.volumes.is_empty()
            && sample.volumes.iter().all(|volume| {
                volume.available_bytes.is_none()
                    && volume.state == CapacityState::Unknown
                    && volume
                        .cause
                        .as_ref()
                        .is_some_and(|cause| cause.code().as_str() == "foreign_os_error")
            }),
        "{sample:?}"
    );
    fixture.host.close().expect("close host");
}

/// T3: the run's first frame write is refused for capacity (the volume is down to nothing when
/// its bytes are admitted), so its capture evidence cannot be finished. The run still commits
/// its terminal, and the run status read reports it failed with the refusal, not running.
#[test]
fn a34_t3_a_run_whose_evidence_meets_a_capacity_refusal_reads_failed() {
    let fixture = fixture();
    late_first_capture(&fixture, Some(Volume::Free(0)));
    let (request, receipt, _) = run(&fixture);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(
        receipt.outcome().code().as_str(),
        "capacity_admission_refused",
        "{receipt:?}"
    );
    let status = RuntimeClient::connect(RuntimeClientConfig::new(
        fixture.root.path(),
        RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("run status client")
    .contained_run_status(
        RunKey::RequestId(request.request_id()),
        RunStatusMode::Brief,
    )
    .expect("run status");
    assert_eq!(status.state, ContainedRunState::Failed, "{status:?}");
    let terminal = status.terminal.as_ref().expect("the run's terminal");
    assert_eq!(terminal.outcome, TaskOutcome::Failure, "{status:?}");
    assert_eq!(
        terminal.failure_code.as_ref().map(|code| code.as_str()),
        Some("capacity_admission_refused"),
        "{status:?}"
    );
    fixture.host.close().expect("close host");
}
