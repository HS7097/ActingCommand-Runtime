// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S2-B2 (register §1.287, "Slice LAB-6S2-B2"; contracts/lab-two-entries.md
// §2.4, §4.2; the coordinator's rulings of 2026-11-13 on board #434, not the owner's words): every
// connection actingctl's command lines make binds `lab` with `Lab-CLI` and no `via`, and keeps the
// envelope it sent on 0.12.58. Fakes only: the runtime client's scripted Runtime on loopback in a
// temporary state root, which refuses the first request after each connection's handshake, so
// that nothing waits; no device, no clock, no sleep.
// - `lab6s2b2_command_lines_bind_lab_with_lab_cli_and_keep_their_envelope`: actingctl's own
//   commands (`run`, `client_config`), one command line for each kind its connection sends first:
//   `status` (Status), `request-shutdown` (RequestShutdown), `install-transition` of a query
//   (InstallTransition) and of a drain (its identity card, DeclareGovernanceIdentity),
//   `config set` (ConfigureSettings), `pause` (PauseScheduling), `resume` (ResumeScheduling) and
//   `task-offset` (its Status read), each with (Cli, Cli), and `agent-apply-resource-targets`
//   (ApplyResourceTargets) and `agent-publish-facts` (PublishFacts), each with (Agent, Adapter).
// - `lab6s2b2_operation_layer_commands_bind_lab_with_lab_cli`: actingctl never says which program
//   it is, so the commands of the operation layer connect as `Front::Actingctl`, whose word is
//   this program's: `routine-summary` (RoutineSummary) with (Cli, Cli).
// - `lab6s2b2_shared_code_binds_lab_with_lab_cli` (Windows): the configurations the code actingctl
//   shares with the Lab builds with this program's word: the rescue module's client
//   (`lab_rescue::runtime_start::client_config`, every connection of `install`, `start` and
//   `restart`) and the watchdog's probe (`watchdog::observe::live_owner`: `watchdog`, the readiness
//   of `start` and `restart`, `install`'s, the overview's `watchdog` block), each with (Cli, Cli),
//   the probe sending `Health` and `BindChannel` only.

use super::{Done, WORD, run};
use actingcommand_contract::outcome::Detail;
use actingcommand_contract::{
    DeclaredInitiator, EventActor, EventSource, FactObservation, FactRecord, FactScope, Initiator,
    RuntimeChannel, RuntimeErrorCode, RuntimeErrorProjection, RuntimeOperation,
    RuntimeReceiptState, RuntimeRequest,
};
use actingcommand_lab_ops::front::{self, Front};
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedConnectionRecord, ScriptedRuntime,
};
use serde_json::json;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// The envelope of a command line's own requests on 0.12.58.
const CLI: (EventActor, EventSource) = (EventActor::Cli, EventSource::Cli);
/// The envelope of `agent-publish-facts` and `agent-apply-resource-targets` on 0.12.58.
const AGENT: (EventActor, EventSource) = (EventActor::Agent, EventSource::Adapter);
/// A transition's query, which declares no identity card.
const QUERY: &str = r#"{"action":"query","transition_id":"transition-b2"}"#;
/// A transition's drain, which declares the identity card first.
const BEGIN_DRAIN: &str = r#"{"action":"begin_drain","transition_id":"transition-b2"}"#;

/// A refusal of the Runtime: denied, `lease_busy`.
fn denied(request: &RuntimeRequest) -> ScriptedAnswer {
    ScriptedAnswer::failed(
        request,
        RuntimeReceiptState::Denied,
        RuntimeErrorProjection::new(RuntimeErrorCode::LeaseBusy, false).receipt_outcome(None, None),
    )
}

/// A binding as a `BindChannel` declares it: the channel, `via` and the word.
type Declared = (
    RuntimeChannel,
    Option<actingcommand_contract::ChannelVia>,
    Option<Initiator>,
);

/// The binding the received `request`, a `BindChannel`, declares.
fn declared(request: &RuntimeRequest) -> Declared {
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

/// Asserts that `record` received the handshake, `Lab-CLI` on the Lab channel without `via`, then
/// `kind`, every request with `envelope`.
fn assert_bound_with_lab_cli(
    record: &ScriptedConnectionRecord,
    kind: &str,
    envelope: (EventActor, EventSource),
    case: &str,
) {
    assert_eq!(
        operations(record),
        ["health", "bind_channel", kind],
        "{case}"
    );
    assert_eq!(
        declared(&record.requests[1]),
        (RuntimeChannel::Lab, None, Some(Initiator::LabCli)),
        "{case}"
    );
    for request in &record.requests {
        assert_eq!(
            (request.actor(), request.source()),
            envelope,
            "{case}: {:?}",
            request.operation()
        );
    }
}

/// `command`'s words, then `file` after its flag when one is given, then `--state-root <state>`.
fn line_with(state: &Path, command: &[&str], file: Option<&Path>) -> Vec<OsString> {
    let mut arguments = command
        .iter()
        .map(|word| OsString::from(*word))
        .collect::<Vec<_>>();
    if let Some(file) = file {
        arguments.push(file.as_os_str().to_owned());
    }
    arguments.push(OsString::from("--state-root"));
    arguments.push(state.as_os_str().to_owned());
    arguments
}

/// `command`'s words, then `--state-root <state>`.
fn line(state: &Path, command: &[&str]) -> Vec<OsString> {
    line_with(state, command, None)
}

#[test]
fn lab6s2b2_command_lines_bind_lab_with_lab_cli_and_keep_their_envelope() {
    assert_eq!(WORD.word(), Initiator::LabCli);
    let temp = TempDir::new().expect("tempdir");
    let state = temp.path().join("state");
    fs::create_dir_all(&state).expect("state root");
    // The files the two agent commands read before they connect.
    let policy = temp.path().join("policy.json");
    fs::write(&policy, b"{}").expect("write the policy document");
    let observation = temp.path().join("observation.json");
    let record = FactRecord::priority_offset(
        FactScope::Instance {
            instance_id: "neutral.b2".to_owned(),
        },
        "neutral.task",
        250,
        1_800_000_000_000,
        "neutral.test.offset",
    )
    .expect("priority offset record");
    fs::write(
        &observation,
        serde_json::to_vec(&FactObservation {
            records: vec![record],
        })
        .expect("observation JSON"),
    )
    .expect("write the observation");
    let agent_publish = line_with(
        &state,
        &["agent-publish-facts", "--record-file"],
        Some(&observation),
    );
    let agent_targets = line_with(
        &state,
        &["agent-apply-resource-targets", "--policy-file"],
        Some(&policy),
    );
    // Each command line, the kind its connection sends first, and its envelope of 0.12.58.
    let cases: Vec<(Vec<OsString>, &str, (EventActor, EventSource))> = vec![
        (line(&state, &["status"]), "status", CLI),
        (line(&state, &["request-shutdown"]), "request_shutdown", CLI),
        (
            line(&state, &["install-transition", "--action-json", QUERY]),
            "install_transition",
            CLI,
        ),
        (
            line(
                &state,
                &["install-transition", "--action-json", BEGIN_DRAIN],
            ),
            "declare_governance_identity",
            CLI,
        ),
        (
            line(&state, &["config", "set", "neutral.key=1"]),
            "configure_settings",
            CLI,
        ),
        (line(&state, &["pause"]), "pause_scheduling", CLI),
        (line(&state, &["resume"]), "resume_scheduling", CLI),
        (
            line(&state, &["task-offset", "neutral.task", "250"]),
            "status",
            CLI,
        ),
        (agent_targets, "apply_resource_targets", AGENT),
        (agent_publish, "publish_facts", AGENT),
    ];
    let runtime = ScriptedRuntime::start(&state, |_| {
        cases
            .iter()
            .map(|_| ScriptedConnection::new().request(denied))
            .collect()
    });
    for (arguments, kind, _) in &cases {
        // The Runtime refuses the request: no command line succeeds.
        assert!(
            !matches!(
                run(arguments.clone(), Detail::Full),
                Ok(Done { refused: false, .. })
            ),
            "{kind}: {arguments:?}"
        );
    }
    let record = runtime.finish();
    assert!(
        record.unscripted.is_empty(),
        "a connection beyond the script"
    );
    assert_eq!(record.connections.len(), cases.len());
    for (connection, (arguments, kind, envelope)) in record.connections.iter().zip(&cases) {
        assert_bound_with_lab_cli(connection, kind, *envelope, &format!("{arguments:?}"));
    }
}

#[test]
fn lab6s2b2_operation_layer_commands_bind_lab_with_lab_cli() {
    // This program never says which program runs the operation layer's commands.
    assert_eq!(front::current(), Front::Actingctl);
    assert_eq!(Front::Actingctl.initiator(), Some(WORD));
    let temp = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(temp.path(), |_| {
        vec![ScriptedConnection::new().request(denied)]
    });
    let exit = actingcommand_lab_ops::routine_summary::dispatch(
        &line(temp.path(), &["routine-summary", "--hour"]),
        Detail::Full,
    )
    .expect("the words of routine-summary");
    assert_ne!(exit.code, 0, "the Runtime refuses the read");
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    let [connection] = record.connections.as_slice() else {
        panic!("{} connections, not one", record.connections.len());
    };
    assert_bound_with_lab_cli(connection, "routine_summary", CLI, "routine-summary");
}

#[cfg(windows)]
#[test]
fn lab6s2b2_shared_code_binds_lab_with_lab_cli() {
    let temp = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(temp.path(), |_| {
        vec![
            ScriptedConnection::new().request(denied),
            ScriptedConnection::new(),
        ]
    });
    // The rescue module's client, as `install`, `start` and `restart` make it for this program.
    let client = actingcommand_runtime_client::RuntimeClient::connect(
        actingcommand_lab_rescue::runtime_start::client_config(temp.path(), Some(WORD)),
    )
    .expect("connect as the rescue module's client");
    assert!(client.status().is_err(), "the scripted Runtime refuses it");
    drop(client);
    // The watchdog's probe, as `watchdog`, `start`, `restart`, `install` and the overview make it
    // for this program.
    actingcommand_watchdog::observe::live_owner(temp.path(), None, Some(WORD))
        .unwrap_or_else(|failure| panic!("the owner answers: {failure}"));
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the two");
    let [rescue, probe] = record.connections.as_slice() else {
        panic!("{} connections, not two", record.connections.len());
    };
    assert_bound_with_lab_cli(rescue, "status", CLI, "the rescue module's client");
    // `Health` and `BindChannel` only.
    assert_eq!(operations(probe), ["health", "bind_channel"]);
    assert_eq!(
        declared(&probe.requests[1]),
        (RuntimeChannel::Lab, None, Some(Initiator::LabCli))
    );
    for request in &probe.requests {
        assert_eq!((request.actor(), request.source()), CLI);
    }
}
