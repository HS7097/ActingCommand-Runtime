// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn post_admission_ocr_failure_persists_one_private_formally_bound_diagnostic() {
    const PROVIDER_DETAIL: &str = "synthetic post-admission OCR provider failure";
    let expected_detail = format!(
        "fatal recognition pack error: ocr observation provider failed for target \
         'fixture/ocr' with Internal: {PROVIDER_DETAIL}"
    );
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("post-admission-ocr-failure.zip");
    let bytes = neutral_post_admission_ocr_contained_task_package();
    fs::write(&package, &bytes).expect("write OCR package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let state = Arc::new(FakeState::default());
    let vision_provider = Arc::new(FakeVisionProvider {
        ocr_failure_detail: Some(PROVIDER_DETAIL),
        ..FakeVisionProvider::default()
    });
    let stable_instance_id = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one("neutral.instance", stable_instance_id, Arc::clone(&state))
                .with_vision_provider(vision_provider.clone()),
        ),
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
            ContainedTaskRequest::new(package.display().to_string(), expected)
                .expect("OCR task request"),
        ),
    );
    let request_id = request.request_id();

    let failed = client.send(&request);
    assert_eq!(
        failed.state(),
        RuntimeReceiptState::Failed,
        "unexpected pre-execution rejection: {:?}",
        failed.error_projection()
    );
    assert_eq!(
        failed.error_projection().expect("typed task failure").code,
        RuntimeErrorCode::BackendOperationFailed
    );
    assert!(failed.result().is_none());
    assert!(
        !String::from_utf8(serde_json::to_vec(&failed).expect("receipt JSON"))
            .expect("UTF-8 receipt")
            .contains(PROVIDER_DETAIL),
        "private detail must not enter the public receipt"
    );
    let replay = client.send(&request);
    assert_eq!(replay.state(), RuntimeReceiptState::Failed);
    assert_eq!(vision_provider.ocr_calls.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 1);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);

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
        .flat_map(|event| {
            event
                .artifacts
                .iter()
                .filter(|artifact| {
                    artifact.kind() == ArtifactKind::DiagnosticJson
                        && artifact.producer == ArtifactProducer::CapturePipeline
                })
                .map(move |artifact| (event, artifact))
        })
        .collect::<Vec<_>>();
    let [(diagnostic_event, diagnostic_artifact)] = diagnostics.as_slice() else {
        panic!("one exact OCR failure diagnostic required: {diagnostics:?}")
    };
    let diagnostic_bytes = read_projected_verified(root.path(), diagnostic_artifact)
        .expect("verified OCR failure diagnostic");
    let document: serde_json::Value =
        serde_json::from_slice(&diagnostic_bytes).expect("OCR failure diagnostic JSON");
    assert_eq!(
        document["schema_version"],
        "actingcommand.runtime.post-admission-ocr-failure.v1"
    );
    assert_eq!(
        document["failure_code"],
        "contained_task_post_admission_ocr_failed"
    );
    assert_eq!(document["detail"], expected_detail);
    assert_eq!(
        document["detail_utf8_bytes"],
        u64::try_from(expected_detail.len()).expect("detail byte count")
    );
    assert_eq!(
        document["detail_sha256"],
        format!("{:x}", Sha256::digest(expected_detail.as_bytes()))
    );
    for (field, value) in [
        (
            "request_id",
            serde_json::to_value(request_id).expect("request identity JSON"),
        ),
        (
            "correlation_id",
            serde_json::to_value(correlation_id).expect("correlation identity JSON"),
        ),
        (
            "instance_id",
            serde_json::to_value(stable_instance_id).expect("instance identity JSON"),
        ),
        (
            "lease_id",
            serde_json::to_value(
                diagnostic_event
                    .links
                    .lease_id()
                    .expect("diagnostic lease identity"),
            )
            .expect("lease identity JSON"),
        ),
        (
            "task_id",
            serde_json::to_value(
                diagnostic_event
                    .links
                    .task_id()
                    .expect("diagnostic task identity"),
            )
            .expect("task identity JSON"),
        ),
        (
            "run_id",
            serde_json::to_value(
                diagnostic_event
                    .links
                    .run_id()
                    .expect("diagnostic run identity"),
            )
            .expect("run identity JSON"),
        ),
        (
            "frame_id",
            serde_json::to_value(
                diagnostic_event
                    .links
                    .frame_id()
                    .expect("diagnostic frame identity"),
            )
            .expect("frame identity JSON"),
        ),
    ] {
        assert_eq!(document[field], value, "{field}");
    }
    assert_eq!(
        diagnostic_artifact.frame_id(),
        diagnostic_event.links.frame_id()
    );
    assert_eq!(
        diagnostic_artifact.run_id.as_ref(),
        diagnostic_event.links.run_id()
    );
    assert_eq!(
        diagnostic_artifact.correlation_id.as_ref(),
        diagnostic_event.links.correlation_id()
    );

    let terminal = events
        .iter()
        .find(|event| event.event_type == EventType::TaskFailed)
        .expect("one task terminal");
    assert!(diagnostic_event.sequence < terminal.sequence);
    assert!(matches!(
        projected_task_semantic_fact(terminal),
        Some(TaskSemanticFact::TerminalCommitted {
            outcome: TaskOutcome::Failure,
            executed_steps: Some(0),
            failure_code: Some(code),
            ..
        }) if code == "contained_task_post_admission_ocr_failed"
    ));
    for event_type in [EventType::TaskFailed, EventType::CaptureSummaryCommitted] {
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == event_type)
                .count(),
            1,
            "one authoritative {event_type:?}"
        );
    }
    let lease_id = *diagnostic_event
        .links
        .lease_id()
        .expect("diagnostic lease identity");
    let releases = host
        .query_persisted_events_for_test(EventQuery {
            lease_id: Some(lease_id),
            event_type: Some(EventType::LeaseReleased),
            ..EventQuery::default()
        })
        .expect("query lease release by formal identity");
    assert_eq!(releases.len(), 1, "one authoritative LeaseReleased");
    let summary_event = events
        .iter()
        .find(|event| event.event_type == EventType::CaptureSummaryCommitted)
        .expect("capture summary");
    let ProjectionPayload::Full(payload) = &summary_event.payload else {
        panic!("forensic capture summary")
    };
    let EventPayload::Capture(CapturePayload::SummaryCommitted(summary)) = payload.as_ref() else {
        panic!("typed capture summary")
    };
    assert_eq!(summary.summary().captured(), 1);
    assert_eq!(summary.summary().persisted(), 1);
    assert_eq!(summary.summary().deduplicated(), 0);
    assert_eq!(summary.summary().dropped(), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}

#[test]
fn post_admission_ocr_failure_diagnostic_persistence_failure_is_fatal_and_preserves_terminal() {
    const PROVIDER_DETAIL: &str = "synthetic persistence failure trigger";
    let root = TempDir::new().expect("tempdir");
    let package = root
        .path()
        .join("post-admission-ocr-persistence-failure.zip");
    let bytes = neutral_post_admission_ocr_contained_task_package();
    fs::write(&package, &bytes).expect("write OCR package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let state = Arc::new(FakeState::default());
    let vision_provider = Arc::new(FakeVisionProvider {
        ocr_failure_detail: Some(PROVIDER_DETAIL),
        ..FakeVisionProvider::default()
    });
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one("neutral.instance", instance_id(), Arc::clone(&state))
                .with_vision_provider(vision_provider.clone()),
        ),
    )
    .expect("runtime host");
    host.fail_next_contained_task_ocr_failure_persistence_for_test()
        .expect("inject OCR diagnostic persistence failure");
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string(), expected)
                .expect("OCR task request"),
        ),
    );

    let failed = client.send(&request);
    assert_eq!(failed.state(), RuntimeReceiptState::Failed);
    let projection = failed
        .error_projection()
        .expect("fatal persistence failure");
    assert!(projection.fatal);
    assert_eq!(projection.code, RuntimeErrorCode::RuntimeFatal);
    assert!(failed.result().is_none());
    assert_eq!(vision_provider.ocr_calls.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 1);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);

    let events = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        })
        .expect("query persistence-failure events");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type() == EventType::ArtifactVerified)
            .flat_map(|event| event.artifacts().iter())
            .filter(|artifact| artifact.kind() == ArtifactKind::DiagnosticJson
                && artifact.producer() == ArtifactProducer::CapturePipeline)
            .count(),
        0
    );
    let task_failed = events
        .iter()
        .filter(|event| event.event_type() == EventType::TaskFailed)
        .collect::<Vec<_>>();
    let [task_failed] = task_failed.as_slice() else {
        panic!("primary task terminal must remain unique: {task_failed:?}")
    };
    assert!(matches!(
        task_failed.payload(),
        EventPayload::Task(TaskPayload::Semantic(payload))
            if matches!(
                payload.fact(),
                TaskSemanticFact::TerminalCommitted {
                    outcome: TaskOutcome::Failure,
                    executed_steps: Some(0),
                    failure_code: Some(code),
                    ..
                } if code == "contained_task_post_admission_ocr_failed"
            )
    ));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type() == EventType::CaptureSummaryCommitted)
            .count(),
        1
    );
    let lease_id = *task_failed
        .links()
        .lease_id()
        .expect("primary task lease identity");
    let releases = host
        .query_persisted_events_for_test(EventQuery {
            lease_id: Some(lease_id),
            event_type: Some(EventType::LeaseReleased),
            ..EventQuery::default()
        })
        .expect("query persistence-failure lease release");
    assert_eq!(releases.len(), 1);
    assert_eq!(
        host.fatal_error()
            .expect("runtime fatal state")
            .expect("fatal artifact error")
            .code(),
        "artifact_store_failure"
    );
    drop(client);
    assert_eq!(
        host.close()
            .expect_err("fatal host closes with persistence failure")
            .code(),
        "artifact_store_failure"
    );
}

#[test]
fn post_admission_ocr_failure_diagnostic_is_absent_for_success_and_other_task_error() {
    for case in [
        "success",
        "other-task-error",
        "task-timeout",
        "post-delay-budget",
    ] {
        let root = TempDir::new().expect("tempdir");
        let package = root.path().join(format!("{case}.zip"));
        let bytes = match case {
            "task-timeout" => neutral_contained_task_package_with_execution_timeout(50),
            "post-delay-budget" => neutral_contained_task_package_with_task_and_timeout(
                br#"{"schema_version":"0.6","task_id":"task","game":"neutral",
                    "server_scope":["test"],"coordinate_space":{"width":2,"height":1},
                    "entry_page":"home","target_page":"terminal","operations":[{
                        "id":"open_terminal","from":"home",
                        "click":{"kind":"point","x":1,"y":0},
                        "unguarded_trusted_coordinate":true,"retryable":false,
                        "post_delay_ms":5000}]}"#,
                5_000,
            ),
            _ => neutral_contained_task_package(),
        };
        fs::write(&package, &bytes).expect("write task package");
        let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
        let state = Arc::new(FakeState::default());
        match case {
            "success" => state
                .transition_capture_after_input
                .store(true, Ordering::Release),
            "other-task-error" => state.unknown_capture.store(true, Ordering::Release),
            "task-timeout" => state.capture_delay_ms.store(100, Ordering::Release),
            "post-delay-budget" => {}
            _ => unreachable!(),
        }
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
                ContainedTaskRequest::new(package.display().to_string(), expected)
                    .expect("task request"),
            ),
        );

        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            if case == "success" {
                RuntimeReceiptState::Completed
            } else {
                RuntimeReceiptState::Failed
            },
            "{case}"
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
                .flat_map(|event| event.artifacts.iter())
                .filter(|artifact| artifact.kind() == ArtifactKind::DiagnosticJson
                    && artifact.producer == ArtifactProducer::CapturePipeline)
                .count(),
            0,
            "{case}"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.event_type,
                    EventType::TaskCompleted | EventType::TaskFailed
                ))
                .count(),
            1,
            "{case}"
        );
        let stream = events
            .iter()
            .filter(|event| event.event_type == EventType::ArtifactVerified)
            .flat_map(|event| &event.artifacts)
            .find(|artifact| {
                artifact.kind == ArtifactKind::DiagnosticJson
                    && artifact.redaction_state
                        == actingcommand_contract::ArtifactRedactionState::Pending
            })
            .expect("verified BRAW stream");
        let document: serde_json::Value = serde_json::from_slice(
            &read_projected_verified(root.path(), stream).expect("verified stream bytes"),
        )
        .unwrap();
        let terminal: actingcommand_contract::TaskDiagnosticRecord = serde_json::from_value(
            document["records"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()
                .clone(),
        )
        .expect("typed terminal readback");
        let terminal_event = events
            .iter()
            .find(|event| {
                matches!(
                    event.event_type,
                    EventType::TaskCompleted | EventType::TaskFailed
                )
            })
            .expect("original run terminal");
        let ProjectionPayload::Full(payload) = &terminal_event.payload else {
            panic!("original Forensic terminal payload")
        };
        let observed = payload.task_timing().expect("same-run timing observations");
        assert_eq!(
            Some(&observed.request_id),
            terminal_event.links.request_id()
        );
        assert_eq!(Some(&observed.task_id), terminal_event.links.task_id());
        assert_eq!(Some(&observed.run_id), terminal_event.links.run_id());
        let final_write = observed
            .finalization
            .diagnostic_record_write
            .last
            .as_ref()
            .unwrap();
        assert_eq!(final_write.record_index, Some(terminal.index));
        assert_eq!(
            final_write.result,
            actingcommand_contract::TaskTimingResult::Ok
        );
        assert!(matches!(
            final_write.elapsed_us,
            actingcommand_contract::ObservedMicroseconds::Measured { .. }
        ));
        assert_eq!(
            observed.preflight.diagnostic_record_write.attempts.unwrap()
                + observed.execution.diagnostic_record_write.attempts.unwrap()
                + observed
                    .finalization
                    .diagnostic_record_write
                    .attempts
                    .unwrap(),
            document["records"].as_array().unwrap().len() as u64,
            "one measured call per original record, including its terminal"
        );
        assert!(observed.execution.recognition_evaluate.attempts.unwrap() > 0);
        if case != "success" {
            let actingcommand_contract::TaskDiagnosticPayload::Terminal(
                actingcommand_contract::TaskDiagnosticTerminalData::TaskError {
                    code,
                    executed_steps,
                    timing,
                    ..
                },
            ) = &terminal.payload
            else {
                panic!("original task error")
            };
            let timing = timing.as_ref().expect("actual timing decision");
            assert_eq!(&observed.task_failure.as_ref().unwrap().timing, timing);
            let dispatched = u32::from(case == "post-delay-budget");
            assert_eq!(*executed_steps, Some(dispatched));
            assert_eq!(
                state.input_count.load(Ordering::Acquire),
                dispatched as usize
            );
            assert_eq!(terminal.step_action_id.is_some(), dispatched == 1);
            if case == "post-delay-budget" {
                assert_eq!(code, "contained_task_timeout");
                assert_eq!(
                    timing.stage,
                    actingcommand_contract::TaskTimingStage::PostInputDelay
                );
                assert_eq!(timing.limit_ms, 5_000);
                assert_eq!(timing.required_delay_ms, Some(5_000));
                let step = events
                    .iter()
                    .find(|event| event.event_type == EventType::TaskStepStarted)
                    .expect("original step event");
                assert_eq!(step.links.action_id(), terminal.step_action_id.as_ref());
            } else {
                assert_eq!(
                    code,
                    if case == "task-timeout" {
                        "contained_task_timeout"
                    } else {
                        "contained_task_page_unknown"
                    }
                );
                assert_eq!(
                    timing.stage,
                    actingcommand_contract::TaskTimingStage::PageRecognition
                );
                assert_eq!(timing.limit_ms, 50);
                assert!(timing.elapsed_ms >= timing.limit_ms);
                assert_eq!(timing.required_delay_ms, None);
            }
            let failed = events
                .iter()
                .find(|event| event.event_type == EventType::TaskFailed)
                .unwrap();
            assert!(matches!(
                projected_task_semantic_fact(failed),
                Some(TaskSemanticFact::TerminalCommitted { executed_steps, .. })
                    if *executed_steps == Some(dispatched)
            ));
        }
        drop(client);
        host.close().expect("close host");
    }
}
