// SPDX-License-Identifier: AGPL-3.0-only

// #455 P2 (0.12.79; register §1.505; the coordinator's rulings 1 to 13 of 11-22 in its work order,
// not the owner's words; contracts/scheduling/README.md, "A pool read from rows of a list"), the
// evaluator's half. The host computes the reading of a pool read from rows of a list before each
// evaluation and gives it to the evaluator ([`GivenPoolReadings`], beside the left-out facts); here
// the readings are given as the host gives them, and every reader of a pool's reading takes them:
// - the number goes where a fact pool's projected reading goes, and a catalog without the kind
//   projects the bytes it projects on main (golden of the neutral example catalog) (t1);
// - a `resource_projection` reads it; an unknown one carries the list's key with its reason as the
//   suggestion of its reason, and raises no fact refresh demand (t2);
// - its expiry wakes the evaluation and then reads `fact_expired` (t3); a timeline reset that
//   invalidates the list invalidates the reading (t4);
// - the net benefit's cost term (t5), the pool's lines and the capped state (t6), the reserve gate
//   with an unknown reading (t7) and the consumption a reading does not show yet (t8);
// - the forward projection starts from it and carries it as a fact reading (t9);
// - a resource target's check and receipt condition (t10), its gap in the evaluation, applied and
//   pending (t11), and the views (t12);
// - fix round 1 (coordinator brief rd-1122/brief-1868-fix1.md, items C1 and D): the no-progress
//   guard's basis takes the reading as a fact pool's takes its fact (t13), and `capped_spend`
//   shows it (t14).
//
// The keys, columns and ids are made up; nothing reads a clock.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::{
    CandidateEligibility, CappedMemory, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DispatchIntent, EvaluationFacts, EvaluationResources, EvaluationTime,
    FactRefreshInputs, FactRefreshSettings, FactValue, ForwardEvidenceGap, ForwardProjectionConfig,
    GivenPoolReading, GivenPoolReadings, GivenReadingState, HostResourceSnapshot, InstanceHolds,
    InstancePoolLines, LeftOutFacts, ListRowsUnknown, NoProgressGuards, NoProgressMemory,
    ObservedFact, OwedReruns, PolicyEvaluation, PolicyEvaluationError, PoolValueSnapshot,
    RemedyPlan, RunSlots, SCHEDULING_SCHEMA_VERSION_V2, ScopeSelector, StoredResourceTargets,
    TaskDecision, TaskRuntimeSnapshot, TaskTerminalState, active_resource_targets,
    check_resource_targets, check_resource_targets_with_readings, compile_catalog,
    evaluate_with_left_out_facts, parse_resource_targets, project_fact_pools,
    project_fact_pools_with_readings, project_forward, project_forward_with_readings,
    targetable_resources_with_readings,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    RESOURCE_TARGETS_FACT_KEY, ResourceTargetConditionState, ResourceTargetPendingReason,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// The list the pool is read from.
const LIST_KEY: &str = "inventory.list.example_kind_a";
/// Read from rows of [`LIST_KEY`] (`item_a` × 200, `item_b` × 400).
const LISTED: &str = "pool.listed";
/// Read from its own fact, which no input holds.
const VALUED: &str = "pool.valued";
const TASK: &str = "task.x";
const OTHER: &str = "task.y";
/// When the list the readings are computed from was read.
const READ_AT: u64 = NOW - 40_000;
const V2_TARGETS: &str = "actingcommand.resource-targets.v2";

// Test 1, where the number goes (ruling 5). A reading the host computed is the pool's snapshot,
// its value the sum and its observation the list's, where a fact pool's projected reading goes; a
// caller's snapshot of the pool is dropped, as every `ledger_fact` pool's is. No snapshot for an
// unknown reading, for a sum the pool's constant capacity cannot hold, and without a reading. A
// catalog without the kind projects the bytes it projects without readings: the neutral example
// catalog keeps its golden bytes.
#[test]
fn p2_t1_the_given_number_goes_where_a_fact_pools_projected_reading_goes() {
    let catalog = catalog(
        vec![task(TASK, clock(), &[], &[])],
        vec![list_pool(10_000, none())],
        Vec::new(),
    );
    let facts = facts(Vec::new());
    let caller = EvaluationResources {
        pools: vec![PoolValueSnapshot {
            pool_id: LISTED.to_owned(),
            value: 0,
            observed_at_unix_ms: NOW,
        }],
        hosts: vec![host()],
    };
    let given = project_fact_pools_with_readings(&catalog, &facts, &caller, &readings(read(1_400)));
    assert_eq!(
        given.pools,
        [PoolValueSnapshot {
            pool_id: LISTED.to_owned(),
            value: 1_400,
            observed_at_unix_ms: READ_AT,
        }]
    );
    for (case, readings) in [
        ("unknown", readings(unknown(ListRowsUnknown::TypeMismatch))),
        ("above the capacity", readings(read(10_001))),
        ("no reading", GivenPoolReadings::default()),
    ] {
        let projected = project_fact_pools_with_readings(&catalog, &facts, &caller, &readings);
        assert!(projected.pools.is_empty(), "{case}: {projected:?}");
    }
    assert!(
        project_fact_pools(&catalog, &facts, &caller)
            .pools
            .is_empty()
    );

    let example = example_catalog();
    let resources = EvaluationResources {
        pools: vec![PoolValueSnapshot {
            pool_id: "fixture-pool-a".to_owned(),
            value: 10,
            observed_at_unix_ms: NOW,
        }],
        hosts: vec![host()],
    };
    let without = project_fact_pools_with_readings(
        &example,
        &facts,
        &resources,
        &GivenPoolReadings::default(),
    );
    let bytes = serde_json::to_string(&without).expect("the resources serialize");
    assert_eq!(bytes, EXAMPLE_RESOURCES_GOLDEN);
    assert_eq!(without, project_fact_pools(&example, &facts, &resources));
}

/// The neutral example catalog's projected resources (its one pool a static snapshot), as main
/// serializes them: the pool's snapshot and the host, the threshold left out.
const EXAMPLE_RESOURCES_GOLDEN: &str = concat!(
    r#"{"pools":[{"pool_id":"fixture-pool-a","value":10,"observed_at_unix_ms":3600060000}],"#,
    r#""hosts":[{"host_id":"host.instance-a","cpu_available_milli":1000,"#,
    r#""gpu_available_milli":1000,"io_available_milli":1000,"#,
    r#""host_responsiveness_basis_points":10000,"third_party_pressure_basis_points":0,"#,
    r#""heavy_dispatch_limit":1,"active_heavy_dispatches":0}]}"#
);

// Test 2, the suggestion a reason carries and no refresh need (rulings 3, 8 and 11). A task due
// while the pool holds at least 1000 runs on a reading of 1400. On a reading the host could not
// compute (`type_mismatch`) it is `trigger_unknown` with the suggestion of the list's key in the
// pool's scope and that reason, and with the reading given beside the left-out facts the key raises
// no fact refresh demand; without it the refresh is asked for the key, as before P2.
#[test]
fn p2_t2_a_projection_reads_the_reading_and_an_unknown_one_suggests_the_list_without_a_refresh() {
    let catalog = catalog(
        vec![task(TASK, at_least(1_000), &[], &[])],
        vec![list_pool(10_000, none())],
        Vec::new(),
    );
    let facts = facts(Vec::new());
    assert!(dispatched(&round(&catalog, &facts, &readings(read(1_400)), NOW)).contains(&TASK));
    let mismatch = readings(unknown(ListRowsUnknown::TypeMismatch));
    let evaluation = round(&catalog, &facts, &mismatch, NOW);
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let unknown = decision(&evaluation, TASK);
    assert!(codes(unknown).contains(&"trigger_unknown"), "{unknown:#?}");
    assert!(suggests(unknown, "type_mismatch"), "{unknown:#?}");
    assert!(
        codes(unknown)
            .iter()
            .all(|code| !code.starts_with("fact_refresh_")),
        "no fact refresh demand: {unknown:#?}"
    );
    let resources = resources(&catalog, &facts, &mismatch);
    let without = round_with(&catalog, &facts, &resources, &LeftOutFacts::default(), NOW);
    let asked = decision(&without, TASK);
    assert!(
        codes(asked)
            .iter()
            .any(|code| code.starts_with("fact_refresh_")),
        "without the given reading the key is asked for: {asked:#?}"
    );
}

// Test 3, expiry (rulings 4 and 9). A reading of a list that expires a second after NOW wakes the
// evaluation a millisecond after that expiry, as a fact reading's does; then the pool reads
// `fact_expired` and the task waits for it.
#[test]
fn p2_t3_the_lists_expiry_wakes_the_evaluation_and_then_reads_fact_expired() {
    let catalog = catalog(
        vec![task(TASK, at_least(1_000), &[], &[])],
        vec![list_pool(10_000, none())],
        Vec::new(),
    );
    let facts = facts(Vec::new());
    let expires_at = NOW + 1_000;
    let given = readings(GivenReadingState::Read {
        value: 1_400,
        observed_at_unix_ms: READ_AT,
        expires_at_unix_ms: Some(expires_at),
    });
    let fresh = round(&catalog, &facts, &given, NOW);
    assert!(dispatched(&fresh).contains(&TASK), "{fresh:#?}");
    assert_eq!(fresh.next_wake_unix_ms, Some(expires_at + 1));
    let expired = round(&catalog, &facts, &given, expires_at + 1);
    assert!(dispatched(&expired).is_empty(), "{expired:#?}");
    let waiting = decision(&expired, TASK);
    assert!(codes(waiting).contains(&"trigger_unknown"), "{waiting:#?}");
    assert!(suggests(waiting, "fact_expired"), "{waiting:#?}");
}

// Test 4, a timeline reset (ruling 3). A daily reset of the instance half an hour before NOW
// invalidates `inventory.`: a reading of a list read before it is gone (the pool reads
// `fact_missing`), one read after it is the pool's value.
#[test]
fn p2_t4_a_reset_that_invalidates_the_list_invalidates_the_reading() {
    let catalog = catalog(
        vec![task(TASK, at_least(1_000), &[], &[])],
        vec![list_pool(10_000, none())],
        vec![reset(NOW - 30 * MINUTE_MS)],
    );
    let facts = facts(Vec::new());
    let read_at = |observed_at_unix_ms: u64| {
        readings(GivenReadingState::Read {
            value: 1_400,
            observed_at_unix_ms,
            expires_at_unix_ms: Some(NOW + HOUR_MS),
        })
    };
    let before = round(&catalog, &facts, &read_at(NOW - HOUR_MS), NOW);
    assert!(dispatched(&before).is_empty(), "{before:#?}");
    assert!(
        suggests(decision(&before, TASK), "fact_missing"),
        "{before:#?}"
    );
    let after = round(&catalog, &facts, &read_at(NOW - 10 * MINUTE_MS), NOW);
    assert!(dispatched(&after).contains(&TASK), "{after:#?}");
}

// Test 5, the net benefit's cost term (ruling 7). A judged task consumes 1 of a valued
// regenerating pool read from rows of a list (capacity 100) and produces 5 of a valued pool: on a
// reading of 40 its benefit is known and it runs; on a reading below the pool's confidence, and on
// a sum the capacity cannot hold, it is `benefit_unknown` `reading_unknown` naming the pool, the
// list's key and the reason, never a known part.
#[test]
fn p2_t5_the_cost_term_reads_the_reading_and_names_the_list_when_unknown() {
    let catalog = catalog(
        vec![judged(task(TASK, clock(), &[(LISTED, 1)], &[(VALUED, 5)]))],
        vec![
            list_pool(100, json!({"amount": 1, "per_ms": 6 * MINUTE_MS})),
            valued_pool(),
        ],
        Vec::new(),
    );
    let facts = facts(Vec::new());
    assert!(dispatched(&round(&catalog, &facts, &readings(read(40)), NOW)).contains(&TASK));
    for (state, reason) in [
        (
            unknown(ListRowsUnknown::FactLowConfidence),
            "fact_low_confidence",
        ),
        (read(150), "type_mismatch"),
    ] {
        let evaluation = round(&catalog, &facts, &readings(state), NOW);
        assert!(dispatched(&evaluation).is_empty(), "{reason}");
        let last = last(decision(&evaluation, TASK));
        assert!(
            last.is(crate::PolicyCode::BenefitUnknown),
            "{reason}: {last:?}"
        );
        assert_eq!(last.value("benefit_gap"), Some(&json!("reading_unknown")));
        assert_eq!(last.value("pool_id"), Some(&json!(LISTED)), "{reason}");
        assert_eq!(last.value("fact_key"), Some(&json!(LIST_KEY)), "{reason}");
        assert_eq!(last.value("unknown_reason"), Some(&json!(reason)));
        assert_eq!(last.value("benefit_milli"), None, "{reason}");
    }
}

// Test 6, the pool's lines and the capped state (rulings 7 and 11). A judged task consumes 1 of a
// valued pool read from rows of a list with a stop line of 800 and a hard line of 100: on a
// reading of 100 its run would leave 99, below the hard line, and it is held `pool_line_held`
// `would_cross_hard_line` with the reading 100; on 500 it runs; on a reading below the pool's
// confidence it is held `capped_cap_unknown` naming the list's key and that reason.
#[test]
fn p2_t6_the_lines_read_the_reading_and_capped_cap_unknown_names_the_list() {
    let mut pool = list_pool(1_000, none());
    pool["stop_line"] = json!(800);
    pool["hard_line"] = json!(100);
    let catalog = catalog(
        vec![judged(task(TASK, clock(), &[(LISTED, 1)], &[(VALUED, 5)]))],
        vec![pool, valued_pool()],
        Vec::new(),
    );
    let facts = facts(Vec::new());
    let held = round(&catalog, &facts, &readings(read(100)), NOW);
    assert!(dispatched(&held).is_empty(), "{held:#?}");
    let line = coded(decision(&held, TASK), "pool_line_held");
    assert_eq!(
        line.value("pool_line_hold"),
        Some(&json!("would_cross_hard_line"))
    );
    assert_eq!(line.value("pool_reading"), Some(&json!(100)), "{line:?}");
    assert!(dispatched(&round(&catalog, &facts, &readings(read(500)), NOW)).contains(&TASK));
    let unknown = round(
        &catalog,
        &facts,
        &readings(unknown(ListRowsUnknown::FactLowConfidence)),
        NOW,
    );
    assert!(dispatched(&unknown).is_empty(), "{unknown:#?}");
    let line = coded(decision(&unknown, TASK), "pool_line_held");
    assert_eq!(
        line.value("pool_line_hold"),
        Some(&json!("capped_cap_unknown"))
    );
    assert_eq!(line.value("fact_key"), Some(&json!(LIST_KEY)), "{line:?}");
    assert_eq!(
        line.value("unknown_reason"),
        Some(&json!("fact_low_confidence"))
    );
}

// Test 7, the reserve gate (ruling 7). A task that consumes 30 and must leave at least 50 runs on
// a reading of 100; on an unknown reading it is held `resource_reserve_insufficient` without an
// amount (case 1 of "Task Reserves").
#[test]
fn p2_t7_the_reserve_gate_reads_the_reading_and_holds_on_an_unknown_one() {
    let catalog = catalog(
        vec![reserved(task(TASK, clock(), &[(LISTED, 30)], &[]), 50)],
        vec![list_pool(1_000, none())],
        Vec::new(),
    );
    let facts = facts(Vec::new());
    assert!(dispatched(&round(&catalog, &facts, &readings(read(100)), NOW)).contains(&TASK));
    let evaluation = round(
        &catalog,
        &facts,
        &readings(unknown(ListRowsUnknown::FactMissing)),
        NOW,
    );
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let last = last(decision(&evaluation, TASK));
    assert_eq!(
        last.code().as_str(),
        "resource_reserve_insufficient",
        "{last:?}"
    );
    assert_eq!(last.value("pool_id"), Some(&json!(LISTED)));
    assert_eq!(last.value("reserve_at_least"), Some(&json!(50)));
    assert_eq!(last.value("amount_after"), None, "case 1: {last:?}");
}

// Test 8, the consumption a reading does not show yet (ruling 4). Task Y, which consumes 30 of
// the pool, was dispatched ten seconds after the list was read: the reading of 100 does not show
// it, so the task with the reserve of 50 would leave 100 − 30 − 30 = 40 and waits for Y. Y
// dispatched ten seconds before the list was read is in the reading, and the task runs.
#[test]
fn p2_t8_the_consumption_since_the_lists_observation_is_counted() {
    let catalog = catalog(
        vec![
            reserved(task(TASK, clock(), &[(LISTED, 30)], &[]), 50),
            task(OTHER, clock(), &[(LISTED, 30)], &[]),
        ],
        vec![list_pool(1_000, none())],
        Vec::new(),
    );
    let given = readings(read(100));
    let after = round(
        &catalog,
        &facts(vec![ran(OTHER, READ_AT + 10_000)]),
        &given,
        NOW,
    );
    assert!(!dispatched(&after).contains(&TASK), "{after:#?}");
    let last = last(decision(&after, TASK));
    assert_eq!(last.code().as_str(), "resource_reserve_insufficient");
    assert_eq!(last.value("amount_after"), Some(&json!(40)), "{last:?}");
    assert_eq!(last.value("task_id"), Some(&json!(OTHER)), "{last:?}");
    let before = round(
        &catalog,
        &facts(vec![ran(OTHER, READ_AT - 10_000)]),
        &given,
        NOW,
    );
    assert!(dispatched(&before).contains(&TASK), "{before:#?}");
}

// Test 9, the forward projection (ruling 7). A task due while the pool holds at least 1000, with
// a cooldown of three hours, over a reading of 1400 whose list expires two hours after NOW: the
// projection starts from the reading and runs the task once, then the reading expires as a fact
// reading does, and the projection names the gap `fact_expired` of the list's key; the pool ends
// at the reading. Without the reading the projection runs nothing and names `fact_missing`.
#[test]
fn p2_t9_the_forward_projection_starts_from_the_reading_and_carries_it() {
    let mut due = task(TASK, at_least(1_000), &[], &[]);
    due["cooldown_ms"] = json!(3 * HOUR_MS);
    let catalog = catalog(vec![due], vec![list_pool(10_000, none())], Vec::new());
    let facts = facts(Vec::new());
    let given = readings(GivenReadingState::Read {
        value: 1_400,
        observed_at_unix_ms: READ_AT,
        expires_at_unix_ms: Some(NOW + 2 * HOUR_MS),
    });
    let resources = resources(&catalog, &facts, &given);
    let config = ForwardProjectionConfig::next_24_hours();
    let projection =
        project_forward_with_readings(&catalog, &facts, &resources, &given, time(NOW), 11, config)
            .expect("the projection");
    let runs = projection
        .steps
        .iter()
        .map(|step| (step.evaluated_at_unix_ms, step.task_ids.clone()))
        .collect::<Vec<_>>();
    assert_eq!(runs, [(NOW, vec![TASK.to_owned()])], "{projection:#?}");
    assert!(
        projection.evidence_gaps.contains(&gap("fact_expired")),
        "{projection:#?}"
    );
    assert!(
        projection
            .final_pools
            .iter()
            .any(|pool| pool.pool_id == LISTED && pool.value == 1_400),
        "{projection:#?}"
    );
    let without = project_forward(&catalog, &facts, &resources, time(NOW), 11, config)
        .expect("the projection without the reading");
    assert!(without.steps.is_empty(), "{without:#?}");
    assert!(
        without.evidence_gaps.contains(&gap("fact_missing")),
        "{without:#?}"
    );
}

// Test 10, a resource target's check and the receipt's condition (ruling 7). A v2 target of at
// least 100 on the pool read from rows of a list is accepted (the pool is observable); its
// condition is computed from the reading (40, observed when the list was, the gap 60), awaits
// observation `low_confidence` on such a reading, and `missing` without one.
#[test]
fn p2_t10_a_target_on_the_pool_is_checked_and_its_condition_is_the_reading() {
    let catalog = targets_catalog();
    let facts = facts(Vec::new());
    let parsed = parse_resource_targets(
        &serde_json::to_vec(&target_document()).expect("the document's bytes"),
    )
    .expect("the document parses");
    let condition = |readings: &GivenPoolReadings| {
        let checked =
            check_resource_targets_with_readings(&parsed, &catalog, &facts, readings, time(NOW))
                .expect("the target is accepted");
        let [condition] = checked.conditions.as_slice() else {
            panic!("one condition: {:?}", checked.conditions);
        };
        assert_eq!(condition.fact_key, LIST_KEY);
        condition.state.clone()
    };
    assert_eq!(
        condition(&readings(read(40))),
        ResourceTargetConditionState::Computed {
            current: 40,
            observed_at_unix_ms: READ_AT,
            gap: 60,
        }
    );
    assert_eq!(
        condition(&readings(unknown(ListRowsUnknown::FactLowConfidence))),
        ResourceTargetConditionState::AwaitingObservation {
            reason: ResourceTargetPendingReason::LowConfidence,
        }
    );
    let checked = check_resource_targets(&parsed, &catalog, &facts, time(NOW))
        .expect("accepted without a reading");
    assert_eq!(
        checked.conditions[0].state,
        ResourceTargetConditionState::AwaitingObservation {
            reason: ResourceTargetPendingReason::Missing,
        }
    );
}

// Test 11, the target's gap in the evaluation (ruling 7). With the target stored, the producing
// task's `resource_targets` reason names it applied on a reading of 40 (gap 60), and pending
// `low_confidence` on such a reading.
#[test]
fn p2_t11_the_evaluations_target_is_applied_on_the_reading_and_pending_without_it() {
    let catalog = targets_catalog();
    let facts = facts_with(vec![stored_target(&catalog)], Vec::new());
    for (state, expected) in [
        (
            read(40),
            "goal-l@pool.listed:applied current=40 at_least=100 gap=60",
        ),
        (
            unknown(ListRowsUnknown::FactLowConfidence),
            "goal-l@pool.listed:pending reason=low_confidence",
        ),
    ] {
        let evaluation = round(&catalog, &facts, &readings(state), NOW);
        let task = decision(&evaluation, TASK);
        assert!(
            task.reasons.iter().any(|reason| {
                reason.outcome.code().as_str() == "resource_targets"
                    && reason.detail.contains(expected)
            }),
            "{expected}: {task:#?}"
        );
    }
}

// Test 12, the views (rulings 7 and 11). The pool is targetable, under its list's key, its
// observation the reading (40, observed when the list was), which a goal entry's `current` shows;
// the stored target's condition in the stored policy's view is computed from it too.
#[test]
fn p2_t12_the_views_show_the_reading() {
    let catalog = targets_catalog();
    let given = readings(read(40));
    let facts = facts_with(vec![stored_target(&catalog)], Vec::new());
    let (targetable, not_targetable) =
        targetable_resources_with_readings(&catalog, &facts, &given, &instance(A, &[]), time(NOW))
            .expect("the targetable pools");
    assert!(
        not_targetable.iter().all(|pool| pool.resource != LISTED),
        "{not_targetable:?}"
    );
    let listed = targetable
        .iter()
        .find(|pool| pool.resource == LISTED)
        .unwrap_or_else(|| panic!("the pool is targetable: {targetable:?}"));
    assert_eq!(listed.fact_key, LIST_KEY);
    assert_eq!(listed.observation.current, Some(40));
    assert_eq!(listed.observation.observed_at_unix_ms, Some(READ_AT));
    assert_eq!(listed.observation.pending, None);
    let StoredResourceTargets::Policy(stored) =
        active_resource_targets(Some(&catalog), &facts, &given, &instance(A, &[]), time(NOW))
            .expect("the stored policy")
    else {
        panic!("a stored policy");
    };
    assert_eq!(
        stored.conditions[0].state,
        ResourceTargetConditionState::Computed {
            current: 40,
            observed_at_unix_ms: READ_AT,
            gap: 60,
        }
    );
}

/// Test 13's fact of A that the same pool reads in the catalog it is compared with.
const COUNTED: &str = "resource.counted";
/// Test 13's task's admitted run.
const RAN_AT: u64 = NOW + 1_000;

// Test 13, the no-progress guard (fix round 1, item C1). A task due while the pool holds at least
// 1000, over a reading of 1400, runs once and its run is admitted; the list is not read again.
// The guard holds the task as it holds the same task over the same pool read from its own fact
// of 1400: `policy_no_progress` naming the one basis key (the list's key, or the fact's) with the
// admission's reading. A list (or fact) read again later with the same sum holds it still; one
// read with another sum (1600) releases the guard, and the task runs again.
#[test]
fn p2_t13_the_no_progress_guard_holds_a_task_on_the_reading_as_on_a_fact() {
    let due = || task(TASK, at_least(1_000), &[], &[]);
    let mut counted = list_pool(10_000, none());
    counted["observation"] = json!({"kind": "fact", "fact_key": COUNTED});
    for (case, catalog, key, listed) in [
        (
            "a pool read from rows of a list",
            catalog(vec![due()], vec![list_pool(10_000, none())], Vec::new()),
            LIST_KEY,
            true,
        ),
        (
            "the same pool read from its fact",
            catalog(vec![due()], vec![counted], Vec::new()),
            COUNTED,
            false,
        ),
    ] {
        let inputs = |value, read_at, tasks| guard_inputs(listed, value, read_at, tasks);
        let mut memory = NoProgressMemory::default();
        let (facts, given) = inputs(1_400, READ_AT, Vec::new());
        let first = guarded_cycle(&catalog, &mut memory, &facts, &given, NOW);
        let intent = first
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == TASK && intent.instance_id == A)
            .unwrap_or_else(|| panic!("{case}: the task runs: {first:#?}"));
        memory.admit(TASK, A, &intent.decision_id, RAN_AT);
        assert!(
            memory.armed(TASK, A).is_some(),
            "{case}: the guard is armed"
        );

        for (at, read_at) in [
            (NOW + 10 * MINUTE_MS, READ_AT),
            (NOW + 20 * MINUTE_MS, NOW + 15 * MINUTE_MS),
        ] {
            let (facts, given) = inputs(1_400, read_at, vec![ran(TASK, RAN_AT)]);
            let round = guarded_cycle(&catalog, &mut memory, &facts, &given, at);
            assert!(dispatched(&round).is_empty(), "{case} at {at}: {round:#?}");
            let held = coded(decision(&round, TASK), "policy_no_progress");
            assert_eq!(
                held.value("basis_readings"),
                Some(&json!([{
                    "fact_key": key,
                    "reading_value": "{\"type\":\"integer\",\"value\":1400}",
                    "observed_at_unix_ms": READ_AT,
                }])),
                "{case} at {at}"
            );
            assert_eq!(
                held.value("served_by_dispatch_unix_ms"),
                Some(&json!(RAN_AT)),
                "{case} at {at}"
            );
        }

        let (facts, given) = inputs(1_600, NOW + 25 * MINUTE_MS, vec![ran(TASK, RAN_AT)]);
        let again = guarded_cycle(&catalog, &mut memory, &facts, &given, NOW + 30 * MINUTE_MS);
        assert_eq!(dispatched(&again), [TASK], "{case}: {again:#?}");
        assert!(
            memory.armed(TASK, A).is_none(),
            "{case}: another sum released the guard"
        );
    }
}

// Test 14, what `capped_spend` shows (fix round 1, item D). A judged consumer of the pool read from
// rows of a list (capacity 1000, stop line 800, trigger line 900, hard line 100) is the head of
// the capped round its reading of 950 begins, and its `capped_spend` shows that reading.
#[test]
fn p2_t14_capped_spend_shows_the_reading() {
    let mut pool = list_pool(1_000, none());
    pool["stop_line"] = json!(800);
    pool["trigger_line"] = json!(900);
    pool["hard_line"] = json!(100);
    let catalog = catalog(
        vec![judged(task(TASK, clock(), &[(LISTED, 1)], &[(VALUED, 5)]))],
        vec![pool, valued_pool()],
        Vec::new(),
    );
    let evaluation = round(&catalog, &facts(Vec::new()), &readings(read(950)), NOW);
    assert!(dispatched(&evaluation).contains(&TASK), "{evaluation:#?}");
    let spend = coded(decision(&evaluation, TASK), "capped_spend");
    for (member, value) in [
        ("pool_id", json!(LISTED)),
        ("pool_reading", json!(950)),
        ("pool_capacity", json!(1_000)),
        ("stop_line", json!(800)),
        ("trigger_line", json!(900)),
        ("hard_line", json!(100)),
    ] {
        assert_eq!(spend.value(member), Some(&value), "{member}: {spend:?}");
    }
}

// ---- The readings, as the host gives them

fn read(value: u64) -> GivenReadingState {
    GivenReadingState::Read {
        value,
        observed_at_unix_ms: READ_AT,
        expires_at_unix_ms: Some(NOW + HOUR_MS),
    }
}

fn unknown(unknown: ListRowsUnknown) -> GivenReadingState {
    GivenReadingState::Unknown(unknown)
}

/// The reading of [`LISTED`] on A.
fn readings(state: GivenReadingState) -> GivenPoolReadings {
    let mut readings = GivenPoolReadings::default();
    readings.give(
        LISTED,
        GivenPoolReading {
            instance_id: A.to_owned(),
            fact_key: LIST_KEY.to_owned(),
            state,
        },
    );
    readings
}

// ---- The catalog: V2, one instance A of the game, every task on its game

fn clock() -> Value {
    json!({
        "kind": "clock",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": HOUR_MS,
            "anchor_ms": 0
        }
    })
}

/// Due while [`LISTED`] holds at least `value`.
fn at_least(value: i64) -> Value {
    json!({
        "kind": "resource_projection",
        "pool_id": LISTED,
        "comparison": "greater_than_or_equal",
        "value": value
    })
}

fn none() -> Value {
    json!({"kind": "none"})
}

fn effects(direction: &str, effects: &[(&str, u64)]) -> Value {
    Value::Array(
        effects
            .iter()
            .map(|(pool, amount)| {
                json!({
                    "pool_id": pool,
                    "direction": direction,
                    "amount": amount,
                    "observation_source": "scan_verified",
                    "confidence_milli": 1000
                })
            })
            .collect(),
    )
}

fn task(id: &str, trigger: Value, consumes: &[(&str, u64)], produces: &[(&str, u64)]) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
        "priority": 100,
        "trigger": trigger,
        "feedback_stop": {
            "kind": "outcome",
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": effects("consume", consumes),
        "produces": effects("produce", produces),
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": DAY_MS,
        "yield_points": [],
        "expected_duration_ms": 60000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

fn judged(mut task: Value) -> Value {
    task["benefit_judged"] = json!(true);
    task
}

/// `task` with a reserve of `at_least` on [`LISTED`].
fn reserved(mut task: Value, at_least: u64) -> Value {
    task["reserves"] = json!([{"pool_id": LISTED, "at_least": at_least}]);
    task
}

fn valuation() -> Value {
    json!({
        "name": "Fixture",
        "unit": "unit",
        "scale": 1,
        "base_weight_milli": 1000,
        "gap": {"rule": "shortfall_linear", "weight_milli": 100}
    })
}

/// [`LISTED`] of `capacity` and `projection`, valued, read from rows of [`LIST_KEY`].
fn list_pool(capacity: u64, projection: Value) -> Value {
    json!({
        "id": LISTED,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": capacity,
        "projection": projection,
        "observation": {
            "kind": "list_rows",
            "fact_key": LIST_KEY,
            "key_column": "id",
            "value_column": "count",
            "rows": [{"key": "item_a", "factor": 200}, {"key": "item_b", "factor": 400}]
        },
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": valuation()
    })
}

fn valued_pool() -> Value {
    json!({
        "id": VALUED,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": 1_000_000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": "resource.valued"},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": valuation()
    })
}

/// A daily reset of A at `at`, invalidating every key under `inventory.`.
fn reset(at: u64) -> Value {
    json!({
        "id": "event.reset",
        "scope": {"kind": "instance", "instance_id": A},
        "event_kind": "reset",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": DAY_MS,
            "anchor_ms": at
        },
        "duration_ms": 0,
        "invalidates_fact_prefixes": ["inventory."],
        "validity": {"from_unix_ms": 0, "until_unix_ms": null}
    })
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-455-p2/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

fn catalog(tasks: Vec<Value>, pools: Vec<Value>, events: Vec<Value>) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.455-p2",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-p2"]
    });
    let document = |member: &str, entries: Vec<Value>| {
        let mut document = json!({
            "schema_version": SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = Value::Array(entries);
        document
    };
    let profile = json!({
        "id": "fixture-activity",
        "scope": {"kind": "game", "game_id": GAME},
        "windows": [{
            "weekdays": [1, 2, 3, 4, 5, 6, 7],
            "utc_offset_minutes": 0,
            "start_minute_of_day": 0,
            "end_minute_of_day": 0
        }],
        "daily_budget": 24,
        "max_window_iterations": 4,
        "session_max_ms": 7200000,
        "detection_budget": {
            "window_dispatch_limit": 4,
            "window_runtime_ms": 300000,
            "expected_duration_ms": 60000
        },
        "minimum_interval_ms": 60000,
        "maximum_interval_ms": 300000,
        "seed_source": "ledger",
        "resample_policy": "same_round_stable",
        "importance_milli": 1000,
        "goals": []
    });
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", tasks)),
        pools: source("pools", &document("pools", pools)),
        activity: source("activity", &document("profiles", vec![profile])),
        timeline: source("timeline", &document("events", events)),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the catalog compiles: {:?}", failure.diagnostics()))
}

/// The catalog of the target tests: the task produces 20 of [`LISTED`].
fn targets_catalog() -> CompiledCatalog {
    catalog(
        vec![task(TASK, clock(), &[], &[(LISTED, 20)])],
        vec![list_pool(1_000, none())],
        Vec::new(),
    )
}

/// The neutral example catalog `catalog-a`, as written.
fn example_catalog() -> CompiledCatalog {
    let bytes = |name: &str, bytes: &[u8]| {
        CatalogDocumentSource::new(
            format!("memory://fixture-455-p2/{name}.json"),
            bytes.to_vec(),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: bytes(
            "tasks",
            include_bytes!("../../../../contracts/scheduling/examples/catalog-a/tasks.json"),
        ),
        pools: bytes(
            "pools",
            include_bytes!("../../../../contracts/scheduling/examples/catalog-a/pools.json"),
        ),
        activity: bytes(
            "activity",
            include_bytes!("../../../../contracts/scheduling/examples/catalog-a/activity.json"),
        ),
        timeline: bytes(
            "timeline",
            include_bytes!("../../../../contracts/scheduling/examples/catalog-a/timeline.json"),
        ),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the example compiles: {:?}", failure.diagnostics()))
}

/// The v2 target `goal-l` of at least 100 on [`LISTED`].
fn target_document() -> Value {
    json!({
        "schema_version": V2_TARGETS,
        "instance": A,
        "valid_until_unix_ms": NOW + DAY_MS,
        "targets": [{
            "id": "goal-l",
            "resource": LISTED,
            "condition": {"kind": "at_least", "amount": 100},
            "apply": {"mode": "adjust", "weight": "score_stage"}
        }]
    })
}

/// The record the formal entry stores for [`target_document`], checked an hour before NOW.
fn stored_target(catalog: &CompiledCatalog) -> ObservedFact {
    let document = target_document();
    let parsed = parse_resource_targets(&serde_json::to_vec(&document).expect("bytes"))
        .expect("the document parses");
    let checked = check_resource_targets(&parsed, catalog, &facts(Vec::new()), time(NOW - HOUR_MS))
        .expect("the document is accepted");
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: RESOURCE_TARGETS_FACT_KEY.to_owned(),
        value: FactValue::RecordList(
            serde_json::from_value(serde_json::to_value(&checked.rows).expect("rows"))
                .expect("record list"),
        ),
        observed_at_unix_ms: NOW - HOUR_MS,
        expires_at_unix_ms: document["valid_until_unix_ms"].as_u64(),
        confidence_milli: 1000,
    }
}

// ---- The evaluation

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

fn host() -> HostResourceSnapshot {
    HostResourceSnapshot {
        host_id: instance(A, &[]).host_id,
        cpu_available_milli: 1_000,
        gpu_available_milli: 1_000,
        io_available_milli: 1_000,
        host_responsiveness_basis_points: 10_000,
        third_party_pressure_basis_points: 0,
        heavy_dispatch_limit: 1,
        active_heavy_dispatches: 0,
        heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
    }
}

fn facts(tasks: Vec<TaskRuntimeSnapshot>) -> EvaluationFacts {
    facts_with(Vec::new(), tasks)
}

fn facts_with(facts: Vec<ObservedFact>, tasks: Vec<TaskRuntimeSnapshot>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-455-p2".to_owned(),
        facts,
        outcomes: Vec::new(),
        tasks,
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

/// `task_id`'s latest run on A, dispatched at `at`, succeeded.
fn ran(task_id: &str, at: u64) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: task_id.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(at),
        eligible_since_unix_ms: None,
        terminal_state: Some(TaskTerminalState::Succeeded),
        completed_window: None,
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
        clock_owed: false,
    }
}

/// The resources the host hands the evaluation: the pools' values projected with `readings`.
fn resources(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    readings: &GivenPoolReadings,
) -> EvaluationResources {
    project_fact_pools_with_readings(
        catalog,
        facts,
        &EvaluationResources {
            pools: Vec::new(),
            hosts: vec![host()],
        },
        readings,
    )
}

/// One round at `at` with `readings` given beside the left-out facts, as the host gives them.
fn round(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    readings: &GivenPoolReadings,
    at: u64,
) -> PolicyEvaluation {
    let resources = resources(catalog, facts, readings);
    let mut left_out = LeftOutFacts::default();
    left_out.give_pool_readings(readings.clone());
    round_with(catalog, facts, &resources, &left_out, at)
}

/// One round at `at`, the fact refresh at its starting defaults with no reader, every candidate
/// admitted.
fn round_with(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    left_out: &LeftOutFacts,
    at: u64,
) -> PolicyEvaluation {
    guarded_round(
        catalog,
        facts,
        resources,
        left_out,
        &NoProgressGuards::default(),
        at,
    )
}

/// Test 13's inputs of a reading of `value` read at `read_at`, with the task states `tasks`: with
/// `listed`, the reading of [`LISTED`] on A as the host gives it; else A's fact [`COUNTED`].
fn guard_inputs(
    listed: bool,
    value: u64,
    read_at: u64,
    tasks: Vec<TaskRuntimeSnapshot>,
) -> (EvaluationFacts, GivenPoolReadings) {
    if listed {
        let state = GivenReadingState::Read {
            value,
            observed_at_unix_ms: read_at,
            expires_at_unix_ms: Some(NOW + HOUR_MS),
        };
        return (facts(tasks), readings(state));
    }
    let fact = ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: COUNTED.to_owned(),
        value: FactValue::Integer(i64::try_from(value).expect("a small reading")),
        observed_at_unix_ms: read_at,
        expires_at_unix_ms: Some(NOW + HOUR_MS),
        confidence_milli: 1000,
    };
    (facts_with(vec![fact], tasks), GivenPoolReadings::default())
}

/// One evaluated cycle at `at` as the host runs it with the guard on for A: the memory observes
/// the facts and `readings`, the round evaluates with the armed guards and `readings` beside the
/// left-out facts, the memory forgets the guards the round released and captures the basis of
/// each new intent with `readings`.
fn guarded_cycle(
    catalog: &CompiledCatalog,
    memory: &mut NoProgressMemory,
    facts: &EvaluationFacts,
    readings: &GivenPoolReadings,
    at: u64,
) -> PolicyEvaluation {
    memory
        .observe_with_readings(catalog, facts, readings, time(at), |alias| alias == A)
        .unwrap_or_else(|error| panic!("the memory observes: {error}"));
    let guards = memory.guards();
    let resources = resources(catalog, facts, readings);
    let mut left_out = LeftOutFacts::default();
    left_out.give_pool_readings(readings.clone());
    let round = guarded_round(catalog, facts, &resources, &left_out, &guards, at);
    memory.forget_released(&guards);
    memory
        .capture_with_readings(
            catalog,
            facts,
            &resources,
            readings,
            time(at),
            &round.dispatch_intents,
            |alias| alias == A,
        )
        .unwrap_or_else(|error| panic!("the memory captures: {error}"));
    round
}

/// [`round_with`] with the no-progress guards `guards`.
fn guarded_round(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
    left_out: &LeftOutFacts,
    guards: &NoProgressGuards,
    at: u64,
) -> PolicyEvaluation {
    let refresh = FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: true,
            min_interval_ms: 300_000,
            daily_limit: 96,
            failure_backoff_ms: 600_000,
            max_consecutive_failures: 3,
            expected_duration_ms: crate::FACT_REFRESH_EXPECTED_DURATION_MS,
        },
        readers: Vec::new(),
        instance_facts_loaded_at_unix_ms: Some(NOW - DAY_MS),
        instance_holds: BTreeMap::new(),
        idle_closed: BTreeMap::new(),
    };
    evaluate_with_left_out_facts::<PolicyEvaluationError>(
        catalog,
        facts,
        resources,
        time(at),
        11,
        &BTreeSet::new(),
        &refresh,
        &RemedyPlan::default(),
        &ChainHandoverInputs::default(),
        &RunSlots::default(),
        &OwedReruns::default(),
        &InstanceHolds::default(),
        guards,
        &mut CappedMemory::default(),
        &InstancePoolLines::default(),
        left_out,
        |_, _, _| Ok(false),
        |_: &DispatchIntent| Ok(CandidateEligibility::Eligible),
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

fn dispatched(evaluation: &PolicyEvaluation) -> Vec<&str> {
    evaluation
        .dispatch_intents
        .iter()
        .filter(|intent| intent.instance_id == A)
        .map(|intent| intent.task_id.as_str())
        .collect()
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == task_id && decision.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}: {evaluation:#?}"))
}

fn codes(decision: &TaskDecision) -> Vec<&str> {
    decision
        .reasons
        .iter()
        .map(|reason| reason.outcome.code().as_str())
        .collect()
}

fn last(decision: &TaskDecision) -> &Outcome {
    &decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {decision:#?}"))
        .outcome
}

/// The outcome of `decision`'s first reason of `code`.
fn coded<'a>(decision: &'a TaskDecision, code: &str) -> &'a Outcome {
    &decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == code)
        .unwrap_or_else(|| panic!("a {code} reason on {decision:#?}"))
        .outcome
}

/// Whether `decision` suggests reading the list's key in A's scope for `reason`.
fn suggests(decision: &TaskDecision, reason: &str) -> bool {
    let scope = ScopeSelector::Instance {
        instance_id: A.to_owned(),
    };
    decision.detection_suggestions.iter().any(|suggestion| {
        suggestion.fact_key == LIST_KEY && suggestion.reason == reason && suggestion.scope == scope
    })
}

/// The forward projection's gap of the list's key for the task, with `code`.
fn gap(code: &str) -> ForwardEvidenceGap {
    ForwardEvidenceGap {
        code: code.to_owned(),
        task_id: Some(TASK.to_owned()),
        fact_key: Some(LIST_KEY.to_owned()),
    }
}
