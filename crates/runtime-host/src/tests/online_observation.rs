// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn online_observation_native_closure_status_privacy_and_failure_boundaries() {
    use actingcommand_contract::{
        ContainedObservationEvidence, ContainedObservationRequest, PageObservationStatus,
    };
    use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
    for mode in 0..7 {
        let root = TempDir::new().unwrap();
        let state = Arc::new(FakeState::default());
        state.unknown_capture.store(mode == 1, Ordering::Release);
        state.fail_capture.store(mode == 4, Ordering::Release);
        state
            .transient_capture_failure
            .store(mode == 4, Ordering::Release);
        let vision = Arc::new(FakeVisionProvider {
            ocr_calls: AtomicU64::new(0),
            ocr_failure_detail: (mode == 3).then_some("private-provider-detail"),
            ..FakeVisionProvider::default()
        });
        let host = RuntimeHost::start(
            config(&root),
            Arc::new(
                FakeProvider::one("node.a", instance_id(), state.clone())
                    .with_vision_provider(vision.clone()),
            ),
        )
        .unwrap();
        let pages = if mode == 2 {
            br#"{"schema_version":"0.6","pages":[{"id":"page","required":["anchor","text"],"optional":["later"]},{"id":"other","required":["text"]}]}"#.as_slice()
        } else {
            br#"{"schema_version":"0.6","pages":[{"id":"page","required":["anchor","text"],"optional":["later"]}]}"#
                .as_slice()
        };
        let files: &[(&str, &[u8])] = &[
            ("control.json", br#"{"game":"neutral","server":"test","entry_task_id":"task"}"#),
            ("resources/manifest.json", br#"{"entry_task_id":"task"}"#),
            ("resources/recognition/neutral.test.pack.json", br#"{"schema_version":"0.6","coordinate_space":{"width":2,"height":1},"targets":[{"type":"color","id":"anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},{"type":"ocr","id":"text","region":"full_frame","languages":["en"],"timeout_ms":1000,"match_mode":"exact","expected":["home"],"case_sensitive":false,"minimum_confidence":0.9,"model_ref":"PP-OCRv6_medium","model_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},{"type":"color","id":"later","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}]}"#),
            ("resources/recognition/neutral.test.pages.json", pages),
            ("resources/navigation/neutral.test.navigation.json", br#"{"navigation":[]}"#),
            ("resources/operations/task/task.json", br#"{"task_id":"task","post_admission_ocr":{"mode":"fields_v1","fields":[{"id":"name","target_id":"text","privacy":"personal"}]}}"#),
        ];
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in files {
            zip.start_file(*name, FileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        let path = root.path().join("observation.zip");
        fs::write(&path, &bytes).unwrap();
        let expected = if mode == 6 {
            "0".repeat(64)
        } else {
            format!("{:x}", Sha256::digest(&bytes))
        };
        if mode == 5 {
            fs::write(root.path().join("artifacts"), b"blocks artifact directory").unwrap();
        }
        let client = RuntimeClient::connect(RuntimeClientConfig::new(
            root.path(),
            EventActor::Lab,
            EventSource::Lab,
        ))
        .unwrap();
        let session = client.begin_debug_session().unwrap();
        let request = ContainedObservationRequest::new(
            path.to_str().unwrap(),
            &expected,
            vec!["text".into()],
        )
        .unwrap();
        let result = session.observe_contained_page("node.a", request);
        if mode >= 4 {
            let error = result.unwrap_err();
            assert_eq!(error.is_fatal(), mode == 5);
            assert_eq!(
                state.capture_count.load(Ordering::Acquire),
                usize::from(mode != 6)
            );
        } else {
            let verified = match result {
                Ok(value) => value,
                Err(error) => {
                    let preserved = root.keep();
                    panic!(
                        "mode={mode} error={error:?}; native evidence retained at {}",
                        preserved.display()
                    );
                }
            };
            let observation = verified.observation();
            assert_eq!(
                observation.status,
                [
                    PageObservationStatus::Recognized,
                    PageObservationStatus::NoMatch,
                    PageObservationStatus::Conflict,
                    PageObservationStatus::Partial
                ][mode]
            );
            assert_eq!(verified.receipt().state(), RuntimeReceiptState::Observed);
            assert_eq!(state.capture_count.load(Ordering::Acquire), 1);
            assert_eq!(
                vision.ocr_calls.load(Ordering::Acquire),
                if mode == 2 { 2 } else { 1 }
            );
            assert_eq!(
                format!("sha256:{:x}", Sha256::digest(verified.png())),
                observation.frame.artifact().sha256
            );
            assert_eq!(
                observation.actual_package_sha256,
                actingcommand_contract::PackageRef::from(expected)
            );
            assert!(
                verified.receipt().terminal().unwrap().sequence > observation.projection_sequence
            );
            let raw = read_projected_verified(root.path(), &observation.artifact).unwrap();
            let evidence: ContainedObservationEvidence = serde_json::from_slice(&raw).unwrap();
            evidence.private_facts.validate().unwrap();
            assert_eq!(
                evidence.private_facts.target_evaluation_count,
                if mode == 2 {
                    4
                } else if mode == 3 {
                    1
                } else {
                    3
                }
            );
            if mode == 3 {
                assert!(observation.facts.rows.iter().any(|row| {
                    row["kind"] == "target_not_evaluated"
                        && row["target_id"] == "later"
                        && row["state"] == "not_evaluated"
                        && row["page_id"] == "page"
                        && row["target_index"] == 0
                }));
            }
            assert!(
                !serde_json::to_string(&observation.facts)
                    .unwrap()
                    .contains("private-provider-detail")
            );
            if mode == 0 {
                assert!(
                    observation
                        .projection
                        .fields
                        .iter()
                        .filter(|field| field["target_id"] == "text")
                        .all(|field| field["redacted"] == true && field["value"].is_null())
                );
            }
            let events = session.query_events(ProjectionProfile::Forensic).unwrap();
            assert!(events.iter().all(|event| event.links.lease_id().is_none()));
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event_type == EventType::ArtifactVerified
                        && event
                            .artifacts
                            .iter()
                            .any(|artifact| artifact.kind == ArtifactKind::DiagnosticJson))
                    .count(),
                1
            );
            assert!(
                events
                    .iter()
                    .any(|event| event.sequence == observation.projection_sequence
                        && event.event_id == observation.projection_event_id)
            );
        }
        assert_eq!(state.input_count.load(Ordering::Acquire), 0);
        drop(session);
        drop(client);
        if mode == 5 {
            assert!(host.close().is_err());
        } else {
            host.close().unwrap();
        }
    }
}

#[test]
fn readonly_observation_uses_one_correlation_and_typed_durable_events() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let observe = client.request_with_correlation(
        correlation,
        RuntimeOperation::ObserveReadonly {
            instance_alias: "node.a".to_string(),
        },
    );
    let completed = client.send(&observe);
    assert_eq!(completed.state(), RuntimeReceiptState::Completed);
    let observation = match completed.result() {
        Some(RuntimeResult::ReadonlyObservationCompleted { observation }) => observation,
        other => panic!("unexpected observation result: {other:?}"),
    };
    assert_eq!((observation.width(), observation.height()), (2, 1));
    assert_eq!(
        observation.capture_backend(),
        RuntimeCaptureBackend::AdbScreencap
    );
    let artifact = read_projected_verified(root.path(), observation.artifact())
        .expect("verified observation artifact");
    assert!(artifact.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(
        event_types_for_correlation(&mut client, correlation_id),
        vec![
            EventType::CliCommand,
            EventType::CommandReceived,
            EventType::CommandValidated,
            EventType::SchedulerAdmitted,
            EventType::CaptureRequested,
            EventType::RecognitionRequested,
            EventType::ArtifactCreated,
            EventType::ArtifactVerified,
            EventType::CaptureCompleted,
            EventType::RecognitionCompleted,
        ]
    );
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert_eq!(state.capture_open_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
}
