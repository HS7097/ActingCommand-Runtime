// SPDX-License-Identifier: AGPL-3.0-only

// #455 RR3e (0.12.104; register §1.560; the coordinator's ruling of 11-29 on the binding, form (b),
// in its work order `rd-1123/brief-455-rr3e.md`; contracts/scheduling/README.md, "The list's
// reader"), the host's half of ruling 2: a list a scheduled run published is bound to the run's
// task beside the reading the evaluation is given, from what the host noted in memory when the run
// published it:
// - a list a direct run published has no reader; the list a scheduled run publishes, its
//   settlement still to come, is bound to the run's task, and stays bound once the run is settled;
//   a list a direct run publishes after it has none again (h1);
// - after a start the list the scheduled run published has no reader, the note being memory only,
//   though its reading is computed as before (h2).
//
// The pool is read from rows of a list (#455 P2's fixture: `item_a` × 200 and `item_b` × 400 of
// `inventory.list.example_kind_a`, read as 1400); the task is catalog-a's, due while the pool holds
// 1000. Names and keys are made up; times are fixed Unix milliseconds on a manual Runtime clock,
// advanced before each publication, and nothing sleeps.

use super::*;
use crate::PolicyTrigger;
use actingcommand_contract::FactScalar;
use actingcommand_policy::{GivenPoolReadings, PolicyEvaluation};
use serde_json::{Value, json};

const ALIAS: &str = POLICY_INSTANCE_ALIAS;
const TASK: &str = "fixture.observe";
/// The pool, read from rows of [`LIST_KEY`].
const LISTED: &str = "pool.listed";
const LIST_KEY: &str = "inventory.list.example_kind_a";
/// When the list is first read, by a direct run: ten seconds before the fixture's time.
const READ_AT: u64 = POLICY_NOW_UNIX_MS - 10_000;
/// How long a list read stays fresh: an hour.
const FRESH_MS: u64 = 3_600_000;
/// How long the scheduled run takes on the Runtime clock: shorter than the dispatch lease's
/// lifetime in the tests' configuration (5 s).
const RUN_MS: u64 = 3_000;
/// When the scheduled run publishes its list: at its end.
const RUN_LIST_AT: u64 = POLICY_NOW_UNIX_MS + RUN_MS;

/// [`LISTED`], the policy instance's, read from rows `item_a` × 200 and `item_b` × 400 of
/// [`LIST_KEY`], valued (step 1, base 1000, gap weight 100).
fn list_pool() -> Value {
    json!({
        "id": LISTED,
        "scope": {"kind": "instance", "instance_id": ALIAS},
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

/// catalog-a declared V2 (its events valid from 0), its pools [`list_pool`] alone, its profiles
/// without goals, its task due while [`LISTED`] holds at least 1000, consuming and producing
/// nothing.
fn sources() -> CatalogSources {
    let mut catalog = policy_sources(1);
    for source in [
        &mut catalog.tasks,
        &mut catalog.pools,
        &mut catalog.activity,
        &mut catalog.timeline,
    ] {
        let mut document: Value =
            serde_json::from_slice(&source.bytes).expect("the fixture's JSON");
        document["schema_version"] = json!(actingcommand_policy::SCHEDULING_SCHEMA_VERSION_V2);
        if let Some(events) = document.get_mut("events").and_then(Value::as_array_mut) {
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
            task["consumes"] = json!([]);
            task["produces"] = json!([]);
        }
        if document.get("pools").is_some() {
            document["pools"] = json!([list_pool()]);
        }
        if let Some(profiles) = document.get_mut("profiles").and_then(Value::as_array_mut) {
            for profile in profiles {
                profile["goals"] = json!([]);
            }
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("the fixture's bytes");
    }
    catalog
}

fn text(value: &str) -> FactScalar {
    FactScalar::String(value.to_owned())
}

/// The list of [`LIST_KEY`], `item_a` 3 and `item_b` 2 (so the pool reads 1400), read at
/// `observed_at` under the snapshot `snapshot`, fresh for [`FRESH_MS`], at confidence 900.
fn list_read_at(observed_at: u64, snapshot: &str) -> FactRecord {
    let row = |key: &str, count: i64| {
        BTreeMap::from([
            ("id".to_owned(), text(key)),
            ("count".to_owned(), FactScalar::Integer(count)),
        ])
    };
    let mut fact = stored_fact(
        FactScope::Instance {
            instance_id: ALIAS.to_owned(),
        },
        LIST_KEY,
        ContractFactValue::RecordList(vec![row("item_a", 3), row("item_b", 2)]),
        snapshot,
        Vec::new(),
    );
    fact.observed_at_unix_ms = observed_at;
    fact.expires_at_unix_ms = Some(observed_at + FRESH_MS);
    // The shared fixture's lifetime policy admits two minutes at most, and an observation whose
    // lifetime lies outside its policy is refused (`fact_observation_invalid`): this list's own
    // policy admits its hour.
    if let Some(policy) = fact.ttl_policy.as_mut() {
        policy.maximum_ms = FRESH_MS;
    }
    fact.confidence_milli = 900;
    fact
}

/// A start of the policy instance's host over `state` on `clock`.
fn open(state: &TempDir, instance: InstanceId, clock: &Arc<ManualRuntimeClock>) -> RuntimeHost {
    RuntimeHost::start(
        config(state).with_runtime_clock(Arc::<ManualRuntimeClock>::clone(clock)),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the runtime host")
}

/// One evaluated cycle at `unix_ms`, the Runtime clock moved there and the host's capacity
/// sampled, over the fixture's facts, no pool snapshot configured.
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

/// The readings the evaluation is given now, as the authoritative projection gives them.
fn readings(host: &RuntimeHost) -> GivenPoolReadings {
    host.projected_policy_inputs_for_test()
        .expect("the projected inputs")
        .2
}

/// The task the list is bound to now.
fn reader(host: &RuntimeHost) -> Option<String> {
    readings(host).reader(ALIAS, LIST_KEY).map(str::to_owned)
}

/// A host on `state` with the fixture's catalog active and the list read by a direct run at
/// [`READ_AT`]; the task, due on that reading, is dispatched and admitted, its run publishes the
/// list at its end ([`RUN_LIST_AT`]) before its settlement, and it is settled, its lease ended.
/// The host, with the reader the list had before the run's publication and after it.
fn published_by_a_scheduled_run(
    state: &TempDir,
    instance: InstanceId,
    clock: &Arc<ManualRuntimeClock>,
) -> (RuntimeHost, Option<String>, Option<String>) {
    let host = open(state, instance, clock);
    host.activate_policy_catalog(&sources())
        .expect("activate the fixture catalog");
    evaluate_at(&host, clock, POLICY_NOW_UNIX_MS);
    host.publish_list_reading_for_test(list_read_at(READ_AT, "snapshot:rr3e-direct"))
        .expect("the list a direct run read");
    let before = reader(&host);
    let evaluation = evaluate_at(&host, clock, POLICY_NOW_UNIX_MS);
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|dispatch| dispatch.task_id == TASK && dispatch.instance_id == ALIAS)
        .unwrap_or_else(|| panic!("the task is due: {evaluation:#?}"))
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("admit the task")
    else {
        panic!("the task is granted");
    };
    clock.advance(RUN_MS);
    host.publish_scheduled_list_reading_for_test(
        &context,
        list_read_at(RUN_LIST_AT, "snapshot:rr3e-run"),
    )
    .expect("the list the scheduled run read");
    let after = reader(&host);
    host.record_policy_dispatch_outcome(&intent.decision_id, &PolicyExecutionInput::Succeeded)
        .expect("the settlement");
    host.release_policy_dispatch_lease_for_test(&context)
        .expect("end the dispatch's lease");
    (host, before, after)
}

// h1 (the study's first R3b host test, the binding of form (b)). The list a direct run published has
// no reader; the list the task's scheduled run publishes before its settlement is bound to that
// task, and stays bound once the run is settled; a list a direct run publishes afterwards has no
// reader again, as its record names no run whose task was noted.
#[test]
fn rr3e_h1_a_list_a_scheduled_run_publishes_is_bound_to_its_task() {
    let state = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let (host, before, after) = published_by_a_scheduled_run(&state, instance_id(), &clock);
    assert_eq!(before, None, "a list a direct run published has no reader");
    assert_eq!(after.as_deref(), Some(TASK));
    assert_eq!(
        reader(&host).as_deref(),
        Some(TASK),
        "settled, the run's list keeps its reader"
    );
    clock.advance(1_000);
    host.publish_list_reading_for_test(list_read_at(
        RUN_LIST_AT + 1_000,
        "snapshot:rr3e-direct-again",
    ))
    .expect("a list a direct run read since");
    assert_eq!(reader(&host), None, "a list a direct run published since");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// h2 (the study's second R3b host test, a known limit of form (b)). After a start of the host on the
// same state root, the list the scheduled run published is still the instance's list and its
// reading is computed as before, but it has no reader: the note was memory only.
#[test]
fn rr3e_h2_after_a_start_the_list_has_no_reader_until_a_scheduled_run_publishes_it_again() {
    let state = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let (host, _, after) = published_by_a_scheduled_run(&state, instance, &clock);
    assert_eq!(after.as_deref(), Some(TASK));
    host.close().expect("close the host");

    let clock = Arc::new(ManualRuntimeClock::new(RUN_LIST_AT + 5_000, 0));
    let host = open(&state, instance, &clock);
    let given = readings(&host);
    assert!(
        given.get(LISTED).is_some(),
        "the reading is computed: {given:?}"
    );
    assert_eq!(given.reader(ALIAS, LIST_KEY), None);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}
