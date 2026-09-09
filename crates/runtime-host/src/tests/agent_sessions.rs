// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn agent_dispatcher_sidecar_child_process() {
    let Ok(root) = std::env::var("ACTINGCOMMAND_AGENT_SIDECAR_ROOT") else {
        return;
    };
    let mode = std::env::var("ACTINGCOMMAND_AGENT_SIDECAR_MODE").expect("sidecar mode");
    let mut client = TestClient::connect_state_root(Path::new(&root));
    match mode.as_str() {
        "start" => {
            let subscription = actingcommand_contract::RuntimeSubscriptionRequest::new(
                EventQuery {
                    event_type: Some(EventType::AgentWakeRequested),
                    ..EventQuery::default()
                },
                ProjectionProfile::Normal,
                actingcommand_contract::SubscriptionCursor::default(),
                1_000,
                8,
            )
            .expect("wake subscription");
            let request = client.agent_request(RuntimeOperation::SubscribeEvents {
                request: subscription,
            });
            let receipt = client.send(&request);
            let RuntimeResult::EventBatch { batch } =
                receipt.result().expect("wake subscription result")
            else {
                panic!("expected wake event batch")
            };
            let wake_id = batch
                .events()
                .iter()
                .find_map(|event| match &event.payload {
                    ProjectionPayload::Public(payload) => match payload.as_ref() {
                        PublicEventPayload::Agent(payload) => payload.agent_wake_id(),
                        _ => None,
                    },
                    _ => None,
                })
                .expect("projected wake id");
            let request = client.agent_request(RuntimeOperation::StartAgentSession { wake_id });
            let receipt = client.send(&request);
            let RuntimeResult::AgentSessionOpened { context } =
                receipt.result().expect("agent session result")
            else {
                panic!("expected agent session context")
            };
            assert_eq!(context.status().state(), AgentAttentionState::Active);
            assert_eq!(context.projection().len(), 2);
            fs::write(
                Path::new(&root).join("agent-session.json"),
                serde_json::to_vec(&context.status().session_id()).expect("session id bytes"),
            )
            .expect("session marker");
        }
        "resume" => {
            let session_id: AgentSessionId = serde_json::from_str(
                &std::env::var("ACTINGCOMMAND_AGENT_SESSION_ID").expect("session id"),
            )
            .expect("typed session id");
            let request = client.agent_request(RuntimeOperation::ResumeAgentSession { session_id });
            let receipt = client.send(&request);
            let RuntimeResult::AgentSessionObserved { context } =
                receipt.result().expect("agent resume result")
            else {
                panic!("expected resumed agent session")
            };
            assert_eq!(context.status().state(), AgentAttentionState::Active);
            let response = AgentSessionResponse::new(
                session_id,
                AgentResponseDisposition::RetryableFailure,
                "fake_sidecar_failed",
                unix_ms_now().expect("wall clock"),
            )
            .expect("agent response");
            let request = client.agent_request(RuntimeOperation::RecordAgentResponse { response });
            let receipt = client.send(&request);
            let RuntimeResult::AgentResponseRecorded { status } =
                receipt.result().expect("agent response result")
            else {
                panic!("expected agent response status")
            };
            assert_eq!(status.state(), AgentAttentionState::PausedNeedsHuman);
            fs::write(Path::new(&root).join("agent-resumed"), b"done").expect("resume marker");
        }
        other => panic!("unexpected sidecar mode: {other}"),
    }
}

#[test]
fn detachable_agent_sidecar_recovers_and_escalates_without_device_authority() {
    let root = TempDir::new().expect("tempdir");
    let shared_instance_id = instance_id();
    let fake_state = Arc::new(FakeState::default());
    let agent_config = AgentDispatcherConfig::new(1, 60_000, 2).expect("agent config");
    let host = RuntimeHost::start(
        config(&root).with_agent_dispatcher(agent_config.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            shared_instance_id,
            Arc::clone(&fake_state),
        )),
    )
    .expect("runtime host");
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:timeline-review-a".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::TimelineReached,
        fact_code: "timeline.review.due".to_owned(),
        observed_at_unix_ms: unix_ms_now().expect("wall clock"),
        detection_budget: None,
    })
    .expect("timeline wake signal");
    let mut observer = TestClient::connect(&host);
    let wakes = projected_events(
        &mut observer,
        EventQuery {
            event_type: Some(EventType::AgentWakeRequested),
            ..EventQuery::default()
        },
    );
    assert_eq!(wakes.len(), 1);
    let ProjectionPayload::Full(payload) = &wakes[0].payload else {
        panic!("expected forensic wake payload")
    };
    let EventPayload::Agent(AgentPayload::WakeRequested(payload)) = payload.as_ref() else {
        panic!("expected agent wake payload")
    };
    assert_eq!(payload.wake().kind(), AgentWakeKind::TimelineReached);
    assert_eq!(
        payload.wake().attention_state(),
        AgentAttentionState::PausedNeedsAgent
    );
    let start = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "tests::agent_sessions::agent_dispatcher_sidecar_child_process",
            "--nocapture",
        ])
        .env("ACTINGCOMMAND_AGENT_SIDECAR_ROOT", root.path())
        .env("ACTINGCOMMAND_AGENT_SIDECAR_MODE", "start")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run starting sidecar");
    assert!(start.success());
    let session_id: AgentSessionId = serde_json::from_slice(
        &fs::read(root.path().join("agent-session.json")).expect("session marker"),
    )
    .expect("session id");
    drop(observer);
    host.close().expect("close runtime with active agent");

    let recovered = RuntimeHost::start(
        config(&root).with_agent_dispatcher(agent_config),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            shared_instance_id,
            Arc::clone(&fake_state),
        )),
    )
    .expect("recovered runtime host");
    let resume = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "tests::agent_sessions::agent_dispatcher_sidecar_child_process",
            "--nocapture",
        ])
        .env("ACTINGCOMMAND_AGENT_SIDECAR_ROOT", root.path())
        .env("ACTINGCOMMAND_AGENT_SIDECAR_MODE", "resume")
        .env(
            "ACTINGCOMMAND_AGENT_SESSION_ID",
            serde_json::to_string(&session_id).expect("session id JSON"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run resumed sidecar");
    assert!(resume.success());
    assert!(root.path().join("agent-resumed").is_file());

    let mut observer = TestClient::connect(&recovered);
    for (event_type, expected) in [
        (EventType::AgentWakeRequested, 1),
        (EventType::AgentSessionStarted, 1),
        (EventType::AgentSessionResumed, 1),
        (EventType::AgentSessionEscalated, 1),
    ] {
        assert_eq!(
            projected_events(
                &mut observer,
                EventQuery {
                    event_type: Some(event_type),
                    ..EventQuery::default()
                },
            )
            .len(),
            expected
        );
    }
    let request = observer.agent_request(RuntimeOperation::AgentSessionStatus { session_id });
    let receipt = observer.send(&request);
    let RuntimeResult::AgentSessionObserved { context } =
        receipt.result().expect("agent status result")
    else {
        panic!("expected agent status")
    };
    assert_eq!(
        context.status().state(),
        AgentAttentionState::PausedNeedsHuman
    );
    assert_eq!(fake_state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.input_count.load(Ordering::SeqCst), 0);
    let drift = PolicyPlanningSignalEventData {
        signal_id: "signal:drift-review-a".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::DriftPredicted,
        fact_code: "goal.primary.drift_predicted".to_owned(),
        observed_at_unix_ms: unix_ms_now().expect("wall clock"),
        detection_budget: None,
    };
    recovered
        .record_policy_planning_signal(drift.clone())
        .expect("drift wake signal");
    recovered
        .record_policy_planning_signal(drift)
        .expect("idempotent drift signal");
    let wakes = projected_events(
        &mut observer,
        EventQuery {
            event_type: Some(EventType::AgentWakeRequested),
            ..EventQuery::default()
        },
    );
    assert_eq!(wakes.len(), 2);
    assert!(wakes.iter().any(|event| {
        matches!(
            &event.payload,
            ProjectionPayload::Full(payload)
                if matches!(
                    payload.as_ref(),
                    EventPayload::Agent(AgentPayload::WakeRequested(payload))
                        if payload.wake().kind() == AgentWakeKind::DriftPredicted
                )
        )
    }));
    drop(observer);
    recovered.close().expect("close recovered runtime");
}

#[test]
fn agent_session_start_and_completion_are_idempotent() {
    let root = TempDir::new().expect("tempdir");
    let fake_state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root)
            .with_agent_dispatcher(AgentDispatcherConfig::new(2, 60_000, 2).expect("agent config")),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&fake_state),
        )),
    )
    .expect("runtime host");
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:agent-completion-a".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::TimelineReached,
        fact_code: "timeline.review.due".to_owned(),
        observed_at_unix_ms: unix_ms_now().expect("wall clock"),
        detection_budget: None,
    })
    .expect("timeline wake signal");
    let mut client = TestClient::connect(&host);
    let wakes = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::AgentWakeRequested),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) = &wakes[0].payload else {
        panic!("expected forensic wake payload")
    };
    let EventPayload::Agent(AgentPayload::WakeRequested(payload)) = payload.as_ref() else {
        panic!("expected agent wake payload")
    };
    let wake_id = payload.wake().wake_id();

    let first = client.agent_request(RuntimeOperation::StartAgentSession { wake_id });
    let first = client.send(&first);
    let RuntimeResult::AgentSessionOpened { context } =
        first.result().expect("first session result")
    else {
        panic!("expected first session context")
    };
    let session_id = context.status().session_id();
    let replay = client.agent_request(RuntimeOperation::StartAgentSession { wake_id });
    let replay = client.send(&replay);
    let RuntimeResult::AgentSessionOpened { context } = replay.result().expect("replay result")
    else {
        panic!("expected replayed session context")
    };
    assert_eq!(context.status().session_id(), session_id);
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::AgentSessionStarted),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );

    let response = AgentSessionResponse::new(
        session_id,
        AgentResponseDisposition::Completed,
        "fake_sidecar_completed",
        unix_ms_now().expect("wall clock"),
    )
    .expect("agent response");
    let request = client.agent_request(RuntimeOperation::RecordAgentResponse { response });
    for reconnect in [false, true] {
        if reconnect {
            drop(client);
            client = TestClient::connect(&host);
        }
        let receipt = client.send(&request);
        let RuntimeResult::AgentResponseRecorded { status } =
            receipt.result().expect("completion result")
        else {
            panic!("expected completion status")
        };
        assert_eq!(status.state(), AgentAttentionState::Completed);
    }
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::AgentSessionCompleted),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );
    assert_eq!(fake_state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.input_count.load(Ordering::SeqCst), 0);
    drop(client);
    host.close().expect("close runtime");
}

#[test]
fn agent_terminal_history_allows_restart_without_dispatcher_config() {
    let root = TempDir::new().expect("tempdir");
    let runtime_instance_id = instance_id();
    let host = RuntimeHost::start(
        config(&root)
            .with_agent_dispatcher(AgentDispatcherConfig::new(2, 60_000, 2).expect("agent config")),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:terminal-history-a".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::TimelineReached,
        fact_code: "timeline.review.due".to_owned(),
        observed_at_unix_ms: unix_ms_now().expect("wall clock"),
        detection_budget: None,
    })
    .expect("timeline wake signal");
    host.close().expect("close runtime with pending wake");
    let error = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .err()
    .expect("pending wake must retain dispatcher config");
    assert_eq!(error.code(), "agent_dispatcher_config_missing");

    let host = RuntimeHost::start(
        config(&root)
            .with_agent_dispatcher(AgentDispatcherConfig::new(2, 60_000, 2).expect("agent config")),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime with pending wake");
    let mut client = TestClient::connect(&host);
    let wakes = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::AgentWakeRequested),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) = &wakes[0].payload else {
        panic!("expected forensic wake payload")
    };
    let EventPayload::Agent(AgentPayload::WakeRequested(payload)) = payload.as_ref() else {
        panic!("expected agent wake payload")
    };
    let request = client.agent_request(RuntimeOperation::StartAgentSession {
        wake_id: payload.wake().wake_id(),
    });
    let receipt = client.send(&request);
    let RuntimeResult::AgentSessionOpened { context } =
        receipt.result().expect("agent session result")
    else {
        panic!("expected agent session context")
    };
    let response = AgentSessionResponse::new(
        context.status().session_id(),
        AgentResponseDisposition::Completed,
        "fake_sidecar_completed",
        unix_ms_now().expect("wall clock"),
    )
    .expect("agent response");
    let request = client.agent_request(RuntimeOperation::RecordAgentResponse { response });
    let receipt = client.send(&request);
    let RuntimeResult::AgentResponseRecorded { status } =
        receipt.result().expect("agent response result")
    else {
        panic!("expected agent response status")
    };
    assert_eq!(status.state(), AgentAttentionState::Completed);
    drop(client);
    host.close().expect("close runtime");

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("terminal history must not require dispatcher config");
    reopened.close().expect("close reopened runtime");
}

#[test]
fn agent_resume_request_replays_across_reconnect_and_restart() {
    let root = TempDir::new().expect("tempdir");
    let runtime_instance_id = instance_id();
    let agent_config = AgentDispatcherConfig::new(2, 60_000, 2).expect("agent config");
    let host = RuntimeHost::start(
        config(&root).with_agent_dispatcher(agent_config.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:resume-replay-a".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::DriftPredicted,
        fact_code: "goal.primary.drift_predicted".to_owned(),
        observed_at_unix_ms: unix_ms_now().expect("wall clock"),
        detection_budget: None,
    })
    .expect("drift wake signal");
    let mut client = TestClient::connect(&host);
    let wakes = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::AgentWakeRequested),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) = &wakes[0].payload else {
        panic!("expected forensic wake payload")
    };
    let EventPayload::Agent(AgentPayload::WakeRequested(payload)) = payload.as_ref() else {
        panic!("expected agent wake payload")
    };
    let request = client.agent_request(RuntimeOperation::StartAgentSession {
        wake_id: payload.wake().wake_id(),
    });
    let receipt = client.send(&request);
    let RuntimeResult::AgentSessionOpened { context } =
        receipt.result().expect("agent session result")
    else {
        panic!("expected agent session context")
    };
    let resume = client.agent_request(RuntimeOperation::ResumeAgentSession {
        session_id: context.status().session_id(),
    });
    let first = client.send(&resume);
    drop(client);

    let mut reconnected = TestClient::connect(&host);
    assert_eq!(reconnected.send(&resume), first);
    assert_eq!(
        projected_events(
            &mut reconnected,
            EventQuery {
                event_type: Some(EventType::AgentSessionResumed),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );
    drop(reconnected);
    host.close().expect("close runtime");

    let reopened = RuntimeHost::start(
        config(&root).with_agent_dispatcher(agent_config),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime");
    let mut recovered = TestClient::connect(&reopened);
    assert_eq!(recovered.send(&resume), first);
    assert_eq!(
        projected_events(
            &mut recovered,
            EventQuery {
                event_type: Some(EventType::AgentSessionResumed),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );
    drop(recovered);
    reopened.close().expect("close reopened runtime");
}

#[test]
fn agent_cross_operation_request_ids_are_rejected_before_append_and_survive_restart() {
    let root = TempDir::new().expect("tempdir");
    let runtime_instance_id = instance_id();
    let agent_config = AgentDispatcherConfig::new(3, 60_000, 2).expect("agent config");
    let host = RuntimeHost::start(
        config(&root).with_agent_dispatcher(agent_config.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:agent-request-collision-a".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::DriftPredicted,
        fact_code: "goal.primary.drift_predicted".to_owned(),
        observed_at_unix_ms: unix_ms_now().expect("wall clock"),
        detection_budget: None,
    })
    .expect("drift wake signal");
    let mut client = TestClient::connect(&host);
    let wakes = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::AgentWakeRequested),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) = &wakes[0].payload else {
        panic!("expected forensic wake payload")
    };
    let EventPayload::Agent(AgentPayload::WakeRequested(payload)) = payload.as_ref() else {
        panic!("expected agent wake payload")
    };
    let start = client.agent_request(RuntimeOperation::StartAgentSession {
        wake_id: payload.wake().wake_id(),
    });
    let start = client.send(&start);
    let RuntimeResult::AgentSessionOpened { context } =
        start.result().expect("agent session result")
    else {
        panic!("expected agent session context")
    };
    let session_id = context.status().session_id();
    let reuse_request_id =
        |request: RuntimeRequest, request_id: actingcommand_contract::RequestId| {
            let mut encoded = serde_json::to_value(request).expect("request JSON");
            encoded["request_id"] = serde_json::to_value(request_id).expect("request id JSON");
            serde_json::from_value::<RuntimeRequest>(encoded).expect("reused request id")
        };

    let resume = client.agent_request(RuntimeOperation::ResumeAgentSession { session_id });
    let resume_id = resume.request_id();
    assert_eq!(client.send(&resume).state(), RuntimeReceiptState::Completed);
    let response = AgentSessionResponse::new(
        session_id,
        AgentResponseDisposition::RetryableFailure,
        "fake_sidecar_failed",
        unix_ms_now().expect("wall clock"),
    )
    .expect("agent response");
    let response_conflict = reuse_request_id(
        client.agent_request(RuntimeOperation::RecordAgentResponse {
            response: response.clone(),
        }),
        resume_id,
    );
    let denied = host
        .process_request_for_test(
            &response_conflict,
            ConnectionId::new(901).expect("connection"),
        )
        .expect("response conflict receipt");
    assert_eq!(denied.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        denied.error_projection().expect("response denial").code,
        RuntimeErrorCode::InvalidRequest
    );
    let resume_events = projected_events(
        &mut client,
        EventQuery {
            request_id: Some(resume_id),
            ..EventQuery::default()
        },
    );
    assert_eq!(resume_events.len(), 1);
    assert_eq!(resume_events[0].event_type, EventType::AgentSessionResumed);

    let response_request = client.agent_request(RuntimeOperation::RecordAgentResponse { response });
    let response_id = response_request.request_id();
    let response_receipt = client.send(&response_request);
    let RuntimeResult::AgentResponseRecorded { status } = response_receipt
        .result()
        .expect("retryable response result")
    else {
        panic!("expected agent response status")
    };
    assert_eq!(status.state(), AgentAttentionState::Active);
    let resume_conflict = reuse_request_id(
        client.agent_request(RuntimeOperation::ResumeAgentSession { session_id }),
        response_id,
    );
    let denied = host
        .process_request_for_test(
            &resume_conflict,
            ConnectionId::new(902).expect("connection"),
        )
        .expect("resume conflict receipt");
    assert_eq!(denied.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        denied.error_projection().expect("resume denial").code,
        RuntimeErrorCode::InvalidRequest
    );
    let response_events = projected_events(
        &mut client,
        EventQuery {
            request_id: Some(response_id),
            ..EventQuery::default()
        },
    );
    assert_eq!(response_events.len(), 1);
    assert_eq!(
        response_events[0].event_type,
        EventType::AgentResponseRecorded
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close runtime");

    let reopened = RuntimeHost::start(
        config(&root).with_agent_dispatcher(agent_config),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime after rejected conflicts");
    for (request, connection_id) in [(&response_conflict, 903), (&resume_conflict, 904)] {
        let denied = reopened
            .process_request_for_test(
                request,
                ConnectionId::new(connection_id).expect("connection"),
            )
            .expect("recovered conflict receipt");
        assert_eq!(denied.state(), RuntimeReceiptState::Denied);
        assert_eq!(
            denied.error_projection().expect("recovered denial").code,
            RuntimeErrorCode::InvalidRequest
        );
    }
    assert!(
        reopened
            .fatal_error()
            .expect("reopened runtime health")
            .is_none()
    );
    let mut recovered = TestClient::connect(&reopened);
    assert_eq!(
        projected_events(
            &mut recovered,
            EventQuery {
                request_id: Some(resume_id),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );
    assert_eq!(
        projected_events(
            &mut recovered,
            EventQuery {
                request_id: Some(response_id),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );
    drop(recovered);
    reopened.close().expect("close reopened runtime");
}

#[test]
fn agent_session_timeout_escalates_to_human() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root)
            .with_agent_dispatcher(AgentDispatcherConfig::new(2, 20, 2).expect("agent config")),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:agent-timeout-a".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::DriftPredicted,
        fact_code: "goal.primary.drift_predicted".to_owned(),
        observed_at_unix_ms: unix_ms_now().expect("wall clock"),
        detection_budget: None,
    })
    .expect("drift wake signal");
    let mut client = TestClient::connect(&host);
    let wakes = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::AgentWakeRequested),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) = &wakes[0].payload else {
        panic!("expected forensic wake payload")
    };
    let EventPayload::Agent(AgentPayload::WakeRequested(payload)) = payload.as_ref() else {
        panic!("expected agent wake payload")
    };
    let request = client.agent_request(RuntimeOperation::StartAgentSession {
        wake_id: payload.wake().wake_id(),
    });
    let receipt = client.send(&request);
    let RuntimeResult::AgentSessionOpened { context } =
        receipt.result().expect("agent session result")
    else {
        panic!("expected agent session context")
    };
    let session_id = context.status().session_id();

    let mut state = AgentAttentionState::Active;
    for _ in 0..200 {
        thread::sleep(Duration::from_millis(10));
        let request = client.agent_request(RuntimeOperation::AgentSessionStatus { session_id });
        let receipt = client.send(&request);
        let RuntimeResult::AgentSessionObserved { context } =
            receipt.result().expect("agent status result")
        else {
            panic!("expected agent status")
        };
        state = context.status().state();
        if state == AgentAttentionState::PausedNeedsHuman {
            break;
        }
    }
    assert_eq!(state, AgentAttentionState::PausedNeedsHuman);
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::AgentSessionEscalated),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );
    drop(client);
    host.close().expect("close runtime");
}

#[test]
fn agent_wake_is_reconciled_from_a_committed_planning_signal() {
    let root = TempDir::new().expect("tempdir");
    let runtime_instance_id = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:recover-wake-a".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::DriftPredicted,
        fact_code: "goal.primary.drift_predicted".to_owned(),
        observed_at_unix_ms: unix_ms_now().expect("wall clock"),
        detection_budget: None,
    })
    .expect("planning signal");
    host.close().expect("close host");

    let reopened = RuntimeHost::start(
        config(&root)
            .with_agent_dispatcher(AgentDispatcherConfig::new(2, 60_000, 2).expect("agent config")),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime");
    let mut observer = TestClient::connect(&reopened);
    assert_eq!(
        projected_events(
            &mut observer,
            EventQuery {
                event_type: Some(EventType::AgentWakeRequested),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );
    drop(observer);
    reopened.close().expect("close reopened runtime");
}
