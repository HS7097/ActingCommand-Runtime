// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn backend_failure_is_visible_and_revokes_the_guard() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.fail_input.store(true, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("node.a");
    let input = client.request(RuntimeOperation::Input {
        token,
        action: InputAction::Reset,
    });
    let receipt = client.send(&input);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed);
    assert_eq!(
        receipt.error_projection().expect("failure").code,
        RuntimeErrorCode::BackendOperationFailed
    );
    wait_until(Duration::from_secs(2), || {
        state.close_count.load(Ordering::Acquire) == 1
    });
    drop(client);
    assert!(host.fatal_error().expect("health").is_none());
    host.close().expect("close host");
}

// Workflow #269 INPUT-FAILURE-CLOSE-v1, authorized Defect regression.
// First reds: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5575761897
// and https://github.com/HS7097/ActingCommand-Workflow/issues/278#issuecomment-5575797919.
#[test]
fn input_failure_closes_retained_capture_for_direct_and_contained_clients() {
    use actingcommand_contract::{ResourceQuiescence, RuntimeLifecyclePhase};
    use actingcommand_device::{
        DeviceErrorDiagnosticMessage, DeviceResourceClosePhase, DeviceResourceKind,
        DeviceResourceQuiescence,
    };
    use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};

    for (contained, open_failure, close_failure, concurrent_observation) in [
        (false, true, false, false),
        (true, true, false, false),
        (false, false, false, false),
        (true, false, false, false),
        (false, true, true, false),
        (true, true, true, false),
        (false, false, true, false),
        (true, false, true, false),
        (false, false, false, true),
        (true, false, false, true),
    ] {
        let root = TempDir::new().expect("tempdir");
        let state = Arc::new(FakeState::default());
        state
            .require_fenced_capture_close
            .store(true, Ordering::Release);
        state
            .block_input
            .store(concurrent_observation, Ordering::Release);
        let primary_text =
            "child_operation=screen_size; source_error=private synthetic input failure";
        let primary = DeviceError::transient(primary_text)
            .with_diagnostic(DeviceErrorCategory::Protocol, "adb.input.bounds_validate")
            .with_diagnostic_context(
                "adb_shell_input",
                if open_failure {
                    "bounds_validate"
                } else if contained {
                    "tap"
                } else {
                    "reset"
                },
                DeviceErrorSensitivity::Sensitive,
            )
            .with_diagnostic_message(
                DeviceErrorDiagnosticMessage::AdbShellInputBoundsUnavailableOrInvalid,
            );
        if open_failure {
            *state.input_open_error.lock().expect("input open error") = Some(primary);
        } else {
            *state.input_error.lock().expect("input error") = Some(primary);
        }
        if close_failure {
            *state
                .capture_close_error
                .lock()
                .expect("capture close error") = Some(
                DeviceError::fatal("private synthetic disconnect failure")
                    .with_resource_close_cause(
                        DeviceResourceKind::ProviderConnection,
                        DeviceResourceClosePhase::DisconnectCall,
                        "fake_capture",
                        None,
                        None,
                        DeviceResourceQuiescence::Unconfirmed,
                        1,
                    ),
            );
        }
        let host = host_with_state(&root, "node.a", Arc::clone(&state));
        let client = RuntimeClient::connect(RuntimeClientConfig::new(
            root.path(),
            EventActor::Cli,
            EventSource::Cli,
        ))
        .expect("official RuntimeClient");
        let execute_input = || {
            if contained {
                let bytes = neutral_contained_task_package();
                let package = root.path().join("input-failure-task.zip");
                fs::write(&package, &bytes).expect("existing inline package");
                let expected =
                    actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
                client
                    .run_contained_task(
                        "node.a",
                        ContainedTaskRequest::new(package.display().to_string(), expected)
                            .expect("task request"),
                    )
                    .expect_err("contained input failure")
            } else {
                client
                    .observe_readonly("node.a")
                    .expect("retained capture before input");
                let token = client.acquire_lease("node.a").expect("business lease");
                client
                    .input(&token, InputAction::Reset)
                    .expect_err("ordinary input failure")
            }
        };
        let error = thread::scope(|scope| {
            let input = scope.spawn(execute_input);
            if concurrent_observation {
                // D1: https://github.com/HS7097/ActingCommand-Runtime/pull/340#pullrequestreview-5135506634
                let observer = RuntimeClient::connect(RuntimeClientConfig::new(
                    root.path(),
                    EventActor::Cli,
                    EventSource::Cli,
                ))
                .expect("independent ordinary observer");
                let deadline = Instant::now() + Duration::from_secs(5);
                while !state.input_started.load(Ordering::Acquire) && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(5));
                }
                let input_started = state.input_started.load(Ordering::Acquire);
                let observation = scope.spawn(move || observer.observe_readonly("node.a"));
                let mut observer_admitted = false;
                while input_started && Instant::now() < deadline {
                    let events = host
                        .query_persisted_events_for_test(EventQuery {
                            event_type: Some(EventType::CaptureRequested),
                            ..EventQuery::default()
                        })
                        .expect("actual observer admission");
                    observer_admitted = events.len() == 2;
                    if observer_admitted {
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                state.block_input.store(false, Ordering::Release);
                let result = observation.join().expect("observer thread");
                assert!(
                    input_started && observer_admitted,
                    "observer must wait behind active input"
                );
                let error = result.expect_err("observer sees retained close in progress");
                assert!(!error.is_fatal());
                assert_eq!(
                    error.projection().expect("visible capture refusal").code,
                    RuntimeErrorCode::CaptureFailed
                );
            }
            input.join().expect("input owner thread")
        });
        assert_eq!(
            error.projection().expect("original input failure").code,
            if open_failure {
                RuntimeErrorCode::BackendOpenFailed
            } else {
                RuntimeErrorCode::BackendOperationFailed
            }
        );
        assert_eq!(error.is_fatal(), close_failure);
        assert!(!format!("{error:?} {error}").contains(primary_text));
        assert_eq!(state.open_count.load(Ordering::Acquire), 1);
        assert_eq!(state.input_count.load(Ordering::Acquire), 0);
        assert_eq!(state.capture_count.load(Ordering::Acquire), 1);
        assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
        assert_eq!(
            state.unfenced_capture_close_count.load(Ordering::Acquire),
            0
        );
        assert_eq!(
            state.close_count.load(Ordering::Acquire),
            usize::from(!open_failure)
        );
        let events = host
            .query_persisted_events_for_test(EventQuery::default())
            .expect("original ledger");
        let inputs = events
            .iter()
            .filter(|event| event.event_type() == EventType::InputFailed)
            .collect::<Vec<_>>();
        assert_eq!(inputs.len(), 1);
        let failed = inputs[0];
        let EventPayload::Input(InputPayload::Failed(outcome)) = failed.payload() else {
            panic!("typed input failure")
        };
        let detail = outcome.detail().expect("primary detail");
        assert_eq!(detail.category(), "protocol");
        assert_eq!(detail.stage(), "adb.input.bounds_validate");
        assert_eq!(detail.backend(), "adb_shell_input");
        assert_eq!(outcome.cleanup_cause().is_some(), close_failure);
        assert_eq!(failed.links().run_id().is_some(), contained);
        assert!(
            !events
                .iter()
                .any(|event| event.event_type() == EventType::InputCommitted)
        );
        let native = events
            .iter()
            .find_map(|event| {
                if let EventPayload::Runtime(actingcommand_contract::RuntimePayload::Failed(
                    failure,
                )) = event.payload()
                {
                    let lifecycle = failure.lifecycle_failure()?;
                    (lifecycle.native_detail()?.text() == primary_text)
                        .then_some((event, lifecycle))
                } else {
                    None
                }
            })
            .expect("original private native input detail");
        assert_eq!(native.0.links(), failed.links());
        assert_eq!(native.1.primary_detail(), outcome.detail());
        assert!(!native.1.native_detail().unwrap().truncated());
        assert!(
            !serde_json::to_string(&native.0.payload().public_projection())
                .unwrap()
                .contains(primary_text)
        );
        let released = events
            .iter()
            .filter(|event| event.event_type() == EventType::LeaseReleased)
            .collect::<Vec<_>>();
        assert_eq!(released.len(), usize::from(!close_failure));
        let quiescence = events.iter().find(|event| matches!(event.payload(),
            EventPayload::Runtime(actingcommand_contract::RuntimePayload::LifecycleObserved(payload))
            if matches!(payload.phase(), RuntimeLifecyclePhase::ResourceQuiescence { quiescence: ResourceQuiescence::Confirmed, .. })));
        if close_failure {
            assert!(quiescence.is_none());
            assert!(host.fatal_error().unwrap().is_some());
        } else {
            let closed = quiescence.expect("real close completion");
            assert_eq!(closed.links().lease_id(), failed.links().lease_id());
            assert_eq!(released[0].links().lease_id(), failed.links().lease_id());
            assert!(closed.sequence() < released[0].sequence());
            assert!(host.fatal_error().unwrap().is_none());
        }
        let terminals = events.iter().filter(|event| matches!(event.payload(),
            EventPayload::Task(TaskPayload::Semantic(payload))
            if matches!(payload.fact(), TaskSemanticFact::TerminalCommitted { outcome: TaskOutcome::Failure, .. }))).collect::<Vec<_>>();
        assert_eq!(terminals.len(), usize::from(contained && !close_failure));
        if let Some(terminal) = terminals.first() {
            assert_eq!(terminal.links().run_id(), failed.links().run_id());
        }
        drop(client);
        assert_eq!(host.close().is_err(), close_failure);
        assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
    }
}

#[test]
fn input_failure_persists_adb_bounds_context() {
    use actingcommand_device::{
        AdbBoundsAction, AdbBoundsCoordinate, AdbInputBoundsContext, AdbInputConnectGeometry,
    };

    let context = AdbInputBoundsContext::new(
        AdbBoundsAction::Tap { x: 101, y: 50 },
        AdbBoundsCoordinate::PointX,
        (100, 200),
        Some(AdbInputConnectGeometry::new(720, 1280, 90)),
    );
    let mut baseline_types: Option<Vec<EventType>> = None;
    for include_context in [false, true] {
        let root = TempDir::new().expect("tempdir");
        let state = Arc::new(FakeState::default());
        let mut error = DeviceError::fatal("tap x 101 exceeds touch screen max 100");
        if include_context {
            error = error.with_adb_input_bounds_context_if_absent(context);
        }
        let expected_message = error
            .diagnostic_message()
            .unwrap_or(error.message())
            .to_owned();
        *state.input_error.lock().expect("input error") = Some(
            error
                .with_diagnostic(DeviceErrorCategory::Protocol, "adb.input.bounds_validate")
                .with_diagnostic_context(
                    "adb_shell_input",
                    "tap",
                    DeviceErrorSensitivity::Sensitive,
                ),
        );
        let host = host_with_state(&root, "node.a", Arc::clone(&state));
        let mut client = TestClient::connect(&host);
        let (_, token) = client.acquire("node.a");
        let correlation = client.ids.mint_correlation_id().expect("correlation");
        let correlation_id = *correlation.transport();
        let request = client.request_with_correlation(
            correlation,
            RuntimeOperation::Input {
                token,
                action: InputAction::Tap { x: 101, y: 50 },
            },
        );
        let receipt = client.send(&request);
        assert_eq!(receipt.state(), RuntimeReceiptState::Failed);
        assert_eq!(
            receipt.error_projection().expect("failure").code,
            RuntimeErrorCode::BackendOperationFailed
        );
        let events = projected_events(
            &mut client,
            EventQuery {
                correlation_id: Some(correlation_id),
                ..EventQuery::default()
            },
        );
        let types = events
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>();
        if let Some(baseline) = &baseline_types {
            let mut expected = baseline.clone();
            let position = expected.len();
            // DEVICE-DIAGNOSTIC-v1 first CI34148715017: the native cause and M4 detail are real facts.
            expected.splice(
                position..position,
                [
                    EventType::RuntimeFailed,
                    EventType::RuntimeLifecycleObserved,
                ],
            );
            assert_eq!(types, expected);
        } else {
            baseline_types = Some(types);
        }
        let flow = events
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type,
                    EventType::InputIntent | EventType::InputFailed
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            flow.iter()
                .map(|event| event.event_type)
                .collect::<Vec<_>>(),
            vec![EventType::InputIntent, EventType::InputFailed]
        );
        assert_eq!(flow[0].links, flow[1].links);
        assert_eq!(flow[1].links.request_id(), Some(&request.request_id()));
        assert_eq!(flow[1].links.correlation_id(), Some(&correlation_id));
        assert!(flow[1].links.action_id().is_some());
        assert!(flow[1].links.lease_id().is_some());
        assert_eq!(flow[1].sensitivity, Sensitivity::Sensitive);
        let ProjectionPayload::Full(payload) = &flow[1].payload else {
            panic!("full input failure")
        };
        let EventPayload::Input(InputPayload::Failed(outcome)) = payload.as_ref() else {
            panic!("input failure")
        };
        let native_events = events
            .iter()
            .filter(|event| event.event_type == EventType::RuntimeFailed)
            .collect::<Vec<_>>();
        assert_eq!(native_events.len(), usize::from(include_context));
        if include_context {
            let native_event = native_events[0];
            assert_eq!(native_event.links, flow[1].links);
            assert_eq!(native_event.sensitivity, Sensitivity::Sensitive);
            let ProjectionPayload::Full(native_payload) = &native_event.payload else {
                panic!("full native cause")
            };
            let EventPayload::Runtime(actingcommand_contract::RuntimePayload::Failed(
                native_outcome,
            )) = native_payload.as_ref()
            else {
                panic!("native failure")
            };
            let lifecycle = native_outcome.lifecycle_failure().expect("typed lifecycle");
            assert_eq!(lifecycle.entered_event_id(), Some(flow[1].event_id));
            assert_eq!(lifecycle.primary_detail(), outcome.detail());
            let native = lifecycle.native_detail().expect("original cause");
            assert_eq!(native.text(), "tap x 101 exceeds touch screen max 100");
            assert!(!native.truncated());
            let public = serde_json::to_string(&native_payload.public_projection())
                .expect("public native failure");
            assert!(!public.contains("tap x 101 exceeds touch screen max 100"));
        }
        let detail = outcome.detail().expect("bounds detail");
        assert_eq!(detail.message(), expected_message);
        assert_eq!(detail.category(), "protocol");
        assert_eq!(detail.stage(), "adb.input.bounds_validate");
        assert_eq!(detail.backend(), "adb_shell_input");
        assert_eq!(detail.operation(), "tap");
        assert_eq!(detail.declared_sensitivity(), Sensitivity::Sensitive);
        assert_eq!(state.open_count.load(Ordering::Acquire), 1);
        assert_eq!(state.input_count.load(Ordering::Acquire), 0);
        assert_eq!(state.close_count.load(Ordering::Acquire), 1);
        assert!(host.fatal_error().expect("health").is_none());
        drop(client);
        host.close().expect("close host");
    }
}

#[test]
fn input_failure_preserves_device_diagnostic_detail_in_global_ledger() {
    use actingcommand_device::{
        AdbRecoveryPath, AdbRecoveryPhase, AdbRecoveryStep, AdbRecoveryText, AdbTargetRecovery,
        AdbTransportState,
    };
    // Workflow #284: transport recovery remains visible if the subsequent action fails.
    let recovery = AdbTargetRecovery {
        endpoint: AdbRecoveryText {
            text: "private-recovery-target:5555".into(),
            truncated: false,
        },
        initial_error: AdbRecoveryText {
            text: "preserved original offline error".into(),
            truncated: false,
        },
        path: AdbRecoveryPath::TargetDisconnectConnect,
        budget_ms: 12000,
        steps: vec![AdbRecoveryStep {
            phase: AdbRecoveryPhase::Verify,
            attempt: 1,
            elapsed_ms: 1,
            command: None,
            error: None,
        }],
        final_state: AdbTransportState::Device,
        recovered: true,
        dropped_count: 0,
    };
    let transport_state = Arc::new(FakeState::default());
    *transport_state.adb_recovery.lock().unwrap() = Some(recovery.clone());
    transport_state.fail_input.store(true, Ordering::Release);
    let kernel = ExecutionKernel::new(Arc::new(FakeProvider::one(
        "node.a",
        instance_id(),
        Arc::clone(&transport_state),
    )));
    let kernel_error = kernel
        .input("node.a", InputAction::Reset)
        .expect_err("typed input failure");
    let detail = kernel_error
        .diagnostic_detail()
        .expect("kernel input detail");
    assert_eq!(detail.category(), "native");
    assert_eq!(detail.stage(), "device_registry.input.operation");
    assert_eq!(detail.backend(), "adb_shell_input");
    assert_eq!(detail.operation(), "reset");
    assert_eq!(detail.message(), "injected backend failure");
    assert_eq!(detail.declared_sensitivity(), Sensitivity::Sensitive);
    let runtime_error = RuntimeHostError::execution("execute_input_backend", &kernel_error);
    assert!(kernel_error.adb_recovery().is_some());
    assert_eq!(
        runtime_error.lifecycle.adb_recovery.as_deref(),
        kernel_error.adb_recovery()
    );
    assert_eq!(runtime_error.diagnostic_detail(), Some(detail));
    assert!(
        !format!("{kernel_error:?} {kernel_error} {runtime_error:?} {runtime_error}")
            .contains("injected backend failure")
    );
    let absent = kernel
        .input("missing", InputAction::Reset)
        .expect_err("missing producer detail");
    assert!(absent.diagnostic_detail().is_none());
    assert!(
        RuntimeHostError::execution("execute_input_backend", &absent)
            .diagnostic_detail()
            .is_none()
    );
    kernel.close().expect("close transport kernel");

    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.fail_input.store(true, Ordering::Release);
    *state.adb_recovery.lock().unwrap() = Some(recovery);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("node.a");
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::Input {
            token,
            action: InputAction::Reset,
        },
    );

    let receipt = client.send(&request);

    assert_eq!(receipt.state(), RuntimeReceiptState::Failed);
    assert_eq!(
        receipt
            .error_projection()
            .expect("coarse input failure")
            .code,
        RuntimeErrorCode::BackendOperationFailed
    );
    let public_receipt = serde_json::to_string(&receipt).expect("public receipt JSON");
    assert!(!public_receipt.contains("injected backend failure"));
    assert!(!format!("{receipt:?}").contains("injected backend failure"));
    let native = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("native warning");
    let warnings = native.iter().filter(|event| {
        matches!(event.payload(), EventPayload::Runtime(actingcommand_contract::RuntimePayload::LifecycleObserved(value))
            if value.adb_recovery().is_some())
    }).collect::<Vec<_>>();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].severity(), EventSeverity::Warning);
    let full = serde_json::to_string(warnings[0].payload()).unwrap();
    assert!(full.contains("preserved original offline error"));
    let public = serde_json::to_string(&warnings[0].payload().public_projection()).unwrap();
    assert!(!public.contains("private-recovery-target"));
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let input_events = events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type,
                EventType::InputIntent | EventType::InputFailed
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(input_events.len(), 2);
    assert_eq!(
        input_events
            .iter()
            .filter(|event| event.event_type == EventType::InputFailed)
            .count(),
        1
    );
    let failure = input_events
        .into_iter()
        .find(|event| event.event_type == EventType::InputFailed)
        .expect("one input failure event");
    assert_eq!(failure.origin.module(), OriginModule::DeviceProxy);
    assert_eq!(failure.sensitivity, Sensitivity::Sensitive);
    let ProjectionPayload::Full(payload) = &failure.payload else {
        panic!("full input failure payload")
    };
    let EventPayload::Input(InputPayload::Failed(outcome)) = payload.as_ref() else {
        panic!("typed input failure payload")
    };
    let detail = outcome.detail().expect("stored input diagnostic detail");
    assert_eq!(detail.category(), "native");
    assert_eq!(detail.stage(), "device_registry.input.operation");
    assert_eq!(detail.backend(), "adb_shell_input");
    assert_eq!(detail.operation(), "reset");
    assert_eq!(detail.message(), "injected backend failure");
    assert_eq!(detail.declared_sensitivity(), Sensitivity::Sensitive);
    wait_until(Duration::from_secs(2), || {
        state.close_count.load(Ordering::Acquire) == 1
    });
    drop(client);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
