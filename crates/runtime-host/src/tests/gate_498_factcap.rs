// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #498 FACT-CAP (register §1.496; `contracts/instance-fact-store.md`, "Capacity ledger"):
// the instance fact store's bound follows its scopes. Each instance's own facts are bounded by the
// instance's bound (`MAX_INSTANCE_FACTS`, 224), each server's and each game's facts by their own (16
// each), counted once whatever instances they apply to; the three add up to what one snapshot holds
// (256). An instance whose own facts reach 80% of its bound is said so once per episode
// (`fact_store_capacity_share_reached`), and a reading the store refuses fails its run with the
// store's refusal as the failure's cause. Tests 1 to 4 and 6 drive the store as the host does (a
// ledger and its State store; each publication checked by the store, appended and followed); test 5
// drives the host with two devices and the list reading package of the S168-4 gate, its fill
// observed at the wall clock as S168-4's is. Instances, servers, games and keys are made up.

use super::gate_308_l4b::ALIAS;
use super::gate_428_s168_4::{
    Device, completed, detail, list, publications, run, start, write_package,
};
use super::*;
use crate::fact_store::{
    FACT_CAPACITY_WARNING_FACTS, InstanceFactStore, MAX_GAME_FACTS, MAX_INSTANCE_FACTS,
    MAX_SERVER_FACTS,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, EventAction, EventDraft, EventLinksDraft, EventOrigin, EventPayloadDraft,
    FactObservation, FactPayloadDraft, IssuedInstanceId, MAX_FACT_RECORDS, RuntimePayload,
    RuntimePayloadDraft, SanitizedEventDraft,
};
use actingcommand_ledger::{GlobalLedgerConfig, Sha256SecretFingerprinter};
use std::ops::Range;

const BASE_MS: u64 = 1_750_000_000_000;
// The time the store checks an observation at: after every observation of tests 1 to 4 and 6.
const NOW_MS: u64 = BASE_MS + 1_000_000;
const LIST_KEY: &str = "inventory.list.example_kind_a";
const OTHER_KEY: &str = "inventory.list.example_kind_b";
// The second instance of test 5.
const OTHER_ALIAS: &str = "neutral.other";
const CAPACITY_EXCEEDED: &str = "fact_store_capacity_exceeded";
const SHARE_REACHED: &str = "fact_store_capacity_share_reached";
const READING_REJECTED: &str = "contained_task_resource_reading_rejected";

fn instance(alias: &str) -> FactScope {
    FactScope::Instance {
        instance_id: alias.to_owned(),
    }
}

fn server(server_id: &str) -> FactScope {
    FactScope::Server {
        server_id: server_id.to_owned(),
    }
}

fn game(game_id: &str) -> FactScope {
    FactScope::Game {
        game_id: game_id.to_owned(),
    }
}

fn context(alias: &str, server_id: &str, game_id: &str) -> InstanceFactContext {
    InstanceFactContext {
        instance_id: alias.to_owned(),
        server_id: server_id.to_owned(),
        game_id: game_id.to_owned(),
    }
}

// A ledger with its State store and an instance fact store that follows it as the host's does.
// The ledger is closed before the root goes.
struct Store {
    ledger: GlobalLedger,
    state: Arc<RuntimeStateStore>,
    live: InstanceFactStore,
    issuer: IdentifierIssuer,
    fingerprinter: Sha256SecretFingerprinter,
    clock_ms: u64,
    _root: TempDir,
}

impl Store {
    fn new() -> Self {
        let root = TempDir::new().expect("tempdir");
        let database = Arc::new(
            RuntimeStateStore::open_database(root.path(), b"0123456789abcdef").expect("database"),
        );
        let state =
            Arc::new(RuntimeStateStore::from_database(Arc::clone(&database)).expect("state store"));
        let ledger = GlobalLedger::open_sqlite_candidate(
            GlobalLedgerConfig::new(root.path(), "factcap-fixture"),
            database,
        )
        .expect("ledger");
        let live = InstanceFactStore::recover(&ledger, Arc::clone(&state)).expect("the live store");
        Self {
            ledger,
            state,
            live,
            issuer: IdentifierIssuer::new().expect("issuer"),
            fingerprinter: Sha256SecretFingerprinter::new(b"factcap-fixture-specification-salt")
                .expect("salt"),
            clock_ms: 0,
            _root: root,
        }
    }

    fn instance_id(&self) -> IssuedInstanceId {
        self.issuer.mint_instance_id().expect("instance id")
    }

    // The fixture's next millisecond after the base.
    fn tick(&mut self) -> u64 {
        self.clock_ms += 1;
        BASE_MS + self.clock_ms
    }

    // One observation in `scope` of the keys `env.<name>_<n>` for each `n` of `numbers`, observed
    // at the fixture's next millisecond under a snapshot id of its own.
    fn observation(
        &mut self,
        scope: &FactScope,
        name: &str,
        numbers: Range<usize>,
        invalidate_on: &[EventType],
    ) -> FactObservation {
        let observed = self.tick();
        FactObservation {
            records: numbers
                .map(|number| FactRecord {
                    scope: scope.clone(),
                    key: format!("env.{name}_{number:03}"),
                    content: FactContent::Inline {
                        value: ContractFactValue::Integer(7),
                    },
                    observed_at_unix_ms: observed,
                    expires_at_unix_ms: None,
                    ttl_policy: None,
                    confidence_milli: 1_000,
                    source_detector: "neutral-detector".to_owned(),
                    source_snapshot_id: format!("snapshot:{name}-{observed}"),
                    schema_version: "fact.v1".to_owned(),
                    resource_bundle_hash: "5".repeat(64),
                    invalidate_on: invalidate_on.to_vec(),
                })
                .collect(),
        }
    }

    // The host's publication of `observation`: the store's check first; then the record appended
    // and the store following the ledger.
    fn publish(
        &mut self,
        observation: FactObservation,
        instances: &[IssuedInstanceId],
    ) -> RuntimeHostResult<()> {
        self.live
            .preview_observation(&observation, NOW_MS, &self.ledger)?;
        self.append_unchecked(observation, instances);
        Ok(())
    }

    // `observation` appended without the store's check, as a build before this one could have
    // stored it under its own bound, and the store following the ledger.
    fn append_unchecked(&mut self, observation: FactObservation, instances: &[IssuedInstanceId]) {
        let payload = FactPayloadDraft::observation(
            observation,
            instances
                .iter()
                .map(|instance| *instance.transport())
                .collect(),
            AuditInput::new(),
        )
        .expect("observation");
        let at_ms = self.tick();
        self.ledger
            .append(self.draft(at_ms, OriginModule::FactStore, payload.into()))
            .expect("append the observation");
        self.live
            .synchronize(&self.ledger)
            .expect("the store follows the ledger");
    }

    fn draft(
        &self,
        at_ms: u64,
        module: OriginModule,
        payload: EventPayloadDraft,
    ) -> SanitizedEventDraft {
        EventDraft::new(
            self.issuer.mint_event_id().expect("event id"),
            at_ms,
            EventSeverity::Info,
            EventOrigin::new(EventSource::Runtime, module, EventActor::Runtime),
            EventLinksDraft::default(),
            payload,
        )
        .sanitize(&self.fingerprinter)
        .expect("draft")
    }

    // A `runtime.takeover`, then each invalidation it derives written with its State tombstone in
    // one transaction, as the host writes them (`synchronize_fact_store_under_gate`).
    fn takeover(&mut self) {
        let at_ms = self.tick();
        self.ledger
            .append(
                self.draft(
                    at_ms,
                    OriginModule::Runtime,
                    RuntimePayloadDraft::takeover(EventAction::RuntimeTakeover, AuditInput::new())
                        .into(),
                ),
            )
            .expect("append the takeover");
        self.live
            .synchronize(&self.ledger)
            .expect("the store follows the takeover");
        for pending in self.live.pending_invalidations() {
            let work = self
                .live
                .prepare_invalidation(&self.ledger, &pending)
                .expect("prepare the tombstone");
            let at_ms = self.tick();
            let draft = self.draft(
                at_ms,
                OriginModule::FactStore,
                FactPayloadDraft::invalidated(pending.data.clone(), AuditInput::new()).into(),
            );
            let event = self
                .ledger
                .append_transaction(draft, Box::new(work))
                .expect("the joint invalidation");
            self.live
                .acknowledge_generated_invalidation(&pending.data, event.sequence())
                .expect("acknowledge the invalidation");
        }
    }

    // The instance's own active records.
    fn own(&self, alias: &str) -> usize {
        self.live.instance_records(alias, None).len()
    }

    fn position(&self) -> u64 {
        self.ledger.latest_sequence().expect("the ledger's head")
    }

    fn close(self) {
        self.ledger.close().expect("close the ledger");
    }
}

// The store's refusal of a key its scope has no room for: `fact_store_capacity_exceeded`, a
// request's refusal, not fatal.
fn assert_full(result: RuntimeHostResult<()>, what: &str) {
    let error = result.expect_err(what);
    assert_eq!(error.code(), CAPACITY_EXCEEDED, "{what}: {error:?}");
    assert!(!error.is_fatal(), "{what}: {error:?}");
}

// Test 1: an instance at its bound is refused a new key, and another instance still takes one.
// One short of its bound, an observation of two new keys of A is refused whole and one of one key
// is taken; at the bound every new key of A is refused, before and after B takes a new key, and
// nothing of A is evicted.
#[test]
fn factcap_a_an_instance_at_its_bound_is_refused_while_another_takes_a_key() {
    let mut store = Store::new();
    let (one, two) = (store.instance_id(), store.instance_id());
    let (a, b) = (instance("alias-a"), instance("alias-b"));
    let fill = store.observation(&a, "fill", 0..MAX_INSTANCE_FACTS - 1, &[]);
    store
        .publish(fill, &[one])
        .expect("A one short of its bound");
    let pair = store.observation(&a, "pair", 0..2, &[]);
    assert_full(store.publish(pair, &[one]), "two new keys of A one short");
    assert_eq!(store.own("alias-a"), MAX_INSTANCE_FACTS - 1);
    let last = store.observation(&a, "last", 0..1, &[]);
    store.publish(last, &[one]).expect("A's last key");
    assert_eq!(store.own("alias-a"), MAX_INSTANCE_FACTS);

    let over = store.observation(&a, "over", 0..1, &[]);
    assert_full(store.publish(over, &[one]), "a new key of A at its bound");
    let other = store.observation(&b, "other", 0..1, &[]);
    store
        .publish(other, &[two])
        .expect("B takes a new key while A is at its bound");
    assert_eq!(store.own("alias-b"), 1);
    let again = store.observation(&a, "again", 0..1, &[]);
    assert_full(store.publish(again, &[one]), "A is still at its bound");
    assert_eq!(
        store.own("alias-a"),
        MAX_INSTANCE_FACTS,
        "nothing is evicted"
    );
    store.close();
}

// Test 2: a second instance's bound is not eaten by the first. With A at its bound, B takes its
// whole bound too: the store then holds twice an instance's bound, more than the 256 facts a host
// held in all before. B is refused at its bound, and a third instance still takes a key.
#[test]
fn factcap_b_a_second_instances_bound_is_not_eaten_by_the_first() {
    let mut store = Store::new();
    let (one, two, three) = (
        store.instance_id(),
        store.instance_id(),
        store.instance_id(),
    );
    let (a, b, c) = (
        instance("alias-a"),
        instance("alias-b"),
        instance("alias-c"),
    );
    let fill_a = store.observation(&a, "fill", 0..MAX_INSTANCE_FACTS, &[]);
    store.publish(fill_a, &[one]).expect("A at its bound");
    let fill_b = store.observation(&b, "fill", 0..MAX_INSTANCE_FACTS, &[]);
    store
        .publish(fill_b, &[two])
        .expect("B's whole bound while A is at its own");
    assert_eq!(store.own("alias-a"), MAX_INSTANCE_FACTS);
    assert_eq!(store.own("alias-b"), MAX_INSTANCE_FACTS);
    assert_eq!(store.live.active_count(), 2 * MAX_INSTANCE_FACTS);
    assert!(store.live.active_count() > MAX_FACT_RECORDS);
    let over = store.observation(&b, "over", 0..1, &[]);
    assert_full(store.publish(over, &[two]), "a new key of B at its bound");
    let third = store.observation(&c, "fill", 0..1, &[]);
    store.publish(third, &[three]).expect("C takes a key");
    assert_eq!(store.own("alias-c"), 1);
    store.close();
}

// Test 3: a shared-scope fact is counted once. A server fact and a game fact that apply to A and
// B take nothing from either instance's bound: each instance then takes its whole bound. The
// server and the game fill their own bounds (16 each, each fact counted once) and are refused past
// them, while another game still takes a fact. A's snapshot, its own facts with its server's and
// its game's all at their bounds, holds the 256 records one snapshot holds and is valid.
#[test]
fn factcap_c_a_shared_fact_is_counted_once() {
    let mut store = Store::new();
    let (one, two) = (store.instance_id(), store.instance_id());
    let (shared_server, shared_game) = (server("server-one"), game("game-one"));
    let first_server = store.observation(&shared_server, "shared", 0..1, &[]);
    store
        .publish(first_server, &[one, two])
        .expect("a server fact of A and B");
    let first_game = store.observation(&shared_game, "shared", 0..1, &[]);
    store
        .publish(first_game, &[one, two])
        .expect("a game fact of A and B");
    for (alias, id) in [("alias-a", one), ("alias-b", two)] {
        let fill = store.observation(&instance(alias), "fill", 0..MAX_INSTANCE_FACTS, &[]);
        store
            .publish(fill, &[id])
            .expect("the instance's whole bound beside the shared facts");
        assert_eq!(store.own(alias), MAX_INSTANCE_FACTS, "{alias}");
    }

    let server_rest = store.observation(&shared_server, "rest", 0..MAX_SERVER_FACTS - 1, &[]);
    store
        .publish(server_rest, &[one, two])
        .expect("the server's whole bound");
    let game_rest = store.observation(&shared_game, "rest", 0..MAX_GAME_FACTS - 1, &[]);
    store
        .publish(game_rest, &[one, two])
        .expect("the game's whole bound");
    let server_over = store.observation(&shared_server, "over", 0..1, &[]);
    assert_full(
        store.publish(server_over, &[one, two]),
        "a new server fact at its bound",
    );
    let game_over = store.observation(&shared_game, "over", 0..1, &[]);
    assert_full(
        store.publish(game_over, &[one, two]),
        "a new game fact at its bound",
    );
    let other_game = store.observation(&game("game-two"), "other", 0..1, &[]);
    store
        .publish(other_game, &[])
        .expect("another game takes a fact");
    assert_eq!(
        store.live.active_count(),
        2 * MAX_INSTANCE_FACTS + MAX_SERVER_FACTS + MAX_GAME_FACTS + 1
    );

    let snapshot = store
        .live
        .snapshot(
            context("alias-a", "server-one", "game-one"),
            store.position(),
        )
        .expect("A's snapshot with every scope at its bound");
    assert_eq!(snapshot.records.len(), MAX_FACT_RECORDS);
    store.close();
}

// Test 4: the warning at the share, once, and cleared (the store's half; test 5 shows the host's
// record). A one short of the share is reported nothing; the key that reaches the share is
// reported once, with A's count; a key past it is not; a takeover that invalidates ten of A's facts
// takes A back under the share and ends the episode, reporting nothing; reaching the share again
// is reported once more. B, far under the share, is never reported.
#[test]
fn factcap_d_the_warning_at_the_share_once_and_cleared() {
    let mut store = Store::new();
    let (one, two) = (store.instance_id(), store.instance_id());
    let (a, b) = (instance("alias-a"), instance("alias-b"));
    let kept = store.observation(&a, "kept", 0..FACT_CAPACITY_WARNING_FACTS - 11, &[]);
    store.publish(kept, &[one]).expect("A's kept facts");
    let dropped = store.observation(&a, "dropped", 0..10, &[EventType::RuntimeTakeover]);
    store
        .publish(dropped, &[one])
        .expect("A's facts a takeover drops");
    let other = store.observation(&b, "other", 0..10, &[]);
    store.publish(other, &[two]).expect("B's facts");
    assert_eq!(store.own("alias-a"), FACT_CAPACITY_WARNING_FACTS - 1);
    assert!(
        store.live.capacity_warnings_due().is_empty(),
        "one short of the share"
    );

    let reach = store.observation(&a, "reach", 0..1, &[]);
    store
        .publish(reach, &[one])
        .expect("the key that reaches the share");
    assert_eq!(
        store.live.capacity_warnings_due(),
        vec![("alias-a".to_owned(), FACT_CAPACITY_WARNING_FACTS)]
    );
    let past = store.observation(&a, "past", 0..1, &[]);
    store.publish(past, &[one]).expect("a key past the share");
    assert!(
        store.live.capacity_warnings_due().is_empty(),
        "once per episode"
    );

    store.takeover();
    assert_eq!(store.own("alias-a"), FACT_CAPACITY_WARNING_FACTS + 1 - 10);
    assert!(
        store.live.capacity_warnings_due().is_empty(),
        "back under the share: the episode ends, nothing is said"
    );
    let again = store.observation(&a, "again", 0..9, &[]);
    store.publish(again, &[one]).expect("A at the share again");
    assert_eq!(
        store.live.capacity_warnings_due(),
        vec![("alias-a".to_owned(), FACT_CAPACITY_WARNING_FACTS)],
        "a new episode"
    );
    assert!(store.live.capacity_warnings_due().is_empty());
    store.close();
}

// One made-up fact of ALIAS, observed at `now`.
fn fill(index: usize, now: u64) -> FactRecord {
    FactRecord {
        scope: instance(ALIAS),
        key: format!("session.example.fill_{index:03}"),
        content: FactContent::Inline {
            value: ContractFactValue::Integer(1),
        },
        observed_at_unix_ms: now,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 1_000,
        source_detector: "detector.fixture".to_owned(),
        source_snapshot_id: format!("snapshot:factcap-fill-{index:03}"),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "a".repeat(64),
        invalidate_on: Vec::new(),
    }
}

// The instance's own records, as its snapshot holds them.
fn own_facts(host: &RuntimeHost, alias: &str) -> usize {
    host.instance_fact_snapshot(context(alias, "test", "neutral"))
        .expect("the instance's facts")
        .records
        .iter()
        .filter(|record| record.scope == instance(alias))
        .count()
}

// The `fact_store_capacity_share_reached` records with their outcomes, in ledger order.
fn warnings(host: &RuntimeHost) -> Vec<(PersistedEvent, Outcome)> {
    let mut events = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::RuntimeFailed),
            ..EventQuery::default()
        })
        .expect("the runtime.failed records");
    events.sort_by_key(PersistedEvent::sequence);
    events
        .into_iter()
        .filter_map(|event| {
            let outcome = match event.payload() {
                EventPayload::Runtime(RuntimePayload::Failed(record))
                    if record.outcome().code().as_str() == SHARE_REACHED =>
                {
                    record.outcome().clone()
                }
                _ => return None,
            };
            Some((event, outcome))
        })
        .collect()
}

// The records of type `event_type` of the run `correlation_id`.
fn run_events(
    host: &RuntimeHost,
    correlation_id: CorrelationId,
    event_type: EventType,
) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery {
        correlation_id: Some(correlation_id),
        event_type: Some(event_type),
        ..EventQuery::default()
    })
    .expect("the run's records")
}

// Test 5: a refused reading's run does not end as a success, and the host said so before. On the
// S168-4 gate's host with two devices, each instance first reads its list once. A's own facts are
// then filled with made-up keys until the store refuses one (not fatal), at A's bound; on the way
// one Warning `fact_store_capacity_share_reached` was recorded, under A's instance link, with the
// share's count and A's bound. A run on A whose list is a new key fails
// `contained_task_resource_reading_rejected` (native detail `read_members:<code>`): its terminal is
// one `task.failed` whose failure has the store's `fact_store_capacity_exceeded` as a cause, after
// the warning and under the same instance; there is no `task.completed`, nothing is published and
// nothing of A evicted. The same list on B is read and published (B's bound is its own), and no
// warning names B.
#[test]
fn factcap_e_a_refused_readings_run_fails_and_another_instance_reads_on() {
    let root = TempDir::new().expect("tempdir");
    let first_list = write_package(&root, "list-a.zip", LIST_KEY);
    let second_list = write_package(&root, "list-b.zip", OTHER_KEY);
    let device = Device::new();
    let other = Device::new();
    let host = start(&root, &[(ALIAS, &device), (OTHER_ALIAS, &other)]);
    for (alias, shown) in [(ALIAS, &device), (OTHER_ALIAS, &other)] {
        shown.shows(vec![list(0..6), detail()]);
        let (receipt, _) = run(&host, alias, &first_list);
        completed(&host, &receipt);
    }

    let now = unix_ms_now().expect("wall clock");
    let refusal = (0..=MAX_INSTANCE_FACTS)
        .find_map(|index| host.publish_fact(fill(index, now)).err())
        .expect("A's bound refuses a key");
    assert_eq!(refusal.code(), CAPACITY_EXCEEDED, "{refusal:?}");
    assert!(!refusal.is_fatal(), "{refusal:?}");
    assert_eq!(own_facts(&host, ALIAS), MAX_INSTANCE_FACTS);
    let found = warnings(&host);
    let [(warning, outcome)] = found.as_slice() else {
        panic!("one warning, {} found", found.len());
    };
    assert_eq!(warning.severity(), EventSeverity::Warning);
    assert_eq!(
        outcome.value("instance_id"),
        Some(&serde_json::Value::from(ALIAS)),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("count"),
        Some(&serde_json::Value::from(FACT_CAPACITY_WARNING_FACTS)),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("limit_count"),
        Some(&serde_json::Value::from(MAX_INSTANCE_FACTS)),
        "{outcome:?}"
    );
    assert!(warning.links().instance_id().is_some());

    device.shows(vec![list(0..6), detail()]);
    let (receipt, correlation_id) = run(&host, ALIAS, &second_list);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Failed,
        "{:?}",
        receipt.outcome()
    );
    assert_eq!(
        receipt.outcome().code().as_str(),
        READING_REJECTED,
        "{:?}",
        receipt.outcome()
    );
    assert_eq!(
        host.failure_detail_for_test(receipt.request_id())
            .expect("the failure's detail"),
        Some(format!("read_members:{CAPACITY_EXCEEDED}"))
    );
    let terminals = run_events(&host, correlation_id, EventType::TaskFailed);
    let [terminal] = terminals.as_slice() else {
        panic!("one task.failed, {} found", terminals.len());
    };
    let failure = terminal
        .payload()
        .outcome()
        .expect("the terminal's failure");
    assert_eq!(failure.code().as_str(), READING_REJECTED, "{failure:?}");
    assert!(
        failure
            .causes()
            .iter()
            .any(|link| link.outcome().code().as_str() == CAPACITY_EXCEEDED),
        "the store's refusal is the failure's cause: {failure:?}"
    );
    assert!(run_events(&host, correlation_id, EventType::TaskCompleted).is_empty());
    assert!(warning.sequence() < terminal.sequence());
    assert_eq!(
        warning.links().instance_id(),
        terminal.links().instance_id()
    );
    assert!(publications(&host, OTHER_KEY).is_empty());
    assert_eq!(
        own_facts(&host, ALIAS),
        MAX_INSTANCE_FACTS,
        "nothing is evicted"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());

    other.shows(vec![list(0..6), detail()]);
    let (receipt, _) = run(&host, OTHER_ALIAS, &second_list);
    completed(&host, &receipt);
    let published = publications(&host, OTHER_KEY);
    assert_eq!(published.len(), 1, "{published:#?}");
    assert_eq!(published[0].scope, instance(OTHER_ALIAS));
    assert_eq!(warnings(&host).len(), 1, "no warning names B");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// Test 6: a store restored at a start with more than 256 facts in all, of several instances,
// restores them all. Three instances of 100 facts each and one game fact of all three (301 facts)
// load from the tagged state as the full replay leaves them; each instance's snapshot holds its
// 100 with the game's one, and the restored store takes a new key of each. A store a build before
// this one wrote, which held at most 256 facts in all and could hold them in one instance, loads
// whole too: 240 own facts of one instance (appended without the store's check, as that build's
// bound let them in) are restored, and that instance takes no new key while it is past its bound.
#[test]
fn factcap_f_a_start_restores_more_than_256_facts_of_several_instances() {
    let mut store = Store::new();
    let ids = [
        store.instance_id(),
        store.instance_id(),
        store.instance_id(),
    ];
    let aliases = ["alias-a", "alias-b", "alias-c"];
    let shared = store.observation(&game("game-one"), "shared", 0..1, &[]);
    store
        .publish(shared, &ids)
        .expect("a game fact of the three");
    for (alias, id) in aliases.into_iter().zip(ids) {
        let fill = store.observation(&instance(alias), "fill", 0..100, &[]);
        store
            .publish(fill, &[id])
            .expect("100 facts of the instance");
    }
    assert_eq!(store.live.active_count(), 301);

    let loaded = InstanceFactStore::recover(&store.ledger, Arc::clone(&store.state))
        .expect("the start's load");
    let replayed = InstanceFactStore::replay_for_test(&store.ledger, Arc::clone(&store.state))
        .expect("the full replay");
    assert_eq!(loaded.active_count(), 301);
    assert_eq!(loaded.view_for_test(), replayed.view_for_test());
    for alias in aliases {
        let snapshot = loaded
            .snapshot(context(alias, "server-one", "game-one"), store.position())
            .expect("the instance's snapshot");
        assert_eq!(snapshot.records.len(), 101, "{alias}");
    }
    store.live = loaded;
    for (alias, id) in aliases.into_iter().zip(ids) {
        let more = store.observation(&instance(alias), "more", 0..1, &[]);
        store
            .publish(more, &[id])
            .expect("the restored store takes a new key of the instance");
        assert_eq!(store.own(alias), 101, "{alias}");
    }
    store.close();

    let mut older = Store::new();
    let id = older.instance_id();
    let fill = older.observation(&instance("alias-a"), "fill", 0..240, &[]);
    older.append_unchecked(fill, &[id]);
    let restored = InstanceFactStore::recover(&older.ledger, Arc::clone(&older.state))
        .expect("an older store's load");
    assert_eq!(restored.active_count(), 240);
    older.live = restored;
    let over = older.observation(&instance("alias-a"), "over", 0..1, &[]);
    assert_full(
        older.publish(over, &[id]),
        "an instance past its bound takes no new key",
    );
    assert_eq!(older.own("alias-a"), 240);
    older.close();
}
