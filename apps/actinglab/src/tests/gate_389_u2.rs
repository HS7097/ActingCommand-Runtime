// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #389 U-2 (unified interface model v2.2.3 §3.2 rules 6 and 7, "Fixing actinglab's Cli
// label"; §8.2 TU-5, TU-6): every Runtime connection actinglab opens (the input proxy behind its
// taps, `session app`, the stream's input forwarding and every other command) binds the Lab
// channel and sends the envelope (Lab, Lab); `--via` names the counter that forwarded the work
// and is recorded beside the channel.

use super::*;
use actingcommand_contract::{
    CommandPayload, ConfigSettingChange, EventActor, EventPayload, EventQuery, EventSource,
    EventType, ProjectionPayload, ProjectionProfile,
};
use actingcommand_runtime_client::RuntimeChannel;

#[test]
fn u2_via_names_the_counter_that_forwarded_the_work() {
    for (arguments, expected) in [
        (vec!["--via", "mcp", "status"], Some(ChannelVia::Mcp)),
        (vec!["--via", "ui", "status"], Some(ChannelVia::Ui)),
        (vec!["status"], None),
    ] {
        let Ok(invocation) = parse_invocation(arguments.clone(), true) else {
            panic!("{arguments:?} parses");
        };
        assert_eq!(invocation.global.via, expected, "{arguments:?}");
    }
    assert!(parse_invocation(["--via", "cli", "status"], true).is_err());
}

#[test]
fn u2_an_actinglab_connection_binds_the_lab_channel_with_a_lab_envelope_and_its_via() {
    let root = TempDir::new().expect("state root");
    let host = start_authoring_runtime(root.path());
    for via in [None, Some(ChannelVia::Mcp)] {
        let client =
            RuntimeClient::connect(lab_client_config_via(root.path(), via)).expect("connect");
        // A key no configuration holds: refused after its command is recorded.
        let refused = client.configure_settings(vec![ConfigSettingChange {
            key: "neutral_section.unknown_key".to_owned(),
            value: Some(serde_json::json!(true)),
        }]);
        assert!(refused.is_err());
    }
    let reader =
        RuntimeClient::connect(lab_client_config_via(root.path(), None)).expect("connect reader");
    let received = reader
        .query_events(
            EventQuery {
                event_type: Some(EventType::CommandReceived),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("command records");
    let channels = received
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
        .collect::<Vec<_>>();
    assert_eq!(
        channels,
        vec![
            (Some(RuntimeChannel::Lab), None),
            (Some(RuntimeChannel::Lab), Some(ChannelVia::Mcp)),
        ]
    );
    let requests = reader
        .query_events(
            EventQuery {
                event_type: Some(EventType::LabRequest),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("lab requests");
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(
            (request.origin.actor(), request.origin.source()),
            (EventActor::Lab, EventSource::Lab)
        );
    }
    drop(reader);
    host.close().expect("close the Runtime");
}
