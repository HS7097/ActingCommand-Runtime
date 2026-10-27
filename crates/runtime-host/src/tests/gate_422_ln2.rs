// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #422 LN-2 (LEAN-L model §1.2 H-a, §3.2 step 5, §5.1 row LN-2, §7 A3 and A6;
// `contracts/ledger-observer.md` §11.3, §11.5): the instance fact store loads from the ledger's
// tagged `state` rows instead of replaying the whole ledger, and the load equals the replay: the
// active records with their sources, `latest_observed`, the input boundaries, the latest
// tombstone of each key without an active record, the pending invalidations and the position.
// The test-only replay is the start's reducer over every event, as the start ran it before LN-2.
// The rows: the synthetic ledger of LN-1, fixtures with input boundaries, and a doctored row.

use super::*;
use crate::fact_store::{FactStoreView, InstanceFactStore, TAGGED_STATE_PREFIXES};
use actingcommand_contract::{
    AuditInput, DiagnosticCode, EffectDisposition, EventAction, EventDraft, EventLinksDraft,
    EventOrigin, EventPayloadDraft, FactObservation, FactPayloadDraft, InputPayloadDraft,
    IssuedInstanceId, RuntimePayloadDraft, TaskPayloadDraft,
};
use actingcommand_ledger::{
    GlobalLedgerConfig, LedgerMaintenance, Sha256SecretFingerprinter, SyntheticInstance,
    SyntheticLedgerSpec, write_synthetic_ledger,
};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};

const BASE_MS: u64 = 1_750_000_000_000;

fn load_and_replay(
    ledger: &GlobalLedger,
    state: &Arc<RuntimeStateStore>,
) -> (FactStoreView, FactStoreView) {
    let loaded = InstanceFactStore::recover(ledger, Arc::clone(state)).expect("the tagged load");
    let replayed =
        InstanceFactStore::replay_for_test(ledger, Arc::clone(state)).expect("the full replay");
    (loaded.view_for_test(), replayed.view_for_test())
}

// LN-2 row, test 1: on LN-1's synthetic ledger (about 3,000 records: runs, frames, steps with
// inputs, instance facts of 24 keys, workbench records) the load equals the replay, makes no
// whole-ledger query, and no read returns more events than the tagged rows.
#[test]
fn ln2_a_the_tagged_load_equals_the_replay_on_the_synthetic_ledger() {
    let root = TempDir::new().expect("the state root");
    super::lean_scale::formal_root(root.path());
    let issuer = IdentifierIssuer::new().expect("issuer");
    let instance_id = *issuer.mint_instance_id().expect("instance id").transport();
    let head = {
        let database =
            RuntimeDatabase::open_existing(root.path(), false).expect("the root's database");
        let spec = SyntheticLedgerSpec::new(
            3_000,
            vec![SyntheticInstance {
                alias: "synthetic-instance-a".to_owned(),
                instance_id,
            }],
        );
        let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
        assert_eq!(generated.records, 3_000, "{generated:?}");
        assert!(generated.instance_facts > 0, "{generated:?}");
        generated.head_sequence
    };
    let database = Arc::new(
        RuntimeStateStore::open_database(root.path(), super::lean_scale::SALT)
            .expect("the State database"),
    );
    let limits = MaintenanceLimits::default();
    let ledger = LedgerMaintenance::acquire(
        root.path(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("the maintenance owner")
    .open_writer(Arc::clone(&database), super::lean_scale::OWNER.into())
    .expect("the writer");
    let state =
        Arc::new(RuntimeStateStore::from_database(Arc::clone(&database)).expect("state store"));

    ledger.take_writer_peak();
    let loaded = InstanceFactStore::recover(&ledger, Arc::clone(&state)).expect("the tagged load");
    let peak = ledger.take_writer_peak();
    let rows = ledger
        .tagged_state(&TAGGED_STATE_PREFIXES)
        .expect("the tagged state")
        .records
        .len();
    let replayed =
        InstanceFactStore::replay_for_test(&ledger, Arc::clone(&state)).expect("the full replay");
    let (loaded, replayed) = (loaded.view_for_test(), replayed.view_for_test());

    assert_eq!(loaded, replayed);
    assert!(
        !loaded.active.is_empty() && loaded.input_boundaries.contains_key(&instance_id),
        "the synthetic history leaves active facts and an input boundary: {loaded:?}"
    );
    assert!(loaded.last_sequence >= head, "{loaded:?}");
    assert_eq!(peak.whole_ledger_queries, 0, "{peak:?}");
    assert!(
        peak.largest_read_events <= rows as u64,
        "no read returns more than the {rows} tagged rows: {peak:?}"
    );
    ledger.close().expect("close the writer");
}

// A ledger with a State store and a live fact store that follows it as the host's does: the
// fixture appends records and persists only the pending invalidations a scenario names.
struct Fixture {
    _root: TempDir,
    database: Arc<RuntimeDatabase>,
    state: Arc<RuntimeStateStore>,
    ledger: GlobalLedger,
    live: InstanceFactStore,
    issuer: IdentifierIssuer,
    fingerprinter: Sha256SecretFingerprinter,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().expect("tempdir");
        let database = Arc::new(
            RuntimeStateStore::open_database(root.path(), b"0123456789abcdef").expect("database"),
        );
        let state =
            Arc::new(RuntimeStateStore::from_database(Arc::clone(&database)).expect("state store"));
        let ledger = GlobalLedger::open_sqlite_candidate(
            GlobalLedgerConfig::new(root.path(), "ln2-fixture"),
            Arc::clone(&database),
        )
        .expect("ledger");
        let live = InstanceFactStore::recover(&ledger, Arc::clone(&state)).expect("the live store");
        Self {
            _root: root,
            database,
            state,
            ledger,
            live,
            issuer: IdentifierIssuer::new().expect("issuer"),
            fingerprinter: Sha256SecretFingerprinter::new(b"ln2-fixture-specification-salt")
                .expect("salt"),
        }
    }

    fn instance(&self) -> IssuedInstanceId {
        self.issuer.mint_instance_id().expect("instance id")
    }

    fn append(
        &self,
        at_ms: u64,
        severity: EventSeverity,
        module: OriginModule,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> PersistedEvent {
        let draft = self.draft(at_ms, severity, module, links, payload);
        self.ledger.append(draft).expect("append")
    }

    fn draft(
        &self,
        at_ms: u64,
        severity: EventSeverity,
        module: OriginModule,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> actingcommand_contract::SanitizedEventDraft {
        EventDraft::new(
            self.issuer.mint_event_id().expect("event id"),
            BASE_MS + at_ms,
            severity,
            EventOrigin::new(EventSource::Runtime, module, EventActor::Runtime),
            links,
            payload,
        )
        .sanitize(&self.fingerprinter)
        .expect("draft")
    }

    // One observation of `keys` in the scope of `alias`, observed at `observed_ms`.
    #[allow(clippy::too_many_arguments)]
    fn publish(
        &self,
        at_ms: u64,
        alias: &str,
        keys: &[&str],
        snapshot: &str,
        observed_ms: u64,
        invalidate_on: Vec<EventType>,
        scope_instances: &[IssuedInstanceId],
    ) -> PersistedEvent {
        let records = keys
            .iter()
            .map(|key| FactRecord {
                scope: FactScope::Instance {
                    instance_id: alias.to_owned(),
                },
                key: (*key).to_owned(),
                content: FactContent::Inline {
                    value: ContractFactValue::Integer(7),
                },
                observed_at_unix_ms: BASE_MS + observed_ms,
                expires_at_unix_ms: None,
                ttl_policy: None,
                confidence_milli: 1_000,
                source_detector: "neutral-detector".to_owned(),
                source_snapshot_id: snapshot.to_owned(),
                schema_version: "fact.v1".to_owned(),
                resource_bundle_hash: "5".repeat(64),
                invalidate_on: invalidate_on.clone(),
            })
            .collect();
        let payload = FactPayloadDraft::observation(
            FactObservation { records },
            scope_instances
                .iter()
                .map(|instance| *instance.transport())
                .collect(),
            AuditInput::new(),
        )
        .expect("observation");
        self.append(
            at_ms,
            EventSeverity::Info,
            OriginModule::FactStore,
            EventLinksDraft::default(),
            payload.into(),
        )
    }

    fn input(&self, at_ms: u64, instance: IssuedInstanceId, failed: bool) -> PersistedEvent {
        let links = EventLinksDraft::default()
            .with_instance_id(instance)
            .with_action_id(self.issuer.mint_action_id().expect("action id"));
        let (severity, payload) = if failed {
            (
                EventSeverity::Error,
                InputPayloadDraft::failed(
                    EventAction::InputTap,
                    DiagnosticCode::InputFailed,
                    EffectDisposition::NotPerformed,
                    AuditInput::new(),
                ),
            )
        } else {
            (
                EventSeverity::Info,
                InputPayloadDraft::committed(
                    EventAction::InputTap,
                    EffectDisposition::Performed,
                    AuditInput::new(),
                ),
            )
        };
        self.append(
            at_ms,
            severity,
            OriginModule::Runtime,
            links,
            payload.into(),
        )
    }

    fn takeover(&self, at_ms: u64) -> PersistedEvent {
        self.append(
            at_ms,
            EventSeverity::Info,
            OriginModule::Runtime,
            EventLinksDraft::default(),
            RuntimePayloadDraft::takeover(EventAction::RuntimeTakeover, AuditInput::new()).into(),
        )
    }

    fn step_finished(&self, at_ms: u64, instance: IssuedInstanceId) -> PersistedEvent {
        self.append(
            at_ms,
            EventSeverity::Info,
            OriginModule::Runtime,
            EventLinksDraft::default()
                .with_instance_id(instance)
                .with_action_id(self.issuer.mint_action_id().expect("action id")),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::StepFinished {
                    step_index: 0,
                    operation_label: "neutral-operation".to_owned(),
                    page_label: "neutral-page".to_owned(),
                    phase: None,
                },
                AuditInput::new(),
            )
            .into(),
        )
    }

    // The live store follows the ledger; of the pending invalidations it derived, only those of
    // `keys` are written, each with its State tombstone in one transaction, as the host writes
    // them (`synchronize_fact_store_under_gate`).
    fn persist_pending(&mut self, at_ms: u64, keys: &[&str]) {
        self.live.synchronize(&self.ledger).expect("synchronize");
        for pending in self.live.pending_invalidations() {
            if !keys.contains(&pending.data.key.as_str()) {
                continue;
            }
            let work = self
                .live
                .prepare_invalidation(&self.ledger, &pending)
                .expect("prepare the tombstone");
            let draft = self.draft(
                at_ms,
                EventSeverity::Info,
                OriginModule::FactStore,
                EventLinksDraft::default(),
                FactPayloadDraft::invalidated(pending.data.clone(), AuditInput::new()).into(),
            );
            let event = self
                .ledger
                .append_transaction(draft, Box::new(work))
                .expect("the joint invalidation");
            self.live
                .acknowledge_generated_invalidation(&pending.data, event.sequence())
                .expect("acknowledge");
        }
    }

    // An adjustment of `key` (Workflow #384 D2-S3): its `fact.invalidated` names the step.
    fn invalidate_adjusted(&mut self, at_ms: u64, alias: &str, key: &str, base: u64, step: u64) {
        self.live.synchronize(&self.ledger).expect("synchronize");
        let (data, work) = self
            .live
            .prepare_adjustment_invalidation(
                &self.ledger,
                &FactScope::Instance {
                    instance_id: alias.to_owned(),
                },
                key,
                base,
                step,
            )
            .expect("prepare the adjustment")
            .expect("the adjusted record is active");
        let draft = self.draft(
            at_ms,
            EventSeverity::Info,
            OriginModule::FactStore,
            EventLinksDraft::default(),
            FactPayloadDraft::invalidated(data, AuditInput::new()).into(),
        );
        self.ledger
            .append_transaction(draft, Box::new(work))
            .expect("the adjustment's invalidation");
        self.live.synchronize(&self.ledger).expect("synchronize");
    }

    // Runs `statement` on the root's database, as a damaged or edited root would hold it.
    fn doctor(&self, statement: &str) {
        self.database
            .connection(DatabaseLocation::OpenExistingDatabase.location())
            .expect("database connection")
            .execute_batch(statement)
            .expect("doctor the index");
    }
}

fn key_of<'a>(view: &'a FactStoreView, key: &str) -> Vec<&'a FactRecord> {
    view.active
        .iter()
        .map(|active| &active.record)
        .filter(|record| record.key == key)
        .collect()
}

// LN-2 row, test 2: fixtures with input boundaries. The load equals the replay for a fact
// invalidated by an input and republished, a fact left pending by an `input.failed` that an
// `input.committed` follows (the first trigger is the failure), a two-key publication one key of
// which an adjustment invalidated (the rows name only the other key), a `runtime.takeover` that
// invalidated one fact with its record written and left another pending, and the input
// boundary of an instance without facts.
#[test]
fn ln2_b_the_tagged_load_equals_the_replay_with_input_boundaries() {
    let mut fixture = Fixture::new();
    let (one, two, three) = (fixture.instance(), fixture.instance(), fixture.instance());
    let inputs = vec![EventType::InputCommitted, EventType::InputFailed];

    // Invalidated by an input, written, then republished.
    fixture.publish(
        20,
        "alias-one",
        &["env.alpha"],
        "snapshot:alpha-1",
        10,
        inputs.clone(),
        &[one],
    );
    fixture.input(100, one, false);
    fixture.persist_pending(110, &["env.alpha"]);
    fixture.publish(
        160,
        "alias-one",
        &["env.alpha"],
        "snapshot:alpha-2",
        150,
        inputs.clone(),
        &[one],
    );

    // Left pending: the failure comes first, then a commit.
    fixture.publish(
        210,
        "alias-two",
        &["env.charlie"],
        "snapshot:charlie",
        200,
        inputs,
        &[two],
    );
    let failure = fixture.input(300, two, true);
    fixture.input(400, two, false);

    // Two keys; an adjustment invalidates one of them.
    let pair = fixture.publish(
        510,
        "alias-one",
        &["resource.gamma", "env.beta"],
        "snapshot:pair",
        500,
        Vec::new(),
        &[one],
    );
    let step = fixture.step_finished(600, one);
    fixture.invalidate_adjusted(
        610,
        "alias-one",
        "resource.gamma",
        pair.sequence(),
        step.sequence(),
    );

    // A takeover: one invalidation written, one left pending.
    let takeover_facts = vec![EventType::RuntimeTakeover];
    fixture.publish(
        710,
        "alias-one",
        &["env.delta"],
        "snapshot:delta",
        700,
        takeover_facts.clone(),
        &[one],
    );
    fixture.publish(
        730,
        "alias-one",
        &["env.epsilon"],
        "snapshot:epsilon",
        720,
        takeover_facts,
        &[one],
    );
    let takeover = fixture.takeover(800);
    fixture.persist_pending(810, &["env.delta"]);

    // The boundary of an instance without facts.
    fixture.input(900, three, false);

    let (loaded, replayed) = load_and_replay(&fixture.ledger, &fixture.state);
    assert_eq!(loaded, replayed);

    // The scenarios left what they set out to leave.
    assert_eq!(
        key_of(&loaded, "env.alpha")
            .iter()
            .map(|record| record.source_snapshot_id.as_str())
            .collect::<Vec<_>>(),
        vec!["snapshot:alpha-2"]
    );
    assert_eq!(key_of(&loaded, "env.beta").len(), 1);
    assert!(key_of(&loaded, "resource.gamma").is_empty());
    let pending = loaded
        .pending
        .iter()
        .map(|(data, _, trigger)| (data.key.as_str(), *trigger))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        pending,
        BTreeSet::from([
            ("env.charlie", failure.sequence()),
            ("env.epsilon", takeover.sequence()),
        ])
    );
    let tombstones = loaded
        .tombstones
        .keys()
        .map(|(_, key)| key.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        tombstones,
        BTreeSet::from(["env.charlie", "resource.gamma", "env.delta", "env.epsilon"])
    );
    assert!(loaded.tombstones.len() < 256);
    for instance in [one, two, three] {
        assert!(
            loaded.input_boundaries.contains_key(instance.transport()),
            "{loaded:?}"
        );
    }
    fixture.ledger.close().expect("close");
}

// LN-2 row, test 3: an index row that disagrees with its record stops the load with the fatal
// `ledger_tag_index_mismatch`: once with an edited value hash, once with the row pointed at
// another record.
#[test]
fn ln2_c_a_doctored_index_row_stops_the_load_with_ledger_tag_index_mismatch() {
    const KEY: &str = "instance_facts/instance:alias-one/env.alpha";
    let doctored = [
        format!(
            "UPDATE ledger_index SET value_sha256='sha256:{}' WHERE role='state' AND key='{KEY}'",
            "0".repeat(64)
        ),
        format!("UPDATE ledger_index SET sequence=sequence-1 WHERE role='state' AND key='{KEY}'"),
    ];
    for statement in doctored {
        let fixture = Fixture::new();
        let one = fixture.instance();
        fixture.publish(
            10,
            "alias-one",
            &["env.zeta"],
            "snapshot:zeta",
            5,
            Vec::new(),
            &[one],
        );
        fixture.publish(
            20,
            "alias-one",
            &["env.alpha"],
            "snapshot:alpha",
            15,
            Vec::new(),
            &[one],
        );
        InstanceFactStore::recover(&fixture.ledger, Arc::clone(&fixture.state))
            .expect("the undoctored load");

        fixture.doctor(&statement);
        let error = InstanceFactStore::recover(&fixture.ledger, Arc::clone(&fixture.state))
            .err()
            .expect("the doctored row stops the load");
        assert_eq!(error.code(), "ledger_tag_index_mismatch", "{statement}");
        assert!(error.is_fatal(), "{statement}");
        let outcome = error.outcome();
        assert_eq!(
            outcome.value("fact_key"),
            Some(&serde_json::Value::String(
                "instance_facts:instance:alias-one:env.alpha".to_owned()
            )),
            "{statement}: {outcome:?}"
        );
        fixture.ledger.close().expect("close");
    }
}
