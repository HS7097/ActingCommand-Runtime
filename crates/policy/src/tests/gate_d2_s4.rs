// SPDX-License-Identifier: AGPL-3.0-only

// D2-S4 gate tests (Workflow #384; `rd-1009/d2/model-d2-fact-refresh.md` v1.4.2 §8, rows T18 to
// T28; policy gate key 46): the scheduler's fact refresh. A candidate that reads an unknown
// realistic instance fact is deferred under one registered reason per key, and the instance's
// recognition maintenance package R_I is requested for the keys its marks write, through the
// same allocation as every candidate. Each affected candidate takes the first row of the table
// of model §3.7 that matches. Every expected value comes from the model row named on each test.
//
// T29 (a blackout period) and T30 (the schedulable-pack state) need the gates of the
// scheduling-rules slices that bring them (SR-4b, SR-11); neither gate is on main, so neither row
// has a test here.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::{Category, Code, Outcome};
use serde_json::{Map, Value, json};

use super::selection_catalog::{
    DAY_MS, HOUR_MS, NOW, at_least, catalog, drop_candidate, fact, fact_ref, field_ref, gate,
    instance, integer_fact, selection,
};
use crate::{
    CandidateEligibility, CatalogDocumentSource, CatalogSources, CompiledCatalog,
    CompletedActivityWindow, DecisionReason, DecisionReasonChain, DispatchIntent, EvaluationFacts,
    EvaluationResources, EvaluationTime, FACT_REFRESH_OPERATION_ID, FACT_REFRESH_PROCEDURE_REF,
    FACT_REFRESH_TASK_ID, FactRefreshInputs, FactRefreshReader, FactRefreshSettings, FactValue,
    HostResourceSnapshot, InstanceHold, InstanceSnapshot, ObservedFact, PolicyEvaluation,
    PolicyEvaluationError, SchedulingDecisionState, TaskDecision, TaskRuntimeSnapshot,
    TaskTerminalState, compile_catalog, evaluate_with_fact_refresh,
};

const ALPHA: &str = "task.alpha";
const X: &str = "task.x";
const Y: &str = "task.y";
const A: &str = "instance-a";
const B: &str = "instance-b";
// A realistic key: an instance fact recognition writes (model §3.1).
const STOCK: &str = "resource.stock";
// An instruction key nobody gave for A.
const MANUAL: &str = "session.manual_request";
// A's recognition maintenance package R_I; its marks write STOCK.
const READER: &str = "package.reader-a";
const REQUESTED: &str = "fact_refresh_requested";
const PENDING: &str = "fact_refresh_pending";
const UNRESOLVED: &str = "fact_refresh_unresolved";
const EXHAUSTED: &str = "fact_refresh_exhausted";
const COOLDOWN: &str = "task_cooldown_active";
const ALREADY_SELECTED: &str = "instance_already_selected";
const HOST_BUDGET: &str = "host_budget_deferred";
const TRIGGER_UNKNOWN: &str = "trigger_unknown";
// The closure's positions 1-2, registered by runtime-host (model-369-queue.md W-1; Workflow
// #191 ps1).
const HELD: &str = "dispatch_instance_held";
const PAUSED: &str = "dispatch_paused_instance";
// A failure a navigation package's run ends with (the contract's registered code).
const NAVIGATION_FAILED: &str = "navigation_arrival_failed";
// The starting defaults of `scheduling.fact_refresh.*` (model §3.4).
const MIN_INTERVAL_MS: u64 = 300_000;
const DAILY_LIMIT: u32 = 96;
const BACKOFF_MS: u64 = 600_000;
const MAX_FAILURES: u32 = 3;
// The fixture's activity profile is open all day at UTC+0: the next activity day starts at the
// next midnight after NOW.
const NEXT_DAY: u64 = (NOW / DAY_MS + 1) * DAY_MS;

// T18 (D2M:598; §3.4; 簿 L704): A's K has expired, A's reader is bound and A is free. A's one
// intent this round is R_I, requested for {K} with `fact_refresh_requested` in its reason chain
// (with the document rule that read K, §7.1); A's candidate that reads K waits under
// `fact_refresh_pending` naming R_I; B runs as usual.
#[test]
fn gate_d2_t18_an_unknown_key_dispatches_the_reader_and_its_candidate_waits_for_it() {
    let evaluation = evaluate_stock(&inputs(vec![reader(0, None)]), Vec::new(), NOW);

    let on_a = intents_on(&evaluation, A);
    assert_eq!(on_a.len(), 1, "{:#?}", evaluation.dispatch_intents);
    let refresh = on_a[0];
    assert_eq!(refresh.task_id, FACT_REFRESH_TASK_ID);
    assert_eq!(refresh.procedure_ref, FACT_REFRESH_PROCEDURE_REF);
    assert_eq!(refresh.operation_id, FACT_REFRESH_OPERATION_ID);
    let requested = chain_of(&evaluation, refresh)
        .reasons
        .iter()
        .filter(|reason| code_of(reason) == REQUESTED)
        .collect::<Vec<_>>();
    assert_eq!(requested.len(), 1, "the key set is {{K}}");
    assert_values(
        requested[0],
        &[
            ("instance_id", json!(A)),
            ("fact_key", json!(STOCK)),
            ("unknown_reason", json!("fact_expired")),
            ("rule_id", json!("gate-stock")),
        ],
    );

    let waiting = decision(&evaluation, ALPHA, A);
    assert_eq!(waiting.state, SchedulingDecisionState::Deferred);
    assert_eq!(refresh_codes(waiting), [PENDING]);
    assert_values(coded(waiting, PENDING)[0], &pending(None, None, None));
    assert!(has_intent(&evaluation, ALPHA, B), "B runs as usual");
    assert!(refresh_codes(decision(&evaluation, ALPHA, B)).is_empty());
}

// T19 (D2M:599; §3.8): the round after R_I's reading landed (A's K fresh again, R_I settled just
// before) scores A's candidate as usual and dispatches it; no reason of the fact refresh is left.
#[test]
fn gate_d2_t19_the_round_after_the_reading_scores_and_dispatches_the_candidate() {
    let catalog = catalog(&[(ALPHA, 100)], Some(&stock_document()));
    let evaluation = evaluate(
        &catalog,
        &[instance(A, &[]), instance(B, &[])],
        vec![fresh_stock(A, NOW - 30_000), fresh_stock(B, NOW - 60_000)],
        vec![refresh_state(
            NOW - 90_000,
            TaskTerminalState::Succeeded,
            Some(NOW - 60_000),
            0,
        )],
        &inputs(vec![reader(1, None)]),
        NOW,
    );
    assert!(has_intent(&evaluation, ALPHA, A));
    assert!(refresh_codes(decision(&evaluation, ALPHA, A)).is_empty());
    assert!(
        evaluation
            .decisions
            .iter()
            .all(|decision| decision.task_id != FACT_REFRESH_TASK_ID),
        "nothing requests the reader"
    );
}

// T20 (D2M:600; §3.4; #369 W-1; 调 §5.2, v1.3 核 18): A is held by a ladder rung or a direct run:
// A's candidate carries only `dispatch_instance_held`, no reason of the fact refresh, and no R_I
// is planned or dispatched. The round the key returns dispatches R_I. A paused instance alike.
#[test]
fn gate_d2_t20_a_held_or_paused_instance_raises_no_demand_until_its_key_returns() {
    for hold in [HELD, PAUSED] {
        let mut held = inputs(vec![reader(0, None)]);
        held.instance_holds
            .insert(A.to_owned(), instance_hold(hold));
        let evaluation = evaluate_stock(&held, Vec::new(), NOW);
        let waiting = decision(&evaluation, ALPHA, A);
        assert_eq!(waiting.state, SchedulingDecisionState::Deferred);
        assert_eq!(last_code(waiting), hold, "{hold}");
        assert!(refresh_codes(waiting).is_empty(), "{hold}");
        assert!(intents_on(&evaluation, A).is_empty(), "{hold}");
        assert!(
            refresh_decision(&evaluation, A).is_none(),
            "{hold}: no R_I candidate"
        );
        assert!(has_intent(&evaluation, ALPHA, B), "{hold}");
    }

    let released = evaluate_stock(&inputs(vec![reader(0, None)]), Vec::new(), NOW);
    assert_eq!(refresh_intents_on(&released, A), 1);
}

// T21 (D2M:601; §3.9 item 5; D1): within `min_interval_ms` of R_I's last dispatch (which read K;
// K has since expired) A's candidate waits, `fact_refresh_pending` with the interval's gate and its
// end as the wake. Rounds the interval does not end (B's facts changing) dispatch nothing on A;
// the round at its end dispatches exactly one R_I.
#[test]
fn gate_d2_t21_the_minimum_interval_holds_the_reader_back_until_it_ends() {
    let dispatched = NOW - 60_000;
    let interval_end = dispatched + MIN_INTERVAL_MS;
    let tasks = || {
        vec![refresh_state(
            dispatched,
            TaskTerminalState::Succeeded,
            Some(dispatched + 10_000),
            0,
        )]
    };
    // K was written by that run and has expired since.
    let read_then_expired = || fact(A, STOCK, 5, dispatched + 20_000, Some(NOW - 1_000));

    let evaluation = evaluate(
        &stock_catalog(),
        &[instance(A, &[]), instance(B, &[])],
        vec![read_then_expired(), fresh_stock(B, NOW - 60_000)],
        tasks(),
        &inputs(vec![reader(1, None)]),
        NOW,
    );
    assert!(intents_on(&evaluation, A).is_empty());
    assert_eq!(
        last_code(refresh_decision(&evaluation, A).expect("R_I's decision")),
        COOLDOWN
    );
    let waiting = decision(&evaluation, ALPHA, A);
    assert_eq!(refresh_codes(waiting), [PENDING]);
    assert_values(
        coded(waiting, PENDING)[0],
        &pending(Some(COOLDOWN), None, Some(interval_end)),
    );
    assert!(
        evaluation
            .next_wake_unix_ms
            .is_some_and(|wake| wake <= interval_end)
    );

    for (round, at) in [NOW + 60_000, NOW + 120_000, NOW + 180_000]
        .into_iter()
        .enumerate()
    {
        let mut changed = fresh_stock(B, at - 1_000);
        changed.value = FactValue::Integer(10 + i64::try_from(round).expect("round"));
        let evaluation = evaluate(
            &stock_catalog(),
            &[instance(A, &[]), instance(B, &[])],
            vec![read_then_expired(), changed],
            tasks(),
            &inputs(vec![reader(1, None)]),
            at,
        );
        assert!(intents_on(&evaluation, A).is_empty(), "round at {at}");
    }

    let evaluation = evaluate(
        &stock_catalog(),
        &[instance(A, &[]), instance(B, &[])],
        vec![read_then_expired(), fresh_stock(B, NOW - 60_000)],
        tasks(),
        &inputs(vec![reader(1, None)]),
        interval_end,
    );
    assert_eq!(intents_on(&evaluation, A).len(), 1);
    assert_eq!(refresh_intents_on(&evaluation, A), 1);
}

// T22 (D2M:602; §3.7 row 4): R_I's dispatches today reached `daily_limit`: A's candidate is
// `fact_refresh_exhausted(refresh_limit=daily_limit)`, waking the next day; B is unaffected. With
// the limit raised the next round dispatches R_I, without a restart.
#[test]
fn gate_d2_t22_the_daily_limit_exhausts_the_reader_until_the_next_day_or_a_higher_limit() {
    let tasks = vec![refresh_state(
        NOW - HOUR_MS,
        TaskTerminalState::Succeeded,
        Some(NOW - HOUR_MS + 30_000),
        0,
    )];
    let evaluation = evaluate_stock(&inputs(vec![reader(DAILY_LIMIT, None)]), tasks.clone(), NOW);
    assert!(intents_on(&evaluation, A).is_empty());
    let waiting = decision(&evaluation, ALPHA, A);
    assert_eq!(refresh_codes(waiting), [EXHAUSTED]);
    assert_values(
        coded(waiting, EXHAUSTED)[0],
        &exhausted("daily_limit", Some(NEXT_DAY)),
    );
    assert!(has_intent(&evaluation, ALPHA, B));

    let mut raised = inputs(vec![reader(DAILY_LIMIT, None)]);
    raised.settings.daily_limit = DAILY_LIMIT + 1;
    let evaluation = evaluate_stock(&raised, tasks, NOW);
    assert_eq!(refresh_intents_on(&evaluation, A), 1);
}

// T23 (D2M:603; §3.7 row 5): R_I's latest dispatch settled successfully after L. (a) K missing, or
// its record older than that dispatch: `fact_refresh_unresolved`, waking when the interval ends;
// the round the interval ends selects R_I again: `fact_refresh_pending`. (b) K read after the
// dispatch but below the document's confidence floor: unresolved. A record R_I wrote that merely
// expired: pending.
#[test]
fn gate_d2_t23_a_successful_reader_that_left_the_key_unknown_is_unresolved() {
    let dispatched = NOW - 60_000;
    let interval_end = dispatched + MIN_INTERVAL_MS;
    let tasks = || {
        vec![refresh_state(
            dispatched,
            TaskTerminalState::Succeeded,
            Some(dispatched + 10_000),
            0,
        )]
    };
    let round = |a_stock: Option<ObservedFact>, at: u64| {
        let mut facts = vec![fresh_stock(B, NOW - 60_000)];
        facts.extend(a_stock);
        evaluate(
            &stock_catalog(),
            &[instance(A, &[]), instance(B, &[])],
            facts,
            tasks(),
            &inputs(vec![reader(1, None)]),
            at,
        )
    };

    for (case, a_stock, unknown_reason) in [
        ("missing", None, "fact_missing"),
        (
            "older than the dispatch",
            Some(fact(
                A,
                STOCK,
                5,
                dispatched - 60_000,
                Some(dispatched - 30_000),
            )),
            "fact_expired",
        ),
        (
            "below the floor",
            Some(ObservedFact {
                confidence_milli: 100,
                ..fact(A, STOCK, 5, dispatched + 30_000, Some(NOW + HOUR_MS))
            }),
            "fact_low_confidence",
        ),
    ] {
        let evaluation = round(a_stock, NOW);
        let waiting = decision(&evaluation, ALPHA, A);
        assert_eq!(refresh_codes(waiting), [UNRESOLVED], "{case}");
        assert_values(
            coded(waiting, UNRESOLVED)[0],
            &[
                ("instance_id", json!(A)),
                ("fact_key", json!(STOCK)),
                ("unknown_reason", json!(unknown_reason)),
                ("package_id", json!(READER)),
                ("next_eligible_unix_ms", json!(interval_end)),
            ],
        );
    }

    let evaluation = round(None, interval_end);
    assert_eq!(refresh_intents_on(&evaluation, A), 1);
    let waiting = decision(&evaluation, ALPHA, A);
    assert_eq!(refresh_codes(waiting), [PENDING]);
    assert_values(
        coded(waiting, PENDING)[0],
        &[
            ("instance_id", json!(A)),
            ("fact_key", json!(STOCK)),
            ("unknown_reason", json!("fact_missing")),
            ("package_id", json!(READER)),
        ],
    );

    let expired = round(
        Some(fact(A, STOCK, 5, dispatched + 20_000, Some(NOW - 1_000))),
        NOW,
    );
    assert_eq!(refresh_codes(decision(&expired, ALPHA, A)), [PENDING]);
}

// T24 (D2M:604; §3.9 item 1): R_I's navigation failed: it backs off for `failure_backoff_ms` and
// A's candidate waits, `fact_refresh_pending` with the failure's code; failures in a row up to
// `max_consecutive_failures` today exhaust it, `refresh_limit=failure_pause`. The evaluation
// never fails.
#[test]
fn gate_d2_t24_a_failed_reader_backs_off_then_rests_after_its_failures_in_a_row() {
    let failure = Code::relayed(NAVIGATION_FAILED).expect("a registered code");
    let dispatched = NOW - 120_000;
    let failed_at = NOW - 100_000;
    let failed = |streak| {
        vec![refresh_state(
            dispatched,
            TaskTerminalState::Failed,
            Some(failed_at),
            streak,
        )]
    };

    let evaluation = evaluate_stock(&inputs(vec![reader(1, Some(failure))]), failed(1), NOW);
    assert!(intents_on(&evaluation, A).is_empty());
    let waiting = decision(&evaluation, ALPHA, A);
    assert_eq!(refresh_codes(waiting), [PENDING]);
    assert_values(
        coded(waiting, PENDING)[0],
        &pending(
            Some(COOLDOWN),
            Some(NAVIGATION_FAILED),
            Some(failed_at + BACKOFF_MS),
        ),
    );

    let streak = u16::try_from(MAX_FAILURES).expect("streak");
    let evaluation = evaluate_stock(&inputs(vec![reader(3, Some(failure))]), failed(streak), NOW);
    assert!(intents_on(&evaluation, A).is_empty());
    let waiting = decision(&evaluation, ALPHA, A);
    assert_eq!(refresh_codes(waiting), [EXHAUSTED]);
    assert_values(
        coded(waiting, EXHAUSTED)[0],
        &exhausted("failure_pause", Some(NEXT_DAY)),
    );
}

// T25 (D2M:605; §3.3, catalog predicates): task X's trigger reads realistic key K in A's own scope
// and K is past its bound: X waits under `fact_refresh_pending` and R_I is dispatched. A trigger
// that reads a missing `session.` manual request keeps only `trigger_unknown`: no demand, no
// warning, no dispatch. One that reads K in a server's scope stays as today.
#[test]
fn gate_d2_t25_a_catalog_predicate_on_an_unknown_key_of_its_instance_requests_the_reader() {
    let stale = fact(A, STOCK, 5, NOW - 2 * HOUR_MS, None);
    let instance_stock = trigger_fact(json!({"kind": "instance", "instance_id": A}), STOCK);
    let evaluation = evaluate(
        &predicate_catalog(instance_stock),
        &[instance(A, &[])],
        vec![stale.clone()],
        Vec::new(),
        &inputs(vec![reader(0, None)]),
        NOW,
    );
    let waiting = decision(&evaluation, X, A);
    assert_eq!(waiting.state, SchedulingDecisionState::Deferred);
    assert!(codes(waiting).contains(&TRIGGER_UNKNOWN));
    assert_eq!(refresh_codes(waiting), [PENDING]);
    assert_values(
        coded(waiting, PENDING)[0],
        &[
            ("instance_id", json!(A)),
            ("fact_key", json!(STOCK)),
            ("unknown_reason", json!("fact_stale")),
            ("package_id", json!(READER)),
        ],
    );
    assert_eq!(refresh_intents_on(&evaluation, A), 1);

    for (case, trigger) in [
        (
            "an instruction key",
            trigger_fact(json!({"kind": "instance", "instance_id": A}), MANUAL),
        ),
        (
            "a server's scope",
            trigger_fact(json!({"kind": "server", "server_id": "server-a"}), STOCK),
        ),
    ] {
        let evaluation = evaluate(
            &predicate_catalog(trigger),
            &[instance(A, &[])],
            vec![stale.clone()],
            Vec::new(),
            &inputs(vec![reader(0, None)]),
            NOW,
        );
        let waiting = decision(&evaluation, X, A);
        assert_eq!(codes(waiting), [TRIGGER_UNKNOWN], "{case}");
        assert!(evaluation.dispatch_intents.is_empty(), "{case}");
        assert!(refresh_decision(&evaluation, A).is_none(), "{case}");
    }
}

// T26 (D2M:606; §3.4, rank): on A, task X reads K (unknown) and task Y does not. With Y ranked
// higher Y is dispatched and X waits for R_I; with X ranked higher R_I, which inherits X's rank, is
// dispatched. Neither order is fixed in advance.
#[test]
fn gate_d2_t26_the_reader_competes_with_the_rank_of_the_candidate_it_unlocks() {
    // Y is A's preferred task, so its affinity decides the gate without reading K.
    let document = selection(
        vec![json!({"name": "instance.affinity", "value_type": {"type": "boolean"}})],
        vec![integer_fact(STOCK, DAY_MS)],
        vec![gate(
            "gate-stock",
            json!({"kind": "any", "of": [
                {"kind": "boolean_equals", "value": field_ref("instance.affinity"), "expected": true},
                at_least(fact_ref(STOCK), 1)
            ]}),
            drop_candidate(),
        )],
        Vec::new(),
    );
    let round = |x_priority: i16, y_priority: i16| {
        evaluate(
            &catalog(&[(X, x_priority), (Y, y_priority)], Some(&document)),
            &[instance(A, &[Y])],
            vec![expired_stock(A)],
            Vec::new(),
            &inputs(vec![reader(0, None)]),
            NOW,
        )
    };

    let evaluation = round(100, 200);
    let on_a = intents_on(&evaluation, A);
    assert_eq!(on_a.len(), 1);
    assert_eq!(on_a[0].task_id, Y);
    assert_eq!(
        last_code(refresh_decision(&evaluation, A).expect("R_I's decision")),
        ALREADY_SELECTED
    );
    assert_values(
        coded(decision(&evaluation, X, A), PENDING)[0],
        &pending(Some(ALREADY_SELECTED), None, None),
    );

    let evaluation = round(200, 100);
    let on_a = intents_on(&evaluation, A);
    assert_eq!(on_a.len(), 1);
    assert_eq!(on_a[0].task_id, FACT_REFRESH_TASK_ID);
    assert_eq!(last_code(decision(&evaluation, Y, A)), ALREADY_SELECTED);
    assert_values(
        coded(decision(&evaluation, X, A), PENDING)[0],
        &pending(None, None, None),
    );
}

// T27 (D2M:607; §3.11; 调 §5.2, v1.3 核 18): after a restart (L later than R_I's last success) K is
// missing and A is held by its startup package: A's candidate carries only
// `dispatch_instance_held`: no demand, no `fact_refresh_unresolved`, no warning. The round its key
// returns dispatches R_I.
#[test]
fn gate_d2_t27_a_restart_held_by_the_startup_package_waits_quietly_then_reads() {
    let tasks = || {
        vec![refresh_state(
            NOW - 3 * HOUR_MS,
            TaskTerminalState::Succeeded,
            Some(NOW - 3 * HOUR_MS + 30_000),
            0,
        )]
    };
    let restarted = |held: bool| {
        let mut inputs = inputs(vec![reader(0, None)]);
        inputs.instance_facts_loaded_at_unix_ms = Some(NOW - 10_000);
        if held {
            inputs
                .instance_holds
                .insert(A.to_owned(), instance_hold(HELD));
        }
        evaluate(
            &stock_catalog(),
            &[instance(A, &[]), instance(B, &[])],
            vec![fresh_stock(B, NOW - 60_000)],
            tasks(),
            &inputs,
            NOW,
        )
    };

    let evaluation = restarted(true);
    let waiting = decision(&evaluation, ALPHA, A);
    assert_eq!(last_code(waiting), HELD);
    assert!(refresh_codes(waiting).is_empty());
    assert!(
        waiting
            .reasons
            .iter()
            .all(|reason| reason.outcome.category() != Category::Warning)
    );
    assert!(refresh_decision(&evaluation, A).is_none());

    let evaluation = restarted(false);
    assert_eq!(refresh_intents_on(&evaluation, A), 1);
    assert_eq!(refresh_codes(decision(&evaluation, ALPHA, A)), [PENDING]);
}

// T28 (D2M:608; §3.7 row 6): A and B share a host with room for one run (cpu 250 of the task's 200,
// R_I's 100). B's candidate ranks ahead and takes it: R_I is `host_budget_deferred` and A's
// candidate waits under `fact_refresh_pending` with that gate, without a warning.
#[test]
fn gate_d2_t28_a_reader_the_host_budget_defers_leaves_its_candidate_pending() {
    let shared = |id: &str, preferred: &[&str]| InstanceSnapshot {
        host_id: "host.shared".to_owned(),
        ..instance(id, preferred)
    };
    let evaluation = evaluate_on(
        &stock_catalog(),
        &[shared(A, &[]), shared(B, &[ALPHA])],
        vec![expired_stock(A), fresh_stock(B, NOW - 60_000)],
        Vec::new(),
        &inputs(vec![reader(0, None)]),
        NOW,
        vec![host("host.shared", 250)],
    );
    assert!(has_intent(&evaluation, ALPHA, B));
    assert!(intents_on(&evaluation, A).is_empty());
    assert_eq!(
        last_code(refresh_decision(&evaluation, A).expect("R_I's decision")),
        HOST_BUDGET
    );
    let waiting = decision(&evaluation, ALPHA, A);
    assert_values(
        coded(waiting, PENDING)[0],
        &pending(Some(HOST_BUDGET), None, None),
    );
    assert!(
        waiting
            .reasons
            .iter()
            .all(|reason| reason.outcome.category() != Category::Warning)
    );
}

// The fact refresh settings at their starting defaults (model §3.4).
fn settings() -> FactRefreshSettings {
    FactRefreshSettings {
        enabled: true,
        min_interval_ms: MIN_INTERVAL_MS,
        daily_limit: DAILY_LIMIT,
        failure_backoff_ms: BACKOFF_MS,
        max_consecutive_failures: MAX_FAILURES,
    }
}

// A's reader, with its dispatches today and the code of its latest failure.
fn reader(dispatched_today: u32, last_failure_code: Option<Code>) -> FactRefreshReader {
    FactRefreshReader {
        instance_id: A.to_owned(),
        package_id: READER.to_owned(),
        marked_keys: BTreeSet::from([STOCK.to_owned()]),
        dispatched_today,
        last_failure_code,
        problem: None,
    }
}

// The readers with the default settings; the facts were loaded a day before NOW.
fn inputs(readers: Vec<FactRefreshReader>) -> FactRefreshInputs {
    FactRefreshInputs {
        settings: settings(),
        readers,
        instance_facts_loaded_at_unix_ms: Some(NOW - DAY_MS),
        instance_holds: BTreeMap::new(),
    }
}

// What the closure's positions 1-2 hold an instance with, under the code they would give.
fn instance_hold(spelling: &str) -> InstanceHold {
    InstanceHold {
        reason: DecisionReason {
            outcome: Outcome::relayed(spelling),
            detail: "held as the cycle starts".to_owned(),
        },
        next_wake_unix_ms: None,
    }
}

// R_I's clocks on A: its latest dispatch, how it settled and when, and its failures in a row.
fn refresh_state(
    dispatched_at: u64,
    terminal: TaskTerminalState,
    completed_at: Option<u64>,
    failure_streak: u16,
) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: FACT_REFRESH_TASK_ID.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(dispatched_at),
        eligible_since_unix_ms: None,
        terminal_state: Some(terminal),
        completed_window: completed_at.map(|completed_at_unix_ms| CompletedActivityWindow {
            window_id: "fixture-activity:window".to_owned(),
            completed_at_unix_ms,
        }),
        last_duration_ms: None,
        failure_streak: Some(failure_streak),
    }
}

// One gate on the realistic key K through a `fact` source.
fn stock_document() -> Value {
    selection(
        Vec::new(),
        vec![integer_fact(STOCK, DAY_MS)],
        vec![gate(
            "gate-stock",
            at_least(fact_ref(STOCK), 1),
            drop_candidate(),
        )],
        Vec::new(),
    )
}

fn stock_catalog() -> CompiledCatalog {
    catalog(&[(ALPHA, 100)], Some(&stock_document()))
}

// A's K has expired.
fn expired_stock(instance_id: &str) -> ObservedFact {
    fact(
        instance_id,
        STOCK,
        5,
        NOW - 2 * HOUR_MS,
        Some(NOW - HOUR_MS),
    )
}

fn fresh_stock(instance_id: &str, observed_at: u64) -> ObservedFact {
    fact(
        instance_id,
        STOCK,
        5,
        observed_at,
        Some(observed_at + HOUR_MS),
    )
}

// Instances A (K expired) and B (K fresh) under the stock document, with R_I's clocks `tasks`.
fn evaluate_stock(
    inputs: &FactRefreshInputs,
    tasks: Vec<TaskRuntimeSnapshot>,
    now: u64,
) -> PolicyEvaluation {
    evaluate(
        &stock_catalog(),
        &[instance(A, &[]), instance(B, &[])],
        vec![expired_stock(A), fresh_stock(B, NOW - 60_000)],
        tasks,
        inputs,
        now,
    )
}

// Every instance on its own host; the Runtime admits every candidate.
fn evaluate(
    catalog: &CompiledCatalog,
    instances: &[InstanceSnapshot],
    facts: Vec<ObservedFact>,
    tasks: Vec<TaskRuntimeSnapshot>,
    inputs: &FactRefreshInputs,
    now: u64,
) -> PolicyEvaluation {
    let hosts = instances
        .iter()
        .map(|instance| host(&instance.host_id, 1_000))
        .collect();
    evaluate_on(catalog, instances, facts, tasks, inputs, now, hosts)
}

fn evaluate_on(
    catalog: &CompiledCatalog,
    instances: &[InstanceSnapshot],
    facts: Vec<ObservedFact>,
    tasks: Vec<TaskRuntimeSnapshot>,
    inputs: &FactRefreshInputs,
    now: u64,
    hosts: Vec<HostResourceSnapshot>,
) -> PolicyEvaluation {
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-fact-refresh".to_owned(),
        facts,
        outcomes: Vec::new(),
        tasks,
        instances: instances.to_vec(),
        priority_offsets: Vec::new(),
    };
    let resources = EvaluationResources {
        pools: Vec::new(),
        hosts,
    };
    let time = EvaluationTime {
        unix_ms: now,
        monotonic_ms: now,
    };
    evaluate_with_fact_refresh::<PolicyEvaluationError>(
        catalog,
        &facts,
        &resources,
        time,
        11,
        &BTreeSet::new(),
        inputs,
        |_| Ok(CandidateEligibility::Eligible),
    )
    .unwrap_or_else(|error| panic!("a fact refresh never fails the evaluation: {error}"))
}

fn host(host_id: &str, cpu_available_milli: u16) -> HostResourceSnapshot {
    HostResourceSnapshot {
        host_id: host_id.to_owned(),
        cpu_available_milli,
        gpu_available_milli: 1_000,
        io_available_milli: 1_000,
        host_responsiveness_basis_points: 10_000,
        third_party_pressure_basis_points: 0,
        heavy_dispatch_limit: 1,
        active_heavy_dispatches: 0,
        heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
    }
}

// A catalog of task X alone, triggered by `trigger`, with the fixture's activity profile.
fn predicate_catalog(trigger: Value) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.fact-refresh-predicates",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-refresh"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let task = json!({
        "id": X,
        "scope": {"kind": "game", "game_id": "game-a"},
        "entrypoint": {"operation_id": "operation.run"},
        "procedure_ref": "procedure.x",
        "priority": 100,
        "trigger": trigger,
        "feedback_stop": {
            "kind": "outcome",
            "task_id": X,
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
    });
    let profile = json!({
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
    });
    let source = |name: &str, document: &Value| {
        CatalogDocumentSource::new(
            format!("memory://fixture/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", json!([task]))),
        pools: source("pools", &document("pools", json!([]))),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| {
        panic!(
            "the predicate catalog compiles: {:?}",
            failure.diagnostics()
        )
    })
}

// A trigger that compares `fact_key` in `scope`, read within fifteen minutes of its observation.
fn trigger_fact(scope: Value, fact_key: &str) -> Value {
    json!({
        "kind": "fact",
        "scope": scope,
        "fact_key": fact_key,
        "comparison": "less_than",
        "value": {"type": "integer", "value": 20},
        "max_age_ms": 900000
    })
}

// The values `fact_refresh_pending` carries for A's K (expired) with these optional values.
fn pending(
    gate_code: Option<&str>,
    failure_code: Option<&str>,
    next_eligible: Option<u64>,
) -> Vec<(&'static str, Value)> {
    let mut values = vec![
        ("instance_id", json!(A)),
        ("fact_key", json!(STOCK)),
        ("unknown_reason", json!("fact_expired")),
        ("package_id", json!(READER)),
    ];
    values.extend(gate_code.map(|code| ("gate_code", json!(code))));
    values.extend(failure_code.map(|code| ("failure_code", json!(code))));
    values.extend(next_eligible.map(|at| ("next_eligible_unix_ms", json!(at))));
    values
}

// The values `fact_refresh_exhausted` carries for A's K under `limit`.
fn exhausted(limit: &str, next_eligible: Option<u64>) -> Vec<(&'static str, Value)> {
    let mut values = vec![
        ("instance_id", json!(A)),
        ("fact_key", json!(STOCK)),
        ("package_id", json!(READER)),
        ("refresh_limit", json!(limit)),
    ];
    values.extend(next_eligible.map(|at| ("next_eligible_unix_ms", json!(at))));
    values
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

fn code_of(reason: &DecisionReason) -> &str {
    reason.outcome.code().as_str()
}

fn codes(decision: &TaskDecision) -> Vec<&str> {
    decision.reasons.iter().map(code_of).collect()
}

fn last_code(decision: &TaskDecision) -> &str {
    decision.reasons.last().map_or("", code_of)
}

// The decision's reasons of the fact refresh, in order.
fn refresh_codes(decision: &TaskDecision) -> Vec<&str> {
    codes(decision)
        .into_iter()
        .filter(|code| code.starts_with("fact_refresh_"))
        .collect()
}

fn coded<'a>(decision: &'a TaskDecision, code: &str) -> Vec<&'a DecisionReason> {
    decision
        .reasons
        .iter()
        .filter(|reason| code_of(reason) == code)
        .collect()
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
        .unwrap_or_else(|| panic!("a decision for {task_id} on {instance_id}"))
}

// R_I's own decision on the instance, when the round planned it.
fn refresh_decision<'a>(
    evaluation: &'a PolicyEvaluation,
    instance_id: &str,
) -> Option<&'a TaskDecision> {
    evaluation.decisions.iter().find(|decision| {
        decision.task_id == FACT_REFRESH_TASK_ID
            && decision.instance_id.as_deref() == Some(instance_id)
    })
}

fn intents_on<'a>(evaluation: &'a PolicyEvaluation, instance_id: &str) -> Vec<&'a DispatchIntent> {
    evaluation
        .dispatch_intents
        .iter()
        .filter(|intent| intent.instance_id == instance_id)
        .collect()
}

fn refresh_intents_on(evaluation: &PolicyEvaluation, instance_id: &str) -> usize {
    intents_on(evaluation, instance_id)
        .into_iter()
        .filter(|intent| intent.task_id == FACT_REFRESH_TASK_ID)
        .count()
}

fn has_intent(evaluation: &PolicyEvaluation, task_id: &str, instance_id: &str) -> bool {
    evaluation
        .dispatch_intents
        .iter()
        .any(|intent| intent.task_id == task_id && intent.instance_id == instance_id)
}

fn chain_of<'a>(
    evaluation: &'a PolicyEvaluation,
    intent: &DispatchIntent,
) -> &'a DecisionReasonChain {
    evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
}
