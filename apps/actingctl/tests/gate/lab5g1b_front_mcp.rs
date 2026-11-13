// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5G1b (register §1.287, "Slice LAB-5G1b"; coordinator work order
//! rd-1112/brief-lab5g1b.md, point 3): the operation layer's MCP front (`lab_ops::front`,
//! `Front::Mcp`) connects to the Runtime as today's MCP server connects. One scripted Runtime
//! (the runtime client's, on loopback in a temporary state root) takes two connections, each
//! refusing the one read after its handshake: the real `actingctl mcp-serve` over pipes (fixture
//! I5, `mcp_harness`), whose `ac_schedule` with an instance reads the status first, then a client
//! made with `Front::Mcp`'s configuration reading the status. Both connections carry the same
//! requests, their issued ids and times left out: `Health`, `BindChannel` with the MCP channel and
//! no counter, then `Status`, each with the envelope (Cli, Cli). The members of the configuration
//! no request carries (timeouts, frame bound, deadline, detail) equal those of the expression
//! today's server builds its connections with. No device, no clock, no sleep.
//!
//! Workflow #434 LAB-6S2-B1 (register §1.287, "Slice LAB-6S2-B1"; adapted: the binding of
//! `Front::Mcp` moved, today's server's moves with LAB-6S2-B2): `Front::Mcp` binds `lab` with
//! `Lab-MCP` and no counter, where today's server binds the MCP channel; every other request and
//! every envelope are today's server's, and the members of its configuration but its binding equal
//! those of today's expression.
//!
//! Workflow #434 LAB-6S2-B2 (register §1.287, "Slice LAB-6S2-B2"; adapted: today's server's
//! binding moved): today's server binds `lab` with `Lab-MCP` and no counter too.

use crate::mcp_harness::McpServer;
use actingcommand_contract::{
    DeclaredInitiator, EventActor, EventSource, Initiator, RuntimeErrorCode,
    RuntimeErrorProjection, RuntimeOperation, RuntimeReceiptState, RuntimeRequest,
};
use actingcommand_lab_ops::front::{self, Front};
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedRuntime,
};
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use tempfile::TempDir;

/// The instance `ac_schedule` names: its status read comes first.
const ALIAS: &str = "neutral.front";

/// A refusal of the Runtime: denied, `lease_busy`.
fn denied(request: &RuntimeRequest) -> ScriptedAnswer {
    ScriptedAnswer::failed(
        request,
        RuntimeReceiptState::Denied,
        RuntimeErrorProjection::new(RuntimeErrorCode::LeaseBusy, false).receipt_outcome(None, None),
    )
}

/// A request as sent, without what each connection issues for itself: its ids and its time.
fn sent(request: &RuntimeRequest) -> Value {
    let mut value = serde_json::to_value(request).expect("a request encodes");
    let members = value.as_object_mut().expect("a request is an object");
    for issued in [
        "request_id",
        "correlation_id",
        "causation_id",
        "submitted_at_unix_ms",
    ] {
        members.remove(issued);
    }
    value
}

#[test]
fn lab5g1b_front_mcp_connects_as_todays_mcp_server() {
    let temp = TempDir::new().expect("tempdir");
    let state = temp.path();
    let runtime = ScriptedRuntime::start(state, |_| {
        vec![
            ScriptedConnection::new().request(denied),
            ScriptedConnection::new().request(denied),
        ]
    });

    // Today's MCP server: its kept connection, made at its first call.
    let mut mcp = McpServer::spawn_with_tier(state, None);
    mcp.initialize();
    let answer = mcp.call_tool(2, "ac_schedule", json!({"instance": ALIAS}));
    assert_eq!(answer["result"]["isError"], json!(true), "{answer}");
    let _ = mcp.finish();

    // The operation layer's MCP front.
    let client =
        RuntimeClient::connect(front::config(Front::Mcp, state)).expect("connect as Front::Mcp");
    assert!(client.status().is_err(), "the scripted Runtime refuses it");
    drop(client);

    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the two");
    let [server, lab] = record.connections.as_slice() else {
        panic!("{} connections, not two", record.connections.len());
    };
    let server_sent = server.requests.iter().map(sent).collect::<Vec<_>>();
    let lab_sent = lab.requests.iter().map(sent).collect::<Vec<_>>();
    // Workflow #434 LAB-6S2-B1 (adapted): every request but the binding is today's server's.
    assert_eq!(
        (&lab_sent[..1], &lab_sent[2..]),
        (&server_sent[..1], &server_sent[2..]),
        "Front::Mcp sends what today's MCP server sends, its binding apart"
    );
    let operations = server_sent
        .iter()
        .map(|request| request["operation"]["operation"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        operations,
        [json!("health"), json!("bind_channel"), json!("status")]
    );
    let bound = |request: &RuntimeRequest| {
        let RuntimeOperation::BindChannel {
            channel,
            via,
            initiator,
        } = request.operation()
        else {
            panic!("the second request is no BindChannel");
        };
        (
            *channel,
            *via,
            initiator.as_ref().and_then(DeclaredInitiator::initiator),
        )
    };
    // Workflow #434 LAB-6S2-B2 (adapted): today's server binds as `Front::Mcp` does.
    assert_eq!(
        bound(&server.requests[1]),
        (RuntimeChannel::Lab, None, Some(Initiator::LabMcp))
    );
    assert_eq!(
        bound(&lab.requests[1]),
        (RuntimeChannel::Lab, None, Some(Initiator::LabMcp))
    );
    for request in server_sent.iter().chain(&lab_sent) {
        assert_eq!(
            (&request["actor"], &request["source"]),
            (&json!(EventActor::Cli), &json!(EventSource::Cli)),
            "{request}"
        );
    }

    // What no request carries, as `apps/actingctl/src/mcp/runtime.rs` builds its connections,
    // with `Front::Mcp`'s binding (Workflow #434 LAB-6S2-B1).
    assert_eq!(
        format!("{:?}", front::config(Front::Mcp, state)),
        format!(
            "{:?}",
            RuntimeClientConfig::new(
                state,
                RuntimeChannel::Lab,
                EventActor::Cli,
                EventSource::Cli
            )
            .with_initiator(Initiator::LabMcp)
        )
    );
}
