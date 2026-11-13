// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S1a (register §1.287, "Slice LAB-6S1a"; contracts/lab-two-entries.md §2.2,
// §4.1, §4.3; `runtime-client` 8): the initiator on the wire. `BindChannel`, `ChannelBound` and the
// receipt each round-trip with and without it; a message of revision 7's shape decodes, as one
// without it; and without it the member is absent from the serialization, so such a message is
// written as before.

use super::*;
use serde_json::{Value, json};

const WORD: Initiator = Initiator::LabCli;

fn request(ids: &IdentifierIssuer, operation: RuntimeOperation) -> RuntimeRequest {
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Lab,
        EventSource::Lab,
        1,
        operation,
    )
    .expect("runtime request")
}

fn bind(initiator: Option<Initiator>) -> RuntimeOperation {
    RuntimeOperation::BindChannel {
        channel: RuntimeChannel::Lab,
        via: None,
        initiator: initiator.map(DeclaredInitiator::from),
    }
}

fn wire(value: &impl Serialize) -> Value {
    serde_json::to_value(value).expect("JSON")
}

#[test]
fn lab6s1a_bind_channel_round_trips_with_and_without_the_initiator() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    for initiator in [None, Some(WORD)] {
        let sent = request(&ids, bind(initiator));
        let encoded = wire(&sent);
        assert_eq!(
            encoded["operation"].get("initiator"),
            initiator.map(|word| json!(word.as_str())).as_ref(),
            "{initiator:?}"
        );
        let decoded: RuntimeRequest = serde_json::from_value(encoded).expect("the request decodes");
        assert_eq!(decoded, sent);
        decoded.validate().expect("the request validates");
    }

    // Revision 7's shape: no member, read as a binding without an initiator, written back as it
    // was read.
    let old = json!({"operation": "bind_channel", "channel": "lab", "via": "mcp"});
    let decoded: RuntimeOperation =
        serde_json::from_value(old.clone()).expect("revision 7's BindChannel decodes");
    assert_eq!(
        decoded,
        RuntimeOperation::BindChannel {
            channel: RuntimeChannel::Lab,
            via: Some(ChannelVia::Mcp),
            initiator: None,
        }
    );
    assert_eq!(wire(&decoded), old);
}

#[test]
fn lab6s1a_channel_bound_round_trips_with_and_without_the_initiator() {
    for initiator in [None, Some(WORD)] {
        let result = RuntimeResult::ChannelBound {
            channel: RuntimeChannel::Lab,
            via: None,
            initiator,
        };
        let encoded = wire(&result);
        assert_eq!(
            encoded.get("initiator"),
            initiator.map(|word| json!(word.as_str())).as_ref(),
            "{initiator:?}"
        );
        let decoded: RuntimeResult = serde_json::from_value(encoded).expect("the answer decodes");
        assert_eq!(decoded, result);
    }

    let old = json!({"kind": "channel_bound", "channel": "ui"});
    let decoded: RuntimeResult =
        serde_json::from_value(old.clone()).expect("revision 7's ChannelBound decodes");
    assert_eq!(
        decoded,
        RuntimeResult::ChannelBound {
            channel: RuntimeChannel::Ui,
            via: None,
            initiator: None,
        }
    );
    assert_eq!(wire(&decoded), old);

    // The words are a closed vocabulary: an answer naming another does not decode.
    assert!(
        serde_json::from_value::<RuntimeResult>(
            json!({"kind": "channel_bound", "channel": "lab", "initiator": "Lab-Other"})
        )
        .is_err()
    );
}

#[test]
fn lab6s1a_a_receipt_round_trips_with_and_without_the_initiator() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    for initiator in [None, Some(WORD)] {
        let health = request(&ids, RuntimeOperation::Health);
        let binding = ChannelBinding {
            channel: RuntimeChannel::Lab,
            via: None,
            initiator,
        };
        let receipt = RuntimeReceipt::success(
            &health,
            RuntimeReceiptState::Completed,
            None,
            RuntimeResult::Health {
                owner_epoch: *ids.mint_owner_epoch().expect("owner epoch").transport(),
            },
        )
        .expect("completed receipt")
        .with_channel(Some(binding));
        assert_eq!(receipt.initiator(), initiator);
        let encoded = wire(&receipt);
        assert_eq!(encoded["channel"], json!("lab"));
        assert_eq!(
            encoded.get("initiator"),
            initiator.map(|word| json!(word.as_str())).as_ref(),
            "{initiator:?}"
        );
        let decoded: RuntimeReceipt = serde_json::from_value(encoded).expect("the receipt decodes");
        decoded.validate().expect("the receipt validates");
        assert_eq!(decoded, receipt);
    }

    // Revision 7's receipt: no member, read as one without an initiator, written back as it was
    // read.
    let bind_request = request(&ids, bind(None));
    let old = json!({
        "schema_version": "actingcommand.runtime.receipt.v2",
        "request_id": wire(&bind_request.request_id()),
        "correlation_id": wire(&bind_request.correlation_id()),
        "state": "completed",
        "result": {"kind": "channel_bound", "channel": "lab", "via": "mcp"},
        "outcome": {
            "schema_version": "actingcommand.outcome.v1",
            "code": "request_bind_channel_completed",
            "category": "success",
            "values": {}
        },
        "channel": "lab",
        "via": "mcp"
    });
    let decoded: RuntimeReceipt =
        serde_json::from_value(old.clone()).expect("revision 7's receipt decodes");
    decoded.validate().expect("the receipt validates");
    assert_eq!(
        (decoded.channel(), decoded.via(), decoded.initiator()),
        (Some(RuntimeChannel::Lab), Some(ChannelVia::Mcp), None)
    );
    assert_eq!(wire(&decoded), old);
}
