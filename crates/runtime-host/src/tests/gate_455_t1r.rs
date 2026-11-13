// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 T1-R (register §1.351; contracts/lab-two-entries.md §3;
// contracts/resource-targets.md, "Entry and receipts"; contracts/runtime-interface.md, "Workbench
// request kinds", the row ApplyResourceTargets.expected_version; coordinator work order
// rd-1114/brief-455-t1r.md): the console's entry takes the resource target document, with its
// version check. Through the host on a manual clock and a fake device, the fixture catalog active
// (its one task produces the fixture pool, observed through a `resource.` fact):
// - the `ui` channel takes `ApplyResourceTargets` and the record is written; every kind is taken
//   on `ui` exactly when it is a handshake kind or the operation table's `ui_kinds` lists it, so
//   every kind off the table stays refused;
// - `expected_version` is compared with the version of the instance's record (the sequence of
//   the `fact.published` that holds it, 0 without one): the equal version applies, another is
//   `fact_observation_version_conflict` with `current_version` and appends nothing; without the
//   member nothing is compared;
// - two writers: a write with a version read before the other writer's is refused until its
//   writer reads again;
// - a replay with the record's version appends nothing and answers as a replay without it.
// Every alias, pool and id is the fixture's or neutral.

use super::gate_383_ao4q::ledger_head;
use super::*;
use crate::host::ui_gate::ui_takes_kind;
use actingcommand_contract::{
    EventId, FactPayload, RESOURCE_TARGETS_FACT_KEY, ResourceTargetView, ResourceTargetsApplied,
    RuntimeChannel, RuntimeOperationKind as Kind,
};
use serde_json::{Value, json};

const ALIAS: &str = POLICY_INSTANCE_ALIAS;
const START: u64 = POLICY_NOW_UNIX_MS;
const DAY_MS: u64 = 86_400_000;
/// The fixture catalog's pool, observed through `resource.primary`; its one task produces it.
const POOL: &str = "fixture-pool-a";
const APPLIED: &str = "request_apply_resource_targets_completed";
const CONFLICT: &str = "fact_observation_version_conflict";
/// The operation table, read as the file it is.
const OPERATION_TABLE: &str = include_str!("../../../../contracts/lab-operations.json");

/// A request's envelope on a connection.
type Envelope = fn(&TestClient, RuntimeOperation) -> RuntimeRequest;

/// The operation table's `ui_kinds`: each row's kind, with the row.
fn table_ui_kinds() -> Vec<(Kind, Value)> {
    let table: Value = serde_json::from_str(OPERATION_TABLE).expect("the operation table");
    table["ui_kinds"]
        .as_array()
        .expect("the table's ui_kinds")
        .iter()
        .map(|row| {
            let name = row["kind"].as_str().expect("a row's kind");
            let kind = Kind::ALL
                .iter()
                .copied()
                .find(|kind| format!("{kind:?}") == name)
                .unwrap_or_else(|| panic!("{name} is no request kind"));
            (kind, row.clone())
        })
        .collect()
}

/// A v2 document of the fixture instance: one target on the fixture pool, at least `amount`, with
/// its own scale and importance (the pool declares no valuation), valid for a day from the
/// fixture's time.
fn document(amount: u64) -> String {
    json!({
        "schema_version": "actingcommand.resource-targets.v2",
        "instance": ALIAS,
        "valid_until_unix_ms": START + DAY_MS,
        "targets": [{
            "id": "neutral.floor",
            "resource": POOL,
            "condition": {"kind": "at_least", "amount": amount},
            "scale": 10,
            "importance_milli": 500,
            "apply": {"mode": "adjust", "weight": "score_stage"}
        }]
    })
    .to_string()
}

struct Fixture {
    _root: TempDir,
    clock: Arc<ManualRuntimeClock>,
    host: RuntimeHost,
}

impl Fixture {
    /// A host on the fixture instance with the fixture catalog active and the Runtime clock at the
    /// fixture's time.
    fn start() -> Self {
        let root = TempDir::new().expect("tempdir");
        let clock = Arc::new(ManualRuntimeClock::new(START, 0));
        let host = RuntimeHost::start(
            config(&root).with_runtime_clock(clock.clone()),
            Arc::new(FakeProvider::one(
                ALIAS,
                instance_id(),
                Arc::new(FakeState::default()),
            )),
        )
        .expect("runtime host");
        host.activate_policy_catalog(&policy_sources(1))
            .expect("activate the catalog");
        Self {
            _root: root,
            clock,
            host,
        }
    }

    /// A connection bound to `channel` by its `BindChannel`.
    fn bound(&self, channel: RuntimeChannel) -> TestClient {
        let mut client = TestClient::connect(&self.host);
        let bind = client.request(RuntimeOperation::BindChannel {
            channel,
            via: None,
            initiator: None,
        });
        let bound = client.send(&bind);
        assert_eq!(bound.state(), RuntimeReceiptState::Completed, "{channel:?}");
        client
    }

    /// `ApplyResourceTargets` of `document` with `expected_version`, sent on `client`'s connection
    /// with `envelope`. The clock moves one millisecond first, so two writes are two observations,
    /// each strictly newer.
    fn apply(
        &self,
        client: &mut TestClient,
        envelope: Envelope,
        document: &str,
        expected_version: Option<u64>,
    ) -> RuntimeReceipt {
        self.clock.advance(1);
        let request = envelope(
            client,
            RuntimeOperation::ApplyResourceTargets {
                document_json: document.to_owned(),
                expected_version,
            },
        );
        client.send(&request)
    }

    /// The instance's `ResourceTargetView`, read on `client`'s connection with `envelope`.
    fn view(&self, client: &mut TestClient, envelope: Envelope) -> ResourceTargetView {
        let request = envelope(
            client,
            RuntimeOperation::ResourceTargetView {
                instance_alias: ALIAS.to_owned(),
            },
        );
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        let Some(RuntimeResult::ResourceTargetView { view }) = receipt.result() else {
            panic!("a resource target view: {receipt:?}");
        };
        (**view).clone()
    }

    /// Every `fact.published` of the instance's resource target policy, in ledger order: its
    /// sequence, id and origin (source, actor).
    fn policy_events(&self) -> Vec<(u64, EventId, EventSource, EventActor)> {
        let mut events = self
            .host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::FactPublished),
                ..EventQuery::default()
            })
            .expect("ledger records")
            .into_iter()
            .filter(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Fact(FactPayload::Published(payload))
                        if payload.record().key == RESOURCE_TARGETS_FACT_KEY
                )
            })
            .map(|event| {
                (
                    event.sequence(),
                    *event.event_id(),
                    event.origin().source(),
                    event.origin().actor(),
                )
            })
            .collect::<Vec<_>>();
        events.sort_by_key(|(sequence, ..)| *sequence);
        events
    }

    fn head(&self) -> u64 {
        ledger_head(&mut TestClient::connect(&self.host))
    }

    fn close(self) {
        assert!(self.host.fatal_error().expect("runtime health").is_none());
        self.host.close().expect("close host");
    }
}

/// The version a view gives the next write: its `active.version`, `0` without `active`.
fn version_of(view: &ResourceTargetView) -> u64 {
    view.active.as_ref().map_or(0, |active| active.version)
}

/// The policy a completed receipt answers, its code the kind's success code and its terminal the
/// event that holds the policy.
#[track_caller]
fn applied(receipt: &RuntimeReceipt) -> ResourceTargetsApplied {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the write is taken: {receipt:?}"
    );
    assert_eq!(receipt.outcome().code().as_str(), APPLIED, "{receipt:?}");
    let Some(RuntimeResult::ResourceTargetsApplied { applied }) = receipt.result() else {
        panic!("an applied policy: {receipt:?}");
    };
    let terminal = receipt.terminal().expect("the write's terminal");
    assert_eq!(
        (terminal.sequence, terminal.event_id),
        (applied.version, applied.event_id)
    );
    (**applied).clone()
}

/// The refusal of another version: Denied `fact_observation_version_conflict` naming
/// `current_version`, with no result and no terminal; `full` (a receipt in full detail) also names
/// the key and the operation, which the console's codes detail leaves out.
#[track_caller]
fn conflict(receipt: &RuntimeReceipt, current_version: u64, full: bool) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), CONFLICT, "{receipt:?}");
    assert_eq!(
        receipt.outcome().value("current_version"),
        Some(&json!(current_version)),
        "{receipt:?}"
    );
    if full {
        assert_eq!(
            receipt.outcome().value("fact_key"),
            Some(&json!(RESOURCE_TARGETS_FACT_KEY)),
            "{receipt:?}"
        );
        assert_eq!(
            receipt.outcome().value("operation"),
            Some(&json!("apply_resource_targets")),
            "{receipt:?}"
        );
    }
    assert!(receipt.result().is_none(), "{receipt:?}");
    assert!(receipt.terminal().is_none(), "{receipt:?}");
}

// On `ui`. The operation table lists `ApplyResourceTargets` among its `ui_kinds`, a write the
// console does not send yet; for every kind of `RuntimeOperationKind::ALL` the `ui` channel takes
// it exactly when it is a handshake kind or the table lists it, so every kind off the table stays
// refused. On a connection bound `ui`, with the console's (User, Ui) envelope, the view gives
// version 0 (no record), and `ApplyResourceTargets` with it is taken: the receipt names the
// channel `ui`, the record is one `fact.published` of the key with the console's origin, its event
// the receipt's terminal, and the view then gives the new version.
#[test]
fn t1r_the_ui_channel_takes_the_document_and_every_kind_off_the_table_stays_refused() {
    let rows = table_ui_kinds();
    let listed = rows.iter().map(|(kind, _)| *kind).collect::<BTreeSet<_>>();
    assert_eq!(listed.len(), rows.len(), "the table lists a kind twice");
    let (_, row) = rows
        .iter()
        .find(|(kind, _)| *kind == Kind::ApplyResourceTargets)
        .expect("the table lists ApplyResourceTargets");
    assert_eq!(
        (row["class"].as_str(), row["console_today"].as_bool()),
        (Some("write"), Some(false)),
        "{row}"
    );
    let mut refused = 0;
    for kind in Kind::ALL {
        let handshake = matches!(kind, Kind::Health | Kind::BindChannel);
        assert_eq!(
            ui_takes_kind(*kind),
            handshake || listed.contains(kind),
            "{kind:?}"
        );
        if !ui_takes_kind(*kind) {
            refused += 1;
        }
    }
    assert_eq!(refused, Kind::ALL.len() - listed.len() - 2);

    let fixture = Fixture::start();
    let mut ui = fixture.bound(RuntimeChannel::Ui);
    let console: Envelope = TestClient::governance_request;
    assert_eq!(version_of(&fixture.view(&mut ui, console)), 0, "no record");
    let receipt = fixture.apply(&mut ui, console, &document(50), Some(0));
    assert_eq!(receipt.channel(), Some(RuntimeChannel::Ui), "{receipt:?}");
    let written = applied(&receipt);
    assert_eq!((written.previous_version, written.replayed), (None, false));
    assert_eq!(
        fixture.policy_events(),
        vec![(
            written.version,
            written.event_id,
            EventSource::Ui,
            EventActor::User
        )],
        "one record of the key, with the console's origin"
    );
    assert_eq!(version_of(&fixture.view(&mut ui, console)), written.version);
    drop(ui);
    fixture.close();
}

// `expected_version` against the instance's record, on a connection whose receipts carry the full
// outcome. Without a record, 1 is the conflict naming `current_version` 0, the key and the
// operation, and appends nothing; 0 applies. With a record, 0 and a version other than the
// record's are the conflict naming the record's version and append nothing; the record's version
// applies, its answer carrying the new version, the one the view then gives; and a write without
// the member applies whatever the record's version. Each write is one `fact.published` of the key,
// its version that event's sequence.
#[test]
fn t1r_expected_version_is_compared_with_the_version_of_the_record() {
    let fixture = Fixture::start();
    let mut client = TestClient::connect(&fixture.host);
    let operator: Envelope = TestClient::request;

    let head = fixture.head();
    conflict(
        &fixture.apply(&mut client, operator, &document(50), Some(1)),
        0,
        true,
    );
    assert_eq!(fixture.head(), head, "a conflict appends nothing");
    assert!(fixture.policy_events().is_empty());

    let first = applied(&fixture.apply(&mut client, operator, &document(50), Some(0)));
    assert_eq!((first.previous_version, first.replayed), (None, false));

    let head = fixture.head();
    for other in [0, first.version + 1] {
        conflict(
            &fixture.apply(&mut client, operator, &document(60), Some(other)),
            first.version,
            true,
        );
    }
    assert_eq!(fixture.head(), head, "a conflict appends nothing");

    let second = applied(&fixture.apply(&mut client, operator, &document(60), Some(first.version)));
    assert!(second.version > first.version, "{second:?}");
    assert_eq!(
        (second.previous_version, second.replayed),
        (Some(first.version), false)
    );
    assert_eq!(
        version_of(&fixture.view(&mut client, operator)),
        second.version
    );

    let third = applied(&fixture.apply(&mut client, operator, &document(70), None));
    assert_eq!(
        (third.previous_version, third.replayed),
        (Some(second.version), false)
    );

    let written = fixture
        .policy_events()
        .into_iter()
        .map(|(sequence, event_id, ..)| (sequence, event_id))
        .collect::<Vec<_>>();
    assert_eq!(
        written,
        vec![
            (first.version, first.event_id),
            (second.version, second.event_id),
            (third.version, third.event_id),
        ],
        "one fact.published per applied write"
    );
    drop(client);
    fixture.close();
}

// Two writers: the console on `ui` (A, its (User, Ui) envelope, whose codes detail keeps a
// refusal's `current_version`) and an agent on `lab` (B, the agent's envelope, writing without the
// member as today's tools do). B writes; A reads the version v; B writes again, so the version is
// v'; A's write with v is the conflict naming v', appends nothing and leaves B's policy in force;
// A reads again, writes with v' and is applied, B's policy its previous version.
#[test]
fn t1r_two_writers_a_version_read_before_the_other_write_is_refused_until_read_again() {
    let fixture = Fixture::start();
    let mut console = fixture.bound(RuntimeChannel::Ui);
    let mut agent = fixture.bound(RuntimeChannel::Lab);
    let ui: Envelope = TestClient::governance_request;
    let tools: Envelope = TestClient::agent_request;

    let before = applied(&fixture.apply(&mut agent, tools, &document(50), None));
    let read = version_of(&fixture.view(&mut console, ui));
    assert_eq!(read, before.version);
    let changed = applied(&fixture.apply(&mut agent, tools, &document(60), None));
    assert_eq!(changed.previous_version, Some(read));

    let head = fixture.head();
    let refused = fixture.apply(&mut console, ui, &document(70), Some(read));
    conflict(&refused, changed.version, false);
    assert_eq!(refused.channel(), Some(RuntimeChannel::Ui), "{refused:?}");
    assert_eq!(fixture.head(), head, "a conflict appends nothing");
    let in_force = fixture.view(&mut console, ui);
    let active = in_force.active.as_ref().expect("a policy in force");
    assert_eq!(
        (active.version, active.policy_sha256.as_str()),
        (changed.version, changed.policy_sha256.as_str()),
        "B's policy stays in force"
    );

    let again = version_of(&in_force);
    assert_eq!(again, changed.version);
    let written = applied(&fixture.apply(&mut console, ui, &document(70), Some(again)));
    assert_eq!(
        (written.previous_version, written.replayed),
        (Some(changed.version), false)
    );
    assert_eq!(
        version_of(&fixture.view(&mut agent, tools)),
        written.version
    );
    drop(console);
    drop(agent);
    fixture.close();
}

// A replay: the same document again with the record's version appends nothing and answers exactly
// as the same document again without the member, today's replay: `replayed`, the record's version
// and event (the receipt's terminal), `previous_version` that version.
#[test]
fn t1r_a_replay_with_the_records_version_appends_nothing_and_answers_as_without_it() {
    let fixture = Fixture::start();
    let mut client = TestClient::connect(&fixture.host);
    let operator: Envelope = TestClient::request;
    let first = applied(&fixture.apply(&mut client, operator, &document(50), None));

    let head = fixture.head();
    let with_version =
        applied(&fixture.apply(&mut client, operator, &document(50), Some(first.version)));
    let without = applied(&fixture.apply(&mut client, operator, &document(50), None));
    assert_eq!(with_version, without, "the answer of today's replay");
    assert_eq!(
        (
            with_version.version,
            with_version.event_id,
            with_version.previous_version,
            with_version.replayed
        ),
        (first.version, first.event_id, Some(first.version), true)
    );
    assert_eq!(fixture.head(), head, "a replay appends nothing");
    assert_eq!(fixture.policy_events().len(), 1);
    drop(client);
    fixture.close();
}
