// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 POOL-1 (forced-compromises mapping row POOL-1, compromise S-04; register §1.76;
// host gate key 95): a `ledger_fact` pool takes its data from readings, the base reset by a
// reading and projected by the pool's regeneration rule.
//
// - End to end on the D2-CHAIN fixture (gate_384_d2s4): the pool's key is unknown, so the task
//   its projection gates waits and the instance's recognition maintenance package is requested
//   for the key; the package's reading of the key is published, as any reading is, with no input
//   invalidation (no `contained_task_resource_reading_live_pool_unsupported` any more); the
//   evaluation takes it as the pool's base and regenerates it, one unit per complete period,
//   until the task's threshold is reached.
// - The source check (register §1.76 PL2): a record the Runtime published from a package reading
//   needs no input invalidation, and a newer reading resets the base; any other record of the key
//   without them still refuses the evaluation (`pool_fact_invalidation_binding_missing`,
//   docs/live-fact-pools.md).

use super::gate_384_d2s4::{
    BUSINESS_TASK, READ_KEY, RECOGNITION_PACKAGE_ID, TerminalReadings, admit_refresh,
    chain_sources, intent_of, recognition_package,
};
use super::*;
use actingcommand_policy::{EligibilityState, FACT_REFRESH_TASK_ID, PolicyEvaluation};

// The fixture pool's id (contracts/scheduling/examples/catalog-a).
const POOL: &str = "fixture-pool-a";
// The pool regenerates one unit per complete period of ten seconds.
const PERIOD_MS: u64 = 10_000;
// The recognition maintenance package reads 35 on its terminal page; the task waits for 38.
const READ_VALUE: i64 = 35;
const THRESHOLD: i64 = 38;
// A package reading of the second test is valid for two minutes, its TTL's maximum.
const VALID_MS: u64 = 120_000;
const PENDING: &str = "fact_refresh_pending";
const REQUESTED: &str = "fact_refresh_requested";

// The D2-CHAIN catalog whose pool observes the recognition package's terminal key through the
// ledger and regenerates one unit per period up to its capacity, 120; its task is triggered by
// the pool's projection reaching THRESHOLD.
fn live_pool_sources() -> CatalogSources {
    let mut sources = chain_sources();
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("live pool tasks JSON");
    tasks["tasks"][0]["trigger"] = serde_json::json!({
        "kind": "resource_projection",
        "pool_id": POOL,
        "comparison": "greater_than_or_equal",
        "value": THRESHOLD
    });
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("live pool tasks bytes");
    let mut pools: serde_json::Value =
        serde_json::from_slice(&sources.pools.bytes).expect("live pool pools JSON");
    pools["pools"][0]["observation"] = serde_json::json!({"kind": "fact", "fact_key": READ_KEY});
    pools["pools"][0]["projection"] = serde_json::json!({"amount": 1, "per_ms": PERIOD_MS});
    pools["pools"][0]["value_source"] =
        serde_json::json!({"kind": "ledger_fact", "minimum_confidence_milli": 900});
    sources.pools.bytes = serde_json::to_vec_pretty(&pools).expect("live pool pools bytes");
    sources
}

// A host under the live pool catalog whose instance binds the recognition maintenance package at
// `package`, when one is given.
fn live_pool_host(root: &TempDir, package: Option<&Path>, state: &Arc<FakeState>) -> RuntimeHost {
    let host = RuntimeHost::start(
        config(root).with_recognition_packages(
            package
                .map(|package| (POLICY_INSTANCE_ALIAS.to_owned(), package.to_path_buf()))
                .into_iter()
                .collect(),
        ),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(state))
                .fixture_simulation()
                .with_vision_provider(Arc::new(TerminalReadings)),
        ),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&live_pool_sources())
        .expect("activate the live pool catalog");
    host
}

// One full policy cycle at `unix_ms`. A `ledger_fact` pool is kept out of the static pool
// snapshot (docs/live-fact-pools.md), so the configured resources hold the hosts only.
fn evaluate_at(host: &RuntimeHost, unix_ms: u64, seed: u64) -> PolicyEvaluation {
    let mut resources = policy_resources();
    resources.pools.clear();
    host.evaluate_policy_cycle_with_test_inputs(
        &policy_facts(),
        &resources,
        EvaluationTime {
            unix_ms,
            monotonic_ms: unix_ms,
        },
        seed,
        PolicyTrigger::Recovery,
    )
    .unwrap_or_else(|error| panic!("the round at {unix_ms} is evaluated: {error:?}"))
    .evaluation
    .expect("a full evaluation")
}

fn task_decision(evaluation: &PolicyEvaluation) -> &actingcommand_policy::TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == BUSINESS_TASK)
        .unwrap_or_else(|| panic!("the task's decision: {evaluation:#?}"))
}

fn has_intent(evaluation: &PolicyEvaluation, task_id: &str) -> bool {
    evaluation
        .dispatch_intents
        .iter()
        .any(|intent| intent.task_id == task_id)
}

// FCM POOL-1, S-04: (1) the live pool has no base, so its task waits under
// `fact_refresh_pending` for the pool's key and the recognition maintenance package is requested
// for it (`fact_refresh_requested`, `fact_missing`); (2) the package's run reads the key and the
// reading is published with an empty `invalidate_on`; (3) the next rounds take the reading as the
// pool's base: 35 regenerates to 37 one millisecond before the third period after the reading
// ends, below the task's 38, and to 38 when it ends, when the task is dispatched.
#[test]
fn pool1_a_reading_feeds_the_live_pool_which_regenerates_from_it() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("recognition.zip");
    fs::write(&package, recognition_package(false)).expect("write the recognition package");
    let state = Arc::new(FakeState::default());
    // The package starts on its home page; after its tap the device shows the terminal page.
    state
        .transition_capture_after_inputs
        .store(1, Ordering::Release);
    let host = live_pool_host(&root, Some(&package), &state);

    let first = evaluate_at(&host, unix_ms_now().expect("wall clock"), 61);
    assert!(
        !has_intent(&first, BUSINESS_TASK),
        "the task is not dispatched on a pool without a base"
    );
    let waiting = task_decision(&first);
    let pending = waiting
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == PENDING)
        .unwrap_or_else(|| panic!("the task waits for the reader: {:?}", waiting.reasons));
    assert_eq!(
        pending.outcome.value("fact_key"),
        Some(&serde_json::json!(READ_KEY))
    );
    assert_eq!(
        pending.outcome.value("unknown_reason"),
        Some(&serde_json::json!("fact_missing"))
    );
    assert_eq!(
        pending.outcome.value("package_id"),
        Some(&serde_json::json!(RECOGNITION_PACKAGE_ID))
    );
    let (refresh, refresh_chain) = intent_of(&first, FACT_REFRESH_TASK_ID);
    let requested = refresh_chain
        .reasons
        .iter()
        .filter(|reason| reason.outcome.code().as_str() == REQUESTED)
        .collect::<Vec<_>>();
    let [requested] = requested.as_slice() else {
        panic!("one requested key: {:?}", refresh_chain.reasons);
    };
    assert_eq!(
        requested.outcome.value("fact_key"),
        Some(&serde_json::json!(READ_KEY))
    );

    record_policy_approval(&host, &refresh);
    let (context, request) = admit_refresh(&host, &refresh, &refresh_chain);
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("the recognition maintenance package runs");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the run's outcome: {:?}; the Runtime's fatal error: {:?}",
        receipt.outcome(),
        host.fatal_error()
    );
    host.complete_scheduled_policy_run(&context, &receipt)
        .expect("the fact refresh settles");
    let base = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("instance facts")
        .records
        .into_iter()
        .find(|record| record.key == READ_KEY)
        .expect("the reading of the pool's key is published");
    assert_eq!(
        base.content,
        FactContent::Inline {
            value: ContractFactValue::Integer(READ_VALUE)
        }
    );
    assert!(
        base.invalidate_on.is_empty(),
        "a reading carries no input invalidation: {:?}",
        base.invalidate_on
    );

    let before = evaluate_at(&host, base.observed_at_unix_ms + 3 * PERIOD_MS - 1, 62);
    assert_eq!(task_decision(&before).eligibility, EligibilityState::False);
    assert!(!has_intent(&before, BUSINESS_TASK));
    assert!(
        !has_intent(&before, FACT_REFRESH_TASK_ID),
        "nothing requests the reader once the pool has its base"
    );
    let reached = evaluate_at(&host, base.observed_at_unix_ms + 3 * PERIOD_MS, 63);
    assert_eq!(task_decision(&reached).eligibility, EligibilityState::True);
    let (_, chain) = intent_of(&reached, BUSINESS_TASK);
    assert!(
        chain.reasons.iter().all(|reason| !reason
            .outcome
            .code()
            .as_str()
            .starts_with("fact_refresh_")),
        "{:?}",
        chain.reasons
    );
    host.close().expect("close the host");
}

// The record of a package reading of the pool's key as the Runtime publishes it (detector
// `resource_reading:<task>/<id>`, no input invalidation), read at `observed_at` and valid for two
// minutes.
fn package_reading(snapshot: &str, value: i64, observed_at: u64) -> FactRecord {
    let mut record = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        READ_KEY,
        ContractFactValue::Integer(value),
        snapshot,
        Vec::new(),
    );
    record.observed_at_unix_ms = observed_at;
    record.expires_at_unix_ms = Some(observed_at + VALID_MS);
    record.confidence_milli = 990;
    record.source_detector = format!(
        "{}task/k2",
        actingcommand_contract::RESOURCE_READING_DETECTOR_PREFIX
    );
    record
}

// Register §1.76 PL2 (FCM POOL-1, S-04): a package reading of the pool's key is the pool's base
// without input invalidations. 30 read a minute ago (T0) regenerates to 37 one millisecond
// before eight periods after T0 and to 38, the task's threshold, at eight periods. A newer
// reading of 31 at T1 resets the base: twenty seconds after T1 the pool is 33, though the old
// base would have reached 38, and it reaches 38 seven periods after T1. A record of the key that
// is no package reading and carries no input invalidation still refuses the evaluation.
#[test]
fn pool1_a_package_reading_is_the_pool_s_base_without_input_invalidations() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = live_pool_host(&root, None, &state);

    let first_read = unix_ms_now().expect("wall clock") - 60_000;
    host.publish_fact(package_reading(
        "run:fixture/frame:first/k2",
        30,
        first_read,
    ))
    .expect("the first reading is published");
    for (at, eligibility) in [
        (first_read + 8 * PERIOD_MS - 1, EligibilityState::False),
        (first_read + 8 * PERIOD_MS, EligibilityState::True),
    ] {
        let evaluation = evaluate_at(&host, at, 71);
        assert_eq!(
            task_decision(&evaluation).eligibility,
            eligibility,
            "at {at}"
        );
        assert_eq!(
            has_intent(&evaluation, BUSINESS_TASK),
            eligibility == EligibilityState::True,
            "at {at}"
        );
    }

    let newer_read = unix_ms_now().expect("wall clock");
    host.publish_fact(package_reading(
        "run:fixture/frame:newer/k2",
        31,
        newer_read,
    ))
    .expect("the newer reading is published");
    for (at, eligibility) in [
        (newer_read + 2 * PERIOD_MS, EligibilityState::False),
        (newer_read + 7 * PERIOD_MS - 1, EligibilityState::False),
        (newer_read + 7 * PERIOD_MS, EligibilityState::True),
    ] {
        let evaluation = evaluate_at(&host, at, 72);
        assert_eq!(
            task_decision(&evaluation).eligibility,
            eligibility,
            "at {at}"
        );
    }

    let manual_at = unix_ms_now().expect("wall clock").max(newer_read + 1);
    let mut manual = package_reading("snapshot:manual", 31, manual_at);
    manual.source_detector = "detector.fixture".to_owned();
    host.publish_fact(manual)
        .expect("an observation without input invalidations is published");
    let mut resources = policy_resources();
    resources.pools.clear();
    let refused = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &resources,
            EvaluationTime {
                unix_ms: newer_read + 7 * PERIOD_MS,
                monotonic_ms: newer_read + 7 * PERIOD_MS,
            },
            73,
            PolicyTrigger::Recovery,
        )
        .expect_err("a live pool record that is no package reading needs the input invalidations");
    assert_eq!(refused.code(), "pool_fact_invalidation_binding_missing");
    host.close().expect("close the host");
}
