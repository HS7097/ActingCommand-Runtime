// SPDX-License-Identifier: AGPL-3.0-only

//! Board #391 item 1: a registered instance alias with upper-case letters is an alias like any
//! other. Through `actingctl mcp-serve`, `ac_pause` and `ac_resume` on such an instance are
//! accepted, and the ledger holds the `client.action` record of each with that alias
//! byte-for-byte. The Runtime is the c4 fake child process (`tests/support/c4_runtime.rs`); the
//! MCP server is the real binary over pipes. Every wait returns as soon as its message arrives;
//! `GATE_WAIT` only bounds a broken run.
//!
//! Workflow #378 A5 (coordinator ruling): a Runtime refusal through MCP carries its receipt's
//! `receipt_state` and `request_id` on the refusal's top outcome. A second `ac_resume` with the
//! revision the first one already lifted is refused (#338 R4: a conditional resume lifts only the
//! pause the caller saw).

use crate::c4_runtime::{RuntimeChild, write_sealed_frame};
use crate::mcp_harness::{McpServer, accepted};
use actingcommand_contract::{
    ClientActionKind, ClientPayload, EventActor, EventPayload, EventQuery, EventSource, EventType,
    ProjectionPayload, ProjectionProfile,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::json;
use tempfile::TempDir;

/// The registered alias of the fake instance: neutral, with upper-case letters.
const ALIAS: &str = "Neutral.Instance";

#[test]
fn gate_mcp_pause_and_resume_record_the_client_action_of_an_upper_case_alias() {
    let root = TempDir::new().expect("tempdir");
    write_sealed_frame(&root.path().join("sealed.png"));
    let mut runtime =
        RuntimeChild::spawn_for_instance(root.path(), "c4_runtime_child_process", ALIAS);
    runtime.wait_ready(root.path());
    let mut mcp = McpServer::spawn(root.path());
    mcp.initialize();

    let pause = mcp.call_tool(2, "ac_pause", json!({"instance": ALIAS}));
    let paused = accepted(&pause, "ac_pause");
    assert_eq!(
        paused["paused"]["scope"],
        json!({"kind": "instance", "instance_alias": ALIAS}),
        "ac_pause answer: {pause}"
    );
    let revision = paused["paused"]["revision"]
        .as_u64()
        .filter(|revision| *revision >= 1)
        .unwrap_or_else(|| panic!("ac_pause answer carries no pause revision: {pause}"));
    let owner_epoch = paused["owner_epoch"]
        .as_str()
        .unwrap_or_else(|| panic!("ac_pause answer carries no owner_epoch: {pause}"))
        .to_owned();

    let resume = mcp.call_tool(
        3,
        "ac_resume",
        json!({
            "instance": ALIAS,
            "expected_owner_epoch": owner_epoch,
            "expected_revision": revision,
        }),
    );
    let resumed = accepted(&resume, "ac_resume");
    assert!(resumed["resumed"].is_object(), "ac_resume answer: {resume}");

    let ledger = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect a ledger client");
    let events = ledger
        .query_events(
            EventQuery {
                event_type: Some(EventType::ClientAction),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("query the client.action events");
    drop(ledger);
    let records = events
        .iter()
        .filter_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Client(ClientPayload::Action(action)) => Some(action.record()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    for control in ["ac_pause", "ac_resume"] {
        let recorded = records
            .iter()
            .filter(|record| record.surface_id() == "mcp" && record.control_id() == control)
            .collect::<Vec<_>>();
        assert_eq!(
            recorded.len(),
            1,
            "client.action records of {control}: {records:?}"
        );
        assert_eq!(
            recorded[0].kind(),
            ClientActionKind::Command,
            "client.action record of {control}: {records:?}"
        );
        assert_eq!(
            recorded[0].instance_alias(),
            Some(ALIAS),
            "client.action record of {control}: {records:?}"
        );
    }

    let stale = mcp.call_tool(
        4,
        "ac_resume",
        json!({
            "instance": ALIAS,
            "expected_owner_epoch": owner_epoch,
            "expected_revision": revision,
        }),
    );
    assert_eq!(
        stale["result"]["isError"],
        json!(true),
        "a resume of a lifted pause was not refused: {stale}"
    );
    let values = &stale["result"]["structuredContent"]["error"]["outcome"]["values"];
    assert_eq!(
        values["receipt_state"],
        json!("denied"),
        "the refusal does not carry its receipt state: {stale}"
    );
    assert!(
        values["request_id"]
            .as_str()
            .is_some_and(|request_id| !request_id.is_empty()),
        "the refusal does not carry its request_id: {stale}"
    );

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}
