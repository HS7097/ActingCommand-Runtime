// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #378 A4 (model sections 7.1 and 7.2): every receipt the Runtime writes carries
//! `outcome`, written with the detail its request asks for.

use super::*;
use crate::codes::HostCode;
use actingcommand_contract::outcome::{Category, Detail};
use actingcommand_contract::{RuntimeErrorProjection, RuntimeOperationKind};
use serde_json::json;

/// A cancellation of a run the Runtime never saw: the host refuses it with its own code.
fn unknown_run_cancellation(ids: &IdentifierIssuer) -> RuntimeOperation {
    RuntimeOperation::CancelContainedTask {
        task_request_id: *ids.mint_request_id().expect("task request id").transport(),
    }
}

/// A completed receipt carries its request kind's success code; a dispatched refusal carries the
/// host's code on top, with the Runtime error code, the operation and the halt as values.
#[test]
fn a4_receipts_carry_the_success_code_or_the_refusal_with_its_runtime_code() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "node.a", Arc::new(FakeState::default()));
    let mut client = TestClient::connect(&host);
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    let status = client.request(RuntimeOperation::Status);
    let completed = client.send(&status);
    assert_eq!(completed.state(), RuntimeReceiptState::Completed);
    assert_eq!(
        Some(completed.outcome().code().as_str()),
        RuntimeOperationKind::Status
            .success_code()
            .map(|code| code.as_str())
    );
    assert_eq!(completed.outcome().category(), Category::Success);
    assert!(completed.outcome().values().is_empty());
    assert!(completed.error_projection().is_none());

    let cancel = client.request(unknown_run_cancellation(&ids));
    let denied = client.send(&cancel);
    assert_eq!(denied.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        denied.outcome().code().as_str(),
        HostCode::ContainedTaskCancellationIdentityMissing.as_str()
    );
    assert_eq!(denied.outcome().category(), Category::Error);
    assert_eq!(
        serde_json::Value::Object(denied.outcome().values().clone()),
        json!({
            "operation": "cancel_contained_task",
            "runtime_code": "invalid_request",
            "host_halted": false
        })
    );
    assert_eq!(
        denied.error_projection(),
        Some(RuntimeErrorProjection::new(
            RuntimeErrorCode::InvalidRequest,
            false
        ))
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close runtime");
}

/// A request the contract refuses is denied with the contract's refusal on top and
/// `invalid_request` as its Runtime error code (model section 7.2, L3).
#[test]
fn a4_a_request_the_contract_refuses_carries_the_refusal_code() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "node.a", Arc::new(FakeState::default()));
    let mut client = TestClient::connect(&host);

    // Instance discovery is the person's or the operator's; an agent adapter is refused.
    let discover = client.request(RuntimeOperation::DiscoverInstances);
    let mut wire = serde_json::to_value(&discover).expect("request JSON");
    wire["actor"] = json!("agent");
    wire["source"] = json!("adapter");
    let from_adapter: RuntimeRequest = serde_json::from_value(wire).expect("request v4 decodes");
    assert!(from_adapter.validate().is_err());

    let denied = client.send(&from_adapter);
    assert_eq!(denied.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        denied.outcome().code().as_str(),
        "invalid_emulator_control_origin"
    );
    assert_eq!(denied.outcome().category(), Category::Error);
    assert_eq!(
        serde_json::Value::Object(denied.outcome().values().clone()),
        json!({"runtime_code": "invalid_request", "host_halted": false})
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close runtime");
}

/// The console's requests (`source: ui`) get the outcome's registered names only, every other
/// source the full outcome, and a request's `detail` overrides its class default (model section
/// 7.1). Codes mode still names the Runtime error code, and the refusal's `host_halted` travels
/// in every detail mode (coordinator ruling A-M1).
#[test]
fn a4_the_receipt_outcome_leaves_with_the_detail_its_request_asks_for() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "node.a", Arc::new(FakeState::default()));
    let mut client = TestClient::connect(&host);
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    let requests = [
        (client.request(unknown_run_cancellation(&ids)), Detail::Full),
        (
            client.agent_request(unknown_run_cancellation(&ids)),
            Detail::Full,
        ),
        (
            client.governance_request(unknown_run_cancellation(&ids)),
            Detail::Codes,
        ),
        (
            client
                .governance_request(unknown_run_cancellation(&ids))
                .with_detail(Detail::Full),
            Detail::Full,
        ),
        (
            client
                .request(unknown_run_cancellation(&ids))
                .with_detail(Detail::Codes),
            Detail::Codes,
        ),
    ];
    for (request, detail) in requests {
        let receipt = client.send(&request);
        let source = request.source();
        assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{source:?}");
        assert_eq!(receipt.outcome_envelope().detail(), detail, "{source:?}");
        assert_eq!(
            receipt.outcome().code().as_str(),
            HostCode::ContainedTaskCancellationIdentityMissing.as_str(),
            "{source:?}"
        );
        let values = match detail {
            Detail::Codes => json!({
                "operation": "cancel_contained_task",
                "runtime_code": "invalid_request",
                "host_halted": false
            }),
            Detail::Full => json!({
                "operation": "cancel_contained_task",
                "runtime_code": "invalid_request",
                "host_halted": false
            }),
        };
        assert_eq!(
            serde_json::Value::Object(receipt.outcome().values().clone()),
            values,
            "{source:?} {detail:?}"
        );
        assert_eq!(
            receipt.error_projection().map(|projection| projection.code),
            Some(RuntimeErrorCode::InvalidRequest),
            "{source:?}"
        );
    }
    drop(client);
    host.close().expect("close runtime");
}

/// A failed material read carries the host's failure on top, with the values its code declares,
/// as every other refusal does (coordinator ruling A-M2): a frame deleted by hand reads
/// `missing`, and the receipt's outcome is `material_read_missing` naming the artifact and its
/// event, over `invalid_request`.
#[test]
fn a4_a_failed_material_read_carries_the_host_failure_on_top() {
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
    let frame_event = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::ArtifactVerified),
            ..EventQuery::default()
        },
    )
    .last()
    .expect("monitor frame event")
    .clone();
    let frame = frame_event
        .artifacts
        .first()
        .expect("monitor artifact")
        .clone();
    let object_key = frame
        .object_key()
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
        config(&root).with_frame_retention_enabled(false),
        Arc::new(FakeProvider::one("node.a", instance_id, state)),
    )
    .expect("restarted runtime host");
    let mut client = TestClient::connect(&restarted);
    let read = client.request(RuntimeOperation::ReadMaterial {
        request: Box::new(actingcommand_contract::RuntimeMaterialReadRequest {
            event: actingcommand_contract::LedgerEventPosition {
                event_id: frame_event.event_id,
                sequence: frame_event.sequence,
            },
            artifact_id: frame.artifact_id,
            snapshot_position: frame_event.sequence,
            byte_count: frame.byte_count,
            sha256: frame.sha256.clone(),
            expected_run_id: None,
            expected_frame_id: None,
            expected_request_id: None,
            expected_correlation_id: None,
            offset: 0,
            requested_length: 1,
            max_reply_bytes: actingcommand_contract::MAX_RUNTIME_MATERIAL_REPLY_BYTES,
        }),
    });
    let receipt = client.send(&read);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    let Some(RuntimeResult::MaterialRead { result }) = receipt.result() else {
        panic!("material read result");
    };
    assert_eq!(
        result.state,
        actingcommand_contract::RuntimeMaterialReadState::Missing
    );
    assert_eq!(
        receipt.outcome().code().as_str(),
        HostCode::MaterialReadMissing.as_str()
    );
    assert_eq!(receipt.outcome().category(), Category::Warning);
    assert_eq!(
        serde_json::Value::Object(receipt.outcome().values().clone()),
        json!({
            "artifact_id": serde_json::to_value(frame.artifact_id).expect("artifact id"),
            "event_id": serde_json::to_value(frame_event.event_id).expect("event id"),
            "operation": "read_runtime_material",
            "runtime_code": "invalid_request",
            "host_halted": false
        })
    );
    assert_eq!(
        receipt.error_projection(),
        Some(RuntimeErrorProjection::new(
            RuntimeErrorCode::InvalidRequest,
            false
        ))
    );
    drop(client);
    assert!(restarted.fatal_error().expect("runtime health").is_none());
    restarted.close().expect("close restarted host");
}

/// Part B M1: a direct run that the kernel ends with a code whose catalog entry requires values
/// the kernel's task error cannot carry (`recognition_sample_deadline_exceeded`: `attempts`,
/// `timeout_ms`, `stage`) is relayed by its spelling, as the host relays every contained-task
/// failure of a direct run (`RunContainedTask`, `backend_operation_failed`). Its Failed receipt
/// carries that code on top, the outcome its record carries plus the receipt's values, builds
/// without a debug panic, and reads back from its IPC frame as written.
#[test]
fn a4_a_relayed_kernel_failure_tops_its_failed_receipt_without_its_values() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            "node.a",
            ids.mint_holder_id().expect("holder id"),
            ContainedTaskRequest::new(r"C:\fixture\contained-task.zip", "0".repeat(64))
                .expect("task request"),
        ),
    );
    let error = RuntimeHostError::relayed_request(
        actingcommand_execution_kernel::ExecutionKernelCode::RecognitionSampleDeadlineExceeded
            .as_str(),
        actingcommand_contract::codes::ContractLocation::RunContainedTask.as_str(),
        RuntimeErrorCode::BackendOperationFailed,
    );
    let task_terminal = TerminalEvent {
        sequence: 7,
        event_id: *ids.mint_event_id().expect("task terminal").transport(),
    };
    let receipt = RuntimeReceipt::failure(
        &request,
        RuntimeReceiptState::Failed,
        Some(task_terminal),
        error.receipt_outcome(),
    )
    .expect("failed receipt")
    .for_request_detail(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed);
    assert_eq!(
        receipt.outcome().code().as_str(),
        "recognition_sample_deadline_exceeded"
    );
    assert_eq!(receipt.outcome().category(), Category::Error);
    assert_eq!(
        serde_json::Value::Object(receipt.outcome().values().clone()),
        json!({
            "operation": "run_contained_task",
            "runtime_code": "backend_operation_failed",
            "host_halted": false
        })
    );
    assert_eq!(receipt.outcome().code(), error.outcome().code());
    let frame = serde_json::to_vec(&receipt).expect("receipt frame");
    let decoded: RuntimeReceipt = serde_json::from_slice(&frame).expect("receipt decodes");
    decoded.validate().expect("decoded receipt validates");
    assert_eq!(decoded, receipt);
    assert_eq!(
        decoded.error_projection(),
        Some(RuntimeErrorProjection::new(
            RuntimeErrorCode::BackendOperationFailed,
            false
        ))
    );
}
