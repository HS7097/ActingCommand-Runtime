// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-6S2-B2 (register §1.287, "Slice LAB-6S2-B2"; contracts/lab-two-entries.md
//! §2.4, §4.2, §4.3; the coordinator's rulings of 2026-11-13 on board #434, not the owner's
//! words): `actingctl` binds `lab` with `Lab-CLI` and today's MCP server (`actingctl mcp-serve`)
//! with `Lab-MCP`, each without `via` and with the envelope it sent on 0.12.58; the Runtime
//! records and answers their requests with the channel their binding had then (the map M and the
//! receipt rule R of LAB-6S2-M). The real binaries, against the runtime client's scripted Runtime
//! or this crate's test host (`RuntimeHost` without instances, in this process). No device, no
//! clock, no sleep.
//!
//! - `lab6s2b2_mcp_serve_binds_lab_with_lab_mcp_and_keeps_its_envelopes` (scripted Runtime): the
//!   server's kept connection (`ac_schedule`'s Status) and a write's connection with (Cli, Cli)
//!   (`ac_config_set`'s identity card) and with (Agent, Adapter) (`ac_remedy_set`'s client
//!   action), each refused: each binds `lab` with `Lab-MCP` and no `via`, every request with its
//!   envelope of 0.12.58.
//! - `lab6s2b2_actingctl_records_as_on_0_12_58` and `lab6s2b2_mcp_serve_records_as_on_0_12_58`
//!   (test host): one `ConfigureSettings` of a key no configuration holds, refused after its
//!   command record, through `actingctl config set` and through `ac_config_set`: the client record
//!   (type, source, module, actor) and the command record (channel, `via`) are the literals of
//!   0.12.58: `cli.command` (Cli, actingctl, Cli) and `cli`, or `mcp`, without `via`.
//! - `lab6s2b2_install_transition_receipts_decode_as_the_released_helper_decodes` and
//!   `lab6s2b2_request_shutdown_receipt_decodes_as_the_released_helper_decodes` (test host): the
//!   receipts the released console's installation helper decodes strictly, as it makes them: the
//!   transition's drain (`install_transition`) and the commit of its ticket (an accepted shutdown),
//!   and the cold `request-shutdown` (an accepted shutdown). Each `receipt` member decodes into the
//!   helper's receipt type ([`ReleasedReceipt`]), which takes every member the receipt has, and
//!   names the CLI channel without `via` and without `initiator`, as on 0.12.58. `--wait`, which
//!   the helper adds to the commit and the shutdown, only adds a member beside `receipt`; the test
//!   host runs in this process, whose exit no wait would see, so it is left out.
//! - `lab6s2b2_a_runtime_that_drops_the_word_leaves_actingctl_unbound_once` (scripted Runtime): a
//!   Runtime that cannot decode the word (one before LAB-6S1a) drops the binding; actingctl
//!   reconnects once, goes on unbound, is answered, and writes the runtime client's report
//!   `client_channel_unsupported` once on stderr.

use crate::held_runtime::{HELD_SALT, NoInstances};
use crate::mcp_harness::McpServer;
use actingcommand_contract::{
    ChannelVia, CommandPayload, DeclaredInitiator, EventActor, EventPayload, EventQuery,
    EventSource, EventType, Initiator, InstallTransitionAction, InstallTransitionTicket,
    OriginModule, ProjectedEvent, ProjectionPayload, ProjectionProfile, RuntimeChannel,
    RuntimeControlPlaneStatus, RuntimeErrorCode, RuntimeErrorProjection, RuntimeOperation,
    RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
};
use actingcommand_lab_ops::front::{self, Front};
use actingcommand_runtime_client::RuntimeClient;
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedConnectionRecord, ScriptedRuntime,
};
use actingcommand_runtime_host::{RuntimeHost, RuntimeHostConfig};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;
use tempfile::TempDir;

/// The instance `ac_schedule` names: its status read comes first.
const ALIAS: &str = "neutral.b2";
/// A key no configuration holds: the Runtime records the request, then refuses it.
const UNKNOWN_KEY: &str = "neutral_section.unknown_key";
/// The transition the drain begins.
const BEGIN_DRAIN: &str = r#"{"action":"begin_drain","transition_id":"lab6s2b2-receipts"}"#;

/// A refusal of the Runtime: denied, `lease_busy`.
fn denied(request: &RuntimeRequest) -> ScriptedAnswer {
    ScriptedAnswer::failed(
        request,
        RuntimeReceiptState::Denied,
        RuntimeErrorProjection::new(RuntimeErrorCode::LeaseBusy, false).receipt_outcome(None, None),
    )
}

/// The binding the received `request`, a `BindChannel`, declares: the channel, `via`, the word.
fn declared(request: &RuntimeRequest) -> (RuntimeChannel, Option<ChannelVia>, Option<Initiator>) {
    let RuntimeOperation::BindChannel {
        channel,
        via,
        initiator,
    } = request.operation()
    else {
        panic!("BindChannel, got {:?}", request.operation());
    };
    (
        *channel,
        *via,
        initiator.as_ref().and_then(DeclaredInitiator::initiator),
    )
}

/// The operation of each request a connection received, as the wire spells it.
fn operations(record: &ScriptedConnectionRecord) -> Vec<String> {
    record
        .requests
        .iter()
        .map(|request| {
            json!(request.operation())["operation"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

/// Asserts that every request of `record`, its handshake included, carries `envelope`.
fn assert_envelope(record: &ScriptedConnectionRecord, envelope: (EventActor, EventSource)) {
    for request in &record.requests {
        assert_eq!(
            (request.actor(), request.source()),
            envelope,
            "{:?}",
            request.operation()
        );
    }
}

/// actingctl with `words`, then `--state-root <state_root>`, in full detail.
fn actingctl(state_root: &Path, words: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args(words)
        .arg("--state-root")
        .arg(state_root)
        .env_remove("ACTINGCOMMAND_DETAIL")
        .output()
        .expect("run actingctl")
}

/// The one stdout line of an exit that succeeded.
fn result_line(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}: {stdout}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|error| panic!("no JSON ({error}): {stdout}"))
}

/// The test host on `root`, without instances.
fn start_host(root: &Path) -> RuntimeHost {
    RuntimeHost::start(
        RuntimeHostConfig::new(root, HELD_SALT),
        Arc::new(NoInstances),
    )
    .expect("start the Runtime")
}

#[test]
fn lab6s2b2_mcp_serve_binds_lab_with_lab_mcp_and_keeps_its_envelopes() {
    let temp = TempDir::new().expect("tempdir");
    let state = temp.path();
    let runtime = ScriptedRuntime::start(state, |_| {
        vec![
            ScriptedConnection::new().request(denied),
            ScriptedConnection::new().request(denied),
            ScriptedConnection::new().request(denied),
        ]
    });
    // Each tool, its arguments, the kind its connection sends after the handshake, and the
    // envelope that connection had on 0.12.58.
    let cases = [
        (
            "ac_schedule",
            json!({"instance": ALIAS}),
            "status",
            (EventActor::Cli, EventSource::Cli),
        ),
        (
            "ac_config_set",
            json!({"changes": [{"key": UNKNOWN_KEY, "value": true}]}),
            "declare_governance_identity",
            (EventActor::Cli, EventSource::Cli),
        ),
        (
            "ac_remedy_set",
            json!({"game": "neutral", "rows": []}),
            "record_client_action",
            (EventActor::Agent, EventSource::Adapter),
        ),
    ];
    let mut mcp = McpServer::spawn_with_tier(state, None);
    mcp.initialize();
    for (id, (tool, arguments, _, _)) in (2_u64..).zip(&cases) {
        let answer = mcp.call_tool(id, tool, arguments.clone());
        assert_eq!(answer["result"]["isError"], json!(true), "{tool}: {answer}");
    }
    let _ = mcp.finish();

    let record = runtime.finish();
    assert!(
        record.unscripted.is_empty(),
        "a connection beyond the three"
    );
    assert_eq!(record.connections.len(), cases.len());
    for (connection, (tool, _, kind, envelope)) in record.connections.iter().zip(&cases) {
        assert_eq!(
            operations(connection),
            ["health", "bind_channel", *kind],
            "{tool}"
        );
        assert_eq!(
            declared(&connection.requests[1]),
            (RuntimeChannel::Lab, None, Some(Initiator::LabMcp)),
            "{tool}"
        );
        assert_envelope(connection, *envelope);
    }
}

/// What a host recorded: each client record's type, source, module and actor, and each command
/// record's channel and `via`.
#[derive(Debug, PartialEq, Eq)]
struct Recorded {
    client: Vec<(EventType, EventSource, OriginModule, EventActor)>,
    command: Vec<(Option<RuntimeChannel>, Option<ChannelVia>)>,
}

/// Every record of `event_type`, read on `reader`.
fn records_of(reader: &RuntimeClient, event_type: EventType) -> Vec<ProjectedEvent> {
    reader
        .query_events(
            EventQuery {
                event_type: Some(event_type),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("records")
}

/// The records of the host whose state root `root` is, read on a connection of actingctl's front
/// (a read records nothing).
fn recorded(root: &Path) -> Recorded {
    let reader = RuntimeClient::connect(front::config(Front::Actingctl, root)).expect("reader");
    let client = [
        EventType::CliCommand,
        EventType::LabRequest,
        EventType::UiAction,
    ]
    .into_iter()
    .flat_map(|event_type| records_of(&reader, event_type))
    .map(|event| {
        (
            event.event_type,
            event.origin.source(),
            event.origin.module(),
            event.origin.actor(),
        )
    })
    .collect();
    let command = records_of(&reader, EventType::CommandReceived)
        .iter()
        .map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Command(CommandPayload::Received(observation)) => {
                    (observation.channel(), observation.via())
                }
                other => panic!("a command.received payload: {other:?}"),
            },
            other => panic!("a full projection: {other:?}"),
        })
        .collect();
    Recorded { client, command }
}

/// What 0.12.58 recorded for a request with the envelope (Cli, Cli) on `channel`.
fn cli_command(channel: RuntimeChannel) -> Recorded {
    Recorded {
        client: vec![(
            EventType::CliCommand,
            EventSource::Cli,
            OriginModule::Actingctl,
            EventActor::Cli,
        )],
        command: vec![(Some(channel), None)],
    }
}

#[test]
fn lab6s2b2_actingctl_records_as_on_0_12_58() {
    let root = TempDir::new().expect("tempdir");
    let host = start_host(root.path());
    let change = format!("{UNKNOWN_KEY}=true");
    let refused = actingctl(root.path(), &["config", "set", change.as_str()]);
    assert!(
        !refused.status.success(),
        "a key no configuration holds is refused: {}",
        String::from_utf8_lossy(&refused.stdout)
    );
    let recorded = recorded(root.path());
    host.close().expect("close the Runtime");
    // actingctl's (Cli, Cli) on `cli`, as on 0.12.58.
    assert_eq!(recorded, cli_command(RuntimeChannel::Cli));
}

#[test]
fn lab6s2b2_mcp_serve_records_as_on_0_12_58() {
    let root = TempDir::new().expect("tempdir");
    let host = start_host(root.path());
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let answer = mcp.call_tool(
        2,
        "ac_config_set",
        json!({"changes": [{"key": UNKNOWN_KEY, "value": true}]}),
    );
    assert_eq!(answer["result"]["isError"], json!(true), "{answer}");
    let _ = mcp.finish();
    let recorded = recorded(root.path());
    host.close().expect("close the Runtime");
    // Today's MCP server's (Cli, Cli) on `mcp`, as on 0.12.58.
    assert_eq!(recorded, cli_command(RuntimeChannel::Mcp));
}

/// The receipt type of the released console's installation helper (acsetup of UI v0.12.16),
/// whose decoder reads the `receipt` member of the line `install-transition` and
/// `request-shutdown` print: `actingcommand_contract::RuntimeReceipt` at the Runtime commit that
/// console pins (`c0f75c7e`), which refuses unknown members and ends at `via`. That commit is not
/// this tree, so its shape is written here member for member: the members are its, each required
/// or optional as there; the ones that do not depend on the connection's binding are read whole,
/// and `channel` and `via` with this tree's types, whose spellings are the pin's.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasedReceipt {
    schema_version: String,
    request_id: Value,
    correlation_id: Value,
    state: Value,
    terminal: Option<Value>,
    result: Option<Value>,
    outcome: Value,
    #[serde(default)]
    resource_declaration: Option<Value>,
    #[serde(default)]
    resource_declaration_event: Option<Value>,
    #[serde(default)]
    resource_targets_rejection: Option<Value>,
    #[serde(default)]
    channel: Option<RuntimeChannel>,
    #[serde(default)]
    via: Option<ChannelVia>,
}

/// Asserts that the `receipt` of `line` decodes as the helper decodes it, that the helper's type
/// takes every member it has (written back member by member, it is the receipt), and that it
/// names the CLI channel without `via` and without `initiator`, as actingctl's receipts did on
/// 0.12.58 (`ChannelBinding::new(Cli)`).
fn assert_released(line: &Value) {
    let receipt = &line["receipt"];
    let ReleasedReceipt {
        schema_version,
        request_id,
        correlation_id,
        state,
        terminal,
        result,
        outcome,
        resource_declaration,
        resource_declaration_event,
        resource_targets_rejection,
        channel,
        via,
    } = serde_json::from_value::<ReleasedReceipt>(receipt.clone()).unwrap_or_else(|error| {
        panic!("the installation helper's receipt type refuses it ({error}): {line}")
    });
    assert_eq!((channel, via), (Some(RuntimeChannel::Cli), None), "{line}");
    let mut written = Map::new();
    written.insert("schema_version".to_owned(), Value::String(schema_version));
    written.insert("request_id".to_owned(), request_id);
    written.insert("correlation_id".to_owned(), correlation_id);
    written.insert("state".to_owned(), state);
    written.insert("outcome".to_owned(), outcome);
    for (name, member) in [
        ("terminal", terminal),
        ("result", result),
        ("resource_declaration", resource_declaration),
        ("resource_declaration_event", resource_declaration_event),
        ("resource_targets_rejection", resource_targets_rejection),
        ("channel", channel.map(|channel| json!(channel))),
        ("via", via.map(|via| json!(via))),
    ] {
        if let Some(member) = member {
            written.insert(name.to_owned(), member);
        }
    }
    assert_eq!(Value::Object(written), *receipt, "{line}");
    // This tree's receipt type reads the same receipt: no word.
    let current = serde_json::from_value::<RuntimeReceipt>(receipt.clone())
        .unwrap_or_else(|error| panic!("the receipt does not decode ({error}): {line}"));
    assert_eq!(
        (current.channel(), current.via(), current.initiator()),
        (Some(RuntimeChannel::Cli), None, None),
        "{line}"
    );
}

#[test]
fn lab6s2b2_install_transition_receipts_decode_as_the_released_helper_decodes() {
    let root = TempDir::new().expect("tempdir");
    let host = start_host(root.path());
    // The helper's close of a running Runtime: the drain, then the commit of its ticket.
    let begun = result_line(&actingctl(
        root.path(),
        &["install-transition", "--action-json", BEGIN_DRAIN],
    ));
    assert_eq!(
        begun["receipt"]["result"]["kind"],
        json!("install_transition"),
        "{begun}"
    );
    let ticket = serde_json::from_value::<InstallTransitionTicket>(
        begun["receipt"]["result"]["status"]["ticket"].clone(),
    )
    .unwrap_or_else(|error| panic!("the drain's ticket ({error}): {begun}"));
    let commit = serde_json::to_string(&InstallTransitionAction::CommitShutdown { ticket })
        .expect("the commit's action");
    let committed = result_line(&actingctl(
        root.path(),
        &["install-transition", "--action-json", commit.as_str()],
    ));
    assert_eq!(
        committed["receipt"]["result"]["kind"],
        json!("shutdown_accepted"),
        "{committed}"
    );
    host.close().expect("close the Runtime");
    assert_released(&begun);
    assert_released(&committed);
}

#[test]
fn lab6s2b2_request_shutdown_receipt_decodes_as_the_released_helper_decodes() {
    let root = TempDir::new().expect("tempdir");
    let host = start_host(root.path());
    // The helper's cold close.
    let accepted = result_line(&actingctl(root.path(), &["request-shutdown"]));
    assert_eq!(
        accepted["receipt"]["result"]["kind"],
        json!("shutdown_accepted"),
        "{accepted}"
    );
    host.close().expect("close the Runtime");
    assert_released(&accepted);
}

#[test]
fn lab6s2b2_a_runtime_that_drops_the_word_leaves_actingctl_unbound_once() {
    let temp = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(temp.path(), |owner_epoch| {
        vec![
            ScriptedConnection::dropping_bind(),
            ScriptedConnection::unbound().request(move |request| {
                ScriptedAnswer::completed(
                    request,
                    RuntimeResult::Status {
                        status: RuntimeControlPlaneStatus::new(owner_epoch, Vec::new())
                            .expect("scripted status"),
                    },
                )
            }),
        ]
    });
    let output = actingctl(temp.path(), &["status"]);
    let record = runtime.finish();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{}: {}; stderr: {stderr}",
        output.status,
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(record.unscripted.is_empty(), "a third connection");
    let [dropped, unbound] = record.connections.as_slice() else {
        panic!("{} connections, not two", record.connections.len());
    };
    // The binding the old Runtime dropped, then one reconnect without `BindChannel`.
    assert_eq!(operations(dropped), ["health", "bind_channel"]);
    assert_eq!(
        declared(&dropped.requests[1]),
        (RuntimeChannel::Lab, None, Some(Initiator::LabCli))
    );
    assert_eq!(operations(unbound), ["health", "status"]);
    assert_envelope(dropped, (EventActor::Cli, EventSource::Cli));
    assert_envelope(unbound, (EventActor::Cli, EventSource::Cli));
    // The runtime client's one report of the process.
    assert_eq!(
        stderr
            .lines()
            .filter(|line| line.contains("client_channel_unsupported"))
            .count(),
        1,
        "{stderr}"
    );
}
