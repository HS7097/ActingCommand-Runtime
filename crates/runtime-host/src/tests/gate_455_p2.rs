// SPDX-License-Identifier: AGPL-3.0-only

// #455 P2 (0.12.79; register §1.505; the coordinator's rulings 1 to 13 of 11-22 in its work order,
// not the owner's words; contracts/scheduling/README.md, "A pool read from rows of a list"), the
// host's half. Before each evaluation, under the fact write gate the fact pools are projected
// under, the host reads the instance's list fact and computes the reading of a pool read from rows
// of a list (`crate::pool_list_rows`); the evaluator is given it beside the left-out facts:
// - the sum of the declared rows, each value times its factor; a key the whole list does not hold
//   counts 0; a list of more than 256 rows is computed as any other (h1);
// - unknown, never zero: no list, a confidence below the pool's minimum, a value that is no
//   non-negative integer, a sum beyond 2^53 − 1 (h2);
// - a catalog configured with such a pool is accepted; the number is in the evaluation's resources
//   and the list in neither its facts nor its projection; one moment at one ledger position gives
//   one identity; the evaluation takes the reading (h3);
// - without a list the pool's task waits `trigger_unknown` with the list's key and raises no fact
//   refresh demand (h4);
// - a catalog without the kind projects its golden resources, gives no reading, and keeps its
//   identity when a list is published (h5);
// - a resource target's receipt and the resource target view (whose targetable row a goal entry's
//   `current` shows) read the reading (h6);
// - the reading is in each instance's view identity: an intent evaluated before the list is read
//   again is stale (h7).
//
// The keys, columns and rows are made up; times are fixed Unix milliseconds on a manual Runtime
// clock, and nothing sleeps.

use super::*;
use crate::PolicyTrigger;
use crate::pool_list_rows::{MAX_LIST_ROWS_READING, list_rows_reading};
use actingcommand_contract::{FactScalar, ResourceTargetConditionState};
use actingcommand_policy::{
    GivenReadingState, ListRowsUnknown, PolicyEvaluation, PoolListRow, TaskDecision,
};

const LIST_KEY: &str = "inventory.list.example_kind_a";
const LISTED: &str = "pool.listed";
const TASK: &str = "fixture.observe";
/// When the fixture's lists are read: ten seconds before the fixture's time.
const READ_AT: u64 = POLICY_NOW_UNIX_MS - 10_000;
/// How long a list read stays fresh.
const FRESH_MS: u64 = 60_000;

fn own_scope() -> FactScope {
    FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    }
}

/// A row `{id, count}` of a list.
fn row(key: &str, count: FactScalar) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([
        ("id".to_owned(), FactScalar::String(key.to_owned())),
        ("count".to_owned(), count),
    ])
}

/// The list of `rows` under [`LIST_KEY`] of the policy instance, read at `observed_at`, fresh for
/// [`FRESH_MS`], at confidence 900.
fn list(rows: Vec<BTreeMap<String, FactScalar>>, observed_at: u64, snapshot: &str) -> FactRecord {
    let mut record = stored_fact(
        own_scope(),
        LIST_KEY,
        ContractFactValue::RecordList(rows),
        snapshot,
        Vec::new(),
    );
    record.observed_at_unix_ms = observed_at;
    record.expires_at_unix_ms = Some(observed_at + FRESH_MS);
    record
}

/// The pure reading of the pool rows `rows` (key, factor) of `list`, by the columns `id` and
/// `count`, with the pool's minimum confidence 500.
fn reading(rows: &[(&str, u64)], list: Option<&FactRecord>) -> GivenReadingState {
    let rows = rows
        .iter()
        .map(|(key, factor)| PoolListRow {
            key: (*key).to_owned(),
            factor: *factor,
        })
        .collect::<Vec<_>>();
    list_rows_reading("id", "count", &rows, 500, list)
}

/// A reading of `value` from a list read at [`READ_AT`].
fn read(value: u64) -> GivenReadingState {
    GivenReadingState::Read {
        value,
        observed_at_unix_ms: READ_AT,
        expires_at_unix_ms: Some(READ_AT + FRESH_MS),
    }
}

// Test 1, the sum (rulings 1 and 2): one row; four rows of factors 200, 400, 1000 and 2000, in
// another order than the list's; a key the whole list does not hold counts 0; a list of 300 rows
// is read as any other. The reading's observation and expiry are the list's.
#[test]
fn p2_h1_the_reading_is_the_sum_of_the_declared_rows_and_a_key_the_list_does_not_hold_counts_0() {
    let integer = FactScalar::Integer;
    let one = list(
        vec![row("item_a", integer(7))],
        READ_AT,
        "snapshot:p2-h1-one",
    );
    assert_eq!(reading(&[("item_a", 1)], Some(&one)), read(7));
    let four = list(
        vec![
            row("item_a", integer(1)),
            row("item_b", integer(2)),
            row("item_c", integer(3)),
            row("item_d", integer(4)),
        ],
        READ_AT,
        "snapshot:p2-h1-four",
    );
    assert_eq!(
        reading(
            &[
                ("item_d", 2_000),
                ("item_a", 200),
                ("item_c", 1_000),
                ("item_b", 400)
            ],
            Some(&four)
        ),
        read(4 * 2_000 + 200 + 3 * 1_000 + 2 * 400)
    );
    assert_eq!(
        reading(&[("item_a", 200), ("item_z", 400)], Some(&four)),
        read(200),
        "a key the list does not hold counts 0"
    );
    let long = list(
        (0..300_i64)
            .map(|index| row(&format!("member.{index:04}"), integer(index)))
            .collect(),
        READ_AT,
        "snapshot:p2-h1-long",
    );
    assert_eq!(
        reading(&[("member.0299", 5), ("member.0000", 7)], Some(&long)),
        read(299 * 5)
    );
}

// Test 2, unknown, never zero (ruling 3): no list is `fact_missing`; a confidence below the
// pool's minimum `fact_low_confidence`; a named row's value that is a text, a negative integer, a
// timestamp or absent is `type_mismatch`; a sum of 2^53 − 1 is read and one beyond it is
// `type_mismatch`, by one row or by two.
#[test]
fn p2_h2_a_reading_is_unknown_never_zero() {
    let unknown = GivenReadingState::Unknown;
    let item = [("item_a", 1)];
    assert_eq!(reading(&item, None), unknown(ListRowsUnknown::FactMissing));
    let mut faint = list(
        vec![row("item_a", FactScalar::Integer(7))],
        READ_AT,
        "snapshot:p2-h2-faint",
    );
    faint.confidence_milli = 499;
    assert_eq!(
        reading(&item, Some(&faint)),
        unknown(ListRowsUnknown::FactLowConfidence)
    );
    for (case, count) in [
        ("a text", FactScalar::String("7".to_owned())),
        ("a negative integer", FactScalar::Integer(-1)),
        ("a timestamp", FactScalar::TimestampMs(7)),
    ] {
        let listed = list(vec![row("item_a", count)], READ_AT, "snapshot:p2-h2-value");
        assert_eq!(
            reading(&item, Some(&listed)),
            unknown(ListRowsUnknown::TypeMismatch),
            "{case}"
        );
    }
    let without_count = list(
        vec![BTreeMap::from([(
            "id".to_owned(),
            FactScalar::String("item_a".to_owned()),
        )])],
        READ_AT,
        "snapshot:p2-h2-without",
    );
    assert_eq!(
        reading(&item, Some(&without_count)),
        unknown(ListRowsUnknown::TypeMismatch)
    );
    let bound = i64::try_from(MAX_LIST_ROWS_READING).expect("the bound is an integer");
    let at_bound = list(
        vec![
            row("item_a", FactScalar::Integer(bound)),
            row("item_b", FactScalar::Integer(1)),
        ],
        READ_AT,
        "snapshot:p2-h2-bound",
    );
    assert_eq!(reading(&item, Some(&at_bound)), read(MAX_LIST_ROWS_READING));
    for rows in [&[("item_a", 2)][..], &[("item_a", 1), ("item_b", 1)][..]] {
        assert_eq!(
            reading(rows, Some(&at_bound)),
            unknown(ListRowsUnknown::TypeMismatch),
            "{rows:?}"
        );
    }
}

// ---- The host fixture: catalog-a declared V2, its task due while the pool read from rows of a
// list holds at least 1000, configured directly (no composer in front of it)

/// [`LISTED`], the policy instance's, read from rows `item_a` × 200 and `item_b` × 400 of
/// [`LIST_KEY`], valued.
fn list_pool() -> serde_json::Value {
    serde_json::json!({
        "id": LISTED,
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "capacity": 10_000,
        "projection": {"kind": "none"},
        "observation": {
            "kind": "list_rows",
            "fact_key": LIST_KEY,
            "key_column": "id",
            "value_column": "count",
            "rows": [{"key": "item_a", "factor": 200}, {"key": "item_b", "factor": 400}]
        },
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {
            "name": "Fixture",
            "unit": "unit",
            "scale": 1,
            "base_weight_milli": 1000,
            "gap": {"rule": "shortfall_linear", "weight_milli": 100}
        }
    })
}

/// The fixture catalog declared V2 (its events valid from 0), its pools [`list_pool`] alone, its
/// task due while [`LISTED`] holds at least 1000 and producing 1 of it.
fn list_sources() -> CatalogSources {
    let mut sources = policy_sources(1);
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: serde_json::Value =
            serde_json::from_slice(&source.bytes).expect("the fixture's JSON");
        document["schema_version"] =
            serde_json::json!(actingcommand_policy::SCHEDULING_SCHEMA_VERSION_V2);
        if let Some(events) = document
            .get_mut("events")
            .and_then(serde_json::Value::as_array_mut)
        {
            for event in events {
                event["validity"] = serde_json::json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
        if document.get("tasks").is_some() {
            document["tasks"][0]["trigger"] = serde_json::json!({
                "kind": "resource_projection",
                "pool_id": LISTED,
                "comparison": "greater_than_or_equal",
                "value": 1_000
            });
            document["tasks"][0]["produces"] = serde_json::json!([{
                "pool_id": LISTED,
                "direction": "produce",
                "amount": 1,
                "observation_source": "scan_verified",
                "confidence_milli": 1000
            }]);
        }
        if document.get("pools").is_some() {
            document["pools"] = serde_json::json!([list_pool()]);
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("the fixture's bytes");
    }
    sources
}

/// A host of the policy instance on the fixture's manual clock, with `sources` active.
fn start(root: &TempDir, sources: &CatalogSources) -> RuntimeHost {
    let host = RuntimeHost::start(
        config(root).with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0))),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the runtime host");
    host.activate_policy_catalog(sources)
        .expect("the catalog, configured directly");
    host
}

/// One evaluated cycle at the fixture's time over the fixture's facts, no pool snapshot
/// configured.
fn evaluate(host: &RuntimeHost) -> PolicyEvaluation {
    let mut resources = policy_resources();
    resources.pools.clear();
    host.evaluate_policy_cycle_with_test_inputs(
        &policy_facts(),
        &resources,
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

fn decision(evaluation: &PolicyEvaluation) -> &TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == TASK
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .unwrap_or_else(|| panic!("{TASK}'s decision: {evaluation:#?}"))
}

fn intent(evaluation: &PolicyEvaluation) -> Option<&DispatchIntent> {
    evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == TASK && intent.instance_id == POLICY_INSTANCE_ALIAS)
}

/// The fixture's list: `item_a` 3 and `item_b` 2, so the pool reads 3 × 200 + 2 × 400 = 1400.
fn fixture_list() -> FactRecord {
    list(
        vec![
            row("item_a", FactScalar::Integer(3)),
            row("item_b", FactScalar::Integer(2)),
        ],
        READ_AT,
        "snapshot:p2-list",
    )
}

// Test 3, the host's reading (rulings 1, 5, 6 and 10; test 5's configured catalog). A catalog
// configured with a pool read from rows of a list is accepted and evaluated (no
// `policy_pool_binding_invalid`). Once the list is published, the projection holds no record under
// `inventory.list.`, its resources hold the pool's snapshot of 1400 observed when the list was,
// and its readings the reading with the list's expiry; two projections at one ledger position give
// one identity and the same resources; the evaluation dispatches the task the reading makes due.
#[test]
fn p2_h3_the_host_computes_the_reading_and_the_evaluation_takes_it() {
    let root = TempDir::new().expect("tempdir");
    let host = start(&root, &list_sources());
    evaluate(&host);
    assert!(intent(&evaluate(&host)).is_none(), "no list, no reading");

    host.publish_list_reading_for_test(fixture_list())
        .expect("the list");
    let (facts, resources, readings) = host
        .projected_policy_inputs_for_test()
        .expect("the projected inputs");
    assert!(
        facts
            .facts
            .iter()
            .all(|fact| !actingcommand_contract::is_list_reading_key(&fact.fact_key)),
        "the list is not in the evaluation's facts: {:?}",
        facts.facts
    );
    assert_eq!(
        resources.pools,
        [PoolValueSnapshot {
            pool_id: LISTED.to_owned(),
            value: 1_400,
            observed_at_unix_ms: READ_AT,
        }]
    );
    let given = readings.get(LISTED).expect("the pool's reading");
    assert_eq!(given.fact_key, LIST_KEY);
    assert_eq!(given.instance_id, POLICY_INSTANCE_ALIAS);
    assert_eq!(given.state, read(1_400));
    let (again, again_resources, _) = host
        .projected_policy_inputs_for_test()
        .expect("the projected inputs again");
    assert_eq!(again.ledger_position, facts.ledger_position);
    assert_eq!(again.fact_snapshot_id, facts.fact_snapshot_id);
    assert_eq!(again_resources, resources);

    let after = evaluate(&host);
    assert!(intent(&after).is_some(), "{after:#?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// Test 4, no refresh need (ruling 8). Without a list the pool's task waits `trigger_unknown` with
// the suggestion of the list's key and `fact_missing`, and raises no fact refresh demand: the key
// is left out for the instance with the reading.
#[test]
fn p2_h4_without_a_list_the_task_waits_with_the_lists_key_and_asks_no_refresh() {
    let root = TempDir::new().expect("tempdir");
    let host = start(&root, &list_sources());
    evaluate(&host);
    let evaluation = evaluate(&host);
    assert!(intent(&evaluation).is_none(), "{evaluation:#?}");
    let waiting = decision(&evaluation);
    let codes = waiting
        .reasons
        .iter()
        .map(|reason| reason.outcome.code().as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"trigger_unknown"), "{codes:?}");
    assert!(
        codes.iter().all(|code| !code.starts_with("fact_refresh")),
        "no fact refresh demand: {codes:?}"
    );
    assert!(
        waiting.detection_suggestions.iter().any(
            |suggestion| suggestion.fact_key == LIST_KEY && suggestion.reason == "fact_missing"
        ),
        "{:?}",
        waiting.detection_suggestions
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// catalog-a's projected resources as main serializes them: its static pool's configured snapshot
/// and the configured host, the threshold left out.
const EXAMPLE_RESOURCES_GOLDEN: &str = concat!(
    r#"{"pools":[{"pool_id":"fixture-pool-a","value":10,"observed_at_unix_ms":1699963200000}],"#,
    r#""hosts":[{"host_id":"fixture-host-a","cpu_available_milli":1000,"#,
    r#""gpu_available_milli":1000,"io_available_milli":1000,"#,
    r#""host_responsiveness_basis_points":10000,"third_party_pressure_basis_points":0,"#,
    r#""heavy_dispatch_limit":1,"active_heavy_dispatches":0}]}"#
);

// Test 5, the golden (ruling 5). With catalog-a, which reads no list, the projection's resources
// serialize to their golden bytes and it gives no reading; a list published then changes neither
// its resources nor its identity.
#[test]
fn p2_h5_a_catalog_without_the_kind_keeps_its_golden_bytes_and_identity() {
    let root = TempDir::new().expect("tempdir");
    let host = start(&root, &policy_sources(1));
    let (before, resources, readings) = host
        .projected_policy_inputs_for_test()
        .expect("the projected inputs");
    assert_eq!(
        serde_json::to_string(&resources).expect("the resources serialize"),
        EXAMPLE_RESOURCES_GOLDEN
    );
    assert!(readings.is_empty());
    host.publish_list_reading_for_test(fixture_list())
        .expect("the list");
    let (after, after_resources, after_readings) = host
        .projected_policy_inputs_for_test()
        .expect("the projected inputs after the list");
    assert_eq!(after.fact_snapshot_id, before.fact_snapshot_id);
    assert_eq!(after_resources, resources);
    assert!(after_readings.is_empty());
    host.close().expect("close the host");
}

// Test 6, a resource target's receipt and the resource target view (ruling 7). A v2 target of at
// least 2000 on the pool is applied, its receipt's condition computed from the reading (1400,
// observed when the list was, the gap 600). The view lists the pool as targetable under its
// list's key with the reading (the row a goal entry's `current` shows), and the stored target's
// condition is the receipt's.
#[test]
fn p2_h6_a_targets_receipt_and_the_target_view_read_the_reading() {
    let root = TempDir::new().expect("tempdir");
    let host = start(&root, &list_sources());
    evaluate(&host);
    host.publish_list_reading_for_test(fixture_list())
        .expect("the list");
    let computed = ResourceTargetConditionState::Computed {
        current: 1_400,
        observed_at_unix_ms: READ_AT,
        gap: 600,
    };
    let document = serde_json::json!({
        "schema_version": "actingcommand.resource-targets.v2",
        "instance": POLICY_INSTANCE_ALIAS,
        "valid_until_unix_ms": POLICY_NOW_UNIX_MS + 86_400_000,
        "targets": [{
            "id": "goal-l",
            "resource": LISTED,
            "condition": {"kind": "at_least", "amount": 2_000},
            "apply": {"mode": "adjust", "weight": "score_stage"}
        }]
    });
    let mut client = TestClient::connect(&host);
    let request = client.request(RuntimeOperation::ApplyResourceTargets {
        document_json: document.to_string(),
        expected_version: None,
    });
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::ResourceTargetsApplied { applied }) = receipt.result() else {
        panic!("the applied targets: {receipt:?}");
    };
    let [condition] = applied.conditions.as_slice() else {
        panic!("one condition: {:?}", applied.conditions);
    };
    assert_eq!(condition.fact_key, LIST_KEY);
    assert_eq!(condition.state, computed);

    let request = client.request(RuntimeOperation::ResourceTargetView {
        instance_alias: POLICY_INSTANCE_ALIAS.to_owned(),
    });
    let receipt = client.send(&request);
    let Some(RuntimeResult::ResourceTargetView { view }) = receipt.result() else {
        panic!("the resource target view: {receipt:?}");
    };
    let listed = view
        .targetable
        .iter()
        .find(|pool| pool.resource == LISTED)
        .unwrap_or_else(|| panic!("the pool is targetable: {view:?}"));
    assert_eq!(listed.fact_key, LIST_KEY);
    assert_eq!(listed.observation.current, Some(1_400));
    assert_eq!(listed.observation.observed_at_unix_ms, Some(READ_AT));
    let active = view.active.as_ref().expect("the stored target");
    assert_eq!(active.conditions[0].state, computed);
    drop(client);
    host.close().expect("close the host");
}

// Test 7, the view identity (ruling 5). An intent evaluated on the reading of 1400 is stale once
// the list is read again (`item_a` 4, so 1600): the number is in its instance's view, so the
// admission refuses it `policy_facts_stale`, and the projection's identity moved.
#[test]
fn p2_h7_a_new_reading_moves_the_instances_view_and_the_intent_is_stale() {
    let root = TempDir::new().expect("tempdir");
    let host = start(&root, &list_sources());
    evaluate(&host);
    host.publish_list_reading_for_test(fixture_list())
        .expect("the list");
    let evaluation = evaluate(&host);
    let intent = intent(&evaluation)
        .unwrap_or_else(|| panic!("the task is due: {evaluation:#?}"))
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    record_policy_approval(&host, &intent);
    host.publish_list_reading_for_test(list(
        vec![
            row("item_a", FactScalar::Integer(4)),
            row("item_b", FactScalar::Integer(2)),
        ],
        READ_AT + 5_000,
        "snapshot:p2-list-again",
    ))
    .expect("the list read again");
    let (again, resources, _) = host
        .projected_policy_inputs_for_test()
        .expect("the projected inputs");
    assert_ne!(again.fact_snapshot_id, intent.fact_snapshot_id);
    assert_eq!(resources.pools[0].value, 1_600);
    assert_eq!(
        host.admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
            .unwrap_err()
            .code(),
        "policy_facts_stale"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}
