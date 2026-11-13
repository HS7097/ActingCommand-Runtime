// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S2-B1 (register §1.287, "Slice LAB-6S2-B1"; contracts/lab-two-entries.md
// §2.4, §4.2; the coordinator's rulings of 2026-11-13 on board #434, not the owner's words): the
// Lab's own programs bind `lab` with their initiator word and no `via`, and keep the envelope they
// sent on 0.12.58; the Runtime records each request with the channel that front had then.
// - Scripted Runtime (the runtime client's, on loopback in a temporary state root):
//   - `lab6s2b1_cli_core_rows_bind_lab_with_the_process_word`: the core rows of a command line and
//     of `serve --stdio` connect as `Front::Actinglab` with the process's word (`Lab-CLI`, the
//     `Lab-制包` or `Lab-救急` of `ACTINGCOMMAND_INITIATOR`; `Lab-MCP` under `serve --stdio --front
//     mcp`): `BindChannel` names `lab`, the word and no `via`, and every request carries (Lab, Lab).
//   - `lab6s2b1_own_commands_serve_and_the_mcp_child_bind_lab_with_their_word`: the connections
//     of actinglab's own commands (`lab_client_config_via`): on a command line and under `serve`
//     the process's word, for MCP's `--via mcp` child and `serve --front mcp` `Lab-MCP` without
//     `via`; `--via ui`, which no word names, and a variable that names no word bind as on
//     0.12.58; every request carries (Lab, Lab).
// - One host each (the test host of this crate's tests): one request that the host records,
//   `ConfigureSettings` of a key no configuration holds (refused after its command record), sent
//   through the front's own configuration; the client record's type, source, module and actor and
//   the command record's channel and `via` are what that front recorded on 0.12.58, written as
//   literals from the host's C4 path (`runtime-host` `requests.rs`, the client record by the
//   envelope's source, `command.received` with the channel the map M of LAB-6S2-M gives):
//   - `lab6s2b1_cli_core_rows_record_as_on_0_12_58`: `lab.request` (Lab, actinglab, Lab), `lab`;
//   - `lab6s2b1_cli_rescue_rows_record_as_on_0_12_58` (Windows): `cli.command` (Cli, actingctl,
//     Cli), `cli`;
//   - `lab6s2b1_serve_without_the_mcp_front_records_as_on_0_12_58`: `lab.request`, `lab`;
//   - `lab6s2b1_serve_with_the_mcp_front_and_the_mcp_child_record_as_on_0_12_58`: `lab.request`,
//     `lab` with `via` `mcp`;
//   - (fix 1) `lab6s2b1_core_rows_under_serve_front_mcp_record_lab_with_via_mcp`: the core rows'
//     front under `serve --stdio --front mcp` (the option read by `serve::front`, the front
//     `cli_module::set_front` sets): `lab.request`, `lab` with `via` `mcp`, the slice's one record
//     change (0.12.58: `lab` without `via`);
//   - `lab6s2b1_mcp_server_in_process_records_as_on_0_12_58`: `cli.command` (Cli, actingctl, Cli),
//     `mcp`;
//   - `lab6s2b1_watchdog_probe_records_nothing` (Windows): the probe with `Lab-看门狗` is
//     answered, and the host records no client and no command record, as for 0.12.58's probe.
// No ids, times or paths are compared, and no clock is read.

use super::*;
use crate::cli_module::{self, serve, words};
use actingcommand_contract::{
    CommandPayload, ConfigSettingChange, DeclaredInitiator, EventActor, EventPayload, EventQuery,
    EventSource, EventType, OriginModule, ProjectedEvent, ProjectionPayload, ProjectionProfile,
    RuntimeControlPlaneStatus, RuntimeOperation, RuntimeRequest, RuntimeResult,
    SchedulingPauseScope,
};
use actingcommand_lab_ops::core_ops::{self, Caller};
use actingcommand_lab_ops::front::{self, Connection, Front};
use actingcommand_lab_ops::initiator::{ENVIRONMENT_VARIABLE, Initiator};
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedConnectionRecord, ScriptedRuntime,
};
use actingcommand_runtime_client::{ChannelVia, RuntimeChannel};

/// A binding as a `BindChannel` declares it: the channel, `via` and the word.
type Declared = (
    RuntimeChannel,
    Option<ChannelVia>,
    Option<actingcommand_contract::Initiator>,
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

/// Asserts that every request `record` holds, its handshake included, carries the envelope
/// `(actor, source)`.
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

/// Sets `ACTINGCOMMAND_INITIATOR` to `value`, or removes it.
fn set_variable(value: Option<&str>) {
    unsafe {
        match value {
            Some(value) => env::set_var(ENVIRONMENT_VARIABLE, value),
            None => env::remove_var(ENVIRONMENT_VARIABLE),
        }
    }
}

/// The scripted answer to the core row `resume`: every pause lifted.
fn resumed(request: &RuntimeRequest) -> ScriptedAnswer {
    ScriptedAnswer::completed(
        request,
        RuntimeResult::SchedulingResumed {
            scope: SchedulingPauseScope::Global,
            revision: 2,
            selfcheck: None,
        },
    )
}

#[test]
fn lab6s2b1_cli_core_rows_bind_lab_with_the_process_word() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let state = temp.path().join("state");
    fs::create_dir_all(&state).unwrap();
    // The variable, the front end `serve --stdio` names, and the word the core rows declare.
    let cases = [
        (None, None, Initiator::LabCli),
        (Some("Lab-制包"), None, Initiator::LabAuthoring),
        (Some("Lab-救急"), None, Initiator::LabRescue),
        (None, Some(serve::Front::Mcp), Initiator::LabMcp),
    ];
    let runtime = ScriptedRuntime::start(&state, |_| {
        cases
            .iter()
            .map(|_| ScriptedConnection::new().request(resumed))
            .collect()
    });
    let _state_root = use_runtime_state_root(&state);
    let table = cli_module::table().expect("the operation table");
    let call = words::parse(table, &["resume".to_owned()]).expect("the line resume");
    let operation = table.operation(&call.name).expect("the row resume");
    for (variable, serve_front, word) in cases {
        set_variable(variable);
        // The front `cli_module::set_front` sets for the process, made for this call.
        let lab = cli_module::process_word(serve_front)
            .map(Front::Actinglab)
            .expect("the process has a word");
        assert_eq!(lab, Front::Actinglab(word), "{variable:?} {serve_front:?}");
        let exit = core_ops::answer(
            operation,
            &call.params,
            &Caller {
                connection: Connection::Connect(lab),
                ..cli_module::core_ops::caller()
            },
        );
        let line = exit.line.expect("the row's line");
        assert_eq!(
            line["outcome"]["code"],
            json!("runtime_scheduling_resumed"),
            "{line}"
        );
    }
    set_variable(None);
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the four");
    assert_eq!(record.connections.len(), cases.len());
    for (connection, (_, _, word)) in record.connections.iter().zip(cases) {
        assert_eq!(
            operations(connection),
            ["health", "bind_channel", "resume_scheduling"],
            "{word}"
        );
        assert_eq!(
            declared(&connection.requests[1]),
            (RuntimeChannel::Lab, None, Some(word.word())),
            "{word}"
        );
        // The core rows' envelope of 0.12.58.
        assert_envelope(connection, (EventActor::Lab, EventSource::Lab));
    }
}

#[test]
fn lab6s2b1_own_commands_serve_and_the_mcp_child_bind_lab_with_their_word() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    // `serve --stdio --front mcp` gives its own commands the `via` MCP's `--via mcp` child names.
    assert_eq!(serve::Front::Mcp.via(), ChannelVia::Mcp);
    // The variable, the `via` of the process, and the binding its own commands declare.
    let cases: [(Option<&str>, Option<ChannelVia>, Declared); 6] = [
        // A command line, and `serve --stdio` without `--front`.
        (
            None,
            None,
            (RuntimeChannel::Lab, None, Some(Initiator::LabCli.word())),
        ),
        (
            Some("Lab-制包"),
            None,
            (
                RuntimeChannel::Lab,
                None,
                Some(Initiator::LabAuthoring.word()),
            ),
        ),
        (
            Some("Lab-救急"),
            None,
            (RuntimeChannel::Lab, None, Some(Initiator::LabRescue.word())),
        ),
        // MCP's `--via mcp` child, and `serve --stdio --front mcp`.
        (
            None,
            Some(ChannelVia::Mcp),
            (RuntimeChannel::Lab, None, Some(Initiator::LabMcp.word())),
        ),
        // No word names the counter `ui`: as on 0.12.58.
        (
            None,
            Some(ChannelVia::Ui),
            (RuntimeChannel::Lab, Some(ChannelVia::Ui), None),
        ),
        // A variable that names no word: as on 0.12.58.
        (
            Some("neutral-word"),
            None,
            (RuntimeChannel::Lab, None, None),
        ),
    ];
    let runtime = ScriptedRuntime::start(temp.path(), |owner_epoch| {
        cases
            .iter()
            .map(|_| {
                ScriptedConnection::new().request(move |request| {
                    ScriptedAnswer::completed(
                        request,
                        RuntimeResult::Status {
                            status: RuntimeControlPlaneStatus::new(owner_epoch, Vec::new())
                                .expect("scripted status"),
                        },
                    )
                })
            })
            .collect()
    });
    for (variable, via, _) in cases {
        set_variable(variable);
        let client = RuntimeClient::connect(lab_client_config_via(temp.path(), via))
            .unwrap_or_else(|error| panic!("{variable:?} {via:?}: {error:?}"));
        client
            .status()
            .unwrap_or_else(|error| panic!("{variable:?} {via:?}: {error:?}"));
        drop(client);
    }
    set_variable(None);
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the six");
    assert_eq!(record.connections.len(), cases.len());
    for (connection, (variable, via, binding)) in record.connections.iter().zip(cases) {
        assert_eq!(
            operations(connection),
            ["health", "bind_channel", "status"],
            "{variable:?} {via:?}"
        );
        assert_eq!(
            declared(&connection.requests[1]),
            binding,
            "{variable:?} {via:?}"
        );
        // actinglab's own commands' envelope of 0.12.58.
        assert_envelope(connection, (EventActor::Lab, EventSource::Lab));
    }
}

/// What a fresh host recorded: each client record's type, source, module and actor, and each
/// command record's channel and `via`.
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

/// One host, one connection made with `config` for its state root, one `ConfigureSettings` of a
/// key no configuration holds (refused after its command record): what the host recorded.
fn recorded_through(config: impl FnOnce(&Path) -> RuntimeClientConfig) -> Recorded {
    let root = TempDir::new().expect("state root");
    let host = start_authoring_runtime(root.path());
    let client = RuntimeClient::connect(config(root.path())).expect("connect");
    let refused = client.configure_settings(
        vec![ConfigSettingChange {
            key: "neutral_section.unknown_key".to_owned(),
            value: Some(json!(true)),
        }],
        None,
    );
    assert!(refused.is_err(), "a key no configuration holds is refused");
    drop(client);
    let recorded = recorded(root.path());
    host.close().expect("close the Runtime");
    recorded
}

/// What 0.12.58 recorded for a request with the envelope (Lab, Lab) on `lab`, with `via`.
fn lab_request(via: Option<ChannelVia>) -> Recorded {
    Recorded {
        client: vec![(
            EventType::LabRequest,
            EventSource::Lab,
            OriginModule::Actinglab,
            EventActor::Lab,
        )],
        command: vec![(Some(RuntimeChannel::Lab), via)],
    }
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
fn lab6s2b1_cli_core_rows_record_as_on_0_12_58() {
    // A command line's process word: on 0.12.58 `Front::Actinglab`, `lab` without a word.
    let recorded =
        recorded_through(|root| front::config(Front::Actinglab(Initiator::LabCli), root));
    assert_eq!(recorded, lab_request(None));
}

#[cfg(windows)]
#[test]
fn lab6s2b1_cli_rescue_rows_record_as_on_0_12_58() {
    // The rescue module's own client with a command line's word: on 0.12.58 `cli`, (Cli, Cli).
    let recorded = recorded_through(|root| {
        actingcommand_lab_rescue::runtime_start::client_config(root, Some(Initiator::LabCli))
    });
    assert_eq!(recorded, cli_command(RuntimeChannel::Cli));
}

#[test]
fn lab6s2b1_serve_without_the_mcp_front_records_as_on_0_12_58() {
    let _guard = env_lock();
    set_variable(None);
    // `serve --stdio` without `--front`: its own commands with the process's word, `Lab-CLI`; on
    // 0.12.58 `lab` without `via`.
    let recorded = recorded_through(|root| lab_client_config_via(root, None));
    assert_eq!(recorded, lab_request(None));
}

#[test]
fn lab6s2b1_serve_with_the_mcp_front_and_the_mcp_child_record_as_on_0_12_58() {
    // `serve --stdio --front mcp` and MCP's `--via mcp` child: `Lab-MCP`; on 0.12.58 `lab` with
    // `via` `mcp`.
    let recorded =
        recorded_through(|root| lab_client_config_via(root, Some(serve::Front::Mcp.via())));
    assert_eq!(recorded, lab_request(Some(ChannelVia::Mcp)));
}

#[test]
fn lab6s2b1_core_rows_under_serve_front_mcp_record_lab_with_via_mcp() {
    // Fix 1, the slice's one record change (register, "What a person sees"): `serve --stdio
    // --front mcp` as a person types it, without `--via mcp`. The option as `serve::main` reads it
    // (the variable absent, as the option requires), and the front `cli_module::set_front` then
    // sets for the core rows: actinglab with the process's word, `Lab-MCP`.
    let words = ["--stdio", "--front", "mcp"].map(str::to_owned);
    let serve_front = serve::front(&words, None).expect("--stdio --front mcp");
    assert_eq!(serve_front, Some(serve::Front::Mcp));
    let lab = cli_module::process_word(serve_front)
        .map(Front::Actinglab)
        .expect("the process has a word");
    assert_eq!(lab, Front::Actinglab(Initiator::LabMcp));
    // A core row's request on that front's connection (`Connection::per_call` makes it with
    // `front::config`): the `ConfigureSettings` the core row `config set` sends
    // (`lab_ops::configure`). Recorded `lab` with `via` `mcp`, where 0.12.58 recorded `lab`
    // without it (`lab6s2b1_cli_core_rows_record_as_on_0_12_58`).
    let recorded = recorded_through(|root| front::config(lab, root));
    assert_eq!(recorded, lab_request(Some(ChannelVia::Mcp)));
}

#[test]
fn lab6s2b1_mcp_server_in_process_records_as_on_0_12_58() {
    // The Lab's MCP server, `Front::Mcp`: `Lab-MCP`, (Cli, Cli); on 0.12.58 `mcp`.
    let recorded = recorded_through(|root| front::config(Front::Mcp, root));
    assert_eq!(recorded, cli_command(RuntimeChannel::Mcp));
}

#[cfg(windows)]
#[test]
fn lab6s2b1_watchdog_probe_records_nothing() {
    let root = TempDir::new().expect("state root");
    let host = start_authoring_runtime(root.path());
    actingcommand_watchdog::observe::live_owner(root.path(), None, Some(Initiator::LabWatchdog))
        .expect("the probe is answered");
    let recorded = recorded(root.path());
    host.close().expect("close the Runtime");
    // `Health` and `BindChannel` only: no record, as for 0.12.58's probe.
    assert_eq!(
        recorded,
        Recorded {
            client: Vec::new(),
            command: Vec::new(),
        }
    );
}
