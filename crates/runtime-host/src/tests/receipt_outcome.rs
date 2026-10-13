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
