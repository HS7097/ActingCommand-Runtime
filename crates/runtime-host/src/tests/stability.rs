// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn contained_task_stability_persistence_failures_are_fatal_without_later_input_or_artifact() {
    for failure_kind in ["artifact_write", "event_append"] {
        let expected_code = if failure_kind == "event_append" {
            "contained_task_stability_event_append_injected_failure"
        } else {
            "artifact_store_failure"
        };
        let root = TempDir::new().expect("tempdir");
        let package = root
            .path()
            .join(format!("neutral-stability-{failure_kind}-failure.zip"));
        let bytes = neutral_stability_contained_task_package(2, 5);
        fs::write(&package, &bytes).expect("write stability package");
        let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
        let state = Arc::new(FakeState::default());
        let host = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                "neutral.instance",
                instance_id(),
                Arc::clone(&state),
            )),
        )
        .expect("runtime host");
        match failure_kind {
            "artifact_write" => host
                .fail_next_contained_task_stability_persistence_for_test()
                .expect("inject diagnostic persistence failure"),
            "event_append" => host
                .fail_next_contained_task_stability_event_append_for_test()
                .expect("inject diagnostic event append failure"),
            _ => unreachable!(),
        }
        let mut client = TestClient::connect(&host);
        client.set_receipt_read_timeout();
        let correlation = client.ids.mint_correlation_id().expect("correlation");
        let correlation_id = *correlation.transport();
        let request = client.request_with_correlation(
            correlation,
            RuntimeOperation::run_contained_task(
                "neutral.instance",
                client.ids.mint_holder_id().expect("holder"),
                ContainedTaskRequest::new(package.display().to_string(), expected)
                    .expect("task request"),
            ),
        );

        let failed = client.send(&request);

        assert_eq!(
            failed.state(),
            RuntimeReceiptState::Failed,
            "{failure_kind}"
        );
        assert!(failed.result().is_none(), "{failure_kind}");
        let error = failed.error_projection().expect("fatal artifact error");
        assert!(error.fatal, "{failure_kind}");
        assert_eq!(error.code, RuntimeErrorCode::RuntimeFatal, "{failure_kind}");
        assert_eq!(
            state.input_count.load(Ordering::Acquire),
            2,
            "{failure_kind}"
        );
        let events = host
            .query_persisted_events_for_test(EventQuery {
                correlation_id: Some(correlation_id),
                ..EventQuery::default()
            })
            .expect("query persistence-failure events without a second IPC request");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type() == EventType::ArtifactVerified)
                .flat_map(|event| event.artifacts().iter())
                .filter(|artifact| artifact.kind() == ArtifactKind::DiagnosticJson
                    && artifact.producer() == ArtifactProducer::CapturePipeline)
                .count(),
            0,
            "{failure_kind}"
        );
        assert_eq!(
            host.fatal_error()
                .expect("runtime fatal state")
                .expect("fatal error")
                .code(),
            expected_code,
            "{failure_kind}"
        );
        drop(client);
        assert_eq!(
            host.close()
                .expect_err("fatal host closes with failure")
                .code(),
            expected_code,
            "{failure_kind}"
        );
    }
}

#[test]
fn contained_task_stability_persists_one_formally_bound_diagnostic_per_comparison() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("neutral-stability-task.zip");
    let bytes = neutral_stability_contained_task_package(2, 6);
    fs::write(&package, &bytes).expect("write stability package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let state = Arc::new(FakeState::default());
    state
        .stability_region_transition_after_inputs
        .store(3, Ordering::Release);
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
    client
        .stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("stability receipt timeout");
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string(), expected)
                .expect("task request"),
        ),
    );

    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            outcome: TaskOutcome::Success,
            executed_steps: 5,
            ..
        })
    ));
    assert_eq!(state.input_count.load(Ordering::Acquire), 5);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 6);

    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let capture_frames = events
        .iter()
        .filter(|event| event.event_type == EventType::ArtifactVerified)
        .flat_map(|event| event.artifacts.iter())
        .filter(|artifact| artifact.kind() == ArtifactKind::CaptureFrame)
        .collect::<Vec<_>>();
    let comparisons = events
        .iter()
        .filter(|event| {
            event.event_type == EventType::ArtifactVerified
                && event.artifacts.iter().any(|artifact| {
                    artifact.kind() == ArtifactKind::DiagnosticJson
                        && artifact.producer == ArtifactProducer::CapturePipeline
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(capture_frames.len(), 6);
    assert_eq!(comparisons.len(), 4, "one artifact per comparison");

    let expected_results = [
        ("unchanged", 0, 1, None),
        ("changed", 1, 0, None),
        ("unchanged", 0, 1, None),
        (
            "unchanged",
            1,
            2,
            Some("consecutive_unchanged_threshold_reached"),
        ),
    ];
    for (index, (event, expected_result)) in comparisons.iter().zip(expected_results).enumerate() {
        let [artifact] = event.artifacts.as_slice() else {
            panic!("comparison must attach exactly one diagnostic artifact")
        };
        let document: serde_json::Value = serde_json::from_slice(
            &read_projected_verified(root.path(), artifact).expect("verified stability diagnostic"),
        )
        .expect("stability diagnostic JSON");
        let previous_frame = capture_frames[index + 1]
            .frame_id()
            .expect("previous formal frame");
        let current_frame = capture_frames[index + 2]
            .frame_id()
            .expect("current formal frame");

        assert_eq!(
            document["schema_version"],
            "actingcommand.runtime.contained-task-stability-comparison.v1"
        );
        assert_eq!(
            document["task_id"],
            serde_json::to_value(event.links.task_id()).unwrap()
        );
        assert_eq!(
            document["run_id"],
            serde_json::to_value(event.links.run_id()).unwrap()
        );
        assert_eq!(
            document["action_id"],
            serde_json::to_value(event.links.action_id()).unwrap()
        );
        assert_eq!(document["step_index"], u64::try_from(index + 1).unwrap());
        assert_eq!(document["operation_label"], "repeat");
        assert_eq!(
            document["previous_frame_id"],
            serde_json::to_value(previous_frame).unwrap()
        );
        assert_eq!(
            document["current_frame_id"],
            serde_json::to_value(current_frame).unwrap()
        );
        assert_eq!(artifact.frame_id(), Some(current_frame));
        assert_eq!(event.links.frame_id(), Some(current_frame));
        assert_eq!(artifact.run_id.as_ref(), event.links.run_id());
        assert_eq!(
            document["region"],
            serde_json::json!({
                "x": 1, "y": 0, "width": 1, "height": 1
            })
        );
        assert_eq!(document["comparison_mode"], "exact_pixels_v1");
        assert_eq!(document["comparison_parameters"], serde_json::json!({}));
        assert_eq!(document["result"], expected_result.0);
        assert_eq!(document["prior_consecutive_unchanged"], expected_result.1);
        assert_eq!(document["new_consecutive_unchanged"], expected_result.2);
        assert_eq!(document["consecutive_unchanged_threshold"], 2);
        assert_eq!(
            document["terminal_reason"],
            serde_json::to_value(expected_result.3).unwrap()
        );
    }
    for event_type in [EventType::TaskCompleted, EventType::LeaseReleased] {
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == event_type)
                .count(),
            1,
            "one authoritative {event_type:?}"
        );
    }
    drop(client);
    host.close().expect("close host");
}

#[test]
fn contained_task_stability_max_steps_uses_the_last_comparison_without_duplicate_artifact() {
    // FAILED-PROGRESS-v1: W6 preserved first red, https://github.com/HS7097/ActingCommand-Workflow/issues/278#issuecomment-5570527033
    for max_steps in [4, 6] {
        let root = TempDir::new().expect("tempdir");
        let package = root.path().join("neutral-stability-max-task.zip");
        let bytes = neutral_stability_contained_task_package(max_steps - 1, max_steps);
        fs::write(&package, &bytes).expect("write stability package");
        let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
        let state = Arc::new(FakeState::default());
        state
            .stability_region_transition_after_inputs
            .store(2, Ordering::Release);
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
        client
            .stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("stability receipt timeout");
        let correlation = client.ids.mint_correlation_id().expect("correlation");
        let correlation_id = *correlation.transport();
        let request = client.request_with_correlation(
            correlation,
            RuntimeOperation::run_contained_task(
                "neutral.instance",
                client.ids.mint_holder_id().expect("holder"),
                ContainedTaskRequest::new(package.display().to_string(), expected)
                    .expect("task request"),
            ),
        );

        let receipt = client.send(&request);
        assert_eq!(receipt.state(), RuntimeReceiptState::Failed);
        assert_eq!(
            state.input_count.load(Ordering::Acquire),
            max_steps as usize
        );
        let events = projected_events(
            &mut client,
            EventQuery {
                correlation_id: Some(correlation_id),
                ..EventQuery::default()
            },
        );
        let diagnostics = events
            .iter()
            .filter(|event| event.event_type == EventType::ArtifactVerified)
            .flat_map(|event| event.artifacts.iter())
            .filter(|artifact| {
                artifact.kind() == ArtifactKind::DiagnosticJson
                    && artifact.producer == ArtifactProducer::CapturePipeline
            })
            .collect::<Vec<_>>();
        assert_eq!(
            diagnostics.len(),
            max_steps as usize - 1,
            "terminal trace adds no artifact"
        );
        let terminal: serde_json::Value = serde_json::from_slice(
            &read_projected_verified(root.path(), diagnostics.last().unwrap())
                .expect("verified max-step diagnostic"),
        )
        .expect("max-step diagnostic JSON");
        assert_eq!(terminal["step_index"], max_steps - 1);
        assert_eq!(terminal["result"], "unchanged");
        assert_eq!(terminal["prior_consecutive_unchanged"], max_steps - 3);
        assert_eq!(terminal["new_consecutive_unchanged"], max_steps - 2);
        assert_eq!(terminal["consecutive_unchanged_threshold"], max_steps - 1);
        assert_eq!(terminal["max_steps"], max_steps);
        assert_eq!(terminal["terminal_reason"], "max_steps_reached");
        let task_failed = events
            .iter()
            .filter(|event| event.event_type == EventType::TaskFailed)
            .collect::<Vec<_>>();
        assert_eq!(task_failed.len(), 1, "one authoritative TaskFailed");
        assert!(matches!(
            projected_task_semantic_fact(task_failed[0]),
            Some(TaskSemanticFact::TerminalCommitted {
                outcome: TaskOutcome::Failure,
                executed_steps: Some(steps),
                failure_code: Some(code),
                ..
            }) if *steps == max_steps && code == "contained_task_requires_scheduler"
        ));
        let streams = events
            .iter()
            .filter(|event| event.event_type == EventType::ArtifactVerified)
            .flat_map(|event| &event.artifacts)
            .filter(|artifact| {
                artifact.kind == ArtifactKind::DiagnosticJson
                    && artifact.producer == ArtifactProducer::ArtifactStore
                    && artifact.redaction_state
                        == actingcommand_contract::ArtifactRedactionState::Pending
            })
            .collect::<Vec<_>>();
        let [stream] = streams.as_slice() else {
            panic!("one native task diagnostic stream")
        };
        let document: serde_json::Value =
            serde_json::from_slice(&read_projected_verified(root.path(), stream).unwrap()).unwrap();
        let terminal = document["records"].as_array().unwrap().last().unwrap();
        assert_eq!(terminal["kind"], "terminal");
        assert_eq!(terminal["data"]["execution"], "task_error");
        assert_eq!(terminal["data"]["executed_steps"], max_steps);
        let lease_id = *task_failed[0]
            .links
            .lease_id()
            .expect("failed task lease identity");
        let releases = host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::LeaseReleased),
                lease_id: Some(lease_id),
                ..EventQuery::default()
            })
            .expect("query synthetic cleanup release by lease identity");
        assert_eq!(releases.len(), 1, "one authoritative LeaseReleased");
        assert!(host.fatal_error().expect("runtime health").is_none());
        drop(client);
        host.close().expect("close host");
    }
}
