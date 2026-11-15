// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S4a (0.12.62; register §1.287, "Slice LAB-6S4a"; contracts/runtime-interface.md,
// "Old Runtimes"): the five request kinds are gone from the contract, so a request whose operation
// carries the tag of one of them, as 0.12.61 spelled it on the wire (`install_transition`,
// `declare_governance_identity`, `record_approval_decision`, `upgrade_state`,
// `upgrade_state_status`), carries an unknown operation tag. It does not decode: the Runtime
// writes no receipt, drops the connection and records the connection's failure once,
// `runtime_protocol_invalid` at the request's decoding (`runtime_request_decode_failed`), as for
// every request it cannot decode. The Runtime latches nothing, and a new connection is answered.
// No clock is read: every request carries a fixed submission time.

use super::*;
use actingcommand_contract::RuntimePayload;
use serde_json::{Value, json};

const ALIAS: &str = "node.a";
/// The code of a connection failure the Runtime records for a request it cannot decode.
const PROTOCOL_INVALID: &str = "runtime_protocol_invalid";
/// The operation tags of the five deleted kinds.
const DELETED_TAGS: [&str; 5] = [
    "install_transition",
    "declare_governance_identity",
    "record_approval_decision",
    "upgrade_state",
    "upgrade_state_status",
];

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

/// A request as a peer of 0.12.61 writes one of the deleted kinds: a valid request whose
/// operation is replaced by `{"operation": tag}`.
fn tagged(ids: &IdentifierIssuer, tag: &str) -> Value {
    let mut wire =
        serde_json::to_value(request(ids, RuntimeOperation::Health)).expect("request JSON");
    wire["operation"] = json!({"operation": tag});
    wire
}

/// The sequence of the ledger's last record.
fn head(host: &RuntimeHost) -> u64 {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
        .last()
        .map_or(0, PersistedEvent::sequence)
}

/// The `runtime.failed` records after `after` whose outcome is a protocol failure, each with its
/// operation.
fn protocol_failures_after(host: &RuntimeHost, after: u64) -> Vec<(String, Value)> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
        .iter()
        .filter(|event| event.sequence() > after)
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
    // The contract decodes none of the five tags.
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    for tag in DELETED_TAGS {
        assert!(
            serde_json::from_value::<RuntimeRequest>(tagged(&ids, tag)).is_err(),
            "{tag} decodes"
        );
    }

    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    let before = head(&host);
    for tag in DELETED_TAGS {
        let mut client = TestClient::connect(&host);
        // A connection that is answered, then the request of a deleted kind.
        let health = request(&client.ids, RuntimeOperation::Health);
        assert_eq!(
            client.send(&health).state(),
            RuntimeReceiptState::Completed,
            "{tag}"
        );
        write_frame(
            &mut client.stream,
            &tagged(&client.ids, tag),
            DEFAULT_RUNTIME_MAX_FRAME_BYTES,
        )
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
    // One connection failure per request, each at its decoding.
    let failure = (
        PROTOCOL_INVALID.to_owned(),
        json!("runtime_request_decode_failed"),
    );
    assert_eq!(
        protocol_failures_after(&host, before),
        vec![failure; DELETED_TAGS.len()]
    );
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "the Runtime latched"
    );
    // A new connection is answered.
    let mut client = TestClient::connect(&host);
    let health = request(&client.ids, RuntimeOperation::Health);
    assert_eq!(client.send(&health).state(), RuntimeReceiptState::Completed);
    drop(client);
    host.close().expect("close the Runtime");
}
