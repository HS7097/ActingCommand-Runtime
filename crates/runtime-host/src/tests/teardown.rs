// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

// Task Contract: Workflow #257 / C1B9, rewritten for Workflow #191 H (the device session
// belongs to the instance: a task end keeps it, the Host close closes it). Test class:
// specification criterion.
#[test]
fn task_end_keeps_instance_session_until_host_close() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("resource-close-order-task.zip");
    let bytes = neutral_contained_task_package(true);
    fs::write(&package, &bytes).expect("write package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = host_with_state(&root, "neutral.instance", Arc::clone(&state));
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

    let receipt = client.send(&request);

    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert_eq!(state.close_count.load(Ordering::Acquire), 0);
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 0);
    let events = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        })
        .expect("query task events");
    assert!(!events.iter().any(|event| matches!(
        event.payload(),
        EventPayload::Runtime(actingcommand_contract::RuntimePayload::LifecycleObserved(payload))
            if matches!(payload.phase(), actingcommand_contract::RuntimeLifecyclePhase::ResourceQuiescence { .. })
    )));
    let task_terminal = events
        .iter()
        .find(|event| event.event_type() == EventType::TaskCompleted)
        .expect("task terminal");
    let lease_release = events
        .iter()
        .find(|event| event.event_type() == EventType::LeaseReleased)
        .expect("lease release");
    assert!(task_terminal.sequence() < lease_release.sequence());
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
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
        let business_lease_id = business_lease.as_ref().map(|token| token.lease_id());
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
            assert_eq!(state.capture_close_count.load(Ordering::Acquire), 0);
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
        assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
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
            if grant.links().lease_id() == business_lease_id.as_ref() {
                continue;
            }
            assert!(events.iter().any(|event| {
                grant.sequence() < event.sequence() && event.sequence() < released.sequence()
                    && matches!(event.payload(), EventPayload::Runtime(actingcommand_contract::RuntimePayload::LifecycleObserved(payload))
                        if matches!(payload.phase(), actingcommand_contract::RuntimeLifecyclePhase::ResourceQuiescence { quiescence: actingcommand_contract::ResourceQuiescence::Confirmed, .. }))
            }));
        }
    }
}

// Task Contract: Workflow #257 / C1B9. Test class: specification criterion. Workflow #403
// (approved 2026-10-13): an unconfirmed close of the instance's own resources now fences that
// instance instead of retaining the owner and ending the Runtime; the name stays for gate 6.
#[test]
fn unconfirmed_teardown_retains_owner_handle_and_rejects_work() {
    // Workflow #342 D1 first red: issuecomment-5974667337. The formal preparation
    // failure closes through the same owner, including an empty backend set.
    for (input_open_failure, operation_unconfirmed, backend_close_failure) in [
        (false, false, false),
        (false, true, false),
        (true, false, false),
        (true, true, false),
        (false, false, true),
        (false, true, true),
    ] {
        use actingcommand_contract::{
            ResourceDisposition, ResourceDispositionScope, ResourceQuiescence,
        };
        use actingcommand_device::DeviceResourceQuiescence;
        let root = TempDir::new().expect("tempdir");
        let state = Arc::new(FakeState::default());
        state
            .require_fenced_capture_close
            .store(true, Ordering::Release);
        let primary = DeviceError::transient("preparation command failed").with_resource_summary(
            if operation_unconfirmed {
                DeviceResourceQuiescence::Unconfirmed
            } else {
                DeviceResourceQuiescence::Confirmed
            },
            3,
        );
        if input_open_failure {
            *state.input_open_error.lock().expect("input open error") = Some(primary);
        } else {
            *state.capture_open_error.lock().expect("capture open error") = Some(primary);
        }
        if backend_close_failure {
            *state.close_error.lock().expect("input close error") = Some(
                DeviceError::fatal("retained input close failed")
                    .with_resource_summary(DeviceResourceQuiescence::Unconfirmed, 1),
            );
        }
        let host = host_with_state(&root, "neutral.instance", Arc::clone(&state));
        let mut client = TestClient::connect(&host);
        let correlation = client.ids.mint_correlation_id().expect("correlation");
        let correlation_id = *correlation.transport();
        let request = client.request_with_correlation(
            correlation,
            RuntimeOperation::SelfCheckInstance {
                instance_alias: "neutral.instance".into(),
            },
        );
        let receipt = client.send(&request);
        let unconfirmed = operation_unconfirmed || backend_close_failure;
        // Workflow #403: an unconfirmed close fences the instance; the self-check still
        // completes, as a failed one.
        assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
        let RuntimeResult::InstanceSelfChecked { selfcheck, .. } = receipt.result().unwrap() else {
            panic!("formal self-check result");
        };
        assert!(!selfcheck.touch.ok);
        assert!(
            selfcheck.failure_code.is_some(),
            "a closed resource is not a successful open"
        );
        let events = host
            .query_persisted_events_for_test(EventQuery {
                correlation_id: Some(correlation_id),
                ..EventQuery::default()
            })
            .expect("preparation ledger");
        let failures = events
            .iter()
            .filter_map(|event| {
                let EventPayload::Runtime(actingcommand_contract::RuntimePayload::Failed(outcome)) =
                    event.payload()
                else {
                    return None;
                };
                outcome
                    .lifecycle_failure()
                    .filter(|failure| failure.resource_dispositions().is_some())
                    .map(|failure| (event, failure))
            })
            .collect::<Vec<_>>();
        assert_eq!(failures.len(), if unconfirmed { 2 } else { 1 });
        let operation = ResourceDisposition {
            scope: ResourceDispositionScope::OperationResources,
            resource_count: 3,
            quiescence: if operation_unconfirmed {
                ResourceQuiescence::Unconfirmed
            } else {
                ResourceQuiescence::Confirmed
            },
        };
        assert_eq!(
            failures[0].1.resource_dispositions(),
            Some([operation].as_slice())
        );
        let groups = [
            operation,
            ResourceDisposition {
                scope: ResourceDispositionScope::SessionBackends,
                resource_count: u16::from(!input_open_failure),
                quiescence: if backend_close_failure {
                    ResourceQuiescence::Unconfirmed
                } else {
                    ResourceQuiescence::Confirmed
                },
            },
        ];
        let closed = if unconfirmed {
            assert_eq!(
                failures[1].1.resource_dispositions(),
                Some(groups.as_slice())
            );
            assert_eq!(
                failures[1].1.entered_event_id(),
                Some(*failures[0].0.event_id())
            );
            assert_ne!(failures[0].0.event_id(), failures[1].0.event_id());
            failures[1].0
        } else {
            events
                .iter()
                .find(|event| {
                    let EventPayload::Runtime(
                        actingcommand_contract::RuntimePayload::LifecycleObserved(payload),
                    ) = event.payload()
                    else {
                        return false;
                    };
                    let actingcommand_contract::RuntimeLifecyclePhase::ResourceQuiescence {
                        resource_count,
                        resource_dispositions,
                        quiescence,
                        ..
                    } = payload.phase()
                    else {
                        return false;
                    };
                    assert_eq!(resource_count, 3 + u16::from(!input_open_failure));
                    assert_eq!(quiescence, ResourceQuiescence::Confirmed);
                    assert_eq!(resource_dispositions.as_deref(), Some(groups.as_slice()));
                    true
                })
                .expect("combined confirmed Close fact")
        };
        // The Runtime's preparation lease uses its own synthetic correlation.
        let prepared_instance_id = *closed.links().instance_id().expect("self-check instance");
        let lease_events = host
            .query_persisted_events_for_test(EventQuery {
                instance_id: Some(prepared_instance_id),
                ..EventQuery::default()
            })
            .expect("preparation lease ledger");
        // Workflow #403: the self-check's own key (the fence's ladder takes one of its own).
        let granted = lease_events
            .iter()
            .filter(|event| {
                event.event_type() == EventType::LeaseGranted
                    && event.links().request_id() == Some(&request.request_id())
            })
            .collect::<Vec<_>>();
        assert_eq!(granted.len(), 1);
        let lease_id = granted[0].links().lease_id().expect("preparation lease");
        assert!(granted[0].sequence() < failures[0].0.sequence());
        let released = lease_events
            .iter()
            .filter(|event| {
                event.event_type() == EventType::LeaseReleased
                    && event.links().lease_id() == Some(lease_id)
            })
            .collect::<Vec<_>>();
        // Workflow #403: the fence keeps the instance out of use, so the key is released.
        assert_eq!(released.len(), 1);
        if let Some(released) = released.first() {
            assert!(closed.sequence() < released.sequence());
        }
        assert!(host.fatal_error().expect("health").is_none());
        let fences = lease_events
            .iter()
            .filter(|event| {
                matches!(event.payload(),
                    EventPayload::Runtime(actingcommand_contract::RuntimePayload::Failed(record))
                        if record.outcome().code().as_str()
                            == crate::codes::HostCode::InstanceResourcesFenced.as_str())
            })
            .count();
        assert_eq!(fences, usize::from(unconfirmed));
        assert_eq!(state.capture_close_count.load(Ordering::Acquire), 0);
        assert_eq!(
            state.close_count.load(Ordering::Acquire),
            usize::from(!input_open_failure)
        );
        assert_eq!(
            state.unfenced_capture_close_count.load(Ordering::Acquire),
            0
        );
        drop(client);
        host.close().expect("close host");
        assert_eq!(state.capture_close_count.load(Ordering::Acquire), 0);
        assert_eq!(
            state.close_count.load(Ordering::Acquire),
            usize::from(!input_open_failure)
        );
    }

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
        let bytes = neutral_contained_task_package(true);
        fs::write(&package, &bytes).expect("write package");
        let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
        let state = Arc::new(FakeState::default());
        state.physical_task_geometry.store(true, Ordering::Release);
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
        client.set_receipt_read_timeout();
        let request = client.request(RuntimeOperation::run_contained_task(
            "neutral.instance",
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string(), expected)
                .expect("task request"),
        ));
        // Workflow #191 H: the task end keeps the session; the teardown is the operator's
        // explicit reconnect, which closes the kept session first.
        assert_eq!(
            client.send(&request).state(),
            RuntimeReceiptState::Completed
        );
        assert_eq!(state.capture_close_count.load(Ordering::Acquire), 0);
        let correlation = client.ids.mint_correlation_id().expect("correlation");
        let correlation_id = *correlation.transport();
        let request = client.request_with_correlation(
            correlation,
            RuntimeOperation::SelfCheckInstance {
                instance_alias: "neutral.instance".into(),
            },
        );

        let failed = client.send(&request);

        // Workflow #403: the unconfirmed close fences the instance; the self-check completes
        // as a failed one with the close's code and the Runtime keeps running.
        assert_eq!(failed.state(), RuntimeReceiptState::Completed);
        let Some(RuntimeResult::InstanceSelfChecked { selfcheck, .. }) = failed.result() else {
            panic!("formal self-check result: {failed:?}");
        };
        assert_eq!(
            selfcheck.failure_code.as_deref(),
            Some("capture_backend_close_failed")
        );
        assert!(host.fatal_error().expect("runtime health").is_none());
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
                    .map(|cause| (cause, outcome.outcome()))
            })
            .collect::<Vec<_>>();
        assert_eq!(causes.len(), 1, "same close occurrence is published once");
        let (cause, failure_outcome) = causes[0];
        // Ledger revision 3 (#378 A3): the cause's native text is `raw_text` on a foreign link of
        // the failure's outcome.
        assert!(
            outcome_with_raw_text(failure_outcome, "injected unconfirmed capture close").is_some(),
            "original failure: {failure_outcome:?}"
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
        assert!(
            !events
                .iter()
                .any(|event| event.event_type() == EventType::TaskCompleted)
        );
        // Workflow #403: the Runtime answers, and its status shows the instance's fence.
        let status = client.request(RuntimeOperation::Status);
        let answered = host
            .process_request_for_test(&status, ConnectionId::new(177).expect("connection"))
            .expect("status receipt");
        assert_eq!(answered.state(), RuntimeReceiptState::Completed);
        let Some(RuntimeResult::Status { status }) = answered.result() else {
            panic!("status result: {answered:?}");
        };
        let fence = status
            .instances()
            .iter()
            .find(|instance| instance.instance_alias() == "neutral.instance")
            .and_then(actingcommand_contract::RuntimeInstanceStatus::resource_fence)
            .expect("the instance's fence");
        assert_eq!(fence.cause.as_str(), "capture_backend_close_failed");

        let takeover = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                "neutral.instance",
                instance_id(),
                Arc::new(FakeState::default()),
            )),
        )
        .err()
        .expect("the running owner blocks takeover");
        assert_eq!(takeover.code(), "owner_conflict");
        drop(client);
        host.close().expect("close host");
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
    use actingcommand_contract::outcome::CauseRelation;
    use actingcommand_contract::{DiagnosticOutcomePayload, PublicEventPayload};

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
                    frame: None,
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
                        .with_diagnostic(
                            DeviceErrorCategory::Protocol,
                            actingcommand_device::codes::DeviceStage::AdbInputBoundsValidate
                                .location(),
                        )
                        .with_diagnostic_context(
                            "adb_shell_input",
                            actingcommand_device::codes::DeviceLocation::InputReset,
                            DeviceErrorSensitivity::Internal,
                        ),
                );
            }
            if let Some(has_detail) = cleanup_detail {
                let mut error = DeviceError::transient("unavailable private close text");
                if has_detail {
                    error = DeviceError::transient("exit_status=1 stderr=cleanup_failed")
                        .with_diagnostic(
                            DeviceErrorCategory::CommandFlush,
                            actingcommand_device::codes::DeviceStage::MaatouchStdinFlush.location(),
                        )
                        .with_diagnostic_context(
                            "maatouch",
                            actingcommand_device::codes::DeviceLocation::InputClose,
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
                        frame: None,
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
            let mut close_summaries = 0;
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
                    && payload.device_diagnostics().is_some()
                {
                    payload
                        .validate()
                        .expect("supplemental detail preserves admission");
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
                    && let Some(lifecycle) = failure.lifecycle_failure()
                    && let Some(groups) = lifecycle.resource_dispositions()
                {
                    assert!(lifecycle.cause().is_none());
                    // Ledger revision 3 (#378 A3): the stage is the outcome's registered location.
                    assert_eq!(
                        outcome_str(failure.outcome(), "stage"),
                        Some(crate::codes::HostLocation::RuntimeLifecycleSessionClose.as_str())
                    );
                    assert_eq!(
                        groups,
                        [actingcommand_contract::ResourceDisposition {
                            scope:
                                actingcommand_contract::ResourceDispositionScope::SessionBackends,
                            resource_count: 1 + u16::from(capture),
                            quiescence: actingcommand_contract::ResourceQuiescence::Confirmed,
                        }]
                    );
                    close_summaries += 1;
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
            assert_eq!(close_summaries, usize::from(cleanup_detail.is_some()));
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
                    "capture_frame_layout"
                } else {
                    "adb_input_bounds_validate"
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
                if capture {
                    "capture_sequence"
                } else {
                    "input_reset"
                }
            );
            assert_eq!(
                outcome.effect_disposition(),
                if capture {
                    EffectDisposition::NotPerformed
                } else {
                    EffectDisposition::Indeterminate
                }
            );
            // Ledger revision 3 (#378 A3): the failed cleanup is the outcome's `cleanup` link (its
            // code) and the record's `cleanup_detail` (where it arose); the cleanup's text is the
            // `cleanup` link's own foreign link (`raw_text`) with the sensitivity the device
            // declared, and the record keeps the highest of them.
            let cleanups = outcome_links(outcome.outcome(), CauseRelation::Cleanup);
            assert_eq!(cleanups.len(), usize::from(cleanup_detail.is_some()));
            if let [cleanup] = cleanups.as_slice() {
                assert_eq!(cleanup.code().as_str(), "input_backend_close_failed");
                assert_eq!(
                    outcome.cleanup_detail().is_some(),
                    cleanup_detail == Some(true)
                );
                if let Some(detail) = outcome.cleanup_detail() {
                    assert_eq!(detail.category(), "command_flush");
                    assert_eq!(detail.stage(), "maatouch_stdin_flush");
                    assert_eq!(detail.backend(), "maatouch");
                    assert_eq!(detail.operation(), "input_close");
                    // Acceptance H-L1: a Secret device text under the `cleanup` link stays in
                    // the record, classed Secret, and makes the record Secret.
                    let foreign = cleanup
                        .causes()
                        .iter()
                        .map(actingcommand_contract::outcome::Link::outcome)
                        .find(|link| link.value("raw_text").is_some())
                        .expect("the cleanup's foreign text");
                    assert_eq!(
                        foreign.value("raw_text"),
                        Some(&serde_json::json!("exit_status=1 stderr=cleanup_failed"))
                    );
                    assert_eq!(
                        foreign.value("raw_text_sensitivity"),
                        Some(&serde_json::json!("secret"))
                    );
                    assert_eq!(
                        flow[1].sensitivity,
                        actingcommand_contract::Sensitivity::Secret
                    );
                }
            } else {
                assert!(outcome.cleanup_detail().is_none());
            }
            let mut serialized = serde_json::to_value(outcome).expect("outcome serialization");
            let decoded: DiagnosticOutcomePayload =
                serde_json::from_value(serialized.clone()).expect("typed reader");
            assert_eq!(&decoded, outcome);
            if cleanup_detail.is_none() {
                assert!(serialized.get("cleanup_detail").is_none());
                baseline_outcome = Some(serialized);
            } else {
                // Without its `cleanup` link and `cleanup_detail` the record equals the
                // baseline's.
                let record = serialized.as_object_mut().expect("outcome object");
                record.remove("cleanup_detail");
                let failure = record["outcome"]
                    .as_object_mut()
                    .expect("the record's outcome");
                let causes = failure["causes"].as_array_mut().expect("outcome links");
                causes.retain(|link| link["relation"] != "cleanup");
                if causes.is_empty() {
                    failure.remove("causes");
                }
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
            // The public projection keeps the outcome codes only: the `cleanup` link's code and
            // no foreign text.
            let public_outcome = public.outcome().expect("public outcome");
            let public_cleanups = outcome_links(public_outcome, CauseRelation::Cleanup);
            assert_eq!(public_cleanups.len(), usize::from(cleanup_detail.is_some()));
            if let [cleanup] = public_cleanups.as_slice() {
                assert_eq!(cleanup.code().as_str(), "input_backend_close_failed");
            }
            assert!(outcome_raw_texts(public_outcome).is_empty());
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
