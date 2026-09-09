// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn runtime_monitor_policy_persists_and_idempotent_updates_do_not_rewrite_state() {
    let root = TempDir::new().expect("tempdir");
    let configured_id = instance_id();
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "node.a",
            configured_id,
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let policy = RuntimeMonitorPolicy::new(1_000, "home", false).expect("policy");
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: policy.clone(),
    });
    let configured = client.send(&configure);
    let RuntimeResult::MonitorConfigured { status } =
        configured.result().expect("configured result")
    else {
        panic!("expected configured monitor");
    };
    assert_eq!(status.policy(), Some(&policy));
    let configured_events = projected_events(
        &mut client,
        EventQuery {
            request_id: Some(configure.request_id()),
            event_type: Some(EventType::CommandValidated),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) = &configured_events[0].payload else {
        panic!("monitor configuration fact");
    };
    let Some(actingcommand_contract::RuntimeStateFact::MonitorChanged { change, .. }) =
        payload.runtime_state()
    else {
        panic!("typed monitor configuration");
    };
    let configuration_version = change.configuration_version;
    assert!(change.applied);
    assert_eq!(&change.status, status);
    assert_eq!(
        event_types_for_request(
            &host,
            &client.ids,
            ConnectionId::new(99).expect("query connection"),
            configure.request_id()
        ),
        vec![
            EventType::CliCommand,
            EventType::CommandReceived,
            EventType::CommandValidated,
        ]
    );
    wait_until(Duration::from_secs(2), || {
        state.monitor_observation_count.load(Ordering::Acquire) >= 1
    });
    wait_until(Duration::from_secs(2), || {
        let status = client.send(&client.request(RuntimeOperation::MonitorStatus));
        matches!(
            status.result(),
            Some(RuntimeResult::MonitorStatus { status })
                if status.instances()[0]
                    .state()
                    .is_some_and(|state| state.run_count() >= 1)
        )
    });
    let journal = root.path().join(MONITOR_FILE_NAME);
    assert!(!journal.exists());

    let repeated = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: policy.clone(),
    });
    assert!(matches!(
        client.send(&repeated).result(),
        Some(RuntimeResult::MonitorConfigured { .. })
    ));
    assert!(!journal.exists());
    let repeated_events = projected_events(
        &mut client,
        EventQuery {
            request_id: Some(repeated.request_id()),
            event_type: Some(EventType::CommandValidated),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) = &repeated_events[0].payload else {
        panic!("idempotent configuration fact");
    };
    assert!(
        matches!(payload.runtime_state(), Some(actingcommand_contract::RuntimeStateFact::MonitorChanged { change, .. })
        if !change.applied && change.configuration_version == configuration_version)
    );
    let status = client.send(&client.request(RuntimeOperation::MonitorStatus));
    let RuntimeResult::MonitorStatus { status } = status.result().expect("monitor status") else {
        panic!("expected monitor status");
    };
    assert_eq!(status.instances().len(), 1);
    assert_eq!(status.instances()[0].policy(), Some(&policy));
    assert!(status.source().is_some());
    drop(client);
    host.close().expect("close host");

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one("node.a", configured_id, state)),
    )
    .expect("reopened runtime");
    let mut client = TestClient::connect(&reopened);
    let status = client.send(&client.request(RuntimeOperation::MonitorStatus));
    let RuntimeResult::MonitorStatus { status } = status.result().expect("reopened status") else {
        panic!("expected reopened monitor status");
    };
    assert_eq!(status.instances()[0].policy(), Some(&policy));
    assert!(
        status.instances()[0]
            .state()
            .is_some_and(|state| state.run_count() >= 1)
    );

    let clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert!(matches!(
        client.send(&clear).result(),
        Some(RuntimeResult::MonitorCleared { status }) if status.policy().is_none()
    ));
    assert!(!journal.exists());
    let cleared_events = projected_events(
        &mut client,
        EventQuery {
            request_id: Some(clear.request_id()),
            event_type: Some(EventType::CommandValidated),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) = &cleared_events[0].payload else {
        panic!("monitor clear fact");
    };
    let Some(actingcommand_contract::RuntimeStateFact::MonitorChanged { change, .. }) =
        payload.runtime_state()
    else {
        panic!("typed clear");
    };
    let cleared_version = change.configuration_version;
    let repeated_clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert!(matches!(
        client.send(&repeated_clear).result(),
        Some(RuntimeResult::MonitorCleared { status }) if status.policy().is_none()
    ));
    assert!(!journal.exists());
    let repeated_events = projected_events(
        &mut client,
        EventQuery {
            request_id: Some(repeated_clear.request_id()),
            event_type: Some(EventType::CommandValidated),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) = &repeated_events[0].payload else {
        panic!("idempotent clear fact");
    };
    assert!(
        matches!(payload.runtime_state(), Some(actingcommand_contract::RuntimeStateFact::MonitorChanged { change, .. })
        if !change.applied && change.configuration_version == cleared_version)
    );
    drop(client);
    reopened.close().expect("close reopened host");
}

#[test]
fn resident_monitor_runs_without_a_client_and_records_artifact_backed_lifecycle() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: RuntimeMonitorPolicy::new(200, "home", false).expect("monitor policy"),
    });
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    drop(client);

    wait_until(Duration::from_secs(2), || {
        state.monitor_observation_count.load(Ordering::Acquire) >= 1
    });
    let mut client = TestClient::connect(&host);
    wait_until(Duration::from_secs(2), || {
        let status = client.send(&client.request(RuntimeOperation::MonitorStatus));
        matches!(
            status.result(),
            Some(RuntimeResult::MonitorStatus { status })
                if status.instances()[0].state().is_some_and(|state| {
                    state.run_count() >= 1
                        && state.last_decision().is_some_and(|decision| {
                            decision.disposition() == MonitorDisposition::Healthy
                        })
                })
        )
    });

    let completed = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::MonitorProbeCompleted),
            ..EventQuery::default()
        },
    );
    let event = completed.last().expect("monitor completion event");
    let ProjectionPayload::Full(payload) = &event.payload else {
        panic!("expected full monitor completion payload");
    };
    let EventPayload::Monitor(MonitorPayload::Completed(detail)) = payload.as_ref() else {
        panic!("expected full monitor completion payload");
    };
    assert_eq!(detail.observation().diagnosis(), MonitorDiagnosis::Healthy);
    assert_eq!(detail.decision().disposition(), MonitorDisposition::Healthy);
    let run_id = *event.links.run_id().expect("monitor run id");
    let lifecycle = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(run_id),
            ..EventQuery::default()
        },
    );
    assert_eq!(
        lifecycle
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            EventType::MonitorProbeRequested,
            EventType::MonitorProbeStarted,
            EventType::CaptureRequested,
            EventType::RecognitionRequested,
            EventType::ArtifactCreated,
            EventType::ArtifactVerified,
            EventType::CaptureCompleted,
            EventType::RecognitionCompleted,
            EventType::MonitorProbeCompleted,
        ]
    );
    let artifact = lifecycle
        .iter()
        .find(|event| event.event_type == EventType::ArtifactVerified)
        .and_then(|event| event.artifacts.first())
        .expect("verified monitor artifact");
    assert!(
        read_projected_verified(root.path(), artifact)
            .expect("monitor artifact bytes")
            .starts_with(b"\x89PNG\r\n\x1a\n")
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);

    let clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert_eq!(client.send(&clear).state(), RuntimeReceiptState::Completed);
    thread::sleep(Duration::from_millis(300));
    let observations = state.monitor_observation_count.load(Ordering::Acquire);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        state.monitor_observation_count.load(Ordering::Acquire),
        observations
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn resident_monitor_uses_completion_based_cadence_without_a_tight_loop() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: RuntimeMonitorPolicy::new(100, "home", false).expect("monitor policy"),
    });
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    wait_until(Duration::from_secs(2), || {
        state.monitor_observation_count.load(Ordering::Acquire) >= 3
    });
    let clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert_eq!(client.send(&clear).state(), RuntimeReceiptState::Completed);

    let started = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::MonitorProbeStarted),
            ..EventQuery::default()
        },
    );
    assert!(started.len() >= 3);
    for pair in started[..3].windows(2) {
        assert!(pair[1].timestamp_unix_ms - pair[0].timestamp_unix_ms >= 100);
    }
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn monitor_recovery_is_scheduler_admitted_without_executing_an_effect() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.monitor_mode.store(1, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: RuntimeMonitorPolicy::new(500, "home", true).expect("monitor policy"),
    });
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    wait_until(Duration::from_secs(2), || {
        !projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::MonitorRecoveryAdmitted),
                ..EventQuery::default()
            },
        )
        .is_empty()
    });
    let clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert_eq!(client.send(&clear).state(), RuntimeReceiptState::Completed);

    let admitted = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::MonitorRecoveryAdmitted),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) = &admitted.last().expect("recovery admission").payload
    else {
        panic!("expected full recovery admission payload");
    };
    let EventPayload::Monitor(MonitorPayload::RecoveryAdmitted(detail)) = payload.as_ref() else {
        panic!("expected full recovery admission payload");
    };
    assert_eq!(detail.recovery(), MonitorRecoveryKind::WakeStandby);
    assert_eq!(
        detail.reason(),
        MonitorRecoveryCoordinationReason::SchedulerAvailable
    );
    assert_eq!(detail.effect_disposition(), EffectDisposition::NotPerformed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::TaskRequested),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn monitor_recovery_is_deferred_by_an_active_fenced_lease() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.monitor_mode.store(1, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("node.a");
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: RuntimeMonitorPolicy::new(500, "home", true).expect("monitor policy"),
    });
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    wait_until(Duration::from_secs(2), || {
        !projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::MonitorRecoveryDeferred),
                ..EventQuery::default()
            },
        )
        .is_empty()
    });
    let clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert_eq!(client.send(&clear).state(), RuntimeReceiptState::Completed);

    let deferred = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::MonitorRecoveryDeferred),
            ..EventQuery::default()
        },
    );
    let event = deferred.last().expect("recovery deferral");
    assert_eq!(event.links.lease_id(), Some(&token.lease_id()));
    let ProjectionPayload::Full(payload) = &event.payload else {
        panic!("expected full recovery deferral payload");
    };
    let EventPayload::Monitor(MonitorPayload::RecoveryDeferred(detail)) = payload.as_ref() else {
        panic!("expected full recovery deferral payload");
    };
    assert_eq!(detail.recovery(), MonitorRecoveryKind::WakeStandby);
    assert_eq!(
        detail.reason(),
        MonitorRecoveryCoordinationReason::ActiveLease
    );
    assert_eq!(detail.effect_disposition(), EffectDisposition::NotPerformed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    let release = client.request(RuntimeOperation::ReleaseLease {
        token: token.clone(),
    });
    assert_eq!(
        client.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn monitor_capture_failure_is_persisted_without_fake_success() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state
        .require_fenced_capture_close
        .store(true, Ordering::Release);
    state.fail_capture.store(true, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: RuntimeMonitorPolicy::new(500, "home", false).expect("monitor policy"),
    });
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    wait_until(Duration::from_secs(2), || {
        let status = client.send(&client.request(RuntimeOperation::MonitorStatus));
        matches!(
            status.result(),
            Some(RuntimeResult::MonitorStatus { status })
                if status.instances()[0].state().is_some_and(|state| {
                    state.run_count() >= 1
                        && state.last_error() == Some(RuntimeErrorCode::CaptureFailed)
                        && state.last_decision().is_none()
                })
        )
    });
    let clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert_eq!(client.send(&clear).state(), RuntimeReceiptState::Completed);
    assert!(
        !projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::MonitorProbeFailed),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::MonitorProbeCompleted),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::MonitorRecoveryAdmitted),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::MonitorRecoveryDeferred),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
    assert_eq!(
        state.unfenced_capture_close_count.load(Ordering::Acquire),
        0
    );
}

#[test]
fn runtime_restart_fails_when_monitor_evidence_is_missing() {
    let root = TempDir::new().expect("tempdir");
    let instance_id = instance_id();
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one("node.a", instance_id, Arc::clone(&state))),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: RuntimeMonitorPolicy::new(500, "home", false).expect("monitor policy"),
    });
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    wait_until(Duration::from_secs(2), || {
        state.monitor_observation_count.load(Ordering::Acquire) >= 1
    });
    let verified = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::ArtifactVerified),
            ..EventQuery::default()
        },
    );
    let object_key = verified
        .last()
        .and_then(|event| event.artifacts.first())
        .and_then(|artifact| artifact.object_key())
        .expect("monitor artifact object key")
        .to_string();
    let clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert_eq!(client.send(&clear).state(), RuntimeReceiptState::Completed);
    drop(client);
    host.close().expect("close host");
    fs::remove_file(root.path().join(object_key)).expect("remove monitor evidence");

    let restarted = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one("node.a", instance_id, state)),
    );
    let error = match restarted {
        Ok(host) => {
            host.close().expect("close unexpected host");
            panic!("missing monitor evidence must fail restart");
        }
        Err(error) => error,
    };
    assert_eq!(error.code(), "ledger_failure");
    assert!(error.is_fatal());
}

#[test]
fn invalid_monitor_provider_observation_poison_runtime_after_recording_failure() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state
        .require_fenced_capture_close
        .store(true, Ordering::Release);
    state.monitor_mode.store(usize::MAX, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: RuntimeMonitorPolicy::new(500, "home", false).expect("monitor policy"),
    });
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    drop(client);
    wait_until(Duration::from_secs(2), || {
        host.fatal_error()
            .expect("runtime health")
            .is_some_and(|error| error.code() == "monitor_observation_invalid")
    });
    assert_eq!(
        host.close()
            .expect_err("invalid observation must fail host")
            .code(),
        "monitor_observation_invalid"
    );
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
    assert_eq!(
        state.unfenced_capture_close_count.load(Ordering::Acquire),
        0
    );
}

#[test]
fn corrupt_monitor_registry_fails_runtime_startup() {
    let root = TempDir::new().expect("tempdir");
    fs::write(root.path().join(MONITOR_FILE_NAME), b"not-json\n")
        .expect("write monitor corruption");
    let result = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    );
    let error = match result {
        Ok(host) => {
            host.close().expect("close unexpected host");
            panic!("corrupt monitor registry must fail startup");
        }
        Err(error) => error,
    };
    assert_eq!(error.code(), "monitor_record_invalid");
    assert!(error.is_fatal());
}

#[test]
fn enabled_performance_monitor_collects_runtime_capture_pipeline_events() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root).with_performance_monitor(PerformanceMonitorConfig::default()),
        Arc::new(FakeProvider::one("node.alpha", instance_id(), state)),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let request = client.request(RuntimeOperation::ObserveReadonly {
        instance_alias: "node.alpha".to_owned(),
    });
    assert_eq!(
        client.send(&request).state(),
        RuntimeReceiptState::Completed
    );
    let observed_at_unix_ms = unix_ms_now().expect("wall clock");
    let context = host
        .performance_context_for_test("node.alpha", observed_at_unix_ms)
        .expect("performance context");
    assert!(context.max_capture_latency_ms.is_some());
    assert!(context.max_recognition_latency_ms.is_some());
    assert_eq!(context.max_action_effect_latency_ms, None);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}

#[test]
fn performance_stutter_is_ledger_visible_and_enriches_policy_failure() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_performance_monitor(PerformanceMonitorConfig::default()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    host.admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("policy admission");
    host.record_pipeline_performance(
        PipelinePerformanceSignal::new(POLICY_INSTANCE_ALIAS, POLICY_NOW_UNIX_MS + 90, 1_500)
            .expect("pipeline signal")
            .with_capture_latency(900)
            .expect("capture latency"),
    )
    .expect("record pipeline performance");

    clock.advance(100);
    let outcome = host
        .record_policy_dispatch_outcome(
            &intent.decision_id,
            &PolicyExecutionInput::Failed {
                error_code: "transient.capture".to_owned(),
                class: PolicyFailureClass::Recoverable,
            },
        )
        .expect("policy failure outcome");
    let PolicyExecutionOutcome::Failed { failure } = outcome.outcome else {
        panic!("expected failure")
    };
    assert_eq!(failure.perf_context.max_frame_gap_ms, Some(1_500));
    assert_eq!(failure.perf_context.max_capture_latency_ms, Some(900));
    assert!(!failure.perf_context.related_event_ids.is_empty());

    let mut client = TestClient::connect(&host);
    let events = projected_events(&mut client, EventQuery::default());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PerformanceStutterDetected)
            .count(),
        1
    );
    drop(client);
    host.close().expect("close host");
}
