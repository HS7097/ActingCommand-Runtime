// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B41 (brief `rd-1021/train-0129/brief-b41.md`, frozen model items 1 to 5, tests T1
// to T4; contracts/scheduling/README.md, "Chain Handover"; policy gate keys 60 to 63): a chain step
// that hands over on its page goes first. One instance; a chain whose predecessor `task.lead` ran
// just before the round and ended on the page the instance rests on; its successor `task.close`
// starts on `game-a/result`; a higher-ranked `task.need` reads an expired realistic fact, so the
// instance's fact refresh reading R_I is requested with that task's rank. Every expected value
// comes from the model item named on each test. Fake clock, neutral ids.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::{Code, Outcome};
use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CandidateEligibility, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, CompletedActivityWindow, DecisionReason, DispatchIntent, EvaluationFacts,
    EvaluationResources, EvaluationTime, FACT_REFRESH_TASK_ID, FactRefreshInputs,
    FactRefreshReader, FactRefreshSettings, HostResourceSnapshot, PolicyEvaluation,
    PolicyEvaluationError, RemedyPlan, RestingPage, SchedulingDecisionState, TaskDecision,
    TaskRuntimeSnapshot, TaskTerminalState, compile_catalog, evaluate_with_chain_handovers,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
// The chain: the predecessor, and the successor that starts on RESULT.
const LEAD: &str = "task.lead";
const CLOSE: &str = "task.close";
// The higher-ranked task that reads the expired fact only it needs.
const NEED: &str = "task.need";
// A realistic key: an instance fact recognition writes; the reader's marks write it.
const STOCK: &str = "resource.stock";
const READER: &str = "package.reader-a";
// The successor's entry page, and a page outside its entry page set.
const RESULT: &str = "game-a/result";
const HOME: &str = "game-a/home";
const WAITING: &str = "policy_chain_handover_waiting";
const PENDING: &str = "fact_refresh_pending";
const NOT_SELECTED: &str = "decision_candidate_not_selected";
// The Runtime's deferral of a pair whose task package was unloaded (Workflow #389: its pending
// dispatch is withdrawn, and the next rounds defer the pair at position 5).
const NOT_LOADED: &str = "pack_registry_not_loaded";
// The Runtime's deferral of a pair that waits for its drawn random delay (Workflow #405 D1,
// position 9); the host names it as a deferral under which a successor still goes first.
const RANDOM_DELAY: &str = "dispatch_random_delay_waiting";
// The predecessor was admitted ten minutes before the round and ended two minutes before it; the
// instance's latest recognition matched its page three minutes before the round, during its run.
const LEAD_ADMITTED: u64 = NOW - 600_000;
const LEAD_COMPLETED: u64 = NOW - 120_000;
const PAGE_SEEN: u64 = NOW - 180_000;

// T1 (model items 1 to 3): a handover with an expired fact that only another task needs. The
// successor is dispatched before the refresh; the refresh waits under the registered reason, its
// affected task names that gate, and the refresh runs in the round after the chain step ran.
#[test]
fn b41_t1_the_successor_goes_before_the_refresh_and_the_refresh_runs_after_it() {
    let catalog = chain_catalog(dependency(LEAD));
    let first = evaluate(
        &catalog,
        NOW,
        vec![lead_state()],
        resting(RESULT, PAGE_SEEN),
        |_| Ok(CandidateEligibility::Eligible),
    );
    assert_eq!(
        tasks_on(&first),
        [CLOSE],
        "the chain step is the round's dispatch: {:#?}",
        first.dispatch_intents
    );
    let refresh = refresh_decision(&first);
    assert_eq!(refresh.state, SchedulingDecisionState::Deferred);
    assert_eq!(last_code(refresh), WAITING);
    assert_values(
        refresh.reasons.last().expect("the refresh's reason"),
        &[
            ("instance_id", json!(A)),
            ("task_id", json!(CLOSE)),
            ("page_id", json!(RESULT)),
        ],
    );
    let affected = coded(decision(&first, NEED), PENDING);
    assert_eq!(affected.len(), 1, "{:#?}", decision(&first, NEED));
    assert_eq!(
        affected[0].outcome.values().get("gate_code"),
        Some(&json!(WAITING)),
        "the task that needs the fact waits for the refresh the handover holds back"
    );

    // The chain step ran: dispatched just after the round, ended on another page.
    let after = NOW + 300_000;
    let close_ran = TaskRuntimeSnapshot {
        task_id: CLOSE.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(NOW + 10_000),
        eligible_since_unix_ms: None,
        terminal_state: Some(TaskTerminalState::Succeeded),
        completed_window: Some(CompletedActivityWindow {
            window_id: "window.fixture".to_owned(),
            completed_at_unix_ms: NOW + 120_000,
        }),
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
    };
    let second = evaluate(
        &catalog,
        after,
        vec![lead_state(), close_ran],
        resting(HOME, NOW + 100_000),
        |_| Ok(CandidateEligibility::Eligible),
    );
    assert_eq!(
        tasks_on(&second),
        [FACT_REFRESH_TASK_ID],
        "the refresh runs after the chain step"
    );
}

// T2 (model item 4): the successor's own admission needs the expired fact. Today's order stays:
// the refresh goes first, and nothing waits under the handover's reason.
#[test]
fn b41_t2_a_successor_that_needs_the_expired_fact_lets_the_refresh_go_first() {
    let catalog = chain_catalog(json!({
        "kind": "all",
        "predicates": [dependency(LEAD), stock_trigger()]
    }));
    let evaluation = evaluate(
        &catalog,
        NOW,
        vec![lead_state()],
        resting(RESULT, PAGE_SEEN),
        |_| Ok(CandidateEligibility::Eligible),
    );
    assert_eq!(
        tasks_on(&evaluation),
        [FACT_REFRESH_TASK_ID],
        "the refresh goes first: {:#?}",
        evaluation.decisions
    );
    let successor = decision(&evaluation, CLOSE);
    assert_eq!(successor.state, SchedulingDecisionState::Deferred);
    assert_eq!(
        coded(successor, PENDING).len(),
        1,
        "the successor waits for the refresh"
    );
    assert_no_handover_reason(&evaluation);
}

// T3 (model item 1): the predecessor ended on a page outside the successor's entry page set. No
// handover: the ranked order dispatches the refresh, and nothing waits under the handover's
// reason.
#[test]
fn b41_t3_a_predecessor_that_ended_outside_the_entry_page_set_hands_nothing_over() {
    let catalog = chain_catalog(dependency(LEAD));
    let evaluation = evaluate(
        &catalog,
        NOW,
        vec![lead_state()],
        resting(HOME, PAGE_SEEN),
        |_| Ok(CandidateEligibility::Eligible),
    );
    assert_eq!(
        tasks_on(&evaluation),
        [FACT_REFRESH_TASK_ID],
        "{:#?}",
        evaluation.decisions
    );
    assert_ne!(
        decision(&evaluation, CLOSE).state,
        SchedulingDecisionState::Selected
    );
    assert_no_handover_reason(&evaluation);
}

// T4 (model items 3 and 5): the chain breaks during a handover. The round of the handover records
// the deferral: the refresh's decision ends with the registered reason, and the dispatch decision
// record of the chain step lists the refresh with that `why`. The successor's dispatch is then
// withdrawn (its package was unloaded) and the next round's admission defers it; that round runs
// the deferred refresh.
#[test]
fn b41_t4_when_the_chain_breaks_the_deferred_refresh_runs_at_the_next_evaluation() {
    let catalog = chain_catalog(dependency(LEAD));
    let first = evaluate(
        &catalog,
        NOW,
        vec![lead_state()],
        resting(RESULT, PAGE_SEEN),
        |_| Ok(CandidateEligibility::Eligible),
    );
    assert_eq!(tasks_on(&first), [CLOSE]);
    assert_eq!(last_code(refresh_decision(&first)), WAITING);
    let intent = &first.dispatch_intents[0];
    let chain = first
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the chain step's reason chain");
    let listed = chain
        .reasons
        .iter()
        .filter(|reason| reason.outcome.code().as_str() == NOT_SELECTED)
        .find(|reason| reason.outcome.values().get("task_id") == Some(&json!(FACT_REFRESH_TASK_ID)))
        .expect("the dispatch decision record lists the refresh");
    assert_eq!(
        listed.outcome.values().get("why"),
        Some(&json!(WAITING)),
        "the deferral is recorded as the refresh's why"
    );

    let next = NOW + 30_000;
    let second = evaluate(
        &catalog,
        next,
        vec![lead_state()],
        resting(RESULT, PAGE_SEEN),
        |intent| {
            Ok(if intent.task_id == CLOSE {
                CandidateEligibility::Deferred {
                    reason: DecisionReason {
                        outcome: Outcome::relayed(NOT_LOADED),
                        detail: format!("fixture admission: {NOT_LOADED}"),
                    },
                    next_wake_unix_ms: None,
                }
            } else {
                CandidateEligibility::Eligible
            })
        },
    );
    assert_eq!(
        tasks_on(&second),
        [FACT_REFRESH_TASK_ID],
        "the deferred refresh runs at the next evaluation: {:#?}",
        second.decisions
    );
    assert_eq!(last_code(decision(&second, CLOSE)), NOT_LOADED);
    assert_no_handover_reason(&second);
}

// T5 (model item 3; coordinator ruling 10-21 on B41: the random delay is part of the successor's
// dispatch). A handover whose successor waits for its non-zero random delay keeps the expired-fact
// refresh deferred, its own delay already over, and nothing runs on the instance; at the planned
// start the successor is dispatched while the refresh still waits; the round after the chain step
// ran dispatches the refresh.
#[test]
fn b41_t5_a_successor_waiting_for_its_random_delay_keeps_the_refresh_deferred() {
    let catalog = chain_catalog(dependency(LEAD));
    let planned_start = NOW + 90_000;
    let waiting = evaluate(
        &catalog,
        NOW,
        vec![lead_state()],
        resting(RESULT, PAGE_SEEN),
        successor_delayed(NOW, planned_start),
    );
    assert!(
        tasks_on(&waiting).is_empty(),
        "nothing runs on the instance while the successor waits for its delay: {:#?}",
        waiting.decisions
    );
    assert_eq!(last_code(decision(&waiting, CLOSE)), RANDOM_DELAY);
    let refresh = refresh_decision(&waiting);
    assert_eq!(refresh.state, SchedulingDecisionState::Deferred);
    assert_eq!(last_code(refresh), WAITING);

    let started = evaluate(
        &catalog,
        planned_start,
        vec![lead_state()],
        resting(RESULT, PAGE_SEEN),
        successor_delayed(planned_start, planned_start),
    );
    assert_eq!(
        tasks_on(&started),
        [CLOSE],
        "the successor is dispatched at its planned start"
    );
    assert_eq!(last_code(refresh_decision(&started)), WAITING);

    // The chain step ran: dispatched at its planned start, ended on another page.
    let after = planned_start + 300_000;
    let close_ran = TaskRuntimeSnapshot {
        task_id: CLOSE.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(planned_start + 5_000),
        eligible_since_unix_ms: None,
        terminal_state: Some(TaskTerminalState::Succeeded),
        completed_window: Some(CompletedActivityWindow {
            window_id: "window.fixture".to_owned(),
            completed_at_unix_ms: planned_start + 60_000,
        }),
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
    };
    let ran = evaluate(
        &catalog,
        after,
        vec![lead_state(), close_ran],
        resting(HOME, planned_start + 30_000),
        successor_delayed(after, planned_start),
    );
    assert_eq!(
        tasks_on(&ran),
        [FACT_REFRESH_TASK_ID],
        "the refresh runs after the chain step"
    );
}

// ---- Fixtures ----

// The chain catalog: the predecessor (in cooldown after its run), the successor triggered by
// `close_trigger` (low priority) and the higher-ranked task that reads the expired fact.
fn chain_catalog(close_trigger: Value) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.chain-handover",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-chain"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let tasks = json!([
        task(LEAD, 200, hourly(), 2 * HOUR_MS),
        task(CLOSE, 100, close_trigger, HOUR_MS),
        task(NEED, 300, stock_trigger(), HOUR_MS),
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
    .unwrap_or_else(|failure| panic!("the chain catalog compiles: {:?}", failure.diagnostics()))
}

fn task(task_id: &str, priority: i16, trigger: Value, cooldown_ms: u64) -> Value {
    json!({
        "id": task_id,
        "scope": {"kind": "game", "game_id": GAME},
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
        "cooldown_ms": cooldown_ms,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "loop_budget": {"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

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

// True once `task_id`'s latest run on the instance succeeded.
fn dependency(task_id: &str) -> Value {
    json!({"kind": "dependency_completed", "task_id": task_id, "terminal_states": ["succeeded"]})
}

// Reads the instance's realistic key within fifteen minutes of its observation.
fn stock_trigger() -> Value {
    json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": STOCK,
        "comparison": "less_than",
        "value": {"type": "integer", "value": 20},
        "max_age_ms": 900000
    })
}

// One profile for the whole game, active all day on every weekday.
fn profile() -> Value {
    json!({
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
    })
}

// The predecessor's latest run on the instance: admitted ten minutes before the round, succeeded
// two minutes before it.
fn lead_state() -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: LEAD.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(LEAD_ADMITTED),
        eligible_since_unix_ms: None,
        terminal_state: Some(TaskTerminalState::Succeeded),
        completed_window: Some(CompletedActivityWindow {
            window_id: "window.fixture".to_owned(),
            completed_at_unix_ms: LEAD_COMPLETED,
        }),
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
    }
}

// The instance rests on `page_id`, recorded at `observed_at_unix_ms`; the successor still goes
// first while it waits for its random delay, as the host passes it.
fn resting(page_id: &str, observed_at_unix_ms: u64) -> ChainHandoverInputs {
    ChainHandoverInputs {
        resting_pages: BTreeMap::from([(
            A.to_owned(),
            RestingPage {
                page_id: page_id.to_owned(),
                observed_at_unix_ms,
            },
        )]),
        successor_wait_codes: vec![
            Code::relayed(RANDOM_DELAY).expect("the Runtime's random delay code is registered"),
        ],
    }
}

// The Runtime's admission at `now` with the instance's random delay on: the successor's drawn delay
// ends at `planned_start`, every other pair's delay is over.
fn successor_delayed(
    now: u64,
    planned_start: u64,
) -> impl FnMut(&DispatchIntent) -> Result<CandidateEligibility, PolicyEvaluationError> {
    move |intent| {
        Ok(if intent.task_id == CLOSE && now < planned_start {
            CandidateEligibility::Deferred {
                reason: DecisionReason {
                    outcome: Outcome::relayed(RANDOM_DELAY),
                    detail: format!(
                        "fixture admission: {RANDOM_DELAY} next_eligible_unix_ms={planned_start}"
                    ),
                },
                next_wake_unix_ms: Some(planned_start),
            }
        } else {
            CandidateEligibility::Eligible
        })
    }
}

// The instance's fact refresh: its reader is bound, marks STOCK and has not run today.
fn refresh_inputs() -> FactRefreshInputs {
    FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: true,
            min_interval_ms: 300_000,
            daily_limit: 96,
            failure_backoff_ms: 600_000,
            max_consecutive_failures: 3,
        },
        readers: vec![FactRefreshReader {
            instance_id: A.to_owned(),
            package_id: READER.to_owned(),
            marked_keys: BTreeSet::from([STOCK.to_owned()]),
            dispatched_today: 0,
            last_failure_code: None,
            problem: None,
        }],
        instance_facts_loaded_at_unix_ms: None,
        instance_holds: BTreeMap::new(),
    }
}

// Evaluates the catalog at `now` on the one instance, whose STOCK expired before the round; the
// successor's package entry holds RESULT alone.
fn evaluate(
    catalog: &CompiledCatalog,
    now: u64,
    tasks: Vec<TaskRuntimeSnapshot>,
    chain: ChainHandoverInputs,
    eligibility: impl FnMut(&DispatchIntent) -> Result<CandidateEligibility, PolicyEvaluationError>,
) -> PolicyEvaluation {
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-chain-handover".to_owned(),
        facts: vec![fact(A, STOCK, 5, NOW - HOUR_MS, Some(NOW - 1_000))],
        outcomes: Vec::new(),
        tasks,
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    };
    let resources = EvaluationResources {
        pools: Vec::new(),
        hosts: vec![HostResourceSnapshot {
            host_id: format!("host.{A}"),
            cpu_available_milli: 1_000,
            gpu_available_milli: 1_000,
            io_available_milli: 1_000,
            host_responsiveness_basis_points: 10_000,
            third_party_pressure_basis_points: 0,
            heavy_dispatch_limit: 1,
            active_heavy_dispatches: 0,
            heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
        }],
    };
    let time = EvaluationTime {
        unix_ms: now,
        monotonic_ms: now,
    };
    evaluate_with_chain_handovers::<PolicyEvaluationError>(
        catalog,
        &facts,
        &resources,
        time,
        11,
        &BTreeSet::new(),
        &refresh_inputs(),
        &RemedyPlan::default(),
        &chain,
        |task, _instance_id, page_id| Ok(task.id == CLOSE && page_id == RESULT),
        eligibility,
    )
    .unwrap_or_else(|error| panic!("the evaluation succeeds: {error}"))
}

// The tasks the round dispatches on the instance.
fn tasks_on(evaluation: &PolicyEvaluation) -> Vec<&str> {
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
        .unwrap_or_else(|| panic!("a decision for {task_id}"))
}

fn refresh_decision(evaluation: &PolicyEvaluation) -> &TaskDecision {
    decision(evaluation, FACT_REFRESH_TASK_ID)
}

fn last_code(decision: &TaskDecision) -> &str {
    decision
        .reasons
        .last()
        .map_or("", |reason| reason.outcome.code().as_str())
}

fn coded<'a>(decision: &'a TaskDecision, code: &str) -> Vec<&'a DecisionReason> {
    decision
        .reasons
        .iter()
        .filter(|reason| reason.outcome.code().as_str() == code)
        .collect()
}

// The reason's values are exactly these.
fn assert_values(reason: &DecisionReason, expected: &[(&str, Value)]) {
    let expected = expected
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect::<serde_json::Map<String, Value>>();
    assert_eq!(reason.outcome.values(), &expected, "{}", reason.detail);
}

// No decision of the round waits under the handover's reason, and no gate names it.
fn assert_no_handover_reason(evaluation: &PolicyEvaluation) {
    for decision in &evaluation.decisions {
        for reason in &decision.reasons {
            assert_ne!(
                reason.outcome.code().as_str(),
                WAITING,
                "{}: {:#?}",
                decision.task_id,
                decision.reasons
            );
            assert_ne!(
                reason.outcome.values().get("gate_code"),
                Some(&json!(WAITING)),
                "{}: {:#?}",
                decision.task_id,
                decision.reasons
            );
        }
    }
}
