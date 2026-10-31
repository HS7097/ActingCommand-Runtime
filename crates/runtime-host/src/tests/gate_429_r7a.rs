// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-7a (register §1.233; contracts/player-hold.md; design R-7a §7.1 "host"): the
// player hold's two request kinds have their wire shapes and no behaviour yet. On each of the
// four channels (`ui`, `cli`, `mcp`, `lab`, each with its frontend's envelope) both are refused
// `not_implemented` (Denied) naming their operation, as R-0's kinds are, and have no effect:
// nothing is appended to the ledger, no instance's status changes, and neither Status nor
// StartReport carries any of the new members.

use super::gate_383_ao4q::ledger_head;
use super::*;
use actingcommand_contract::{
    ChannelVia, PlayerHoldExpectation, PlayerHoldStartupChoice, RuntimeChannel,
    RuntimeInstanceStatus,
};
use serde_json::{Value, json};

const ALIAS: &str = "node.a";

fn status_json(reader: &mut TestClient) -> (Vec<RuntimeInstanceStatus>, Value) {
    let request = reader.request(RuntimeOperation::Status);
    let receipt = reader.send(&request);
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("the status: {receipt:?}");
    };
    (
        status.instances().to_vec(),
        serde_json::to_value(status).expect("status JSON"),
    )
}

/// The status JSON without its `source`'s sampling interval (`sampled_started_at_unix_ms`,
/// `sampled_completed_at_unix_ms`), the time of the read itself; both must be there.
fn without_sampling_interval(status: &Value) -> Value {
    let mut status = status.clone();
    let source = status
        .get_mut("source")
        .and_then(Value::as_object_mut)
        .expect("the status source");
    for sampled in ["sampled_started_at_unix_ms", "sampled_completed_at_unix_ms"] {
        assert!(source.remove(sampled).is_some(), "{sampled}");
    }
    status
}

fn start_report_json(reader: &mut TestClient) -> Value {
    let request = reader.request(RuntimeOperation::StartReport);
    let receipt = reader.send(&request);
    let Some(RuntimeResult::StartReport { report }) = receipt.result() else {
        panic!("the start report: {receipt:?}");
    };
    serde_json::to_value(report).expect("start report JSON")
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

#[test]
fn r7a_both_player_hold_kinds_answer_not_implemented_on_every_channel_with_no_effect() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    let mut reader = TestClient::connect(&host);

    // One connection per channel, bound as its frontend binds it.
    let channels = [
        (RuntimeChannel::Ui, None, EventActor::User, EventSource::Ui),
        (RuntimeChannel::Cli, None, EventActor::Cli, EventSource::Cli),
        (RuntimeChannel::Mcp, None, EventActor::Cli, EventSource::Cli),
        (
            RuntimeChannel::Lab,
            Some(ChannelVia::Mcp),
            EventActor::Lab,
            EventSource::Lab,
        ),
    ];
    let mut clients = Vec::new();
    for (channel, via, actor, source) in channels {
        let mut client = TestClient::connect(&host);
        let bind = client.request(RuntimeOperation::BindChannel { channel, via });
        let bound = client.send(&bind);
        assert_eq!(bound.state(), RuntimeReceiptState::Completed, "{channel:?}");
        clients.push((client, channel, actor, source));
    }

    let (instances_before, status_before) = status_json(&mut reader);
    let report_before = start_report_json(&mut reader);
    let head_before = ledger_head(&mut reader);

    for (client, channel, actor, source) in &mut clients {
        let owner_epoch = *client.ids.mint_owner_epoch().expect("epoch").transport();
        let hold_id = *client.ids.mint_request_id().expect("hold id").transport();
        let cases = [
            (
                RuntimeOperation::AcquirePlayerHold {
                    instance_alias: ALIAS.to_owned(),
                    drain_timeout_ms: 60_000,
                },
                "acquire_player_hold",
            ),
            (
                RuntimeOperation::ReleasePlayerHold {
                    instance_alias: ALIAS.to_owned(),
                    expected: PlayerHoldExpectation {
                        owner_epoch,
                        hold_id,
                        revision: 1,
                    },
                    startup_package: Some(PlayerHoldStartupChoice::Skip),
                },
                "release_player_hold",
            ),
        ];
        for (operation, name) in cases {
            let request = request_as(client, *actor, *source, operation);
            let receipt = client.send(&request);
            assert_eq!(
                receipt.state(),
                RuntimeReceiptState::Denied,
                "{name} on {channel:?}"
            );
            assert_eq!(
                receipt.outcome().code().as_str(),
                "not_implemented",
                "{name} on {channel:?}"
            );
            assert_eq!(
                receipt.outcome().value("operation"),
                Some(&json!(name)),
                "{name} on {channel:?}"
            );
            assert_eq!(receipt.channel(), Some(*channel), "{name}");
            assert!(receipt.result().is_none(), "{name} on {channel:?}");
            assert!(receipt.terminal().is_none(), "{name} on {channel:?}");
        }
    }

    assert_eq!(ledger_head(&mut reader), head_before);
    let (instances_after, status_after) = status_json(&mut reader);
    assert_eq!(instances_after, instances_before);
    // Each Status read samples at its own time, so its sampling interval is the read's and no
    // state. Every other member is unchanged: the owner epoch, each instance's members, the
    // store revisions and the ledger's committed head.
    assert_eq!(
        status_after["source"]["ledger_committed_head"],
        status_before["source"]["ledger_committed_head"]
    );
    assert_eq!(
        without_sampling_interval(&status_after),
        without_sampling_interval(&status_before)
    );
    for instance in status_after["instances"].as_array().expect("instances") {
        for absent in ["player_hold", "player_hold_operation", "control"] {
            assert!(instance.get(absent).is_none(), "{absent}: {instance}");
        }
    }
    let report_after = start_report_json(&mut reader);
    for absent in [
        "player_holds_restored",
        "player_holds_dropped",
        "player_hold_not_durable",
    ] {
        assert!(
            report_after.get(absent).is_none(),
            "{absent}: {report_after}"
        );
    }
    assert_eq!(
        report_after["dispatches_since_start"],
        report_before["dispatches_since_start"]
    );
    drop(clients);
    drop(reader);
    host.close().expect("close host");
}
