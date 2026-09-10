// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn readonly_artifact_store_failure_is_fatal_without_fake_success() {
    // First red: Workflow #269, issuecomment-5576835769 (ARTIFACT-PERSIST-v2).
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    fs::write(root.path().join("artifacts"), b"blocks artifact directory")
        .expect("block artifact directory");
    let mut client = TestClient::connect(&host);
    let observe = client.request(RuntimeOperation::ObserveReadonly {
        instance_alias: "node.a".to_string(),
    });

    let failed = client.send(&observe);

    assert_eq!(failed.state(), RuntimeReceiptState::Failed);
    assert!(failed.result().is_none());
    let error = failed.error_projection().expect("fatal artifact error");
    assert!(error.fatal);
    assert_eq!(error.code, RuntimeErrorCode::RuntimeFatal);
    assert_eq!(
        host.fatal_error()
            .expect("runtime fatal state")
            .expect("fatal error")
            .code(),
        "artifact_directory_failed"
    );
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("native failure facts");
    let failures = events
        .iter()
        .filter(|event| event.event_type() == EventType::ArtifactStoreFailed)
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 1);
    assert!(failures[0].artifacts().is_empty());
    let payload = failures[0].payload();
    let failure = payload.artifact_failure().expect("typed persistence error");
    assert_eq!(
        failure.stage,
        actingcommand_contract::ArtifactFailureStage::BeforePublication
    );
    assert_eq!(failure.primary.code, "artifact_directory_failed");
    assert_eq!(failure.primary.operation, "store_artifact");
    assert!(!failure.primary.native_detail.text().is_empty());
    assert!(payload.sensitivity() >= Sensitivity::Sensitive);
    let public = serde_json::to_string(&payload.public_projection()).expect("public failure");
    assert!(!public.contains("native_detail"));
    assert!(!public.contains(failure.primary.native_detail.text()));
    let fatal = host.fatal_error().expect("health").expect("fatal");
    assert_eq!(fatal.operation(), "store_artifact");
    let native = fatal
        .lifecycle
        .native_detail
        .as_ref()
        .expect("Host native I/O");
    assert!(native.text().contains(failure.primary.native_detail.text()));
    assert!(!format!("{fatal} {fatal:?}").contains(failure.primary.native_detail.text()));
    let repeated = fatal
        .clone()
        .with_related_failure("diagnostic_cleanup", &fatal);
    assert_eq!(
        repeated.lifecycle.native_detail,
        fatal.lifecycle.native_detail
    );
    let cleanup_dir = root.path().join("diagnostic-cleanup");
    fs::create_dir(&cleanup_dir).expect("cleanup target");
    let cleanup_io = fs::remove_file(&cleanup_dir).expect_err("native cleanup failure");
    let cleanup =
        RuntimeHostError::artifact(actingcommand_artifact_store::ArtifactStoreError::fatal(
            "artifact_cleanup_failed",
            "cleanup_artifact_temp",
            cleanup_io.to_string(),
        ));
    let combined = fatal
        .clone()
        .with_related_failure("diagnostic_cleanup", &cleanup);
    assert_eq!(combined.code(), fatal.code());
    assert_eq!(combined.operation(), fatal.operation());
    let combined_native = combined
        .lifecycle
        .native_detail
        .as_ref()
        .expect("both causes");
    assert!(
        combined_native
            .text()
            .contains(failure.primary.native_detail.text())
    );
    assert!(combined_native.text().contains(&cleanup_io.to_string()));
    assert!(
        combined_native
            .text()
            .contains("diagnostic_cleanup artifact_cleanup_failed during cleanup_artifact_temp")
    );
    host.record_lifecycle_failure(
        RuntimeLifecycleFailureStage::OperationCleanup,
        RuntimeLifecycleFailure::Host(&combined),
    )
    .expect("native merged failure record");
    let recorded = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::RuntimeFailed),
            ..EventQuery::default()
        })
        .expect("native lifecycle failures");
    assert!(recorded.iter().any(|event| matches!(event.payload(), EventPayload::Runtime(actingcommand_contract::RuntimePayload::Failed(outcome)) if outcome.lifecycle_failure().and_then(|value| value.native_detail()) == Some(combined_native.as_ref()))));
    for event in recorded {
        assert!(event.artifacts().is_empty());
        let public = serde_json::to_string(&event.payload().public_projection())
            .expect("public merged failure");
        assert!(!public.contains(failure.primary.native_detail.text()));
        assert!(!public.contains(&cleanup_io.to_string()));
    }
    drop(client);
    assert_eq!(
        host.close()
            .expect_err("fatal host closes with failure")
            .code(),
        "artifact_directory_failed"
    );
}

#[test]
fn readonly_failures_are_visible_and_terminal_without_fake_success() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.fail_capture.store(true, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let observe = client.request_with_correlation(
        correlation,
        RuntimeOperation::ObserveReadonly {
            instance_alias: "node.a".to_string(),
        },
    );
    let failed = client.send(&observe);
    assert_eq!(failed.state(), RuntimeReceiptState::Failed);
    assert_eq!(
        failed.error_projection().expect("failure").code,
        RuntimeErrorCode::CaptureFailed
    );
    assert!(failed.result().is_none());
    let events = projected_events(&mut client, EventQuery {
        correlation_id: Some(correlation_id), ..EventQuery::default()
    }).into_iter().filter(|event| !matches!(
        &event.payload, ProjectionPayload::Full(payload) if payload.device_diagnostics().is_some()
    )).map(|event| event.event_type).collect::<Vec<_>>();
    assert_eq!(
        &events[events.len() - 2..],
        [EventType::CaptureFailed, EventType::RecognitionFailed]
    );
    assert_eq!(state.capture_open_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
    assert!(host.fatal_error().expect("runtime health").is_none());

    // Workflow #257 C-M4-v1: existing diagnostic specification across real appends.
    for _ in 1..17 {
        let request = client.request(RuntimeOperation::ObserveReadonly {
            instance_alias: "node.a".to_owned(),
        });
        assert_eq!(client.send(&request).state(), RuntimeReceiptState::Failed);
    }
    let observed = projected_events(&mut client, EventQuery::default());
    let sources = observed
        .iter()
        .filter(|event| event.event_type == EventType::CaptureFailed)
        .collect::<Vec<_>>();
    assert_eq!(
        sources.len(),
        17,
        "all required failures survive the supplemental budget"
    );
    let details = observed
        .iter()
        .filter_map(|event| {
            let ProjectionPayload::Full(payload) = &event.payload else {
                return None;
            };
            payload
                .device_diagnostics()
                .map(|budget| (event, payload, budget))
        })
        .collect::<Vec<_>>();
    assert_eq!(details.len(), 16);
    for (index, (event, payload, budget)) in details.iter().enumerate() {
        assert_eq!(budget.emitted_count as usize, index + 1);
        assert_eq!(budget.folded_count, 0);
        let first = budget.first.as_ref().expect("first source");
        let last = budget.last.as_ref().expect("last source");
        assert_eq!(first.source_event_id, sources[0].event_id);
        assert_eq!(last.source_event_id, sources[index].event_id);
        assert_eq!(last.source_sequence, sources[index].sequence);
        assert!(last.source_sequence < event.sequence);
        assert_eq!(
            last.detail.as_ref().expect("full detail").message(),
            "injected capture failure"
        );
        assert_eq!(event.links, sources[index].links);
        payload.validate().expect("valid supplemental payload");
        let public = serde_json::to_string(&payload.public_projection()).expect("public budget");
        assert!(!public.contains("injected capture failure"));
        let mut invalid = serde_json::to_value(payload).expect("budget JSON");
        invalid["payload"]["data"]["device_diagnostics"]["emitted_count"] = serde_json::json!(17);
        // Full typed admission remains authoritative; malformed transport cannot admit an over-budget record.
        let invalid = serde_json::from_value::<EventPayload>(invalid)
            .expect("structurally typed over-budget payload");
        assert!(invalid.validate().is_err());
    }
    let epoch = host.runtime_info().owner_epoch();
    drop(client);
    host.close().expect("close host");
    let ledger = GlobalLedger::open_evidence(
        actingcommand_ledger::GlobalLedgerEvidenceConfig::new(root.path()),
        |_| None,
    )
    .expect("read closed authoritative ledger");
    let closed = ledger.query(&EventQuery::default());
    let summaries = closed
        .iter()
        .filter_map(|event| {
            let EventPayload::Runtime(actingcommand_contract::RuntimePayload::LifecycleObserved(
                value,
            )) = event.payload()
            else {
                return None;
            };
            (value.phase()
                == actingcommand_contract::RuntimeLifecyclePhase::DeviceDiagnosticSummary)
                .then(|| (event, value.device_diagnostics().expect("summary budget")))
        })
        .collect::<Vec<_>>();
    let [(summary, budget)] = summaries.as_slice() else {
        panic!("one close summary");
    };
    assert_eq!(budget.owner_epoch, epoch);
    assert_eq!((budget.emitted_count, budget.folded_count), (16, 1));
    assert_eq!(
        budget.first.as_ref().unwrap().source_event_id,
        sources[0].event_id
    );
    assert_eq!(
        budget.last.as_ref().unwrap().source_event_id,
        sources[16].event_id
    );
    assert_eq!(
        budget.last.as_ref().unwrap().source_sequence,
        sources[16].sequence
    );
    assert_eq!(closed.last().unwrap().event_id(), summary.event_id());
    summary
        .payload()
        .validate()
        .expect("complete validated summary");
}

#[test]
fn capture_failure_persists_nemu_resolution_context() {
    use actingcommand_device::{
        MumuInstallSource, NemuConfiguredAdbClass, NemuResolutionContext, NemuResolutionCountKind,
        NemuResolutionReason,
    };

    let context = NemuResolutionContext::new(NemuResolutionReason::SharedAdbMultipleDllVersions)
        .with_count(NemuResolutionCountKind::DllVersions, 2, false)
        .with_source(MumuInstallSource::ConfiguredBackendPath)
        .with_provenance(Some(NemuConfiguredAdbClass::SharedMumu), false, false);
    let mut baseline_types: Option<Vec<EventType>> = None;
    for include_context in [false, true] {
        let root = TempDir::new().expect("tempdir");
        let state = Arc::new(FakeState::default());
        let mut error = DeviceError::fatal("original Nemu resolution error");
        if include_context {
            error = error.with_nemu_resolution_context_if_absent(context);
        }
        let expected_message = error
            .diagnostic_message()
            .unwrap_or(error.message())
            .to_owned();
        *state.capture_open_error.lock().expect("capture open error") = Some(
            error
                .with_diagnostic(DeviceErrorCategory::Protocol, "nemu.installation.resolve")
                .with_diagnostic_context(
                    "nemu_ipc",
                    "installation_resolve",
                    DeviceErrorSensitivity::Internal,
                ),
        );
        let host = host_with_state(&root, "node.a", Arc::clone(&state));
        let mut client = TestClient::connect(&host);
        let correlation = client.ids.mint_correlation_id().expect("correlation");
        let correlation_id = *correlation.transport();
        let request = client.request_with_correlation(
            correlation,
            RuntimeOperation::ObserveReadonly {
                instance_alias: "node.a".to_owned(),
            },
        );
        let receipt = client.send(&request);
        assert_eq!(receipt.state(), RuntimeReceiptState::Failed);
        assert_eq!(
            receipt.error_projection().expect("failure").code,
            RuntimeErrorCode::CaptureFailed
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
            let position = expected
                .iter()
                .position(|kind| *kind == EventType::RecognitionFailed)
                .expect("recognition terminal");
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
                    EventType::CaptureRequested
                        | EventType::RecognitionRequested
                        | EventType::CaptureFailed
                        | EventType::RecognitionFailed
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            flow.iter()
                .map(|event| event.event_type)
                .collect::<Vec<_>>(),
            vec![
                EventType::CaptureRequested,
                EventType::RecognitionRequested,
                EventType::CaptureFailed,
                EventType::RecognitionFailed,
            ]
        );
        for event in &flow {
            assert_eq!(event.links, flow[0].links);
            assert_eq!(event.links.request_id(), Some(&request.request_id()));
            assert_eq!(event.links.correlation_id(), Some(&correlation_id));
            assert!(event.links.frame_id().is_some());
            assert!(event.links.recognition_id().is_some());
        }
        let ProjectionPayload::Full(payload) = &flow[2].payload else {
            panic!("full failure payload")
        };
        let EventPayload::Capture(CapturePayload::Failed(outcome)) = payload.as_ref() else {
            panic!("capture failure")
        };
        let native_events = events
            .iter()
            .filter(|event| event.event_type == EventType::RuntimeFailed)
            .collect::<Vec<_>>();
        assert_eq!(native_events.len(), usize::from(include_context));
        if include_context {
            let native_event = native_events[0];
            assert_eq!(native_event.links, flow[2].links);
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
            assert_eq!(lifecycle.entered_event_id(), Some(flow[2].event_id));
            assert_eq!(lifecycle.primary_detail(), outcome.detail());
            let native = lifecycle.native_detail().expect("original cause");
            assert_eq!(native.text(), "original Nemu resolution error");
            assert!(!native.truncated());
            let public = serde_json::to_string(&native_payload.public_projection())
                .expect("public native failure");
            assert!(!public.contains("original Nemu resolution error"));
        }
        let detail = outcome.detail().expect("resolution detail");
        assert_eq!(detail.message(), expected_message);
        assert_eq!(detail.category(), "protocol");
        assert_eq!(detail.stage(), "nemu.installation.resolve");
        assert_eq!(detail.backend(), "nemu_ipc");
        assert_eq!(detail.operation(), "installation_resolve");
        assert_eq!(detail.declared_sensitivity(), Sensitivity::Internal);
        assert_eq!(state.capture_open_count.load(Ordering::Acquire), 1);
        assert_eq!(state.capture_count.load(Ordering::Acquire), 0);
        assert!(host.fatal_error().expect("health").is_none());
        drop(client);
        host.close().expect("close host");
    }
}

#[test]
fn capture_failure_preserves_device_diagnostic_detail_in_global_ledger() {
    let transport_state = Arc::new(FakeState::default());
    transport_state.fail_capture.store(true, Ordering::Release);
    let kernel = ExecutionKernel::new(Arc::new(FakeProvider::one(
        "node.a",
        instance_id(),
        Arc::clone(&transport_state),
    )));
    let kernel_error = kernel.capture("node.a").expect_err("typed capture failure");
    let detail = kernel_error
        .diagnostic_detail()
        .expect("kernel capture detail");
    assert_eq!(detail.category(), "native");
    assert_eq!(detail.stage(), "device_registry.capture.operation");
    assert_eq!(detail.backend(), "nemu_ipc");
    assert_eq!(detail.operation(), "capture");
    assert_eq!(detail.message(), "injected capture failure");
    assert_eq!(detail.declared_sensitivity(), Sensitivity::Sensitive);
    let runtime_error = RuntimeHostError::execution("execute_capture_backend", &kernel_error);
    assert_eq!(runtime_error.diagnostic_detail(), Some(detail));
    assert!(
        !format!("{kernel_error:?} {kernel_error} {runtime_error:?} {runtime_error}")
            .contains("injected capture failure")
    );
    kernel.close().expect("close transport kernel");

    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.fail_capture.store(true, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let observe = client.request_with_correlation(
        correlation,
        RuntimeOperation::ObserveReadonly {
            instance_alias: "node.a".to_string(),
        },
    );

    let receipt = client.send(&observe);

    assert_eq!(receipt.state(), RuntimeReceiptState::Failed);
    assert_eq!(
        receipt
            .error_projection()
            .expect("coarse capture failure")
            .code,
        RuntimeErrorCode::CaptureFailed
    );
    let public_receipt = serde_json::to_string(&receipt).expect("public receipt JSON");
    assert!(!public_receipt.contains("injected capture failure"));
    assert!(!format!("{receipt:?}").contains("injected capture failure"));
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let capture_flow = events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type,
                EventType::CaptureRequested
                    | EventType::RecognitionRequested
                    | EventType::CaptureFailed
                    | EventType::RecognitionFailed
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(capture_flow.len(), 4);
    assert_eq!(
        capture_flow
            .iter()
            .filter(|event| event.event_type == EventType::CaptureFailed)
            .count(),
        1
    );
    let failure = capture_flow
        .into_iter()
        .find(|event| event.event_type == EventType::CaptureFailed)
        .expect("one capture failure event");
    assert_eq!(failure.origin.module(), OriginModule::Capture);
    assert_eq!(failure.sensitivity, Sensitivity::Sensitive);
    let ProjectionPayload::Full(payload) = &failure.payload else {
        panic!("full capture failure payload")
    };
    let EventPayload::Capture(CapturePayload::Failed(outcome)) = payload.as_ref() else {
        panic!("typed capture failure payload")
    };
    let detail = outcome.detail().expect("stored capture diagnostic detail");
    assert_eq!(detail.category(), "native");
    assert_eq!(detail.stage(), "device_registry.capture.operation");
    assert_eq!(detail.backend(), "nemu_ipc");
    assert_eq!(detail.operation(), "capture");
    assert_eq!(detail.message(), "injected capture failure");
    assert_eq!(detail.declared_sensitivity(), Sensitivity::Sensitive);
    assert_eq!(state.capture_open_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}
