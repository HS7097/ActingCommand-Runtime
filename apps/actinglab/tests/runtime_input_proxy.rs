// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    ApplicationLifecycleAction, ArtifactKind, CaptureSequenceSpec, EventAction, EventActor,
    EventPayload, EventQuery, EventSource, EventType, IdentifierIssuer, InstanceId,
    PackageDebugRequest, ProjectionPayload, ProjectionProfile, RetentionClass, RuntimeErrorCode,
    RuntimeEvidenceExportRequest, TaskOutcome, TaskPayload, TaskSemanticFact,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceResult, Frame, InputBackend, PixelFormat,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use actingcommand_runtime_host::{
    ExecutionBackendProvider, ResolvedExecutionInstance, RuntimeClock, RuntimeClockSample,
    RuntimeHost, RuntimeHostConfig, RuntimeHostResult,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::FileOptions;

#[derive(Default)]
struct FakeState {
    taps: AtomicUsize,
    captures: AtomicUsize,
    closes: AtomicUsize,
    application_calls: AtomicUsize,
    application_action: AtomicUsize,
    transition_after_tap: AtomicBool,
    tap_started: AtomicBool,
    tap_delay_ms: AtomicUsize,
    /// Serve 16x9 frames with a matching display geometry for Runtime-owned Task runs.
    physical_task_geometry: AtomicBool,
    // Test plan E-3 G11 (§3.10): from this capture number on (when not zero) the transition after
    // a tap is over and the page shows again.
    home_from_capture: AtomicUsize,
    // Test plan E-3 G11: the host's manual Runtime clock, and how far each capture moves it.
    clock: OnceLock<Arc<ManualClock>>,
    clock_ms_per_capture: AtomicU64,
    // #411 Lab L3: the long taps and swipes the fake performed; a long tap starts the transition
    // after a press as a tap does.
    long_taps: AtomicUsize,
    swipes: AtomicUsize,
    // #411 Lab L3: the instance's configured input is `nemu_ipc` (the Runtime then presses only on
    // a frame it captured); its fake backends stay the same.
    nemu_input: AtomicBool,
}

// Test plan E-3 G11 (§3.10): a manual Runtime clock. It moves only when a capture (or a wait of
// the kernel) moves it, so an arrival wait's budget is spent on it and not on the wall clock.
struct ManualClock {
    unix_origin_ms: u64,
    monotonic_ms: AtomicU64,
}

impl ManualClock {
    fn new() -> Self {
        let unix_origin_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after the epoch")
            .as_millis();
        Self {
            unix_origin_ms: u64::try_from(unix_origin_ms).expect("unix ms in u64"),
            monotonic_ms: AtomicU64::new(0),
        }
    }

    fn advance(&self, duration_ms: u64) {
        self.monotonic_ms.fetch_add(duration_ms, Ordering::SeqCst);
    }
}

impl RuntimeClock for ManualClock {
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        let monotonic_ms = self.monotonic_ms.load(Ordering::SeqCst);
        Ok(RuntimeClockSample {
            unix_ms: self.unix_origin_ms + monotonic_ms,
            monotonic_ms,
        })
    }

    fn pause(&self, duration: Duration) {
        let duration_ms = duration.as_nanos().div_ceil(1_000_000);
        self.advance(u64::try_from(duration_ms).unwrap_or(u64::MAX));
    }
}

#[test]
fn lab_operation_evidence_consistency_preserves_complete_and_incomplete_records() {
    use actingcommand_contract::{
        ContainedLabOperationRequest, InputAction, LabArrivalCondition, LabArrivalStatus,
        LabOperationEvidence, LabOperationSelection, LabProjectionHint,
        verify_lab_operation_evidence,
    };
    let root = TempDir::new().unwrap();
    let runtime_root = root.path().join("runtime");
    let package = root.path().join("lab-evidence.zip");
    let mut package_files: Vec<(&str, &[u8])> = vec![
        ("control.json", br#"{"game":"neutral","server":"test","entry_task_id":"seed"}"#),
        ("resources/manifest.json", br#"{"entry_task_id":"seed"}"#),
        ("resources/operations/seed/task.json", br#"{}"#),
        ("resources/recognition/neutral.test.pack.json", br#"{"schema_version":"0.3","game":"neutral","server":"test","coordinate_space":{"width":1,"height":1},"targets":[{"type":"color","id":"anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},{"type":"color","id":"private-value","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}]}"#),
        ("resources/recognition/neutral.test.pages.json", br#"{"schema_version":"0.3","pages":[{"id":"neutral/home","required":["anchor"],"optional":["private-value"]}]}"#),
        ("resources/navigation/neutral.test.navigation.json", br#"{"schema_version":"0.3","navigation":[]}"#),
        ("resources/navigation/neutral.test.projection.json", br#"{"schema_version":"actingcommand.page-projection-metadata.v1","actions":[],"targets":[{"target_id":"private-value","privacy":"personal","source":"neutral/spec"}],"fields":[],"pages":[]}"#),
    ];
    let hashes: serde_json::Map<String, Value> = package_files
        .iter()
        .filter(|(path, _)| path.starts_with("resources/") && *path != "resources/manifest.json")
        .map(|(path, bytes)| {
            (
                path.strip_prefix("resources/").unwrap().to_string(),
                serde_json::json!(format!("{:x}", Sha256::digest(bytes))),
            )
        })
        .collect();
    let manifest =
        serde_json::to_vec(&serde_json::json!({"entry_task_id":"seed","hashes":hashes})).unwrap();
    package_files
        .iter_mut()
        .find(|(path, _)| *path == "resources/manifest.json")
        .unwrap()
        .1 = &manifest;
    write_zip(&package, &package_files);
    let state = Arc::new(FakeState::default());
    // Test plan E-3 G11: the arrival waits run on a manual Runtime clock.
    let clock = Arc::new(ManualClock::new());
    if state.clock.set(Arc::clone(&clock)).is_err() {
        panic!("the fake's clock is set once");
    }
    let instance_id = *IdentifierIssuer::new()
        .unwrap()
        .mint_instance_id()
        .unwrap()
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"lab-evidence-spec").with_runtime_clock(clock),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: state.clone(),
            frame_size: 1,
        }),
    )
    .unwrap();
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_runtime_client::RuntimeChannel::Lab,
        actingcommand_contract::EventActor::Lab,
        EventSource::Lab,
    ))
    .unwrap();
    for (selection, timeout_ms) in [
        (
            LabOperationSelection::Coordinates {
                action: InputAction::Tap { x: 0, y: 0 },
            },
            None,
        ),
        (
            LabOperationSelection::Element {
                id: "undeclared".into(),
            },
            None,
        ),
        (
            LabOperationSelection::Coordinates {
                action: InputAction::Tap { x: 0, y: 0 },
            },
            Some(5_000),
        ),
        (
            LabOperationSelection::Coordinates {
                action: InputAction::Tap { x: 0, y: 0 },
            },
            Some(1_000),
        ),
    ] {
        let input_expected = matches!(selection, LabOperationSelection::Coordinates { .. });
        let timed_out = timeout_ms == Some(1_000);
        let complete = input_expected && !timed_out;
        state
            .transition_after_tap
            .store(timeout_ms.is_some(), Ordering::Release);
        // Test plan E-3 G11 (§3.10): the reached row's transition ends at a capture, the third
        // arrival sample after the operation's before frame; the timed-out row's captures move
        // the Runtime clock past its `timeout_ms` (400 ms each); no row waits on the wall clock.
        let captures = state.captures.load(Ordering::Acquire);
        state.home_from_capture.store(
            if timeout_ms == Some(5_000) {
                captures + 4
            } else {
                0
            },
            Ordering::Release,
        );
        state
            .clock_ms_per_capture
            .store(if timed_out { 400 } else { 0 }, Ordering::Release);
        let session = client.begin_debug_session().unwrap();
        let verified = session
            .run_contained_lab_operation(
                "node.a",
                ContainedLabOperationRequest {
                    package_path: package.to_str().unwrap().to_string(),
                    selection,
                    projection_hint: LabProjectionHint {
                        sequence: None,
                        content_sha256: None,
                    },
                    after: timeout_ms.map(|timeout_ms| LabArrivalCondition {
                        page_id: "neutral/home".into(),
                        timeout_ms,
                    }),
                },
                actingcommand_contract::ObservationFramePlan::SINGLE,
            )
            .unwrap();
        let operation = verified.operation().clone();
        let prepared = &operation.record.prepared;
        let events = client
            .query_events(
                EventQuery {
                    request_id: Some(prepared.request_id),
                    correlation_id: Some(prepared.correlation_id),
                    ..EventQuery::default()
                },
                ProjectionProfile::Forensic,
            )
            .unwrap();
        let lease_events = client
            .query_events(
                EventQuery {
                    lease_id: prepared.lease_id,
                    ..EventQuery::default()
                },
                ProjectionProfile::Forensic,
            )
            .unwrap();
        let action_events = if let Some(action_id) = operation.record.input_action_id {
            client
                .query_events(
                    EventQuery {
                        action_id: Some(action_id),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Forensic,
                )
                .unwrap()
        } else {
            Vec::new()
        };
        let artifacts = events
            .iter()
            .filter(|event| event.event_type == EventType::ArtifactVerified)
            .flat_map(|event| &event.artifacts)
            .map(|reference| {
                (
                    reference.artifact_id,
                    actingcommand_artifact_store::read_projected_verified(&runtime_root, reference)
                        .unwrap(),
                )
            })
            .collect();
        let evidence = LabOperationEvidence {
            operation,
            terminal: verified.receipt().terminal().unwrap(),
            events,
            lease_events,
            action_events,
            artifacts,
        };
        verify_lab_operation_evidence(&evidence).unwrap();
        // Workflow #375 R5c: the fixture's frames are identical, so the after frame of a tap
        // without an arrival wait is marked against its before frame on the operation's own
        // request; the online evidence above still verifies.
        if input_expected && timeout_ms.is_none() {
            assert!(
                evidence
                    .events
                    .iter()
                    .any(|event| event.event_type == EventType::CaptureDedupWindow)
            );
        }
        assert_eq!(evidence.operation.record.failure.is_none(), complete);
        assert_eq!(
            evidence.operation.record.after_frame.is_some(),
            input_expected
        );
        assert_eq!(
            evidence.operation.record.input_event.is_some(),
            input_expected
        );
        if timeout_ms.is_some() {
            let arrival = evidence.operation.record.arrival.as_ref().unwrap();
            assert_eq!(
                arrival.status,
                if timed_out {
                    LabArrivalStatus::TimedOut
                } else {
                    LabArrivalStatus::Reached
                }
            );
            assert!(!arrival.samples.is_empty());
            assert!(arrival.samples.len() <= actingcommand_contract::MAX_LAB_ARRIVAL_FRAMES);
            if !timed_out {
                assert!(arrival.samples.len() >= 2);
            }
            assert_eq!(
                evidence.operation.record.effect,
                actingcommand_contract::EffectDisposition::Performed
            );
            if timed_out {
                assert!(arrival.elapsed_ms >= 1_000);
                assert_eq!(
                    evidence.operation.record.failure.as_ref().unwrap().code,
                    "lab_arrival_timeout"
                );
            }
        } else {
            assert!(evidence.operation.record.arrival.is_none());
        }
        let projection = evidence
            .operation
            .record
            .prepared
            .before_projection
            .as_ref()
            .unwrap();
        let internal: actingcommand_contract::ContainedObservationEvidence =
            serde_json::from_slice(&evidence.artifacts[&projection.artifact.artifact_id]).unwrap();
        assert!(
            internal
                .private_facts
                .rows
                .iter()
                .any(|row| row["target"]["target_id"] == "private-value")
        );
        assert!(
            projection
                .facts
                .rows
                .iter()
                .any(|row| row["target"]["target_id"] == "private-value"
                    && row["target"]["redacted"] == true)
        );
        assert!(
            !serde_json::to_string(&evidence.operation)
                .unwrap()
                .contains("private_facts")
        );
        let mut mismatched = evidence.clone();
        mismatched.operation.record.prepared.request_id = *IdentifierIssuer::new()
            .unwrap()
            .mint_request_id()
            .unwrap()
            .transport();
        assert!(verify_lab_operation_evidence(&mismatched).is_err());
        let mut corrupt = evidence.clone();
        corrupt
            .artifacts
            .get_mut(
                &evidence
                    .operation
                    .record
                    .prepared_artifact
                    .artifact
                    .artifact_id,
            )
            .unwrap()[0] ^= 1;
        assert_eq!(
            verify_lab_operation_evidence(&corrupt)
                .unwrap_err()
                .code()
                .as_str(),
            "runtime_lab_artifact_hash_mismatch"
        );
        if complete {
            let mut missing_intent = evidence.clone();
            missing_intent.action_events.clear();
            assert!(verify_lab_operation_evidence(&missing_intent).is_err());
        }
        client.status().unwrap();
    }
    assert_eq!(state.taps.load(Ordering::Acquire), 3);
    drop(client);
    host.close().unwrap();
}

/// Workflow #369 Q-2, Q-3 (5), E5: a Lab operation stays an immediate try. On an instance
/// another client holds, it is refused `lease_busy` at its lease stage, before any frame or
/// input, and its evidence verifies as before.
#[test]
fn lab_operation_on_a_held_instance_is_refused_busy_before_any_input() {
    use actingcommand_contract::{
        ContainedLabOperationRequest, EventFamily, InputAction, LabOperationSelection,
        LabOperationStage, LabProjectionHint,
    };
    let root = TempDir::new().unwrap();
    let runtime_root = root.path().join("runtime");
    let package = root.path().join("lab-busy.zip");
    let mut package_files: Vec<(&str, &[u8])> = vec![
        ("control.json", br#"{"game":"neutral","server":"test","entry_task_id":"seed"}"#),
        ("resources/manifest.json", br#"{"entry_task_id":"seed"}"#),
        ("resources/operations/seed/task.json", br#"{}"#),
        ("resources/recognition/neutral.test.pack.json", br#"{"schema_version":"0.3","game":"neutral","server":"test","coordinate_space":{"width":1,"height":1},"targets":[{"type":"color","id":"anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},{"type":"color","id":"private-value","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}]}"#),
        ("resources/recognition/neutral.test.pages.json", br#"{"schema_version":"0.3","pages":[{"id":"neutral/home","required":["anchor"],"optional":["private-value"]}]}"#),
        ("resources/navigation/neutral.test.navigation.json", br#"{"schema_version":"0.3","navigation":[]}"#),
        ("resources/navigation/neutral.test.projection.json", br#"{"schema_version":"actingcommand.page-projection-metadata.v1","actions":[],"targets":[{"target_id":"private-value","privacy":"personal","source":"neutral/spec"}],"fields":[],"pages":[]}"#),
    ];
    let hashes: serde_json::Map<String, Value> = package_files
        .iter()
        .filter(|(path, _)| path.starts_with("resources/") && *path != "resources/manifest.json")
        .map(|(path, bytes)| {
            (
                path.strip_prefix("resources/").unwrap().to_string(),
                serde_json::json!(format!("{:x}", Sha256::digest(bytes))),
            )
        })
        .collect();
    let manifest =
        serde_json::to_vec(&serde_json::json!({"entry_task_id":"seed","hashes":hashes})).unwrap();
    package_files
        .iter_mut()
        .find(|(path, _)| *path == "resources/manifest.json")
        .unwrap()
        .1 = &manifest;
    write_zip(&package, &package_files);
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .unwrap()
        .mint_instance_id()
        .unwrap()
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"lab-evidence-spec"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: state.clone(),
            frame_size: 1,
        }),
    )
    .unwrap();
    let holder = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .unwrap();
    let token = holder.acquire_lease("node.a").unwrap();
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_runtime_client::RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .unwrap();
    let captures = state.captures.load(Ordering::Acquire);

    let verified = client
        .begin_debug_session()
        .unwrap()
        .run_contained_lab_operation(
            "node.a",
            ContainedLabOperationRequest {
                package_path: package.to_str().unwrap().to_string(),
                selection: LabOperationSelection::Coordinates {
                    action: InputAction::Tap { x: 0, y: 0 },
                },
                projection_hint: LabProjectionHint {
                    sequence: None,
                    content_sha256: None,
                },
                after: None,
            },
            actingcommand_contract::ObservationFramePlan::SINGLE,
        )
        .unwrap();

    let record = &verified.operation().record;
    let failure = record.failure.as_ref().expect("the busy refusal");
    assert_eq!(failure.stage, LabOperationStage::Lease);
    assert_eq!(failure.code, "lease_busy");
    assert_eq!(failure.error.code, RuntimeErrorCode::LeaseBusy);
    assert!(record.prepared.lease_id.is_none());
    assert!(record.prepared.before_frame.is_none());
    assert!(!record.input_returned);
    assert_eq!(state.captures.load(Ordering::Acquire), captures);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);
    let events = client
        .query_events(
            EventQuery {
                correlation_id: Some(record.prepared.correlation_id),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .unwrap();
    let refusal = failure.event.expect("the refusal's record");
    assert!(events.iter().any(|event| event.sequence == refusal.sequence
        && event.event_type == EventType::SchedulerDenied));
    assert!(!events.iter().any(|event| matches!(
        event.event_type.family(),
        EventFamily::Capture | EventFamily::Input | EventFamily::Task
    )));
    holder.release_lease(&token).unwrap();
    drop(client);
    drop(holder);
    host.close().unwrap();
}

#[test]
fn resource_restore_preserves_native_evidence_and_read_only_source_parsing() {
    use actingcommand_contract::{
        ContainedLabOperationRequest, LabOperationSelection, LabProjectionHint,
    };
    use actingcommand_resource_tooling::open_published_package;
    use serde_json::json;
    let root = TempDir::new().unwrap();
    let runtime_root = root.path().join("runtime");
    let local = root.path().join("local");
    let config = root.path().join("actinglab.json");
    fs::write(&config, "{}").unwrap();
    let source = root.path().join("source");
    let seed_dir = source.join("ours/operations/seed");
    fs::create_dir_all(seed_dir.join("assets")).unwrap();
    fs::create_dir_all(source.join("ours/navigation")).unwrap();
    let template = Frame::from_pixels(
        1,
        1,
        vec![255, 0, 0],
        PixelFormat::Rgb8,
        CaptureBackendName::AdbScreencap,
    )
    .unwrap()
    .encode_png_fast()
    .unwrap();
    fs::write(seed_dir.join("assets/HOME.png"), &template).unwrap();
    // Workflow #396 REC-7: a one-pixel ccorr_normed template is flat (its flat-field score is
    // 1000 milli), so only a threshold of 0 is admitted; the page still needs its color check.
    let source_task = json!({"schema_version":"0.6","task_id":"seed","game":"neutral","server_scope":["test"],"locale":"en-US",
        "coordinate_space":{"width":2,"height":2},"defaults":{"template_threshold":0.9,"color_max_distance":0.0},
        "anchors":[{"id":"home","template":"assets/HOME.png","region":{"mode":"full_frame"},"threshold":0.0,
            "color_check":{"region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}},"expected":[255,0,0]}}],
        "color_probes":[{"id":"private-value","region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}},"expected":[255,0,0]}],
        "entry_page":"home","target_page":"home","goal":"Explicit neutral author goal",
        "operations":[{"id":"seed-tap","purpose":"source purpose","from":"home","to":null,"click":{"kind":"point","x":1,"y":1},
            "guard":{"page_id":"home","target_id":"page/home","expected_rect":{"x":1,"y":1,"width":1,"height":1},"verify_template":"assets/HOME.png"},"consumes":[],"produces":[]}]});
    fs::write(
        seed_dir.join("task.json"),
        serde_json::to_vec_pretty(&source_task).unwrap(),
    )
    .unwrap();
    fs::write(
        source.join("ours/operations/resources.json"),
        br#"{"schema_version":"1.0","resources":[],"resource_count":0}"#,
    )
    .unwrap();
    fs::write(source.join("ours/navigation/neutral.test.projection.json"),br#"{"schema_version":"actingcommand.page-projection-metadata.v1","actions":[],"targets":[{"target_id":"private-value","privacy":"personal","source":"neutral/spec"}],"fields":[],"pages":[]}"#).unwrap();
    let parsed = actingcommand_resource_tooling::OperationParser::load(
        &source.join("ours"),
        None,
        None,
        None,
    )
    .unwrap()
    .build_all()
    .unwrap();
    let mut package_files = std::collections::BTreeMap::from([
        ("control.json", serde_json::to_vec(&json!({"schema_version":"Lab-1y.control.v1",
            "game":"neutral","server":"test","package_id":"neutral.test.seed",
            "entry_task_id":"seed","execution_mode":"navigable_route","resolution":{"width":2,"height":2}})).unwrap()),
        ("resources/operations/seed/task.json", serde_json::to_vec(&source_task).unwrap()),
        ("resources/operations/seed/assets/HOME.png", template.clone()),
        ("resources/operations/resources.json", fs::read(source.join("ours/operations/resources.json")).unwrap()),
        ("resources/recognition/neutral.test.pack.json", serde_json::to_vec(&parsed.pack).unwrap()),
        ("resources/recognition/neutral.test.pages.json", serde_json::to_vec(&parsed.pages).unwrap()),
        ("resources/navigation/neutral.test.navigation.json", serde_json::to_vec(&parsed.navigation).unwrap()),
        ("resources/navigation/neutral.test.projection.json", fs::read(source.join("ours/navigation/neutral.test.projection.json")).unwrap()),
    ]);
    let files = package_files
        .iter()
        .filter_map(|(path, bytes)| {
            path.strip_prefix("resources/").map(
                |path| json!({"path":path,"sha256":format!("sha256:{:x}", Sha256::digest(bytes))}),
            )
        })
        .collect::<Vec<_>>();
    package_files.insert(
        "resources/manifest.json",
        serde_json::to_vec(&json!({"schema_version":"0.3","entry_task_id":"seed","files":files}))
            .unwrap(),
    );
    let original = root.path().join("original.zip");
    write_zip(
        &original,
        &package_files
            .iter()
            .map(|(path, bytes)| (*path, bytes.as_slice()))
            .collect::<Vec<_>>(),
    );
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .unwrap()
        .mint_instance_id()
        .unwrap()
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"restore-native-spec"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: state.clone(),
            frame_size: 2,
        }),
    )
    .unwrap();
    let mut request_ids = Vec::new();
    let mut frame_hashes = Vec::new();
    for (flag, coordinates) in [("--tap", "1,1"), ("--swipe", "1,1,1,0,1")] {
        let output = run_actinglab_json(
            &config,
            &runtime_root,
            &local,
            [
                "--json",
                "--instance",
                "node.a",
                "do",
                flag,
                coordinates,
                "--capture",
                "--zip",
                original.to_str().unwrap(),
                "--after-page",
                "neutral/home",
                "--verbose",
            ],
        );
        assert_eq!(output["data"]["executed"], true);
        assert_eq!(output["data"]["arrival"]["status"], "reached");
        request_ids.push(output["data"]["req_id"].as_str().unwrap().to_string());
        let operation: actingcommand_contract::ContainedLabOperationResult =
            serde_json::from_value(output["data"]["operation_record"].clone()).unwrap();
        frame_hashes.push(
            operation
                .record
                .prepared
                .before_frame
                .unwrap()
                .observation
                .artifact()
                .sha256
                .clone(),
        );
    }
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_runtime_client::RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .unwrap();
    // Workflow #375 R5c: the fixture's frames are identical, so each operation's later frames
    // are marked against its earlier ones; the restore below still restores every step.
    let first_request = serde_json::from_value(json!(request_ids[0])).unwrap();
    assert!(
        client
            .query_events(
                EventQuery {
                    request_id: Some(first_request),
                    ..EventQuery::default()
                },
                ProjectionProfile::Forensic,
            )
            .unwrap()
            .iter()
            .any(|event| event.event_type == EventType::CaptureDedupWindow)
    );
    let session = client.begin_debug_session().unwrap();
    let published_source = open_published_package(&original).unwrap();
    let incomplete = session
        .run_contained_lab_operation(
            "node.a",
            ContainedLabOperationRequest {
                package_path: published_source.path().to_str().unwrap().into(),
                selection: LabOperationSelection::Element {
                    id: "undeclared".into(),
                },
                projection_hint: LabProjectionHint {
                    sequence: None,
                    content_sha256: None,
                },
                after: None,
            },
            actingcommand_contract::ObservationFramePlan::SINGLE,
        )
        .unwrap();
    request_ids.push(
        serde_json::to_value(incomplete.receipt().request_id())
            .unwrap()
            .as_str()
            .unwrap()
            .to_string(),
    );
    assert_eq!(
        incomplete.operation().record.effect,
        actingcommand_contract::EffectDisposition::NotPerformed
    );
    state.transition_after_tap.store(true, Ordering::Release);
    let (timeout_exit, timeout) = run_actinglab_failure_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "--instance",
            "node.a",
            "do",
            "--tap",
            "1,1",
            "--capture",
            "--zip",
            original.to_str().unwrap(),
            "--after-page",
            "neutral/home",
            "--after-timeout-ms",
            "1000",
            "--verbose",
        ],
    );
    let timeout_record = &timeout["error"]["details"];
    assert_eq!(timeout_record["effect"], "performed");
    assert_eq!(timeout_record["arrival"]["status"], "timed_out");
    // #411 A19 (exec key 93): a timed-out arrival answers "the page did not arrive" in its own
    // code (exit 3), not device_error, and `arrival` is the last member of its details, after
    // `after_attachment`.
    assert_eq!(timeout_exit, 3);
    assert_eq!(
        timeout["error"]["code"],
        actingcommand_contract::codes::SharedCode::LabArrivalTimeout.as_str()
    );
    let timeout_members = timeout_record
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(timeout_members.last(), Some(&"arrival"));
    let attachment_at = timeout_members
        .iter()
        .position(|member| *member == "after_attachment")
        .expect("after_attachment in the details");
    assert!(attachment_at < timeout_members.len() - 1);
    request_ids.push(timeout_record["req_id"].as_str().unwrap().to_string());
    drop(session);
    drop(client);
    host.close().unwrap();
    let snapshot = actingcommand_ledger::GlobalLedger::open_evidence(
        actingcommand_ledger::GlobalLedgerEvidenceConfig::new(&runtime_root),
        |reference| {
            Some(
                actingcommand_artifact_store::verify_projected_read_only(&runtime_root, reference)
                    .unwrap(),
            )
        },
    )
    .unwrap();
    let through = snapshot.latest_sequence().to_string();
    let native_before = serde_json::to_vec(snapshot.events()).unwrap();
    let restored = root.path().join("restored");
    let output = run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "resource",
            "restore",
            "--repo",
            restored.to_str().unwrap(),
            "--state-root",
            runtime_root.to_str().unwrap(),
            "--request-id",
            &request_ids[1],
            "--request-id",
            &request_ids[0],
            "--request-id",
            &request_ids[2],
            "--request-id",
            &request_ids[3],
            "--through-sequence",
            &through,
            "--zip",
            original.to_str().unwrap(),
            "--task-id",
            "restored",
            "--entry-page",
            "home",
            "--target-page",
            "home",
            "--goal",
            "Explicit restored goal",
        ],
    );
    assert_eq!(output["data"]["record_count"], 4);
    assert_eq!(output["data"]["operation_count"], 2);
    assert_eq!(output["data"]["awaiting_author_input"], json!([]));
    assert_eq!(output["data"]["gaps"].as_array().unwrap().len(), 2);
    assert!(
        output["data"]["gaps"]
            .to_string()
            .contains("lab_arrival_not_reached")
    );
    let task_bytes = fs::read(restored.join("ours/operations/restored/task.json")).unwrap();
    let task: Value = serde_json::from_slice(&task_bytes).unwrap();
    assert_eq!(task["schema_version"], "0.6");
    assert_eq!(task["target_page"], "home");
    assert_eq!(task["operations"][0]["click"]["kind"], "point");
    assert_eq!(task["operations"][1]["click"]["kind"], "drag");
    assert_eq!(task["operations"][1]["click"]["duration_ms"], 1);
    assert_eq!(task["operations"][0]["purpose"], "");
    assert_eq!(task["operations"][0]["to"], Value::Null);
    let source_operation = &task["operations"][0];
    assert_eq!(source_operation["from"], "home");
    assert_eq!(source_operation["expect_after"]["page_id"], "home");
    let recorded_after = &source_operation["provenance"]["after"]["projection"];
    assert_eq!(recorded_after["page"], "neutral/home");
    assert_eq!(
        source_operation["provenance"]["after_condition"]["page_id"],
        "neutral/home"
    );
    assert_eq!(
        source_operation["provenance"]["arrival"]["status"],
        "reached"
    );
    assert!(
        task["operations"][0]["provenance"]["input_intent"]["sequence"]
            .as_u64()
            .unwrap()
            < task["operations"][1]["provenance"]["input_intent"]["sequence"]
                .as_u64()
                .unwrap()
    );
    assert!(
        !String::from_utf8(task_bytes.clone())
            .unwrap()
            .contains("private_facts")
    );
    assert!(task["color_probes"].as_array().unwrap().is_empty());
    let assets = fs::read_dir(restored.join("ours/operations/restored/assets"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(assets.len(), 1);
    assert_eq!(fs::read(&assets[0]).unwrap(), template);
    assert!(!frame_hashes.contains(&format!(
        "sha256:{:x}",
        Sha256::digest(fs::read(&assets[0]).unwrap())
    )));
    let metadata: Value = serde_json::from_slice(
        &fs::read(restored.join("ours/navigation/neutral.test.projection.json")).unwrap(),
    )
    .unwrap();
    assert!(
        metadata["actions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|action| action["safety"] == "dangerous")
    );
    let restored_outputs = actingcommand_resource_tooling::OperationParser::load(
        &restored.join("ours"),
        None,
        None,
        None,
    )
    .unwrap()
    .build_all()
    .unwrap();
    assert_eq!(restored_outputs.pack["game"], "neutral");
    assert_eq!(restored_outputs.pack["server"], "test");
    assert!(
        !restored_outputs.pack["targets"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fs::read(restored.join("ours/operations/restored/task.json")).unwrap(),
        task_bytes
    );
    run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "package",
            "validate",
            "--zip",
            original.to_str().unwrap(),
        ],
    );
    let pending = root.path().join("pending");
    let pending_output = run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "resource",
            "restore",
            "--repo",
            pending.to_str().unwrap(),
            "--state-root",
            runtime_root.to_str().unwrap(),
            "--request-id",
            &request_ids[0],
            "--through-sequence",
            &through,
            "--zip",
            original.to_str().unwrap(),
            "--task-id",
            "pending",
        ],
    );
    assert_eq!(
        pending_output["data"]["awaiting_author_input"],
        json!(["target_page", "entry_page"])
    );
    let pending_task: Value = serde_json::from_slice(
        &fs::read(pending.join("ours/operations/pending/task.json")).unwrap(),
    )
    .unwrap();
    assert!(pending_task.get("target_page").is_none() && pending_task.get("entry_page").is_none());
    // Workflow #375 R5d: `actinglab resource restore` reads frames that the cleaner moved into
    // their kept folders (here by hand, as actingd would): the same restore after moving the
    // first request's frames lists the same gaps and no missing material.
    let first_request: actingcommand_contract::RequestId =
        serde_json::from_value(json!(request_ids[0])).unwrap();
    let moved_keys = snapshot
        .events()
        .iter()
        .filter(|event| {
            event.event_type() == EventType::ArtifactVerified
                && event.links().request_id() == Some(&first_request)
        })
        .flat_map(|event| event.artifacts().iter())
        .filter(|artifact| artifact.kind() == ArtifactKind::CaptureFrame)
        .map(|artifact| artifact.project(true).object_key.unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(!moved_keys.is_empty());
    let leaf = runtime_root
        .join("kept")
        .join("2026-10-12")
        .join("node_a-1-lab");
    fs::create_dir_all(&leaf).unwrap();
    for (index, key) in moved_keys.iter().enumerate() {
        let source = runtime_root.join(key.as_str());
        let object_file = source.file_name().unwrap().to_str().unwrap().to_owned();
        fs::rename(
            &source,
            leaf.join(format!("061502-{index:03}_{object_file}")),
        )
        .unwrap();
    }
    let moved = root.path().join("moved");
    let moved_output = run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "resource",
            "restore",
            "--repo",
            moved.to_str().unwrap(),
            "--state-root",
            runtime_root.to_str().unwrap(),
            "--request-id",
            &request_ids[0],
            "--through-sequence",
            &through,
            "--zip",
            original.to_str().unwrap(),
            "--task-id",
            "pending",
        ],
    );
    assert_eq!(
        moved_output["data"]["record_count"],
        pending_output["data"]["record_count"]
    );
    assert_eq!(moved_output["data"]["gaps"], pending_output["data"]["gaps"]);
    assert!(
        !moved_output["data"]["gaps"]
            .to_string()
            .contains("material_missing")
    );
    run_actinglab_failure_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "resource",
            "restore",
            "--repo",
            restored.to_str().unwrap(),
            "--state-root",
            runtime_root.to_str().unwrap(),
            "--request-id",
            &request_ids[0],
            "--through-sequence",
            &through,
            "--zip",
            original.to_str().unwrap(),
            "--task-id",
            "restored",
        ],
    );
    assert_eq!(
        fs::read(restored.join("ours/operations/restored/task.json")).unwrap(),
        task_bytes
    );
    let after = actingcommand_ledger::GlobalLedger::open_evidence(
        actingcommand_ledger::GlobalLedgerEvidenceConfig::new(&runtime_root),
        |reference| {
            Some(
                actingcommand_artifact_store::verify_projected_read_only(&runtime_root, reference)
                    .unwrap(),
            )
        },
    )
    .unwrap();
    assert_eq!(serde_json::to_vec(after.events()).unwrap(), native_before);
    assert_eq!(state.taps.load(Ordering::Acquire), 2);
}

// Workflow #336 PKG-S2 (option 丙), model M-PKG v1.1 §4.3 S2-T5 (exec gate 55): a Lab `do`
// records its package by `package_id` only, and `resource restore` pairs the recorded
// operation with the original package by that id: the same package restores, also when one of
// its files changed in place; a package with another id is refused.
#[test]
fn s2_t5_lab_do_names_its_package_and_restore_pairs_by_package_id() {
    let root = TempDir::new().unwrap();
    let runtime_root = root.path().join("runtime");
    let local = root.path().join("local");
    let config = root.path().join("actinglab.json");
    fs::write(&config, "{}").unwrap();
    let source = root.path().join("source");
    let seed_dir = source.join("ours/operations/seed");
    fs::create_dir_all(seed_dir.join("assets")).unwrap();
    fs::create_dir_all(source.join("ours/navigation")).unwrap();
    let template = Frame::from_pixels(
        1,
        1,
        vec![255, 0, 0],
        PixelFormat::Rgb8,
        CaptureBackendName::AdbScreencap,
    )
    .unwrap()
    .encode_png_fast()
    .unwrap();
    fs::write(seed_dir.join("assets/HOME.png"), &template).unwrap();
    let source_task = json!({"schema_version":"0.6","task_id":"seed","game":"neutral","server_scope":["test"],"locale":"en-US",
        "coordinate_space":{"width":2,"height":2},"defaults":{"template_threshold":0.9,"color_max_distance":0.0},
        "anchors":[{"id":"home","template":"assets/HOME.png","region":{"mode":"full_frame"},"threshold":0.0,
            "color_check":{"region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}},"expected":[255,0,0]}}],
        "color_probes":[{"id":"private-value","region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}},"expected":[255,0,0]}],
        "entry_page":"home","target_page":"home","goal":"Explicit neutral author goal",
        "operations":[{"id":"seed-tap","purpose":"source purpose","from":"home","to":null,"click":{"kind":"point","x":1,"y":1},
            "guard":{"page_id":"home","target_id":"page/home","expected_rect":{"x":1,"y":1,"width":1,"height":1},"verify_template":"assets/HOME.png"},"consumes":[],"produces":[]}]});
    fs::write(
        seed_dir.join("task.json"),
        serde_json::to_vec(&source_task).unwrap(),
    )
    .unwrap();
    let resources = br#"{"schema_version":"1.0","resources":[],"resource_count":0}"#;
    fs::write(source.join("ours/operations/resources.json"), resources).unwrap();
    let projection = br#"{"schema_version":"actingcommand.page-projection-metadata.v1","actions":[],"targets":[{"target_id":"private-value","privacy":"personal","source":"neutral/spec"}],"fields":[],"pages":[]}"#;
    fs::write(
        source.join("ours/navigation/neutral.test.projection.json"),
        projection,
    )
    .unwrap();
    let parsed = actingcommand_resource_tooling::OperationParser::load(
        &source.join("ours"),
        None,
        None,
        None,
    )
    .unwrap()
    .build_all()
    .unwrap();
    // One package by its `package_id` and its task file's bytes.
    let write_package = |name: &str, package_id: &str, task: Vec<u8>| {
        let control = serde_json::to_vec(&json!({"schema_version":"Lab-1y.control.v1",
            "game":"neutral","server":"test","package_id":package_id,
            "entry_task_id":"seed","execution_mode":"navigable_route","resolution":{"width":2,"height":2}}))
        .unwrap();
        let pack = serde_json::to_vec(&parsed.pack).unwrap();
        let pages = serde_json::to_vec(&parsed.pages).unwrap();
        let navigation = serde_json::to_vec(&parsed.navigation).unwrap();
        let path = root.path().join(name);
        write_zip(
            &path,
            &[
                ("control.json", control.as_slice()),
                (
                    "resources/manifest.json",
                    br#"{"schema_version":"0.3","entry_task_id":"seed"}"#.as_slice(),
                ),
                ("resources/operations/seed/task.json", task.as_slice()),
                (
                    "resources/operations/seed/assets/HOME.png",
                    template.as_slice(),
                ),
                ("resources/operations/resources.json", resources.as_slice()),
                (
                    "resources/recognition/neutral.test.pack.json",
                    pack.as_slice(),
                ),
                (
                    "resources/recognition/neutral.test.pages.json",
                    pages.as_slice(),
                ),
                (
                    "resources/navigation/neutral.test.navigation.json",
                    navigation.as_slice(),
                ),
                (
                    "resources/navigation/neutral.test.projection.json",
                    projection.as_slice(),
                ),
            ],
        );
        path
    };
    let original = write_package(
        "original.zip",
        "neutral.test.seed",
        serde_json::to_vec(&source_task).unwrap(),
    );
    // The same package with its task file rewritten in place (same declarations, other bytes).
    let changed = write_package(
        "changed.zip",
        "neutral.test.seed",
        serde_json::to_vec_pretty(&source_task).unwrap(),
    );
    let other = write_package(
        "other.zip",
        "neutral.test.other",
        serde_json::to_vec(&source_task).unwrap(),
    );
    assert_ne!(fs::read(&original).unwrap(), fs::read(&changed).unwrap());

    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .unwrap()
        .mint_instance_id()
        .unwrap()
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"restore-package-label"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: state.clone(),
            frame_size: 2,
        }),
    )
    .unwrap();
    let output = run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "--instance",
            "node.a",
            "do",
            "--tap",
            "1,1",
            "--capture",
            "--zip",
            original.to_str().unwrap(),
            "--after-page",
            "neutral/home",
            "--verbose",
        ],
    );
    assert_eq!(output["data"]["executed"], true);
    let request_id = output["data"]["req_id"].as_str().unwrap().to_string();
    let prepared = &output["data"]["operation_record"]["record"]["prepared"];
    assert_eq!(prepared["package_label"], "neutral.test.seed");
    for member in ["expected_package_sha256", "actual_package_sha256"] {
        assert!(prepared.get(member).is_none(), "{member}");
    }
    host.close().unwrap();
    let through = actingcommand_ledger::GlobalLedger::open_evidence(
        actingcommand_ledger::GlobalLedgerEvidenceConfig::new(&runtime_root),
        |reference| {
            Some(
                actingcommand_artifact_store::verify_projected_read_only(&runtime_root, reference)
                    .unwrap(),
            )
        },
    )
    .unwrap()
    .latest_sequence()
    .to_string();

    for (repo, package) in [("restored", &original), ("restored-changed", &changed)] {
        let restored = run_actinglab_json(
            &config,
            &runtime_root,
            &local,
            [
                "--json",
                "resource",
                "restore",
                "--repo",
                root.path().join(repo).to_str().unwrap(),
                "--state-root",
                runtime_root.to_str().unwrap(),
                "--request-id",
                &request_id,
                "--through-sequence",
                &through,
                "--zip",
                package.to_str().unwrap(),
                "--task-id",
                "restored",
            ],
        );
        assert_eq!(
            restored["data"]["package_label"], "neutral.test.seed",
            "{repo}"
        );
        assert!(restored["data"].get("package_sha256").is_none(), "{repo}");
    }
    let (_, refused) = run_actinglab_failure_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "resource",
            "restore",
            "--repo",
            root.path().join("restored-other").to_str().unwrap(),
            "--state-root",
            runtime_root.to_str().unwrap(),
            "--request-id",
            &request_id,
            "--through-sequence",
            &through,
            "--zip",
            other.to_str().unwrap(),
            "--task-id",
            "restored",
        ],
    );
    assert_eq!(refused["error"]["code"], "package_invalid");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("package identity mismatch"),
        "{refused}"
    );
    assert!(!root.path().join("restored-other").exists());
}

#[test]
fn online_observe_cli_consumes_verified_projection_and_keeps_raw_offline_contracts() {
    let root = TempDir::new().unwrap();
    let runtime_root = root.path().join("runtime");
    let local = root.path().join("local");
    let config = root.path().join("actinglab.json");
    fs::write(&config, "{}").unwrap();
    let package = root.path().join("observe.zip");
    let frame = root.path().join("frame.png");
    write_zip(&package, &[
        ("control.json", br#"{"game":"neutral","server":"test","package_id":"neutral.observation","entry_task_id":"task"}"#),
        ("resources/manifest.json", br#"{"entry_task_id":"task"}"#),
        ("resources/operations/task/task.json", br#"{}"#),
        ("resources/recognition/neutral.test.pack.json", br#"{"schema_version":"0.3","coordinate_space":{"width":1,"height":1},"targets":[{"type":"color","id":"anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}]}"#),
        ("resources/recognition/neutral.test.pages.json", br#"{"schema_version":"0.3","pages":[{"id":"page","required":["anchor"]}]}"#),
        ("resources/navigation/neutral.test.navigation.json", br#"{"schema_version":"0.3","navigation":[]}"#),
    ]);
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .unwrap()
        .mint_instance_id()
        .unwrap()
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"online-observation-cli"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: state.clone(),
            frame_size: 1,
        }),
    )
    .unwrap();
    let output = run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "--instance",
            "node.a",
            "observe",
            "--capture",
            "--zip",
            package.to_str().unwrap(),
            "--targets",
            "anchor",
            "--with-frame",
            frame.to_str().unwrap(),
            "--verbose",
        ],
    );
    assert_eq!(output["data"]["state"], "recognized");
    assert_eq!(output["data"]["observation"]["page"], "page");
    assert_eq!(output["data"]["facts"]["target_evaluation_count"], 1);
    // Workflow #336 PKG-S2: the projection source names its package by `package_id` only.
    assert_eq!(
        output["data"]["projection_source"]["package_label"],
        "neutral.observation"
    );
    assert!(
        output["data"]["projection_source"]
            .get("actual_package_sha256")
            .is_none()
    );
    assert!(
        output["data"]["terminal"]["sequence"].as_u64().unwrap()
            > output["data"]["projection_source"]["projection_sequence"]
                .as_u64()
                .unwrap()
    );
    assert!(frame.is_file());
    let minimum = run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "--instance",
            "node.a",
            "observe",
            "--capture",
            "--zip",
            package.to_str().unwrap(),
        ],
    );
    assert!(
        serde_json::to_vec(&minimum["data"]).unwrap().len()
            <= actingcommand_ledger::MIN_PROJECTION_HARD_LIMIT_BYTES
    );
    assert_eq!(minimum["data"]["observation"]["page"], "page");
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_runtime_client::RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .unwrap();
    let raw = client.observe_readonly("node.a").unwrap();
    assert!(
        matches!(raw.receipt().result(), Some(actingcommand_contract::RuntimeResult::ReadonlyObservationCompleted { observation }) if observation.verdict() == actingcommand_contract::RecognitionVerdict::FrameDecoded)
    );
    let before = state.captures.load(Ordering::Acquire);
    let offline = run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "observe",
            "--scene",
            frame.to_str().unwrap(),
            "--zip",
            package.to_str().unwrap(),
            "--verbose",
        ],
    );
    assert_eq!(offline["data"]["observation"]["page"], "page");
    assert_eq!(state.captures.load(Ordering::Acquire), before);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);
    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .unwrap();
    assert!(
        events
            .iter()
            .all(|event| event.event_type != EventType::LeaseGranted)
    );
    drop(client);
    host.close().unwrap();
}

// Workflow #411 Lab L4 (exec gate key 98, register §1.120 row L4-1): `observe --capture --record`
// stores the backend that captured the Runtime frame in recording.json, named as
// `capture --record` names it, where it stored `null`. The fake Runtime's frames come from
// `adb_screencap`. Requirement basis: rd-1019/plan-lab-backends/plan.md §4 L4 R1.
#[test]
fn l4_observe_capture_record_stores_the_runtime_frames_capture_backend() {
    let root = TempDir::new().unwrap();
    let runtime_root = root.path().join("runtime");
    let local = root.path().join("local");
    let config = root.path().join("actinglab.json");
    fs::write(&config, "{}").unwrap();
    let package = root.path().join("observe.zip");
    write_zip(&package, &[
        ("control.json", br#"{"game":"neutral","server":"test","package_id":"neutral.observation","entry_task_id":"task"}"#),
        ("resources/manifest.json", br#"{"entry_task_id":"task"}"#),
        ("resources/operations/task/task.json", br#"{}"#),
        ("resources/recognition/neutral.test.pack.json", br#"{"schema_version":"0.3","coordinate_space":{"width":1,"height":1},"targets":[{"type":"color","id":"anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}]}"#),
        ("resources/recognition/neutral.test.pages.json", br#"{"schema_version":"0.3","pages":[{"id":"page","required":["anchor"]}]}"#),
        ("resources/navigation/neutral.test.navigation.json", br#"{"schema_version":"0.3","navigation":[]}"#),
    ]);
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .unwrap()
        .mint_instance_id()
        .unwrap()
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"lab-l4-observe-record"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 1,
        }),
    )
    .unwrap();

    run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "--instance",
            "node.a",
            "record",
            "start",
            "--task-id",
            "neutral_task",
            "--record-id",
            "record_l4",
        ],
    );
    let observed = run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "--instance",
            "node.a",
            "observe",
            "--capture",
            "--zip",
            package.to_str().unwrap(),
            "--record",
        ],
    );
    host.close().unwrap();

    let answered = &observed["data"]["record"]["frame"];
    assert_eq!(answered["source"], "runtime_observation", "{observed}");
    assert_eq!(answered["capture_backend"], "adb_screencap", "{observed}");
    let recording: Value = serde_json::from_slice(
        &fs::read(
            local
                .join("ActingCommand")
                .join("actinglab")
                .join("session")
                .join("record-artifacts")
                .join("record_l4")
                .join("lab")
                .join("recording.json"),
        )
        .expect("the recording"),
    )
    .expect("recording.json");
    let stored = &recording["steps"][0]["frames"][0];
    assert_eq!(stored["frame_id"], answered["frame_id"], "{recording}");
    assert_eq!(stored["source"], "runtime_observation", "{recording}");
    assert_eq!(stored["capture_backend"], "adb_screencap", "{recording}");
}

// #411 Lab L3 (exec key 97): a neutral navigation package on the fake's 1x1 frames: `neutral/home`
// (a red frame) and `neutral/page_a` (a blue one), and one edge from home to page_a that presses
// `click`.
fn write_l3_navigation_package(path: &Path, click: &str) {
    let navigation = format!(
        r#"{{"schema_version":"0.3","game":"neutral","server":"test","navigation":[{{"id":"home_to_page_a","from_page":"neutral/home","to_page":"neutral/page_a","click":{click}}}],"destructive_actions":[]}}"#
    );
    write_zip(
        path,
        &[
            (
                "control.json",
                br#"{"game":"neutral","server":"test","entry_task_id":"task"}"#,
            ),
            (
                "resources/manifest.json",
                br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
            ),
            ("resources/operations/task/task.json", br#"{}"#),
            (
                "resources/recognition/neutral.test.pack.json",
                br#"{"schema_version":"0.3","coordinate_space":{"width":1,"height":1},"targets":[{"type":"color","id":"home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},{"type":"color","id":"page_a_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]}]}"#,
            ),
            (
                "resources/recognition/neutral.test.pages.json",
                br#"{"schema_version":"0.3","pages":[{"id":"neutral/home","required":["home_anchor"]},{"id":"neutral/page_a","required":["page_a_anchor"]}]}"#,
            ),
            (
                "resources/navigation/neutral.test.navigation.json",
                navigation.as_bytes(),
            ),
        ],
    );
}

// #411 Lab L3: a Runtime host over the fake for one test, with the instance `node.a`.
fn l3_host(runtime_root: &Path, state: &Arc<FakeState>, label: &[u8]) -> RuntimeHost {
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    RuntimeHost::start(
        RuntimeHostConfig::new(runtime_root, label),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(state),
            frame_size: 1,
        }),
    )
    .expect("runtime host")
}

/// #411 Lab L3 (exec key 97; R2): each input of an `ensure --capture` route names the Runtime
/// frame its edge's guard was decided on, so the route taps on an instance whose input is
/// `nemu_ipc`, which presses only on a frame the Runtime captured and committed last; without the
/// frame the Runtime refuses the input (`nemu_input_frame_required`).
#[test]
fn l3_ensure_capture_taps_on_a_nemu_input_instance_with_its_guard_frame() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let package = root.path().join("navigation.zip");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    write_l3_navigation_package(&package, r#"{"kind":"point","x":0,"y":0}"#);
    let state = Arc::new(FakeState::default());
    state.transition_after_tap.store(true, Ordering::Release);
    state.nemu_input.store(true, Ordering::Release);
    let host = l3_host(&runtime_root, &state, b"lab-l3-nemu-ensure");

    let ensure = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "ensure",
            "--capture",
            "--to",
            "neutral/page_a",
            "--step-timeout-ms",
            "100",
            "--poll-ms",
            "1",
            "--zip",
            package.to_str().expect("package path"),
        ],
    );
    host.close().expect("close host");

    assert_eq!(ensure["data"]["state"], "arrived", "{ensure}");
    assert_eq!(ensure["data"]["executed"], true, "{ensure}");
    assert_eq!(ensure["data"]["page"], "neutral/page_a", "{ensure}");
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
}

/// #411 Lab L3 (exec key 97; R1): a route of `ensure --capture` presses a `long_press` edge as
/// one long tap at the centre of its box, as the kernel presses a pack's long press, not as a
/// swipe that does not move.
#[test]
fn l3_ensure_capture_presses_a_long_press_edge_as_a_long_tap() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let package = root.path().join("navigation.zip");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    write_l3_navigation_package(
        &package,
        r#"{"kind":"long_press","x":0,"y":0,"width":1,"height":1,"duration_ms":800}"#,
    );
    let state = Arc::new(FakeState::default());
    state.transition_after_tap.store(true, Ordering::Release);
    let host = l3_host(&runtime_root, &state, b"lab-l3-long-press-ensure");

    let ensure = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "ensure",
            "--capture",
            "--to",
            "neutral/page_a",
            "--step-timeout-ms",
            "100",
            "--poll-ms",
            "1",
            "--zip",
            package.to_str().expect("package path"),
        ],
    );
    host.close().expect("close host");

    assert_eq!(ensure["data"]["state"], "arrived", "{ensure}");
    assert_eq!(ensure["data"]["page"], "neutral/page_a", "{ensure}");
    assert_eq!(state.long_taps.load(Ordering::Acquire), 1);
    assert_eq!(state.swipes.load(Ordering::Acquire), 0);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);
}

/// #411 Lab L3 (exec key 97; R1): online `do <element-id> --capture` on an element whose
/// projected input is a long press (a drag that does not move) sends one long tap at its point
/// over its duration; the prepared geometry stays that drag, and the client's verification of the
/// operation's evidence, which resolves the selection again, passes.
#[test]
fn l3_do_capture_on_a_long_press_element_sends_a_long_tap_and_its_evidence_verifies() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let package = root.path().join("navigation.zip");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    write_l3_navigation_package(
        &package,
        r#"{"kind":"long_press","x":0,"y":0,"width":1,"height":1,"duration_ms":800}"#,
    );
    let state = Arc::new(FakeState::default());
    let host = l3_host(&runtime_root, &state, b"lab-l3-long-press-do");

    let observe = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "observe",
            "--capture",
            "--zip",
            package.to_str().expect("package path"),
            "--verbose",
        ],
    );
    let elements = observe["data"]["observation"]["elements"]
        .as_array()
        .unwrap_or_else(|| panic!("observation elements: {observe}"));
    assert_eq!(elements.len(), 1, "the home page's one edge: {observe}");
    let element = elements[0]["id"]
        .as_str()
        .expect("the long press element")
        .to_string();
    let action = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "do",
            &element,
            "--capture",
            "--zip",
            package.to_str().expect("package path"),
            "--verbose",
        ],
    );
    host.close().expect("close host");

    assert_eq!(action["data"]["executed"], true, "{action}");
    assert_eq!(action["data"]["effect"], "performed", "{action}");
    let operation: actingcommand_contract::ContainedLabOperationResult =
        serde_json::from_value(action["data"]["operation_record"].clone())
            .expect("the verified operation record");
    let prepared = &operation.record.prepared;
    assert_eq!(
        serde_json::to_value(&prepared.action).expect("action JSON"),
        json!({"kind": "long_tap", "x": 0, "y": 0, "duration_ms": 800})
    );
    assert_eq!(
        serde_json::to_value(&prepared.geometry).expect("geometry JSON"),
        json!({"type": "drag",
               "from_rect": {"x": 0, "y": 0, "width": 1, "height": 1},
               "to_rect": {"x": 0, "y": 0, "width": 1, "height": 1},
               "from": {"x": 0, "y": 0}, "to": {"x": 0, "y": 0},
               "duration_ms": 800})
    );
    assert!(operation.record.failure.is_none(), "{action}");
    assert_eq!(state.long_taps.load(Ordering::Acquire), 1);
    assert_eq!(state.swipes.load(Ordering::Acquire), 0);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);
}

/// #411 Lab L7 (exec key 100; R1): the `--capture` client forms of `do` (planning, `--dry-run`),
/// `ensure` and `wait` decide the Runtime frame's page as `current-page` does: the fake's red
/// frame, which `neutral/home` and `neutral/also_home` both match, is refused with
/// `page_recognition_conflict` (exit 2) naming both pages in page order, instead of being taken as
/// the first of them, and nothing is pressed.
#[test]
fn l7_capture_forms_of_do_ensure_and_wait_refuse_a_frame_two_pages_match() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let package = root.path().join("navigation.zip");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    write_zip(
        &package,
        &[
            (
                "control.json",
                br#"{"game":"neutral","server":"test","entry_task_id":"task"}"#,
            ),
            (
                "resources/manifest.json",
                br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
            ),
            ("resources/operations/task/task.json", br#"{}"#),
            (
                "resources/recognition/neutral.test.pack.json",
                br#"{"schema_version":"0.3","coordinate_space":{"width":1,"height":1},"targets":[{"type":"color","id":"home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},{"type":"color","id":"also_home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},{"type":"color","id":"page_a_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]},{"type":"color","id":"home_button","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0],"click":{"x":0,"y":0,"width":1,"height":1}}]}"#,
            ),
            (
                "resources/recognition/neutral.test.pages.json",
                br#"{"schema_version":"0.3","pages":[{"id":"neutral/home","required":["home_anchor"]},{"id":"neutral/also_home","required":["also_home_anchor"]},{"id":"neutral/page_a","required":["page_a_anchor"]}]}"#,
            ),
            (
                "resources/navigation/neutral.test.navigation.json",
                br#"{"schema_version":"0.3","game":"neutral","server":"test","navigation":[{"id":"home_to_page_a","from_page":"neutral/home","to_page":"neutral/page_a","click":{"kind":"point","x":0,"y":0}}],"destructive_actions":[]}"#,
            ),
        ],
    );
    let package = package.to_str().expect("package path");
    let state = Arc::new(FakeState::default());
    state.transition_after_tap.store(true, Ordering::Release);
    let host = l3_host(&runtime_root, &state, b"lab-l7-page-conflict");

    let refusals = [
        (
            "do --capture --dry-run",
            run_actinglab_failure_json(
                &config_path,
                &runtime_root,
                &local_app_data,
                [
                    "--json",
                    "--instance",
                    "node.a",
                    "do",
                    "home_button",
                    "--capture",
                    "--dry-run",
                    "--zip",
                    package,
                ],
            ),
        ),
        (
            "ensure --capture",
            run_actinglab_failure_json(
                &config_path,
                &runtime_root,
                &local_app_data,
                [
                    "--json",
                    "--instance",
                    "node.a",
                    "ensure",
                    "--capture",
                    "--to",
                    "neutral/page_a",
                    "--step-timeout-ms",
                    "100",
                    "--poll-ms",
                    "1",
                    "--zip",
                    package,
                ],
            ),
        ),
        (
            "wait --capture",
            run_actinglab_failure_json(
                &config_path,
                &runtime_root,
                &local_app_data,
                [
                    "--json",
                    "--instance",
                    "node.a",
                    "wait",
                    "--capture",
                    "--page",
                    "neutral/home",
                    "--timeout-ms",
                    "100",
                    "--poll-ms",
                    "1",
                    "--zip",
                    package,
                ],
            ),
        ),
    ];
    host.close().expect("close host");

    for (form, (exit_code, envelope)) in refusals {
        assert_eq!(exit_code, 2, "{form}: {envelope}");
        assert_eq!(
            envelope["error"]["code"], "page_recognition_conflict",
            "{form}: {envelope}"
        );
        assert_eq!(
            envelope["error"]["details"]["matched_pages"],
            json!(["neutral/home", "neutral/also_home"]),
            "{form}: {envelope}"
        );
    }
    assert_eq!(state.taps.load(Ordering::Acquire), 0);
    assert_eq!(state.long_taps.load(Ordering::Acquire), 0);
    assert_eq!(state.swipes.load(Ordering::Acquire), 0);
}

struct FakeBackend {
    state: Arc<FakeState>,
    closed: bool,
}

impl InputBackend for FakeBackend {
    fn tap(
        &mut self,
        _witness: &actingcommand_contract::FencedWrite,
        _x: i32,
        _y: i32,
    ) -> DeviceResult<()> {
        self.state.tap_started.store(true, Ordering::Release);
        let delay_ms = self.state.tap_delay_ms.load(Ordering::Acquire);
        if delay_ms > 0 {
            thread::sleep(Duration::from_millis(delay_ms as u64));
        }
        self.state.taps.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn long_tap(
        &mut self,
        _witness: &actingcommand_contract::FencedWrite,
        _x: i32,
        _y: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        self.state.long_taps.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn swipe(
        &mut self,
        _witness: &actingcommand_contract::FencedWrite,
        _x1: i32,
        _y1: i32,
        _x2: i32,
        _y2: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        self.state.swipes.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn key(
        &mut self,
        _witness: &actingcommand_contract::FencedWrite,
        _key: &str,
    ) -> DeviceResult<()> {
        Ok(())
    }

    fn text(
        &mut self,
        _witness: &actingcommand_contract::FencedWrite,
        _text: &str,
    ) -> DeviceResult<()> {
        Ok(())
    }

    fn reset(&mut self, _witness: &actingcommand_contract::FencedWrite) -> DeviceResult<()> {
        Ok(())
    }

    fn close_once(
        &mut self,
        _authority: actingcommand_device::DeviceCloseAuthority,
    ) -> DeviceResult<actingcommand_device::DeviceResourceCloseOutcome> {
        if !self.closed {
            self.closed = true;
            self.state.closes.fetch_add(1, Ordering::AcqRel);
        }
        Ok(actingcommand_device::DeviceResourceCloseOutcome::confirmed(
            1,
        ))
    }
}

struct FakeProvider {
    instance_alias: &'static str,
    instance_id: InstanceId,
    state: Arc<FakeState>,
    frame_size: u32,
}

struct FakeCapture {
    state: Arc<FakeState>,
    frame_size: u32,
}

impl CaptureBackend for FakeCapture {
    fn capture(&mut self) -> DeviceResult<Frame> {
        let captured = self.state.captures.fetch_add(1, Ordering::AcqRel) + 1;
        let home_from = self.state.home_from_capture.load(Ordering::Acquire);
        if home_from > 0 && captured >= home_from {
            self.state
                .transition_after_tap
                .store(false, Ordering::Release);
        }
        if let Some(clock) = self.state.clock.get() {
            clock.advance(self.state.clock_ms_per_capture.load(Ordering::Acquire));
        }
        let color = if self.state.transition_after_tap.load(Ordering::Acquire)
            && (self.state.taps.load(Ordering::Acquire) > 0
                || self.state.long_taps.load(Ordering::Acquire) > 0)
        {
            [0, 0, 255]
        } else {
            [255, 0, 0]
        };
        let (width, height) = if self.state.physical_task_geometry.load(Ordering::Acquire) {
            (16, 9)
        } else {
            (self.frame_size, self.frame_size)
        };
        let pixels = (0..width * height).flat_map(|_| color).collect();
        Frame::from_pixels(
            width,
            height,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
    }
    fn observe_geometry(
        &mut self,
        _deadline: Instant,
    ) -> DeviceResult<actingcommand_contract::CaptureGeometryObservation> {
        use actingcommand_contract::{
            CaptureExtent, CaptureGeometry, CaptureGeometryObservation, CaptureGeometrySource,
            CaptureGeometryUnknownReason, CaptureRotation, CaptureRotationObservation,
            CaptureRotationSource, CaptureWmSizeKind,
        };
        if !self.state.physical_task_geometry.load(Ordering::Acquire) {
            return Ok(CaptureGeometryObservation::Unknown(
                CaptureGeometryUnknownReason::BackendUnsupported,
            ));
        }
        // The same 16x9 extent this fake serves; no device is queried.
        let extent = CaptureExtent::new(16, 9).expect("positive fake extent");
        Ok(CaptureGeometryObservation::Observed(CaptureGeometry {
            backend: CaptureBackendName::AdbScreencap,
            source: CaptureGeometrySource::AdbDefaultDisplay {
                serial: "<sealed-test>".to_string(),
                wm_extent: extent,
                wm_size_kind: CaptureWmSizeKind::Physical,
            },
            logical_display_extent: extent,
            rotation: CaptureRotationObservation::Observed {
                rotation: CaptureRotation::R0,
                source: CaptureRotationSource::DumpsysDisplayOrientation,
            },
            sampled_at: std::time::SystemTime::now(),
            frame_transform: None,
            safe_insets: None,
        }))
    }
    fn close_once(
        &mut self,
        _authority: actingcommand_device::DeviceCloseAuthority,
    ) -> DeviceResult<actingcommand_device::DeviceResourceCloseOutcome> {
        Ok(actingcommand_device::DeviceResourceCloseOutcome::confirmed(
            0,
        ))
    }
}

impl ExecutionBackendProvider for FakeProvider {
    fn instance_aliases(&self) -> Vec<String> {
        vec![self.instance_alias.to_string()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == self.instance_alias).then(|| {
            let resolved = ResolvedExecutionInstance::new(self.instance_id, "<sealed-test>");
            if !self.state.nemu_input.load(Ordering::Acquire) {
                return resolved;
            }
            // #411 Lab L3: an instance configured as a Nemu one (neutral test values).
            resolved.with_configuration(actingcommand_contract::EffectiveDeviceConfiguration {
                input_backend: "nemu_ipc".to_string(),
                capture_backend: "nemu_ipc".to_string(),
                input_adb: "<sealed-test>".to_string(),
                capture_adb: "<sealed-test>".to_string(),
                configured_serial: None,
                resolved_serial: "TESTSERIAL0001".to_string(),
                input_command_timeout_ms: 5_000,
                capture_command_timeout_ms: 5_000,
                capture_timeout_ms: 5_000,
                configured_mumu_root: None,
                configured_capture_dll: None,
            })
        })
    }

    fn open_input(
        &self,
        instance_alias: &str,
    ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn InputBackend>>> {
        let open = || -> DeviceResult<Box<dyn InputBackend>> {
            assert_eq!(instance_alias, self.instance_alias);
            Ok(Box::new(FakeBackend {
                state: Arc::clone(&self.state),
                closed: false,
            }))
        };
        let result: DeviceResult<Box<dyn InputBackend>> = open();
        result.map(|backend| {
            actingcommand_device::OpenedBackend::unobserved(
                backend,
                actingcommand_contract::BackendOpenEntry::Input,
            )
        })
    }

    fn open_capture(
        &self,
        instance_alias: &str,
        _memory: Option<&actingcommand_device::FrameMemoryBudget>,
    ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn CaptureBackend>>> {
        let open = || -> DeviceResult<Box<dyn CaptureBackend>> {
            assert_eq!(instance_alias, self.instance_alias);
            Ok(Box::new(FakeCapture {
                state: Arc::clone(&self.state),
                frame_size: self.frame_size,
            }))
        };
        let result: DeviceResult<Box<dyn CaptureBackend>> = open();
        result.map(|backend| {
            actingcommand_device::OpenedBackend::unobserved(
                backend,
                actingcommand_contract::BackendOpenEntry::Capture,
            )
        })
    }

    // Slice #316-B3: the fake device always reports its assigned application in the
    // foreground, so the foreground gate passes exactly as before the gate existed.
    fn observe_foreground_application(
        &self,
        _instance_alias: &str,
    ) -> DeviceResult<actingcommand_runtime_host::ForegroundApplicationObservation> {
        Ok(
            actingcommand_runtime_host::ForegroundApplicationObservation {
                foreground: Some("neutral.application".to_owned()),
                assigned: "neutral.application".to_owned(),
            },
        )
    }

    fn control_application(
        &self,
        _witness: &actingcommand_contract::FencedWrite,
        instance_alias: &str,
        action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        assert_eq!(instance_alias, self.instance_alias);
        self.state.application_calls.fetch_add(1, Ordering::AcqRel);
        self.state.application_action.store(
            match action {
                ApplicationLifecycleAction::Launch => 1,
                ApplicationLifecycleAction::Stop => 2,
                ApplicationLifecycleAction::Restart => 3,
            },
            Ordering::Release,
        );
        Ok(())
    }
}

#[test]
fn session_app_routes_application_lifecycle_through_runtime_without_client_package_identity() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-application-test"),
        Arc::new(FakeProvider {
            instance_alias: "neutral.instance",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 1,
        }),
    )
    .expect("runtime host");

    let lease_client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("lease client");
    let token = lease_client
        .acquire_lease("neutral.instance")
        .expect("active lease");
    let (busy_exit, busy) = run_actinglab_failure_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "neutral.instance",
            "session",
            "app",
            "force-stop",
        ],
    );
    assert_eq!(busy_exit, 4);
    assert_eq!(busy["error"]["code"], "device_error");
    // Workflow #369 W-3 (S5): the command waits its cap in the instance's queue, then is
    // refused `lease_queue_expired` (`queue_expired`) instead of an immediate `lease_busy`.
    assert!(
        busy["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("runtime code queue_expired"))
    );
    assert_eq!(state.application_calls.load(Ordering::Acquire), 0);
    lease_client
        .release_lease(&token)
        .expect("release active lease");

    let output = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "neutral.instance",
            "session",
            "app",
            "force-stop",
        ],
    );
    assert_eq!(
        output["data"]["receipt"]["result"]["kind"],
        "application_lifecycle_completed"
    );
    assert_eq!(output["data"]["receipt"]["result"]["action"], "stop");
    assert_eq!(state.application_calls.load(Ordering::Acquire), 1);
    assert_eq!(state.application_action.load(Ordering::Acquire), 2);

    let (exit_code, rejected) = run_actinglab_failure_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "neutral.instance",
            "session",
            "app",
            "launch",
            "--package",
            "client.supplied.identity",
        ],
    );
    assert_eq!(exit_code, 2);
    assert_eq!(rejected["error"]["code"], "validation_failed");
    assert_eq!(state.application_calls.load(Ordering::Acquire), 1);
    drop(lease_client);
    host.close().expect("close host");
}

#[test]
fn session_status_and_monitor_policy_project_resident_runtime_without_legacy_state() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let legacy_session_root = local_app_data.join("ActingCommand/actinglab/session");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-session-adapter-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state,
            frame_size: 1,
        }),
    )
    .expect("runtime host");

    let status = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        ["--json", "session", "status", "--diagnostics"],
    );
    assert_eq!(status["data"]["running"], true);
    assert_eq!(
        status["data"]["diagnostics"]["liveness"]["authority"],
        "runtime"
    );
    assert_eq!(
        status["data"]["diagnostics"]["instances"]["instances"][0]["instance_alias"],
        "node.a"
    );

    let unconfigured = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "session",
            "monitor-policy",
            "status",
        ],
    );
    assert_eq!(unconfigured["data"]["configured"], false);

    let configured = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "session",
            "monitor-policy",
            "set",
            "--capture",
            "--expect",
            "home",
            "--interval-ms",
            "60000",
        ],
    );
    assert_eq!(configured["data"]["status"], "configured");
    assert_eq!(
        configured["data"]["policy"]["runtime_policy"]["expected_page"],
        "home"
    );

    let cleared = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "session",
            "monitor-policy",
            "clear",
        ],
    );
    assert_eq!(cleared["data"]["status"], "cleared");
    assert_eq!(cleared["data"]["state_preserved"], false);
    assert!(!legacy_session_root.exists());
    host.close().expect("close host");
}

#[test]
fn session_stream_projects_runtime_capture_sequence_without_legacy_state() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let legacy_session_root = local_app_data.join("ActingCommand/actinglab/session");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-stream-adapter-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 2,
        }),
    )
    .expect("runtime host");

    let stream = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "session",
            "stream",
            "--max-frames",
            "2",
            "--interval-ms",
            "1",
        ],
    );

    assert_eq!(stream["data"]["mode"], "bounded_stream");
    for field in [
        "stream_id",
        "mode",
        "instance",
        "transport",
        "max_frames",
        "interval_ms",
        "capture",
        "trusted_channel",
        "contract",
        "input_relay",
        "events",
        "frames",
    ] {
        assert!(stream["data"].get(field).is_some(), "missing {field}");
    }
    assert_eq!(
        stream["data"]["contract"]["schema_version"],
        "session.stream.v0.1"
    );
    assert_eq!(stream["data"]["input_relay"]["status"], "disabled");
    let frames = stream["data"]["frames"].as_array().expect("stream frames");
    assert_eq!(frames.len(), 2);
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(frame["index"], index);
        assert_eq!(frame["captured"], true);
        assert_eq!(frame["frame"]["width"], 2);
        assert_eq!(frame["frame"]["height"], 2);
        assert_eq!(frame["freshness"]["status"], "runtime_artifact_verified");
        assert!(frame["artifact"]["object_key"].is_string());
        assert_eq!(frame["frame"]["digest"], frame["artifact"]["sha256"]);
    }
    let event_types = stream["data"]["events"]
        .as_array()
        .expect("stream events")
        .iter()
        .map(|event| event["type"].as_str().expect("event type"))
        .collect::<Vec<_>>();
    assert_eq!(
        event_types,
        [
            "stream.started",
            "stream.frame_sampled",
            "stream.frame_sampled",
            "stream.completed"
        ]
    );
    assert_eq!(state.captures.load(Ordering::Acquire), 2);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);
    assert!(!legacy_session_root.exists());

    let reconnected = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        ["--json", "session", "status"],
    );
    assert_eq!(reconnected["data"]["running"], true);

    let (fresh_exit, fresh_error) = run_actinglab_failure_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "session",
            "stream",
            "--max-frames",
            "2",
            "--require-fresh",
        ],
    );
    assert_eq!(fresh_exit, 2);
    assert_eq!(fresh_error["error"]["code"], "validation_failed");
    assert!(
        fresh_error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("not supported"))
    );
    assert_eq!(state.captures.load(Ordering::Acquire), 2);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_runtime_client::RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime remains discoverable");
    client.health().expect("Runtime remains alive");
    drop(client);
    host.close().expect("close host");
}

#[test]
fn runtime_backed_session_clients_fail_visibly_when_runtime_is_unavailable() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("missing-runtime");
    let local_app_data = root.path().join("local-app-data");
    let legacy_session_root = local_app_data.join("ActingCommand/actinglab/session");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");

    let failures = [
        run_actinglab_failure_json(
            &config_path,
            &runtime_root,
            &local_app_data,
            ["--json", "session", "status"],
        ),
        run_actinglab_failure_json(
            &config_path,
            &runtime_root,
            &local_app_data,
            [
                "--json",
                "--instance",
                "node.a",
                "session",
                "monitor-policy",
                "status",
            ],
        ),
        run_actinglab_failure_json(
            &config_path,
            &runtime_root,
            &local_app_data,
            [
                "--json",
                "--instance",
                "node.a",
                "session",
                "stream",
                "--max-frames",
                "2",
            ],
        ),
    ];
    for (exit_code, failure) in failures {
        assert_eq!(exit_code, 5);
        assert_eq!(failure["ok"], false);
        assert_eq!(failure["error"]["code"], "runtime_not_running");
        assert!(failure["data"].is_null());
    }
    assert!(!legacy_session_root.exists());
}

fn run_actinglab_json<const N: usize>(
    config_path: &Path,
    runtime_root: &Path,
    local_app_data: &Path,
    arguments: [&str; N],
) -> Value {
    let output = run_actinglab_output(config_path, runtime_root, local_app_data, arguments);
    assert!(
        output.status.success(),
        "actinglab failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("ActingLab JSON")
}

fn run_actinglab_failure_json<const N: usize>(
    config_path: &Path,
    runtime_root: &Path,
    local_app_data: &Path,
    arguments: [&str; N],
) -> (i32, Value) {
    let output = run_actinglab_output(config_path, runtime_root, local_app_data, arguments);
    assert!(
        !output.status.success(),
        "actinglab unexpectedly succeeded: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let exit_code = output.status.code().expect("actinglab exit code");
    let envelope = serde_json::from_slice(&output.stdout).expect("ActingLab error JSON");
    (exit_code, envelope)
}

fn run_actinglab_output<const N: usize>(
    config_path: &Path,
    runtime_root: &Path,
    local_app_data: &Path,
    arguments: [&str; N],
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args(arguments)
        .env("ACTINGLAB_CONFIG_PATH", config_path)
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", runtime_root)
        .env("LOCALAPPDATA", local_app_data)
        .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
        .env_remove("ACTINGLAB_SESSION_STATE_DIR")
        .output()
        .expect("run actinglab")
}

#[test]
fn lab_package_debug_is_a_correlated_runtime_request_without_device_authority() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    let package = root.path().join("debug-package.zip");
    write_runtime_owned_lab_package(&package);
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-package-debug-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 2,
        }),
    )
    .expect("runtime host");

    let output = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "lab",
            "debug-package",
            "--zip",
            package.to_str().expect("package path"),
        ],
    );
    assert_eq!(output["data"]["authority"], "runtime");
    assert_eq!(output["data"]["summary"]["task_id"], "task");
    // Workflow #336 PKG-S4 (option 丙): the package is named by its path; the summary names no
    // digest.
    assert!(
        output["data"]["summary"].get("verified_sha256").is_none(),
        "{output}"
    );
    assert_eq!(
        output["data"]["terminal_receipt"]["correlation_id"],
        output["data"]["correlation_id"]
    );
    assert!(
        output["data"]["events"]
            .as_array()
            .is_some_and(|events| !events.is_empty())
    );
    assert_eq!(state.captures.load(Ordering::Acquire), 0);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);

    let correlation_id = output["data"]["correlation_id"]
        .as_str()
        .expect("debug correlation");
    let watch = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "lab",
            "watch",
            "--req",
            correlation_id,
            "--after",
            "0",
            "--wait-ms",
            "50",
            "--max-events",
            "16",
        ],
    );
    assert_eq!(watch["data"]["authority"], "runtime_global_ledger");
    assert_eq!(watch["data"]["progress"]["state"], "advanced");
    assert!(
        watch["data"]["progress"]["event_count"]
            .as_u64()
            .is_some_and(|count| count > 0)
    );
    assert!(watch["data"]["progress"].get("percent").is_none());
    assert!(watch["data"]["progress"].get("completed").is_none());
    let first = &watch["data"]["events"][0];
    let target_sequence = first["sequence"].as_u64().expect("watch event sequence");
    let predecessor_sequence = target_sequence
        .checked_sub(1)
        .expect("watch event must have a positive sequence");
    let sequence = target_sequence.to_string();
    let after = predecessor_sequence.to_string();
    let filtered = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "lab",
            "watch",
            "--correlation-id",
            correlation_id,
            "--origin-module",
            first["origin"]["module"].as_str().unwrap(),
            "--source",
            first["origin"]["source"].as_str().unwrap(),
            "--event-type",
            first["event_type"].as_str().unwrap(),
            "--minimum-severity",
            first["severity"].as_str().unwrap(),
            "--from-sequence",
            &sequence,
            "--to-sequence",
            &sequence,
            "--after",
            &after,
            "--wait-ms",
            "1",
            "--max-events",
            "1",
        ],
    );
    if filtered["data"]["events"] != json!([first])
        || filtered["data"]["filter"]["correlation_id"] != correlation_id
        || filtered["data"]["filter"]["origin_module"] != first["origin"]["module"]
    {
        let mut context = [0_u8; 4 * 1024];
        let mut remaining = &mut context[..];
        let formatted = writeln!(
            remaining,
            "filtered subscription: S={target_sequence}; P={predecessor_sequence}; progress.next_sequence={}; progress.state={}; filter={}",
            filtered["data"]["progress"]["next_sequence"],
            filtered["data"]["progress"]["state"],
            filtered["data"]["filter"],
        );
        let used = 4 * 1024 - remaining.len();
        let text = match std::str::from_utf8(&context[..used]) {
            Ok(text) => text,
            Err(error) => std::str::from_utf8(&context[..error.valid_up_to()])
                .expect("valid subscription context prefix"),
        };
        eprint!("{text}");
        if let Err(error) = formatted {
            eprintln!(
                "\nfiltered subscription context truncated or formatting failed (4-KiB buffer): {error}"
            );
        }
    }
    assert_eq!(filtered["data"]["events"], json!([first]));
    assert_eq!(filtered["data"]["filter"]["correlation_id"], correlation_id);
    assert_eq!(
        filtered["data"]["filter"]["origin_module"],
        first["origin"]["module"]
    );
    let absent = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "lab",
            "watch",
            "--req",
            correlation_id,
            "--diagnostic-code",
            "owner_conflict",
            "--from-sequence",
            &sequence,
            "--to-sequence",
            &sequence,
            "--after",
            &after,
            "--wait-ms",
            "1",
            "--max-events",
            "1",
        ],
    );
    if absent["data"]["events"] != json!([]) || absent["data"]["progress"]["state"] != "idle" {
        let mut context = [0_u8; 4 * 1024];
        let mut remaining = &mut context[..];
        let formatted = writeln!(
            remaining,
            "absent subscription: S={target_sequence}; P={predecessor_sequence}; progress.next_sequence={}; progress.state={}; filter={}",
            absent["data"]["progress"]["next_sequence"],
            absent["data"]["progress"]["state"],
            absent["data"]["filter"],
        );
        let used = 4 * 1024 - remaining.len();
        let text = match std::str::from_utf8(&context[..used]) {
            Ok(text) => text,
            Err(error) => std::str::from_utf8(&context[..error.valid_up_to()])
                .expect("valid subscription context prefix"),
        };
        eprint!("{text}");
        if let Err(error) = formatted {
            eprintln!(
                "\nabsent subscription context truncated or formatting failed (4-KiB buffer): {error}"
            );
        }
    }
    assert_eq!(absent["data"]["events"], json!([]));
    assert_eq!(absent["data"]["progress"]["state"], "idle");
    for invalid in [
        vec!["--origin-module", "unknown"],
        vec!["--diagnostic-code", "unknown"],
        vec!["--request-id", correlation_id],
        vec!["--req"],
        vec!["--req", correlation_id, "--correlation-id", correlation_id],
        vec!["--origin-module", "runtime", "--origin-module", "runtime"],
        vec!["--unsupported", "value"],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_actinglab"))
            .args(["--json", "lab", "watch"])
            .args(invalid)
            .env("ACTINGLAB_CONFIG_PATH", &config_path)
            .env(
                "ACTINGCOMMAND_RUNTIME_STATE_ROOT",
                root.path().join("absent-runtime"),
            )
            .env("LOCALAPPDATA", &local_app_data)
            .output()
            .expect("invalid watch command");
        assert_eq!(
            result.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
    }
    assert_eq!(state.captures.load(Ordering::Acquire), 0);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);

    assert_eq!(host.runtime_info().pid(), std::process::id());
    host.close().expect("close host");
}

#[test]
fn runtime_owned_evidence_export_without_formal_summary_fails_closed() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let config_path = root.path().join("actinglab.json");
    let package = root.path().join("debug-package.zip");
    let evidence = root.path().join("runtime-evidence.zip");
    fs::write(&config_path, "{}").expect("write config");
    write_runtime_owned_lab_package(&package);
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-evidence-export-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 2,
        }),
    )
    .expect("runtime host");

    let export_output = run_actinglab_output(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "lab",
            "export-evidence",
            "--zip",
            package.to_str().expect("package path"),
            "--out",
            evidence.to_str().expect("evidence path"),
            "--outcome",
            "success",
        ],
    );
    assert!(!export_output.status.success());
    let export = serde_json::from_slice::<Value>(&export_output.stdout).expect("export JSON");
    assert_eq!(export["ok"], false);
    assert_eq!(export["error"]["code"], "device_error");
    let message = export["error"]["message"]
        .as_str()
        .expect("typed export failure message");
    assert!(message.contains("runtime_request_rejected"));
    assert!(message.contains("evidence_export_failed"));
    assert!(!evidence.exists());
    assert_eq!(state.captures.load(Ordering::Acquire), 0);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_runtime_client::RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime client");
    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("Runtime events");
    assert!(
        events
            .iter()
            .all(|event| event.event_type != EventType::CaptureSummaryCommitted)
    );
    assert!(
        events
            .iter()
            .all(|event| event.event_type != EventType::ArtifactExportCompleted)
    );
    assert!(
        events
            .iter()
            .flat_map(|event| &event.artifacts)
            .all(|artifact| artifact.kind() != ArtifactKind::EvidenceArchive)
    );
    drop(client);

    assert!(host.fatal_error().expect("Runtime fatal state").is_none());
    host.close().expect("close host");
}

#[test]
fn runtime_debug_session_export_without_formal_summary_fails_closed() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let package = root.path().join("debug-package.zip");
    let evidence = root.path().join("captured-evidence.zip");
    write_runtime_owned_lab_package(&package);
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-captured-evidence-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 2,
        }),
    )
    .expect("runtime host");
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_runtime_client::RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime client");
    let session = client.begin_debug_session().expect("debug session");
    session
        .debug_package(
            PackageDebugRequest::new(package.to_string_lossy().into_owned())
                .expect("debug package request"),
        )
        .expect("debug package");
    session
        .capture_sequence(
            "node.a",
            CaptureSequenceSpec::new(1, 0).expect("capture spec"),
        )
        .expect("Runtime capture sequence");
    let error = session
        .export_evidence(
            RuntimeEvidenceExportRequest::new(
                evidence.to_string_lossy().into_owned(),
                TaskOutcome::Success,
            )
            .expect("evidence request"),
        )
        .expect_err("missing formal capture summary must fail closed");
    assert_eq!(error.code().as_str(), "runtime_request_rejected");
    assert_eq!(error.operation().as_str(), "export_evidence");
    assert_eq!(
        error.projection().expect("typed export projection").code,
        RuntimeErrorCode::EvidenceExportFailed
    );
    assert!(!error.is_fatal());
    assert!(error.committed_receipt().is_none());
    assert!(!evidence.exists());
    let events = session
        .query_events(ProjectionProfile::Forensic)
        .expect("debug events");
    let captures = events
        .iter()
        .flat_map(|event| &event.artifacts)
        .filter(|artifact| artifact.kind() == actingcommand_contract::ArtifactKind::CaptureFrame)
        .collect::<Vec<_>>();
    assert!(!captures.is_empty());
    assert!(
        captures
            .iter()
            .all(|artifact| artifact.retention_class == RetentionClass::DebugFull)
    );
    assert!(
        events
            .iter()
            .all(|event| event.event_type != EventType::CaptureSummaryCommitted)
    );
    assert!(
        events
            .iter()
            .all(|event| event.event_type != EventType::ArtifactExportCompleted)
    );
    assert!(
        events
            .iter()
            .flat_map(|event| &event.artifacts)
            .all(|artifact| artifact.kind() != ArtifactKind::EvidenceArchive)
    );
    assert_eq!(state.captures.load(Ordering::Acquire), 1);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn production_do_uses_runtime_capture_and_fenced_input() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let resources = root.path().join("resources");
    let semantic_package = root.path().join("semantic.zip");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    write_semantic_resources(&resources);
    write_semantic_package(&semantic_package, &resources);
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-drive-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 1,
        }),
    )
    .expect("runtime host");

    let output = Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args([
            "--json",
            "--instance",
            "node.a",
            "do",
            "--tap",
            "0,0",
            "--capture",
            "--zip",
            semantic_package.to_str().expect("semantic package path"),
        ])
        .env("ACTINGLAB_CONFIG_PATH", &config_path)
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", &runtime_root)
        .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
        .env_remove("ACTINGLAB_SESSION_STATE_DIR")
        .env_remove("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG")
        .output()
        .expect("run actinglab do");

    assert!(
        output.status.success(),
        "actinglab failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = serde_json::from_slice::<Value>(&output.stdout).expect("CLI JSON");
    assert_eq!(
        envelope
            .pointer("/data/device/authority")
            .and_then(Value::as_str),
        Some("runtime_execution_kernel")
    );
    assert!(envelope.pointer("/data/needs_detection").is_none());
    assert_eq!(state.captures.load(Ordering::Acquire), 2);
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
    host.close().expect("close host");
}

#[test]
fn online_lab2_observe_and_do_share_runtime_authority_without_local_state() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let resources = root.path().join("resources");
    let semantic_package = root.path().join("semantic.zip");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    write_navigation_resources(&resources);
    let pack = fs::read_to_string(resources.join("recognition/arknights.cn.pack.json"))
        .unwrap()
        .replace("[0,0,255]", "[0,255,0]");
    let pages = fs::read(resources.join("recognition/arknights.cn.pages.json")).unwrap();
    let navigation = fs::read(resources.join("navigation/arknights.cn.navigation.json")).unwrap();
    write_zip(&semantic_package, &[
        ("control.json", br#"{"game":"arknights","server":"cn","entry_task_id":"task"}"#),
        ("resources/manifest.json", br#"{"schema_version":"0.3","entry_task_id":"task"}"#),
        ("resources/operations/task/task.json", br#"{"task_id":"task","post_admission_ocr":{"mode":"fields_v1","fields":[{"id":"name","target_id":"home_anchor","privacy":"personal"}]}}"#),
        ("resources/recognition/arknights.cn.pack.json", pack.as_bytes()),
        ("resources/recognition/arknights.cn.pages.json", &pages),
        ("resources/navigation/arknights.cn.navigation.json", &navigation),
    ]);
    let state = Arc::new(FakeState::default());
    state.transition_after_tap.store(true, Ordering::Release);
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-lab2-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 1,
        }),
    )
    .expect("runtime host");

    let observe = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "observe",
            "--capture",
            "--zip",
            semantic_package.to_str().expect("semantic package path"),
            "--verbose",
        ],
    );
    assert_eq!(
        observe
            .pointer("/data/arbitration/authority")
            .and_then(Value::as_str),
        Some("runtime_scheduler")
    );

    let element = observe["data"]["observation"]["elements"][0]["id"]
        .as_str()
        .expect("current element")
        .to_string();
    let old_sequence = observe["data"]["projection_source"]["projection_sequence"]
        .as_u64()
        .unwrap()
        .to_string();
    let old_hash = observe["data"]["projection_source"]["content_sha256"]
        .as_str()
        .unwrap();
    let action = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "do",
            &element,
            "--capture",
            "--zip",
            semantic_package.to_str().expect("semantic package path"),
            "--projection-sequence",
            &old_sequence,
            "--projection-hash",
            old_hash,
            "--verbose",
        ],
    );
    assert_eq!(
        action.pointer("/data/executed").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        action
            .pointer("/data/device/authority")
            .and_then(Value::as_str),
        Some("runtime_execution_kernel")
    );
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
    assert!(state.captures.load(Ordering::Acquire) >= 3);
    assert!(!local_app_data.join("ActingCommand/actinglab/lab2").exists());
    let operation: actingcommand_contract::ContainedLabOperationResult =
        serde_json::from_value(action["data"]["operation_record"].clone()).unwrap();
    assert_eq!(
        operation
            .record
            .prepared
            .projection_hint
            .sequence
            .unwrap()
            .to_string(),
        old_sequence
    );
    assert!(
        operation
            .record
            .prepared
            .before_projection
            .as_ref()
            .unwrap()
            .projection_sequence
            > old_sequence.parse::<u64>().unwrap()
    );
    assert_eq!(
        operation.record.effect,
        actingcommand_contract::EffectDisposition::Performed
    );
    assert!(operation.record.failure.is_none());
    assert!(
        !operation
            .record
            .after_projection
            .as_ref()
            .unwrap()
            .projection
            .matched
    );
    let before = operation
        .record
        .prepared
        .before_projection
        .as_ref()
        .unwrap();
    let raw =
        actingcommand_artifact_store::read_projected_verified(&runtime_root, &before.artifact)
            .unwrap();
    let evidence: actingcommand_contract::ContainedObservationEvidence =
        serde_json::from_slice(&raw).unwrap();
    assert!(
        evidence
            .private_facts
            .rows
            .iter()
            .any(|row| row["target"]["target_id"] == "home_anchor"
                && !row["target"]["evaluation"].is_null())
    );
    assert!(
        before
            .facts
            .rows
            .iter()
            .any(|row| row["target"]["target_id"] == "home_anchor"
                && row["target"]["redacted"] == true
                && row["target"]["evaluation"].is_null())
    );
    assert!(
        !serde_json::to_string(&action)
            .unwrap()
            .contains("private_facts")
    );
    for (flag, coordinates) in [("--tap", "0,0"), ("--swipe", "0,0,0,0,1")] {
        let explicit = run_actinglab_json(
            &config_path,
            &runtime_root,
            &local_app_data,
            [
                "--json",
                "--instance",
                "node.a",
                "do",
                flag,
                coordinates,
                "--capture",
                "--zip",
                semantic_package.to_str().unwrap(),
                "--projection-sequence",
                &old_sequence,
                "--projection-hash",
                old_hash,
                "--verbose",
            ],
        );
        assert_eq!(explicit["data"]["executed"], true);
        assert_eq!(explicit["data"]["effect"], "performed");
        assert_eq!(
            explicit["data"]["operation_record"]["record"]["prepared"]["selection"]["mode"],
            "coordinates"
        );
        assert_eq!(
            explicit["data"]["operation_record"]["record"]["prepared"]["selected_element"],
            Value::Null
        );
        assert_eq!(
            explicit["data"]["operation_record"]["record"]["prepared"]["before_projection"]["projection"]
                ["matched"],
            false
        );
    }
    assert_eq!(state.taps.load(Ordering::Acquire), 2);
    assert_eq!(state.captures.load(Ordering::Acquire), 7);

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_runtime_client::RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime client");
    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("Runtime events");
    let input = events
        .iter()
        .find(|event| event.event_type == EventType::InputCommitted)
        .expect("input committed");
    let correlation = input.links.correlation_id().copied().expect("correlation");
    let correlated = events
        .iter()
        .filter(|event| event.links.correlation_id().copied() == Some(correlation))
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
    assert_event_order(
        &correlated,
        &[
            EventType::LeaseGranted,
            EventType::CaptureCompleted,
            EventType::InputCommitted,
            EventType::CaptureCompleted,
            EventType::LeaseReleased,
        ],
    );
    let inputs = events
        .iter()
        .filter(|event| event.event_type == EventType::InputCommitted)
        .collect::<Vec<_>>();
    assert_eq!(inputs.len(), 3);
    for input in inputs {
        let request = input.links.request_id();
        let chain = events
            .iter()
            .filter(|event| event.links.request_id() == request)
            .collect::<Vec<_>>();
        assert_eq!(
            chain
                .iter()
                .filter(|event| event.event_type == EventType::InputIntent)
                .count(),
            1
        );
        assert_eq!(
            chain
                .iter()
                .filter(|event| event.event_type == EventType::CaptureCompleted)
                .count(),
            2
        );
        assert_eq!(
            chain
                .iter()
                .filter(|event| event.event_type == EventType::ArtifactVerified
                    && event.artifacts[0].kind == ArtifactKind::DiagnosticJson)
                .count(),
            4
        );
        assert!(
            chain
                .iter()
                .filter(|event| matches!(
                    event.event_type,
                    EventType::CaptureCompleted
                        | EventType::InputIntent
                        | EventType::InputCommitted
                ))
                .all(|event| event.links.lease_id() == input.links.lease_id())
        );
    }

    drop(client);
    host.close().expect("close host");
}

#[test]
fn online_lab2_do_guard_failure_records_observation_without_runtime_input() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let resources = root.path().join("resources");
    let semantic_package = root.path().join("semantic.zip");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    write_semantic_resources(&resources);
    let pack_path = resources.join("recognition/arknights.cn.pack.json");
    let pack = fs::read_to_string(&pack_path).expect("recognition pack");
    fs::write(&pack_path, pack.replace("[255,0,0]", "[0,0,255]"))
        .expect("mismatched recognition pack");
    fs::write(
        resources.join("recognition/arknights.cn.pages.json"),
        br#"{"schema_version":"0.3","pages":[{"id":"arknights/home","required":["home_button"]}]}"#,
    )
    .expect("declared arrival page");
    write_semantic_package(&semantic_package, &resources);
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-lab2-guard-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 1,
        }),
    )
    .expect("runtime host");

    let (exit_code, failure) = run_actinglab_failure_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "do",
            "home_button",
            "--capture",
            "--zip",
            semantic_package.to_str().expect("semantic package path"),
            "--verbose",
        ],
    );
    assert_eq!(exit_code, 3, "{failure}");
    assert_eq!(failure["error"]["code"], "capability_insufficient");
    assert_eq!(failure["error"]["details"]["failure"]["stage"], "selection");
    assert_eq!(failure["error"]["details"]["effect"], "not_performed");
    assert_eq!(
        failure["error"]["details"]["ledger"]["authority"],
        "runtime_global_ledger"
    );
    assert_eq!(state.taps.load(Ordering::Acquire), 0);
    assert_eq!(state.captures.load(Ordering::Acquire), 1);

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_runtime_client::RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime client");
    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("Runtime events");
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::CaptureCompleted)
    );
    assert!(
        events
            .iter()
            .all(|event| event.event_type != EventType::InputCommitted)
    );

    let (_, outside) = run_actinglab_failure_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "do",
            "--tap",
            "1,0",
            "--capture",
            "--zip",
            semantic_package.to_str().unwrap(),
            "--verbose",
        ],
    );
    assert_eq!(
        outside["error"]["details"]["failure"]["code"],
        "lab_coordinates_out_of_frame"
    );
    assert_eq!(outside["error"]["details"]["effect"], "not_performed");
    assert_eq!(state.taps.load(Ordering::Acquire), 0);
    assert_eq!(state.captures.load(Ordering::Acquire), 2);

    state.tap_started.store(false, Ordering::Release);
    state.tap_delay_ms.store(2_000, Ordering::Release);
    let (post_failure, queued) = thread::scope(|scope| {
        let operation = scope.spawn(|| {
            run_actinglab_output(
                &config_path,
                &runtime_root,
                &local_app_data,
                [
                    "--json",
                    "--instance",
                    "node.a",
                    "do",
                    "--tap",
                    "0,0",
                    "--capture",
                    "--zip",
                    semantic_package.to_str().unwrap(),
                    "--after-page",
                    "arknights/home",
                    "--verbose",
                ],
            )
        });
        wait_until(Duration::from_secs(5), || {
            state.tap_started.load(Ordering::Acquire)
        });
        let queued = client
            .queue_lease(
                "node.a",
                actingcommand_contract::LeaseQueuePolicy::new(
                    actingcommand_contract::LeasePriority::High,
                    5_000,
                )
                .unwrap(),
            )
            .unwrap();
        let actingcommand_runtime_client::LeaseAdmission::Queued(queued) = queued else {
            panic!("input's destructive step must defer transfer");
        };
        assert!(queued.preempt_requested());
        (operation.join().unwrap(), queued)
    });
    assert!(!post_failure.status.success());
    let post_failure: Value = serde_json::from_slice(&post_failure.stdout).unwrap();
    let details = &post_failure["error"]["details"];
    assert_eq!(details["effect"], "performed");
    assert_eq!(details["executed"], true);
    assert_eq!(details["failure"]["stage"], "after_frame");
    assert_eq!(details["arrival"]["status"], "aborted");
    assert_eq!(details["arrival"]["samples"], serde_json::json!([]));
    assert!(details["after"].is_null());
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
    assert_eq!(state.captures.load(Ordering::Acquire), 3);
    let operation: actingcommand_contract::ContainedLabOperationResult =
        serde_json::from_value(details["operation_record"].clone()).unwrap();
    let native = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .unwrap();
    let input = native
        .iter()
        .find(|event| event.sequence == operation.record.input_event.unwrap().sequence)
        .unwrap();
    assert_eq!(input.event_type, EventType::InputCommitted);
    assert_eq!(
        input.links.lease_id().copied(),
        operation.record.prepared.lease_id
    );
    assert!(
        native
            .iter()
            .any(|event| event.event_type == EventType::LeaseTransferred
                && event.sequence > input.sequence)
    );
    assert!(operation.record.after_frame.is_none());
    assert!(operation.record.after_projection.is_none());
    let actingcommand_runtime_client::LeaseAdmission::Granted(next_token) =
        client.poll_queued_lease(queued.request_id()).unwrap()
    else {
        panic!("queued lease must transfer at the existing input boundary");
    };
    assert_ne!(
        Some(next_token.lease_id()),
        operation.record.prepared.lease_id
    );
    client.release_lease(&next_token).unwrap();

    drop(client);
    host.close().expect("close host");

    let failed_root = root.path().join("artifact-failure-runtime");
    let failed_state = Arc::new(FakeState::default());
    let failed_host = RuntimeHost::start(
        RuntimeHostConfig::new(&failed_root, b"lab-operation-artifact-failure"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: failed_state.clone(),
            frame_size: 1,
        }),
    )
    .unwrap();
    fs::write(
        failed_root.join("artifacts"),
        b"blocks original artifact directory",
    )
    .unwrap();
    let (exit, fatal) = run_actinglab_failure_json(
        &config_path,
        &failed_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "do",
            "--tap",
            "0,0",
            "--capture",
            "--zip",
            semantic_package.to_str().unwrap(),
            "--verbose",
        ],
    );
    assert_ne!(exit, 0, "{fatal}");
    assert_eq!(failed_state.taps.load(Ordering::Acquire), 0);
    assert!(failed_host.close().is_err());
}

#[test]
fn online_lab2_ensure_and_wait_use_runtime_authority_without_local_state() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let resources = root.path().join("resources");
    let semantic_package = root.path().join("navigation.zip");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    write_navigation_resources(&resources);
    write_semantic_package(&semantic_package, &resources);
    let state = Arc::new(FakeState::default());
    state.transition_after_tap.store(true, Ordering::Release);
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-lab2-route-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 1,
        }),
    )
    .expect("runtime host");

    let wait_home = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "wait",
            "--capture",
            "--page",
            "home",
            "--zip",
            semantic_package.to_str().expect("semantic package path"),
        ],
    );
    assert_eq!(wait_home["data"]["state"], "arrived");
    assert_eq!(
        wait_home["data"]["arbitration"]["authority"],
        "runtime_scheduler"
    );

    let ensure = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "ensure",
            "--capture",
            "--to",
            "target",
            "--step-timeout-ms",
            "100",
            "--poll-ms",
            "1",
            "--zip",
            semantic_package.to_str().expect("semantic package path"),
        ],
    );
    assert_eq!(ensure["data"]["state"], "arrived");
    assert_eq!(ensure["data"]["page"], "arknights/target");
    assert_eq!(ensure["data"]["executed"], true);
    assert_eq!(
        ensure["data"]["arbitration"]["authority"],
        "runtime_scheduler"
    );

    let wait_stable = run_actinglab_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "--instance",
            "node.a",
            "wait",
            "--capture",
            "--stable",
            "target_anchor",
            "--zip",
            semantic_package.to_str().expect("semantic package path"),
        ],
    );
    assert_eq!(wait_stable["data"]["state"], "stable");
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
    assert!(state.captures.load(Ordering::Acquire) >= 6);
    assert!(!local_app_data.join("ActingCommand/actinglab/lab2").exists());

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_runtime_client::RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime client");
    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("Runtime events");
    let input = events
        .iter()
        .find(|event| event.event_type == EventType::InputCommitted)
        .expect("input committed");
    let correlation = input.links.correlation_id().copied().expect("correlation");
    let correlated = events
        .iter()
        .filter(|event| event.links.correlation_id().copied() == Some(correlation))
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
    assert_event_order(
        &correlated,
        &[
            EventType::CaptureCompleted,
            EventType::LeaseGranted,
            EventType::InputCommitted,
            EventType::LeaseReleased,
            EventType::CaptureCompleted,
        ],
    );

    drop(client);
    host.close().expect("close host");
}

fn write_semantic_resources(root: &std::path::Path) {
    let recognition = root.join("recognition");
    let navigation = root.join("navigation");
    fs::create_dir_all(&recognition).expect("recognition dir");
    fs::create_dir_all(&navigation).expect("navigation dir");
    fs::write(
        recognition.join("arknights.cn.pack.json"),
        r#"{
            "schema_version":"0.3",
            "coordinate_space":{"width":1,"height":1},
            "targets":[
                {"type":"color","id":"home_button","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0],"click":{"x":10,"y":20,"width":4,"height":6}}
            ]
        }"#,
    )
    .expect("recognition pack");
    fs::write(
        recognition.join("arknights.cn.pages.json"),
        r#"{"schema_version":"0.3","pages":[]}"#,
    )
    .expect("page set");
    fs::write(
        navigation.join("arknights.cn.navigation.json"),
        r#"{"schema_version":"0.3","game":"arknights","server":"cn","navigation":[],"destructive_actions":[]}"#,
    )
    .expect("navigation graph");
}

fn write_navigation_resources(root: &std::path::Path) {
    let recognition = root.join("recognition");
    let navigation = root.join("navigation");
    fs::create_dir_all(&recognition).expect("recognition dir");
    fs::create_dir_all(&navigation).expect("navigation dir");
    fs::write(
        recognition.join("arknights.cn.pack.json"),
        r#"{
            "schema_version":"0.3",
            "coordinate_space":{"width":1,"height":1},
            "targets":[
                {"type":"color","id":"home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                {"type":"color","id":"target_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]}
            ]
        }"#,
    )
    .expect("recognition pack");
    fs::write(
        recognition.join("arknights.cn.pages.json"),
        r#"{
            "schema_version":"0.3",
            "pages":[
                {"id":"arknights/home","required":["home_anchor"]},
                {"id":"arknights/target","required":["target_anchor"]}
            ]
        }"#,
    )
    .expect("page set");
    fs::write(
        navigation.join("arknights.cn.navigation.json"),
        r#"{
            "schema_version":"0.3",
            "game":"arknights",
            "server":"cn",
            "navigation":[{
                "id":"home_to_target",
                "from_page":"arknights/home",
                "to_page":"arknights/target",
                "click":{"kind":"point","x":0,"y":0}
            }],
            "destructive_actions":[]
        }"#,
    )
    .expect("navigation graph");
}

fn write_semantic_package(path: &Path, root: &Path) {
    let pack = fs::read(root.join("recognition/arknights.cn.pack.json")).expect("pack");
    let pages = fs::read(root.join("recognition/arknights.cn.pages.json")).expect("pages");
    let navigation =
        fs::read(root.join("navigation/arknights.cn.navigation.json")).expect("navigation");
    write_zip(
        path,
        &[
            (
                "control.json",
                br#"{"game":"arknights","server":"cn","entry_task_id":"task"}"#,
            ),
            (
                "resources/manifest.json",
                br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
            ),
            ("resources/operations/task/task.json", br#"{}"#),
            ("resources/recognition/arknights.cn.pack.json", &pack),
            ("resources/recognition/arknights.cn.pages.json", &pages),
            (
                "resources/navigation/arknights.cn.navigation.json",
                &navigation,
            ),
        ],
    );
}

#[test]
fn production_tap_uses_runtime_proxy_without_local_adb_configuration() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let config_path = root.path().join("actinglab.json");
    fs::write(&config_path, "{}").expect("write config");
    let state = Arc::new(FakeState::default());
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-proxy-test"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 1,
        }),
    )
    .expect("runtime host");

    let output = Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args(["--instance", "node.a", "tap", "10", "20"])
        .env("ACTINGLAB_CONFIG_PATH", &config_path)
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", &runtime_root)
        .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
        .env_remove("ACTINGLAB_SESSION_STATE_DIR")
        .env_remove("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG")
        .output()
        .expect("run actinglab tap");

    assert!(
        output.status.success(),
        "actinglab failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = serde_json::from_slice::<Value>(&output.stdout).expect("CLI JSON");
    assert_eq!(
        envelope.pointer("/data/backend").and_then(Value::as_str),
        Some("runtime_proxy")
    );
    let outcome = envelope
        .pointer("/data/input_outcome")
        .expect("single-input outcome");
    assert_eq!(outcome["input_stage"], "committed");
    assert_eq!(outcome["close_stage"], "succeeded");
    let receipt: actingcommand_contract::RuntimeReceipt =
        serde_json::from_value(outcome["input_receipt"].clone())
            .expect("original Runtime input receipt");
    receipt.validate().expect("valid input receipt envelope");
    assert_eq!(
        receipt.state(),
        actingcommand_contract::RuntimeReceiptState::Completed
    );
    assert!(receipt.terminal().is_some());
    assert!(matches!(
        receipt.result(),
        Some(actingcommand_contract::RuntimeResult::InputCommitted { .. })
    ));
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
    assert_eq!(state.closes.load(Ordering::Acquire), 0);
    host.close().expect("close host");
    assert_eq!(state.closes.load(Ordering::Acquire), 1);
}

#[test]
fn production_lab_run_routes_device_effects_through_runtime_only() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let config_path = root.path().join("actinglab.json");
    let package_path = root.path().join("runtime-owned-lab.zip");
    let result_path = root.path().join("result.zip");
    let adb_marker = root.path().join("forbidden-adb-invoked");
    fs::write(&config_path, "{}").expect("write config");
    write_runtime_owned_lab_package(&package_path);
    let forbidden_adb = write_forbidden_adb(root.path(), &adb_marker);
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state.transition_after_tap.store(true, Ordering::Release);
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-run-test"),
        Arc::new(FakeProvider {
            instance_alias: "neutral.instance",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 2,
        }),
    )
    .expect("runtime host");

    let output = Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args([
            "--json",
            "--instance",
            "neutral.instance",
            "--game",
            "neutral",
            "--server",
            "test",
            "lab",
            "run",
            "--zip",
            package_path.to_str().expect("package path"),
            "--out",
            result_path.to_str().expect("result path"),
        ])
        .env("ACTINGLAB_CONFIG_PATH", &config_path)
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", &runtime_root)
        .env("ACTINGCOMMAND_ADB_PATH", &forbidden_adb)
        .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
        .env_remove("ACTINGLAB_SESSION_STATE_DIR")
        .env_remove("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG")
        .output()
        .expect("run actinglab lab run");

    assert!(output.status.success(), "Runtime-owned task must complete");
    let envelope = serde_json::from_slice::<Value>(&output.stdout).expect("CLI JSON");
    assert_eq!(
        envelope
            .pointer("/data/runtime_flow/receipt/result/kind")
            .and_then(Value::as_str),
        Some("contained_task_completed"),
        "unexpected Lab run response: {envelope}"
    );
    assert_eq!(
        envelope
            .pointer("/data/runtime_flow/receipt/result/final_page")
            .and_then(Value::as_str),
        Some("neutral/terminal")
    );
    assert!(result_path.is_file());
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
    assert!(state.captures.load(Ordering::Acquire) >= 2);
    assert_eq!(state.closes.load(Ordering::Acquire), 0);
    assert!(
        !adb_marker.exists(),
        "ActingLab invoked a local ADB backend"
    );

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_runtime_client::RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime client");
    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("Runtime events");
    let terminal = events
        .iter()
        .find(|event| {
            event.event_type == EventType::TaskCompleted
                && matches!(
                    &event.payload,
                    ProjectionPayload::Full(payload)
                        if payload.as_ref().action() == EventAction::RuntimeTaskRun
                )
        })
        .expect("Runtime Lab run terminal");
    let correlation = *terminal
        .links
        .correlation_id()
        .expect("Runtime Lab run correlation");
    let correlated = events
        .iter()
        .filter(|event| event.links.correlation_id() == Some(&correlation))
        .collect::<Vec<_>>();
    let event_types = correlated
        .iter()
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
    assert_event_order(
        &event_types,
        &[
            EventType::CommandReceived,
            EventType::CommandValidated,
            EventType::LeaseRequested,
            EventType::LeaseGranted,
            EventType::TaskRequested,
            EventType::TaskStarted,
            EventType::CaptureRequested,
            EventType::CaptureCompleted,
            EventType::RecognitionCompleted,
            EventType::TaskStepStarted,
            EventType::InputIntent,
            EventType::InputCommitted,
            EventType::TaskStepFinished,
            EventType::TaskTerminalIntent,
            EventType::TaskCompleted,
            EventType::LeaseReleased,
        ],
    );
    let actions = correlated
        .iter()
        .filter_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => Some(payload.as_ref().action()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(actions.contains(&EventAction::RuntimeTaskRun));

    drop(client);
    host.close().expect("close host");
    assert_eq!(state.closes.load(Ordering::Acquire), 1);
}

#[test]
fn runtime_finishes_and_rebuilds_lab_run_after_actinglab_client_is_killed() {
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let config_path = root.path().join("actinglab.json");
    let package_path = root.path().join("runtime-owned-lab.zip");
    let result_path = root.path().join("client-result.zip");
    let adb_marker = root.path().join("forbidden-adb-invoked");
    fs::write(&config_path, "{}").expect("write config");
    write_runtime_owned_lab_package(&package_path);
    let forbidden_adb = write_forbidden_adb(root.path(), &adb_marker);
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state.transition_after_tap.store(true, Ordering::Release);
    state.tap_delay_ms.store(500, Ordering::Release);
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-killed-client-test"),
        Arc::new(FakeProvider {
            instance_alias: "neutral.instance",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 2,
        }),
    )
    .expect("runtime host");

    let mut client_process = Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args([
            "--json",
            "--instance",
            "neutral.instance",
            "--game",
            "neutral",
            "--server",
            "test",
            "lab",
            "run",
            "--zip",
            package_path.to_str().expect("package path"),
            "--out",
            result_path.to_str().expect("result path"),
        ])
        .env("ACTINGLAB_CONFIG_PATH", &config_path)
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", &runtime_root)
        .env("LOCALAPPDATA", &local_app_data)
        .env("ACTINGCOMMAND_ADB_PATH", &forbidden_adb)
        .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
        .env_remove("ACTINGLAB_SESSION_STATE_DIR")
        .env_remove("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ActingLab client");
    wait_until(Duration::from_secs(5), || {
        state.tap_started.load(Ordering::Acquire)
    });
    client_process.kill().expect("kill ActingLab client");
    let status = client_process.wait().expect("wait killed ActingLab client");
    assert!(!status.success(), "killed ActingLab client succeeded");

    wait_until(Duration::from_secs(5), || {
        state.taps.load(Ordering::Acquire) == 1 && state.captures.load(Ordering::Acquire) >= 2
    });
    assert!(
        !result_path.exists(),
        "killed client unexpectedly published its local result projection"
    );
    assert!(
        !adb_marker.exists(),
        "ActingLab invoked a local ADB backend"
    );

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_runtime_client::RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("fresh Runtime client");
    let events = wait_for_runtime_task_terminal(&client);
    let facts = events
        .iter()
        .filter_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => Some(payload.fact()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    for required in [
        "package_admitted",
        "run_started",
        "evidence_indexed",
        "recognition_started",
        "recognition_completed",
        "step_started",
        "effect_intent",
        "effect_completed",
        "step_finished",
        "finalizing",
        "terminal_committed",
    ] {
        assert!(
            facts.iter().any(|fact| task_fact_kind(fact) == required),
            "missing Runtime semantic fact {required}: {facts:#?}"
        );
    }
    assert!(facts.iter().any(|fact| matches!(
        fact,
        TaskSemanticFact::TerminalCommitted {
            task_result: TaskOutcome::Success,
            final_page: Some(page),
            executed_steps: Some(1),
            outcome: None,
            ..
        } if page == "neutral/terminal"
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::TaskCompleted)
            .count(),
        1
    );

    drop(client);
    host.close().expect("close host");
    assert_eq!(state.closes.load(Ordering::Acquire), 1);
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let started = Instant::now();
    while !predicate() {
        assert!(
            started.elapsed() < timeout,
            "timed out waiting for condition"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_runtime_task_terminal(
    client: &RuntimeClient,
) -> Vec<actingcommand_contract::ProjectedEvent> {
    let started = Instant::now();
    loop {
        let events = client
            .query_events(EventQuery::default(), ProjectionProfile::Forensic)
            .expect("query Runtime ledger after ActingLab client kill");
        if events.iter().any(|event| {
            matches!(
                event.event_type,
                EventType::TaskCompleted | EventType::TaskFailed | EventType::TaskCancelled
            )
        }) {
            return events;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "Runtime task terminal did not become durable after ActingLab client kill"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn task_fact_kind(fact: &TaskSemanticFact) -> &'static str {
    match fact {
        TaskSemanticFact::PackageAdmitted { .. } => "package_admitted",
        TaskSemanticFact::RunStarted => "run_started",
        TaskSemanticFact::EvidenceIndexed { .. } => "evidence_indexed",
        TaskSemanticFact::GeometryObserved { .. } => "geometry_observed",
        TaskSemanticFact::RecognitionStarted { .. } => "recognition_started",
        TaskSemanticFact::RecognitionCompleted { .. } => "recognition_completed",
        TaskSemanticFact::EntryRecognition { .. } => "entry_recognition",
        TaskSemanticFact::EntryRecoveryDecision { .. } => "entry_recovery_decision",
        TaskSemanticFact::EntryRecoveryPackageAdmitted { .. } => "entry_recovery_package_admitted",
        TaskSemanticFact::EntryRecoveryCompleted { .. } => "entry_recovery_completed",
        TaskSemanticFact::EntryRecoveryFailed { .. } => "entry_recovery_failed",
        TaskSemanticFact::EntryTargetDisposition { .. } => "entry_target_disposition",
        TaskSemanticFact::InterruptHandled { .. } => "interrupt_handled",
        TaskSemanticFact::InterruptAmbiguous { .. } => "interrupt_ambiguous",
        TaskSemanticFact::NavigationLocated { .. } => "navigation_located",
        TaskSemanticFact::NavigationRoutePlanned { .. } => "navigation_route_planned",
        TaskSemanticFact::NavigationWarning { .. } => "navigation_warning",
        TaskSemanticFact::StepStarted { .. } => "step_started",
        TaskSemanticFact::SelectionEvaluated { .. } => "selection_evaluated",
        TaskSemanticFact::EffectIntent { .. } => "effect_intent",
        TaskSemanticFact::EffectCompleted { .. } => "effect_completed",
        TaskSemanticFact::StepFinished { .. } => "step_finished",
        TaskSemanticFact::Finalizing { .. } => "finalizing",
        TaskSemanticFact::TerminalCommitted { .. } => "terminal_committed",
        TaskSemanticFact::TerminalRejected { .. } => "terminal_rejected",
    }
}

fn assert_event_order(actual: &[EventType], expected: &[EventType]) {
    let mut cursor = 0;
    for expected_type in expected {
        let offset = actual[cursor..]
            .iter()
            .position(|actual_type| actual_type == expected_type)
            .unwrap_or_else(|| panic!("missing Runtime event {expected_type:?} in {actual:?}"));
        cursor += offset + 1;
    }
}

fn write_runtime_owned_lab_package(path: &Path) {
    write_zip(
        path,
        &[
            (
                "control.json",
                br#"{
                    "schema_version":"Lab-1y.control.v1",
                    "package_id":"neutral.runtime-owned.recovery",
                    "execution_mode":"navigable_route",
                    "game":"neutral",
                    "server":"test",
                    "resolution":{"width":16,"height":9},
                    "entry_task_id":"task",
                    "capture_interval_ms":1,
                    "step_timeout_ms":1,
                    "max_steps":3
                }"#,
            ),
            (
                "resources/manifest.json",
                br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
            ),
            (
                "resources/operations/task/task.json",
                br#"{
                    "schema_version":"0.6",
                    "task_id":"task",
                    "game":"neutral",
                    "server_scope":["test"],
                    "coordinate_space":{"width":16,"height":9},
                    "defaults":{"max_attempts":1,"retry_interval_ms":1},
                    "entry_page":"home",
                    "target_page":"terminal",
                    "recovery":{"kind":"return_home","task_id":"return_home"},
                    "operations":[{
                        "id":"open_terminal",
                        "purpose":"force a sealed recovery suggestion",
                        "from":"home",
                        "to":"terminal",
                        "click":{"kind":"point","x":1,"y":1},
                        "retryable":true,
                        "unguarded_trusted_coordinate":true
                    }]
                }"#,
            ),
            (
                "resources/operations/return_home/task.json",
                br#"{
                    "schema_version":"0.6",
                    "task_id":"return_home",
                    "game":"neutral",
                    "server_scope":["test"],
                    "coordinate_space":{"width":16,"height":9},
                    "target_page":"home",
                    "operations":[{
                        "id":"return_home_action",
                        "purpose":"sealed successor fixture",
                        "from":"any",
                        "to":"home",
                        "click":{"kind":"point","x":1,"y":1},
                        "unguarded_trusted_coordinate":true
                    }]
                }"#,
            ),
            (
                "resources/recognition/neutral.test.pack.json",
                br#"{
                    "schema_version":"0.3",
                    "game":"neutral",
                    "server":"test",
                    "coordinate_space":{"width":16,"height":9},
                    "defaults":{"color_max_distance":0.0},
                    "targets":[
                        {"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                        {"type":"color","id":"page/terminal","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]}
                    ]
                }"#,
            ),
            (
                "resources/recognition/neutral.test.pages.json",
                br#"{
                    "schema_version":"0.3",
                    "pages":[
                        {"id":"neutral/home","required":["page/home"],"optional":[],"forbidden":[]},
                        {"id":"neutral/terminal","required":["page/terminal"],"optional":[],"forbidden":[]}
                    ]
                }"#,
            ),
        ],
    );
}

fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
    let file = File::create(path).expect("zip file");
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, contents) in files {
        zip.start_file(*name, options).expect("zip entry");
        zip.write_all(contents).expect("zip content");
    }
    zip.finish().expect("finish zip");
}

fn write_forbidden_adb(root: &Path, marker: &Path) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let path = root.join("forbidden-adb.cmd");
        fs::write(
            &path,
            format!(
                "@echo off\r\necho invoked>\"{}\"\r\nexit /b 99\r\n",
                marker.display()
            ),
        )
        .expect("write forbidden adb");
        path
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("forbidden-adb");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\necho invoked > \"{}\"\nexit 99\n",
                marker.display()
            ),
        )
        .expect("write forbidden adb");
        let mut permissions = fs::metadata(&path).expect("adb metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("adb permissions");
        path
    }
}

// Workflow #331 RSU-1 (additions-v012 row RSU-1; forced compromise T-01): a Runtime runs a
// contained task (home, one tap, terminal) and keeps its frames; actinglab
// recognition-regression then replays the frames that the run's evidence references (the
// per-step frame references of MCP-6) from the state root, offline, against a package. The
// package that ran agrees with its original record on every target; a new version whose
// terminal page expects another colour loses that target on every frame the original record
// found it on, so its error rate against the original record is 1000 per mille, while the home
// page's targets still agree.
#[test]
fn rsu1_recognition_regression_replays_the_kept_frames_against_the_original_record() {
    use std::io::Read;
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local_app_data = root.path().join("local-app-data");
    let config_path = root.path().join("actinglab.json");
    let package_path = root.path().join("runtime-owned-lab.zip");
    let result_path = root.path().join("result.zip");
    let adb_marker = root.path().join("forbidden-adb-invoked");
    fs::write(&config_path, "{}").expect("write config");
    write_runtime_owned_lab_package(&package_path);
    let forbidden_adb = write_forbidden_adb(root.path(), &adb_marker);
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state.transition_after_tap.store(true, Ordering::Release);
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-recognition-regression"),
        Arc::new(FakeProvider {
            instance_alias: "neutral.instance",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 2,
        }),
    )
    .expect("runtime host");
    let output = Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args([
            "--json",
            "--instance",
            "neutral.instance",
            "--game",
            "neutral",
            "--server",
            "test",
            "lab",
            "run",
            "--zip",
            package_path.to_str().expect("package path"),
            "--out",
            result_path.to_str().expect("result path"),
        ])
        .env("ACTINGLAB_CONFIG_PATH", &config_path)
        .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", &runtime_root)
        .env("ACTINGCOMMAND_ADB_PATH", &forbidden_adb)
        .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
        .env_remove("ACTINGLAB_SESSION_STATE_DIR")
        .env_remove("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG")
        .output()
        .expect("run actinglab lab run");
    assert!(output.status.success(), "the contained task must complete");

    // The run's evidence as MCP-6 references its frames: every verified capture frame of the
    // run's request, as `<sequence>:<event_id>:<artifact_id>`.
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_runtime_client::RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime client");
    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("Runtime events");
    let request_id = *events
        .iter()
        .find(|event| {
            event.event_type == EventType::TaskCompleted
                && matches!(
                    &event.payload,
                    ProjectionPayload::Full(payload)
                        if payload.as_ref().action() == EventAction::RuntimeTaskRun
                )
        })
        .and_then(|event| event.links.request_id())
        .expect("the contained run's request");
    let text = |value: Value| match value {
        Value::String(text) => text,
        other => other.to_string(),
    };
    let references = client
        .query_events(
            EventQuery {
                request_id: Some(request_id),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("the run's events")
        .iter()
        .filter(|event| event.event_type == EventType::ArtifactVerified)
        .flat_map(|event| {
            event
                .artifacts
                .iter()
                .filter(|artifact| {
                    artifact.kind == ArtifactKind::CaptureFrame
                        && artifact.producer
                            == actingcommand_contract::ArtifactProducer::CaptureStore
                })
                .map(|artifact| {
                    json!({"ref": format!(
                        "{}:{}:{}",
                        event.sequence,
                        text(json!(event.event_id)),
                        text(json!(artifact.artifact_id))
                    )})
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert!(!references.is_empty(), "the run kept no frame");
    let evidence = root.path().join("evidence.json");
    fs::write(
        &evidence,
        serde_json::to_vec(&json!({
            "request_id": request_id,
            "steps": [{"step_index": 0, "frames": references}]
        }))
        .expect("evidence JSON"),
    )
    .expect("write evidence");
    drop(client);
    host.close().expect("close host");

    // The new version: the terminal page expects green instead of blue.
    let version = root.path().join("new-version.zip");
    let mut archive =
        zip::ZipArchive::new(File::open(&package_path).expect("open package")).expect("package");
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("package entry");
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("entry bytes");
        if entry.name() == "resources/recognition/neutral.test.pack.json" {
            let text = String::from_utf8(bytes).expect("pack text");
            assert_eq!(text.matches(r#""expected":[0,0,255]"#).count(), 1);
            bytes = text
                .replace(r#""expected":[0,0,255]"#, r#""expected":[0,255,0]"#)
                .into_bytes();
        }
        entries.push((entry.name().to_owned(), bytes));
    }
    write_zip(
        &version,
        &entries
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect::<Vec<_>>(),
    );

    let regression = |package: &Path| {
        run_actinglab_json(
            &config_path,
            &runtime_root,
            &local_app_data,
            [
                "--json",
                "recognition-regression",
                "--state-root",
                runtime_root.to_str().expect("state root"),
                "--evidence",
                evidence.to_str().expect("evidence"),
                "--package",
                package.to_str().expect("package"),
            ],
        )["data"]
            .clone()
    };
    let target = |data: &Value, id: &str| {
        data["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .find(|target| target["target"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("no tally of {id}: {data}"))
    };

    let same = regression(&package_path);
    assert_eq!(same["mode"], "offline_recognition_regression");
    assert_eq!(same["executed"], false);
    assert_eq!(same["production_global_ledger_written"], false);
    assert_eq!(same["package_id"], "neutral.runtime-owned.recovery");
    assert_eq!(
        same["summary"]["frames_referenced"],
        json!(references.len())
    );
    for id in ["page/home", "page/terminal"] {
        let tally = target(&same, id);
        assert!(tally["compared"].as_u64() > Some(0), "{tally}");
        assert_eq!(tally["original_hits"], tally["compared"], "{tally}");
        assert_eq!(tally["disagreements"], 0, "{tally}");
        assert_eq!(tally["error_rate_milli"], 0, "{tally}");
    }
    assert_eq!(same["summary"]["disagreements"], 0);

    let changed = regression(&version);
    let home = target(&changed, "page/home");
    assert_eq!(home["disagreements"], 0, "{home}");
    let terminal = target(&changed, "page/terminal");
    assert!(terminal["compared"].as_u64() > Some(0), "{terminal}");
    assert_eq!(terminal["hits"], 0, "{terminal}");
    assert_eq!(terminal["lost"], terminal["compared"], "{terminal}");
    assert_eq!(terminal["gained"], 0, "{terminal}");
    assert_eq!(terminal["error_rate_milli"], 1000, "{terminal}");
    let replayed = changed["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .flat_map(|frame| frame["targets"].as_array().cloned().unwrap_or_default())
        .filter(|entry| entry["target"] == "page/terminal")
        .collect::<Vec<_>>();
    assert!(!replayed.is_empty());
    for entry in replayed {
        assert_eq!(entry["original_matched"], true, "{entry}");
        assert_eq!(entry["matched"], false, "{entry}");
        assert_eq!(entry["agrees"], false, "{entry}");
        assert!(
            entry["score"]["distance_milli"].as_i64() > Some(0),
            "{entry}"
        );
    }
}

// Workflow #410 item 1 (board #410, approved by Alice 2026-10-15; the 0.12 end). Every observe
// and do answer states the frame's coordinate system: device pixels W x H, the orientation, and
// that tap and tap_rect take coordinates in that same system. preview writes a JPEG preview of
// the frame and answers its path and scale. After an action, do attaches its result by default:
// the after page's projection when the page is known, the after frame's path and its preview's
// path when it is not. No image is put into an answer, only paths. The fake device serves 16 x 9
// frames, red until a tap and blue after one once the transition is switched on; the package
// knows only the red page.
#[test]
fn labobs_observe_and_do_state_the_frame_coordinates_and_do_attaches_its_result() {
    use image::ImageDecoder;
    use std::collections::BTreeSet;
    let root = TempDir::new().expect("tempdir");
    let runtime_root = root.path().join("runtime");
    let local = root.path().join("local-app-data");
    let config = root.path().join("actinglab.json");
    fs::write(&config, "{}").expect("write config");
    let package = root.path().join("frame.zip");
    write_zip(&package, &[
        ("control.json", br#"{"game":"neutral","server":"test","package_id":"neutral.frame","entry_task_id":"task"}"#),
        ("resources/manifest.json", br#"{"entry_task_id":"task"}"#),
        ("resources/operations/task/task.json", br#"{}"#),
        ("resources/recognition/neutral.test.pack.json", br#"{"schema_version":"0.3","coordinate_space":{"width":16,"height":9},"targets":[{"type":"color","id":"anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}]}"#),
        ("resources/recognition/neutral.test.pages.json", br#"{"schema_version":"0.3","pages":[{"id":"neutral/home","required":["anchor"]}]}"#),
        ("resources/navigation/neutral.test.navigation.json", br#"{"schema_version":"0.3","navigation":[]}"#),
    ]);
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&runtime_root, b"actinglab-labobs"),
        Arc::new(FakeProvider {
            instance_alias: "node.a",
            instance_id,
            state: Arc::clone(&state),
            frame_size: 1,
        }),
    )
    .expect("runtime host");
    let package = package.to_str().expect("package path");
    let frames = local.join("ActingCommand").join("actinglab").join("frames");
    let frame_files = || -> BTreeSet<std::path::PathBuf> {
        fs::read_dir(&frames)
            .map(|entries| entries.map(|entry| entry.expect("entry").path()).collect())
            .unwrap_or_default()
    };
    let assert_coordinates = |statement: &Value, what: &str| {
        assert_eq!(
            statement["device_pixels"],
            json!([16, 9]),
            "{what}: {statement}"
        );
        assert_eq!(statement["orientation"], "landscape", "{what}: {statement}");
        for input in ["tap", "tap_rect"] {
            assert!(
                statement["applies_to"]
                    .as_array()
                    .is_some_and(|inputs| inputs.iter().any(|named| named == input)),
                "{what}: {input} is in the frame's coordinates: {statement}"
            );
        }
    };
    // A preview {path, width, height, scale}: a JPEG file of that size in the Lab frame
    // directory; the frame is not larger than 1024, so it keeps its size at scale 1.
    let assert_preview = |preview: &Value, what: &str| {
        let path = std::path::PathBuf::from(
            preview["path"]
                .as_str()
                .unwrap_or_else(|| panic!("{what}: the preview's path: {preview}")),
        );
        assert_eq!(path.parent(), Some(frames.as_path()), "{what}: {preview}");
        let decoder = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(
            fs::read(&path).expect("the preview file"),
        ))
        .unwrap_or_else(|error| panic!("{what}: the preview is not a JPEG: {error}"));
        assert_eq!(decoder.dimensions(), (16, 9), "{what}: {preview}");
        assert_eq!(
            (&preview["width"], &preview["height"], &preview["scale"]),
            (&json!(16), &json!(9), &json!(1.0)),
            "{what}: {preview}"
        );
    };

    // observe --capture --preview: the frame's coordinates and its preview.
    let observed = run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "--instance",
            "node.a",
            "observe",
            "--capture",
            "--zip",
            package,
            "--preview",
        ],
    );
    assert_coordinates(&observed["data"]["frame_coordinates"], "observe");
    assert_preview(&observed["data"]["preview"], "observe");

    // do on a page that stays known: the after page's projection, and no frame file.
    let written_before = frame_files();
    let known = run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "--instance",
            "node.a",
            "do",
            "--tap",
            "1,1",
            "--capture",
            "--zip",
            package,
        ],
    );
    assert_eq!(known["data"]["executed"], true, "{known}");
    assert_coordinates(&known["data"]["frame_coordinates"], "do on a known page");
    let attachment = &known["data"]["after_attachment"];
    assert_eq!(
        attachment
            .as_object()
            .map(|members| members.keys().cloned().collect::<BTreeSet<_>>()),
        Some(BTreeSet::from(["kind", "observation"].map(str::to_string))),
        "{attachment}"
    );
    assert_eq!(attachment["kind"], "page_projection", "{attachment}");
    assert_eq!(
        attachment["observation"]["page"], "neutral/home",
        "{attachment}"
    );
    assert_eq!(
        attachment["observation"]["frame"]["width"], 16,
        "{attachment}"
    );
    assert_eq!(
        frame_files(),
        written_before,
        "a known page writes no frame file"
    );

    // do onto a page the package does not know: the after frame's path and its preview's path.
    state.transition_after_tap.store(true, Ordering::Release);
    let unknown = run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "--instance",
            "node.a",
            "do",
            "--tap",
            "1,1",
            "--capture",
            "--zip",
            package,
        ],
    );
    assert_eq!(unknown["data"]["executed"], true, "{unknown}");
    assert_coordinates(
        &unknown["data"]["frame_coordinates"],
        "do on an unknown page",
    );
    let attachment = &unknown["data"]["after_attachment"];
    assert_eq!(
        attachment
            .as_object()
            .map(|members| members.keys().cloned().collect::<BTreeSet<_>>()),
        Some(BTreeSet::from(
            ["frame_coordinates", "frame_path", "kind", "preview"].map(str::to_string)
        )),
        "{attachment}"
    );
    assert_eq!(attachment["kind"], "frame", "{attachment}");
    assert_coordinates(&attachment["frame_coordinates"], "the after frame");
    let frame_path = std::path::PathBuf::from(
        attachment["frame_path"]
            .as_str()
            .unwrap_or_else(|| panic!("the after frame's path: {attachment}")),
    );
    assert_eq!(frame_path.parent(), Some(frames.as_path()), "{attachment}");
    let after = actingcommand_recognition::Scene::from_png(
        &fs::read(&frame_path).expect("the after frame file"),
    )
    .expect("the after frame is a PNG");
    assert_eq!((after.width(), after.height()), (16, 9));
    assert_eq!(
        &after.rgb8_pixels()[..3],
        &[0, 0, 255],
        "the frame after the tap"
    );
    assert_preview(&attachment["preview"], "the after frame");

    // Offline observe of that frame states its coordinates too.
    let offline = run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "observe",
            "--scene",
            frame_path.to_str().expect("frame path"),
            "--zip",
            package,
        ],
    );
    assert_coordinates(&offline["data"]["frame_coordinates"], "offline observe");
    host.close().expect("close host");
}
