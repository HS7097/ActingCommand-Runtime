// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S1a (register §1.287, "Slice LAB-6S1a"; contracts/lab-two-entries.md §2.2;
// `runtime-client` 8): a Lab session configured with an initiator declares it in the one
// handshake's `BindChannel`, and the handshake takes the Runtime's `ChannelBound` only when it
// echoes both the channel and the word: an echo that differs in the word is refused as one that
// differs in the channel is (`runtime_result_unexpected`). The reconnect of `send_or_dropped` and
// the player hold's temporary client declare the same word.

use super::*;
use crate::test_support::{ScriptedAnswer, ScriptedConnection, ScriptedRuntime};
use actingcommand_contract::{DeclaredInitiator, RuntimeControlPlaneStatus};

const LAB_WORDS: [Initiator; 5] = [
    Initiator::LabCli,
    Initiator::LabMcp,
    Initiator::LabAuthoring,
    Initiator::LabRescue,
    Initiator::LabWatchdog,
];

fn lab_config(root: &TempDir, initiator: Option<Initiator>) -> RuntimeClientConfig {
    let config = RuntimeClientConfig::new(
        root.path(),
        RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    );
    match initiator {
        Some(word) => config.with_initiator(word),
        None => config,
    }
}

const fn lab_binding(initiator: Option<Initiator>) -> ChannelBinding {
    ChannelBinding {
        channel: RuntimeChannel::Lab,
        via: None,
        initiator,
    }
}

// The binding a received `BindChannel` declares.
fn bound_as(request: &RuntimeRequest) -> ChannelBinding {
    let RuntimeOperation::BindChannel {
        channel,
        via,
        initiator,
    } = request.operation()
    else {
        panic!("BindChannel, got {:?}", request.operation());
    };
    ChannelBinding {
        channel: *channel,
        via: *via,
        initiator: initiator.as_ref().and_then(DeclaredInitiator::initiator),
    }
}

// The error `RuntimeClient::connect` fails with when the Runtime echoes `echoed` to a Lab session
// that declares `declared`; `None` when it connects.
fn connect_against_echo(
    declared: Option<Initiator>,
    echoed: Option<Initiator>,
) -> Option<RuntimeClientError> {
    let root = TempDir::new().expect("tempdir");
    let server = scripted_runtime(&root, move |listener, owner_epoch| {
        let (mut stream, _) = listener.accept().expect("accept the client");
        let health = read_scripted_request(&mut stream);
        assert!(matches!(health.operation(), RuntimeOperation::Health));
        write_scripted_result(&mut stream, &health, RuntimeResult::Health { owner_epoch });
        let bind = read_scripted_request(&mut stream);
        assert_eq!(bound_as(&bind), lab_binding(declared));
        write_scripted_result(
            &mut stream,
            &bind,
            RuntimeResult::ChannelBound {
                channel: RuntimeChannel::Lab,
                via: None,
                initiator: echoed,
            },
        );
        // Until the client hangs up.
        let mut rest = Vec::new();
        let _ = stream.read_to_end(&mut rest);
    });
    // A client that connected is dropped here, so the script sees it hang up.
    let refused = RuntimeClient::connect(lab_config(&root, declared)).err();
    server
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    refused
}

// The handshake with each of the five Lab words: `BindChannel` names `lab` and the word as the
// contract writes it, and the echo of both is taken.
#[test]
fn lab6s1a_a_lab_session_declares_its_word_and_takes_its_echo() {
    for word in LAB_WORDS {
        let root = TempDir::new().expect("tempdir");
        let runtime = ScriptedRuntime::start(root.path(), |_| vec![ScriptedConnection::new()]);
        let client =
            RuntimeClient::connect(lab_config(&root, Some(word))).expect("the echo is taken");
        assert!(!client.channel_unsupported().expect("the client's state"));
        drop(client);
        let record = runtime.finish();
        let bind = &record.connections[0].requests[1];
        assert_eq!(bound_as(bind), lab_binding(Some(word)));
        assert_eq!(
            serde_json::to_value(bind.operation()).expect("request JSON")["initiator"],
            json!(word.as_str())
        );
    }
}

// An echo without the word, with another word, or with a word the session did not declare fails
// the connect as an echo of another channel does.
#[test]
fn lab6s1a_an_echo_that_differs_in_the_word_is_refused() {
    for (declared, echoed) in [
        (Some(Initiator::LabCli), None),
        (Some(Initiator::LabCli), Some(Initiator::LabMcp)),
        (None, Some(Initiator::LabCli)),
    ] {
        let error =
            connect_against_echo(declared, echoed).expect("an echo that differs is refused");
        assert_eq!(
            error.code(),
            ClientCode::RuntimeResultUnexpected.code(),
            "{declared:?} {echoed:?}: {error:?}"
        );
    }
}

// The reconnect after a dropped request binds the same channel and word; so does the temporary
// client the player hold's settlement reads another owner epoch's status through.
#[test]
fn lab6s1a_a_reconnect_and_the_temporary_client_declare_the_same_word() {
    let expected = lab_binding(Some(Initiator::LabRescue));

    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![
            ScriptedConnection::new().request(|_| ScriptedAnswer::Close),
            ScriptedConnection::new(),
        ]
    });
    let client =
        RuntimeClient::connect(lab_config(&root, expected.initiator)).expect("runtime client");
    let error = client
        .resource_target_view("node.a")
        .expect_err("the dropped request is not answered");
    assert_eq!(error.code().as_str(), "runtime_operation_unsupported");
    drop(client);
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "{:?}", record.unscripted);
    assert_eq!(
        record
            .connections
            .iter()
            .map(|connection| bound_as(&connection.requests[1]))
            .collect::<Vec<_>>(),
        vec![expected, expected]
    );

    let root = TempDir::new().expect("tempdir");
    let old = ScriptedRuntime::start(root.path(), |_| {
        vec![ScriptedConnection::new().request(|_| ScriptedAnswer::Close)]
    });
    let client =
        RuntimeClient::connect(lab_config(&root, expected.initiator)).expect("runtime client");
    // The restart writes the new runtime-info before the frame goes out.
    let new = ScriptedRuntime::start(root.path(), |owner_epoch| {
        vec![ScriptedConnection::new().request(move |request| {
            assert!(
                matches!(request.operation(), RuntimeOperation::Status),
                "a status read, got {:?}",
                request.operation()
            );
            ScriptedAnswer::completed(
                request,
                RuntimeResult::Status {
                    status: RuntimeControlPlaneStatus::new(owner_epoch, Vec::new())
                        .expect("scripted status"),
                },
            )
        })]
    });
    let error = client
        .acquire_player_hold("node.a", 1_000)
        .expect_err("the frame was dropped");
    let latched = client.status().expect_err("the handle stays latched");
    assert_eq!(
        latched.code(),
        ClientCode::RuntimeOwnerEpochChanged.code(),
        "{error:?}"
    );
    drop(client);
    assert_eq!(bound_as(&old.finish().connections[0].requests[1]), expected);
    let record = new.finish();
    assert!(record.unscripted.is_empty(), "{:?}", record.unscripted);
    assert_eq!(bound_as(&record.connections[0].requests[1]), expected);
}
