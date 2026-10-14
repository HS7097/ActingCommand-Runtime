// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #389 MCP-4, register W2 (MCP-0 command table v1.1 §4.4; contracts/v012-register.md
// §3): the general read InstanceFactSnapshot answers one instance's facts, its own and those of
// the server and the game its configuration binds it to, each with its observation and expiry
// times, and the server and game it resolved. Like a status read (register W1,
// contracts/ledger-observer.md §13) it appends no ledger record, its source names the ledger's
// committed head, and no ledger position or snapshot id of its own goes on the wire. An alias
// the Runtime does not hold is refused instance_unknown; a reply larger than one IPC frame is
// refused instance_fact_snapshot_too_large_for_frame, and the Runtime goes on.
//
// The ledger head is read through an event query, which appends nothing; no lease is held, so
// nothing else writes between two reads.

use super::gate_383_ao4q::ledger_head;
use super::*;

// The server and the game the test configuration binds POLICY_INSTANCE_ALIAS to.
const SERVER: &str = "fixture-server-a";
const GAME: &str = "fixture-game-a";
const OTHER_SERVER: &str = "fixture-server-b";
// A frame bound that holds a request and a refusal but not the answer of FRAME_RECORDS records.
const FRAME_BYTES: usize = 2_048;
const FRAME_RECORDS: i64 = 12;

fn own_scope() -> FactScope {
    FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    }
}

fn start(config: RuntimeHostConfig) -> RuntimeHost {
    RuntimeHost::start(
        config,
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host")
}

fn instance_fact_read(client: &TestClient, instance: &str) -> RuntimeRequest {
    client.request(RuntimeOperation::InstanceFactSnapshot {
        instance: instance.to_owned(),
    })
}

// The answered record of the published record's scope and key.
fn answered<'a>(records: &'a [FactRecord], published: &FactRecord) -> Option<&'a FactRecord> {
    records
        .iter()
        .find(|record| record.scope == published.scope && record.key == published.key)
}

#[test]
fn mcp4_an_instance_fact_read_answers_the_instance_s_facts_and_appends_nothing() {
    let root = TempDir::new().expect("tempdir");
    let host = start(config(&root));
    let own = stored_fact(
        own_scope(),
        "inventory.items",
        ContractFactValue::RecordList(Vec::new()),
        "snapshot:mcp4-instance",
        Vec::new(),
    );
    let server = stored_fact(
        FactScope::Server {
            server_id: SERVER.to_owned(),
        },
        "env.ui_theme",
        ContractFactValue::String("Neutral".to_owned()),
        "snapshot:mcp4-server",
        Vec::new(),
    );
    let game = stored_fact(
        FactScope::Game {
            game_id: GAME.to_owned(),
        },
        "timeline.window_open",
        ContractFactValue::Boolean(true),
        "snapshot:mcp4-game",
        Vec::new(),
    );
    let other_server = stored_fact(
        FactScope::Server {
            server_id: OTHER_SERVER.to_owned(),
        },
        "env.ui_theme",
        ContractFactValue::String("Neutral".to_owned()),
        "snapshot:mcp4-other-server",
        Vec::new(),
    );
    for record in [&own, &server, &game, &other_server] {
        host.publish_fact(record.clone()).expect("publish a fact");
    }
    let mut client = TestClient::connect(&host);
    let mut reader = TestClient::connect(&host);

    let head = ledger_head(&mut reader);
    let request = instance_fact_read(&client, POLICY_INSTANCE_ALIAS);
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert_eq!(
        receipt.outcome().code().as_str(),
        "request_instance_fact_snapshot_completed"
    );
    let Some(RuntimeResult::InstanceFactSnapshot { snapshot, source }) = receipt.result() else {
        panic!(
            "expected an instance fact snapshot, got {:?}",
            receipt.result()
        );
    };

    // The resolved scope: the instance and the server and game its configuration names.
    assert_eq!(snapshot.instance_alias, POLICY_INSTANCE_ALIAS);
    assert_eq!(snapshot.server_id.as_deref(), Some(SERVER));
    assert_eq!(snapshot.game_id.as_deref(), Some(GAME));
    // Its own, its server's and its game's records, with their content, observation and expiry
    // times; another server's record does not apply.
    for published in [&own, &server, &game] {
        let found = answered(&snapshot.records, published)
            .unwrap_or_else(|| panic!("{published:?} is not answered: {:?}", snapshot.records));
        assert_eq!(
            (
                &found.content,
                found.observed_at_unix_ms,
                found.expires_at_unix_ms
            ),
            (
                &published.content,
                published.observed_at_unix_ms,
                published.expires_at_unix_ms
            ),
            "{published:?}"
        );
    }
    assert!(
        answered(&snapshot.records, &other_server).is_none(),
        "another server's record is answered: {:?}",
        snapshot.records
    );
    assert!(
        snapshot.records.iter().all(|record| match &record.scope {
            FactScope::Instance { instance_id } => instance_id == POLICY_INSTANCE_ALIAS,
            FactScope::Server { server_id } => server_id == SERVER,
            FactScope::Game { game_id } => game_id == GAME,
        }),
        "a record that does not apply to the instance is answered: {:?}",
        snapshot.records
    );
    // No ledger position and no snapshot id of the read go on the wire.
    let wire = serde_json::to_value(receipt.result()).expect("result JSON");
    assert!(
        wire["snapshot"].get("ledger_position").is_none()
            && wire["snapshot"].get("snapshot_id").is_none(),
        "{wire}"
    );

    // Like a status read: the source names the committed head, and nothing is appended.
    assert_eq!(source.ledger_committed_head, head);
    assert!(source.sampled_started_at_unix_ms <= source.sampled_completed_at_unix_ms);
    assert_eq!(ledger_head(&mut reader), head, "the read appends a record");
    assert!(
        projected_events(
            &mut reader,
            EventQuery {
                request_id: Some(request.request_id()),
                ..EventQuery::default()
            },
        )
        .is_empty(),
        "the read leaves a record of its request"
    );

    // An alias the Runtime does not hold.
    let unknown = client.send(&instance_fact_read(&client, "fixture-instance-unknown"));
    assert_eq!(unknown.state(), RuntimeReceiptState::Denied);
    assert_eq!(unknown.outcome().code().as_str(), "instance_unknown");

    drop(reader);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn mcp4_an_instance_fact_read_larger_than_one_frame_is_refused_and_the_runtime_goes_on() {
    let root = TempDir::new().expect("tempdir");
    let host = start(config(&root).with_maximum_frame_bytes(FRAME_BYTES));
    for index in 0..FRAME_RECORDS {
        host.publish_fact(stored_fact(
            own_scope(),
            &format!("inventory.mcp4_{index}"),
            ContractFactValue::Integer(index),
            &format!("snapshot:mcp4-{index}"),
            Vec::new(),
        ))
        .expect("publish a fact");
    }
    let mut client = TestClient::connect(&host);

    let refused = client.send(&instance_fact_read(&client, POLICY_INSTANCE_ALIAS));
    assert_eq!(refused.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        refused.outcome().code().as_str(),
        "instance_fact_snapshot_too_large_for_frame"
    );
    // The refusal is no failure of the Runtime: the next request is answered.
    let health = client.send(&client.request(RuntimeOperation::Health));
    assert_eq!(health.state(), RuntimeReceiptState::Completed);

    drop(client);
    host.close().expect("close host");
}
