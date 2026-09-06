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
    ExecutionBackendProvider, ResolvedExecutionInstance, RuntimeHost, RuntimeHostConfig,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};
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
}

#[test]
fn lab_operation_evidence_consistency_preserves_complete_and_incomplete_records() {
    use actingcommand_contract::{
        ContainedLabOperationRequest, InputAction, LabOperationEvidence, LabOperationSelection,
        LabProjectionHint, verify_lab_operation_evidence,
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
    let hash = format!("{:x}", Sha256::digest(fs::read(&package).unwrap()));
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
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
        actingcommand_contract::EventActor::Lab,
        EventSource::Lab,
    ))
    .unwrap();
    for selection in [
        LabOperationSelection::Coordinates {
            action: InputAction::Tap { x: 0, y: 0 },
        },
        LabOperationSelection::Element {
            id: "undeclared".into(),
        },
    ] {
        let complete = matches!(selection, LabOperationSelection::Coordinates { .. });
        let session = client.begin_debug_session().unwrap();
        let verified = session
            .run_contained_lab_operation(
                "node.a",
                ContainedLabOperationRequest {
                    package_path: package.to_str().unwrap().to_string(),
                    expected_sha256: hash.clone(),
                    selection,
                    projection_hint: LabProjectionHint {
                        sequence: None,
                        content_sha256: None,
                    },
                },
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
        assert_eq!(evidence.operation.record.failure.is_none(), complete);
        assert_eq!(evidence.operation.record.after_frame.is_some(), complete);
        assert_eq!(evidence.operation.record.input_event.is_some(), complete);
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
            verify_lab_operation_evidence(&corrupt).unwrap_err().code(),
            "runtime_lab_artifact_hash_mismatch"
        );
        if complete {
            let mut missing_intent = evidence.clone();
            missing_intent.action_events.clear();
            assert!(verify_lab_operation_evidence(&missing_intent).is_err());
        }
        client.status().unwrap();
    }
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
    drop(client);
    host.close().unwrap();
}

#[test]
fn resource_restore_uses_native_evidence_and_existing_package_chain() {
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
    let source_task = json!({"schema_version":"0.6","task_id":"seed","game":"neutral","server_scope":["test"],"locale":"en-US",
        "coordinate_space":{"width":2,"height":2},"defaults":{"template_threshold":0.9,"color_max_distance":0.0},
        "anchors":[{"id":"home","template":"assets/HOME.png","region":{"mode":"full_frame"},"threshold":0.9}],
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
    run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "resource",
            "convert",
            "--repo",
            source.to_str().unwrap(),
        ],
    );
    let original = root.path().join("original.zip");
    run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "package",
            "build-task",
            "--repo",
            source.to_str().unwrap(),
            "--task",
            "seed",
            "--out",
            original.to_str().unwrap(),
        ],
    );
    let source_bytes = open_published_package(&original)
        .unwrap()
        .read_all()
        .unwrap();
    let hash = format!("{:x}", Sha256::digest(&source_bytes));
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
                "--expected-sha256",
                &hash,
                "--verbose",
            ],
        );
        assert_eq!(output["data"]["executed"], true);
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
        EventActor::Lab,
        EventSource::Lab,
    ))
    .unwrap();
    let session = client.begin_debug_session().unwrap();
    let published_source = open_published_package(&original).unwrap();
    let incomplete = session
        .run_contained_lab_operation(
            "node.a",
            ContainedLabOperationRequest {
                package_path: published_source.path().to_str().unwrap().into(),
                expected_sha256: hash.clone(),
                selection: LabOperationSelection::Element {
                    id: "undeclared".into(),
                },
                projection_hint: LabProjectionHint {
                    sequence: None,
                    content_sha256: None,
                },
            },
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
    drop(session);
    drop(client);
    host.close().unwrap();
    let snapshot = actingcommand_ledger::GlobalLedger::open_read_only(
        actingcommand_ledger::GlobalLedgerReadOnlyConfig::new(runtime_root.join("ledger")),
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
            "--through-sequence",
            &through,
            "--zip",
            original.to_str().unwrap(),
            "--expected-sha256",
            &hash,
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
    assert_eq!(output["data"]["record_count"], 3);
    assert_eq!(output["data"]["operation_count"], 2);
    assert_eq!(output["data"]["awaiting_author_input"], json!([]));
    assert_eq!(output["data"]["gaps"].as_array().unwrap().len(), 1);
    let task_bytes = fs::read(restored.join("ours/operations/restored/task.json")).unwrap();
    let task: Value = serde_json::from_slice(&task_bytes).unwrap();
    assert_eq!(task["schema_version"], "0.6");
    assert_eq!(task["target_page"], "home");
    assert_eq!(task["operations"][0]["click"]["kind"], "point");
    assert_eq!(task["operations"][1]["click"]["kind"], "drag");
    assert_eq!(task["operations"][1]["click"]["duration_ms"], 1);
    assert_eq!(task["operations"][0]["purpose"], "");
    assert_eq!(task["operations"][0]["to"], Value::Null);
    assert_eq!(
        task["operations"][0]["expect_after"]["page_id"],
        "neutral/home"
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
    run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "resource",
            "convert",
            "--repo",
            restored.to_str().unwrap(),
        ],
    );
    let built = root.path().join("restored.zip");
    run_actinglab_json(
        &config,
        &runtime_root,
        &local,
        [
            "--json",
            "package",
            "build-task",
            "--repo",
            restored.to_str().unwrap(),
            "--task",
            "restored",
            "--out",
            built.to_str().unwrap(),
        ],
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
            built.to_str().unwrap(),
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
            "--expected-sha256",
            &hash,
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
            "--expected-sha256",
            &hash,
            "--task-id",
            "restored",
        ],
    );
    assert_eq!(
        fs::read(restored.join("ours/operations/restored/task.json")).unwrap(),
        task_bytes
    );
    let after = actingcommand_ledger::GlobalLedger::open_read_only(
        actingcommand_ledger::GlobalLedgerReadOnlyConfig::new(runtime_root.join("ledger")),
        |reference| {
            Some(
                actingcommand_artifact_store::verify_projected_read_only(&runtime_root, reference)
                    .unwrap(),
            )
        },
    )
    .unwrap();
    assert_eq!(serde_json::to_vec(after.events()).unwrap(), native_before);
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
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
        ("control.json", br#"{"game":"neutral","server":"test","entry_task_id":"task"}"#),
        ("resources/manifest.json", br#"{"entry_task_id":"task"}"#),
        ("resources/operations/task/task.json", br#"{}"#),
        ("resources/recognition/neutral.test.pack.json", br#"{"schema_version":"0.3","coordinate_space":{"width":1,"height":1},"targets":[{"type":"color","id":"anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}]}"#),
        ("resources/recognition/neutral.test.pages.json", br#"{"schema_version":"0.3","pages":[{"id":"page","required":["anchor"]}]}"#),
        ("resources/navigation/neutral.test.navigation.json", br#"{"schema_version":"0.3","navigation":[]}"#),
    ]);
    let expected = format!("{:x}", Sha256::digest(fs::read(&package).unwrap()));
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
            "--expected-sha256",
            &expected,
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
    assert_eq!(
        output["data"]["projection_source"]["actual_package_sha256"],
        expected
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
            "--expected-sha256",
            &expected,
        ],
    );
    assert!(
        serde_json::to_vec(&minimum["data"]).unwrap().len()
            <= actingcommand_ledger::MIN_PROJECTION_HARD_LIMIT_BYTES
    );
    assert_eq!(minimum["data"]["observation"]["page"], "page");
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
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
            "--expected-sha256",
            &expected,
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

struct FakeBackend {
    state: Arc<FakeState>,
    closed: bool,
}

impl InputBackend for FakeBackend {
    fn tap(&mut self, _x: i32, _y: i32) -> DeviceResult<()> {
        self.state.tap_started.store(true, Ordering::Release);
        let delay_ms = self.state.tap_delay_ms.load(Ordering::Acquire);
        if delay_ms > 0 {
            thread::sleep(Duration::from_millis(delay_ms as u64));
        }
        self.state.taps.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn long_tap(&mut self, _x: i32, _y: i32, _duration_ms: u64) -> DeviceResult<()> {
        Ok(())
    }

    fn swipe(
        &mut self,
        _x1: i32,
        _y1: i32,
        _x2: i32,
        _y2: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        Ok(())
    }

    fn key(&mut self, _key: &str) -> DeviceResult<()> {
        Ok(())
    }

    fn text(&mut self, _text: &str) -> DeviceResult<()> {
        Ok(())
    }

    fn reset(&mut self) -> DeviceResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DeviceResult<()> {
        if !self.closed {
            self.closed = true;
            self.state.closes.fetch_add(1, Ordering::AcqRel);
        }
        Ok(())
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
        self.state.captures.fetch_add(1, Ordering::AcqRel);
        let color = if self.state.transition_after_tap.load(Ordering::Acquire)
            && self.state.taps.load(Ordering::Acquire) > 0
        {
            [0, 0, 255]
        } else {
            [255, 0, 0]
        };
        let pixels = (0..self.frame_size * self.frame_size)
            .flat_map(|_| color)
            .collect();
        Frame::from_pixels(
            self.frame_size,
            self.frame_size,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
    }
}

impl ExecutionBackendProvider for FakeProvider {
    fn instance_aliases(&self) -> Vec<String> {
        vec![self.instance_alias.to_string()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == self.instance_alias)
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "<sealed-test>"))
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        assert_eq!(instance_alias, self.instance_alias);
        Ok(Box::new(FakeBackend {
            state: Arc::clone(&self.state),
            closed: false,
        }))
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        assert_eq!(instance_alias, self.instance_alias);
        Ok(Box::new(FakeCapture {
            state: Arc::clone(&self.state),
            frame_size: self.frame_size,
        }))
    }

    fn control_application(
        &self,
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
    assert!(
        busy["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("LeaseBusy"))
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
    let expected_sha256 = format!("{:x}", Sha256::digest(fs::read(&package).expect("package")));
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
            "--expected-sha256",
            &expected_sha256,
        ],
    );
    assert_eq!(output["data"]["authority"], "runtime");
    assert_eq!(output["data"]["summary"]["task_id"], "task");
    assert_eq!(
        output["data"]["summary"]["verified_sha256"],
        expected_sha256
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
    assert_eq!(state.captures.load(Ordering::Acquire), 0);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);

    let (_, failure) = run_actinglab_failure_json(
        &config_path,
        &runtime_root,
        &local_app_data,
        [
            "--json",
            "lab",
            "debug-package",
            "--zip",
            package.to_str().expect("package path"),
            "--expected-sha256",
            &"0".repeat(64),
        ],
    );
    assert_eq!(failure["ok"], false);
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
    let expected_sha256 = format!("{:x}", Sha256::digest(fs::read(&package).expect("package")));
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
            "--expected-sha256",
            &expected_sha256,
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
    assert!(message.contains("EvidenceExportFailed"));
    assert!(!evidence.exists());
    assert_eq!(state.captures.load(Ordering::Acquire), 0);
    assert_eq!(state.taps.load(Ordering::Acquire), 0);

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
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
    let expected_sha256 = format!("{:x}", Sha256::digest(fs::read(&package).expect("package")));
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
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Runtime client");
    let session = client.begin_debug_session().expect("debug session");
    session
        .debug_package(
            PackageDebugRequest::new(package.to_string_lossy().into_owned(), expected_sha256)
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
    assert_eq!(error.code(), "runtime_request_rejected");
    assert_eq!(error.operation(), "export_evidence");
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
    let expected_sha256 = format!(
        "{:x}",
        Sha256::digest(fs::read(&semantic_package).expect("semantic package"))
    );
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
            "--expected-sha256",
            &expected_sha256,
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
    let expected_sha256 = format!(
        "{:x}",
        Sha256::digest(fs::read(&semantic_package).expect("semantic package"))
    );
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
            "--expected-sha256",
            &expected_sha256,
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
            "--expected-sha256",
            &expected_sha256,
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
                "--expected-sha256",
                &expected_sha256,
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
    write_semantic_package(&semantic_package, &resources);
    let expected_sha256 = format!(
        "{:x}",
        Sha256::digest(fs::read(&semantic_package).expect("semantic package"))
    );
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
            "--expected-sha256",
            &expected_sha256,
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
            "--expected-sha256",
            &expected_sha256,
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
                    "--expected-sha256",
                    &expected_sha256,
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
            "--expected-sha256",
            &expected_sha256,
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
    let expected_sha256 = format!(
        "{:x}",
        Sha256::digest(fs::read(&semantic_package).expect("semantic package"))
    );
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
            "--expected-sha256",
            &expected_sha256,
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
            "--expected-sha256",
            &expected_sha256,
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
            "--expected-sha256",
            &expected_sha256,
        ],
    );
    assert_eq!(wait_stable["data"]["state"], "stable");
    assert_eq!(state.taps.load(Ordering::Acquire), 1);
    assert!(state.captures.load(Ordering::Acquire) >= 6);
    assert!(!local_app_data.join("ActingCommand/actinglab/lab2").exists());

    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &runtime_root,
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
    let expected_sha256 = format!(
        "{:x}",
        Sha256::digest(fs::read(&package_path).expect("read package"))
    );
    let forbidden_adb = write_forbidden_adb(root.path(), &adb_marker);
    let state = Arc::new(FakeState::default());
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
            "--expected-sha256",
            &expected_sha256,
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
    let expected_sha256 = format!(
        "{:x}",
        Sha256::digest(fs::read(&package_path).expect("read package"))
    );
    let forbidden_adb = write_forbidden_adb(root.path(), &adb_marker);
    let state = Arc::new(FakeState::default());
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
            "--expected-sha256",
            &expected_sha256,
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
            outcome: TaskOutcome::Success,
            final_page: Some(page),
            executed_steps: 1,
            failure_code: None,
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
        TaskSemanticFact::RecognitionStarted { .. } => "recognition_started",
        TaskSemanticFact::RecognitionCompleted { .. } => "recognition_completed",
        TaskSemanticFact::EntryRecognition { .. } => "entry_recognition",
        TaskSemanticFact::EntryRecoveryDecision { .. } => "entry_recovery_decision",
        TaskSemanticFact::EntryRecoveryPackageAdmitted { .. } => "entry_recovery_package_admitted",
        TaskSemanticFact::EntryRecoveryCompleted { .. } => "entry_recovery_completed",
        TaskSemanticFact::EntryRecoveryFailed { .. } => "entry_recovery_failed",
        TaskSemanticFact::EntryTargetDisposition { .. } => "entry_target_disposition",
        TaskSemanticFact::StepStarted { .. } => "step_started",
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
                    "resolution":{"width":2,"height":2},
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
                    "coordinate_space":{"width":2,"height":2},
                    "defaults":{"timeout_ms":1,"max_attempts":1,"retry_interval_ms":1,"post_wait_freezes_ms":0},
                    "entry_page":"home",
                    "target_page":"terminal",
                    "recovery":{"kind":"return_home","task_id":"return_home"},
                    "max_task_retries":1,
                    "on_exhausted":"pause",
                    "operations":[{
                        "id":"open_terminal",
                        "purpose":"force a sealed recovery suggestion",
                        "from":"home",
                        "to":"terminal",
                        "click":{"kind":"point","x":1,"y":1},
                        "retryable":true,
                        "effect":"navigation_only",
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
                    "coordinate_space":{"width":2,"height":2},
                    "target_page":"home",
                    "operations":[{
                        "id":"return_home_action",
                        "purpose":"sealed successor fixture",
                        "from":"any",
                        "to":"home",
                        "click":{"kind":"point","x":1,"y":1},
                        "effect":"navigation_only",
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
                    "coordinate_space":{"width":2,"height":2},
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
