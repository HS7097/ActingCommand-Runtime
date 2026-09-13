// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn runtime_executes_neutral_contained_task_without_lab_ownership() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("neutral-task.zip");
    let bytes = neutral_contained_task_package();
    fs::write(&package, &bytes).expect("write package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let stable_instance_id = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral.instance",
            stable_instance_id,
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    // #97 P5: the same production ingress must reject an unconsumed nested declaration
    // before lease/device work and return the exact native rejection fact.
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let mut invalid_task: serde_json::Value = serde_json::from_reader(
        archive
            .by_name("resources/operations/task/task.json")
            .unwrap(),
    )
    .unwrap();
    invalid_task["operations"][0]["click"]["unused_field"] = serde_json::json!("private-value");
    let invalid_bytes =
        neutral_contained_task_package_with_task(&serde_json::to_vec(&invalid_task).unwrap());
    let invalid_path = root.path().join("unconsumed-task.zip");
    fs::write(&invalid_path, &invalid_bytes).unwrap();
    let invalid_hash =
        actingcommand_pack_containment::Sha256Hash::digest(&invalid_bytes).to_string();
    let rejected_request = client.request(RuntimeOperation::run_contained_task(
        "neutral.instance",
        client.ids.mint_holder_id().unwrap(),
        ContainedTaskRequest::new(invalid_path.display().to_string(), invalid_hash.clone())
            .unwrap(),
    ));
    let rejected = client.send(&rejected_request);
    assert_eq!(rejected.state(), RuntimeReceiptState::Denied);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 0);
    let refusal = rejected
        .resource_declaration()
        .expect("typed declaration refusal");
    assert_eq!(
        refusal.declared_package.legacy_sha256(),
        Some(invalid_hash.as_str())
    );
    assert_eq!(
        refusal.verified_package.as_ref(),
        Some(&refusal.declared_package)
    );
    assert_eq!(
        refusal.issue.declaration_file,
        "resources/operations/task/task.json"
    );
    assert_eq!(refusal.issue.field_path, "/operations/0/click/unused_field");
    assert_eq!(
        refusal.issue.reason,
        actingcommand_contract::ResourceDeclarationReason::UnknownField
    );
    let rejected_events = projected_events(
        &mut client,
        EventQuery {
            request_id: Some(rejected_request.request_id()),
            ..EventQuery::default()
        },
    );
    let [event] = rejected_events.as_slice() else {
        panic!("only one pre-execution rejection fact");
    };
    assert_eq!(event.event_type, EventType::RuntimeFailed);
    assert_eq!(
        event.sequence,
        rejected.resource_declaration_event().unwrap().sequence
    );
    assert_eq!(
        event.event_id,
        rejected.resource_declaration_event().unwrap().event_id
    );
    assert_eq!(
        event.links.correlation_id(),
        Some(&rejected_request.correlation_id())
    );
    let ProjectionPayload::Full(payload) = &event.payload else {
        panic!("full rejection payload");
    };
    let EventPayload::Runtime(actingcommand_contract::RuntimePayload::Failed(record)) =
        payload.as_ref()
    else {
        panic!("native Runtime failure");
    };
    assert_eq!(record.resource_declaration(), Some(refusal));
    assert_eq!(record.effect_disposition(), EffectDisposition::NotPerformed);
    assert!(
        !serde_json::to_string(&rejected)
            .unwrap()
            .contains("private-value")
    );
    assert!(
        !serde_json::to_string(event)
            .unwrap()
            .contains("private-value")
    );
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string(), expected.clone())
                .expect("task request"),
        ),
    );

    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            outcome: TaskOutcome::Success,
            final_page: Some(page),
            executed_steps: 1,
            ..
        }) if page == "neutral/terminal"
    ));
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 2);
    let event_types = event_types_for_correlation(&mut client, correlation_id);
    for required in [
        EventType::TaskRequested,
        EventType::TaskStarted,
        EventType::CaptureCompleted,
        EventType::TaskEvidenceIndexed,
        EventType::TaskRecognitionStarted,
        EventType::RecognitionCompleted,
        EventType::TaskRecognitionCompleted,
        EventType::TaskStepStarted,
        EventType::TaskEffectIntent,
        EventType::InputIntent,
        EventType::InputCommitted,
        EventType::TaskEffectCompleted,
        EventType::TaskStepFinished,
        EventType::TaskTerminalIntent,
        EventType::TaskCompleted,
    ] {
        assert!(event_types.contains(&required), "missing {required:?}");
    }
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let input_intents = events
        .iter()
        .filter(|event| event.event_type == EventType::InputIntent)
        .collect::<Vec<_>>();
    let [input_intent] = input_intents.as_slice() else {
        panic!("one physical input intent");
    };
    let ProjectionPayload::Full(payload) = &input_intent.payload else {
        panic!("full input intent");
    };
    let EventPayload::Input(actingcommand_contract::InputPayload::Intent(input)) = payload.as_ref()
    else {
        panic!("typed input intent");
    };
    let provenance = input
        .provenance()
        .expect("durable input provenance")
        .clone();
    let physical_id = *input_intent.links.action_id().expect("physical action");
    let step_intents = events
        .iter()
        .filter(|event| event.event_type == EventType::TaskEffectIntent)
        .collect::<Vec<_>>();
    let [step_intent] = step_intents.as_slice() else {
        panic!("one task effect intent");
    };
    assert_eq!(
        provenance.source_step_action_id.as_ref(),
        step_intent.links.action_id()
    );
    assert_eq!(
        provenance.before_frame_id.as_ref(),
        step_intent.links.frame_id()
    );
    assert_ne!(Some(&physical_id), step_intent.links.action_id());
    let TaskSemanticFact::EffectIntent { action, .. } =
        projected_task_semantic_fact(step_intent).unwrap()
    else {
        panic!("step input action");
    };
    assert_eq!(&provenance.input_action, action);
    let outcomes = events
        .iter()
        .filter(|event| event.event_type == EventType::InputCommitted)
        .collect::<Vec<_>>();
    let [outcome] = outcomes.as_slice() else {
        panic!("one physical outcome");
    };
    assert_eq!(outcome.links.action_id(), Some(&physical_id));
    assert!(
        step_intent.sequence < input_intent.sequence && input_intent.sequence < outcome.sequence
    );
    let after_captures = events
        .iter()
        .filter(|event| {
            event.event_type == EventType::CaptureRequested
                && event.links.action_id() == Some(&physical_id)
        })
        .collect::<Vec<_>>();
    let [after_capture] = after_captures.as_slice() else {
        panic!("first post-input capture");
    };
    let after_frame = *after_capture.links.frame_id().unwrap();
    let before_frame = provenance.before_frame_id.unwrap();
    assert_ne!(before_frame, after_frame);
    assert!(outcome.sequence < after_capture.sequence);
    for frame_id in [before_frame, after_frame] {
        let completed = events
            .iter()
            .filter(|event| {
                event.event_type == EventType::CaptureCompleted
                    && event.links.frame_id() == Some(&frame_id)
            })
            .collect::<Vec<_>>();
        let [completed] = completed.as_slice() else {
            panic!("one capture completion per frame");
        };
        assert_eq!(
            completed.links.action_id(),
            (frame_id == after_frame).then_some(&physical_id)
        );
        for event_type in [EventType::ArtifactCreated, EventType::ArtifactVerified] {
            let artifacts = events
                .iter()
                .filter(|event| {
                    event.event_type == event_type
                        && event.links.frame_id() == Some(&frame_id)
                        && event
                            .artifacts
                            .iter()
                            .any(|artifact| artifact.kind == ArtifactKind::CaptureFrame)
                })
                .collect::<Vec<_>>();
            let [artifact_event] = artifacts.as_slice() else {
                panic!("one persisted PNG event");
            };
            assert_eq!(
                artifact_event.links.action_id(),
                completed.links.action_id()
            );
            assert_eq!(artifact_event.links.run_id(), input_intent.links.run_id());
            assert_eq!(artifact_event.links.task_id(), input_intent.links.task_id());
            assert_eq!(
                artifact_event.links.request_id(),
                input_intent.links.request_id()
            );
            let [artifact] = artifact_event.artifacts.as_slice() else {
                panic!("one PNG reference");
            };
            let bytes = read_projected_verified(root.path(), artifact).unwrap();
            assert_eq!(
                artifact.sha256,
                format!("sha256:{:x}", Sha256::digest(bytes))
            );
        }
    }
    let semantic = events
        .iter()
        .filter_map(projected_task_semantic_fact)
        .collect::<Vec<_>>();
    assert!(semantic.iter().any(|fact| matches!(
        fact,
        TaskSemanticFact::PackageAdmitted { package_sha256, .. }
            if package_sha256 == &actingcommand_contract::PackageRef::from(&expected)
    )));
    assert_eq!(
        semantic
            .iter()
            .filter(|fact| matches!(fact, TaskSemanticFact::EvidenceIndexed { .. }))
            .count(),
        2
    );
    assert_eq!(
        semantic
            .iter()
            .filter(|fact| matches!(fact, TaskSemanticFact::RecognitionStarted { .. }))
            .count(),
        2
    );
    assert_eq!(
        semantic
            .iter()
            .filter(|fact| matches!(fact, TaskSemanticFact::RecognitionCompleted { .. }))
            .count(),
        2
    );
    assert_eq!(
        semantic
            .iter()
            .filter(|fact| matches!(fact, TaskSemanticFact::TerminalCommitted { .. }))
            .count(),
        1
    );
    assert!(semantic.iter().any(|fact| matches!(
        fact,
        TaskSemanticFact::TerminalCommitted {
            scheduling_disposition: None,
            ..
        }
    )));
    let evidence_frames = events
        .iter()
        .filter(|event| event.event_type == EventType::TaskEvidenceIndexed)
        .map(|event| *event.links.frame_id().expect("evidence frame id"))
        .collect::<BTreeSet<_>>();
    let verified_frames = events
        .iter()
        .filter(|event| event.event_type == EventType::ArtifactVerified)
        .filter(|event| {
            event.artifacts.iter().any(|artifact| {
                artifact.kind == ArtifactKind::CaptureFrame
                    && artifact.producer == ArtifactProducer::CaptureStore
            })
        })
        .map(|event| *event.links.frame_id().expect("artifact frame id"))
        .collect::<BTreeSet<_>>();
    assert_eq!(evidence_frames, verified_frames);
    assert_eq!(evidence_frames.len(), 2);
    let summaries = events
        .iter()
        .filter(|event| event.event_type == EventType::CaptureSummaryCommitted)
        .collect::<Vec<_>>();
    let [summary_event] = summaries.as_slice() else {
        panic!("contained task must commit one capture summary");
    };
    let terminal_event = events
        .iter()
        .find(|event| event.event_type == EventType::TaskCompleted)
        .expect("task terminal");
    let diagnostics = events
        .iter()
        .filter(|event| event.event_type == EventType::ArtifactVerified)
        .filter(|event| {
            event.artifacts.iter().any(|artifact| {
                artifact.kind == ArtifactKind::DiagnosticJson
                    && artifact.redaction_state
                        == actingcommand_contract::ArtifactRedactionState::Pending
            })
        })
        .collect::<Vec<_>>();
    let [diagnostic] = diagnostics.as_slice() else {
        panic!("one task stream publication")
    };
    assert!(diagnostic.sequence < terminal_event.sequence);
    let artifact = &diagnostic.artifacts[0];
    let created = events
        .iter()
        .filter(|event| {
            event.event_type == EventType::ArtifactCreated
                && event
                    .artifacts
                    .iter()
                    .any(|reference| reference.artifact_id == artifact.artifact_id)
        })
        .collect::<Vec<_>>();
    assert_eq!(created.len(), 1);
    assert!(created[0].sequence < diagnostic.sequence);
    let document: serde_json::Value =
        serde_json::from_slice(&read_projected_verified(root.path(), artifact).unwrap()).unwrap();
    assert_eq!(
        document["request_id"],
        serde_json::to_value(request.request_id()).unwrap()
    );
    assert_eq!(
        document["run_id"],
        serde_json::to_value(input_intent.links.run_id().unwrap()).unwrap()
    );
    let records = document["records"].as_array().unwrap();
    let start = records
        .iter()
        .find(|record| record["kind"] == "step_started")
        .unwrap();
    assert_eq!(
        start["frame_id"],
        serde_json::to_value(before_frame).unwrap()
    );
    assert_eq!(
        start["step_action_id"],
        serde_json::to_value(provenance.source_step_action_id.unwrap()).unwrap()
    );
    let elapsed = records
        .iter()
        .find(|record| record["kind"] == "step_elapsed")
        .unwrap();
    assert_eq!(
        elapsed["frame_id"],
        serde_json::to_value(after_frame).unwrap()
    );
    assert_eq!(
        elapsed["physical_action_id"],
        serde_json::to_value(physical_id).unwrap()
    );
    assert_eq!(elapsed["data"]["completed"], true);
    for source in events.iter().filter(|event| {
        event.event_type == EventType::ArtifactVerified && event.sequence < diagnostic.sequence
    }) {
        for reference in &source.artifacts {
            assert_eq!(
                records
                    .iter()
                    .filter(|record| record["kind"] == "artifact"
                        && record["data"]["source_sequence"] == source.sequence
                        && record["data"]["artifact"]["artifact_id"]
                            == serde_json::to_value(reference.artifact_id).unwrap()
                        && record["data"]["artifact"]["sha256"] == reference.sha256)
                    .count(),
                1
            );
        }
    }
    assert_eq!(records.last().unwrap()["kind"], "terminal");
    assert!(summary_event.sequence < terminal_event.sequence);
    assert_eq!(summary_event.links.run_id(), terminal_event.links.run_id());
    assert_eq!(
        summary_event.origin,
        actingcommand_contract::EventOrigin::new(
            EventSource::Runtime,
            OriginModule::CapturePipeline,
            EventActor::Runtime,
        )
    );
    let ProjectionPayload::Full(payload) = &summary_event.payload else {
        panic!("forensic capture summary");
    };
    let EventPayload::Capture(CapturePayload::SummaryCommitted(summary)) = payload.as_ref() else {
        panic!("typed capture summary");
    };
    assert_eq!(summary.summary().captured(), 2);
    assert_eq!(summary.summary().persisted(), 2);
    assert_eq!(summary.summary().deduplicated(), 0);
    assert_eq!(summary.summary().dropped(), 0);
    assert_eq!(
        summary.summary().evidence_completeness(),
        actingcommand_contract::EvidenceCompleteness::Complete
    );
    assert_eq!(summary.summary().frames().len(), 2);
    assert_eq!(
        summary.summary().frames()[0].artifact().frame_id,
        Some(before_frame)
    );
    assert_eq!(
        summary.summary().frames()[1].artifact().frame_id,
        Some(after_frame)
    );
    assert_eq!(
        summary
            .summary()
            .pinned()
            .iter()
            .map(|pin| (pin.frame_index(), pin.reason()))
            .collect::<Vec<_>>(),
        vec![
            (Some(0), PinnedFrameReason::PreInput),
            (Some(0), PinnedFrameReason::RecognitionEvidence),
            (Some(1), PinnedFrameReason::PostInput),
            (Some(1), PinnedFrameReason::RecognitionEvidence),
            (Some(1), PinnedFrameReason::Terminal),
        ]
    );
    let summary_record = summary.summary().clone();
    drop(client);
    host.close().expect("close host");

    let restarted = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral.instance",
            stable_instance_id,
            state,
        )),
    )
    .expect("restart runtime host");
    let mut replay_client = TestClient::connect(&restarted);
    let replayed = projected_events(
        &mut replay_client,
        EventQuery {
            correlation_id: Some(correlation_id),
            event_type: Some(EventType::CaptureSummaryCommitted),
            ..EventQuery::default()
        },
    );
    let [replayed_summary] = replayed.as_slice() else {
        panic!("restart must project one capture summary");
    };
    let ProjectionPayload::Full(payload) = &replayed_summary.payload else {
        panic!("replayed forensic capture summary");
    };
    let EventPayload::Capture(CapturePayload::SummaryCommitted(summary)) = payload.as_ref() else {
        panic!("replayed typed capture summary");
    };
    assert_eq!(summary.summary(), &summary_record);
    let restored_inputs = projected_events(
        &mut replay_client,
        EventQuery {
            correlation_id: Some(correlation_id),
            event_type: Some(EventType::InputIntent),
            ..EventQuery::default()
        },
    );
    let [restored_input] = restored_inputs.as_slice() else {
        panic!("one restored input");
    };
    let ProjectionPayload::Full(payload) = &restored_input.payload else {
        panic!("full restored intent");
    };
    let EventPayload::Input(actingcommand_contract::InputPayload::Intent(input)) = payload.as_ref()
    else {
        panic!("typed restored intent");
    };
    assert_eq!(input.provenance(), Some(&provenance));
    assert_eq!(restored_input.links.action_id(), Some(&physical_id));
    drop(replay_client);
    restarted.close().expect("close restarted host");
}
