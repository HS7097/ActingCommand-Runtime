// SPDX-License-Identifier: AGPL-3.0-only

// UPD-1 fix round 5 (review `rd-1029/review-upd1-r3.md` L3; register §1.215 UPD-13;
// `contracts/scheduling-blackout.md`, "Order of the deferrals", position 4a): the evaluator's
// whole-instance holds (`InstanceHolds`, `evaluate_with_instance_holds`), which the Runtime passes
// for the instances that wait for their client update, the store update tasks exempt.
//
// - T1: a hold that holds nothing changes nothing. With no hold, with a hold of an instance the
//   round does not know, and with a hold whose exempt tasks are every task of the round, the
//   evaluation equals `evaluate_with_owed_reruns`'s byte for byte, in a round that settles
//   candidates behind their instance's selected candidate and in one that settles them behind the
//   shared run limit.
// - T2: A is held, its store update task S exempt, and the eligibility closure answers position 4a
//   for every other candidate of A it is asked about. A candidate of A that the round settles
//   before it asks the closure (behind S that took A, behind the shared run limit, and the fact
//   refresh behind S) is deferred `application_update_pending` with no wake, and the candidate
//   waiting for the fact refresh names that code as its gate; S keeps the shared limit's own code;
//   B's decisions, intents and reason chains are those of the round without the hold (#419); and a
//   candidate the closure is asked about keeps the closure's answer (position 1 before 4a).
//
// Two instances, each on a host of its own. On A: S (priority 900), the ordinary task T (100, or
// 1000 to rank first) and the task X, whose trigger reads A's realistic key K, stale, so the fact
// refresh R_I is requested for it with X's rank (500). On B: the task U (300, or 1000 to rank
// first). Every expected value comes from the review's L3 and the contract, never from what the
// code does; nothing reads a clock.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::{Code, Outcome};
use serde_json::{Map, Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CandidateEligibility, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DecisionReason, DecisionReasonChain, DispatchIntent, EvaluationFacts,
    EvaluationResources, EvaluationTime, FACT_REFRESH_TASK_ID, FactRefreshInputs,
    FactRefreshReader, FactRefreshSettings, HostResourceSnapshot, InstanceHolds, OwedReruns,
    PolicyEvaluation, PolicyEvaluationError, RemedyPlan, RunSlots, SchedulingDecisionState,
    TaskDecision, compile_catalog, evaluate_with_instance_holds, evaluate_with_owed_reruns,
};

const A: &str = "instance-a";
const B: &str = "instance-b";
/// A's store update task, exempt from A's hold.
const S: &str = "task.store";
/// A's ordinary task.
const T: &str = "task.ordinary";
/// A's task whose trigger reads K.
const X: &str = "task.reads";
/// B's task.
const U: &str = "task.other";
/// A realistic key of A (one that recognition writes), stale at NOW.
const K: &str = "resource.stock";
/// A's recognition maintenance package; its marks write K.
const READER: &str = "package.reader-a";
/// The Runtime's application update fact, and position 4a's code (registered by runtime-host).
const UPDATE_KEY: &str = "health.application.update_required";
const PENDING: &str = "application_update_pending";
/// The closure's position 1 (registered by runtime-host).
const PAUSED: &str = "dispatch_paused_instance";
const ALREADY_SELECTED: &str = "instance_already_selected";
const CONCURRENT_RUNS: &str = "policy_concurrent_runs_deferred";
const REFRESH_PENDING: &str = "fact_refresh_pending";

// ---- T1

// A hold that holds nothing gives the evaluation of the entry point without holds, byte for byte
// and with every decision's wake (review L3 T1: (a) no hold, (b) a hold of an instance the round
// does not know, (c) a hold of A whose exempt tasks are every task of the round). Each round first
// shows it reaches the settlement it is about: T and R_I wait behind S, or behind U's one run.
#[test]
fn upd1_holds_t1_a_hold_that_holds_nothing_changes_no_byte() {
    let mut unknown = InstanceHolds::default();
    unknown.hold(
        "instance-unknown",
        hold_reason(),
        pending_code(),
        BTreeSet::new(),
    );
    let every_task = held_a(&[S, T, X, U, FACT_REFRESH_TASK_ID]);
    for (round_case, u_priority, slots, settled) in [
        (
            "behind the selected candidate",
            300,
            RunSlots::default(),
            ALREADY_SELECTED,
        ),
        (
            "behind the shared run limit",
            1000,
            one_run(),
            CONCURRENT_RUNS,
        ),
    ] {
        let plain = round(100, u_priority, &slots, None, eligible);
        assert_eq!(
            last_code(decision(&plain, T, A)),
            settled,
            "{round_case}: {plain:#?}"
        );
        assert_eq!(
            last_code(refresh_decision(&plain)),
            settled,
            "{round_case}: {plain:#?}"
        );
        for (case, holds) in [
            ("no hold", &InstanceHolds::default()),
            ("an instance the round does not know", &unknown),
            ("every task exempt", &every_task),
        ] {
            let held = round(100, u_priority, &slots, Some(holds), eligible);
            assert_eq!(bytes(&held), bytes(&plain), "{round_case}, {case}");
            assert_eq!(
                held, plain,
                "{round_case}, {case}: every decision's wake too"
            );
        }
    }
}

// ---- T2

// The selected-instance site and the fact refresh's gate: S ranks first and takes A, so T and R_I
// are settled behind it without the closure. Each is deferred `application_update_pending` (the
// instance and the key) with no wake, never `instance_already_selected`; X, which waits for R_I,
// carries `fact_refresh_pending` with that code as its gate and no wake. B is the round's without
// the hold.
#[test]
fn upd1_holds_t2_behind_the_selected_store_task_the_hold_defers_and_spares_b() {
    let holds = held_a(&[S]);
    let plain = round(100, 300, &RunSlots::default(), None, gated);
    let held = round(100, 300, &RunSlots::default(), Some(&holds), gated);
    assert_eq!(dispatched(&held, A), vec![S], "{held:#?}");

    let ordinary = decision(&held, T, A);
    assert_held(ordinary, "T behind S");
    assert!(!has(ordinary, ALREADY_SELECTED), "{:?}", ordinary.reasons);
    let refresh = refresh_decision(&held);
    assert_held(refresh, "R_I behind S");
    assert!(!has(refresh, ALREADY_SELECTED), "{:?}", refresh.reasons);

    let reads = decision(&held, X, A);
    let pending = coded(reads, REFRESH_PENDING);
    assert_eq!(pending.len(), 1, "{:?}", reads.reasons);
    assert_values(
        pending[0],
        &[
            ("instance_id", json!(A)),
            ("fact_key", json!(K)),
            ("unknown_reason", json!("fact_stale")),
            ("package_id", json!(READER)),
            ("gate_code", json!(PENDING)),
        ],
    );
    assert_eq!(reads.next_wake_unix_ms, None, "no wake for X");

    assert_eq!(dispatched(&held, B), vec![U], "{held:#?}");
    assert_eq!(on_b(&held), on_b(&plain), "B is untouched (#419)");
}

// The run-limit site: one run at a time, and U ranks first and takes it. T and R_I of A are
// deferred `application_update_pending` with no wake in place of `policy_concurrent_runs_deferred`;
// S, exempt, keeps that code. B is the round's without the hold.
#[test]
fn upd1_holds_t2_behind_the_shared_run_limit_only_the_exempt_task_keeps_the_limit_code() {
    let holds = held_a(&[S]);
    let plain = round(100, 1000, &one_run(), None, gated);
    let held = round(100, 1000, &one_run(), Some(&holds), gated);
    assert_eq!(dispatched(&held, B), vec![U], "{held:#?}");
    assert!(dispatched(&held, A).is_empty(), "{held:#?}");

    let ordinary = decision(&held, T, A);
    assert_held(ordinary, "T behind the limit");
    assert!(!has(ordinary, CONCURRENT_RUNS), "{:?}", ordinary.reasons);
    let refresh = refresh_decision(&held);
    assert_held(refresh, "R_I behind the limit");
    assert!(!has(refresh, CONCURRENT_RUNS), "{:?}", refresh.reasons);

    let store = decision(&held, S, A);
    assert_eq!(last_code(store), CONCURRENT_RUNS, "{:?}", store.reasons);
    assert!(!has(store, PENDING), "S is exempt: {:?}", store.reasons);

    assert_eq!(on_b(&held), on_b(&plain), "B is untouched (#419)");
}

// Positions 1 to 4 first: T ranks ahead of S, so the closure is asked about T and answers its
// position 1 (the instance's operator pause). T's last reason is that code, and the hold adds
// nothing to it; S is still the instance's dispatch, and R_I behind it is held.
#[test]
fn upd1_holds_t2_a_candidate_the_closure_is_asked_about_keeps_positions_1_to_4() {
    let holds = held_a(&[S]);
    let held = round(1000, 300, &RunSlots::default(), Some(&holds), paused_first);
    let ordinary = decision(&held, T, A);
    assert_eq!(ordinary.state, SchedulingDecisionState::Deferred);
    assert_eq!(last_code(ordinary), PAUSED, "{:?}", ordinary.reasons);
    assert!(!has(ordinary, PENDING), "{:?}", ordinary.reasons);
    assert_eq!(dispatched(&held, A), vec![S], "{held:#?}");
    assert_held(refresh_decision(&held), "R_I behind S");
}

// ---- The fixture

/// One round at [`NOW`] of [`catalog`] with these priorities of T and U, under `slots`, the holds
/// `holds` (`None`: `evaluate_with_owed_reruns`, the entry point without holds) and the closure
/// `answer`.
fn round(
    t_priority: i16,
    u_priority: i16,
    slots: &RunSlots,
    holds: Option<&InstanceHolds>,
    answer: fn(&DispatchIntent) -> CandidateEligibility,
) -> PolicyEvaluation {
    let catalog = catalog(t_priority, u_priority);
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-upd1-holds".to_owned(),
        facts: vec![fact(A, K, 5, NOW - 2 * HOUR_MS, None)],
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[]), instance(B, &[])],
        priority_offsets: Vec::new(),
    };
    let resources = EvaluationResources {
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
    };
    let time = EvaluationTime {
        unix_ms: NOW,
        monotonic_ms: NOW,
    };
    let refresh = refresh_inputs();
    let evaluated = match holds {
        None => evaluate_with_owed_reruns::<PolicyEvaluationError>(
            &catalog,
            &facts,
            &resources,
            time,
            11,
            &BTreeSet::new(),
            &refresh,
            &RemedyPlan::default(),
            &ChainHandoverInputs::default(),
            slots,
            &OwedReruns::default(),
            |_, _, _| Ok(false),
            |intent: &DispatchIntent| Ok(answer(intent)),
        ),
        Some(holds) => evaluate_with_instance_holds::<PolicyEvaluationError>(
            &catalog,
            &facts,
            &resources,
            time,
            11,
            &BTreeSet::new(),
            &refresh,
            &RemedyPlan::default(),
            &ChainHandoverInputs::default(),
            slots,
            &OwedReruns::default(),
            holds,
            |_, _, _| Ok(false),
            |intent: &DispatchIntent| Ok(answer(intent)),
        ),
    };
    evaluated.unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

/// The shared run limit at one run at a time, none admitted before the round.
fn one_run() -> RunSlots {
    RunSlots {
        admitted_load_profiles: Vec::new(),
        max_concurrent_runs: Some(1),
    }
}

/// The fact refresh at its starting defaults, A's reader marking K, the facts loaded a day ago.
fn refresh_inputs() -> FactRefreshInputs {
    FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: true,
            min_interval_ms: 300_000,
            daily_limit: 96,
            failure_backoff_ms: 600_000,
            max_consecutive_failures: 3,
            expected_duration_ms: crate::FACT_REFRESH_EXPECTED_DURATION_MS,
        },
        readers: vec![FactRefreshReader {
            instance_id: A.to_owned(),
            package_id: READER.to_owned(),
            marked_keys: BTreeSet::from([K.to_owned()]),
            dispatched_today: 0,
            last_failure_code: None,
            problem: None,
        }],
        instance_facts_loaded_at_unix_ms: Some(NOW - DAY_MS),
        instance_holds: BTreeMap::new(),
        idle_closed: BTreeMap::new(),
    }
}

/// Position 4a's code.
fn pending_code() -> Code {
    Code::relayed(PENDING).expect("a registered code")
}

/// Position 4a's reason on A: the instance and the key, as the Runtime writes it.
fn hold_reason() -> DecisionReason {
    DecisionReason {
        outcome: Outcome::new(pending_code())
            .instance_id(A)
            .fact_key(UPDATE_KEY),
        detail: "the instance waits for its client update".to_owned(),
    }
}

/// A held with position 4a's reason and gate, `exempt` its exempt tasks.
fn held_a(exempt: &[&str]) -> InstanceHolds {
    let mut holds = InstanceHolds::default();
    holds.hold(
        A,
        hold_reason(),
        pending_code(),
        exempt.iter().map(|task| (*task).to_owned()).collect(),
    );
    holds
}

/// The closure without a gate: every candidate it is asked about is eligible.
fn eligible(_: &DispatchIntent) -> CandidateEligibility {
    CandidateEligibility::Eligible
}

/// The closure as the Runtime's position 4a answers while A waits: every candidate of A but S is
/// deferred with no wake (the fact refresh's naming the gate), every other candidate is eligible.
fn gated(intent: &DispatchIntent) -> CandidateEligibility {
    if intent.instance_id != A || intent.task_id == S {
        return CandidateEligibility::Eligible;
    }
    if intent.task_id == FACT_REFRESH_TASK_ID {
        CandidateEligibility::Refresh {
            reason: hold_reason(),
            next_wake_unix_ms: None,
            gate: pending_code(),
            limit: None,
        }
    } else {
        CandidateEligibility::Deferred {
            reason: hold_reason(),
            next_wake_unix_ms: None,
        }
    }
}

/// [`gated`], with A's operator pause (position 1) answered first for T.
fn paused_first(intent: &DispatchIntent) -> CandidateEligibility {
    if intent.instance_id == A && intent.task_id == T {
        return CandidateEligibility::Deferred {
            reason: DecisionReason {
                outcome: Outcome::relayed(PAUSED),
                detail: "the operator paused the instance".to_owned(),
            },
            next_wake_unix_ms: None,
        };
    }
    gated(intent)
}

/// The catalog: S, T and X on A and U on B, with these priorities of T and U; S, T and U hourly,
/// due at [`NOW`].
fn catalog(t_priority: i16, u_priority: i16) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.upd1-holds",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-upd1-holds"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let hourly = || {
        json!({
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": HOUR_MS,
                "anchor_ms": 0
            }
        })
    };
    let reads_k = json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": K,
        "comparison": "less_than",
        "value": {"type": "integer", "value": 20},
        "max_age_ms": 900000
    });
    let tasks = json!([
        task(S, A, 900, hourly()),
        task(T, A, t_priority, hourly()),
        task(X, A, 500, reads_k),
        task(U, B, u_priority, hourly()),
    ]);
    let source = |name: &str, document: &Value| {
        CatalogDocumentSource::new(
            format!("memory://fixture/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", tasks)),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([profile()]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

/// One task of `instance_id` with its priority and trigger; its feedback stop stays false.
fn task(task_id: &str, instance_id: &str, priority: i16, trigger: Value) -> Value {
    json!({
        "id": task_id,
        "scope": {"kind": "instance", "instance_id": instance_id},
        "entrypoint": {"operation_id": "operation.run"},
        "procedure_ref": format!("procedure.{task_id}"),
        "priority": priority,
        "trigger": trigger,
        "feedback_stop": {
            "kind": "outcome",
            "task_id": task_id,
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
        "loop_budget": {"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// One profile for the fixture game, open all day on every weekday.
fn profile() -> Value {
    json!({
        "id": "fixture-activity",
        "scope": {"kind": "game", "game_id": "game-a"},
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
    })
}

// ---- Reading the round

fn bytes(evaluation: &PolicyEvaluation) -> Vec<u8> {
    serde_json::to_vec(evaluation).expect("the evaluation serializes")
}

fn decision<'a>(
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

/// R_I's decision on A.
fn refresh_decision(evaluation: &PolicyEvaluation) -> &TaskDecision {
    decision(evaluation, FACT_REFRESH_TASK_ID, A)
}

fn last_code(decision: &TaskDecision) -> &str {
    decision
        .reasons
        .last()
        .map_or("", |reason| reason.outcome.code().as_str())
}

fn has(decision: &TaskDecision, code: &str) -> bool {
    decision
        .reasons
        .iter()
        .any(|reason| reason.outcome.code().as_str() == code)
}

fn coded<'a>(decision: &'a TaskDecision, code: &str) -> Vec<&'a DecisionReason> {
    decision
        .reasons
        .iter()
        .filter(|reason| reason.outcome.code().as_str() == code)
        .collect()
}

/// The tasks dispatched on `instance_id`.
fn dispatched<'a>(evaluation: &'a PolicyEvaluation, instance_id: &str) -> Vec<&'a str> {
    evaluation
        .dispatch_intents
        .iter()
        .filter(|intent| intent.instance_id == instance_id)
        .map(|intent| intent.task_id.as_str())
        .collect()
}

/// B's part of the round: its decisions (each with its wake), its intents and their reason chains.
fn on_b(
    evaluation: &PolicyEvaluation,
) -> (
    Vec<TaskDecision>,
    Vec<DispatchIntent>,
    Vec<DecisionReasonChain>,
) {
    let intents: Vec<DispatchIntent> = evaluation
        .dispatch_intents
        .iter()
        .filter(|intent| intent.instance_id == B)
        .cloned()
        .collect();
    let chains = evaluation
        .reason_chains
        .iter()
        .filter(|chain| {
            intents
                .iter()
                .any(|intent| intent.reason_chain_id == chain.id)
        })
        .cloned()
        .collect();
    let decisions = evaluation
        .decisions
        .iter()
        .filter(|decision| decision.instance_id.as_deref() == Some(B))
        .cloned()
        .collect();
    (decisions, intents, chains)
}

/// `decision` waits at position 4a: deferred, its last reason `application_update_pending` naming
/// A and the key, with no wake.
fn assert_held(decision: &TaskDecision, case: &str) {
    assert_eq!(
        decision.state,
        SchedulingDecisionState::Deferred,
        "{case}: {:?}",
        decision.reasons
    );
    let reason = decision.reasons.last().expect("a reason");
    assert_eq!(
        reason.outcome.code().as_str(),
        PENDING,
        "{case}: {:?}",
        decision.reasons
    );
    assert_values(
        reason,
        &[("instance_id", json!(A)), ("fact_key", json!(UPDATE_KEY))],
    );
    assert_eq!(decision.next_wake_unix_ms, None, "{case}: no wake");
}

/// The reason's values are exactly these.
fn assert_values(reason: &DecisionReason, expected: &[(&str, Value)]) {
    let expected = expected
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect::<Map<String, Value>>();
    assert_eq!(
        reason.outcome.values(),
        &expected,
        "{}: {}",
        reason.outcome.code().as_str(),
        reason.detail
    );
}
