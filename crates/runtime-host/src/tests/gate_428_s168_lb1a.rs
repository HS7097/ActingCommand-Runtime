// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 S168-LB1a (since 0.12.55; register §1.309 rows LB1-1, LB1-3, LB1-5 and LB1-6, the
// slice's section §1.311; `contracts/instance-fact-store.md`, "List reading facts", "Left out of
// the projection", "The frame bound with long lists"; `contracts/scheduling/README.md`, "Record
// lists and the projection"), the host's halves:
//
// - a list of 1024 rows is published and read back, from the store, through the instance fact
//   read and from the ledger after a restart; a ledger that holds only what revision 7 wrote (a
//   list of 256 rows) opens;
// - the scheduler's projection holds no record under `inventory.list.`, whatever its rows: the
//   evaluation's facts and its `fact_snapshot_id` are those without the lists, every decision is
//   the same, and a list published between an evaluation and its admission leaves the intent
//   current;
// - another record list of 257 rows is left out for its instance only: the predicate that reads it
//   is Unknown (`fact_missing`) with no fact refresh demand, the evaluation of every instance goes
//   on, one `policy_fact_rows_exceeded` warning is recorded after the start and a second
//   evaluation records none, and the other instance decides as before;
// - the instance fact read by key prefix keeps the prefix's records before the view is measured:
//   an instance too large for one frame whole is answered for a prefix that fits, with
//   `records_total`, and a read without a prefix answers without it.
//
// Every expected value is the contract's. The keys and rows are made up; times are fixed Unix
// milliseconds on a manual Runtime clock, and nothing sleeps.

use super::*;
use crate::{PolicyDispatchAdmission, PolicyTrigger};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{FactScalar, RuntimePayload};
use actingcommand_policy::{EligibilityState, PolicyEvaluation, TaskDecision};

const LIST_KEY: &str = "inventory.list.example_kind_a";
const LONG_LIST_KEY: &str = "inventory.list.example_kind_b";
const LATE_LIST_KEY: &str = "inventory.list.example_kind_c";
const ROWS_KEY: &str = "inventory.example_rows";
const ROWS_EXCEEDED: &str = "policy_fact_rows_exceeded";
const A_TASK: &str = "fixture.observe";
const B_TASK: &str = "fixture.observe-b";
const SERVER: &str = "fixture-server-a";
const GAME: &str = "fixture-game-a";

fn own_scope() -> FactScope {
    FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    }
}

// `count` rows `{id, open}`, the ids in ascending byte order.
fn list_rows(count: usize) -> Vec<BTreeMap<String, FactScalar>> {
    (0..count)
        .map(|index| {
            BTreeMap::from([
                (
                    "id".to_owned(),
                    FactScalar::String(format!("member.{index:04}")),
                ),
                ("open".to_owned(), FactScalar::Boolean(index % 2 == 0)),
            ])
        })
        .collect()
}

// A record list of `count` rows under `key`, of the policy instance, observed at the fixture's time.
fn list_record(key: &str, count: usize) -> FactRecord {
    stored_fact(
        own_scope(),
        key,
        ContractFactValue::RecordList(list_rows(count)),
        &format!("snapshot:lb1a-{key}-{count}"),
        Vec::new(),
    )
}

fn rows_of(record: &FactRecord) -> usize {
    match &record.content {
        FactContent::Inline {
            value: ContractFactValue::RecordList(rows),
        } => rows.len(),
        other => panic!("a record list, got {other:?}"),
    }
}

fn context() -> InstanceFactContext {
    InstanceFactContext {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        server_id: SERVER.to_owned(),
        game_id: GAME.to_owned(),
    }
}

// The policy instance's active record of `key`, as the host's projection holds it.
fn stored(host: &RuntimeHost, key: &str) -> Option<FactRecord> {
    host.instance_fact_snapshot(context())
        .expect("the instance's facts")
        .records
        .into_iter()
        .find(|record| record.key == key)
}

// A host of the policy instance on the fixture's manual clock, with `id` its registered instance.
fn start_one(root: &TempDir, id: InstanceId) -> RuntimeHost {
    RuntimeHost::start(
        config(root).with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0))),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the runtime host")
}

// The instance fact read of the policy instance, with `key_prefix`, as a client sends it.
fn fact_read(client: &mut TestClient, key_prefix: Option<&str>) -> RuntimeReceipt {
    let request = client.request(RuntimeOperation::InstanceFactSnapshot {
        instance: POLICY_INSTANCE_ALIAS.to_owned(),
        key_prefix: key_prefix.map(str::to_owned),
    });
    client.send(&request)
}

// The view and `records_total` of a completed instance fact read.
fn answered(receipt: &RuntimeReceipt) -> (Vec<FactRecord>, Option<u32>) {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{:?}",
        receipt.outcome()
    );
    let Some(RuntimeResult::InstanceFactSnapshot {
        snapshot,
        records_total,
        ..
    }) = receipt.result()
    else {
        panic!("an instance fact snapshot, got {:?}", receipt.result());
    };
    (snapshot.records.clone(), *records_total)
}

// LB1-1: a ledger that holds only what revision 7 writes, a list of 256 rows, opens with this build
// and the list reads back. A list of 1024 rows, which a reader of revision 7 refuses, is published
// and read back from the store and through the instance fact read, and after a restart the start's
// load of the ledger reads it back too.
#[test]
fn lb1a_a_list_of_1024_rows_is_published_and_read_back_after_a_restart() {
    let root = TempDir::new().expect("tempdir");
    let id = instance_id();
    // The fixture's records are observed before the host clock, so each start (as every restart
    // test's) runs on the host's own clock.
    let start = |root: &TempDir| {
        RuntimeHost::start(
            config(root),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                id,
                Arc::new(FakeState::default()),
            )),
        )
        .expect("the runtime host")
    };
    let host = start(&root);
    host.publish_list_reading_for_test(list_record(LIST_KEY, 256))
        .expect("a list of 256 rows");
    host.close().expect("close the host");

    let host = start(&root);
    assert_eq!(
        rows_of(&stored(&host, LIST_KEY).expect("the list of 256 rows after the restart")),
        256
    );
    host.publish_list_reading_for_test(list_record(LONG_LIST_KEY, 1_024))
        .expect("a list of 1024 rows");
    assert_eq!(
        rows_of(&stored(&host, LONG_LIST_KEY).expect("the list of 1024 rows")),
        1_024
    );
    let mut client = TestClient::connect(&host);
    let (records, total) = answered(&fact_read(&mut client, None));
    assert_eq!(total, None, "a read without a prefix answers no total");
    let long = records
        .iter()
        .find(|record| record.key == LONG_LIST_KEY)
        .expect("the list in the read");
    assert_eq!(rows_of(long), 1_024);
    drop(client);
    host.close().expect("close the host");

    let host = start(&root);
    let reloaded = stored(&host, LONG_LIST_KEY).expect("the list of 1024 rows after the restart");
    assert_eq!(rows_of(&reloaded), 1_024);
    assert_eq!(reloaded.content, list_record(LONG_LIST_KEY, 1_024).content);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// LB1-5b, the bound itself: a list read whole is measured, on the record a run's end would publish,
// against the observation's 512 KiB. 1024 rows of the key and 8 columns of 64 bytes are over it, and
// the observation's own write check refuses that record too; 1024 rows of the key and 3 short
// columns are within it. (A run's miss is `gate_428_s168_4::list_reading_over_observation_bytes_is_a_miss`.)
#[test]
fn lb1a_a_list_is_measured_against_the_observation_bytes() {
    let list = |columns: usize, width: usize| {
        let rows = (0..1_024)
            .map(|index| {
                let mut row = BTreeMap::from([(
                    "id".to_owned(),
                    FactScalar::String(format!("member.{index:04}")),
                )]);
                for column in 0..columns {
                    row.insert(format!("c{column}"), FactScalar::String("v".repeat(width)));
                }
                row
            })
            .collect::<Vec<_>>();
        stored_fact(
            own_scope(),
            "inventory.list.example_kind_d",
            ContractFactValue::RecordList(rows),
            "snapshot:lb1a-measured",
            Vec::new(),
        )
    };
    let wide = list(8, 64);
    assert!(crate::host::list_record_over_observation_bytes(&wide));
    assert!(
        actingcommand_contract::FactObservation {
            records: vec![wide],
        }
        .validate()
        .is_err()
    );
    let narrow = list(3, 1);
    assert!(!crate::host::list_record_over_observation_bytes(&narrow));
    actingcommand_contract::FactObservation {
        records: vec![narrow],
    }
    .validate()
    .expect("1024 narrow rows fit one observation");
}

// One evaluated cycle at the fixture's time over `facts` and `resources`.
fn evaluate_with(
    host: &RuntimeHost,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
) -> PolicyEvaluation {
    host.evaluate_policy_cycle_with_test_inputs(
        facts,
        resources,
        EvaluationTime {
            unix_ms: POLICY_NOW_UNIX_MS,
            monotonic_ms: POLICY_NOW_UNIX_MS,
        },
        7,
        PolicyTrigger::Recovery,
    )
    .expect("a policy cycle")
    .evaluation
    .expect("an evaluated cycle")
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str, alias: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(alias)
        })
        .unwrap_or_else(|| panic!("{task_id} on {alias}: {evaluation:#?}"))
}

fn codes(decision: &TaskDecision) -> Vec<&str> {
    decision
        .reasons
        .iter()
        .map(|reason| reason.outcome.code().as_str())
        .collect()
}

// What a decision decides: its state, its eligibility and its reasons' codes.
fn decided(decision: &TaskDecision) -> (String, EligibilityState, Vec<&str>) {
    (
        format!("{:?}", decision.state),
        decision.eligibility,
        codes(decision),
    )
}

fn intent_of<'a>(evaluation: &'a PolicyEvaluation, alias: &str) -> &'a DispatchIntent {
    evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.instance_id == alias)
        .unwrap_or_else(|| panic!("{alias}'s dispatch intent: {evaluation:#?}"))
}

// LB1-3: the projection leaves out every record under `inventory.list.`, of 3 rows and of 1024. The
// evaluation's facts hold none, its `fact_snapshot_id` is the one before the lists were published,
// and every decision is the same (an evaluation before the lists and one after, both after the
// start's first, on one host). A list published between an evaluation and its admission leaves
// the intent current: it is admitted, not refused `policy_facts_stale`.
#[test]
fn lb1a_the_projection_holds_no_list_and_every_decision_is_as_without_it() {
    let root = TempDir::new().expect("tempdir");
    let host = start_one(&root, instance_id());
    host.activate_policy_catalog(&policy_sources(1))
        .expect("the fixture catalog");
    let (facts, resources) = (policy_facts(), policy_resources());
    evaluate_with(&host, &facts, &resources);
    let before = evaluate_with(&host, &facts, &resources);

    host.publish_list_reading_for_test(list_record(LIST_KEY, 3))
        .expect("a list of 3 rows");
    host.publish_list_reading_for_test(list_record(LONG_LIST_KEY, 1_024))
        .expect("a list of 1024 rows");
    let projected = host
        .projected_policy_facts_for_test()
        .expect("the projected facts");
    assert!(
        projected
            .facts
            .iter()
            .all(|fact| !actingcommand_contract::is_list_reading_key(&fact.fact_key)),
        "the projection holds a list: {:?}",
        projected.facts
    );

    let after = evaluate_with(&host, &facts, &resources);
    let before_intent = intent_of(&before, POLICY_INSTANCE_ALIAS);
    let after_intent = intent_of(&after, POLICY_INSTANCE_ALIAS);
    assert_eq!(after_intent.task_id, before_intent.task_id);
    assert_eq!(
        after_intent.fact_snapshot_id, before_intent.fact_snapshot_id,
        "the lists are not in the snapshot identity"
    );
    assert_eq!(
        after.decisions.iter().map(decided).collect::<Vec<_>>(),
        before.decisions.iter().map(decided).collect::<Vec<_>>(),
        "every decision is as without the lists"
    );

    let intent = after_intent.clone();
    let reasons = after
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    host.publish_list_reading_for_test(list_record(LATE_LIST_KEY, 2))
        .expect("a list between the evaluation and the admission");
    record_policy_approval(&host, &intent);
    assert!(
        matches!(
            host.admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
                .expect("the admission"),
            PolicyDispatchAdmission::Granted { .. }
        ),
        "a list's publication does not make the intent stale"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// The catalog of A's task and B's task, A's trigger a record deadline over `inventory.example_rows`
// of A's own scope (a realistic key, which the fact refresh would be asked for).
fn deadline_sources() -> CatalogSources {
    let mut sources = pending_policy_sources(1);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("the fixture tasks");
    tasks["tasks"][0]["trigger"] = serde_json::json!({
        "kind": "record_deadline",
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "fact_key": ROWS_KEY,
        "timestamp_field": "due_at_unix_ms",
        "within_ms": 172_800_000_u64,
        "max_age_ms": 600_000
    });
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("the fixture task bytes");
    sources
}

// A and B on the fixture's manual clock, each its own registered instance.
fn two_instance_host(root: &TempDir) -> RuntimeHost {
    let host = RuntimeHost::start(
        config(root).with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0))),
        Arc::new(FakeProvider::from_entries([
            (
                POLICY_INSTANCE_ALIAS.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            ),
            (
                POLICY_INSTANCE_ALIAS_B.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            ),
        ])),
    )
    .expect("the two-instance host");
    host.activate_policy_catalog(&deadline_sources())
        .expect("the deadline catalog");
    host
}

// The outcomes of the `runtime.failed` records of `code`, in ledger order.
fn failed(host: &RuntimeHost, code: &str) -> Vec<Outcome> {
    let mut events = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::RuntimeFailed),
            ..EventQuery::default()
        })
        .expect("the runtime.failed records");
    events.sort_by_key(PersistedEvent::sequence);
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.outcome().code().as_str() == code =>
            {
                Some(record.outcome().clone())
            }
            _ => None,
        })
        .collect()
}

// LB1-5: before the record exists, A's record deadline reads `inventory.example_rows` as missing
// and asks the fact refresh for it (`fact_refresh_unbound`: A binds no reader). A record list of 257
// rows of A, more than the evaluation reads, is held by the store and left out of the projection
// for A: the evaluation of both instances goes on (it is not skipped for the long list), A's
// predicate is Unknown with the suggestion `fact_missing` and no `fact_refresh_*` reason, one
// Warning `policy_fact_rows_exceeded` names the key and the 257 rows, and a second evaluation
// records no other. B decides as before the record.
#[test]
fn lb1a_a_long_record_list_is_left_out_for_its_instance_with_one_warning() {
    let root = TempDir::new().expect("tempdir");
    let host = two_instance_host(&root);
    let (facts, resources) = (pending_policy_facts(), pending_policy_resources());
    evaluate_with(&host, &facts, &resources);
    let before = evaluate_with(&host, &facts, &resources);
    let asked = decision(&before, A_TASK, POLICY_INSTANCE_ALIAS);
    assert_eq!(asked.eligibility, EligibilityState::Unknown);
    assert!(
        codes(asked).contains(&"fact_refresh_unbound"),
        "a missing key is asked for: {:?}",
        codes(asked)
    );

    let rows = (0..257_u64)
        .map(|index| {
            BTreeMap::from([(
                "due_at_unix_ms".to_owned(),
                FactScalar::TimestampMs(POLICY_NOW_UNIX_MS + 1_000 * (index + 1)),
            )])
        })
        .collect::<Vec<_>>();
    host.publish_fact(stored_fact(
        own_scope(),
        ROWS_KEY,
        ContractFactValue::RecordList(rows),
        "snapshot:lb1a-rows",
        Vec::new(),
    ))
    .expect("a record list of 257 rows");
    assert_eq!(
        rows_of(&stored(&host, ROWS_KEY).expect("the store holds it")),
        257
    );

    let after = evaluate_with(&host, &facts, &resources);
    let left_out = decision(&after, A_TASK, POLICY_INSTANCE_ALIAS);
    assert_eq!(left_out.eligibility, EligibilityState::Unknown);
    assert!(
        codes(left_out).contains(&"trigger_unknown"),
        "{:?}",
        codes(left_out)
    );
    assert!(
        codes(left_out)
            .iter()
            .all(|code| !code.starts_with("fact_refresh")),
        "a left-out key raises no fact refresh demand: {:?}",
        codes(left_out)
    );
    assert!(
        left_out.detection_suggestions.iter().any(
            |suggestion| suggestion.fact_key == ROWS_KEY && suggestion.reason == "fact_missing"
        ),
        "{:?}",
        left_out.detection_suggestions
    );
    assert!(
        !after
            .dispatch_intents
            .iter()
            .any(|intent| intent.instance_id == POLICY_INSTANCE_ALIAS),
        "nothing is dispatched on A for its unknown trigger"
    );
    assert_eq!(
        decided(decision(&after, B_TASK, POLICY_INSTANCE_ALIAS_B)),
        decided(decision(&before, B_TASK, POLICY_INSTANCE_ALIAS_B)),
        "B decides as before"
    );
    assert_eq!(
        intent_of(&after, POLICY_INSTANCE_ALIAS_B).task_id,
        intent_of(&before, POLICY_INSTANCE_ALIAS_B).task_id
    );
    assert!(
        super::gate_419_iso_pe::skipped_failure_codes(&host).is_empty(),
        "no evaluation was skipped"
    );

    let warnings = failed(&host, ROWS_EXCEEDED);
    let [warning] = warnings.as_slice() else {
        panic!("one warning after the start, got {warnings:#?}");
    };
    assert_eq!(
        warning.category(),
        actingcommand_contract::outcome::Category::Warning
    );
    assert_eq!(
        warning.value("fact_key"),
        Some(&serde_json::json!(ROWS_KEY))
    );
    assert_eq!(warning.value("count"), Some(&serde_json::json!(257)));

    let again = evaluate_with(&host, &facts, &resources);
    assert_eq!(
        decided(decision(&again, A_TASK, POLICY_INSTANCE_ALIAS)),
        decided(left_out)
    );
    assert_eq!(
        failed(&host, ROWS_EXCEEDED).len(),
        1,
        "a second evaluation records no other warning"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// LB1-6: an instance whose whole view is larger than one frame is refused as before
// (`instance_fact_snapshot_too_large_for_frame`), and answered for a key prefix that fits: the
// prefix's records only, with `records_total` the instance's records before the filter, as the
// host's own projection of the instance counts them; a prefix that matches nothing answers no
// record and the same total. A read without a prefix answers no total.
#[test]
fn lb1a_a_key_prefix_is_kept_before_the_frame_is_measured() {
    let root = TempDir::new().expect("tempdir");
    // A frame of 8 KiB: it holds the answer of the instance's few records, not of 12 more of 900
    // bytes each.
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0)))
            .with_maximum_frame_bytes(8_192),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the runtime host");
    host.publish_fact(stored_fact(
        own_scope(),
        "inventory.lb1a_kept",
        ContractFactValue::Integer(7),
        "snapshot:lb1a-kept",
        Vec::new(),
    ))
    .expect("the kept record");
    let mut client = TestClient::connect(&host);
    let (_, total) = answered(&fact_read(&mut client, None));
    assert_eq!(total, None, "a read without a prefix answers no total");
    drop(client);

    for index in 0..12 {
        host.publish_fact(stored_fact(
            own_scope(),
            &format!("inventory.lb1a_pad_{index}"),
            ContractFactValue::String("x".repeat(900)),
            &format!("snapshot:lb1a-pad-{index}"),
            Vec::new(),
        ))
        .expect("a padding record");
    }
    let expected_total = u32::try_from(
        host.instance_fact_snapshot(context())
            .expect("the instance's facts")
            .records
            .len(),
    )
    .expect("a record count");
    let mut client = TestClient::connect(&host);

    let whole = fact_read(&mut client, None);
    assert_eq!(whole.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        whole.outcome().code().as_str(),
        "instance_fact_snapshot_too_large_for_frame"
    );

    let (records, total) = answered(&fact_read(&mut client, Some("inventory.lb1a_kept")));
    assert_eq!(
        records
            .iter()
            .map(|record| record.key.as_str())
            .collect::<Vec<_>>(),
        ["inventory.lb1a_kept"]
    );
    assert_eq!(total, Some(expected_total));

    let (records, total) = answered(&fact_read(&mut client, Some("inventory.lb1a_none")));
    assert!(records.is_empty());
    assert_eq!(total, Some(expected_total));

    drop(client);
    host.close().expect("close the host");
}
