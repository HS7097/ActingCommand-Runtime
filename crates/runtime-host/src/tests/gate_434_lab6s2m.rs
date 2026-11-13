// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S2-M (register §1.287, "Slice LAB-6S2-M"; contracts/lab-two-entries.md §2.4,
// §4.3): on a `lab` connection that declares a word, the host records each request with the
// channel that program's binding had on 0.12.58 (the map M, `ChannelBinding::recorded`, keyed by
// the word and the request's envelope) and answers it by the receipt rule R. Within one host a
// connection bound as a program binds on 0.12.58 and one bound `lab` with the word it will
// declare, sending the same requests with the same envelope, write the same records (type,
// source, module, actor, channel, `via`), the same settings row channel and the same receipts,
// ids left out; only the receipt of a request from the Lab carries the word, as from LAB-6S1a.
// A binding without a word, `cli`, `mcp`, `ui` and an unbound connection are answered and
// recorded as before, and the UI gate never judges a mapped connection.

use super::*;
use actingcommand_contract::{
    CONFIG_OVERRIDES_FACT_KEY, ChannelBinding, ChannelVia, CommandPayload, ConfigParameter,
    ConfigParameterSource, ConfigSettingChange, DeclaredInitiator, FactObservation, FactScalar,
    Initiator, RequestId, RuntimeChannel, RuntimeConfigManifest, RuntimeOperationKind,
    RuntimeShutdownTarget, ui_allows,
};
use actingcommand_runtime_config::{
    OVERRIDE_CHANNEL, OVERRIDE_CONFIG_KEY, STUCK_RECOVERY, instance_key,
};
use serde_json::{Value, json};

const ALIAS: &str = "node.a";
/// A task of the scheduling identifier charset, for the priority offsets.
const TASK: &str = "task.a";

const LAB_WORDS: [Initiator; 5] = [
    Initiator::LabCli,
    Initiator::LabMcp,
    Initiator::LabAuthoring,
    Initiator::LabRescue,
    Initiator::LabWatchdog,
];

/// What each request of [`compared_requests`] is answered on both connections: its state and,
/// for a refusal, its code.
const ANSWERS: [(RuntimeReceiptState, Option<&str>); 5] = [
    (RuntimeReceiptState::Completed, None),
    (RuntimeReceiptState::Completed, None),
    (RuntimeReceiptState::Completed, None),
    (
        RuntimeReceiptState::Denied,
        Some("contained_task_cancellation_identity_missing"),
    ),
    (RuntimeReceiptState::Denied, Some("runtime_shutdown_denied")),
];

/// `lab` with the word `word`, as a program binds from LAB-6S2-B1 and LAB-6S2-B2.
const fn lab_with(word: Initiator) -> ChannelBinding {
    ChannelBinding {
        channel: RuntimeChannel::Lab,
        via: None,
        initiator: Some(word),
    }
}

// A host with one instance whose `stuck_recovery`, a key an instruction may change while the
// Runtime runs, the configuration carries; the key's name.
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

// A connection bound to `binding` by its `BindChannel`, sent with the CLI's envelope. The answer
// echoes the binding, its word included: rule R leaves the answer to `BindChannel` as it is.
fn bound(host: &RuntimeHost, binding: ChannelBinding) -> TestClient {
    let mut client = TestClient::connect(host);
    let bind = client.request(RuntimeOperation::BindChannel {
        channel: binding.channel,
        via: binding.via,
        initiator: binding.initiator.map(DeclaredInitiator::from),
    });
    let answer = client.send(&bind);
    assert_eq!(
        answer.state(),
        RuntimeReceiptState::Completed,
        "{binding:?}: {answer:?}"
    );
    assert_eq!(
        answer.result(),
        Some(&RuntimeResult::ChannelBound {
            channel: binding.channel,
            via: binding.via,
            initiator: binding.initiator,
        }),
        "{binding:?}"
    );
    assert_eq!(answered(&answer), Some(binding), "{binding:?}");
    client
}

// The channel members of `receipt` as a binding; `None` when it names no channel.
fn answered(receipt: &RuntimeReceipt) -> Option<ChannelBinding> {
    receipt.channel().map(|channel| ChannelBinding {
        channel,
        via: receipt.via(),
        initiator: receipt.initiator(),
    })
}

// The receipt as JSON without what each request mints for itself or the ledger assigns it (the
// request and correlation ids, the terminal's event id and ledger position, a published fact's
// event id, and the settings' version token, an opaque hash over the rows in effect whose stamps
// hold the request id and the time) and without the channel members, which [`answered`] reads.
fn comparable(receipt: &RuntimeReceipt) -> Value {
    let mut wire = serde_json::to_value(receipt).expect("receipt JSON");
    let members = wire.as_object_mut().expect("a receipt is a JSON object");
    for member in [
        "request_id",
        "correlation_id",
        "channel",
        "via",
        "initiator",
    ] {
        members.remove(member);
    }
    if let Some(terminal) = members.get_mut("terminal").and_then(Value::as_object_mut) {
        terminal.remove("event_id");
        terminal.remove("sequence");
    }
    if let Some(result) = members.get_mut("result").and_then(Value::as_object_mut) {
        result.remove("event_id");
        result.remove("config_version");
    }
    wire
}

/// What the comparison reads of one record of a request.
#[derive(Debug, PartialEq, Eq)]
struct Recorded {
    event_type: EventType,
    source: EventSource,
    module: OriginModule,
    actor: EventActor,
    channel: Option<RuntimeChannel>,
    via: Option<ChannelVia>,
}

// The channel and `via` a command record names; none for every other record.
fn command_channel(payload: &EventPayload) -> (Option<RuntimeChannel>, Option<ChannelVia>) {
    match payload {
        EventPayload::Command(CommandPayload::Received(record)) => (record.channel(), record.via()),
        EventPayload::Command(CommandPayload::Validated(record)) => {
            (record.channel(), record.via())
        }
        EventPayload::Command(CommandPayload::Rejected(record)) => (record.channel(), record.via()),
        _ => (None, None),
    }
}

// Every record of `request`, in ledger order.
fn records(host: &RuntimeHost, request: &RuntimeRequest) -> Vec<Recorded> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
        .into_iter()
        .filter(|event| event.links().request_id() == Some(&request.request_id()))
        .map(|event| {
            let (channel, via) = command_channel(event.payload());
            Recorded {
                event_type: event.event_type(),
                source: event.origin().source(),
                module: event.origin().module(),
                actor: event.origin().actor(),
                channel,
                via,
            }
        })
        .collect()
}

// The channel of `key`'s `host.config_overrides` row, read on `reader`.
fn settings_row_channel(reader: &mut TestClient, key: &str) -> Option<FactScalar> {
    let request = reader.request(RuntimeOperation::RuntimeFactSnapshot);
    let receipt = reader.send(&request);
    let Some(RuntimeResult::RuntimeFactSnapshot { snapshot, .. }) = receipt.result() else {
        panic!("the runtime fact snapshot: {receipt:?}");
    };
    let record = snapshot
        .records
        .iter()
        .find(|record| record.key == CONFIG_OVERRIDES_FACT_KEY)
        .expect("host.config_overrides");
    let ContractFactValue::RecordList(rows) = &record.value else {
        panic!("host.config_overrides is a record list");
    };
    let named = FactScalar::String(key.to_owned());
    rows.iter()
        .find(|row| row.get(OVERRIDE_CONFIG_KEY) == Some(&named))
        .and_then(|row| row.get(OVERRIDE_CHANNEL))
        .cloned()
}

fn set_stuck_recovery(key: &str) -> RuntimeOperation {
    RuntimeOperation::ConfigureSettings {
        expected_version: None,
        changes: vec![ConfigSettingChange {
            key: key.to_owned(),
            value: Some(json!(false)),
        }],
    }
}

// A priority offset of the instance, observed at `observed_at_unix_ms`.
fn publish_offset(observed_at_unix_ms: u64) -> RuntimeOperation {
    let record = FactRecord::priority_offset(
        FactScope::Instance {
            instance_id: ALIAS.to_owned(),
        },
        TASK,
        250,
        observed_at_unix_ms,
        "neutral.test.offset",
    )
    .expect("priority offset record");
    RuntimeOperation::PublishFacts {
        observation: FactObservation {
            records: vec![record],
        },
    }
}

// The requests each connection of a pair sends, in this order: a setting a request may change
// while the Runtime runs; a priority offset, observed at `observed_at_unix_ms` (each
// connection's own: an equal observation is a replay, which appends nothing); a client action; a
// stop of `run`, a run no instance runs, refused after its command record; and last a shutdown of
// `target`, another process, which the host records and refuses, so that it keeps running for
// the pair's other connection.
fn compared_requests(
    key: &str,
    run: RequestId,
    target: RuntimeShutdownTarget,
    observed_at_unix_ms: u64,
) -> [RuntimeOperation; 5] {
    [
        set_stuck_recovery(key),
        publish_offset(observed_at_unix_ms),
        RuntimeOperation::RecordClientAction {
            action: ClientActionRecord::new(
                "settings",
                "refresh",
                ClientActionKind::Button,
                None,
                None,
            )
            .expect("client action"),
        },
        RuntimeOperation::StopInstanceRun {
            instance_alias: ALIAS.to_owned(),
            request_id: run,
            wait_ms: 0,
        },
        RuntimeOperation::RequestShutdown { target },
    ]
}

// One host, two connections: `today`, bound as a program binds on 0.12.58, and one bound `lab`
// with `word`, the word that program will declare; both send [`compared_requests`] with the
// envelope `(actor, source)`, the request of `today` first. Each pair is answered alike, its
// receipts equal but for the ids and the channel members; `today`'s receipt names `today` and
// the other's `receipted` (rule R); the two requests write the same records, a command record
// naming `today`'s channel and `via`; and the settings row names `today`'s channel after either.
fn assert_recorded_and_answered_as_today(
    today: ChannelBinding,
    word: Initiator,
    (actor, source): (EventActor, EventSource),
    receipted: ChannelBinding,
) {
    let root = TempDir::new().expect("tempdir");
    let (host, key) = start_with_setting(&root);
    let mut before = bound(&host, today);
    let mut after = bound(&host, lab_with(word));
    let mut reader = TestClient::connect(&host);
    // A request id no run has, never sent, and the target of another process.
    let run = reader.request(RuntimeOperation::Health).request_id();
    let live = host.runtime_info().shutdown_target();
    let other = RuntimeShutdownTarget {
        pid: live.pid.wrapping_add(1),
        ..live
    };
    let row = Some(FactScalar::String(today.channel.as_str().to_owned()));
    let pairs = compared_requests(&key, run, other, 1)
        .into_iter()
        .zip(compared_requests(&key, run, other, 2))
        .zip(ANSWERS);
    for ((on_today, on_word), (state, code)) in pairs {
        let kind = on_today.kind();
        let today_request = request_as(&before, actor, source, on_today);
        let today_receipt = before.send(&today_request);
        assert_eq!(
            settings_row_channel(&mut reader, &key),
            row,
            "{kind:?}: the settings row after today's request"
        );
        let word_request = request_as(&after, actor, source, on_word);
        let word_receipt = after.send(&word_request);
        assert_eq!(
            settings_row_channel(&mut reader, &key),
            row,
            "{kind:?}: the settings row after the request on `lab` with {word:?}"
        );

        for receipt in [&today_receipt, &word_receipt] {
            assert_eq!(receipt.state(), state, "{kind:?}: {receipt:?}");
            if let Some(code) = code {
                assert_eq!(receipt.outcome().code().as_str(), code, "{kind:?}");
            }
        }
        assert_eq!(
            comparable(&word_receipt),
            comparable(&today_receipt),
            "{kind:?}"
        );
        assert_eq!(answered(&today_receipt), Some(today), "{kind:?}");
        assert_eq!(answered(&word_receipt), Some(receipted), "{kind:?}");

        let today_records = records(&host, &today_request);
        assert!(!today_records.is_empty(), "{kind:?}: no record");
        assert_eq!(records(&host, &word_request), today_records, "{kind:?}");
        if matches!(
            kind,
            RuntimeOperationKind::ConfigureSettings | RuntimeOperationKind::StopInstanceRun
        ) {
            assert!(
                today_records.iter().any(|record| {
                    record.event_type == EventType::CommandReceived
                        && (record.channel, record.via) == (Some(today.channel), today.via)
                }),
                "{kind:?}: {today_records:?}"
            );
        }
    }
    drop(before);
    drop(after);
    drop(reader);
    host.close().expect("close host");
}

// actingctl's row: a program that binds `cli` with the CLI's envelope and will bind `lab` with
// `Lab-CLI` is recorded `cli` and answered `cli`, without a word.
#[test]
fn lab6s2m_lab_cli_with_the_clis_envelope_is_recorded_and_answered_as_cli() {
    assert_recorded_and_answered_as_today(
        ChannelBinding::new(RuntimeChannel::Cli),
        Initiator::LabCli,
        (EventActor::Cli, EventSource::Cli),
        ChannelBinding::new(RuntimeChannel::Cli),
    );
}

// The MCP servers' row: a program that binds `mcp` with the CLI's envelope and will bind `lab`
// with `Lab-MCP` is recorded `mcp` and answered `mcp`, without a word.
#[test]
fn lab6s2m_lab_mcp_with_the_clis_envelope_is_recorded_and_answered_as_mcp() {
    assert_recorded_and_answered_as_today(
        ChannelBinding::new(RuntimeChannel::Mcp),
        Initiator::LabMcp,
        (EventActor::Cli, EventSource::Cli),
        ChannelBinding::new(RuntimeChannel::Mcp),
    );
}

// actinglab's row: a program that binds `lab` without a word with the Lab's envelope and will
// bind `lab` with `Lab-CLI` is recorded `lab`; its receipts carry the word (LAB-6S1a).
#[test]
fn lab6s2m_lab_cli_with_the_labs_envelope_is_recorded_as_lab_without_a_word() {
    assert_recorded_and_answered_as_today(
        ChannelBinding::new(RuntimeChannel::Lab),
        Initiator::LabCli,
        (EventActor::Lab, EventSource::Lab),
        lab_with(Initiator::LabCli),
    );
}

// The MCP servers' Lab children: a program that binds `lab` with `via` `mcp` and the Lab's
// envelope and will bind `lab` with `Lab-MCP` is recorded `lab` with `via` `mcp`; its receipts
// carry the word (LAB-6S1a), which takes the place of `via` on the wire.
#[test]
fn lab6s2m_lab_mcp_with_the_labs_envelope_is_recorded_as_lab_via_mcp() {
    assert_recorded_and_answered_as_today(
        ChannelBinding {
            channel: RuntimeChannel::Lab,
            via: Some(ChannelVia::Mcp),
            initiator: None,
        },
        Initiator::LabMcp,
        (EventActor::Lab, EventSource::Lab),
        lab_with(Initiator::LabMcp),
    );
}

// The receipt rule R: on `lab` with each of the five words, a request with the CLI's envelope or
// an agent adapter's answers with the channel the host records for it (`mcp` under `Lab-MCP`,
// `cli` under every other word), without `via` and without an `initiator` member; a request of
// the same connection with the Lab's envelope answers with `lab` and the word, as from LAB-6S1a.
// The answer to `BindChannel` carries the binding with its word, the first one's and a refused
// second one's.
#[test]
fn lab6s2m_the_receipt_rule_on_a_lab_binding_that_declares_a_word() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    for word in LAB_WORDS {
        let binding = lab_with(word);
        let mut client = bound(&host, binding);
        let recorded = if word == Initiator::LabMcp {
            RuntimeChannel::Mcp
        } else {
            RuntimeChannel::Cli
        };
        for (actor, source) in [
            (EventActor::Cli, EventSource::Cli),
            (EventActor::Agent, EventSource::Adapter),
        ] {
            for operation in [RuntimeOperation::Status, RuntimeOperation::Health] {
                let request = request_as(&client, actor, source, operation);
                let receipt = client.send(&request);
                assert_eq!(
                    receipt.state(),
                    RuntimeReceiptState::Completed,
                    "{word:?} {source:?}: {receipt:?}"
                );
                assert_eq!(
                    answered(&receipt),
                    Some(ChannelBinding::new(recorded)),
                    "{word:?} {source:?}"
                );
                let wire = serde_json::to_value(&receipt).expect("receipt JSON");
                assert_eq!(wire["channel"], json!(recorded.as_str()), "{word:?}");
                for member in ["via", "initiator"] {
                    assert!(
                        wire.get(member).is_none(),
                        "{word:?} {source:?}: the receipt names {member}"
                    );
                }
            }
        }

        let from_lab = request_as(
            &client,
            EventActor::Lab,
            EventSource::Lab,
            RuntimeOperation::Status,
        );
        let receipt = client.send(&from_lab);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{word:?}: {receipt:?}"
        );
        assert_eq!(answered(&receipt), Some(binding), "{word:?}");
        let wire = serde_json::to_value(&receipt).expect("receipt JSON");
        assert_eq!(wire["channel"], json!("lab"), "{word:?}");
        assert_eq!(wire["initiator"], json!(word.as_str()), "{word:?}");
        assert!(wire.get("via").is_none(), "{word:?}");

        let again = client.request(RuntimeOperation::BindChannel {
            channel: RuntimeChannel::Cli,
            via: None,
            initiator: None,
        });
        let again = client.send(&again);
        assert_eq!(
            again.outcome().code().as_str(),
            "channel_binding_exists",
            "{word:?}"
        );
        assert_eq!(again.outcome().value("channel"), Some(&json!("lab")));
        assert_eq!(answered(&again), Some(binding), "{word:?}");
    }
    host.close().expect("close host");
}

// A binding without a word is recorded and answered as before: one setting on each of `lab`
// (with the CLI's envelope), `lab` with `via` `mcp` (the CLI's), `cli` (the Lab's), `mcp` (the
// Lab's) and an unbound connection (the CLI's, which binds it `cli`) completes, answers with the
// connection's own channel and `via` and no `initiator` member, and its `command.received`
// names them; on `ui` (the CLI's envelope) the gate refuses it, the key having no console tier,
// and its one `command.rejected` names `ui`.
#[test]
fn lab6s2m_a_binding_without_a_word_is_answered_and_recorded_as_before() {
    let root = TempDir::new().expect("tempdir");
    let (host, key) = start_with_setting(&root);
    let lab_via_mcp = ChannelBinding {
        channel: RuntimeChannel::Lab,
        via: Some(ChannelVia::Mcp),
        initiator: None,
    };
    let cli = (EventActor::Cli, EventSource::Cli);
    let lab = (EventActor::Lab, EventSource::Lab);
    for (binding, (actor, source), expected) in [
        (
            Some(ChannelBinding::new(RuntimeChannel::Lab)),
            cli,
            ChannelBinding::new(RuntimeChannel::Lab),
        ),
        (Some(lab_via_mcp), cli, lab_via_mcp),
        (
            Some(ChannelBinding::new(RuntimeChannel::Cli)),
            lab,
            ChannelBinding::new(RuntimeChannel::Cli),
        ),
        (
            Some(ChannelBinding::new(RuntimeChannel::Mcp)),
            lab,
            ChannelBinding::new(RuntimeChannel::Mcp),
        ),
        (
            Some(ChannelBinding::new(RuntimeChannel::Ui)),
            cli,
            ChannelBinding::new(RuntimeChannel::Ui),
        ),
        (None, cli, ChannelBinding::new(RuntimeChannel::Cli)),
    ] {
        let mut client = match binding {
            Some(binding) => bound(&host, binding),
            None => TestClient::connect(&host),
        };
        let request = request_as(&client, actor, source, set_stuck_recovery(&key));
        let receipt = client.send(&request);
        assert_eq!(answered(&receipt), Some(expected), "{binding:?}");
        let wire = serde_json::to_value(&receipt).expect("receipt JSON");
        assert!(wire.get("initiator").is_none(), "{binding:?}");
        let on_ui = expected.channel == RuntimeChannel::Ui;
        let (state, written) = if on_ui {
            assert_eq!(
                receipt.outcome().code().as_str(),
                "channel_request_refused",
                "{receipt:?}"
            );
            (RuntimeReceiptState::Denied, EventType::CommandRejected)
        } else {
            (RuntimeReceiptState::Completed, EventType::CommandReceived)
        };
        assert_eq!(receipt.state(), state, "{binding:?}: {receipt:?}");
        let found = records(&host, &request);
        let named = found
            .iter()
            .filter(|record| record.event_type == written)
            .map(|record| (record.channel, record.via))
            .collect::<Vec<_>>();
        assert_eq!(
            named,
            [(Some(expected.channel), expected.via)],
            "{binding:?}: {found:?}"
        );
    }
    host.close().expect("close host");
}

// The UI gate never judges a mapped connection: M leaves `ui` as it is and maps no binding to
// `ui` for any client envelope; a priority offset, a kind off the UI list, is refused on a
// connection bound to `ui` and, with the console's envelope, served on `lab` with each word,
// answered with the recorded channel and recorded with the console's origin.
#[test]
fn lab6s2m_the_ui_gate_never_judges_a_mapped_connection() {
    let ui = ChannelBinding::new(RuntimeChannel::Ui);
    for source in [
        EventSource::Cli,
        EventSource::Ui,
        EventSource::Lab,
        EventSource::Adapter,
    ] {
        assert_eq!(ui.recorded(source), ui, "{source:?}");
        for word in LAB_WORDS {
            assert_ne!(
                lab_with(word).recorded(source).channel,
                RuntimeChannel::Ui,
                "{word:?} {source:?}"
            );
        }
    }

    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    assert!(!ui_allows(RuntimeOperationKind::PublishFacts));
    let mut observed_at = 1;
    let mut console = bound(&host, ui);
    let refused = request_as(
        &console,
        EventActor::User,
        EventSource::Ui,
        publish_offset(observed_at),
    );
    let refused = console.send(&refused);
    assert_eq!(
        (refused.state(), refused.outcome().code().as_str()),
        (RuntimeReceiptState::Denied, "channel_request_refused"),
        "{refused:?}"
    );

    for word in LAB_WORDS {
        observed_at += 1;
        let mut client = bound(&host, lab_with(word));
        let request = request_as(
            &client,
            EventActor::User,
            EventSource::Ui,
            publish_offset(observed_at),
        );
        let served = client.send(&request);
        assert_eq!(
            served.state(),
            RuntimeReceiptState::Completed,
            "{word:?}: {served:?}"
        );
        let recorded = if word == Initiator::LabMcp {
            RuntimeChannel::Mcp
        } else {
            RuntimeChannel::Cli
        };
        assert_eq!(
            answered(&served),
            Some(ChannelBinding::new(recorded)),
            "{word:?}"
        );
        let found = records(&host, &request);
        assert!(
            found.iter().any(|record| {
                record.event_type == EventType::FactPublished
                    && (record.source, record.actor) == (EventSource::Ui, EventActor::User)
            }),
            "{word:?}: {found:?}"
        );
    }
    drop(console);
    host.close().expect("close host");
}
