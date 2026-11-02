// SPDX-License-Identifier: AGPL-3.0-only

// #407 PF-2 (register §1.252; contracts/task-diagnostic-stream.md): `RecognitionStarted`'s two
// commits are observed stage by stage, each in the family of `RecognitionCompleted`'s commit of
// its kind. The records and their order stay; the `*_append` spans stay `RecognitionCompleted`'s
// alone. No time is asserted: every span here reads the process clock, and nothing sleeps.

use super::*;

#[test]
fn recognition_started_stages_are_observed() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("neutral-task.zip");
    fs::write(&package, neutral_contained_task_package(true)).expect("write package");
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral.instance",
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string()).expect("task request"),
        ),
    );
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );

    // The run's two recognitions, each still one `recognition.requested` before its one
    // `task.recognition_started`.
    let started_events = events
        .iter()
        .filter(|event| event.event_type == EventType::TaskRecognitionStarted)
        .collect::<Vec<_>>();
    assert_eq!(started_events.len(), 2);
    for started in started_events {
        let recognition = started.links.recognition_id();
        assert!(recognition.is_some());
        let requested = events
            .iter()
            .filter(|event| {
                event.event_type == EventType::RecognitionRequested
                    && event.links.recognition_id() == recognition
            })
            .collect::<Vec<_>>();
        let [requested] = requested.as_slice() else {
            panic!("one recognition.requested per recognition: {requested:?}");
        };
        assert!(requested.sequence < started.sequence);
    }

    let terminal = events
        .iter()
        .find(|event| event.event_type == EventType::TaskCompleted)
        .expect("the run's terminal");
    let ProjectionPayload::Full(payload) = &terminal.payload else {
        panic!("full terminal payload");
    };
    let timing = payload
        .task_timing()
        .expect("the run's timing observations");
    let mut started_total = 0;
    let mut completed_total = 0;
    for boundaries in [&timing.preflight, &timing.execution, &timing.finalization]
        .into_iter()
        .filter_map(|phase| phase.boundaries.as_deref())
    {
        let started = boundaries
            .recognition_started_record
            .attempts
            .expect("started records counted");
        let completed = boundaries
            .recognition_payload_append
            .attempts
            .expect("completed commits counted");
        // The whole-call spans stay `RecognitionCompleted`'s alone.
        assert_eq!(boundaries.recognition_task_append.attempts, Some(completed));
        // Each family holds one commit of each record.
        for stages in [
            &boundaries.recognition_payload_stages,
            &boundaries.recognition_task_stages,
        ] {
            for span in [
                &stages.fact_gate,
                &stages.draft,
                &stages.writer_response,
                &stages.device_diagnostics,
                &stages.fact_sync,
                &stages.pipeline,
            ] {
                assert_eq!(span.attempts, Some(started + completed));
                if started + completed > 0 {
                    assert_eq!(span.errors, Some(0));
                    assert_eq!(
                        span.status,
                        actingcommand_contract::TaskTimingObservationState::Observed
                    );
                    assert!(span.total_us.is_some());
                }
            }
        }
        started_total += started;
        completed_total += completed;
    }
    assert_eq!(started_total, 2, "both started records are observed");
    assert_eq!(completed_total, 2, "both completed records are observed");

    drop(client);
    host.close().expect("close host");
}
