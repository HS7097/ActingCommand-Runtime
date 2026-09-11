// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

// Task Contract: Workflow #257 / C1B9. Test class: specification criterion.
#[test]
fn task_teardown_precedes_terminal_and_lease_release() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("resource-close-order-task.zip");
    let bytes = neutral_contained_task_package();
    fs::write(&package, &bytes).expect("write package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = host_with_state(&root, "neutral.instance", Arc::clone(&state));
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

    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let resource_close = events
        .iter()
        .find(|event| event.event_type == EventType::RuntimeLifecycleObserved)
        .expect("resource close lifecycle");
    let task_terminal = events
        .iter()
        .find(|event| event.event_type == EventType::TaskCompleted)
        .expect("task terminal");
    let lease_release = events
        .iter()
        .find(|event| event.event_type == EventType::LeaseReleased)
        .expect("lease release");
    assert!(resource_close.sequence < task_terminal.sequence);
    assert!(task_terminal.sequence < lease_release.sequence);
    drop(client);
    host.close().expect("close host");
}

// Task Contract: Workflow #257 / READ-SESSION-CLOSE-v1. Test class: Defect regression.
// First red: Workflow #269 issuecomment-5571706024 (W32).
#[test]
fn readonly_sessions_close_through_real_resource_leases_without_input() {
    for mode in 0..3 {
        let root = TempDir::new().expect("tempdir");
        let state = Arc::new(FakeState::default());
        state
            .require_fenced_capture_close
            .store(true, Ordering::Release);
        state.fail_capture.store(mode == 1, Ordering::Release);
        state
            .transient_capture_failure
            .store(mode == 1, Ordering::Release);
        let host = host_with_state(&root, "node.a", Arc::clone(&state));
        let mut client = TestClient::connect(&host);
        let business_lease = (mode == 2).then(|| client.acquire("node.a").1);
        let observe = client.request(RuntimeOperation::ObserveReadonly {
            instance_alias: "node.a".into(),
        });
        let receipt = client.send(&observe);
        assert_eq!(
            receipt.state(),
            if mode == 1 {
                RuntimeReceiptState::Failed
            } else {
                RuntimeReceiptState::Completed
            }
        );
        if mode == 1 {
            let error = receipt.error_projection().expect("real capture failure");
            assert_eq!(error.code, RuntimeErrorCode::CaptureFailed);
            assert!(!error.fatal);
        }
        if let Some(token) = business_lease {
            let release = client.request(RuntimeOperation::ReleaseLease { token });
            assert_eq!(
                client.send(&release).state(),
                RuntimeReceiptState::Completed
            );
            assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
            let observe = client.request(RuntimeOperation::ObserveReadonly {
                instance_alias: "node.a".into(),
            });
            assert_eq!(
                client.send(&observe).state(),
                RuntimeReceiptState::Completed
            );
        }
        assert_eq!(state.open_count.load(Ordering::Acquire), 0);
        assert_eq!(state.input_count.load(Ordering::Acquire), 0);
        assert!(host.fatal_error().expect("health").is_none());
        drop(client);
        host.close().expect("owned read resources close normally");
        assert_eq!(
            state.capture_close_count.load(Ordering::Acquire),
            if mode == 2 { 2 } else { 1 }
        );
        assert_eq!(
            state.unfenced_capture_close_count.load(Ordering::Acquire),
            0
        );
        let ledger = GlobalLedger::open_evidence(
            actingcommand_ledger::GlobalLedgerEvidenceConfig::new(root.path()),
            |reference| {
                Some(
                    actingcommand_artifact_store::verify_projected_read_only(
                        root.path(),
                        reference,
                    )
                    .expect("verify original capture artifact"),
                )
            },
        )
        .expect("closed authoritative ledger");
        assert!(ledger.corrupt_tail().is_none());
        let events = ledger.query(&EventQuery::default());
        assert!(!events.iter().any(|event| matches!(
            event.event_type(),
            EventType::InputIntent | EventType::InputCommitted
        )));
        let grants = events
            .iter()
            .filter(|event| event.event_type() == EventType::LeaseGranted)
            .collect::<Vec<_>>();
        assert_eq!(grants.len(), if mode == 2 { 2 } else { 1 });
        for grant in grants {
            let released = events
                .iter()
                .find(|event| {
                    event.event_type() == EventType::LeaseReleased
                        && event.links().lease_id() == grant.links().lease_id()
                })
                .expect("real lease released");
            assert!(events.iter().any(|event| {
                grant.sequence() < event.sequence() && event.sequence() < released.sequence()
                    && matches!(event.payload(), EventPayload::Runtime(actingcommand_contract::RuntimePayload::LifecycleObserved(payload))
                        if matches!(payload.phase(), actingcommand_contract::RuntimeLifecyclePhase::ResourceQuiescence { quiescence: actingcommand_contract::ResourceQuiescence::Confirmed, .. }))
            }));
        }
    }
}

// Task Contract: Workflow #257 / C1B9. Test class: specification criterion.
#[test]
fn unconfirmed_teardown_retains_owner_handle_and_rejects_work() {
    // Workflow #269 NEMU-STDIO-FACTS-v1: failed metadata queries remain secondary
    // typed facts through the existing Device -> Kernel -> Host ledger path.
    let stdio_facts = Arc::new(actingcommand_device::VendorStdioFacts {
        process_id: 41,
        process_created_filetime: actingcommand_device::StdioFact::Unknown(
            actingcommand_device::StdioUnknown::QueryFailed(
                actingcommand_device::StdioNativeError::Win32 { code: 5 },
            ),
        ),
        started_filetime: 10,
        steps: vec![actingcommand_device::StdioStep {
            phase: actingcommand_device::StdioPhase::Close,
            api: actingcommand_device::StdioApi::Unlink,
            target: actingcommand_device::StdioReference::CaptureStdout,
            source: None,
            completed_filetime: 11,
            returned: -1,
            error: Some(actingcommand_device::StdioNativeError::Io { code: Some(32) }),
            before: None,
            after: None,
            related: None,
            target_retirement: None,
        }],
        dropped_count: 0,
        paths: Vec::new(),
        restart_manager: None,
    });
    for close_error in [
        DeviceError::fatal("injected unconfirmed capture close"),
        DeviceError::transient("injected unconfirmed capture close"),
    ] {
        let root = TempDir::new().expect("tempdir");
        let package = root.path().join("unconfirmed-resource-close-task.zip");
        let bytes = neutral_contained_task_package();
        fs::write(&package, &bytes).expect("write package");
        let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
        let state = Arc::new(FakeState::default());
        state
            .require_fenced_capture_close
            .store(true, Ordering::Release);
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        let close_error = close_error.with_resource_close_cause(
            actingcommand_device::DeviceResourceKind::CaptureBackend,
            actingcommand_device::DeviceResourceClosePhase::Close,
            "nemu_vendor_stdio",
            None,
            None,
            actingcommand_device::DeviceResourceQuiescence::Unconfirmed,
            1,
        );
        let occurrence = Arc::clone(close_error.resource_close_causes()[0].occurrence());
        let close_error = close_error.with_vendor_stdio_facts(Arc::clone(&stdio_facts));
        assert!(Arc::ptr_eq(
            &occurrence,
            close_error.resource_close_causes()[0].occurrence()
        ));
        assert_eq!(
            close_error.resource_close_causes()[0].vendor_stdio(),
            Some(stdio_facts.as_ref())
        );
        *state
            .capture_close_error
            .lock()
            .expect("capture close error") = Some(close_error);
        let host = host_with_state(&root, "neutral.instance", Arc::clone(&state));
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

        let failed = client.send(&request);

        assert_eq!(failed.state(), RuntimeReceiptState::Failed);
        assert!(failed.terminal().is_none());
        assert!(
            failed
                .error_projection()
                .expect("fatal close projection")
                .fatal
        );
        let fatal = host
            .fatal_error()
            .expect("runtime health")
            .expect("fatal state");
        assert_eq!(fatal.code(), "capture_backend_close_failed");
        assert_eq!(fatal.operation(), "close_execution_session");
        assert!(fatal.is_fatal());
        let shutdown = client.request(RuntimeOperation::RequestShutdown {
            target: host.runtime_info().shutdown_target(),
        });
        let shutdown = host
            .process_request_for_test(&shutdown, ConnectionId::new(177).expect("connection"))
            .expect("fatal wins shutdown");
        assert_eq!(shutdown.state(), RuntimeReceiptState::Failed);
        assert_eq!(shutdown.error_projection(), Some(fatal.projection()));
        assert!(shutdown.terminal().is_none());
        assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
        let events = host
            .query_persisted_events_for_test(EventQuery {
                correlation_id: Some(correlation_id),
                ..EventQuery::default()
            })
            .expect("query unconfirmed lifecycle");
        assert!(events.iter().any(|event| {
            event.event_type() == EventType::RuntimeFailed
                && serde_json::to_string(event.payload())
                    .expect("runtime failure JSON")
                    .contains("\"quiescence\":\"unconfirmed\"")
        }));
        let causes = events
            .iter()
            .filter_map(|event| {
                let EventPayload::Runtime(actingcommand_contract::RuntimePayload::Failed(outcome)) =
                    event.payload()
                else {
                    return None;
                };
                outcome
                    .lifecycle_failure()
                    .and_then(|failure| failure.cause())
                    .filter(|cause| cause.vendor_stdio().is_some())
            })
            .collect::<Vec<_>>();
        assert_eq!(causes.len(), 1, "same close occurrence is published once");
        let cause = causes[0];
        assert_eq!(
            cause.native_detail().expect("original failure").text(),
            "injected unconfirmed capture close"
        );
        let facts = cause.vendor_stdio().expect("typed owner facts");
        assert_eq!(facts.process_id, 41);
        assert_eq!(
            facts.process_created_filetime,
            actingcommand_contract::StdioFact::Unknown(
                actingcommand_contract::StdioUnknown::QueryFailed(
                    actingcommand_contract::StdioNativeError::Win32 { code: 5 }
                ),
            )
        );
        assert_eq!(facts.steps[0].returned, -1);
        assert_eq!(
            facts.steps[0].error,
            Some(actingcommand_contract::StdioNativeError::Io { code: Some(32) })
        );
        assert_eq!(facts.dropped_count, 0);
        assert!(!events.iter().any(|event| {
            serde_json::to_string(&event.payload().public_projection())
                .expect("public projection")
                .contains("vendor_stdio\":")
        }));
        assert!(!events.iter().any(|event| matches!(
            event.event_type(),
            EventType::TaskCompleted | EventType::LeaseReleased
        )));
        let status = client.request(RuntimeOperation::Status);
        let rejected = host
            .process_request_for_test(&status, ConnectionId::new(177).expect("connection"))
            .expect("fatal receipt");
        assert_eq!(rejected.state(), RuntimeReceiptState::Failed);
        assert!(rejected.terminal().is_none());

        let takeover = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                "neutral.instance",
                instance_id(),
                Arc::new(FakeState::default()),
            )),
        )
        .err()
        .expect("retained owner blocks takeover");
        assert_eq!(takeover.code(), "owner_conflict");
        drop(client);
        assert!(host.close().is_err());
        assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
        assert_eq!(
            state.unfenced_capture_close_count.load(Ordering::Acquire),
            0
        );
    }
}

// Task Contract: Workflow #257 / C1B7. Test class: specification criterion.
#[test]
fn required_failure_events_preserve_cleanup_detail() {
    use actingcommand_contract::{
        CleanupCauseSeverity, DiagnosticOutcomePayload, PublicEventPayload,
    };

    for capture in [false, true] {
        let mut baseline_types = None;
        let mut baseline_outcome = None;
        for cleanup_detail in [None, Some(false), Some(true)] {
            let root = TempDir::new().expect("tempdir");
            let state = Arc::new(FakeState::default());
            state
                .require_fenced_capture_close
                .store(capture, Ordering::Release);
            let host = host_with_state(&root, "node.a", Arc::clone(&state));
            let mut client = TestClient::connect(&host);
            let (_, token) = client.acquire("node.a");
            if capture {
                let prime = client.request(RuntimeOperation::Input {
                    token: token.clone(),
                    action: InputAction::Reset,
                });
                assert_eq!(client.send(&prime).state(), RuntimeReceiptState::Completed);
                state.fail_capture.store(true, Ordering::Release);
                state
                    .transient_capture_failure
                    .store(true, Ordering::Release);
            } else {
                *state.input_error.lock().expect("input error") = Some(
                    DeviceError::transient("primary bounded input context")
                        .with_diagnostic(DeviceErrorCategory::Protocol, "adb.input.operation")
                        .with_diagnostic_context(
                            "adb_shell_input",
                            "reset",
                            DeviceErrorSensitivity::Internal,
                        ),
                );
            }
            if let Some(has_detail) = cleanup_detail {
                let mut error = DeviceError::transient("unavailable private close text");
                if has_detail {
                    error = DeviceError::transient("exit_status=1 stderr=cleanup_failed")
                        .with_diagnostic(DeviceErrorCategory::CommandFlush, "maatouch.stdin.flush")
                        .with_diagnostic_context(
                            "maatouch",
                            "close",
                            DeviceErrorSensitivity::Secret,
                        );
                }
                *state.close_error.lock().expect("close error") =
                    Some(error.with_resource_close_cause(
                        actingcommand_device::DeviceResourceKind::InputBackend,
                        actingcommand_device::DeviceResourceClosePhase::Close,
                        "fake_input",
                        None,
                        None,
                        actingcommand_device::DeviceResourceQuiescence::Confirmed,
                        1,
                    ));
            }
            let correlation = client.ids.mint_correlation_id().expect("correlation");
            let correlation_id = *correlation.transport();
            let request = client.request_with_correlation(
                correlation,
                if capture {
                    RuntimeOperation::ObserveReadonly {
                        instance_alias: "node.a".to_owned(),
                    }
                } else {
                    RuntimeOperation::Input {
                        token,
                        action: InputAction::Reset,
                    }
                },
            );
            let receipt = client.send(&request);
            assert_eq!(receipt.state(), RuntimeReceiptState::Failed);
            assert_eq!(
                receipt.error_projection().expect("primary error").code,
                if capture {
                    RuntimeErrorCode::CaptureFailed
                } else {
                    RuntimeErrorCode::BackendOperationFailed
                }
            );
            assert!(!receipt.error_projection().expect("transient failure").fatal);
            let receipt_text = format!(
                "{} {receipt:?}",
                serde_json::to_string(&receipt).expect("receipt")
            );
            for private in [
                "primary bounded input context",
                "injected capture failure",
                "cleanup_failed",
                "unavailable private close text",
            ] {
                assert!(!receipt_text.contains(private));
            }
            let events = projected_events(
                &mut client,
                EventQuery {
                    correlation_id: Some(correlation_id),
                    ..EventQuery::default()
                },
            );
            let mut types = Vec::new();
            let mut resource_causes = 0;
            for event in &events {
                if let ProjectionPayload::Full(payload) = &event.payload
                    && let EventPayload::Runtime(
                        actingcommand_contract::RuntimePayload::LifecycleObserved(value),
                    ) = payload.as_ref()
                    && let actingcommand_contract::RuntimeLifecyclePhase::ResourceQuiescence {
                        quiescence,
                        ..
                    } = value.phase()
                {
                    assert_eq!(
                        quiescence,
                        actingcommand_contract::ResourceQuiescence::Confirmed
                    );
                    continue;
                }
                if let ProjectionPayload::Full(payload) = &event.payload
                    && let Some(budget) = payload.device_diagnostics()
                {
                    payload
                        .validate()
                        .expect("supplemental detail preserves admission");
                    assert!(
                        budget.emitted_count
                            <= actingcommand_contract::DEVICE_DIAGNOSTIC_DETAIL_LIMIT
                    );
                    assert!(
                        !serde_json::to_string(&payload.public_projection())
                            .unwrap()
                            .contains("cleanup_failed")
                    );
                    continue;
                }
                if let ProjectionPayload::Full(payload) = &event.payload
                    && let EventPayload::Runtime(actingcommand_contract::RuntimePayload::Failed(
                        failure,
                    )) = payload.as_ref()
                    && let Some(cause) = failure
                        .lifecycle_failure()
                        .and_then(|lifecycle| lifecycle.cause())
                    && cause.resource().is_some()
                {
                    assert_eq!(
                        cause.quiescence(),
                        Some(actingcommand_contract::ResourceQuiescence::Confirmed)
                    );
                    assert_eq!(
                        cause.owner_disposition(),
                        Some(actingcommand_contract::OwnerResourceDisposition::ConfirmedClosed)
                    );
                    resource_causes += 1;
                } else {
                    types.push(event.event_type);
                }
            }
            assert_eq!(resource_causes, usize::from(cleanup_detail.is_some()));
            if let Some(baseline) = &baseline_types {
                assert_eq!(&types, baseline);
            } else {
                baseline_types = Some(types);
            }
            let requested = if capture {
                EventType::CaptureRequested
            } else {
                EventType::InputIntent
            };
            let failed = if capture {
                EventType::CaptureFailed
            } else {
                EventType::InputFailed
            };
            let flow = events
                .iter()
                .filter(|event| event.event_type == requested || event.event_type == failed)
                .collect::<Vec<_>>();
            assert_eq!(
                flow.iter()
                    .map(|event| event.event_type)
                    .collect::<Vec<_>>(),
                [requested, failed]
            );
            assert_eq!(flow[0].links, flow[1].links);
            assert_eq!(flow[1].links.request_id(), Some(&request.request_id()));
            assert_eq!(flow[1].links.correlation_id(), Some(&correlation_id));
            if capture {
                assert!(flow[1].links.frame_id().is_some());
                let terminal = events
                    .iter()
                    .find(|event| event.event_type == EventType::RecognitionFailed)
                    .expect("recognition terminal");
                assert_eq!(
                    receipt.terminal().expect("terminal").sequence,
                    terminal.sequence
                );
            } else {
                assert!(flow[1].links.action_id().is_some());
                assert!(flow[1].links.lease_id().is_some());
                assert_eq!(
                    receipt.terminal().expect("terminal").sequence,
                    flow[1].sequence
                );
            }
            let ProjectionPayload::Full(payload) = &flow[1].payload else {
                panic!("full failure payload")
            };
            payload.validate().expect("validated failure payload");
            let outcome = match payload.as_ref() {
                EventPayload::Input(InputPayload::Failed(outcome))
                | EventPayload::Capture(CapturePayload::Failed(outcome)) => outcome,
                _ => panic!("input or capture failure"),
            };
            let primary = outcome.detail().expect("primary detail");
            assert_eq!(
                primary.stage(),
                if capture {
                    "device_registry.capture.operation"
                } else {
                    "adb.input.operation"
                }
            );
            assert_eq!(
                primary.backend(),
                if capture {
                    "nemu_ipc"
                } else {
                    "adb_shell_input"
                }
            );
            assert_eq!(
                primary.operation(),
                if capture { "capture" } else { "reset" }
            );
            assert_eq!(
                outcome.effect_disposition(),
                if capture {
                    EffectDisposition::NotPerformed
                } else {
                    EffectDisposition::Indeterminate
                }
            );
            assert_eq!(outcome.cleanup_cause().is_some(), cleanup_detail.is_some());
            if let Some(cause) = outcome.cleanup_cause() {
                assert_eq!(cause.code(), "input_backend_close_failed");
                assert_eq!(cause.severity(), CleanupCauseSeverity::Transient);
                assert_eq!(cause.detail().is_some(), cleanup_detail == Some(true));
                if let Some(detail) = cause.detail() {
                    assert_eq!(detail.category(), "command_flush");
                    assert_eq!(detail.stage(), "maatouch.stdin.flush");
                    assert_eq!(detail.backend(), "maatouch");
                    assert_eq!(detail.operation(), "close");
                    assert_eq!(detail.message(), "exit_status=1 stderr=cleanup_failed");
                    assert_eq!(detail.declared_sensitivity(), Sensitivity::Secret);
                    assert_eq!(flow[1].sensitivity, Sensitivity::Secret);
                }
            }
            let mut serialized = serde_json::to_value(outcome).expect("outcome serialization");
            let decoded: DiagnosticOutcomePayload =
                serde_json::from_value(serialized.clone()).expect("typed reader");
            assert_eq!(&decoded, outcome);
            if cleanup_detail.is_none() {
                assert!(serialized.get("cleanup_cause").is_none());
                baseline_outcome = Some(serialized);
            } else {
                serialized
                    .as_object_mut()
                    .expect("outcome object")
                    .remove("cleanup_cause");
                assert_eq!(Some(&serialized), baseline_outcome.as_ref());
            }
            assert!(!format!("{outcome:?}").contains("cleanup_failed"));
            let normal = client.request(RuntimeOperation::QueryEvents {
                query: EventQuery {
                    correlation_id: Some(correlation_id),
                    ..EventQuery::default()
                },
                profile: ProjectionProfile::Normal,
                page: RuntimeEventQueryPageRequest::new(128, None).expect("normal page"),
            });
            let normal_receipt = client.send(&normal);
            let RuntimeResult::EventPage { page } = normal_receipt.result().expect("normal events")
            else {
                panic!("normal page")
            };
            let normal_failure = page
                .events()
                .iter()
                .find(|event| event.event_type == failed)
                .expect("normal failure");
            let ProjectionPayload::Public(public) = &normal_failure.payload else {
                panic!("normal public projection")
            };
            let public = match public.as_ref() {
                PublicEventPayload::Input(value) | PublicEventPayload::Capture(value) => value,
                _ => panic!("normal input/capture"),
            };
            assert_eq!(public.cleanup_cause().is_some(), cleanup_detail.is_some());
            if let Some(cause) = public.cleanup_cause() {
                assert_eq!(cause.code(), "input_backend_close_failed");
                assert_eq!(cause.severity(), CleanupCauseSeverity::Transient);
                assert!(cause.detail().is_none());
            }
            let normal_json = serde_json::to_string(&normal_receipt).expect("normal JSON");
            for private in [
                "primary bounded input context",
                "injected capture failure",
                "cleanup_failed",
                "unavailable private close text",
            ] {
                assert!(!normal_json.contains(private));
            }
            assert_eq!(state.open_count.load(Ordering::Acquire), 1);
            assert_eq!(state.input_actions.lock().expect("input calls").len(), 1);
            assert_eq!(
                state.input_count.load(Ordering::Acquire),
                usize::from(capture)
            );
            assert_eq!(state.close_count.load(Ordering::Acquire), 1);
            assert_eq!(
                state.capture_open_count.load(Ordering::Acquire),
                usize::from(capture)
            );
            assert_eq!(
                state.capture_count.load(Ordering::Acquire),
                usize::from(capture)
            );
            assert_eq!(
                state.capture_close_count.load(Ordering::Acquire),
                usize::from(capture)
            );
            assert!(host.fatal_error().expect("runtime health").is_none());
            drop(client);
            host.close().expect("close host");
        }
    }
}
