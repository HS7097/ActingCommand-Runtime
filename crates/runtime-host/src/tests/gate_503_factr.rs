// SPDX-License-Identifier: AGPL-3.0-only

// #503 FACT-R (0.12.103; register §1.555; `contracts/instance-fact-store.md`, "Expiry
// reclamations"): a scope of the instance fact store at its bound takes a publication's new keys in
// the room of its facts that expired more than seven days before, each removed by its own record.
// The plan picks exactly what the publication needs, the longest expired first, never a fact
// without an expiry, one expired less than the margin, one of the observation's own keys or one of
// the four keys whose writers read an expired record back; with too few candidates nothing is
// written and the refusal is the one of before. The records are one Warning `runtime.failed`
// `fact_store_expired_facts_reclaimed`, then per fact one `fact.invalidated` whose trigger is the
// warning and whose time is the fact's own expiry, with its State tombstone; the start's tagged
// load, the replay and the history at a position rebuild the live store, and the State projection
// takes the third form and refuses a made-up one. Tests 1 to 8 and 10 drive the store as the host
// does (a ledger and its State store; the plan, the check with the room it frees, the records, the
// check again and the publication, each appended and followed), on the fixture's own clock; test 9
// drives the host with the list reading package of the S168-4 gate, its facts observed at the wall
// clock as S168-4's are; test 11 reads the declaration. Instances, games and keys are made up.

use super::gate_308_l4b::ALIAS;
use super::gate_428_s168_4::{
    Device, completed, detail, list, publications, run, start, write_package,
};
use super::*;
use crate::codes::{FactCapacityCode, HostLocation};
use crate::fact_store::{
    ExpiryReclamation, FactTransaction, InstanceFactStore, MAX_GAME_FACTS, MAX_INSTANCE_FACTS,
    fact_transaction_error,
};
use actingcommand_contract::codes::ContractLocation;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, DiagnosticCode, DiagnosticDetailDraft, EventDraft, EventLinksDraft, EventOrigin,
    EventPayloadDraft, FACT_RECLAIM_AFTER_EXPIRY_MS, FactInvalidationEventData, FactObservation,
    FactPayload, FactPayloadDraft, GOAL_RECORDS_FACT_KEY, IssuedInstanceId, MAX_FACT_TTL_MS,
    PACK_SCHEDULE_FACT_KEY, REMEDY_CHOICES_FACT_KEY, RESOURCE_TARGETS_FACT_KEY, RuntimePayload,
    RuntimePayloadDraft, SanitizedEventDraft, priority_offset_fact_key,
};
use actingcommand_ledger::{GlobalLedgerConfig, Sha256SecretFingerprinter};
use std::ops::Range;

const BASE_MS: u64 = 1_750_000_000_000;
const HOUR_MS: u64 = 60 * 60 * 1_000;
const DAY_MS: u64 = 24 * HOUR_MS;
// The fixture's clock for the publications that reclaim: ten days after the base, so a fact that
// expired in the base's first two days has been expired for more than the margin of seven.
const LATER_MS: u64 = BASE_MS + 10 * DAY_MS;
const RECLAIMED: &str = "fact_store_expired_facts_reclaimed";
const CAPACITY_EXCEEDED: &str = "fact_store_capacity_exceeded";
const NOT_NEWER: &str = "fact_observation_not_newer";
const SNAPSHOT_INVALIDATED: &str = "fact_source_snapshot_invalidated";
const SOURCE_MISMATCH: &str = "fact_invalidation_source_mismatch";
const TARGET_MISMATCH: &str = "fact_invalidation_target_mismatch";
const LIST_KEY: &str = "inventory.list.example_kind_a";
const OTHER_KEY: &str = "inventory.list.example_kind_b";

fn instance(alias: &str) -> FactScope {
    FactScope::Instance {
        instance_id: alias.to_owned(),
    }
}

fn game(game_id: &str) -> FactScope {
    FactScope::Game {
        game_id: game_id.to_owned(),
    }
}

fn context(alias: &str) -> InstanceFactContext {
    InstanceFactContext {
        instance_id: alias.to_owned(),
        server_id: "server-one".to_owned(),
        game_id: "game-one".to_owned(),
    }
}

// The keys `env.<name>_<n>` for each `n` of `numbers`.
fn keys(name: &str, numbers: Range<usize>) -> Vec<String> {
    numbers
        .map(|number| format!("env.{name}_{number:03}"))
        .collect()
}

// One made-up fact of `key` in `scope`, observed at `observed` under `snapshot`, which expires at
// `expires_at` (never without one).
fn record(
    scope: &FactScope,
    key: &str,
    observed: u64,
    expires_at: Option<u64>,
    invalidate_on: &[EventType],
    snapshot: &str,
) -> FactRecord {
    FactRecord {
        scope: scope.clone(),
        key: key.to_owned(),
        content: FactContent::Inline {
            value: ContractFactValue::Integer(7),
        },
        observed_at_unix_ms: observed,
        expires_at_unix_ms: expires_at,
        ttl_policy: expires_at.map(|_| FactTtlPolicy {
            minimum_ms: 1,
            maximum_ms: MAX_FACT_TTL_MS,
            source: FactTtlSource::DetectorContract,
        }),
        confidence_milli: 1_000,
        source_detector: "neutral-detector".to_owned(),
        source_snapshot_id: snapshot.to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "5".repeat(64),
        invalidate_on: invalidate_on.to_vec(),
    }
}

// The origin of every record the fact store writes.
fn fact_store_origin() -> EventOrigin {
    EventOrigin::new(
        EventSource::Runtime,
        OriginModule::FactStore,
        EventActor::Runtime,
    )
}

// The invalidation a `fact.invalidated` record carries.
fn invalidation(event: &PersistedEvent) -> FactInvalidationEventData {
    match event.payload() {
        EventPayload::Fact(FactPayload::Invalidated(payload)) => payload.invalidation().clone(),
        _ => panic!("a fact.invalidated record: {:?}", event.event_type()),
    }
}

// The outcome of a reclamation's warning; `None` for any other record.
fn reclaimed(event: &PersistedEvent) -> Option<Outcome> {
    match event.payload() {
        EventPayload::Runtime(RuntimePayload::Failed(record))
            if record.outcome().code().as_str() == RECLAIMED =>
        {
            Some(record.outcome().clone())
        }
        _ => None,
    }
}

// The store's refusal of a key its scope has no room for, a request's refusal, not fatal.
fn assert_full<T: std::fmt::Debug>(result: RuntimeHostResult<T>, what: &str) {
    let error = result.expect_err(what);
    assert_eq!(error.code(), CAPACITY_EXCEEDED, "{what}: {error:?}");
    assert!(!error.is_fatal(), "{what}: {error:?}");
}

// A ledger with its State store and an instance fact store that follows it as the host's does, on
// a clock of the fixture's own. The trail keeps the live store's records after each record the
// fixture appended, by its sequence. The ledger is closed before the root goes.
struct Store {
    ledger: GlobalLedger,
    state: Arc<RuntimeStateStore>,
    live: InstanceFactStore,
    issuer: IdentifierIssuer,
    fingerprinter: Sha256SecretFingerprinter,
    now_ms: u64,
    trail: Vec<(u64, Vec<FactRecord>)>,
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
            GlobalLedgerConfig::new(root.path(), "factr-fixture"),
            database,
        )
        .expect("ledger");
        let live = InstanceFactStore::recover(&ledger, Arc::clone(&state)).expect("the live store");
        Self {
            ledger,
            state,
            live,
            issuer: IdentifierIssuer::new().expect("issuer"),
            fingerprinter: Sha256SecretFingerprinter::new(b"factr-fixture-specification-salt")
                .expect("salt"),
            now_ms: BASE_MS,
            trail: Vec::new(),
            _root: root,
        }
    }

    fn instance_id(&self) -> IssuedInstanceId {
        self.issuer.mint_instance_id().expect("instance id")
    }

    // The fixture's next millisecond.
    fn tick(&mut self) -> u64 {
        self.now_ms += 1;
        self.now_ms
    }

    // The fixture's clock moves on to `at_ms`.
    fn jump(&mut self, at_ms: u64) {
        assert!(at_ms > self.now_ms, "the clock moves on");
        self.now_ms = at_ms;
    }

    // One observation in `scope` of `keys`, observed at the fixture's next millisecond under a
    // snapshot id of its own, expiring at `expires_at`.
    fn observation(
        &mut self,
        scope: &FactScope,
        keys: &[String],
        expires_at: Option<u64>,
        invalidate_on: &[EventType],
    ) -> FactObservation {
        let observed = self.tick();
        let snapshot = format!("snapshot:factr-{observed}");
        FactObservation {
            records: keys
                .iter()
                .map(|key| record(scope, key, observed, expires_at, invalidate_on, &snapshot))
                .collect(),
        }
    }

    fn draft(
        &self,
        at_ms: u64,
        severity: EventSeverity,
        origin: EventOrigin,
        payload: EventPayloadDraft,
    ) -> SanitizedEventDraft {
        EventDraft::new(
            self.issuer.mint_event_id().expect("event id"),
            at_ms,
            severity,
            origin,
            EventLinksDraft::default(),
            payload,
        )
        .sanitize(&self.fingerprinter)
        .expect("draft")
    }

    fn head(&self) -> u64 {
        self.ledger.latest_sequence().expect("the ledger's head")
    }

    // The store follows the ledger; the trail keeps its records at the head.
    fn follow(&mut self) {
        self.live
            .synchronize(&self.ledger)
            .expect("the store follows the ledger");
        let head = self.head();
        let records = self
            .live
            .active_records_if_at(head)
            .expect("the store at the head");
        self.trail.push((head, records));
    }

    // `observation` appended as one `fact.published` under `origin`, without the store's check,
    // and followed.
    fn append(
        &mut self,
        observation: FactObservation,
        instances: &[IssuedInstanceId],
        origin: EventOrigin,
    ) {
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
        let draft = self.draft(at_ms, EventSeverity::Info, origin, payload.into());
        self.ledger.append(draft).expect("append the observation");
        self.follow();
    }

    // The host's publication of `observation` (`publish_facts_under_gate`) at the fixture's clock:
    // the plan and the check with the room it frees; when it reclaims, the reclamation's records
    // and the check again without the room; then the publication, followed. The plan is returned.
    fn publish(
        &mut self,
        observation: FactObservation,
        instances: &[IssuedInstanceId],
    ) -> RuntimeHostResult<Option<ExpiryReclamation>> {
        let now = self.now_ms;
        let plan = self.live.plan_expiry_reclamation(&observation, now);
        let freed = plan.as_ref().map_or(0, ExpiryReclamation::freed);
        self.live
            .preview_observation_freeing(&observation, now, &self.ledger, freed)?;
        if let Some(plan) = &plan {
            self.reclaim(plan)?;
            self.live
                .preview_observation(&observation, now, &self.ledger)?;
        }
        self.append(observation, instances, fact_store_origin());
        Ok(plan)
    }

    // The reclamation's records as the host writes them (`reclaim_expired_under_gate`): its
    // warning, the invalidations the warning derives by the ordinary rule, then each reclaimed
    // fact's invalidation with its State tombstone in one transaction, each followed.
    fn reclaim(&mut self, plan: &ExpiryReclamation) -> RuntimeHostResult<()> {
        let warning = self.warning(plan.scope(), plan.records().len(), plan.bound());
        self.write_pending();
        for (key, sequence) in plan.records() {
            let (data, work) = self.live.prepare_expiry_reclamation(
                &self.ledger,
                plan.scope(),
                key,
                *sequence,
                &warning,
            )?;
            self.invalidate(data, work)?;
        }
        Ok(())
    }

    // One Warning `runtime.failed` `fact_store_expired_facts_reclaimed` of `count` facts of `scope`,
    // whose bound is `bound`, under the system links, as the host writes it; followed.
    fn warning(&mut self, scope: &FactScope, count: usize, bound: usize) -> PersistedEvent {
        let outcome = Outcome::new(FactCapacityCode::ExpiredFactsReclaimed)
            .operation(ContractLocation::PublishFacts);
        let outcome = match scope {
            FactScope::Instance { instance_id } => outcome.instance_id(instance_id.as_str()),
            FactScope::Server { server_id } => outcome.server(server_id.as_str()),
            FactScope::Game { game_id } => outcome.game(game_id.as_str()),
        }
        .count(u32::try_from(count).expect("a count"))
        .limit_count(u32::try_from(bound).expect("a bound"));
        let payload = RuntimePayloadDraft::failed(
            DiagnosticCode::RuntimeDiagnostic,
            EffectDisposition::NotPerformed,
            outcome,
            DiagnosticDetailDraft::new(
                "instance_fact_store",
                HostLocation::CommitFact,
                "runtime_host",
                ContractLocation::PublishFacts.as_str(),
            ),
            AuditInput::new(),
        );
        let at_ms = self.tick();
        let draft = self.draft(
            at_ms,
            EventSeverity::Warning,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            payload.into(),
        );
        let warning = self.ledger.append(draft).expect("append the warning");
        self.follow();
        warning
    }

    // Each invalidation the store derived and has not written yet, with its State tombstone in one
    // transaction, acknowledged and followed (`synchronize_fact_store_under_gate`).
    fn write_pending(&mut self) {
        for pending in self.live.pending_invalidations() {
            let work = self
                .live
                .prepare_invalidation(&self.ledger, &pending)
                .expect("prepare the tombstone");
            let at_ms = self.tick();
            let draft = self.draft(
                at_ms,
                EventSeverity::Info,
                fact_store_origin(),
                FactPayloadDraft::invalidated(pending.data.clone(), AuditInput::new()).into(),
            );
            let event = self
                .ledger
                .append_transaction(draft, Box::new(work))
                .expect("the joint invalidation");
            self.live
                .acknowledge_generated_invalidation(&pending.data, event.sequence())
                .expect("acknowledge the invalidation");
            self.follow();
        }
    }

    // `data` appended with its State tombstone `work` in one transaction and followed; the State
    // projection's refusal is the error, and then nothing is appended.
    fn invalidate(
        &mut self,
        data: FactInvalidationEventData,
        work: FactTransaction,
    ) -> RuntimeHostResult<()> {
        let at_ms = self.tick();
        let draft = self.draft(
            at_ms,
            EventSeverity::Info,
            fact_store_origin(),
            FactPayloadDraft::invalidated(data, AuditInput::new()).into(),
        );
        self.ledger
            .append_transaction(draft, Box::new(work))
            .map_err(fact_transaction_error)?;
        self.follow();
        Ok(())
    }

    // The active facts stored under exactly `scope`.
    fn count(&self, scope: &FactScope) -> usize {
        self.live
            .active_records_if_at(self.head())
            .expect("the store at the head")
            .iter()
            .filter(|record| &record.scope == scope)
            .count()
    }

    fn active(&self, scope: &FactScope, key: &str) -> bool {
        self.live.active_record(scope, key).is_some()
    }

    // The sequence of `key`'s active record in `scope`.
    fn sequence(&self, scope: &FactScope, key: &str) -> u64 {
        self.live
            .active_revision(scope, key)
            .map(|(_, sequence, _)| sequence)
            .expect("an active record")
    }

    // The records of `event_type`, in ledger order.
    fn events(&self, event_type: EventType) -> Vec<PersistedEvent> {
        let mut events = self
            .ledger
            .query(EventQuery {
                event_type: Some(event_type),
                ..EventQuery::default()
            })
            .expect("the records");
        events.sort_by_key(PersistedEvent::sequence);
        events
    }

    fn close(self) {
        self.ledger.close().expect("close the ledger");
    }
}

// The keys of the reclamation `plan`, in its order.
fn plan_keys(plan: &ExpiryReclamation) -> Vec<&str> {
    plan.records().iter().map(|(key, _)| key.as_str()).collect()
}

// Test 1: an instance at its bound with facts expired long ago takes a new key in the room of the
// longest expired one. Three facts expire one, two and three days after the base (published third,
// first and second), 221 have no expiry; ten days after the base a new key reclaims exactly the
// fact expired one day after the base. The ledger then shows the warning, one `fact.invalidated`
// whose trigger is the warning and whose time is the fact's expiry, and the publication, in that
// order; the instance stays at its bound, the fact is gone from the store and the snapshot, and
// the two others stay.
#[test]
fn factr_a_a_full_instance_takes_a_new_key_in_the_room_of_its_longest_expired_fact() {
    let mut store = Store::new();
    let one = store.instance_id();
    let a = instance("alias-a");
    for (name, days) in [("third", 3), ("first", 1), ("second", 2)] {
        let expiring = store.observation(&a, &keys(name, 0..1), Some(BASE_MS + days * DAY_MS), &[]);
        let plan = store.publish(expiring, &[one]).expect("an expiring fact");
        assert!(plan.is_none(), "{plan:?}");
    }
    let plain = store.observation(&a, &keys("plain", 0..MAX_INSTANCE_FACTS - 3), None, &[]);
    store.publish(plain, &[one]).expect("A at its bound");
    assert_eq!(store.count(&a), MAX_INSTANCE_FACTS);
    let first = "env.first_000";
    let sequence = store.sequence(&a, first);

    store.jump(LATER_MS);
    let new_key = store.observation(&a, &keys("new", 0..1), None, &[]);
    let plan = store
        .publish(new_key, &[one])
        .expect("the new key takes the room")
        .expect("a reclamation");
    assert_eq!(plan.records(), [(first.to_owned(), sequence)].as_slice());
    assert_eq!(plan.bound(), MAX_INSTANCE_FACTS);
    assert_eq!(store.count(&a), MAX_INSTANCE_FACTS);
    assert!(!store.active(&a, first));
    assert!(store.active(&a, "env.second_000") && store.active(&a, "env.third_000"));
    assert!(store.active(&a, "env.new_000"));
    let snapshot = store
        .live
        .snapshot(context("alias-a"), store.head())
        .expect("A's snapshot");
    assert!(snapshot.records.iter().all(|record| record.key != first));

    let warnings = store.events(EventType::RuntimeFailed);
    let [warning] = warnings.as_slice() else {
        panic!("one warning, {} found", warnings.len());
    };
    assert!(reclaimed(warning).is_some());
    let invalidations = store.events(EventType::FactInvalidated);
    let [reclaiming] = invalidations.as_slice() else {
        panic!("one invalidation, {} found", invalidations.len());
    };
    let data = invalidation(reclaiming);
    assert_eq!((data.scope.clone(), data.key.as_str()), (a.clone(), first));
    assert_eq!(data.invalidated_by_event_id, *warning.event_id());
    assert_eq!(data.invalidated_by_event_type, EventType::RuntimeFailed);
    assert_eq!(data.invalidated_at_unix_ms, BASE_MS + DAY_MS);
    let publication = store
        .events(EventType::FactPublished)
        .pop()
        .expect("the publication");
    assert!(warning.sequence() < reclaiming.sequence());
    assert!(reclaiming.sequence() < publication.sequence());
    assert_eq!(publication.sequence(), store.head());
    store.close();
}

// Test 2: three new keys reclaim three, in the order of their expiries, under one warning; with
// fewer candidates than a publication needs nothing is written and the refusal is the one of
// before. A second instance at its bound with two facts expired long ago is refused three new keys
// (`fact_store_capacity_exceeded`, not fatal): no warning, no invalidation, the ledger's head where
// it was, and both facts still stored.
#[test]
fn factr_b_three_new_keys_take_three_and_too_few_candidates_write_nothing() {
    let mut store = Store::new();
    let one = store.instance_id();
    let a = instance("alias-a");
    for days in [2, 1, 3] {
        let expiring = store.observation(
            &a,
            &keys(&format!("day{days}"), 0..1),
            Some(BASE_MS + days * DAY_MS),
            &[],
        );
        store.publish(expiring, &[one]).expect("an expiring fact");
    }
    let plain = store.observation(&a, &keys("plain", 0..MAX_INSTANCE_FACTS - 3), None, &[]);
    store.publish(plain, &[one]).expect("A at its bound");
    store.jump(LATER_MS);
    let three = store.observation(&a, &keys("new", 0..3), None, &[]);
    let plan = store
        .publish(three, &[one])
        .expect("three new keys")
        .expect("a reclamation");
    assert_eq!(
        plan_keys(&plan),
        ["env.day1_000", "env.day2_000", "env.day3_000"]
    );
    assert_eq!(store.count(&a), MAX_INSTANCE_FACTS);
    let warnings = store.events(EventType::RuntimeFailed);
    let [warning] = warnings.as_slice() else {
        panic!("one warning, {} found", warnings.len());
    };
    let reclaimed_keys = store
        .events(EventType::FactInvalidated)
        .iter()
        .map(invalidation)
        .map(|data| {
            assert_eq!(data.invalidated_by_event_id, *warning.event_id());
            data.key
        })
        .collect::<Vec<_>>();
    assert_eq!(
        reclaimed_keys,
        ["env.day1_000", "env.day2_000", "env.day3_000"]
    );
    store.close();

    let mut fewer = Store::new();
    let two = fewer.instance_id();
    let b = instance("alias-b");
    for days in [1, 2] {
        let expiring = fewer.observation(
            &b,
            &keys(&format!("day{days}"), 0..1),
            Some(BASE_MS + days * DAY_MS),
            &[],
        );
        fewer.publish(expiring, &[two]).expect("an expiring fact");
    }
    let plain = fewer.observation(&b, &keys("plain", 0..MAX_INSTANCE_FACTS - 2), None, &[]);
    fewer.publish(plain, &[two]).expect("B at its bound");
    fewer.jump(LATER_MS);
    let head = fewer.head();
    let three = fewer.observation(&b, &keys("new", 0..3), None, &[]);
    assert_full(
        fewer.publish(three, &[two]),
        "three new keys and two candidates",
    );
    assert_eq!(fewer.head(), head, "nothing is written");
    assert!(fewer.events(EventType::RuntimeFailed).is_empty());
    assert!(fewer.events(EventType::FactInvalidated).is_empty());
    assert!(fewer.active(&b, "env.day1_000") && fewer.active(&b, "env.day2_000"));
    assert_eq!(fewer.count(&b), MAX_INSTANCE_FACTS);
    fewer.close();
}

// Test 3: what is never taken, and the order among equals. At its bound the instance holds facts
// that expired an hour after the base under the four keys whose writers read an expired record
// back and under a key the publication carries again, a fact expired six days before the
// publication (less than the margin), 215 without an expiry, and three that expire one day after
// the base: one published first, then two in one observation. A publication of the carried key
// and two new keys reclaims two: the first published one, then the smaller key of the pair. Every
// excluded fact stays, the carried key holds the publication's record, and the pair's other key
// stays.
#[test]
fn factr_c_the_margin_and_the_exclusions_keep_their_facts_and_ties_go_by_sequence_then_key() {
    let mut store = Store::new();
    let one = store.instance_id();
    let a = instance("alias-a");
    let excluded = [
        PACK_SCHEDULE_FACT_KEY,
        REMEDY_CHOICES_FACT_KEY,
        RESOURCE_TARGETS_FACT_KEY,
        GOAL_RECORDS_FACT_KEY,
    ];
    let own = keys("own", 0..1);
    for key in excluded
        .iter()
        .map(|key| (*key).to_owned())
        .chain(own.clone())
    {
        let early = store.observation(&a, &[key], Some(BASE_MS + HOUR_MS), &[]);
        store.publish(early, &[one]).expect("a fact expired early");
    }
    let recent = store.observation(&a, &keys("recent", 0..1), Some(LATER_MS - 6 * DAY_MS), &[]);
    store
        .publish(recent, &[one])
        .expect("a fact expired less than the margin before");
    let tie_first = store.observation(&a, &keys("tie_first", 0..1), Some(BASE_MS + DAY_MS), &[]);
    store
        .publish(tie_first, &[one])
        .expect("the first of the ties");
    let tie_pair = store.observation(&a, &keys("tie_pair", 0..2), Some(BASE_MS + DAY_MS), &[]);
    store
        .publish(tie_pair, &[one])
        .expect("two ties of one publication");
    let plain = store.observation(&a, &keys("plain", 0..MAX_INSTANCE_FACTS - 9), None, &[]);
    store.publish(plain, &[one]).expect("A at its bound");
    assert_eq!(store.count(&a), MAX_INSTANCE_FACTS);

    store.jump(LATER_MS);
    let carried = [own.clone(), keys("new", 0..2)].concat();
    let publication = store.observation(&a, &carried, None, &[]);
    let observed = publication.records[0].observed_at_unix_ms;
    let plan = store
        .publish(publication, &[one])
        .expect("the publication")
        .expect("a reclamation");
    assert_eq!(plan_keys(&plan), ["env.tie_first_000", "env.tie_pair_000"]);
    assert_eq!(store.count(&a), MAX_INSTANCE_FACTS);
    for key in excluded {
        assert!(store.active(&a, key), "{key}");
    }
    assert!(store.active(&a, "env.recent_000"));
    assert!(store.active(&a, "env.tie_pair_001"));
    assert!(store.active(&a, "env.plain_000"));
    let carried_record = store
        .live
        .active_record(&a, &own[0])
        .expect("the carried key");
    assert_eq!(carried_record.observed_at_unix_ms, observed);
    assert_eq!(carried_record.expires_at_unix_ms, None);
    store.close();
}

// The history of tests 4 and 10 on `store`: at the bound of `alias-a`, a fact without an expiry
// that lists `runtime.failed`, three facts expired one, two and three days after the base and 220
// without an expiry; ten days after the base a publication of two new keys, whose warning drops the
// listing fact by the ordinary rule and which reclaims the fact expired first, then one of two
// more new keys, which reclaims the other two. Returns the two plans.
fn two_reclamations(store: &mut Store) -> (ExpiryReclamation, ExpiryReclamation) {
    let one = store.instance_id();
    let a = instance("alias-a");
    let listing = store.observation(
        &a,
        &keys("listing", 0..1),
        None,
        &[EventType::RuntimeFailed],
    );
    store
        .publish(listing, &[one])
        .expect("a fact that lists runtime.failed");
    for days in [1, 2, 3] {
        let expiring = store.observation(
            &a,
            &keys(&format!("day{days}"), 0..1),
            Some(BASE_MS + days * DAY_MS),
            &[],
        );
        store.publish(expiring, &[one]).expect("an expiring fact");
    }
    let plain = store.observation(&a, &keys("plain", 0..MAX_INSTANCE_FACTS - 4), None, &[]);
    store.publish(plain, &[one]).expect("A at its bound");
    store.jump(LATER_MS);
    let first = store.observation(&a, &keys("first", 0..2), None, &[]);
    let first = store
        .publish(first, &[one])
        .expect("two new keys")
        .expect("the first reclamation");
    let second = store.observation(&a, &keys("second", 0..2), None, &[]);
    let second = store
        .publish(second, &[one])
        .expect("two more new keys")
        .expect("the second reclamation");
    (first, second)
}

// Test 4: the start's tagged load, the full replay and the history at a ledger position rebuild
// the live store after reclamations. On the history above, the load and the replay equal the live
// store (LN-2's view: the active records with their sources, `latest_observed`, the tombstones,
// the pending invalidations and the position), each reclaimed key's `latest_observed` being its
// expiry; and the history at every position from the first warning on (before and after each
// invalidation and each publication) equals the live store's records at that position.
#[test]
fn factr_d_a_start_a_replay_and_the_history_rebuild_the_live_store_after_reclamations() {
    let mut store = Store::new();
    let (first, second) = two_reclamations(&mut store);
    assert_eq!(plan_keys(&first), ["env.day1_000"]);
    assert_eq!(plan_keys(&second), ["env.day2_000", "env.day3_000"]);

    let loaded = InstanceFactStore::recover(&store.ledger, Arc::clone(&store.state))
        .expect("the start's load");
    let replayed = InstanceFactStore::replay_for_test(&store.ledger, Arc::clone(&store.state))
        .expect("the full replay");
    let live = store.live.view_for_test();
    assert_eq!(loaded.view_for_test(), live);
    assert_eq!(replayed.view_for_test(), live);
    let a = instance("alias-a");
    for days in [1, 2, 3] {
        assert_eq!(
            live.latest_observed
                .get(&(a.clone(), format!("env.day{days}_000"))),
            Some(&(BASE_MS + days * DAY_MS)),
            "day {days}"
        );
    }

    let first_warning = store
        .events(EventType::RuntimeFailed)
        .first()
        .map(PersistedEvent::sequence)
        .expect("the first warning");
    let positions = store
        .trail
        .iter()
        .filter(|(position, _)| *position >= first_warning)
        .collect::<Vec<_>>();
    assert!(positions.len() >= 7, "{}", positions.len());
    for (position, records) in positions {
        let history = InstanceFactStore::active_records_at(&store.ledger, *position)
            .expect("the history at the position");
        assert_eq!(&history, records, "at {position}");
    }
    store.close();
}

// One case of test 5: what is made up, the fact's expiry, the trigger's type the invalidation
// names and its time (`None`: the warning's own time); `reducers_see_it` is false for the one case
// the record cannot show (the margin, whose trigger time only the State projection reads).
struct Forgery {
    what: &'static str,
    expires_at: Option<u64>,
    trigger_type: EventType,
    at: Option<u64>,
    reducers_see_it: bool,
}

// One fact of `alias-a` expiring at `expires_at` and, ten days after the base, a reclamation's
// warning that would name it; with the invalidation `case` makes up of it, and the sequence of the
// fact's publication.
fn made_up(case: &Forgery) -> (Store, FactInvalidationEventData, u64, PersistedEvent) {
    let mut store = Store::new();
    let one = store.instance_id();
    let a = instance("alias-a");
    let fact = store.observation(&a, &keys("made_up", 0..1), case.expires_at, &[]);
    store.publish(fact, &[one]).expect("the fact");
    let key = "env.made_up_000";
    let (published, snapshot) = store
        .live
        .active_revision(&a, key)
        .map(|(record, sequence, _)| (sequence, record.source_snapshot_id.clone()))
        .expect("the fact");
    store.jump(LATER_MS);
    let warning = store.warning(&a, 1, MAX_INSTANCE_FACTS);
    let warning_at = warning.timestamp_unix_ms();
    let data = FactInvalidationEventData {
        scope: a,
        key: key.to_owned(),
        source_snapshot_id: snapshot,
        invalidated_at_unix_ms: case.at.unwrap_or(warning_at),
        invalidated_by_event_id: *warning.event_id(),
        invalidated_by_event_type: case.trigger_type,
    };
    (store, data, published, warning)
}

// Test 5: a made-up reclamation is refused fatally by the State projection (the joint transaction
// rolls back and nothing is appended), by the live store's reducer, by the history at a position
// and by the replay alike: a trigger of another type, a fact expired less than the margin before
// the trigger (the State projection's, which alone reads the trigger's time), a time other than the
// fact's expiry, and a fact without an expiry. The State projection is asked on one store, the
// other readers on another, where the made-up record is appended without its tombstone.
#[test]
fn factr_e_a_made_up_reclamation_is_refused_by_the_state_the_store_the_history_and_the_replay() {
    let long_ago = BASE_MS + DAY_MS;
    let recent = LATER_MS - 3 * DAY_MS;
    let cases = [
        Forgery {
            what: "a trigger of another type",
            expires_at: Some(long_ago),
            trigger_type: EventType::RuntimeTakeover,
            at: Some(long_ago),
            reducers_see_it: true,
        },
        Forgery {
            what: "expired less than the margin before the trigger",
            expires_at: Some(recent),
            trigger_type: EventType::RuntimeFailed,
            at: Some(recent),
            reducers_see_it: false,
        },
        Forgery {
            what: "a time other than the expiry",
            expires_at: Some(long_ago),
            trigger_type: EventType::RuntimeFailed,
            at: Some(long_ago + 1),
            reducers_see_it: true,
        },
        Forgery {
            what: "a fact without an expiry",
            expires_at: None,
            trigger_type: EventType::RuntimeFailed,
            at: None,
            reducers_see_it: true,
        },
    ];
    for case in &cases {
        let what = case.what;
        let (mut store, data, published, warning) = made_up(case);
        let head = store.head();
        let work = store
            .live
            .prepare_made_up_invalidation_for_test(
                &store.ledger,
                &data,
                published,
                warning.sequence(),
            )
            .expect("the tombstone's work");
        let refused = store
            .invalidate(data.clone(), work)
            .expect_err("the State projection refuses it");
        assert!(refused.is_fatal(), "{what}: {refused:?}");
        assert!(
            [SOURCE_MISMATCH, TARGET_MISMATCH].contains(&refused.code()),
            "{what}: {refused:?}"
        );
        assert_eq!(store.head(), head, "{what}: nothing is appended");
        assert!(store.active(&data.scope, &data.key), "{what}");
        store.close();
        if !case.reducers_see_it {
            continue;
        }

        let (mut store, data, _, _) = made_up(case);
        let head = store.head();
        let mut probe = store.live.clone();
        let refused = probe
            .commit_invalidation(data.clone(), head + 1)
            .expect_err("the live store's reducer refuses it");
        assert!(refused.is_fatal(), "{what}: {refused:?}");
        assert_eq!(refused.code(), TARGET_MISMATCH, "{what}: {refused:?}");
        let at_ms = store.tick();
        let draft = store.draft(
            at_ms,
            EventSeverity::Info,
            fact_store_origin(),
            FactPayloadDraft::invalidated(data, AuditInput::new()).into(),
        );
        store
            .ledger
            .append(draft)
            .expect("the made-up record, without its tombstone");
        let refused = InstanceFactStore::active_records_at(&store.ledger, store.head())
            .expect_err("the history refuses it");
        assert!(refused.is_fatal(), "{what}: {refused:?}");
        assert_eq!(refused.code(), SOURCE_MISMATCH, "{what}: {refused:?}");
        let Err(refused) =
            InstanceFactStore::replay_for_test(&store.ledger, Arc::clone(&store.state))
        else {
            panic!("{what}: the replay refuses it");
        };
        assert!(refused.is_fatal(), "{what}: {refused:?}");
        store.close();
    }
}

// Test 6: after a reclamation the reclaimed snapshot is refused for good, and a reading of the key
// observed after its expiry but before the reclamation (a run that read the key before and
// publishes after) is accepted as a new key. At its bound the instance holds K, expired one day
// after the base, J, two days, and 222 without an expiry; a new key reclaims K. K's old record is
// refused `fact_source_snapshot_invalidated`; a reading observed at K's expiry is refused
// `fact_observation_not_newer` (the floor is the expiry); one observed five days after the base is
// accepted and reclaims J for its room.
#[test]
fn factr_f_a_reading_captured_before_the_reclamation_is_a_new_key_after_it() {
    let mut store = Store::new();
    let one = store.instance_id();
    let a = instance("alias-a");
    let k = store.observation(&a, &keys("k", 0..1), Some(BASE_MS + DAY_MS), &[]);
    let old = k.clone();
    store.publish(k, &[one]).expect("K");
    let j = store.observation(&a, &keys("j", 0..1), Some(BASE_MS + 2 * DAY_MS), &[]);
    store.publish(j, &[one]).expect("J");
    let plain = store.observation(&a, &keys("plain", 0..MAX_INSTANCE_FACTS - 2), None, &[]);
    store.publish(plain, &[one]).expect("A at its bound");
    store.jump(LATER_MS);
    let new_key = store.observation(&a, &keys("l", 0..1), None, &[]);
    let plan = store
        .publish(new_key, &[one])
        .expect("L")
        .expect("a reclamation");
    assert_eq!(plan_keys(&plan), ["env.k_000"]);

    let now = store.now_ms;
    let refused = store
        .live
        .preview_observation(&old, now, &store.ledger)
        .expect_err("the reclaimed snapshot");
    assert_eq!(refused.code(), SNAPSHOT_INVALIDATED, "{refused:?}");
    assert!(!refused.is_fatal(), "{refused:?}");
    let at_expiry = FactObservation {
        records: vec![record(
            &a,
            "env.k_000",
            BASE_MS + DAY_MS,
            None,
            &[],
            "snapshot:factr-k-at-expiry",
        )],
    };
    let refused = store
        .live
        .preview_observation(&at_expiry, now, &store.ledger)
        .expect_err("a reading observed at the expiry");
    assert_eq!(refused.code(), NOT_NEWER, "{refused:?}");
    let captured = FactObservation {
        records: vec![record(
            &a,
            "env.k_000",
            BASE_MS + 5 * DAY_MS,
            None,
            &[],
            "snapshot:factr-k-captured",
        )],
    };
    let plan = store
        .publish(captured, &[one])
        .expect("the reading captured before the reclamation")
        .expect("a reclamation for its room");
    assert_eq!(plan_keys(&plan), ["env.j_000"]);
    let k = store.live.active_record(&a, "env.k_000").expect("K again");
    assert_eq!(k.observed_at_unix_ms, BASE_MS + 5 * DAY_MS);
    assert_eq!(store.count(&a), MAX_INSTANCE_FACTS);
    store.close();
}

// Test 7: a game at its bound of 16 whose priority offset, published under the operator's request
// (`cli`, `cli`, the fact store's module) and expired long ago, is reclaimed for a new key of the
// game: the State projection takes the request's origin for this form, the tombstone is written in
// the joint transaction and the offset's snapshot is refused afterwards. Whether the policy input
// identity changes is not asserted here.
#[test]
fn factr_g_a_full_game_reclaims_an_expired_offset_a_request_published() {
    let mut store = Store::new();
    let g = game("game-one");
    let offset_key = priority_offset_fact_key("example.task");
    let offset = store.observation(&g, &[offset_key.clone()], Some(BASE_MS + DAY_MS), &[]);
    let old = offset.clone();
    store.append(
        offset,
        &[],
        EventOrigin::new(EventSource::Cli, OriginModule::FactStore, EventActor::Cli),
    );
    let plain = store.observation(&g, &keys("plain", 0..MAX_GAME_FACTS - 1), None, &[]);
    store.publish(plain, &[]).expect("the game at its bound");
    assert_eq!(store.count(&g), MAX_GAME_FACTS);
    store.jump(LATER_MS);
    let new_key = store.observation(&g, &keys("new", 0..1), None, &[]);
    let plan = store
        .publish(new_key, &[])
        .expect("the game's new key")
        .expect("a reclamation");
    assert_eq!(plan_keys(&plan), [offset_key.as_str()]);
    assert_eq!(plan.bound(), MAX_GAME_FACTS);
    assert!(!store.active(&g, &offset_key));
    assert_eq!(store.count(&g), MAX_GAME_FACTS);
    let warning = store
        .events(EventType::RuntimeFailed)
        .pop()
        .expect("the warning");
    let outcome = reclaimed(&warning).expect("the reclamation's warning");
    assert_eq!(
        outcome.value("game"),
        Some(&serde_json::Value::from("game-one")),
        "{outcome:?}"
    );
    let refused = store
        .live
        .preview_observation(&old, store.now_ms, &store.ledger)
        .expect_err("the offset's snapshot");
    assert_eq!(refused.code(), SNAPSHOT_INVALIDATED, "{refused:?}");
    store.close();
}

// Test 8: a store a build before 0.12.76 filled past its bound (240 facts of one instance,
// appended without the store's check) reclaims what takes it back to its bound with a new key (17)
// when its facts expired long ago cover it, and is refused as before when they do not (10).
#[test]
fn factr_h_a_store_past_its_bound_reclaims_back_to_it_or_is_refused() {
    for (expired, takes) in [(10, false), (20, true)] {
        let mut older = Store::new();
        let one = older.instance_id();
        let a = instance("alias-a");
        let expiring = older.observation(
            &a,
            &keys("expired", 0..expired),
            Some(BASE_MS + DAY_MS),
            &[],
        );
        older.append(expiring, &[one], fact_store_origin());
        let plain = older.observation(&a, &keys("plain", 0..240 - expired), None, &[]);
        older.append(plain, &[one], fact_store_origin());
        assert_eq!(older.count(&a), 240);
        older.jump(LATER_MS);
        let head = older.head();
        let new_key = older.observation(&a, &keys("new", 0..1), None, &[]);
        if takes {
            let plan = older
                .publish(new_key, &[one])
                .expect("the new key")
                .expect("a reclamation");
            assert_eq!(plan.records().len(), 240 + 1 - MAX_INSTANCE_FACTS);
            assert_eq!(older.count(&a), MAX_INSTANCE_FACTS);
        } else {
            assert_full(
                older.publish(new_key, &[one]),
                "too few facts expired long ago",
            );
            assert_eq!(older.head(), head, "nothing is written");
            assert_eq!(older.count(&a), 240);
        }
        older.close();
    }
}

// One made-up fact of ALIAS, observed nine days before `now` and expired eight days before it.
fn expired_fill(index: usize, now: u64) -> FactRecord {
    record(
        &instance(ALIAS),
        &format!("session.example.fill_{index:03}"),
        now - 9 * DAY_MS,
        Some(now - 8 * DAY_MS),
        &[],
        &format!("snapshot:factr-fill-{index:03}"),
    )
}

// The instance's own records, as its snapshot holds them.
fn own_facts(host: &RuntimeHost, alias: &str) -> usize {
    host.instance_fact_snapshot(InstanceFactContext {
        instance_id: alias.to_owned(),
        server_id: "test".to_owned(),
        game_id: "neutral".to_owned(),
    })
    .expect("the instance's facts")
    .records
    .iter()
    .filter(|record| record.scope == instance(alias))
    .count()
}

// Test 9: on the S168-4 gate's host, a run whose new list reading meets the instance at its bound,
// filled with facts expired eight days before, ends `completed`, not
// `contained_task_resource_reading_rejected`: its list is published and the instance stays at its
// bound. One Warning `fact_store_expired_facts_reclaimed` stands under the system links
// (`instance_id` the alias, `count` 1, `limit_count` 224), and one `fact.invalidated` names it:
// the first fill, whose time is its expiry.
#[test]
fn factr_i_a_runs_new_list_on_a_full_instance_takes_the_room_of_expired_facts() {
    let root = TempDir::new().expect("tempdir");
    let first_list = write_package(&root, "list-a.zip", LIST_KEY);
    let second_list = write_package(&root, "list-b.zip", OTHER_KEY);
    let device = Device::new();
    let host = start(&root, &[(ALIAS, &device)]);
    device.shows(vec![list(0..6), detail()]);
    let (receipt, _) = run(&host, ALIAS, &first_list);
    completed(&host, &receipt);

    let now = unix_ms_now().expect("wall clock");
    let own = own_facts(&host, ALIAS);
    for index in own..MAX_INSTANCE_FACTS {
        host.publish_fact(expired_fill(index, now))
            .expect("a fill expired long ago");
    }
    assert_eq!(own_facts(&host, ALIAS), MAX_INSTANCE_FACTS);

    device.shows(vec![list(0..6), detail()]);
    let (receipt, _) = run(&host, ALIAS, &second_list);
    completed(&host, &receipt);
    assert_eq!(publications(&host, OTHER_KEY).len(), 1);
    assert_eq!(own_facts(&host, ALIAS), MAX_INSTANCE_FACTS);

    let mut failed = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::RuntimeFailed),
            ..EventQuery::default()
        })
        .expect("the runtime.failed records");
    failed.sort_by_key(PersistedEvent::sequence);
    let warnings = failed
        .into_iter()
        .filter_map(|event| reclaimed(&event).map(|outcome| (event, outcome)))
        .collect::<Vec<_>>();
    let [(warning, outcome)] = warnings.as_slice() else {
        panic!("one reclamation, {} found", warnings.len());
    };
    assert_eq!(warning.severity(), EventSeverity::Warning);
    assert!(warning.links().instance_id().is_none(), "the system links");
    assert_eq!(
        outcome.value("instance_id"),
        Some(&serde_json::Value::from(ALIAS)),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("count"),
        Some(&serde_json::Value::from(1)),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("limit_count"),
        Some(&serde_json::Value::from(MAX_INSTANCE_FACTS)),
        "{outcome:?}"
    );
    let reclaimed_facts = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::FactInvalidated),
            ..EventQuery::default()
        })
        .expect("the invalidations")
        .iter()
        .map(invalidation)
        .filter(|data| data.invalidated_by_event_id == *warning.event_id())
        .collect::<Vec<_>>();
    let [data] = reclaimed_facts.as_slice() else {
        panic!("one reclaimed fact, {} found", reclaimed_facts.len());
    };
    assert_eq!(data.key, format!("session.example.fill_{own:03}"));
    assert_eq!(data.invalidated_at_unix_ms, now - 8 * DAY_MS);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// Test 10: a fact that lists `runtime.failed` is dropped by the reclamation's warning by the
// ordinary rule and counts as freed. On test 4's history the first publication of two new keys
// reclaims one fact only: its plan frees two, the listing fact's invalidation names the warning
// with the warning's own time, and the reclaimed fact's names it with its expiry.
#[test]
fn factr_j_a_fact_that_lists_runtime_failed_is_dropped_by_the_warning_and_counted_as_freed() {
    let mut store = Store::new();
    let (first, _) = two_reclamations(&mut store);
    assert_eq!(plan_keys(&first), ["env.day1_000"]);
    assert_eq!(first.freed(), 2);
    let a = instance("alias-a");
    assert!(!store.active(&a, "env.listing_000"));
    let warning = store
        .events(EventType::RuntimeFailed)
        .into_iter()
        .next()
        .expect("the first warning");
    let by_first = store
        .events(EventType::FactInvalidated)
        .iter()
        .map(invalidation)
        .filter(|data| data.invalidated_by_event_id == *warning.event_id())
        .map(|data| (data.key, data.invalidated_at_unix_ms))
        .collect::<Vec<_>>();
    assert_eq!(
        by_first,
        [
            ("env.listing_000".to_owned(), warning.timestamp_unix_ms()),
            ("env.day1_000".to_owned(), BASE_MS + DAY_MS),
        ]
    );
    assert!(BASE_MS + DAY_MS + FACT_RECLAIM_AFTER_EXPIRY_MS < warning.timestamp_unix_ms());
    store.close();
}

// Test 11: the Runtime's declaration says `ledger` [3, 11], and the interfaces document's `ledger`
// row of revision 11 states that range.
#[test]
fn factr_k_the_declaration_and_the_ledger_row_say_3_to_11() {
    let declaration: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../distribution/windows/component-interfaces.json"
    ))
    .expect("the declaration is JSON");
    assert_eq!(declaration["speaks"]["ledger"], serde_json::json!([3, 11]));
    let row = include_str!("../../../../contracts/component-interfaces.md")
        .lines()
        .skip_while(|line| !line.starts_with("| `ledger` |"))
        .skip(1)
        .take_while(|line| line.starts_with("| | | | "))
        .find(|line| line.starts_with("| | | | 11 |"))
        .expect("the ledger row of revision 11");
    assert!(row.contains("declares [3, 11]"), "{row}");
}
