// SPDX-License-Identifier: AGPL-3.0-only

// #486 ASLEEP-1 (0.12.77; register §1.497; `contracts/scheduling/README.md`, "An asleep
// instance"), the policy half: an instance the Runtime names asleep (`InstanceHolds::mark_asleep`:
// its emulator stands stopped by its idle action, and nothing but that stop withdrew its
// availability) is judged, while its snapshot holds it unavailable, as if it were available, and
// nothing is selected on it.
//
// - T1: with no asleep instance (none named, one named whose snapshot holds it available, one
//   named that the round does not know) the evaluation equals the one of the entry point without
//   the input, byte for byte and with every decision's wake, and so does an unavailable instance
//   no one names (T4's case).
// - T2: an asleep instance, one candidate past its trigger with every gate open: it is deferred
//   with `instance_asleep_wake_due` (its alias) and the evaluation's time as its wake, not
//   selected, no intent and no reason chain; the eligible candidate behind it carries exactly the
//   decision it carries behind the same candidate selected on the instance available.
// - T3: the same instance with the candidate's budget spent (the admission's deferral), with its
//   benefit unknown and with a pool line holding it: each time the decision the instance
//   available gets, code and wake, and the admission is asked about it as on an available one.
// - T4: an unavailable instance that is not asleep is blocked at placement as before.
//
// The catalog is neutral and V2: tasks of the fixture game on an hourly clock due at NOW, a valued
// pool and a capped pool of the instance read from their keys (`gate_ss2h`'s shape). Every expected
// value comes from the work order's rulings 1 to 4, never from what the code does; nothing reads a
// clock.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CandidateEligibility, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DecisionReason, DispatchIntent, EligibilityState, EvaluationFacts,
    EvaluationResources, EvaluationTime, FactRefreshInputs, FactRefreshSettings,
    HostResourceSnapshot, InstanceHolds, InstanceSnapshot, OwedReruns, PolicyCode,
    PolicyEvaluation, PolicyEvaluationError, RemedyPlan, RunSlots, SchedulingDecisionState,
    TaskDecision, compile_catalog, evaluate_with_instance_holds, evaluate_with_owed_reruns,
    project_fact_pools,
};
use actingcommand_contract::outcome::Outcome;

const A: &str = "instance-a";
const B: &str = "instance-b";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// A maintenance task of priority 300: the round's first candidate.
const DUE: &str = "task_due";
/// A maintenance task of priority 100: the candidate behind it.
const BEHIND: &str = "task_behind";
/// A judged task that declares no effect: its net benefit is unknown.
const UNKNOWN: &str = "task_unknown";
/// A judged consumer of the capped pool below its cap, whose run would cross its hard line.
const LINED: &str = "task_lined";
/// The capped pool and the facts its reading and capacity are read from (`gate_ss2h`'s).
const POOL: &str = "pool.neutral";
const READING_KEY: &str = "resource.neutral";
const CAPACITY_KEY: &str = "resource.neutral.capacity";
const OUT: &str = "pool.out";
/// The admission's code of a spent budget (registered by runtime-host), and the release it names.
const BUDGET: &str = "policy_budget_exhausted";
const RELEASE: u64 = NOW + 5 * HOUR_MS;

// ---- T1

#[test]
fn asleep_t1_with_no_asleep_instance_the_evaluation_is_the_one_without_the_input() {
    let catalog = catalog(vec![maintenance(DUE, 300), maintenance(BEHIND, 100)]);
    for (case, a_available) in [("A available", true), ("A unavailable", false)] {
        let instances = [snapshot(A, a_available), snapshot(B, true)];
        let plain = round_without_holds(&catalog, &instances, &eligible);
        let mut available = InstanceHolds::default();
        available.mark_asleep(B);
        let mut unknown = InstanceHolds::default();
        unknown.mark_asleep("instance-unknown");
        for (holds_case, holds) in [
            ("no instance named", &InstanceHolds::default()),
            ("an available instance named", &available),
            ("an instance the round does not know named", &unknown),
        ] {
            let asleep = round(&catalog, &instances, holds, &eligible);
            assert_eq!(bytes(&asleep), bytes(&plain), "{case}, {holds_case}");
            assert_eq!(
                asleep, plain,
                "{case}, {holds_case}: decision for decision, every wake too"
            );
        }
    }
}

// ---- T2

#[test]
fn asleep_t2_the_candidate_the_round_would_select_waits_as_the_task_it_is_woken_for() {
    let catalog = catalog(vec![maintenance(DUE, 300), maintenance(BEHIND, 100)]);
    let awake = round_without_holds(&catalog, &[snapshot(A, true)], &eligible);
    assert_eq!(dispatched(&awake), [DUE], "{awake:#?}");
    let asleep = round(&catalog, &[snapshot(A, false)], &asleep_a(), &eligible);

    // Not selected: no intent, no reason chain.
    assert!(asleep.dispatch_intents.is_empty(), "{asleep:#?}");
    assert!(asleep.reason_chains.is_empty(), "{asleep:#?}");
    let woken_for = decision(&asleep, DUE);
    assert_eq!(woken_for.state, SchedulingDecisionState::Deferred);
    assert_eq!(woken_for.eligibility, EligibilityState::True);
    let last = woken_for.reasons.last().expect("a reason");
    assert!(
        last.outcome.is(PolicyCode::InstanceAsleepWakeDue),
        "{woken_for:#?}"
    );
    assert_eq!(last.outcome.value("instance_id"), Some(&json!(A)));
    assert_eq!(
        woken_for.next_wake_unix_ms,
        Some(NOW),
        "its wake is the evaluation's time"
    );
    assert!(
        !woken_for.reasons.iter().any(|reason| {
            reason.outcome.is(PolicyCode::PlacementSelected)
                || reason.outcome.is(PolicyCode::RankAssigned)
                || reason.outcome.is(PolicyCode::DecisionRecord)
        }),
        "no reason claims a selection: {woken_for:#?}"
    );
    assert_eq!(
        asleep.next_wake_unix_ms,
        Some(NOW),
        "the evaluation wakes now"
    );

    // The candidate behind it: what it is behind the same candidate selected.
    let behind = decision(&asleep, BEHIND);
    assert!(
        behind
            .reasons
            .last()
            .is_some_and(|reason| reason.outcome.is(PolicyCode::InstanceAlreadySelected)),
        "{behind:#?}"
    );
    assert_eq!(behind, decision(&awake, BEHIND), "as behind a selected one");
}

// ---- T3

#[test]
fn asleep_t3_a_gate_after_placement_decides_as_on_an_available_instance() {
    // A spent budget: the admission defers the task to its release.
    let budget = catalog(vec![maintenance(DUE, 300)]);
    let spent = |intent: &DispatchIntent| CandidateEligibility::Deferred {
        reason: DecisionReason {
            outcome: Outcome::relayed(BUDGET),
            detail: format!("fixture admission: {} spent", intent.task_id),
        },
        next_wake_unix_ms: Some(RELEASE),
    };
    // Its benefit unknown: a judged task that declares no effect.
    let unknown = catalog(vec![judged_without_effects(UNKNOWN)]);
    // A pool line: read at 120 of a cap of 180, its run of 30 would leave 90, below the hard line
    // 104.
    let lined = catalog(vec![judged_consumer(LINED, 30, 40)]);
    for (case, catalog, task, answer, code) in [
        (
            "a spent budget",
            &budget,
            DUE,
            &spent as &dyn Fn(&DispatchIntent) -> CandidateEligibility,
            BUDGET,
        ),
        (
            "an unknown benefit",
            &unknown,
            UNKNOWN,
            &eligible as &dyn Fn(&DispatchIntent) -> CandidateEligibility,
            PolicyCode::BenefitUnknown.as_str(),
        ),
        (
            "a pool line",
            &lined,
            LINED,
            &eligible as &dyn Fn(&DispatchIntent) -> CandidateEligibility,
            PolicyCode::PoolLineHeld.as_str(),
        ),
    ] {
        let awake = round_without_holds(catalog, &[snapshot(A, true)], answer);
        let asleep = round(catalog, &[snapshot(A, false)], &asleep_a(), answer);
        let held = decision(&asleep, task);
        assert_eq!(
            held,
            decision(&awake, task),
            "{case}: the available decision, code and wake"
        );
        assert_eq!(held.state, SchedulingDecisionState::Deferred, "{case}");
        assert_eq!(
            held.reasons
                .last()
                .map(|reason| reason.outcome.code().as_str()),
            Some(code),
            "{case}: {held:#?}"
        );
        if case == "a spent budget" {
            assert_eq!(held.next_wake_unix_ms, Some(RELEASE), "{case}: the release");
        }
        if case == "a pool line" {
            assert_eq!(
                held.reasons
                    .last()
                    .and_then(|reason| reason.outcome.value("pool_line_hold")),
                Some(&json!("would_cross_hard_line")),
                "{case}"
            );
        }
        assert!(
            asleep.dispatch_intents.is_empty() && awake.dispatch_intents.is_empty(),
            "{case}: nothing is dispatched"
        );
        assert!(
            !held
                .reasons
                .iter()
                .any(|reason| reason.outcome.is(PolicyCode::InstanceUnavailable)
                    || reason.outcome.is(PolicyCode::InstanceAsleepWakeDue)),
            "{case}: {held:#?}"
        );
    }
}

// ---- T4

#[test]
fn asleep_t4_an_unavailable_instance_that_is_not_asleep_is_blocked_as_before() {
    let catalog = catalog(vec![maintenance(DUE, 300)]);
    let instances = [snapshot(A, false), snapshot(B, false)];
    let plain = round_without_holds(&catalog, &instances, &eligible);
    let mut b_asleep = InstanceHolds::default();
    b_asleep.mark_asleep(B);
    let judged = round(&catalog, &instances, &b_asleep, &eligible);
    let blocked = decision(&judged, DUE);
    assert_eq!(blocked.state, SchedulingDecisionState::Blocked);
    assert_eq!(blocked.eligibility, EligibilityState::True);
    assert!(
        blocked
            .reasons
            .last()
            .is_some_and(|reason| reason.outcome.is(PolicyCode::InstanceUnavailable)),
        "{blocked:#?}"
    );
    assert_eq!(blocked, decision(&plain, DUE), "A is blocked as before");
    let woken_for = decision_on(&judged, DUE, B);
    assert!(
        woken_for
            .reasons
            .last()
            .is_some_and(|reason| reason.outcome.is(PolicyCode::InstanceAsleepWakeDue)),
        "B, asleep, is woken for its task: {woken_for:#?}"
    );
}

// ---- The fixture

/// A named asleep.
fn asleep_a() -> InstanceHolds {
    let mut holds = InstanceHolds::default();
    holds.mark_asleep(A);
    holds
}

/// The fixture game's instance `instance_id`, available or not.
fn snapshot(instance_id: &str, available: bool) -> InstanceSnapshot {
    InstanceSnapshot {
        available,
        ..instance(instance_id, &[])
    }
}

/// The admission without a gate.
fn eligible(_: &DispatchIntent) -> CandidateEligibility {
    CandidateEligibility::Eligible
}

/// The facts of one round: the capped pool's reading (120, a minute old) and capacity (180) on
/// every instance.
fn facts(instances: &[InstanceSnapshot]) -> EvaluationFacts {
    let mut facts = Vec::new();
    for instance in instances {
        facts.push(fact(
            &instance.instance_id,
            READING_KEY,
            120,
            NOW - MINUTE_MS,
            None,
        ));
        facts.push(fact(
            &instance.instance_id,
            CAPACITY_KEY,
            180,
            NOW - HOUR_MS,
            None,
        ));
    }
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-asleep".to_owned(),
        facts,
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: instances.to_vec(),
        priority_offsets: Vec::new(),
    }
}

fn resources(catalog: &CompiledCatalog, facts: &EvaluationFacts) -> EvaluationResources {
    project_fact_pools(
        catalog,
        facts,
        &EvaluationResources {
            pools: Vec::new(),
            hosts: facts
                .instances
                .iter()
                .map(|instance| HostResourceSnapshot {
                    host_id: instance.host_id.clone(),
                    cpu_available_milli: 1_000,
                    gpu_available_milli: 1_000,
                    io_available_milli: 1_000,
                    host_responsiveness_basis_points: 10_000,
                    third_party_pressure_basis_points: 0,
                    heavy_dispatch_limit: 1,
                    active_heavy_dispatches: 0,
                    heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
                })
                .collect(),
        },
    )
}

/// The fact refresh off: no reader is bound.
fn refresh_inputs() -> FactRefreshInputs {
    FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: false,
            min_interval_ms: 300_000,
            daily_limit: 96,
            failure_backoff_ms: 600_000,
            max_consecutive_failures: 3,
            expected_duration_ms: crate::FACT_REFRESH_EXPECTED_DURATION_MS,
        },
        readers: Vec::new(),
        instance_facts_loaded_at_unix_ms: Some(NOW - DAY_MS),
        instance_holds: std::collections::BTreeMap::new(),
        idle_closed: std::collections::BTreeMap::new(),
    }
}

fn time() -> EvaluationTime {
    EvaluationTime {
        unix_ms: NOW,
        monotonic_ms: NOW,
    }
}

/// One round at NOW with the holds (and the instances asleep) `holds`.
fn round(
    catalog: &CompiledCatalog,
    instances: &[InstanceSnapshot],
    holds: &InstanceHolds,
    answer: &dyn Fn(&DispatchIntent) -> CandidateEligibility,
) -> PolicyEvaluation {
    let facts = facts(instances);
    evaluate_with_instance_holds::<PolicyEvaluationError>(
        catalog,
        &facts,
        &resources(catalog, &facts),
        time(),
        11,
        &BTreeSet::new(),
        &refresh_inputs(),
        &RemedyPlan::default(),
        &ChainHandoverInputs::default(),
        &RunSlots::default(),
        &OwedReruns::default(),
        holds,
        |_, _, _| Ok(false),
        |intent: &DispatchIntent| Ok(answer(intent)),
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

/// The same round by the entry point without the input (`evaluate_with_owed_reruns`).
fn round_without_holds(
    catalog: &CompiledCatalog,
    instances: &[InstanceSnapshot],
    answer: &dyn Fn(&DispatchIntent) -> CandidateEligibility,
) -> PolicyEvaluation {
    let facts = facts(instances);
    evaluate_with_owed_reruns::<PolicyEvaluationError>(
        catalog,
        &facts,
        &resources(catalog, &facts),
        time(),
        11,
        &BTreeSet::new(),
        &refresh_inputs(),
        &RemedyPlan::default(),
        &ChainHandoverInputs::default(),
        &RunSlots::default(),
        &OwedReruns::default(),
        |_, _, _| Ok(false),
        |intent: &DispatchIntent| Ok(answer(intent)),
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

fn bytes(evaluation: &PolicyEvaluation) -> Vec<u8> {
    serde_json::to_vec(evaluation).expect("the evaluation serializes")
}

fn dispatched(evaluation: &PolicyEvaluation) -> Vec<&str> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| intent.task_id.as_str())
        .collect()
}

/// The decision of `task_id` on A.
fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    decision_on(evaluation, task_id, A)
}

fn decision_on<'a>(
    evaluation: &'a PolicyEvaluation,
    task_id: &str,
    instance_id: &str,
) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(instance_id)
        })
        .unwrap_or_else(|| panic!("a decision of {task_id} on {instance_id}: {evaluation:#?}"))
}

// ---- The catalog

fn hourly() -> Value {
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

/// A task of the game on its hourly clock, of `priority`, unmarked (`gate_ss2h`'s).
fn maintenance(id: &str, priority: i16) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
        "priority": priority,
        "trigger": hourly(),
        "feedback_stop": {
            "kind": "outcome",
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
        "produces": [],
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

/// A judged task of priority 100 that declares no effect.
fn judged_without_effects(id: &str) -> Value {
    let mut task = maintenance(id, 100);
    task["benefit_judged"] = json!(true);
    task
}

fn effect(pool: &str, direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// A judged consumer of the capped pool (`gate_ss2h`'s `judged`): `consumed` of it a run,
/// `produced` units of the valued pool, due while the capped pool holds its consumption.
fn judged_consumer(id: &str, consumed: u64, produced: u64) -> Value {
    let mut task = judged_without_effects(id);
    task["consumes"] = json!([effect(POOL, "consume", consumed)]);
    task["produces"] = json!([effect(OUT, "produce", produced)]);
    task["trigger"] = json!({
        "kind": "resource_projection",
        "pool_id": POOL,
        "comparison": "greater_than_or_equal",
        "value": consumed
    });
    task
}

/// A pool of A that does not regenerate, read from `resource.<the id without "pool.">`, weighed
/// 1000 milli a unit.
fn valued(id: &str) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": 1_000_000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000}
    })
}

/// The capacity fact plus `offset`.
fn line(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

/// The valued pool and the capped pool: its capacity read from its fact, its stop line 40 below
/// the cap and its hard line 76 below it.
fn pools() -> Value {
    let mut capped = valued(POOL);
    capped["capacity"] = line(0);
    capped["stop_line"] = line(-40);
    capped["hard_line"] = line(-76);
    json!([valued(OUT), capped])
}

fn catalog(tasks: Vec<Value>) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.asleep",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-asleep"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
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
    let source = |name: &str, document: &Value| {
        CatalogDocumentSource::new(
            format!("memory://fixture-asleep/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", pools())),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}
