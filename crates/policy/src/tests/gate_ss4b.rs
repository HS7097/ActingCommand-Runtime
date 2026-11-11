// SPDX-License-Identifier: AGPL-3.0-only

// SS-4b gate tests (register §1.310; coordinator work order `rd-1111/brief-ss4b-guard-cause.md`,
// tests 1 to 4; `contracts/scheduling/README.md`, "No-progress guard"). A no-progress guard holds
// its pair only while the pair is due for the cause the guard served: its trigger, read part by
// part as at capture and as it is true now, gives basis keys that are all keys of the guard's
// basis. A pair due by no basis key (a clock slot) or by a key outside the basis is not held, and
// its guard is released for good; the loop the guard exists for is still stopped.
//
// Neutral V2 catalogs on instance `instance-a`. Each round runs the Runtime's memory as the host
// does at every evaluated cycle: the memory observes the facts, the round evaluates with the armed
// guards, the memory forgets the guards the round released and captures the basis of each new
// intent; an admission then arms the pair's guard. Deterministic: fixed instants, no clock read.
// Every expected value comes from the coordinator's ruling in the work order, never from what the
// code does.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, instance};
use crate::{
    BasisReading, CandidateEligibility, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DecisionReason, EvaluationFacts, EvaluationResources, EvaluationTime,
    FactRefreshInputs, FactRefreshReader, FactRefreshSettings, FactValue, HostResourceSnapshot,
    InstanceHolds, NoProgressGuards, NoProgressMemory, ObservedFact, OwedReruns, PolicyEvaluation,
    PolicyEvaluationError, RemedyPlan, RunSlots, SCHEDULING_SCHEMA_VERSION_V2,
    SchedulingDecisionState, ScopeSelector, TaskDecision, TaskRuntimeSnapshot, TaskTerminalState,
    compile_catalog, evaluate_with_instance_holds, evaluate_with_no_progress,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const READER: &str = "fixture.reader";
const MINUTE_MS: u64 = 60_000;

// The chain head of the incident's shape, and the task its dependency names (never due here).
const HEAD: &str = "task.head";
const UPSTREAM: &str = "task.upstream";
// Test 2's task, and test 3's guarded task and the task that takes the instance once.
const LOOP: &str = "task.loop";
const GUARDED: &str = "task.guarded";
const BLOCKER: &str = "task.blocker";

// Two completion times the head is due at, and one no package publishes.
const FIRST_READY: &str = "resource.neutral.ready_first";
const SECOND_READY: &str = "resource.neutral.ready_second";
const NO_READY: &str = "resource.neutral.ready_none";
// Stocks and counts of the instance.
const STOCK: &str = "resource.neutral.stock";
const AMOUNT: &str = "resource.neutral.amount";
const READY_AT: &str = "resource.neutral.ready_at";
const COUNT: &str = "resource.neutral.count";
const DEMAND: &str = "resource.neutral.demand";
// The daily window the head's clock slot stands in.
const WINDOW: &str = "window.daily";

const NO_PROGRESS: &str = "policy_no_progress";
const TRIGGER_FALSE: &str = "trigger_false";
const TIMELINE_ACTIVE: &str = "timeline_active";

// A whole hour; every instant below is counted from it, so the hourly clock occurs at TOP and at
// TOP + 1 h.
const TOP: u64 = 1_000 * HOUR_MS;
// Every reading of round 1, observed half an hour before TOP.
const READ_AT: u64 = TOP - 30 * MINUTE_MS;
// The upstream task's settled run.
const UPSTREAM_RAN: u64 = TOP - 2 * HOUR_MS;
// The head's run before round 1, after the hour's clock occurrence: the slot at TOP is served.
const EARLIER_RUN: u64 = TOP + 5 * MINUTE_MS;
// The first completion time, reached before round 1.
const FIRST_TIME: u64 = TOP + 8 * MINUTE_MS;
// Round 1 evaluates, its dispatch is admitted, and the run succeeds.
const ROUND_1: u64 = TOP + 10 * MINUTE_MS;
const DISPATCHED: u64 = TOP + 11 * MINUTE_MS;
// The first completion time read again with the same time, after the dispatch.
const REREAD_AT: u64 = TOP + 15 * MINUTE_MS;
// Neither a new completion time nor a clock slot.
const NEITHER_AT: u64 = TOP + 20 * MINUTE_MS;
// The second completion time, and an evaluation just after it.
const SECOND_TIME: u64 = TOP + 30 * MINUTE_MS;
const SECOND_DUE_AT: u64 = TOP + 31 * MINUTE_MS;
// A second completion time that comes only after the next clock slot.
const LATE_SECOND_TIME: u64 = TOP + 3 * HOUR_MS;
// Just after the next clock occurrence, and the one after it.
const NEXT_SLOT_AT: u64 = TOP + HOUR_MS + MINUTE_MS;
const SLOT_AFTER_AT: u64 = TOP + 2 * HOUR_MS + MINUTE_MS;

// Test 1, the incident in its real shape. The head's trigger is an `any` of a clock slot (an
// hourly clock beside a daily window) and, after it, an `all` of an `any` of two completion
// times (`once_per_reading`) and a dependency. Round 1 dispatches it for the first time: the
// first true child of the `any` is the `all`, and in it the first completion time, so the guard
// is armed with that one key. No reading changes after its successful run.
// (a) The second completion time comes: the head is due for a key outside the basis, so it is not
// held but dispatched, the guard is released, and the new dispatch arms a guard on the second
// time. (b) Instead the next clock slot comes: due by no basis key, the head is dispatched and
// its dispatch arms no guard. (c) Neither: the head is not due (the first time is served); with
// the first time read again at the same time, it is due for the served cause and held as SS-4
// holds it, its reason naming the admission's reading.
#[test]
fn ss4b_t1_the_incident_shape_runs_at_a_new_completion_time_and_at_the_next_clock_slot() {
    let catalog = head_catalog();

    // (a) The second completion time.
    let mut memory = armed_head(&catalog, SECOND_TIME);
    let facts = after_head_run(SECOND_TIME, false);
    let round = cycle(&catalog, &mut memory, &facts, SECOND_DUE_AT);
    assert_eq!(dispatched(&round), [HEAD], "(a) {round:#?}");
    assert!(
        !codes(decision(&round, HEAD)).contains(&NO_PROGRESS),
        "(a) {:?}",
        codes(decision(&round, HEAD))
    );
    assert!(memory.armed(HEAD, A).is_none(), "(a) the guard is released");
    admit(&mut memory, &round, HEAD, SECOND_DUE_AT + MINUTE_MS);
    let guard = memory
        .armed(HEAD, A)
        .expect("(a) the new dispatch arms a guard");
    assert_eq!(guard.dispatched_at_unix_ms, SECOND_DUE_AT + MINUTE_MS);
    assert_eq!(
        guard.basis.readings,
        [BasisReading {
            scope: instance_scope(),
            fact_key: SECOND_READY.to_owned(),
            value: Some(FactValue::TimestampMs(SECOND_TIME)),
            observed_at_unix_ms: Some(READ_AT),
        }],
        "(a) the new guard holds the second completion time"
    );

    // (b) The next clock slot, the second completion time still ahead.
    let mut memory = armed_head(&catalog, LATE_SECOND_TIME);
    let facts = after_head_run(LATE_SECOND_TIME, false);
    let round = cycle(&catalog, &mut memory, &facts, NEXT_SLOT_AT);
    assert_eq!(dispatched(&round), [HEAD], "(b) {round:#?}");
    let due = codes(decision(&round, HEAD));
    assert!(!due.contains(&NO_PROGRESS), "(b) {due:?}");
    assert!(due.contains(&TIMELINE_ACTIVE), "(b) {due:?}");
    assert!(memory.armed(HEAD, A).is_none(), "(b) the guard is released");
    admit(&mut memory, &round, HEAD, NEXT_SLOT_AT + MINUTE_MS);
    assert!(
        memory.armed(HEAD, A).is_none(),
        "(b) a dispatch the clock slot decided arms no guard"
    );

    // (c) Neither: not due.
    let mut memory = armed_head(&catalog, SECOND_TIME);
    let facts = after_head_run(SECOND_TIME, false);
    let round = cycle(&catalog, &mut memory, &facts, NEITHER_AT);
    assert!(dispatched(&round).is_empty(), "(c) {round:#?}");
    assert_eq!(last_code(decision(&round, HEAD)), TRIGGER_FALSE);
    assert!(memory.armed(HEAD, A).is_some(), "(c) the guard stays armed");

    // (c) The first completion time read again, the same time: due for the served cause, held.
    let facts = after_head_run(SECOND_TIME, true);
    let round = cycle(&catalog, &mut memory, &facts, NEITHER_AT);
    assert!(dispatched(&round).is_empty(), "(c) {round:#?}");
    let held = decision(&round, HEAD);
    assert_eq!(held.state, SchedulingDecisionState::Deferred);
    assert_eq!(last_code(held), NO_PROGRESS);
    assert_values(
        held.reasons.last().expect("a reason"),
        &[
            (
                "basis_readings",
                json!([{
                    "fact_key": FIRST_READY,
                    "reading_value": format!("{{\"type\":\"timestamp_ms\",\"value\":{FIRST_TIME}}}"),
                    "observed_at_unix_ms": READ_AT,
                }]),
            ),
            ("served_by_dispatch_unix_ms", json!(DISPATCHED)),
        ],
    );
    assert!(memory.armed(HEAD, A).is_some(), "(c) the guard stays armed");
}

// Test 2, the loop SS-4 exists for: a pair due on a stock reading, run once, no reading changed
// since, is held at every later evaluation; so is one whose stock reading stands in an `all`
// beside an hourly clock, at each later occurrence of the clock (between occurrences it is not
// due). Each round names the one basis key with the admission's reading, and the guard stays.
#[test]
fn ss4b_t2_a_run_without_progress_is_still_held_beside_a_due_clock() {
    let cases: [(&str, Value, &[u64]); 2] = [
        (
            "a stock reading",
            stock_trigger(STOCK),
            &[NEITHER_AT, NEXT_SLOT_AT, SLOT_AFTER_AT],
        ),
        (
            "a clock beside a stock reading",
            json!({"kind": "all", "predicates": [hourly(), stock_trigger(STOCK)]}),
            &[NEXT_SLOT_AT, SLOT_AFTER_AT],
        ),
    ];
    for (case, trigger, evaluated_at) in cases {
        let catalog = neutral_catalog(json!([task(LOOP, 200, trigger)]), json!([]));
        let readings = vec![integer(STOCK, 5, READ_AT)];
        let mut memory = NoProgressMemory::default();
        let first = cycle(
            &catalog,
            &mut memory,
            &evaluation_facts(readings.clone(), Vec::new()),
            ROUND_1,
        );
        assert_eq!(dispatched(&first), [LOOP], "{case}: round 1");
        admit(&mut memory, &first, LOOP, DISPATCHED);
        assert!(memory.armed(LOOP, A).is_some(), "{case}: armed");

        let settled = vec![ran(LOOP, DISPATCHED, TaskTerminalState::Succeeded)];
        for &at in evaluated_at {
            let round = cycle(
                &catalog,
                &mut memory,
                &evaluation_facts(readings.clone(), settled.clone()),
                at,
            );
            assert!(dispatched(&round).is_empty(), "{case} at {at}: {round:#?}");
            let held = decision(&round, LOOP);
            assert_eq!(last_code(held), NO_PROGRESS, "{case} at {at}");
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
                }])),
                "{case} at {at}"
            );
            assert!(
                memory.armed(LOOP, A).is_some(),
                "{case} at {at}: the guard stays"
            );
        }
    }
}

// Test 3. The guarded task is due by an `any` of (1) a completion time beside an amount, (2) a
// count, (3) the amount alone. Round 1 is decided by (1): the guard holds the amount and the
// completion time. After the successful run the time is served, and the pair is due by (3), the
// amount alone, a subset of the basis: held. Then the count reads 2: due by (2), a key outside
// the basis: not held and released, though another task takes the instance this round. Then
// the count reads 0 again and the completion time is read again with the same time, so (1), the
// served cause itself, is due: the pair is dispatched, while the guard as it was armed would
// still hold it.
#[test]
fn ss4b_t3_a_subset_of_the_basis_holds_and_a_key_outside_it_releases_for_good() {
    let trigger = json!({
        "kind": "any",
        "predicates": [
            {"kind": "all", "predicates": [ready_at(READY_AT), stock_trigger(AMOUNT)]},
            stock_trigger(COUNT),
            stock_trigger(AMOUNT)
        ]
    });
    let catalog = neutral_catalog(
        json!([
            task(GUARDED, 200, trigger),
            task(BLOCKER, 300, stock_trigger(DEMAND))
        ]),
        json!([]),
    );
    let readings = |count: (i64, u64), demand: (i64, u64), ready_observed_at: u64| {
        vec![
            integer(AMOUNT, 5, READ_AT),
            timestamp(READY_AT, FIRST_TIME, ready_observed_at),
            integer(COUNT, count.0, count.1),
            integer(DEMAND, demand.0, demand.1),
        ]
    };
    let mut memory = NoProgressMemory::default();
    let first = cycle(
        &catalog,
        &mut memory,
        &evaluation_facts(readings((0, READ_AT), (0, READ_AT), READ_AT), Vec::new()),
        ROUND_1,
    );
    assert_eq!(dispatched(&first), [GUARDED], "round 1");
    admit(&mut memory, &first, GUARDED, DISPATCHED);
    let as_armed = memory.guards();
    let settled = vec![ran(GUARDED, DISPATCHED, TaskTerminalState::Succeeded)];

    // Due by the amount alone, a subset of the basis: held, naming both basis keys.
    let round = cycle(
        &catalog,
        &mut memory,
        &evaluation_facts(
            readings((0, READ_AT), (0, READ_AT), READ_AT),
            settled.clone(),
        ),
        NEITHER_AT,
    );
    assert!(dispatched(&round).is_empty(), "{round:#?}");
    let held = decision(&round, GUARDED);
    assert_eq!(last_code(held), NO_PROGRESS);
    assert_eq!(
        held.reasons
            .last()
            .expect("a reason")
            .outcome
            .value("basis_readings"),
        Some(&json!([
            {
                "fact_key": AMOUNT,
                "reading_value": "{\"type\":\"integer\",\"value\":5}",
                "observed_at_unix_ms": READ_AT,
            },
            {
                "fact_key": READY_AT,
                "reading_value": format!("{{\"type\":\"timestamp_ms\",\"value\":{FIRST_TIME}}}"),
                "observed_at_unix_ms": READ_AT,
            }
        ]))
    );
    assert!(
        memory.armed(GUARDED, A).is_some(),
        "a subset keeps the guard"
    );

    // Due by the count, outside the basis: not held, released; the other task takes the instance.
    let counted_at = TOP + 24 * MINUTE_MS;
    let round = cycle(
        &catalog,
        &mut memory,
        &evaluation_facts(
            readings((2, counted_at), (1, counted_at), READ_AT),
            settled.clone(),
        ),
        TOP + 25 * MINUTE_MS,
    );
    assert_eq!(dispatched(&round), [BLOCKER], "{round:#?}");
    assert!(
        !codes(decision(&round, GUARDED)).contains(&NO_PROGRESS),
        "{:?}",
        codes(decision(&round, GUARDED))
    );
    assert!(memory.armed(GUARDED, A).is_none(), "released");

    // The served cause due again: the release stays, and the pair is dispatched.
    let again_at = TOP + 30 * MINUTE_MS;
    let read_again = TOP + 29 * MINUTE_MS;
    let again = evaluation_facts(
        readings((0, read_again), (0, read_again), TOP + 28 * MINUTE_MS),
        settled,
    );
    let round = cycle(&catalog, &mut memory, &again, again_at);
    assert_eq!(dispatched(&round), [GUARDED], "{round:#?}");
    assert!(!codes(decision(&round, GUARDED)).contains(&NO_PROGRESS));
    let by_old_guard = evaluate_round(&catalog, &again, &as_armed, again_at);
    assert!(dispatched(&by_old_guard).is_empty(), "{by_old_guard:#?}");
    assert_eq!(last_code(decision(&by_old_guard, GUARDED)), NO_PROGRESS);
}

// Test 4: with the guard switched off on the instance, and with no guard, each evaluation of
// test 1's head (neither; the served cause again; the second completion time; the next clock
// slot) is the one `evaluate_with_instance_holds` gives, byte for byte, as before this slice; the
// memory switched off keeps nothing, before and after the round.
#[test]
fn ss4b_t4_with_the_guard_off_or_no_guard_the_evaluation_is_as_before() {
    let catalog = head_catalog();
    for (case, second_time, reread, at, runs) in [
        ("neither", SECOND_TIME, false, NEITHER_AT, false),
        (
            "the served cause again",
            SECOND_TIME,
            true,
            NEITHER_AT,
            true,
        ),
        (
            "the second completion time",
            SECOND_TIME,
            false,
            SECOND_DUE_AT,
            true,
        ),
        (
            "the next clock slot",
            LATE_SECOND_TIME,
            false,
            NEXT_SLOT_AT,
            true,
        ),
    ] {
        let facts = after_head_run(second_time, reread);
        let before = evaluate_with_instance_holds::<PolicyEvaluationError>(
            &catalog,
            &facts,
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
            |_, _, _| Ok(false),
            |_| Ok(CandidateEligibility::Eligible),
        )
        .unwrap_or_else(|error| panic!("{case}: the round evaluates: {error}"));
        let expected: &[&str] = if runs { &[HEAD] } else { &[] };
        assert_eq!(dispatched(&before), expected, "{case}");

        let none = evaluate_round(&catalog, &facts, &NoProgressGuards::default(), at);
        assert_eq!(none, before, "{case}: no guard");
        assert_eq!(bytes(&none), bytes(&before), "{case}: no guard");

        let mut memory = armed_head(&catalog, second_time);
        memory
            .observe(&catalog, &facts, time(at), |_| false)
            .unwrap_or_else(|error| panic!("{case}: the memory observes: {error}"));
        let guards = memory.guards();
        assert!(guards.is_empty(), "{case}: off, the memory keeps nothing");
        let off = evaluate_round(&catalog, &facts, &guards, at);
        memory.forget_released(&guards);
        assert!(memory.guards().is_empty(), "{case}: still nothing");
        assert_eq!(off, before, "{case}: off");
        assert_eq!(bytes(&off), bytes(&before), "{case}: off");
    }
}

// ---- Fixtures ----

// The head (priority 200) and the task its dependency names (priority 100, due at a completion
// time no package publishes, so never), with the daily window of the head's clock slot.
fn head_catalog() -> CompiledCatalog {
    let trigger = json!({
        "kind": "any",
        "predicates": [
            {
                "kind": "all",
                "predicates": [hourly(), {"kind": "timeline_active", "event_id": WINDOW}]
            },
            {
                "kind": "all",
                "predicates": [
                    {"kind": "any", "predicates": [ready_at(FIRST_READY), ready_at(SECOND_READY)]},
                    {
                        "kind": "dependency_completed",
                        "task_id": UPSTREAM,
                        "terminal_states": ["succeeded"]
                    }
                ]
            }
        ]
    });
    let never = json!({
        "kind": "fact_time_reached",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": NO_READY,
        "offset_ms": 0,
        "max_age_ms": null
    });
    neutral_catalog(
        json!([task(HEAD, 200, trigger), task(UPSTREAM, 100, never)]),
        json!([daily_window()]),
    )
}

fn neutral_catalog(tasks: Value, events: Value) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.guard-cause",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-guard-cause"]
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
            format!("memory://fixture-guard-cause/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", tasks)),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([profile()]))),
        timeline: source("timeline", &document("events", events)),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

// A window of instance A that lasts all day, every day.
fn daily_window() -> Value {
    json!({
        "id": WINDOW,
        "scope": {"kind": "instance", "instance_id": A},
        "event_kind": "activity",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": DAY_MS,
            "anchor_ms": 0
        },
        "duration_ms": DAY_MS,
        "invalidates_fact_prefixes": [],
        "validity": {"from_unix_ms": 0, "until_unix_ms": null}
    })
}

fn task(task_id: &str, priority: i16, trigger: Value) -> Value {
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
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "loop_budget": {"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
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

// Due once per reading at the time instance A's `key` holds.
fn ready_at(key: &str) -> Value {
    json!({
        "kind": "fact_time_reached",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": key,
        "offset_ms": 0,
        "max_age_ms": null,
        "once_per_reading": true
    })
}

// Due while instance A's `key` is at least 1.
fn stock_trigger(key: &str) -> Value {
    json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": key,
        "comparison": "greater_than_or_equal",
        "value": {"type": "integer", "value": 1},
        "max_age_ms": null
    })
}

// One profile for the whole game, active all day on every weekday.
fn profile() -> Map<String, Value> {
    let Value::Object(profile) = json!({
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
    }) else {
        unreachable!("a JSON object")
    };
    profile
}

fn instance_scope() -> ScopeSelector {
    ScopeSelector::Instance {
        instance_id: A.to_owned(),
    }
}

fn reading(key: &str, value: FactValue, observed_at: u64) -> ObservedFact {
    ObservedFact {
        scope: instance_scope(),
        fact_key: key.to_owned(),
        value,
        observed_at_unix_ms: observed_at,
        expires_at_unix_ms: None,
        confidence_milli: 1_000,
    }
}

fn integer(key: &str, value: i64, observed_at: u64) -> ObservedFact {
    reading(key, FactValue::Integer(value), observed_at)
}

fn timestamp(key: &str, at: u64, observed_at: u64) -> ObservedFact {
    reading(key, FactValue::TimestampMs(at), observed_at)
}

// The head's two completion times: the first read at READ_AT, or read again at REREAD_AT with the
// same time; the second at `second_time`, read at READ_AT.
fn head_readings(second_time: u64, reread: bool) -> Vec<ObservedFact> {
    vec![
        timestamp(
            FIRST_READY,
            FIRST_TIME,
            if reread { REREAD_AT } else { READ_AT },
        ),
        timestamp(SECOND_READY, second_time, READ_AT),
    ]
}

// `task_id`'s latest run on instance A, dispatched at `at`.
fn ran(task_id: &str, at: u64, terminal_state: TaskTerminalState) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: task_id.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(at),
        eligible_since_unix_ms: None,
        terminal_state: Some(terminal_state),
        completed_window: None,
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
    }
}

// The facts after the head's guarded run succeeded.
fn after_head_run(second_time: u64, reread: bool) -> EvaluationFacts {
    evaluation_facts(
        head_readings(second_time, reread),
        vec![
            ran(HEAD, DISPATCHED, TaskTerminalState::Succeeded),
            ran(UPSTREAM, UPSTREAM_RAN, TaskTerminalState::Succeeded),
        ],
    )
}

// The memory after round 1: the head, whose run before it served the hour's clock slot, is
// dispatched for the first completion time, and its admission at DISPATCHED arms the guard on
// that one key.
fn armed_head(catalog: &CompiledCatalog, second_time: u64) -> NoProgressMemory {
    let mut memory = NoProgressMemory::default();
    let facts = evaluation_facts(
        head_readings(second_time, false),
        vec![
            ran(HEAD, EARLIER_RUN, TaskTerminalState::Succeeded),
            ran(UPSTREAM, UPSTREAM_RAN, TaskTerminalState::Succeeded),
        ],
    );
    let round = cycle(catalog, &mut memory, &facts, ROUND_1);
    assert_eq!(dispatched(&round), [HEAD], "round 1 dispatches the head");
    admit(&mut memory, &round, HEAD, DISPATCHED);
    let guard = memory.armed(HEAD, A).expect("the admission arms the guard");
    assert_eq!(
        guard.basis.readings,
        [BasisReading {
            scope: instance_scope(),
            fact_key: FIRST_READY.to_owned(),
            value: Some(FactValue::TimestampMs(FIRST_TIME)),
            observed_at_unix_ms: Some(READ_AT),
        }],
        "round 1's guard holds the first completion time alone"
    );
    memory
}

fn evaluation_facts(facts: Vec<ObservedFact>, tasks: Vec<TaskRuntimeSnapshot>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-guard-cause".to_owned(),
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

// The fact refresh at its starting defaults, A's maintenance package marking the stocks.
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
            marked_keys: BTreeSet::from([
                STOCK.to_owned(),
                AMOUNT.to_owned(),
                COUNT.to_owned(),
                DEMAND.to_owned(),
            ]),
            dispatched_today: 0,
            last_failure_code: None,
            problem: None,
        }],
        instance_facts_loaded_at_unix_ms: Some(TOP - DAY_MS),
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

// One evaluated cycle at `at`, as the host runs it: the memory observes `facts`, the round
// evaluates with the armed guards, the memory forgets the guards the round released and captures
// the basis of each new intent.
fn cycle(
    catalog: &CompiledCatalog,
    memory: &mut NoProgressMemory,
    facts: &EvaluationFacts,
    at: u64,
) -> PolicyEvaluation {
    memory
        .observe(catalog, facts, time(at), |alias| alias == A)
        .unwrap_or_else(|error| panic!("the memory observes: {error}"));
    let guards = memory.guards();
    let round = evaluate_round(catalog, facts, &guards, at);
    memory.forget_released(&guards);
    memory
        .capture(
            catalog,
            facts,
            &resources(),
            time(at),
            &round.dispatch_intents,
            |alias| alias == A,
        )
        .unwrap_or_else(|error| panic!("the memory captures: {error}"));
    round
}

// The admission of `task_id`'s intent of `round` at `at`, which arms the pair's guard.
fn admit(memory: &mut NoProgressMemory, round: &PolicyEvaluation, task_id: &str, at: u64) {
    let intent = round
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id)
        .unwrap_or_else(|| panic!("{task_id} has an intent"));
    memory.admit(task_id, A, &intent.decision_id, at);
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
