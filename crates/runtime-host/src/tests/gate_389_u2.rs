// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #389 U-2 (unified interface model v2.2.3 §3.2, §4.2, §5.1, §5.2, §8.2 TU-2, TU-3,
// TU-4, TU-6, TU-7, TU-9; contracts/runtime-interface.md, "Channels" and "Records"): the door
// binds each connection to one channel and marks every request, its receipt and the
// `command.received` the C4 path writes with it, and refuses nothing by channel or by Lab
// option before U-6. A connection that never sends BindChannel is bound from its first
// request's envelope source until U-6. actingd's configured approvals are written in process
// with the approval provenance (Governance, User, Ui).

use super::gate_383_ao4q::ledger_head;
use super::*;
use actingcommand_contract::{
    ChannelVia, CommandPayload, ConfigParameter, ConfigParameterSource, ConfigSettingChange,
    FactScalar, RuntimeChannel, RuntimeConfigManifest,
};
use actingcommand_runtime_config::{OVERRIDE_CHANNEL, STUCK_RECOVERY, instance_key};
use serde_json::{Value, json};

const ALIAS: &str = "node.a";

// A host with one instance whose `stuck_recovery` an instruction may change.
fn start_with_setting(root: &TempDir) -> (RuntimeHost, String) {
    let instance = instance_id();
    let Value::String(id) = serde_json::to_value(instance).expect("instance id encodes") else {
        panic!("an instance id is a string");
    };
    let key = instance_key(&id, STUCK_RECOVERY);
    let mut parameters = vec![ConfigParameter {
        key: key.clone(),
        value: FactScalar::Boolean(true),
        source: ConfigParameterSource::Default,
        settable: false,
    }];
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("the key is in the key table");
    let host = RuntimeHost::start(
        config(root).with_config_manifest(RuntimeConfigManifest {
            subsystems: Vec::new(),
            parameters,
        }),
        Arc::new(FakeProvider::one(
            ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    (host, key)
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

fn bind(
    client: &mut TestClient,
    channel: RuntimeChannel,
    via: Option<ChannelVia>,
) -> RuntimeReceipt {
    let request = client.request(RuntimeOperation::BindChannel { channel, via });
    client.send(&request)
}

fn set_stuck_recovery(key: &str) -> RuntimeOperation {
    RuntimeOperation::ConfigureSettings {
        changes: vec![ConfigSettingChange {
            key: key.to_owned(),
            value: Some(json!(false)),
        }],
    }
}

// The channel and `via` of the `command.received` of `request`.
fn received_channel(
    reader: &mut TestClient,
    request: &RuntimeRequest,
) -> (Option<RuntimeChannel>, Option<ChannelVia>) {
    let received = projected_events(
        reader,
        EventQuery {
            event_type: Some(EventType::CommandReceived),
            correlation_id: Some(request.correlation_id()),
            ..EventQuery::default()
        },
    );
    let [received] = received.as_slice() else {
        panic!("one command.received of the request: {received:?}");
    };
    let ProjectionPayload::Full(payload) = &received.payload else {
        panic!("a full projection");
    };
    let EventPayload::Command(CommandPayload::Received(observation)) = payload.as_ref() else {
        panic!("a command.received payload: {payload:?}");
    };
    (observation.channel(), observation.via())
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

// TU-2: a connection bound `cli` keeps `cli` for a request whose envelope is (Lab, Lab): its
// receipt, its command.received and its host.config_overrides row name `cli`. A second
// BindChannel is refused with channel_binding_exists naming the channel kept, and the binding
// stays. The Status answer names no channel and no connection.
#[test]
fn u2_a_bound_connection_marks_its_requests_with_its_channel_whatever_their_envelope() {
    let root = TempDir::new().expect("tempdir");
    let (host, key) = start_with_setting(&root);
    let mut client = TestClient::connect(&host);
    let mut reader = TestClient::connect(&host);

    let health = client.request(RuntimeOperation::Health);
    assert_eq!(client.send(&health).channel(), None);
    let bound = bind(&mut client, RuntimeChannel::Cli, None);
    assert_eq!(bound.state(), RuntimeReceiptState::Completed);
    assert_eq!(
        bound.outcome().code().as_str(),
        "request_bind_channel_completed"
    );
    assert_eq!(
        bound.result(),
        Some(&RuntimeResult::ChannelBound {
            channel: RuntimeChannel::Cli,
            via: None,
        })
    );
    assert_eq!(bound.channel(), Some(RuntimeChannel::Cli));

    let configure = request_as(
        &client,
        EventActor::Lab,
        EventSource::Lab,
        set_stuck_recovery(&key),
    );
    let receipt = client.send(&configure);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert_eq!(receipt.channel(), Some(RuntimeChannel::Cli));
    assert_eq!(receipt.via(), None);
    assert_eq!(
        received_channel(&mut reader, &configure),
        (Some(RuntimeChannel::Cli), None)
    );
    let overrides = {
        let request = reader.request(RuntimeOperation::RuntimeFactSnapshot);
        let receipt = reader.send(&request);
        let Some(RuntimeResult::RuntimeFactSnapshot { snapshot }) = receipt.result() else {
            panic!("the runtime fact snapshot: {receipt:?}");
        };
        let record = snapshot
            .records
            .iter()
            .find(|record| record.key == actingcommand_contract::CONFIG_OVERRIDES_FACT_KEY)
            .expect("host.config_overrides");
        let ContractFactValue::RecordList(rows) = &record.value else {
            panic!("host.config_overrides is a record list");
        };
        rows.clone()
    };
    assert_eq!(overrides.len(), 1);
    assert_eq!(
        overrides[0].get(OVERRIDE_CHANNEL),
        Some(&FactScalar::String("cli".to_owned()))
    );

    let again = bind(&mut client, RuntimeChannel::Lab, None);
    assert_eq!(again.state(), RuntimeReceiptState::Denied);
    assert_eq!(again.outcome().code().as_str(), "channel_binding_exists");
    assert_eq!(again.outcome().value("channel"), Some(&json!("cli")));
    assert_eq!(again.channel(), Some(RuntimeChannel::Cli));

    let status = client.request(RuntimeOperation::Status);
    let status = client.send(&status);
    assert_eq!(status.channel(), Some(RuntimeChannel::Cli));
    let Some(RuntimeResult::Status { status }) = status.result() else {
        panic!("the status: {status:?}");
    };
    let status = serde_json::to_value(status).expect("status encodes");
    for member in ["channel", "via", "connections", "connection_count"] {
        assert!(!holds_member(&status, member), "Status names {member}");
    }
    drop(client);
    drop(reader);
    host.close().expect("close host");
}

// TU-3: Health, BindChannel and a Status read append no record; a write records its channel.
#[test]
fn u2_the_handshake_and_a_status_read_append_nothing_and_a_write_names_its_channel() {
    let root = TempDir::new().expect("tempdir");
    let (host, key) = start_with_setting(&root);
    let mut client = TestClient::connect(&host);
    let mut reader = TestClient::connect(&host);

    let before = ledger_head(&mut reader);
    let health = client.request(RuntimeOperation::Health);
    assert_eq!(client.send(&health).state(), RuntimeReceiptState::Completed);
    let bound = bind(&mut client, RuntimeChannel::Mcp, None);
    assert_eq!(bound.state(), RuntimeReceiptState::Completed);
    let status = client.request(RuntimeOperation::Status);
    assert_eq!(client.send(&status).state(), RuntimeReceiptState::Completed);
    assert_eq!(ledger_head(&mut reader), before);

    let configure = client.request(set_stuck_recovery(&key));
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    assert_eq!(
        received_channel(&mut reader, &configure),
        (Some(RuntimeChannel::Mcp), None)
    );
    drop(client);
    drop(reader);
    host.close().expect("close host");
}

// TU-4: until U-6 a connection that never sends BindChannel is bound at its first request by
// that request's envelope source: Ui -> ui, Lab -> lab, Cli and Adapter -> cli. The binding is
// one of the four values and stays (a later BindChannel names it); the Status answer is the
// same structure, with no channel in it.
#[test]
fn u2_an_unbound_connection_is_bound_from_its_first_request_s_envelope_until_u6() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    for (actor, source, channel) in [
        (EventActor::Ui, EventSource::Ui, "ui"),
        (EventActor::User, EventSource::Ui, "ui"),
        (EventActor::Lab, EventSource::Lab, "lab"),
        (EventActor::Cli, EventSource::Cli, "cli"),
        (EventActor::Agent, EventSource::Adapter, "cli"),
    ] {
        let mut client = TestClient::connect(&host);
        let status = request_as(&client, actor, source, RuntimeOperation::Status);
        let receipt = client.send(&status);
        assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
        assert_eq!(
            receipt.channel().map(RuntimeChannel::as_str),
            Some(channel),
            "{source:?}"
        );
        let Some(RuntimeResult::Status { status }) = receipt.result() else {
            panic!("the status: {receipt:?}");
        };
        let status = serde_json::to_value(status).expect("status encodes");
        assert!(!holds_member(&status, "channel"));
        let again = bind(&mut client, RuntimeChannel::Mcp, None);
        assert_eq!(again.outcome().code().as_str(), "channel_binding_exists");
        assert_eq!(again.outcome().value("channel"), Some(&json!(channel)));
    }
    host.close().expect("close host");
}

// TU-6 (the Runtime's half): a Lab connection that MCP forwarded binds `lab` with `via = mcp`,
// and its command records carry both; `via` on another channel is refused before any binding.
#[test]
fn u2_a_lab_connection_forwarded_by_mcp_names_lab_and_mcp_on_its_records() {
    let root = TempDir::new().expect("tempdir");
    let (host, key) = start_with_setting(&root);
    let mut client = TestClient::connect(&host);
    let mut reader = TestClient::connect(&host);

    let off_lab = bind(&mut client, RuntimeChannel::Cli, Some(ChannelVia::Mcp));
    assert_eq!(off_lab.state(), RuntimeReceiptState::Denied);
    assert_eq!(off_lab.outcome().code().as_str(), "invalid_bind_channel");

    let bound = bind(&mut client, RuntimeChannel::Lab, Some(ChannelVia::Mcp));
    assert_eq!(
        bound.result(),
        Some(&RuntimeResult::ChannelBound {
            channel: RuntimeChannel::Lab,
            via: Some(ChannelVia::Mcp),
        })
    );
    let configure = request_as(
        &client,
        EventActor::Lab,
        EventSource::Lab,
        set_stuck_recovery(&key),
    );
    let receipt = client.send(&configure);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert_eq!(
        (receipt.channel(), receipt.via()),
        (Some(RuntimeChannel::Lab), Some(ChannelVia::Mcp))
    );
    assert_eq!(
        received_channel(&mut reader, &configure),
        (Some(RuntimeChannel::Lab), Some(ChannelVia::Mcp))
    );
    drop(client);
    drop(reader);
    host.close().expect("close host");
}

// TU-7 (the door's half): with neither Lab option in the configuration, the Lab channel still
// takes an option-two operation (a tap) and the door refuses no option-one request: its receipt
// carries the channel, and a refusal is the handler's own, never a channel or Lab-option code.
#[test]
fn u2_the_lab_channel_takes_option_requests_before_u6_without_any_lab_option() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    let mut client = TestClient::connect(&host);
    let mut reader = TestClient::connect(&host);
    assert_eq!(
        bind(&mut client, RuntimeChannel::Lab, None).state(),
        RuntimeReceiptState::Completed
    );

    let acquire = request_as(
        &client,
        EventActor::Lab,
        EventSource::Lab,
        RuntimeOperation::acquire_lease(ALIAS, client.ids.mint_holder_id().expect("holder id")),
    );
    let acquired = client.send(&acquire);
    assert_eq!(acquired.channel(), Some(RuntimeChannel::Lab));
    let Some(RuntimeResult::LeaseGranted { token }) = acquired.result() else {
        panic!("the lease: {acquired:?}");
    };
    let tap = request_as(
        &client,
        EventActor::Lab,
        EventSource::Lab,
        RuntimeOperation::Input {
            frame: None,
            token: token.clone(),
            action: InputAction::Tap { x: 10, y: 20 },
        },
    );
    let tapped = client.send(&tap);
    assert_eq!(tapped.state(), RuntimeReceiptState::Completed);
    assert_eq!(tapped.channel(), Some(RuntimeChannel::Lab));
    assert_eq!(
        received_channel(&mut reader, &tap),
        (Some(RuntimeChannel::Lab), None)
    );

    // Option one: writing resource targets is not refused by the door (no catalog is active
    // here, so the request's own checks refuse it).
    let targets = request_as(
        &client,
        EventActor::Agent,
        EventSource::Adapter,
        RuntimeOperation::ApplyResourceTargets {
            document_json: "{}".to_owned(),
        },
    );
    let refused = client.send(&targets);
    assert_eq!(refused.channel(), Some(RuntimeChannel::Lab));
    let code = refused.outcome().code().as_str().to_owned();
    assert!(
        !code.starts_with("channel_") && !code.starts_with("lab_module_"),
        "the door refused {code}"
    );
    drop(client);
    drop(reader);
    host.close().expect("close host");
}

// TU-9: actingd's configured approvals are written in process: the record's provenance is
// (Governance, User, Ui), the approval projection recovers it as usual, no identity card is
// declared for it and no approval record of another provenance exists.
#[test]
fn u2_a_configured_approval_is_written_in_process_with_the_approval_provenance() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    let decision = ApprovalDecisionRecord::new(
        "approval:u2-configured",
        ApprovalDisposition::Approved,
        ApprovalTarget::Catalog {
            catalog_hash: format!("sha256:{}", "a".repeat(64)),
            catalog_version: 1,
        },
        actingcommand_contract::ApprovalReasonCode::new("configured_catalog_approval")
            .expect("approval reason"),
    )
    .expect("approval decision");

    host.transcribe_configured_approval(&decision)
        .expect("the configured approval is written");

    let latest = host
        .latest_approval_decisions(&["approval:u2-configured".to_owned()])
        .expect("the approval projection recovers");
    assert_eq!(latest.get("approval:u2-configured"), Some(&decision));
    let mut reader = TestClient::connect(&host);
    let approvals = projected_events(
        &mut reader,
        EventQuery {
            event_type: Some(EventType::ApprovalDecision),
            ..EventQuery::default()
        },
    );
    assert_eq!(approvals.len(), 1);
    for approval in &approvals {
        assert_eq!(
            (
                approval.origin.module(),
                approval.origin.actor(),
                approval.origin.source()
            ),
            (OriginModule::Governance, EventActor::User, EventSource::Ui)
        );
    }
    assert!(
        projected_events(
            &mut reader,
            EventQuery {
                event_type: Some(EventType::GovernanceIdentityDeclared),
                ..EventQuery::default()
            },
        )
        .is_empty()
    );
    drop(reader);
    host.close().expect("close host");
}
