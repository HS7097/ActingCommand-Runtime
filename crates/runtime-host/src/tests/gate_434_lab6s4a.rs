// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S4a (0.12.63; register §1.287, "Slice LAB-6S4a"; contracts/runtime-interface.md,
// "Old Runtimes"): the five request kinds are gone from the contract. A request of one of them in
// the complete shape 0.12.61 decoded (its tag `install_transition`, `declare_governance_identity`,
// `record_approval_decision`, `upgrade_state` or `upgrade_state_status`, with every member that
// release required; `RuntimeOperation`, `InstallTransitionAction` and `UpgradeStateAction` of
// v0.12.61) now carries an unknown operation tag, and that alone keeps it from decoding: the
// Runtime writes no receipt, drops the connection and records the connection's failure once,
// `runtime_protocol_invalid` at the request's decoding (`runtime_request_decode_failed`), as for
// every request it cannot decode. Nothing else is recorded for the request, the Runtime latches
// nothing, does not stop, and a new connection's `Status` is served. No clock is read: every
// request carries a fixed submission time.

use super::*;
use actingcommand_contract::{
    GovernanceIdentityCard, RequestId, RuntimePayload, RuntimeShutdownTarget,
};
use serde_json::{Value, json};

const ALIAS: &str = "node.a";
/// The code of a connection failure the Runtime records for a request it cannot decode.
const PROTOCOL_INVALID: &str = "runtime_protocol_invalid";
/// The transition the installer's and the upgrade state's requests name.
const TRANSITION_ID: &str = "transition-lab6s4a";

/// A request with the command line's envelope and a fixed submission time.
fn request(ids: &IdentifierIssuer, operation: RuntimeOperation) -> RuntimeRequest {
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        1,
        operation,
    )
    .expect("runtime request")
}

/// The operation of each deleted kind, with its tag, in the complete shape v0.12.61 decoded and
/// validated, for the Runtime `target`: `InstallTransition{target, action}` with the query action,
/// `DeclareGovernanceIdentity{card}`, `RecordApprovalDecision{decision}`,
/// `UpgradeState{target, action}` with `enter`, and `UpgradeStateStatus{transition_id}`. The card,
/// the decision and the target are today's types, which the slice did not change.
fn complete_operations(target: &RuntimeShutdownTarget) -> [(&'static str, Value); 5] {
    let card = GovernanceIdentityCard {
        client: "actingctl".to_owned(),
        client_version: Some("0.12.61".to_owned()),
        instance: None,
    };
    let decision = ApprovalDecisionRecord::new(
        "approval:lab6s4a",
        ApprovalDisposition::Approved,
        ApprovalTarget::Catalog {
            catalog_hash: format!("sha256:{}", "f".repeat(64)),
            catalog_version: 1,
        },
        actingcommand_contract::ApprovalReasonCode::new("user_confirmed").expect("approval reason"),
    )
    .expect("approval decision");
    [
        (
            "install_transition",
            json!({
                "operation": "install_transition",
                "target": target,
                "action": {"action": "query", "transition_id": TRANSITION_ID}
            }),
        ),
        (
            "declare_governance_identity",
            json!({"operation": "declare_governance_identity", "card": card}),
        ),
        (
            "record_approval_decision",
            json!({"operation": "record_approval_decision", "decision": decision}),
        ),
        (
            "upgrade_state",
            json!({
                "operation": "upgrade_state",
                "target": target,
                "action": {"action": "enter", "transition_id": TRANSITION_ID, "timeout_ms": 60_000}
            }),
        ),
        (
            "upgrade_state_status",
            json!({"operation": "upgrade_state_status", "transition_id": TRANSITION_ID}),
        ),
    ]
}

/// A request as a peer of 0.12.61 writes `operation`: a valid request of today's envelope whose
/// operation is replaced, with the request's own identities.
fn complete_request(
    ids: &IdentifierIssuer,
    operation: &Value,
) -> (Value, RequestId, CorrelationId) {
    let base = request(ids, RuntimeOperation::Health);
    let mut wire = serde_json::to_value(&base).expect("request JSON");
    wire["operation"] = operation.clone();
    (wire, base.request_id(), base.correlation_id())
}

/// The sequence of the ledger's last record.
fn head(host: &RuntimeHost) -> u64 {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
        .last()
        .map_or(0, PersistedEvent::sequence)
}

/// The records after `after`.
fn records_after(host: &RuntimeHost, after: u64) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
        .into_iter()
        .filter(|event| event.sequence() > after)
        .collect()
}

/// The `runtime.failed` records among `records` whose outcome is a protocol failure, each with its
/// operation.
fn protocol_failures(records: &[PersistedEvent]) -> Vec<(String, Value)> {
    records
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.outcome().code().as_str() == PROTOCOL_INVALID =>
            {
                Some((
                    PROTOCOL_INVALID.to_owned(),
                    record
                        .outcome()
                        .value("operation")
                        .cloned()
                        .unwrap_or(Value::Null),
                ))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn lab6s4a_an_unknown_operation_tag_is_refused_undecoded_and_the_runtime_goes_on() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    let operations = complete_operations(&host.runtime_info().shutdown_target());

    // The contract decodes none of the five, and only for their tag: each body is complete, so
    // the one thing serde refuses is the unknown variant.
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    for (tag, operation) in &operations {
        let (wire, _, _) = complete_request(&ids, operation);
        let error = serde_json::from_value::<RuntimeRequest>(wire)
            .expect_err("a request of a deleted kind decodes")
            .to_string();
        assert!(
            error.contains(&format!("unknown variant `{tag}`")),
            "{tag}: {error}"
        );
    }

    let before = head(&host);
    let mut sent = Vec::new();
    for (tag, operation) in &operations {
        let mut client = TestClient::connect(&host);
        // A connection that is answered, then the request of a deleted kind.
        let health = request(&client.ids, RuntimeOperation::Health);
        assert_eq!(
            client.send(&health).state(),
            RuntimeReceiptState::Completed,
            "{tag}"
        );
        let (wire, request_id, correlation_id) = complete_request(&client.ids, operation);
        sent.push((*tag, request_id, correlation_id));
        write_frame(&mut client.stream, &wire, DEFAULT_RUNTIME_MAX_FRAME_BYTES)
            .expect("write the request frame");
        client.set_receipt_read_timeout();
        match read_frame(&mut client.stream, DEFAULT_RUNTIME_MAX_FRAME_BYTES) {
            Ok(FrameRead::Closed) | Err(_) => {}
            Ok(FrameRead::Data(frame)) => panic!(
                "{tag}: the Runtime wrote a frame: {}",
                String::from_utf8_lossy(&frame)
            ),
            Ok(FrameRead::Idle) => panic!("{tag}: the Runtime kept the connection open"),
        }
    }
    let records = records_after(&host, before);
    // One connection failure per request, each at its decoding.
    let failure = (
        PROTOCOL_INVALID.to_owned(),
        json!("runtime_request_decode_failed"),
    );
    assert_eq!(protocol_failures(&records), vec![failure; operations.len()]);
    // Nothing is recorded for the requests themselves: no record names one of them.
    for (tag, request_id, correlation_id) in &sent {
        assert!(
            records.iter().all(|event| {
                event.links().request_id() != Some(request_id)
                    && event.links().correlation_id() != Some(correlation_id)
            }),
            "{tag}: a record names the request"
        );
    }
    // Nothing changed: no latch, no stop, and admission is open.
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "the Runtime latched"
    );
    assert!(
        !host.is_shutdown_requested().expect("runtime shutdown flag"),
        "the Runtime is stopping"
    );
    let mut client = TestClient::connect(&host);
    let status = request(&client.ids, RuntimeOperation::Status);
    assert_eq!(client.send(&status).state(), RuntimeReceiptState::Completed);
    drop(client);
    host.close().expect("close the Runtime");
}
