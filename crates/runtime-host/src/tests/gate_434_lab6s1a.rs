// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S1a (register §1.287, "Slice LAB-6S1a"; contracts/lab-two-entries.md §2.2,
// §4.3; contracts/runtime-interface.md, "Binding"): the first part of the door's
// `gate_two_entries`. The door also takes `BindChannel{channel: lab, initiator}` with one of the
// five Lab words: the connection is bound, the answer echoes the word and every receipt of the
// connection carries it beside `channel`. Every binding without a word (`lab` alone, `lab` with
// `via`, `cli`, `mcp`, `ui`, and the binding a connection's first request makes) answers byte for
// byte as before: its `ChannelBound` and its receipts are held to the literal JSON below, the
// bytes the Runtime wrote before this slice. A word on `ui`, `cli` or `mcp`, a word with `via`,
// the word `UI` and a word outside the list are `invalid_bind_channel`, the connection stays
// unbound and a valid `BindChannel` on it still binds; a second `BindChannel` after a bound one is
// `channel_binding_exists`, as before.

use super::*;
use actingcommand_contract::{ChannelVia, DeclaredInitiator, Initiator, RuntimeChannel};
use serde_json::{Value, json};

const ALIAS: &str = "node.a";

const LAB_WORDS: [Initiator; 5] = [
    Initiator::LabCli,
    Initiator::LabMcp,
    Initiator::LabAuthoring,
    Initiator::LabRescue,
    Initiator::LabWatchdog,
];

fn bind_request(
    client: &TestClient,
    channel: RuntimeChannel,
    via: Option<ChannelVia>,
    initiator: Option<Initiator>,
) -> RuntimeRequest {
    client.request(RuntimeOperation::BindChannel {
        channel,
        via,
        initiator: initiator.map(DeclaredInitiator::from),
    })
}

// A `BindChannel` whose operation is `operation` as a peer writes it on the wire: a shape the
// contract refuses, so no client builds it.
fn wire_bind_request(client: &TestClient, operation: Value) -> RuntimeRequest {
    let request = bind_request(client, RuntimeChannel::Lab, None, None);
    let mut wire = serde_json::to_value(&request).expect("request JSON");
    wire["operation"] = operation;
    serde_json::from_value(wire).expect("the frame decodes")
}

// A request of `client`'s connection with the envelope `(actor, source)`.
fn request_as(
    client: &TestClient,
    actor: EventActor,
    source: EventSource,
    operation: RuntimeOperation,
) -> RuntimeRequest {
    RuntimeRequest::new(
        client.ids.mint_request_id().expect("request id"),
        client.ids.mint_correlation_id().expect("correlation id"),
        None,
        actor,
        source,
        1,
        operation,
    )
    .expect("runtime request")
}

// The receipt frame of `request`, as the bytes the Runtime wrote.
fn receipt_bytes(client: &mut TestClient, request: &RuntimeRequest) -> String {
    write_frame(&mut client.stream, request, DEFAULT_RUNTIME_MAX_FRAME_BYTES)
        .expect("write the request frame");
    match read_frame(&mut client.stream, DEFAULT_RUNTIME_MAX_FRAME_BYTES)
        .expect("read the receipt frame")
    {
        FrameRead::Data(frame) => String::from_utf8(frame).expect("a receipt is UTF-8"),
        FrameRead::Idle | FrameRead::Closed => panic!("no receipt frame"),
    }
}

// The literal bytes of a completed receipt of `request` before this slice: `result` and the
// connection's members (`members`) as the JSON text they are written as.
fn completed_receipt(request: &RuntimeRequest, code: &str, result: &str, members: &str) -> String {
    let request_id = serde_json::to_value(request.request_id())
        .expect("request id")
        .to_string();
    let correlation_id = serde_json::to_value(request.correlation_id())
        .expect("correlation id")
        .to_string();
    format!(
        r#"{{"schema_version":"actingcommand.runtime.receipt.v2","request_id":{request_id},"correlation_id":{correlation_id},"state":"completed","result":{result},"outcome":{{"schema_version":"actingcommand.outcome.v1","code":"{code}","category":"success","values":{{}}}},{members}}}"#
    )
}

// Whether `value` holds a member named `name` at any depth.
fn holds_member(value: &Value, name: &str) -> bool {
    match value {
        Value::Object(members) => members
            .iter()
            .any(|(key, member)| key == name || holds_member(member, name)),
        Value::Array(items) => items.iter().any(|item| holds_member(item, name)),
        _ => false,
    }
}

// `lab` with each of the five Lab words is bound, its answer echoes the word and every receipt
// of the connection carries it; the Status answer names no initiator; a second `BindChannel` is
// refused as before and the binding, word included, stays.
#[test]
fn lab6s1a_lab_with_each_lab_word_is_bound_echoed_and_carried_by_its_receipts() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    for word in LAB_WORDS {
        let mut client = TestClient::connect(&host);
        let health = client.request(RuntimeOperation::Health);
        assert_eq!(client.send(&health).initiator(), None, "{word:?}");

        let bind = bind_request(&client, RuntimeChannel::Lab, None, Some(word));
        let bound = client.send(&bind);
        assert_eq!(bound.state(), RuntimeReceiptState::Completed, "{word:?}");
        assert_eq!(
            bound.outcome().code().as_str(),
            "request_bind_channel_completed"
        );
        assert_eq!(
            bound.result(),
            Some(&RuntimeResult::ChannelBound {
                channel: RuntimeChannel::Lab,
                via: None,
                initiator: Some(word),
            })
        );
        assert_eq!(
            (bound.channel(), bound.via(), bound.initiator()),
            (Some(RuntimeChannel::Lab), None, Some(word))
        );

        // Workflow #434 LAB-6S2-M (rule R): a request from the Lab is receipted with the word; one
        // with the CLI's envelope, as `client.request` builds, with the channel the host records
        // for it and no word (`gate_434_lab6s2m`).
        for operation in [RuntimeOperation::Status, RuntimeOperation::Health] {
            let request = request_as(&client, EventActor::Lab, EventSource::Lab, operation);
            let receipt = client.send(&request);
            assert_eq!(receipt.state(), RuntimeReceiptState::Completed, "{word:?}");
            assert_eq!(
                (receipt.channel(), receipt.via(), receipt.initiator()),
                (Some(RuntimeChannel::Lab), None, Some(word))
            );
            let wire = serde_json::to_value(&receipt).expect("receipt JSON");
            assert_eq!(wire["initiator"], json!(word.as_str()));
            if let Some(RuntimeResult::Status { status }) = receipt.result() {
                let status = serde_json::to_value(status).expect("status JSON");
                assert!(!holds_member(&status, "initiator"), "Status names it");
            }
        }

        let again = bind_request(&client, RuntimeChannel::Cli, None, None);
        let again = client.send(&again);
        assert_eq!(again.state(), RuntimeReceiptState::Denied);
        assert_eq!(again.outcome().code().as_str(), "channel_binding_exists");
        assert_eq!(again.outcome().value("channel"), Some(&json!("lab")));
        assert_eq!(again.initiator(), Some(word));
    }
    host.close().expect("close host");
}

// Every binding without a word answers byte for byte as before: its `ChannelBound` and a later
// receipt on the connection (`Health`'s) are the literal bytes below; so is a receipt on a
// connection bound by its first request's envelope source.
#[test]
fn lab6s1a_a_binding_without_a_word_answers_byte_for_byte_as_before() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    let owner_epoch = serde_json::to_value(host.runtime_info().owner_epoch())
        .expect("owner epoch")
        .to_string();
    let health_result = format!(r#"{{"kind":"health","owner_epoch":{owner_epoch}}}"#);

    for (channel, via, bound, members) in [
        (
            RuntimeChannel::Lab,
            None,
            r#"{"kind":"channel_bound","channel":"lab"}"#,
            r#""channel":"lab""#,
        ),
        (
            RuntimeChannel::Lab,
            Some(ChannelVia::Mcp),
            r#"{"kind":"channel_bound","channel":"lab","via":"mcp"}"#,
            r#""channel":"lab","via":"mcp""#,
        ),
        (
            RuntimeChannel::Cli,
            None,
            r#"{"kind":"channel_bound","channel":"cli"}"#,
            r#""channel":"cli""#,
        ),
        (
            RuntimeChannel::Mcp,
            None,
            r#"{"kind":"channel_bound","channel":"mcp"}"#,
            r#""channel":"mcp""#,
        ),
        (
            RuntimeChannel::Ui,
            None,
            r#"{"kind":"channel_bound","channel":"ui"}"#,
            r#""channel":"ui""#,
        ),
    ] {
        let mut client = TestClient::connect(&host);
        let bind = bind_request(&client, channel, via, None);
        assert_eq!(
            receipt_bytes(&mut client, &bind),
            completed_receipt(&bind, "request_bind_channel_completed", bound, members),
            "{channel:?} {via:?}"
        );
        let health = client.request(RuntimeOperation::Health);
        assert_eq!(
            receipt_bytes(&mut client, &health),
            completed_receipt(&health, "request_health_completed", &health_result, members),
            "{channel:?} {via:?}"
        );
    }

    for (actor, source, members) in [
        (EventActor::User, EventSource::Ui, r#""channel":"ui""#),
        (EventActor::Lab, EventSource::Lab, r#""channel":"lab""#),
        (EventActor::Cli, EventSource::Cli, r#""channel":"cli""#),
    ] {
        let mut client = TestClient::connect(&host);
        let status = request_as(&client, actor, source, RuntimeOperation::Status);
        assert_eq!(
            client.send(&status).state(),
            RuntimeReceiptState::Completed,
            "{source:?}"
        );
        let health = client.request(RuntimeOperation::Health);
        assert_eq!(
            receipt_bytes(&mut client, &health),
            completed_receipt(&health, "request_health_completed", &health_result, members),
            "{source:?}"
        );
    }
    host.close().expect("close host");
}

// A word on `ui`, `cli` or `mcp`, a word with `via`, the word `UI` and a word outside the list
// are refused `invalid_bind_channel` before any binding; the connection stays unbound, so a valid
// `BindChannel` on it then binds, and a second one after it is `channel_binding_exists`.
#[test]
fn lab6s1a_a_refused_word_leaves_the_connection_unbound() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    for operation in [
        json!({"operation": "bind_channel", "channel": "ui", "initiator": "Lab-CLI"}),
        json!({"operation": "bind_channel", "channel": "ui", "initiator": "UI"}),
        json!({"operation": "bind_channel", "channel": "cli", "initiator": "Lab-CLI"}),
        json!({"operation": "bind_channel", "channel": "mcp", "initiator": "Lab-MCP"}),
        json!({"operation": "bind_channel", "channel": "lab", "via": "mcp", "initiator": "Lab-MCP"}),
        json!({"operation": "bind_channel", "channel": "lab", "initiator": "UI"}),
        json!({"operation": "bind_channel", "channel": "lab", "initiator": "lab-cli"}),
        json!({"operation": "bind_channel", "channel": "lab", "initiator": "Lab-Other"}),
    ] {
        let mut client = TestClient::connect(&host);
        let refused = wire_bind_request(&client, operation.clone());
        assert!(refused.validate().is_err(), "{operation}");
        let refused = client.send(&refused);
        assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{operation}");
        assert_eq!(
            refused.outcome().code().as_str(),
            "invalid_bind_channel",
            "{operation}"
        );
        assert_eq!(
            (refused.channel(), refused.via(), refused.initiator()),
            (None, None, None),
            "{operation}"
        );

        let bind = bind_request(&client, RuntimeChannel::Lab, None, Some(Initiator::LabCli));
        let bound = client.send(&bind);
        assert_eq!(bound.state(), RuntimeReceiptState::Completed, "{operation}");
        assert_eq!(
            bound.result(),
            Some(&RuntimeResult::ChannelBound {
                channel: RuntimeChannel::Lab,
                via: None,
                initiator: Some(Initiator::LabCli),
            }),
            "{operation}"
        );

        let again = bind_request(&client, RuntimeChannel::Lab, None, Some(Initiator::LabCli));
        let again = client.send(&again);
        assert_eq!(
            again.outcome().code().as_str(),
            "channel_binding_exists",
            "{operation}"
        );
        assert_eq!(again.outcome().value("channel"), Some(&json!("lab")));
    }
    host.close().expect("close host");
}
