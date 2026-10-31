// SPDX-License-Identifier: AGPL-3.0-only

// SS-4 gate tests (register §1.231; design `rd-1029/design-sch-state.md` §4.7 and the SS-4 entry
// of §5, tests 1 to 5; plan `rd-1029/plan-state-driven.md`, the notes on removing the counts;
// coordinator work order `rd-1031/brief-ss4.md`, tests 1 to 7; `contracts/scheduling/README.md`,
// "No-progress guard"). After a successful run none of whose basis facts (what its admission read)
// has a reading of another value, the (task, instance) pair waits with `policy_no_progress` until
// one has; failures keep their retry path; with the guard off the evaluation is the one before.
//
// A neutral V2 catalog on instance `instance-a`: the guarded task G (priority 200), due while A's
// stock is at least 1, and the task O (priority 100), due at every whole hour, the instance's other
// work. Each case runs the Runtime's memory as the host does: round 1 captures the basis of G's
// intent and its admission arms the guard; round 2, after G's run, observes the facts and
// evaluates with the armed guards. Deterministic: fixed instants, no clock read. Every expected
// value comes from the work order and the design, never from what the code does.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{StandardPackRootBytes, read_standard_pack_root};
use serde_json::{Map, Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::{
    CandidateEligibility, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, CompletedActivityWindow, DecisionReason, EligibilityState, EvaluationFacts,
    EvaluationResources, EvaluationTime, FACT_REFRESH_TASK_ID, FactRefreshInputs,
    FactRefreshReader, FactRefreshSettings, FactValue, HostResourceSnapshot, InstanceHolds,
    NoProgressGuards, NoProgressMemory, ObservedFact, OwedReruns, PolicyEvaluation,
    PolicyEvaluationError, RemedyPlan, RunSlots, SCHEDULING_SCHEMA_VERSION_V2,
    SchedulingDecisionState, ScopeSelector, StandardPackBinding, StandardPackComposeInput,
    StandardPackComposition, StandardPackGameFiles, TaskDecision, TaskRuntimeSnapshot,
    TaskTerminalState, compile_catalog, compose_standard_pack_rules, evaluate_with_instance_holds,
    evaluate_with_no_progress,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const GUARDED: &str = "task.guarded";
const OTHER: &str = "task.other";
// A's stock, which the maintenance package reads, and a self-read remaining count (SS-5).
const STOCK: &str = "resource.neutral.stock";
const REMAINING: &str = "resource.neutral.remaining";
// A fact of no family recognition writes: a lock, no basis key.
const LOCK: &str = "session.neutral.unlocked";
const READER: &str = "fixture.reader";
// The daily reset's latest occurrence, half an hour before NOW.
const RESET_AT: u64 = NOW - 1_800_000;
// The reading round 1 admits on, observed an hour before NOW.
const READ_AT: u64 = NOW - HOUR_MS;
// Round 1 evaluates, the dispatch is admitted, and the run settles a minute later.
const ROUND_1: u64 = NOW - 700_000;
const DISPATCHED: u64 = NOW - 600_000;
const SETTLED: u64 = DISPATCHED + 60_000;
const NO_PROGRESS: &str = "policy_no_progress";
const FACT_PROBE: &str = "fact_probe";
const TRIGGER_FALSE: &str = "trigger_false";
const TRIGGER_UNKNOWN: &str = "trigger_unknown";
const REQUESTED: &str = "fact_refresh_requested";
const RETRY_IMMEDIATE: &str = "failure_retry_immediate";
// The standard-pack fixtures: one game root with one package on one server, one bound instance.
const SCHEDULE_INSTANCE: &str = "fixture-instance-a";
const SERVER: &str = "server-a";
const PACKAGE: &str = "fixture.neutral";
const ESCALATION_HIGH: &str = "standard_pack_escalation_threshold_high";

// Test 1: after G's successful run no basis key has a newer reading: G is deferred with
// `policy_no_progress`, naming its one basis key with the admission's reading and the run's
// dispatch, no wake, and not dispatched; the instance's other work runs. A dispatch decided on no
// pool, stock, remaining count or completion time has no basis: a clock beside a lock fact, and an
// `any` whose clock came first and was due though the stock gate held too. Nothing is armed, and
// the clock alone decides the next hour as before.
#[test]
fn ss4_t1_a_successful_run_without_a_newer_reading_holds_the_pair() {
    for (case, trigger) in [
        (
            "a clock beside a lock",
            json!({"kind": "all", "predicates": [hourly(), fact_trigger(LOCK, false)]}),
        ),
        (
            "an any its clock decided",
            json!({"kind": "any", "predicates": [hourly(), fact_trigger(STOCK, false)]}),
        ),
    ] {
        let edge = neutral_catalog(
            json!([task(GUARDED, 200, trigger), task(OTHER, 100, hourly())]),
            reset(&["resource.other."]),
        );
        let readings = vec![fact(LOCK, 1, READ_AT, None), stock(5, READ_AT, None)];
        let first = round_1(&edge, &readings);
        let mut memory = NoProgressMemory::default();
        memory
            .capture(
                &edge,
                &evaluation_facts(readings.clone(), Vec::new()),
                &resources(),
                time(ROUND_1),
                &first.dispatch_intents,
                |_| true,
            )
            .unwrap_or_else(|error| panic!("{case}: the memory captures: {error}"));
        memory.admit(
            GUARDED,
            A,
            &first.dispatch_intents[0].decision_id,
            DISPATCHED,
        );
        assert!(memory.guards().is_empty(), "{case}: no basis, no guard");
        let next_hour = evaluate_round(
            &edge,
            &evaluation_facts(readings, succeeded()),
            &memory.guards(),
            NOW,
        );
        assert_eq!(dispatched(&next_hour), [GUARDED], "{case}");
    }

    let catalog = stock_catalog();
    let memory = armed(&catalog, &stock_facts(5, READ_AT));
    let round = round_2(
        &catalog,
        &memory,
        stock_facts(5, READ_AT),
        TaskTerminalState::Succeeded,
    );
    assert_eq!(dispatched(&round), [OTHER]);
    let held = decision(&round, GUARDED);
    assert_eq!(held.state, SchedulingDecisionState::Deferred);
    assert_eq!(held.eligibility, EligibilityState::True);
    assert_eq!(held.next_wake_unix_ms, None, "no wake");
    assert_eq!(last_code(held), NO_PROGRESS);
    assert_values(
        held.reasons.last().expect("a reason"),
        &[
            (
                "basis_readings",
                json!([{
                    "fact_key": STOCK,
                    "reading_value": "{\"type\":\"integer\",\"value\":5}",
                    "observed_at_unix_ms": READ_AT,
                }]),
            ),
            ("served_by_dispatch_unix_ms", json!(DISPATCHED)),
        ],
    );
}

// Test 2: G's run published a stock of another value: the guard is released and G is a candidate
// again, dispatched ahead of O. Released for good: a later reading of the admission's value does
// not arm it again. The evaluator given the guard as it was armed does not hold G either.
#[test]
fn ss4_t2_a_reading_of_another_value_makes_the_pair_a_candidate_again() {
    let catalog = stock_catalog();
    let mut memory = armed(&catalog, &stock_facts(5, READ_AT));
    let as_armed = memory.guards();
    let spent = stock_facts(4, SETTLED);
    observe(&catalog, &mut memory, &spent, NOW);
    assert_eq!(memory.armed(GUARDED, A), None, "released");
    let spent = evaluation_facts(spent, succeeded());
    let round = evaluate_round(&catalog, &spent, &memory.guards(), NOW);
    assert_eq!(dispatched(&round), [GUARDED]);
    assert!(!codes(decision(&round, GUARDED)).contains(&NO_PROGRESS));
    let with_old_guard = evaluate_round(&catalog, &spent, &as_armed, NOW);
    assert_eq!(dispatched(&with_old_guard), [GUARDED]);

    let back = stock_facts(5, SETTLED + 60_000);
    observe(&catalog, &mut memory, &back, NOW);
    assert_eq!(
        memory.armed(GUARDED, A),
        None,
        "a released guard stays released"
    );
    let round = evaluate_round(
        &catalog,
        &evaluation_facts(back, succeeded()),
        &memory.guards(),
        NOW,
    );
    assert_eq!(dispatched(&round), [GUARDED]);
}

// Test 3: G's run left a newer reading of the same value: still no progress (only values are
// compared). The reason names the admission's reading, not the newer one.
#[test]
fn ss4_t3_a_newer_reading_of_the_same_value_is_no_progress() {
    let catalog = stock_catalog();
    let memory = armed(&catalog, &stock_facts(5, READ_AT));
    let round = round_2(
        &catalog,
        &memory,
        stock_facts(5, SETTLED),
        TaskTerminalState::Succeeded,
    );
    assert_eq!(dispatched(&round), [OTHER]);
    let held = decision(&round, GUARDED);
    assert_eq!(last_code(held), NO_PROGRESS);
    assert_eq!(
        held.reasons
            .last()
            .expect("a reason")
            .outcome
            .value("basis_readings"),
        Some(&json!([{
            "fact_key": STOCK,
            "reading_value": "{\"type\":\"integer\",\"value\":5}",
            "observed_at_unix_ms": READ_AT,
        }]))
    );
}

// Test 4: G's run failed: the guard holds nothing, G takes its retry path as before: a candidate
// past its cooldown, dispatched, and as an immediate rerun with `failure_retry_immediate`.
#[test]
fn ss4_t4_a_failed_run_takes_the_retry_path() {
    let catalog = stock_catalog();
    let memory = armed(&catalog, &stock_facts(5, READ_AT));
    let round = round_2(
        &catalog,
        &memory,
        stock_facts(5, READ_AT),
        TaskTerminalState::Failed,
    );
    assert_eq!(dispatched(&round), [GUARDED]);
    assert!(!codes(decision(&round, GUARDED)).contains(&NO_PROGRESS));

    let facts = evaluation_facts(
        stock_facts(5, READ_AT),
        vec![ran(GUARDED, DISPATCHED, TaskTerminalState::Failed)],
    );
    let retried = evaluate_with_no_progress::<PolicyEvaluationError>(
        &catalog,
        &facts,
        &resources(),
        time(NOW),
        11,
        &BTreeSet::from([(GUARDED.to_owned(), A.to_owned())]),
        &refresh_inputs(),
        &RemedyPlan::default(),
        &ChainHandoverInputs::default(),
        &RunSlots::default(),
        &OwedReruns::default(),
        &InstanceHolds::default(),
        &memory.guards(),
        |_, _, _| Ok(false),
        |_| Ok(CandidateEligibility::Eligible),
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"));
    assert_eq!(dispatched(&retried), [GUARDED]);
    let codes = codes(decision(&retried, GUARDED));
    assert!(codes.contains(&RETRY_IMMEDIATE), "{codes:?}");
    assert!(!codes.contains(&NO_PROGRESS), "{codes:?}");
}

// Test 5: with the guard off on the instance the evaluation is the one before SS-4, byte for byte
// and with every decision's wake: the memory captures nothing for it and forgets what it armed,
// and an evaluation with no guard equals `evaluate_with_instance_holds`'s.
#[test]
fn ss4_t5_with_the_guard_off_the_evaluation_is_as_before() {
    let catalog = stock_catalog();
    let facts = evaluation_facts(
        stock_facts(5, READ_AT),
        vec![ran(GUARDED, DISPATCHED, TaskTerminalState::Succeeded)],
    );
    let before = evaluate_with_instance_holds::<PolicyEvaluationError>(
        &catalog,
        &facts,
        &resources(),
        time(NOW),
        11,
        &BTreeSet::new(),
        &refresh_inputs(),
        &RemedyPlan::default(),
        &ChainHandoverInputs::default(),
        &RunSlots::default(),
        &OwedReruns::default(),
        &InstanceHolds::default(),
        |_, _, _| Ok(false),
        |_| Ok(CandidateEligibility::Eligible),
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"));

    // Turned off after the guard was armed: forgotten.
    let mut memory = armed(&catalog, &stock_facts(5, READ_AT));
    memory
        .observe(&catalog, &facts, time(NOW), |_| false)
        .unwrap_or_else(|error| panic!("the memory observes: {error}"));
    assert!(memory.guards().is_empty());
    let off = evaluate_round(&catalog, &facts, &memory.guards(), NOW);
    assert_eq!(off, before);
    assert_eq!(bytes(&off), bytes(&before));
    assert_eq!(dispatched(&off), [GUARDED]);

    // Off from the start: nothing is captured, so the admission arms nothing.
    let round_1 = round_1(&catalog, &stock_facts(5, READ_AT));
    let mut memory = NoProgressMemory::default();
    memory
        .capture(
            &catalog,
            &evaluation_facts(stock_facts(5, READ_AT), Vec::new()),
            &resources(),
            time(ROUND_1),
            &round_1.dispatch_intents,
            |_| false,
        )
        .unwrap_or_else(|error| panic!("the memory captures: {error}"));
    memory.admit(
        GUARDED,
        A,
        &round_1.dispatch_intents[0].decision_id,
        DISPATCHED,
    );
    assert!(memory.guards().is_empty());
    assert_eq!(
        evaluate_round(&catalog, &facts, &memory.guards(), NOW),
        before
    );
}

// Test 6: a rule that writes no daily bound and an `on_failure.escalation_threshold` above 10 is
// one `standard_pack_escalation_threshold_high` warning (game, the rule, its threshold, the bound
// 10), whether it writes `loop_budget` without `daily_limit` or no `loop_budget`; a rule with a
// daily limit, or with a threshold of 10, gives none. The file composes all the same, and the
// composition's own warnings do not carry it (only the resource tools report it).
#[test]
fn ss4_t6_no_daily_bound_and_a_high_escalation_threshold_is_warned_about() {
    let composed = compose(&json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [],
        "rules": [
            rule("rule.without_daily_limit", Some(json!({"window_iteration_limit": 4, "max_runtime_ms": 300000})), 11),
            rule("rule.daily_limit", Some(json!({"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000})), 1000),
            rule("rule.at_the_bound", None, 10),
            rule("rule.without_loop_budget", None, 11),
        ],
        "activity": [profile()]
    }))
    .unwrap_or_else(|failure| panic!("the schedule composes: {failure}"));
    let warning = |rule: &str, threshold: u64| {
        Outcome::new(SharedCode::StandardPackEscalationThresholdHigh)
            .game(GAME)
            .task_id(rule)
            .count(threshold)
            .limit_count(10u64)
    };
    assert_eq!(
        composed.escalation_warnings(),
        [
            warning("rule.without_daily_limit", 11),
            warning("rule.without_loop_budget", 11),
        ]
    );
    assert!(
        composed
            .warnings()
            .iter()
            .all(|warning| warning.code().as_str() != ESCALATION_HIGH),
        "{:?}",
        composed.warnings()
    );
    assert_eq!(composed.compiled().catalog().tasks.tasks.len(), 4);
}

// Test 7a (SS-5's probe): G's gate is a self-read count, probed while unknown. The probe run is
// admitted on no reading. (i) It publishes `remaining = 0`: a reading where there was none releases
// the guard, so G yields by its own gate (`trigger_false`, no `policy_no_progress`); after the next
// reset clears the reading the probe admits G again. (ii) It publishes nothing: in the same period
// the probe is spent (`fact_probe`, `trigger_false`), and after the reset the probe would admit G
// again, but no reading came since the admission: G waits with `policy_no_progress`, its key named
// with no admission reading.
#[test]
fn ss4_t7a_a_probe_run_that_publishes_zero_releases_the_guard() {
    let catalog = probe_catalog();
    let next_day = NOW + DAY_MS;

    let mut memory = armed(&catalog, &[]);
    let published = vec![remaining(0, SETTLED)];
    observe(&catalog, &mut memory, &published, NOW);
    assert_eq!(memory.armed(GUARDED, A), None, "released");
    let round = evaluate_round(
        &catalog,
        &evaluation_facts(published.clone(), succeeded()),
        &memory.guards(),
        NOW,
    );
    assert_eq!(dispatched(&round), [OTHER]);
    assert_eq!(codes(decision(&round, GUARDED)), [TRIGGER_FALSE]);
    observe(&catalog, &mut memory, &published, next_day);
    let round = evaluate_round(
        &catalog,
        &evaluation_facts(published, succeeded()),
        &memory.guards(),
        next_day,
    );
    assert_eq!(
        dispatched(&round),
        [GUARDED],
        "the reset admits the probe again"
    );
    assert_eq!(code_of(&decision(&round, GUARDED).reasons[0]), FACT_PROBE);

    let mut memory = armed(&catalog, &[]);
    observe(&catalog, &mut memory, &[], NOW);
    let round = evaluate_round(
        &catalog,
        &evaluation_facts(Vec::new(), succeeded()),
        &memory.guards(),
        NOW,
    );
    assert_eq!(dispatched(&round), [OTHER]);
    assert_eq!(
        codes(decision(&round, GUARDED)),
        [FACT_PROBE, TRIGGER_FALSE]
    );
    observe(&catalog, &mut memory, &[], next_day);
    let round = evaluate_round(
        &catalog,
        &evaluation_facts(Vec::new(), succeeded()),
        &memory.guards(),
        next_day,
    );
    assert_eq!(dispatched(&round), [OTHER]);
    let held = decision(&round, GUARDED);
    assert_eq!(last_code(held), NO_PROGRESS);
    assert_eq!(code_of(&held.reasons[0]), FACT_PROBE);
    assert_eq!(
        held.reasons
            .last()
            .expect("a reason")
            .outcome
            .value("basis_readings"),
        Some(&json!([{"fact_key": REMAINING}]))
    );
}

// Test 7b (the fact refresh): the stock G was admitted on expires; G's trigger is unknown and the
// maintenance package is asked for the stock, the guard notwithstanding. The refresh publishes the
// same value with a newer timestamp: the guard is not released, G waits with `policy_no_progress`,
// and nothing more is asked of the maintenance package.
#[test]
fn ss4_t7b_a_refresh_that_publishes_the_same_value_does_not_release_the_guard() {
    let catalog = stock_catalog();
    let expiring = vec![stock(5, READ_AT, Some(SETTLED + 60_000))];
    let mut memory = armed(&catalog, &expiring);

    observe(&catalog, &mut memory, &expiring, NOW);
    let round = evaluate_round(
        &catalog,
        &evaluation_facts(expiring, succeeded()),
        &memory.guards(),
        NOW,
    );
    assert_eq!(requested_keys(&round), [STOCK]);
    let waiting = decision(&round, GUARDED);
    assert!(
        codes(waiting).contains(&TRIGGER_UNKNOWN),
        "{:?}",
        codes(waiting)
    );
    assert!(!codes(waiting).contains(&NO_PROGRESS));

    let later = NOW + 120_000;
    let refreshed = vec![stock(5, NOW + 60_000, Some(NOW + DAY_MS))];
    observe(&catalog, &mut memory, &refreshed, later);
    assert!(
        memory.armed(GUARDED, A).is_some(),
        "the same value releases nothing"
    );
    let round = evaluate_round(
        &catalog,
        &evaluation_facts(refreshed, succeeded()),
        &memory.guards(),
        later,
    );
    assert_eq!(dispatched(&round), [OTHER]);
    assert_eq!(last_code(decision(&round, GUARDED)), NO_PROGRESS);
    assert!(
        requested_keys(&round).is_empty(),
        "{:?}",
        requested_keys(&round)
    );
}

// ---- Fixtures ----

// G due while A's stock is at least 1, and O, under a reset that does not clear the stock.
fn stock_catalog() -> CompiledCatalog {
    neutral_catalog(
        json!([
            task(GUARDED, 200, fact_trigger(STOCK, false)),
            task(OTHER, 100, hourly()),
        ]),
        reset(&["resource.other."]),
    )
}

// G due while its self-read count is at least 1, probed while it is unknown, and O, under a reset
// that clears the count.
fn probe_catalog() -> CompiledCatalog {
    neutral_catalog(
        json!([
            task(GUARDED, 200, fact_trigger(REMAINING, true)),
            task(OTHER, 100, hourly()),
        ]),
        reset(&["resource.neutral."]),
    )
}

fn neutral_catalog(tasks: Value, event: Value) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.no-progress",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-no-progress"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let source = |name: &str, document: &Value| {
        CatalogDocumentSource::new(
            format!("memory://fixture-no-progress/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    let mut profile = profile();
    profile.insert("scope".to_owned(), json!({"kind": "game", "game_id": GAME}));
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", tasks)),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([event]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

// The daily reset of instance A, clearing `invalidated`.
fn reset(invalidated: &[&str]) -> Value {
    json!({
        "id": "reset.daily",
        "scope": {"kind": "instance", "instance_id": A},
        "event_kind": "reset",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": DAY_MS,
            "anchor_ms": RESET_AT
        },
        "duration_ms": 0,
        "invalidates_fact_prefixes": invalidated,
        "validity": {"from_unix_ms": 0, "until_unix_ms": null}
    })
}

fn task(task_id: &str, priority: i16, trigger: Value) -> Value {
    let mut body = task_body(trigger, outcome_stop_of(task_id), Some(loop_budget()), 3);
    body.insert("id".to_owned(), json!(task_id));
    body.insert("scope".to_owned(), json!({"kind": "game", "game_id": GAME}));
    body.insert(
        "entrypoint".to_owned(),
        json!({"operation_id": "operation.run"}),
    );
    body.insert(
        "procedure_ref".to_owned(),
        json!(format!("procedure.{task_id}")),
    );
    body.insert("priority".to_owned(), json!(priority));
    Value::Object(body)
}

fn loop_budget() -> Value {
    json!({"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000})
}

// A task's members besides its identity, scope, entry point and priority.
fn task_body(
    trigger: Value,
    feedback_stop: Value,
    loop_budget: Option<Value>,
    escalation_threshold: u64,
) -> Map<String, Value> {
    let Value::Object(mut body) = json!({
        "trigger": trigger,
        "feedback_stop": feedback_stop,
        "consumes": [],
        "produces": [],
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": escalation_threshold
        },
        "sensitive": false,
        "next_run_clamp_ms": DAY_MS,
        "yield_points": [],
        "expected_duration_ms": 60000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    }) else {
        unreachable!("a JSON object")
    };
    if let Some(loop_budget) = loop_budget {
        body.insert("loop_budget".to_owned(), loop_budget);
    }
    body
}

// Stops once the task's own result says it completed (never, in these rounds).
fn outcome_stop_of(task_id: &str) -> Value {
    json!({
        "kind": "outcome",
        "task_id": task_id,
        "outcome_key": "completed",
        "comparison": "eq",
        "value": {"type": "boolean", "value": true}
    })
}

// Due while A's `key` is at least 1; with `probe`, probed while it is unknown (SS-5).
fn fact_trigger(key: &str, probe: bool) -> Value {
    let mut trigger = json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": key,
        "comparison": "greater_than_or_equal",
        "value": {"type": "integer", "value": 1},
        "max_age_ms": null
    });
    if probe {
        trigger["on_unknown"] = json!("probe");
    }
    trigger
}

// A clock due at every whole hour (the instance's monotonic clock equals Unix time here).
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

// One profile, active all day on every weekday, without its scope.
fn profile() -> Map<String, Value> {
    let Value::Object(profile) = json!({
        "id": "fixture-activity",
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
    }) else {
        unreachable!("a JSON object")
    };
    profile
}

fn fact(key: &str, value: i64, observed_at: u64, expires_at: Option<u64>) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: key.to_owned(),
        value: FactValue::Integer(value),
        observed_at_unix_ms: observed_at,
        expires_at_unix_ms: expires_at,
        confidence_milli: 1_000,
    }
}

fn stock(value: i64, observed_at: u64, expires_at: Option<u64>) -> ObservedFact {
    fact(STOCK, value, observed_at, expires_at)
}

fn stock_facts(value: i64, observed_at: u64) -> Vec<ObservedFact> {
    vec![stock(value, observed_at, None)]
}

fn remaining(value: i64, observed_at: u64) -> ObservedFact {
    fact(REMAINING, value, observed_at, None)
}

// `task_id`'s latest run on instance A, dispatched at `at`, settled a minute later.
fn ran(task_id: &str, at: u64, terminal_state: TaskTerminalState) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: task_id.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(at),
        eligible_since_unix_ms: None,
        terminal_state: Some(terminal_state),
        completed_window: Some(CompletedActivityWindow {
            window_id: "fixture-window".to_owned(),
            completed_at_unix_ms: at + 60_000,
        }),
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
    }
}

// G's guarded run, settled as a success.
fn succeeded() -> Vec<TaskRuntimeSnapshot> {
    vec![ran(GUARDED, DISPATCHED, TaskTerminalState::Succeeded)]
}

fn evaluation_facts(facts: Vec<ObservedFact>, tasks: Vec<TaskRuntimeSnapshot>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-no-progress".to_owned(),
        facts,
        outcomes: Vec::new(),
        tasks,
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

fn time(unix_ms: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms,
        monotonic_ms: unix_ms,
    }
}

fn resources() -> EvaluationResources {
    EvaluationResources {
        pools: Vec::new(),
        hosts: vec![HostResourceSnapshot {
            host_id: instance(A, &[]).host_id,
            cpu_available_milli: 1_000,
            gpu_available_milli: 1_000,
            io_available_milli: 1_000,
            host_responsiveness_basis_points: 10_000,
            third_party_pressure_basis_points: 0,
            heavy_dispatch_limit: 1,
            active_heavy_dispatches: 0,
            heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
        }],
    }
}

// The fact refresh at its starting defaults, A's maintenance package marking the stock.
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
            marked_keys: BTreeSet::from([STOCK.to_owned()]),
            dispatched_today: 0,
            last_failure_code: None,
            problem: None,
        }],
        instance_facts_loaded_at_unix_ms: Some(NOW - DAY_MS),
        instance_holds: BTreeMap::new(),
        idle_closed: BTreeMap::new(),
    }
}

// The Runtime's round at `at` with `guards`; the Runtime admits every candidate.
fn evaluate_round(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    guards: &NoProgressGuards,
    at: u64,
) -> PolicyEvaluation {
    evaluate_with_no_progress::<PolicyEvaluationError>(
        catalog,
        facts,
        &resources(),
        time(at),
        11,
        &BTreeSet::new(),
        &refresh_inputs(),
        &RemedyPlan::default(),
        &ChainHandoverInputs::default(),
        &RunSlots::default(),
        &OwedReruns::default(),
        &InstanceHolds::default(),
        guards,
        |_, _, _| Ok(false),
        |_| Ok(CandidateEligibility::Eligible),
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

// Round 1: G has not run; it is selected on `facts`.
fn round_1(catalog: &CompiledCatalog, facts: &[ObservedFact]) -> PolicyEvaluation {
    let round = evaluate_round(
        catalog,
        &evaluation_facts(facts.to_vec(), Vec::new()),
        &NoProgressGuards::default(),
        ROUND_1,
    );
    assert_eq!(dispatched(&round), [GUARDED], "round 1 selects G");
    round
}

// The memory after round 1 on `facts`: G's basis captured and its dispatch admitted at
// DISPATCHED, the guard armed.
fn armed(catalog: &CompiledCatalog, facts: &[ObservedFact]) -> NoProgressMemory {
    let round = round_1(catalog, facts);
    let mut memory = NoProgressMemory::default();
    memory
        .capture(
            catalog,
            &evaluation_facts(facts.to_vec(), Vec::new()),
            &resources(),
            time(ROUND_1),
            &round.dispatch_intents,
            |alias| alias == A,
        )
        .unwrap_or_else(|error| panic!("the memory captures: {error}"));
    memory.admit(
        GUARDED,
        A,
        &round.dispatch_intents[0].decision_id,
        DISPATCHED,
    );
    assert!(
        memory.armed(GUARDED, A).is_some(),
        "the admission arms the guard"
    );
    memory
}

// The memory observes `facts` at `at`, as the host does before it evaluates.
fn observe(
    catalog: &CompiledCatalog,
    memory: &mut NoProgressMemory,
    facts: &[ObservedFact],
    at: u64,
) {
    memory
        .observe(
            catalog,
            &evaluation_facts(facts.to_vec(), succeeded()),
            time(at),
            |alias| alias == A,
        )
        .unwrap_or_else(|error| panic!("the memory observes: {error}"));
}

// Round 2 at NOW, after G's run settled as `terminal_state`, with `facts`.
fn round_2(
    catalog: &CompiledCatalog,
    memory: &NoProgressMemory,
    facts: Vec<ObservedFact>,
    terminal_state: TaskTerminalState,
) -> PolicyEvaluation {
    let mut memory = memory.clone();
    let facts = evaluation_facts(facts, vec![ran(GUARDED, DISPATCHED, terminal_state)]);
    memory
        .observe(catalog, &facts, time(NOW), |alias| alias == A)
        .unwrap_or_else(|error| panic!("the memory observes: {error}"));
    evaluate_round(catalog, &facts, &memory.guards(), NOW)
}

// One `schedule.json` rule running the package at every whole hour, with `loop_budget` and the
// escalation threshold given.
fn rule(rule_id: &str, loop_budget: Option<Value>, escalation_threshold: u64) -> Value {
    let mut rule = task_body(
        hourly(),
        outcome_stop_of(rule_id),
        loop_budget,
        escalation_threshold,
    );
    rule.shift_remove("instance_overrides");
    rule.insert("id".to_owned(), json!(rule_id));
    rule.insert("package_id".to_owned(), json!(PACKAGE));
    rule.insert("priority".to_owned(), json!(100));
    Value::Object(rule)
}

// Composes `schedule` in the fixture game root (its bundle lists the one package) for the one
// bound instance, without a Runtime manifest at hand.
fn compose(schedule: &Value) -> Result<StandardPackComposition, crate::StandardPackComposeFailure> {
    let bundle = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": [{
            "package_id": PACKAGE,
            "server": SERVER,
            "entry_task_id": "task",
            "path": format!("packs/{PACKAGE}"),
            "file_count": 1,
            "byte_count": 64
        }]
    }))
    .expect("bundle bytes");
    let root = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle.as_slice()),
            ..StandardPackRootBytes::default()
        },
        GAME,
    )
    .unwrap_or_else(|issue| panic!("the root reads: {issue}"))
    .expect("the root holds bundle.json");
    let schedule = serde_json::to_vec_pretty(schedule).expect("schedule bytes");
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: schedule.as_slice(),
            root: &root,
            maintenance: &[],
        },
    )]);
    let instances = BTreeMap::from([(
        SCHEDULE_INSTANCE.to_owned(),
        StandardPackBinding {
            game_id: GAME.to_owned(),
            server_id: SERVER.to_owned(),
        },
    )]);
    compose_standard_pack_rules(&StandardPackComposeInput {
        instances: &instances,
        games: &games,
        subsystems: None,
        catalog_version: 1,
    })
}

// The keys the maintenance package's decision requests, in its reasons' order.
fn requested_keys(evaluation: &PolicyEvaluation) -> Vec<String> {
    evaluation
        .decisions
        .iter()
        .filter(|decision| decision.task_id == FACT_REFRESH_TASK_ID)
        .flat_map(|decision| decision.reasons.iter())
        .filter(|reason| code_of(reason) == REQUESTED)
        .filter_map(|reason| reason.outcome.value("fact_key"))
        .filter_map(|key| key.as_str().map(str::to_owned))
        .collect()
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == task_id && decision.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("{task_id}'s decision on instance A"))
}

fn dispatched(evaluation: &PolicyEvaluation) -> Vec<&str> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| intent.task_id.as_str())
        .collect()
}

fn code_of(reason: &DecisionReason) -> &str {
    reason.outcome.code().as_str()
}

fn codes(decision: &TaskDecision) -> Vec<&str> {
    decision.reasons.iter().map(code_of).collect()
}

fn last_code(decision: &TaskDecision) -> &str {
    decision.reasons.last().map_or("", code_of)
}

fn bytes(evaluation: &PolicyEvaluation) -> Vec<u8> {
    serde_json::to_vec(evaluation).expect("the evaluation serializes")
}

// The reason's values are exactly these.
fn assert_values(reason: &DecisionReason, expected: &[(&str, Value)]) {
    let expected = expected
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect::<Map<String, Value>>();
    assert_eq!(
        reason.outcome.values(),
        &expected,
        "{}: {}",
        code_of(reason),
        reason.detail
    );
}
