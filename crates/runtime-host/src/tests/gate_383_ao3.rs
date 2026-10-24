// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #383 AO-3, the host test rows AO3-a to AO3-i (model-ledger-observer v1.4.1 §14.2;
// `contracts/ledger-observer.md` §3, §4, §13): the instance fact store applies a change first,
// counts it in its lineage's revision and is copied to the ledger by the gates; every recorded
// event reaches the store's `observe_trigger` at its capture; snapshot ids follow the two stores'
// revisions, not the ledger head. Every expected value comes from the model rows named on each
// test.
//
// The two stores' (lineage, revision) are read through a status read, which appends nothing
// (AO-4q). The host's capture point keeps, in tests, the order of what it handed the ledger and
// whether each hand-off waited for the commit.

use super::*;
use actingcommand_contract::{
    FactPayload, LedgerSource, RuntimeStateStoreRevision, SourceRevision, TaskSemanticFact,
};
use actingcommand_ledger::WriterStallGate;

const WAKE_WAIT: Duration = Duration::from_secs(10);

fn primary_context() -> InstanceFactContext {
    InstanceFactContext {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        server_id: "fixture-server-a".to_owned(),
        game_id: "fixture-game-a".to_owned(),
    }
}

// An instance-scoped record of the primary instance.
fn instance_fact(key: &str, snapshot: &str, invalidate_on: Vec<EventType>) -> FactRecord {
    stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        key,
        ContractFactValue::Integer(7),
        snapshot,
        invalidate_on,
    )
}

// The two stores' (lineage, revision), as a status read names them.
fn store_revisions(
    client: &mut TestClient,
) -> (RuntimeStateStoreRevision, RuntimeStateStoreRevision) {
    let request = client.request(RuntimeOperation::Status);
    let receipt = client.send(&request);
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("expected a status, got {:?}", receipt.result());
    };
    let source = status.source().expect("the status names its source");
    (
        source
            .instance_facts
            .expect("the instance fact store's lineage and revision"),
        source
            .runtime_facts
            .expect("the workbench's lineage and revision"),
    )
}

// Whether the primary instance's facts hold an active record of `key`.
fn holds(host: &RuntimeHost, key: &str) -> bool {
    host.instance_fact_snapshot(primary_context())
        .expect("the primary instance's facts")
        .records
        .iter()
        .any(|record| record.key == key)
}

fn events_of(host: &RuntimeHost, event_type: EventType) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(event_type),
        ..EventQuery::default()
    })
    .expect("ledger events")
}

// The store changes the capture point handed the ledger since `from`: (type, waited).
fn store_captures_since(host: &RuntimeHost, from: usize) -> Vec<(EventType, bool)> {
    host.ledger_captures_for_test()
        .expect("the host's captures")[from..]
        .iter()
        .filter(|capture| {
            matches!(
                capture.event_type,
                EventType::FactPublished | EventType::FactInvalidated
            )
        })
        .map(|capture| (capture.event_type, capture.committed))
        .collect()
}

fn capture_count(host: &RuntimeHost) -> usize {
    host.ledger_captures_for_test()
        .expect("the host's captures")
        .len()
}

fn send_completed(client: &mut TestClient, operation: RuntimeOperation) -> RuntimeRequest {
    let request = client.request(operation);
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{:?}: {receipt:?}",
        request.operation()
    );
    request
}

fn record_action(client: &mut TestClient, target: &str) -> RuntimeRequest {
    send_completed(
        client,
        RuntimeOperation::RecordClientAction {
            action: ClientActionRecord::new(
                "settings",
                target,
                ClientActionKind::Button,
                None,
                None,
            )
            .expect("client action"),
        },
    )
}

// AO3-a (rules 1, 6): publishing a fact moves the instance fact store's revision by one in the
// same lineage and the value is readable at once; the host's capture point captured the
// publication once, carrying the store's (lineage, revision), and made no synchronous ledger
// call for it.
#[test]
fn ao3_a_a_publication_is_one_store_revision_and_one_capture_without_a_synchronous_call() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let mut client = TestClient::connect(&host);
    let (before, _) = store_revisions(&mut client);
    let from = capture_count(&host);

    let record = instance_fact("inventory.ao3_a", "snapshot:ao3-a", Vec::new());
    let event_id = host.publish_fact(record.clone()).expect("publish the fact");

    assert_eq!(
        store_captures_since(&host, from),
        [(EventType::FactPublished, false)],
        "one capture, no synchronous ledger call"
    );
    let (after, _) = store_revisions(&mut client);
    assert_eq!(after.lineage, before.lineage, "the same lineage");
    assert_eq!(after.revision, before.revision + 1, "one change");
    assert!(
        host.instance_fact_snapshot(primary_context())
            .expect("the primary instance's facts")
            .records
            .contains(&record),
        "the value is readable"
    );
    let published = events_of(&host, EventType::FactPublished);
    let stored = published
        .iter()
        .find(|event| *event.event_id() == event_id)
        .expect("the publication's record");
    assert_eq!(
        stored.capture().and_then(|capture| capture.source),
        Some(SourceRevision {
            source: LedgerSource::InstanceFacts,
            lineage: after.lineage,
            revision: after.revision,
        }),
        "the record carries the store's (lineage, revision)"
    );
    drop(client);
    host.close().expect("close host");
}

// AO3-b (rule 1; D2 A-2): while the ledger's flush is held, a publication still succeeds and the
// policy driver's business notification still goes out.
#[test]
fn ao3_b_a_publication_while_the_flush_is_held_succeeds_and_wakes_the_driver() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let wakes = host.open_policy_wakes().expect("the driver's wakes");
    let gate = WriterStallGate::arm(root.path()).expect("arm the writer stall gate");

    let event_id = host
        .publish_fact(instance_fact(
            "inventory.ao3_b",
            "snapshot:ao3-b",
            Vec::new(),
        ))
        .expect("the publication succeeds while the flush is held");
    assert!(
        wakes.take(WAKE_WAIT).contains(&PolicyTrigger::FactsChanged),
        "the driver is woken"
    );
    assert!(
        gate.wait_held_commit(WAKE_WAIT),
        "the flush holds the publication's record"
    );

    gate.release();
    assert!(
        events_of(&host, EventType::FactPublished)
            .iter()
            .any(|event| *event.event_id() == event_id),
        "the record is committed once the flush goes on"
    );
    host.close().expect("close host");
}

// AO3-c (§3.5): an intent evaluated before an input and admitted after it is refused
// `policy_facts_stale`, through a fact the instances of one server share, which the input
// invalidated.
#[test]
fn ao3_c_an_input_between_evaluation_and_admission_makes_the_intent_stale() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    host.publish_fact(stored_fact(
        FactScope::Server {
            server_id: "fixture-server-a".to_owned(),
        },
        "env.shared_state",
        ContractFactValue::Boolean(true),
        "snapshot:ao3-c",
        vec![EventType::InputCommitted, EventType::InputFailed],
    ))
    .expect("publish the shared fact");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);

    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire(POLICY_INSTANCE_ALIAS);
    send_completed(
        &mut client,
        RuntimeOperation::Input {
            frame: None,
            token: token.clone(),
            action: InputAction::Tap { x: 10, y: 20 },
        },
    );
    send_completed(&mut client, RuntimeOperation::ReleaseLease { token });
    drop(client);

    let error = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect_err("an input after the evaluation makes the intent stale");
    assert_eq!(error.code(), "policy_facts_stale");
    assert!(!error.is_fatal());
    host.close().expect("close host");
}

// AO3-d (§3.3): a fact whose `invalidate_on` names an event type is invalidated as soon as that
// event is emitted, and the capture order is the triggering event, then the invalidation.
#[test]
fn ao3_d_the_event_a_fact_waits_on_invalidates_it_at_once_and_comes_first() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    host.publish_fact(instance_fact(
        "inventory.ao3_d",
        "snapshot:ao3-d",
        vec![EventType::InputCommitted, EventType::InputFailed],
    ))
    .expect("publish the fact");

    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire(POLICY_INSTANCE_ALIAS);
    let input = send_completed(
        &mut client,
        RuntimeOperation::Input {
            frame: None,
            token,
            action: InputAction::Tap { x: 10, y: 20 },
        },
    );
    assert!(
        !holds(&host, "inventory.ao3_d"),
        "the fact is invalidated at once"
    );

    let committed = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::InputCommitted),
            request_id: Some(input.request_id()),
            ..EventQuery::default()
        })
        .expect("the input's record");
    let [committed] = committed.as_slice() else {
        panic!("one input.committed: {committed:?}");
    };
    let invalidated = events_of(&host, EventType::FactInvalidated);
    let invalidation = invalidated
        .iter()
        .find(|event| {
            matches!(
                event.payload(),
                EventPayload::Fact(FactPayload::Invalidated(payload))
                    if payload.invalidation().key == "inventory.ao3_d"
            )
        })
        .expect("the fact's invalidation");
    let EventPayload::Fact(FactPayload::Invalidated(payload)) = invalidation.payload() else {
        unreachable!("selected above");
    };
    assert_eq!(
        payload.invalidation().invalidated_by_event_id,
        *committed.event_id()
    );
    assert_eq!(
        payload.invalidation().invalidated_by_event_type,
        EventType::InputCommitted
    );
    assert!(
        committed.sequence() < invalidation.sequence(),
        "the triggering event is captured before the invalidation it caused"
    );
    drop(client);
    host.close().expect("close host");
}

// AO3-e (§3.4): the snapshot id depends only on the two stores' revisions and the records: a
// ledger head that moved while neither store changed leaves it as it was.
#[test]
fn ao3_e_the_snapshot_id_follows_the_store_revisions_and_records_not_the_ledger_head() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let mut client = TestClient::connect(&host);
    host.publish_fact(instance_fact(
        "inventory.ao3_e",
        "snapshot:ao3-e",
        Vec::new(),
    ))
    .expect("publish the fact");
    let revisions = store_revisions(&mut client);
    let first = host
        .instance_fact_snapshot(primary_context())
        .expect("the first snapshot");

    record_action(&mut client, "ao3-e");

    let second = host
        .instance_fact_snapshot(primary_context())
        .expect("the second snapshot");
    assert_eq!(
        store_revisions(&mut client),
        revisions,
        "neither store changed"
    );
    assert_ne!(
        second.ledger_position, first.ledger_position,
        "the ledger head moved"
    );
    assert_eq!(second.records, first.records);
    assert_eq!(second.snapshot_id, first.snapshot_id);
    drop(client);
    host.close().expect("close host");
}

// AO3-f (§3.2): the store's acceptance rules are today's: the identical record is idempotent, an
// observation of the same or an older millisecond is refused, and a full store refuses a new key
// and evicts nothing.
#[test]
fn ao3_f_the_acceptance_rules_stay() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let mut client = TestClient::connect(&host);
    let base = instance_fact("inventory.ao3_f", "snapshot:ao3-f", Vec::new());
    let first = host.publish_fact(base.clone()).expect("publish the fact");
    let mut older = base.clone();
    older.source_snapshot_id = "snapshot:ao3-f-older".to_owned();
    older.observed_at_unix_ms -= 1;
    let mut same_millisecond = base.clone();
    same_millisecond.source_snapshot_id = "snapshot:ao3-f-same".to_owned();
    for (name, record, expected) in [
        ("the identical record", base, Ok(first)),
        (
            "an older observation",
            older,
            Err("fact_observation_not_newer"),
        ),
        (
            "an observation of the same millisecond",
            same_millisecond,
            Err("fact_observation_not_newer"),
        ),
    ] {
        let (revision, _) = store_revisions(&mut client);
        let result = host.publish_fact(record).map_err(|error| {
            assert!(!error.is_fatal(), "{name}: {error:?}");
            error.code().to_owned()
        });
        assert_eq!(result, expected.map_err(str::to_owned), "{name}");
        assert_eq!(store_revisions(&mut client).0, revision, "{name}");
    }

    // The host shares 256 active facts; the primary instance's facts are all of them here.
    let held = host
        .instance_fact_snapshot(primary_context())
        .expect("the primary instance's facts")
        .records
        .len();
    for index in held..256 {
        host.publish_fact(instance_fact(
            &format!("inventory.ao3_f_{index}"),
            &format!("snapshot:ao3-f-{index}"),
            Vec::new(),
        ))
        .expect("a fact below the limit");
    }
    let (revision, _) = store_revisions(&mut client);
    let error = host
        .publish_fact(instance_fact(
            "inventory.ao3_f_full",
            "snapshot:ao3-f-full",
            Vec::new(),
        ))
        .expect_err("a full store refuses a new key");
    assert_eq!(error.code(), "fact_store_capacity_exceeded");
    assert!(!error.is_fatal());
    assert_eq!(store_revisions(&mut client).0, revision);
    assert_eq!(
        host.instance_fact_snapshot(primary_context())
            .expect("the primary instance's facts")
            .records
            .len(),
        256,
        "nothing is evicted"
    );
    drop(client);
    host.close().expect("close host");
}

// AO3-g (rule 8; §3.2): a refused change leaves the store's revision as it was, captures nothing
// and leaves no `fact.*` record in the ledger.
#[test]
fn ao3_g_a_refused_change_moves_no_revision_and_records_nothing() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let mut client = TestClient::connect(&host);
    let base = instance_fact(
        "inventory.ao3_g",
        "snapshot:ao3-g",
        vec![EventType::ClientAction],
    );
    host.publish_fact(base.clone()).expect("publish the fact");
    let mut older = base.clone();
    older.source_snapshot_id = "snapshot:ao3-g-older".to_owned();
    older.observed_at_unix_ms -= 1;
    let mut future = base.clone();
    future.source_snapshot_id = "snapshot:ao3-g-future".to_owned();
    future.observed_at_unix_ms = u64::MAX / 2;
    future.expires_at_unix_ms = None;
    future.ttl_policy = None;
    let mut reserved = base.clone();
    reserved.key = format!(
        "{}ao3_g",
        actingcommand_execution_kernel::RUN_PARAMETERS_KEY_PREFIX
    );
    reserved.source_snapshot_id = "snapshot:ao3-g-reserved".to_owned();
    // The client action invalidates the fact; its own snapshot is refused afterwards.
    record_action(&mut client, "ao3-g");
    let mut republished = base;
    republished.observed_at_unix_ms += 1;

    for (name, record) in [
        ("an older observation", older),
        ("an observation in the future", future),
        ("a reserved key", reserved),
        ("an invalidated snapshot", republished),
    ] {
        let (revision, _) = store_revisions(&mut client);
        let from = capture_count(&host);
        let error = host.publish_fact(record.clone()).expect_err(name);
        assert!(!error.is_fatal(), "{name}: {error:?}");
        assert_eq!(store_revisions(&mut client).0, revision, "{name}");
        assert_eq!(store_captures_since(&host, from), [], "{name}");
        assert!(
            events_of(&host, EventType::FactPublished)
                .iter()
                .all(|event| !matches!(
                    event.payload(),
                    EventPayload::Fact(FactPayload::Published(payload))
                        if payload.records().contains(&record)
                )),
            "{name}: no record of it"
        );
    }
    drop(client);
    host.close().expect("close host");
}

// AO3-h (§3.3 invariant; review 2 H1): for each gate this test reaches, a fact whose
// `invalidate_on` names the type of the event the gate emits is invalidated as soon as the event
// is emitted: the fan-out point (G1, a client action), the queue and leases (G4, a grant and a
// release), the input (G6), the task trace (G7), the joint captures (G15, an approval and a
// planning signal) and the process lifecycle (G16, the start).
#[test]
fn ao3_h_each_gate_s_event_invalidates_the_fact_waiting_on_it_at_once() {
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    let start = |root: &TempDir| {
        RuntimeHost::start(
            config(root),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                registered,
                Arc::new(FakeState::default()),
            )),
        )
        .expect("runtime host")
    };
    let host = start(&root);
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let waiting = |host: &RuntimeHost, gate: &str, trigger: EventType| {
        let key = format!("inventory.ao3_h_{}", gate.to_ascii_lowercase());
        host.publish_fact(instance_fact(
            &key,
            &format!("snapshot:ao3-h-{gate}"),
            vec![trigger],
        ))
        .unwrap_or_else(|error| panic!("{gate}: publish the waiting fact: {error:?}"));
        assert!(holds(host, &key), "{gate}: the fact is active");
        key
    };
    let mut client = TestClient::connect(&host);

    let key = waiting(&host, "G1", EventType::ClientAction);
    record_action(&mut client, "ao3-h");
    assert!(!holds(&host, &key), "G1: invalidated at once");

    let key = waiting(&host, "G4_grant", EventType::LeaseGranted);
    let (acquire, token) = client.acquire(POLICY_INSTANCE_ALIAS);
    assert!(!holds(&host, &key), "G4 grant: invalidated at once");

    let key = waiting(&host, "G6", EventType::InputCommitted);
    send_completed(
        &mut client,
        RuntimeOperation::Input {
            frame: None,
            token: token.clone(),
            action: InputAction::Tap { x: 10, y: 20 },
        },
    );
    assert!(!holds(&host, &key), "G6: invalidated at once");

    let key = waiting(&host, "G7", EventType::TaskStepFinished);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    host.append_contained_task_semantic_for_test(
        &acquire,
        &token,
        ids.mint_task_id().expect("task id"),
        ids.mint_run_id().expect("run id"),
        TaskSemanticFact::StepFinished {
            step_index: 0,
            operation_label: "operation.observe".to_owned(),
            page_label: "page.ao3".to_owned(),
            phase: None,
        },
    )
    .expect("a task step record");
    assert!(!holds(&host, &key), "G7: invalidated at once");

    let key = waiting(&host, "G4_release", EventType::LeaseReleased);
    send_completed(&mut client, RuntimeOperation::ReleaseLease { token });
    assert!(!holds(&host, &key), "G4 release: invalidated at once");

    let key = waiting(&host, "G15_approval", EventType::ApprovalDecision);
    let (_, intent, _) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    assert!(!holds(&host, &key), "G15 approval: invalidated at once");

    let key = waiting(
        &host,
        "G15_planning",
        EventType::PolicyPlanningSignalObserved,
    );
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:ao3-h".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::GoalMissed,
        outcome: planning_signal_outcome(PolicyPlanningSignalKind::GoalMissed),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS + 1,
        detection_budget: None,
    })
    .expect("a planning signal");
    assert!(!holds(&host, &key), "G15 planning: invalidated at once");

    let key = waiting(&host, "G16", EventType::RuntimeStarted);
    drop(client);
    host.close().expect("close host");
    let host = start(&root);
    assert!(!holds(&host, &key), "G16: invalidated by the start");
    host.close().expect("close the restarted host");
}

// AO3-i (rules 7, 9, 14; §4.1), for the gates AO-3 builds: one business pass is captured
// exactly once with its type (the fan-out point G1, the instance fact store's entry G2, the
// process lifecycle G16), the gate refuses nothing to the business, and the store's entry
// captures without a wait.
#[test]
fn ao3_i_one_pass_is_one_capture_and_the_store_gate_does_not_wait() {
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    let start = |root: &TempDir| {
        RuntimeHost::start(
            config(root),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                registered,
                Arc::new(FakeState::default()),
            )),
        )
        .expect("runtime host")
    };
    let host = start(&root);
    let mut client = TestClient::connect(&host);

    // G2: a publication.
    let from = capture_count(&host);
    host.publish_fact(instance_fact(
        "inventory.ao3_i",
        "snapshot:ao3-i",
        Vec::new(),
    ))
    .expect("G2 refuses nothing to the business");
    assert_eq!(
        store_captures_since(&host, from),
        [(EventType::FactPublished, false)],
        "G2: one capture, without a wait"
    );

    // G1: a client action through the fan-out point.
    let from = capture_count(&host);
    record_action(&mut client, "ao3-i");
    let actions = host
        .ledger_captures_for_test()
        .expect("the host's captures")[from..]
        .iter()
        .filter(|capture| capture.event_type == EventType::ClientAction)
        .count();
    assert_eq!(actions, 1, "G1: one capture");
    drop(client);
    host.close().expect("close host");

    // G16: the start records of the next epoch, each once.
    let host = start(&root);
    let captures = host
        .ledger_captures_for_test()
        .expect("the host's captures");
    let count = |event_type: EventType| {
        captures
            .iter()
            .filter(|capture| capture.event_type == event_type)
            .count()
    };
    assert_eq!(count(EventType::RuntimeStarted), 1, "G16: runtime.started");
    assert_eq!(
        count(EventType::RuntimeInstanceBound),
        1,
        "G16: one runtime.instance_bound per registered instance"
    );
    host.close().expect("close the restarted host");
}
