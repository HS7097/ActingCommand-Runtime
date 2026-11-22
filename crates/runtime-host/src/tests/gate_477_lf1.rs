// SPDX-License-Identifier: AGPL-3.0-only

// #477 LF-1 (0.12.80; register §1.509; the coordinator's ruling after the two reviews of #1872,
// not the owner's words; contracts/scheduling/README.md, "Pool lines", "At each evaluation"), the
// host's half, through the real settlement path. Before each evaluation the host gives the
// evaluator the pairs whose latest admitted dispatch carried a `run_cap` on its recorded
// `policy_dispatch_parameters`, with the instant that run ended (`PolicyHost::handed_runs`); a
// pool such a run's task consumes is unknown, `fact_stale`, until a reading observed at or after
// that end:
// - a run handed 3 turns ends; the next evaluation, the pool not read again, holds the task: its
//   trigger suggests the pool's key with `fact_stale`, and nothing is dispatched;
// - a restart of the host on the same state root gives the same answer;
// - a reading observed after the run's end releases the pool.
//
// The pool is read from rows of a list (#455 P2's fixture: `item_a` × 200 and `item_b` × 400 of
// `inventory.list.example_kind_a`, read as 1400); the task is catalog-a's, due while the pool holds
// 1000, consuming 400 a run and reserving 200, so SS-6 hands it ⌊(1400 − 200) ÷ 400⌋ = 3 turns.
// Times are fixed Unix milliseconds on a manual Runtime clock, and nothing sleeps.

use super::*;
use crate::PolicyTrigger;
use actingcommand_contract::FactScalar;
use actingcommand_policy::{PolicyCode, PolicyEvaluation, TaskDecision};
use serde_json::json;

const LIST_KEY: &str = "inventory.list.example_kind_a";
const LISTED: &str = "pool.listed";
const TASK: &str = "fixture.observe";
/// When the fixture's list is first read: ten seconds before the fixture's time.
const READ_AT: u64 = POLICY_NOW_UNIX_MS - 10_000;
/// How long a list read stays fresh.
const FRESH_MS: u64 = 60_000;
/// How long the run takes on both of the Runtime clock's readings: shorter than the dispatch
/// lease's lifetime in the tests' configuration (5 s), so the lease is still the dispatch's when
/// the run ends it, as other host tests take a dispatch from its admission to its end; a longer
/// step lets the lease lapse and the lease sweep end it first (`lease_mismatch` at the release).
const RUN_MS: u64 = 3_000;
/// The Runtime clock when the run is settled, its end.
const SETTLED_AT: u64 = POLICY_NOW_UNIX_MS + RUN_MS;

/// [`LISTED`], the policy instance's, read from rows `item_a` × 200 and `item_b` × 400 of
/// [`LIST_KEY`], valued.
fn list_pool() -> serde_json::Value {
    json!({
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

/// catalog-a declared V2 (its events valid from 0), its pools [`list_pool`] alone, its task due
/// while [`LISTED`] holds at least 1000, consuming 400 of it a run and reserving 200, producing
/// nothing; its profiles without goals.
fn sources() -> CatalogSources {
    let mut sources = policy_sources(1);
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: serde_json::Value =
            serde_json::from_slice(&source.bytes).expect("the fixture's JSON");
        document["schema_version"] = json!(actingcommand_policy::SCHEDULING_SCHEMA_VERSION_V2);
        if let Some(events) = document
            .get_mut("events")
            .and_then(serde_json::Value::as_array_mut)
        {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
        if document.get("tasks").is_some() {
            let task = &mut document["tasks"][0];
            task["trigger"] = json!({
                "kind": "resource_projection",
                "pool_id": LISTED,
                "comparison": "greater_than_or_equal",
                "value": 1_000
            });
            task["consumes"] = json!([{
                "pool_id": LISTED,
                "direction": "consume",
                "amount": 400,
                "observation_source": "scan_verified",
                "confidence_milli": 1000
            }]);
            task["produces"] = json!([]);
            task["reserves"] = json!([{"pool_id": LISTED, "at_least": 200}]);
        }
        if document.get("pools").is_some() {
            document["pools"] = json!([list_pool()]);
        }
        if let Some(profiles) = document
            .get_mut("profiles")
            .and_then(serde_json::Value::as_array_mut)
        {
            for profile in profiles {
                profile["goals"] = json!([]);
            }
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("the fixture's bytes");
    }
    sources
}

/// A row `{id, count}` of the list.
fn row(key: &str, count: i64) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([
        ("id".to_owned(), FactScalar::String(key.to_owned())),
        ("count".to_owned(), FactScalar::Integer(count)),
    ])
}

/// The list `item_a` 3 and `item_b` 2 (so the pool reads 1400) of the policy instance, read at
/// `observed_at`, fresh for [`FRESH_MS`].
fn list_read_at(observed_at: u64, snapshot: &str) -> FactRecord {
    let mut record = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        LIST_KEY,
        ContractFactValue::RecordList(vec![row("item_a", 3), row("item_b", 2)]),
        snapshot,
        Vec::new(),
    );
    record.observed_at_unix_ms = observed_at;
    record.expires_at_unix_ms = Some(observed_at + FRESH_MS);
    record
}

/// A start of the policy instance's host over `root` on `clock`.
fn open(root: &TempDir, instance: InstanceId, clock: &Arc<ManualRuntimeClock>) -> RuntimeHost {
    RuntimeHost::start(
        config(root).with_runtime_clock(Arc::<ManualRuntimeClock>::clone(clock)),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the runtime host")
}

/// One evaluated cycle at `unix_ms`, the Runtime clock moved there and the host's capacity
/// sampled, as its capacity monitor does while time passes, over the fixture's facts, no pool
/// snapshot configured.
fn evaluate_at(host: &RuntimeHost, clock: &ManualRuntimeClock, unix_ms: u64) -> PolicyEvaluation {
    clock.set_unix_ms(unix_ms);
    host.capacity_sampler_for_test().expect("capacity owner")().expect("capacity sample");
    let mut resources = policy_resources();
    resources.pools.clear();
    host.evaluate_policy_cycle_with_test_inputs(
        &policy_facts(),
        &resources,
        EvaluationTime {
            unix_ms,
            monotonic_ms: unix_ms,
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

/// Whether `evaluation`'s decision of the task suggests reading the list's key for `fact_stale`.
fn stale(evaluation: &PolicyEvaluation) -> bool {
    decision(evaluation)
        .detection_suggestions
        .iter()
        .any(|suggestion| suggestion.fact_key == LIST_KEY && suggestion.reason == "fact_stale")
}

// The run handed 3 turns, its settlement, a restart and a new reading, through the host.
#[test]
fn lf1_h1_a_run_handed_a_run_cap_leaves_its_pool_unknown_until_read_after_its_end() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = open(&root, instance, &clock);
    host.activate_policy_catalog(&sources())
        .expect("activate the fixture catalog");
    evaluate_at(&host, &clock, POLICY_NOW_UNIX_MS);
    host.publish_list_reading_for_test(list_read_at(READ_AT, "snapshot:lf1-list"))
        .expect("the list");

    let evaluation = evaluate_at(&host, &clock, POLICY_NOW_UNIX_MS);
    let intent_now = intent(&evaluation)
        .unwrap_or_else(|| panic!("the task is due: {evaluation:#?}"))
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent_now.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    let run_cap = reasons
        .reasons
        .iter()
        .find(|reason| reason.outcome.is(PolicyCode::PolicyDispatchParameters))
        .and_then(|reason| reason.outcome.value("run_cap"))
        .cloned();
    assert_eq!(run_cap, Some(json!(3)), "{reasons:#?}");
    record_policy_approval(&host, &intent_now);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent_now, &reasons, &policy_context(&host, &intent_now))
        .expect("admit the task")
    else {
        panic!("the task is granted");
    };
    clock.advance(RUN_MS);
    host.complete_policy_dispatch(&intent_now.decision_id)
        .expect("complete the dispatch");
    host.release_policy_dispatch_lease_for_test(&context)
        .expect("end the dispatch's lease");

    let held = evaluate_at(&host, &clock, SETTLED_AT + 5_000);
    assert!(intent(&held).is_none(), "{held:#?}");
    assert!(
        stale(&held),
        "the list read before the run is stale: {held:#?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");

    let clock = Arc::new(ManualRuntimeClock::new(SETTLED_AT + 6_000, 0));
    let host = open(&root, instance, &clock);
    for unix_ms in [SETTLED_AT + 7_000, SETTLED_AT + 8_000] {
        let restarted = evaluate_at(&host, &clock, unix_ms);
        assert!(intent(&restarted).is_none(), "{restarted:#?}");
        assert!(
            stale(&restarted),
            "the same after a restart: {restarted:#?}"
        );
    }

    clock.set_unix_ms(SETTLED_AT + 10_000);
    host.publish_list_reading_for_test(list_read_at(SETTLED_AT + 10_000, "snapshot:lf1-again"))
        .expect("the list read again");
    let released = evaluate_at(&host, &clock, SETTLED_AT + 20_000);
    assert!(
        !stale(&released),
        "a reading after the run's end releases the pool: {released:#?}"
    );
    assert!(
        decision(&released)
            .reasons
            .iter()
            .all(|reason| reason.outcome.code().as_str() != "trigger_unknown"),
        "the trigger reads the pool again: {released:#?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}
