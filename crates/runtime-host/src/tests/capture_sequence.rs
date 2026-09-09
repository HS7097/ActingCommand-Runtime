// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn bounded_capture_sequence_returns_unique_verified_observations_without_input() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let capture = client.request_with_correlation(
        correlation,
        RuntimeOperation::CaptureSequence {
            instance_alias: "node.a".to_string(),
            spec: CaptureSequenceSpec::new(3, 5).expect("sequence spec"),
        },
    );

    let completed = client.send(&capture);

    assert_eq!(completed.state(), RuntimeReceiptState::Completed);
    let sequence = match completed.result() {
        Some(RuntimeResult::CaptureSequenceCompleted { sequence }) => sequence,
        other => panic!("unexpected sequence result: {other:?}"),
    };
    assert_eq!(sequence.observations().len(), 3);
    let artifact_ids = sequence
        .observations()
        .iter()
        .map(|observation| observation.artifact().artifact_id)
        .collect::<BTreeSet<_>>();
    let frame_ids = sequence
        .observations()
        .iter()
        .map(|observation| *observation.artifact().frame_id().expect("frame id"))
        .collect::<BTreeSet<_>>();
    let object_keys = sequence
        .observations()
        .iter()
        .map(|observation| {
            observation
                .artifact()
                .object_key()
                .expect("object key")
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(artifact_ids.len(), 3);
    assert_eq!(frame_ids.len(), 3);
    assert_eq!(object_keys.len(), 3);
    for observation in sequence.observations() {
        assert!(
            read_projected_verified(root.path(), observation.artifact())
                .expect("verified sequence artifact")
                .starts_with(b"\x89PNG\r\n\x1a\n")
        );
    }
    let events = event_types_for_correlation(&mut client, correlation_id);
    assert_eq!(
        events,
        [
            EventType::CliCommand,
            EventType::CommandReceived,
            EventType::CommandValidated,
            EventType::SchedulerAdmitted,
        ]
        .into_iter()
        .chain(
            [
                EventType::CaptureRequested,
                EventType::RecognitionRequested,
                EventType::ArtifactCreated,
                EventType::ArtifactVerified,
                EventType::CaptureCompleted,
                EventType::RecognitionCompleted,
            ]
            .into_iter()
            .cycle()
            .take(18),
        )
        .collect::<Vec<_>>()
    );
    assert_eq!(state.capture_open_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 3);
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
}

#[test]
fn capture_sequence_partial_failure_keeps_evidence_without_fake_success_or_input() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.fail_capture_on.store(2, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::CaptureSequence {
            instance_alias: "node.a".to_string(),
            spec: CaptureSequenceSpec::new(3, 0).expect("sequence spec"),
        },
    );

    let failed = client.send(&request);

    assert_eq!(failed.state(), RuntimeReceiptState::Failed);
    assert!(failed.result().is_none());
    assert_eq!(
        failed.error_projection().expect("capture failure").code,
        RuntimeErrorCode::CaptureFailed
    );
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::ArtifactVerified)
            .count(),
        1
    );
    assert_eq!(
        events.last().map(|event| event.event_type),
        Some(EventType::RecognitionFailed)
    );
    assert!(!events.iter().any(|event| matches!(
        event.event_type,
        EventType::InputIntent | EventType::InputCommitted | EventType::InputFailed
    )));
    assert_eq!(state.capture_count.load(Ordering::Acquire), 2);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}

#[test]
fn capture_sequence_bounds_are_rejected_before_backend_open() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let valid = client.request(RuntimeOperation::CaptureSequence {
        instance_alias: "node.a".to_string(),
        spec: CaptureSequenceSpec::new(1, 0).expect("valid spec"),
    });
    let mut encoded = serde_json::to_value(valid).expect("request JSON");
    encoded["operation"]["spec"]["frame_count"] = serde_json::json!(61);
    let invalid = serde_json::from_value::<RuntimeRequest>(encoded).expect("wire request");

    let denied = client.send(&invalid);

    assert_eq!(denied.state(), RuntimeReceiptState::Denied);
    assert!(denied.result().is_none());
    assert_eq!(state.capture_open_count.load(Ordering::Acquire), 0);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 0);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
}
