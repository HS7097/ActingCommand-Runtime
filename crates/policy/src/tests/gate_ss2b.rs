// SPDX-License-Identifier: AGPL-3.0-only

// SS-2b (register §1.225; Alice's ruling 104 point 3 and ruling 116, 2026-10-30; brief
// `rd-1029/brief-116-112-114.md` SS-2b tests 1 to 9; contracts/scheduling/README.md, "Net benefit
// within a tier"):
// - a judged candidate (a task the catalog marks `benefit_judged`, neither in a chain head's run
//   window nor a manual request) ranks within its tier by its net benefit per expected
//   millisecond `B ÷ E`, the handwritten priority aside;
// - a known `B ≤ 0` is `policy_benefit_nonpositive`, any unknown input `benefit_unknown`; neither
//   is dispatched, and the known part of a partly known candidate is never added up;
// - unjudged candidates rank as before; a catalog without a mark decides as before;
// - consumption costs its weight, a regenerating pool's discounted by its fullness; an expected
//   amount of 350 milli counts as 0.35;
// - the pending markers validate in V2 and are refused elsewhere; the composer marks the rules
//   whose package is not maintenance-role and counts the judged rules' coverage.
//
// The catalog is neutral and V2: tasks on one instance with an hourly clock due at NOW, five pools
// of the instance read from their keys. Every candidate's host is idle. Every expected value comes
// from the work order and the contract text.

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    BenefitCoverage, CandidateEligibility, CatalogCompileFailure, CatalogDiagnosticCode,
    CatalogDocumentSource, CatalogSources, CompiledCatalog, DecisionReason, DispatchIntent,
    EligibilityState, EvaluationFacts, EvaluationResources, EvaluationTime, HostResourceSnapshot,
    ObservedFact, PolicyCode, PolicyEvaluation, PolicyEvaluationError, SchedulingDecisionState,
    StandardPackBinding, StandardPackComposeInput, StandardPackComposition, StandardPackGameFiles,
    TaskDecision, TaskRuntimeSnapshot, TaskTerminalState, compile_catalog,
    compose_standard_pack_rules, evaluate_with_eligibility, project_fact_pools,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    BundleIndex, StandardPackProblem, StandardPackRootBytes, read_standard_pack_root,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const OPERATION: &str = "operation.run";
/// Valued 1000 milli a unit (scale 1), no regeneration.
const VALUED: &str = "pool.valued";
/// Valued 100 milli a unit (scale 1), no regeneration.
const LOW: &str = "pool.low";
/// Valued 1000 milli a unit (scale 1), regenerating one unit every six minutes up to 100.
const STAMINA: &str = "pool.stamina";
const STAMINA_KEY: &str = "resource.stamina";
/// Its valuation is the pending marker.
const PENDING: &str = "pool.pending";
/// It declares no valuation.
const BARE: &str = "pool.bare";
const MINUTE_MS: u64 = 60_000;
/// The deferral the fixture's admission answers when it holds every candidate, so that the round
/// asks it about every candidate in allocation order.
const NOT_LOADED: &str = "pack_registry_not_loaded";

// Test 1: two judged candidates of one tier rank by B ÷ E. The one with the higher B wins though
// its handwritten priority is lower; of two with different E, the higher B per millisecond wins
// though its B is lower. The winner's breakdown carries B and E as values.
#[test]
fn ss2b_t1_judged_candidates_rank_by_benefit_per_millisecond_within_a_tier() {
    let by_benefit = catalog(vec![
        task(
            "task_priority",
            300,
            vec![produce(LOW, 1)],
            Vec::new(),
            true,
        ),
        task(
            "task_benefit",
            100,
            vec![produce(VALUED, 1)],
            Vec::new(),
            true,
        ),
    ]);
    let (evaluation, _) = round(&by_benefit, Vec::new(), Vec::new(), true);
    assert_eq!(dispatched(&evaluation), ["task_benefit"], "{evaluation:#?}");
    let breakdown = reason(decision(&evaluation, "task_benefit"), "rank_breakdown");
    assert_eq!(breakdown.outcome.value("benefit_milli"), Some(&json!(1000)));
    assert_eq!(
        breakdown.outcome.value("expected_duration_ms"),
        Some(&json!(60_000))
    );
    assert!(
        breakdown
            .detail
            .ends_with(" benefit_milli=1000 expected_duration_ms=60000"),
        "{}",
        breakdown.detail
    );

    // B 1000 over 600 000 ms against B 200 over 60 000 ms: the second earns twice as much a
    // millisecond.
    let by_rate = catalog(vec![
        with(
            task("task_slow", 300, vec![produce(VALUED, 1)], Vec::new(), true),
            "expected_duration_ms",
            json!(600_000),
        ),
        task("task_fast", 100, vec![produce(LOW, 2)], Vec::new(), true),
    ]);
    let (evaluation, _) = round(&by_rate, Vec::new(), Vec::new(), true);
    assert_eq!(dispatched(&evaluation), ["task_fast"], "{evaluation:#?}");
    let breakdown = reason(decision(&evaluation, "task_fast"), "rank_breakdown");
    assert_eq!(breakdown.outcome.value("benefit_milli"), Some(&json!(200)));
}

// Test 2: a judged candidate whose known B is not positive is deferred with
// `policy_benefit_nonpositive` and is not dispatched, even alone: B = 100 - 1000 = -900, and B = 0.
#[test]
fn ss2b_t2_a_known_nonpositive_benefit_is_not_dispatched() {
    for (case, produces, consumes, expected) in [
        ("a loss", produce(LOW, 1), consume(VALUED, 1), -900),
        ("even", produce(VALUED, 1), consume(VALUED, 1), 0),
    ] {
        let catalog = catalog(vec![task(
            "task_loss",
            100,
            vec![produces],
            vec![consumes],
            true,
        )]);
        let (evaluation, _) = round(&catalog, Vec::new(), Vec::new(), true);
        assert!(
            dispatched(&evaluation).is_empty(),
            "{case}: {evaluation:#?}"
        );
        let held = decision(&evaluation, "task_loss");
        assert_eq!(held.state, SchedulingDecisionState::Deferred, "{case}");
        assert_eq!(held.eligibility, EligibilityState::True, "{case}");
        let last = last(held);
        assert!(
            last.is(PolicyCode::PolicyBenefitNonpositive),
            "{case}: {held:#?}"
        );
        assert_eq!(
            last.value("benefit_milli"),
            Some(&json!(expected)),
            "{case}"
        );
        assert_eq!(
            last.value("expected_duration_ms"),
            Some(&json!(60_000)),
            "{case}"
        );
    }
}

// Test 3: every unknown input gives `benefit_unknown` and the candidate is never dispatched, even
// alone: no declaration, a pending entry, a pending valuation, a missing valuation, an unknown and
// an expired reading. A partly known candidate is unknown: its known 5000 milli are not added up.
#[test]
fn ss2b_t3_every_unknown_input_is_benefit_unknown_and_never_dispatched() {
    let stale = fact(
        A,
        STAMINA_KEY,
        50,
        NOW - 2 * MINUTE_MS,
        Some(NOW - MINUTE_MS),
    );
    let none: Vec<ObservedFact> = Vec::new();
    let cases = [
        (
            "no declaration",
            Vec::new(),
            Vec::new(),
            none,
            "no_effects",
            None,
        ),
        (
            "a pending entry",
            vec![pending(VALUED, "produce")],
            Vec::new(),
            Vec::new(),
            "entry_pending",
            Some(VALUED),
        ),
        (
            "a pending valuation",
            vec![produce(PENDING, 1)],
            Vec::new(),
            Vec::new(),
            "valuation_pending",
            Some(PENDING),
        ),
        (
            "a missing valuation, partly known",
            vec![produce(VALUED, 5), produce(BARE, 1)],
            Vec::new(),
            Vec::new(),
            "valuation_missing",
            Some(BARE),
        ),
        (
            "a pending consumption",
            vec![produce(VALUED, 5)],
            vec![pending(LOW, "consume")],
            Vec::new(),
            "entry_pending",
            Some(LOW),
        ),
        (
            "an unknown reading, partly known",
            vec![produce(VALUED, 5)],
            vec![consume(STAMINA, 10)],
            Vec::new(),
            "reading_unknown",
            Some(STAMINA),
        ),
        (
            "an expired reading",
            vec![produce(VALUED, 5)],
            vec![consume(STAMINA, 10)],
            vec![stale],
            "reading_unknown",
            Some(STAMINA),
        ),
    ];
    for (case, produces, consumes, readings, gap, pool) in cases {
        let catalog = catalog(vec![task("task_unknown", 100, produces, consumes, true)]);
        let (evaluation, asked) = round(&catalog, Vec::new(), readings, true);
        assert!(
            dispatched(&evaluation).is_empty(),
            "{case}: {evaluation:#?}"
        );
        assert!(asked.is_empty(), "{case}: the admission is never asked");
        let held = decision(&evaluation, "task_unknown");
        assert_eq!(held.state, SchedulingDecisionState::Deferred, "{case}");
        assert_eq!(held.eligibility, EligibilityState::True, "{case}");
        let last = last(held);
        assert!(last.is(PolicyCode::BenefitUnknown), "{case}: {held:#?}");
        assert_eq!(last.value("benefit_gap"), Some(&json!(gap)), "{case}");
        assert_eq!(
            last.value("pool_id"),
            pool.map(|pool| json!(pool)).as_ref(),
            "{case}"
        );
        assert_eq!(
            last.value("benefit_milli"),
            None,
            "{case}: never the known part"
        );
        if gap == "reading_unknown" {
            assert_eq!(last.value("fact_key"), Some(&json!(STAMINA_KEY)), "{case}");
            let reason = if case.starts_with("an expired") {
                "fact_expired"
            } else {
                "fact_missing"
            };
            assert_eq!(last.value("unknown_reason"), Some(&json!(reason)), "{case}");
        }
    }

    // Beside a positive candidate, an unknown one never runs either.
    let catalog = catalog(vec![
        task("task_unknown", 300, Vec::new(), Vec::new(), true),
        task("task_known", 100, vec![produce(LOW, 1)], Vec::new(), true),
    ]);
    let (evaluation, _) = round(&catalog, Vec::new(), Vec::new(), true);
    assert_eq!(dispatched(&evaluation), ["task_known"], "{evaluation:#?}");
}

// Test 4: unjudged candidates rank as before. Maintenance-role tasks (no mark) come first and keep
// their order and their decisions; a manual request (its trigger requires a `session.`
// instruction) and a chain successor in its head's run window are business tasks that are not
// judged: no `benefit_unknown` though they declare nothing, and they rank before the judged
// candidates of their tier.
#[test]
fn ss2b_t4_unjudged_candidates_rank_as_before() {
    let maintenance = |judged: bool| {
        catalog(vec![
            task(
                "task_judged",
                300,
                vec![produce(VALUED, 1)],
                Vec::new(),
                judged,
            ),
            task("task_maint_low", 100, Vec::new(), Vec::new(), false),
            task("task_maint_high", 200, Vec::new(), Vec::new(), false),
        ])
    };
    let (marked, asked) = round(&maintenance(true), Vec::new(), Vec::new(), false);
    assert_eq!(asked, ["task_maint_high", "task_maint_low", "task_judged"]);
    let (unmarked, _) = round(&maintenance(false), Vec::new(), Vec::new(), false);
    for task_id in ["task_maint_high", "task_maint_low"] {
        assert_eq!(
            decision(&marked, task_id),
            decision(&unmarked, task_id),
            "{task_id} decides as in a catalog without a mark"
        );
    }

    // A manual request.
    let request = all(vec![hourly(), at_least("session.request.task_request", 1)]);
    let catalog_request = catalog(vec![
        task(
            "task_judged",
            300,
            vec![produce(VALUED, 1)],
            Vec::new(),
            true,
        ),
        with(
            task("task_request", 100, Vec::new(), Vec::new(), true),
            "trigger",
            request,
        ),
    ]);
    let readings = vec![fact(
        A,
        "session.request.task_request",
        1,
        NOW - MINUTE_MS,
        None,
    )];
    let (evaluation, asked) = round(&catalog_request, Vec::new(), readings, false);
    assert_eq!(asked, ["task_request", "task_judged"], "{evaluation:#?}");
    assert!(no_benefit_reason(decision(&evaluation, "task_request")));

    // A chain successor in its head's run window ranks with its head and is not judged.
    let successor = all(vec![hourly(), dependency("task_head")]);
    let catalog_chain = catalog(vec![
        task("task_head", 300, vec![produce(LOW, 1)], Vec::new(), true),
        task(
            "task_other",
            300,
            vec![produce(VALUED, 1)],
            Vec::new(),
            true,
        ),
        with(
            task("task_next", 100, Vec::new(), Vec::new(), true),
            "trigger",
            successor,
        ),
    ]);
    let head_ran = vec![state(
        "task_head",
        NOW - 10 * MINUTE_MS,
        TaskTerminalState::Succeeded,
    )];
    let (evaluation, asked) = round(&catalog_chain, head_ran, Vec::new(), false);
    assert_eq!(
        asked,
        ["task_next", "task_other", "task_head"],
        "{evaluation:#?}"
    );
    let next = decision(&evaluation, "task_next");
    assert!(no_benefit_reason(next));
    assert_eq!(next.rank.as_ref().map(|rank| rank.priority), Some(300));
}

// Test 5: a catalog without a mark decides as before: the handwritten priority orders it whatever
// the values (the higher priority runs though its value is lower), no candidate is judged, no
// breakdown carries a value or a benefit field, and two evaluations of the same inputs are byte
// for byte equal. (The neutral catalog's identity is pinned by `gate_386_cf4`'s golden hash.)
#[test]
fn ss2b_t5_a_catalog_without_marks_decides_as_before() {
    let catalog = catalog(vec![
        task(
            "task_priority",
            300,
            vec![produce(LOW, 1)],
            Vec::new(),
            false,
        ),
        task(
            "task_benefit",
            100,
            vec![produce(VALUED, 1)],
            Vec::new(),
            false,
        ),
        task("task_none", 200, Vec::new(), Vec::new(), false),
    ]);
    let (evaluation, _) = round(&catalog, Vec::new(), Vec::new(), true);
    let (again, _) = round(&catalog, Vec::new(), Vec::new(), true);
    assert_eq!(
        serde_json::to_vec(&evaluation).expect("bytes"),
        serde_json::to_vec(&again).expect("bytes")
    );
    assert_eq!(
        dispatched(&evaluation),
        ["task_priority"],
        "{evaluation:#?}"
    );
    let text = serde_json::to_string(&evaluation).expect("text");
    for absent in [
        "benefit_unknown",
        "policy_benefit_nonpositive",
        "benefit_milli",
    ] {
        assert!(!text.contains(absent), "{absent}: {text}");
    }
    let compiled = serde_json::to_string(catalog.catalog().tasks.tasks.as_slice()).expect("tasks");
    assert!(!compiled.contains("benefit_judged"), "{compiled}");
}

// Test 6: consumption lowers B. A pool that does not regenerate costs its full weight: 2 x 1000
// - 3 x 100 = 1700. A regenerating pool costs W * clamp((cap - cur) / (cap - reserve), 0, 1):
// nothing at capacity, W at the reserve line, half way between them half. Ten units of stamina
// at 1000 milli each against one valued unit: 1000, -9000 and -4000; below the line the share
// stays 1. A candidate whose B is not positive wakes at the pool's next step.
#[test]
fn ss2b_t6_consumption_lowers_the_benefit_by_its_cost() {
    let catalog_plain = catalog(vec![task(
        "task_plain",
        100,
        vec![produce(VALUED, 2)],
        vec![consume(LOW, 3)],
        true,
    )]);
    let (evaluation, _) = round(&catalog_plain, Vec::new(), Vec::new(), true);
    assert_eq!(dispatched(&evaluation), ["task_plain"]);
    let breakdown = reason(decision(&evaluation, "task_plain"), "rank_breakdown");
    assert_eq!(breakdown.outcome.value("benefit_milli"), Some(&json!(1700)));

    let catalog_regen = catalog(vec![with(
        task(
            "task_regen",
            100,
            vec![produce(VALUED, 1)],
            vec![consume(STAMINA, 10)],
            true,
        ),
        "reserves",
        json!([{"pool_id": STAMINA, "at_least": 20}]),
    )]);
    let observed = NOW - MINUTE_MS;
    let at = |amount: i64| vec![fact(A, STAMINA_KEY, amount, observed, None)];
    let (full, _) = round(&catalog_regen, Vec::new(), at(100), true);
    assert_eq!(dispatched(&full), ["task_regen"], "{full:#?}");
    let breakdown = reason(decision(&full, "task_regen"), "rank_breakdown");
    assert_eq!(breakdown.outcome.value("benefit_milli"), Some(&json!(1000)));
    for (amount, expected) in [(20, -9_000), (60, -4_000), (10, -9_000)] {
        let (evaluation, _) = round(&catalog_regen, Vec::new(), at(amount), true);
        assert!(
            dispatched(&evaluation).is_empty(),
            "{amount}: {evaluation:#?}"
        );
        let held = decision(&evaluation, "task_regen");
        let last = last(held);
        assert!(
            last.is(PolicyCode::PolicyBenefitNonpositive),
            "{amount}: {held:#?}"
        );
        assert_eq!(
            last.value("benefit_milli"),
            Some(&json!(expected)),
            "{amount}"
        );
        assert_eq!(
            held.next_wake_unix_ms,
            Some(observed + 6 * MINUTE_MS),
            "{amount}: the pool's next step"
        );
    }
}

// Test 7: an `expected_amount_milli` of 350 counts as 0.35 of a unit: B = 0.35 x 1000 = 350, not
// 0 and not a whole unit's 1000.
#[test]
fn ss2b_t7_an_expected_amount_of_350_milli_counts_as_a_fraction() {
    let catalog = catalog(vec![task(
        "task_fraction",
        100,
        vec![expected(VALUED, 350)],
        Vec::new(),
        true,
    )]);
    let (evaluation, _) = round(&catalog, Vec::new(), Vec::new(), true);
    assert_eq!(
        dispatched(&evaluation),
        ["task_fraction"],
        "{evaluation:#?}"
    );
    let breakdown = reason(decision(&evaluation, "task_fraction"), "rank_breakdown");
    assert_eq!(breakdown.outcome.value("benefit_milli"), Some(&json!(350)));
}

// Test 8: the pending markers validate in V2 (a pool's `valuation`, an entry's `amount` in either
// direction) and serialize as written; a catalog without them keeps its members. Bad shapes are
// refused with named codes: another string (`type_mismatch`), a pending amount beside an expected
// one (`limit_exceeded`), a mark that is no boolean (`type_mismatch`), and any marker or mark in a
// V1 document (`type_mismatch` at the marker, `unknown_field` at the mark).
#[test]
fn ss2b_t8_the_pending_markers_validate_in_v2_and_bad_shapes_are_refused() {
    let catalog = catalog(vec![task(
        "task_pending",
        100,
        vec![pending(VALUED, "produce")],
        vec![pending(LOW, "consume")],
        true,
    )]);
    let bundle = catalog.catalog();
    let pending_pool = bundle
        .pools
        .pools
        .iter()
        .find(|pool| pool.id == PENDING)
        .expect("the pending pool");
    assert!(pending_pool.valuation_pending());
    assert!(pending_pool.declared_valuation().is_none());
    assert_eq!(
        serde_json::to_value(pending_pool).expect("pool")["valuation"],
        json!("pending")
    );
    let pending_task = &bundle.tasks.tasks[0];
    assert!(pending_task.produces[0].is_pending() && pending_task.consumes[0].is_pending());
    assert_eq!(pending_task.produces[0].units(), None);
    let written = serde_json::to_value(pending_task).expect("task");
    assert_eq!(written["produces"][0]["amount"], json!("pending"));
    assert_eq!(written["consumes"][0]["amount"], json!("pending"));
    assert_eq!(written["benefit_judged"], json!(true));

    let refused_v2 = |tasks: Vec<Value>, pools: Value, code: CatalogDiagnosticCode, path: &str| {
        let failure = compile_v2(tasks, pools).expect_err("refused");
        assert!(
            refused_at(&failure, code, path),
            "{code:?} at {path}: {:?}",
            failure.diagnostics()
        );
    };
    let mut later = pools();
    later[3]["valuation"] = json!("later");
    refused_v2(
        vec![task_value("task_pending")],
        later,
        CatalogDiagnosticCode::TypeMismatch,
        "/pools/3/valuation",
    );
    refused_v2(
        vec![task(
            "task_pending",
            100,
            vec![effect(VALUED, "produce", json!("Pending"))],
            Vec::new(),
            true,
        )],
        pools(),
        CatalogDiagnosticCode::TypeMismatch,
        "/tasks/0/produces/0/amount",
    );
    let mut both = pending(VALUED, "produce");
    both["expected_amount_milli"] = json!(350);
    refused_v2(
        vec![task("task_pending", 100, vec![both], Vec::new(), true)],
        pools(),
        CatalogDiagnosticCode::LimitExceeded,
        "/tasks/0/produces/0",
    );
    refused_v2(
        vec![with(
            task_value("task_pending"),
            "benefit_judged",
            json!("yes"),
        )],
        pools(),
        CatalogDiagnosticCode::TypeMismatch,
        "/tasks/0/benefit_judged",
    );

    // V1: the example catalog with one marker or the mark.
    let refused_v1 = |case: &str, failure: CatalogCompileFailure, code, path: &str| {
        assert!(
            refused_at(&failure, code, path),
            "{case}: {code:?} at {path}: {:?}",
            failure.diagnostics()
        );
    };
    refused_v1(
        "a pending valuation",
        compile_v1_example(|_, pools| pools["pools"][0]["valuation"] = json!("pending"))
            .expect_err("a pending valuation"),
        CatalogDiagnosticCode::TypeMismatch,
        "/pools/0/valuation",
    );
    refused_v1(
        "a pending amount",
        compile_v1_example(|tasks, _| {
            tasks["tasks"][0]["produces"][0]["amount"] = json!("pending");
        })
        .expect_err("a pending amount"),
        CatalogDiagnosticCode::TypeMismatch,
        "/tasks/0/produces/0/amount",
    );
    refused_v1(
        "the mark",
        compile_v1_example(|tasks, _| tasks["tasks"][0]["benefit_judged"] = json!(true))
            .expect_err("the mark"),
        CatalogDiagnosticCode::UnknownField,
        "/tasks/0/benefit_judged",
    );
}

// Test 9: the composer marks every rule whose package is not maintenance-role and no other, and
// counts the judged rules: one complete, two pending (a pending entry, a pending valuation), two
// undeclared (no entry; a pool with neither a valuation nor the marker), each undeclared rule one
// `standard_pack_benefit_undeclared` warning; the maintenance-role rule is not counted. The
// composition's own warnings are unchanged. A rule that writes the mark itself is refused.
#[test]
fn ss2b_t9_the_composer_marks_the_judged_rules_and_counts_their_coverage() {
    let composition = compose(schedule(vec![
        rule("rule.complete", vec![produce_unscoped(VALUED, json!(1))]),
        rule(
            "rule.entry",
            vec![produce_unscoped(VALUED, json!("pending"))],
        ),
        rule("rule.valuation", vec![produce_unscoped(PENDING, json!(1))]),
        rule("rule.none", Vec::new()),
        rule("rule.bare", vec![produce_unscoped(BARE, json!(1))]),
        rule("rule.maint", Vec::new()),
    ]))
    .unwrap_or_else(|failure| panic!("the fixture composes: {failure}"));
    let marks = composition
        .compiled()
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| (task.id.clone(), task.benefit_judged))
        .collect::<Vec<_>>();
    assert_eq!(
        marks,
        [
            ("rule.complete".to_owned(), Some(true)),
            ("rule.entry".to_owned(), Some(true)),
            ("rule.valuation".to_owned(), Some(true)),
            ("rule.none".to_owned(), Some(true)),
            ("rule.bare".to_owned(), Some(true)),
            ("rule.maint".to_owned(), None),
        ]
    );
    assert_eq!(
        composition.benefit_coverage(),
        [(
            GAME.to_owned(),
            BenefitCoverage {
                complete: 1,
                pending: 2,
                undeclared: 2,
                // SS-2c (register §1.227): the count of rules declaring themselves maintenance.
                maintenance_declared: 0,
            }
        )]
    );
    let warnings = composition.benefit_warnings();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    for (warning, (rule_id, pool_id)) in warnings
        .iter()
        .zip([("rule.none", None), ("rule.bare", Some(BARE))])
    {
        assert!(
            warning.is(SharedCode::StandardPackBenefitUndeclared),
            "{warning:?}"
        );
        assert_eq!(warning.value("game"), Some(&json!(GAME)));
        assert_eq!(warning.value("task_id"), Some(&json!(rule_id)));
        assert_eq!(
            warning.value("pool_id"),
            pool_id.map(|pool| json!(pool)).as_ref()
        );
    }
    assert!(
        composition.warnings().is_empty(),
        "{:?}",
        composition.warnings()
    );

    let mut written = rule("rule.complete", vec![produce_unscoped(VALUED, json!(1))]);
    written["benefit_judged"] = json!(false);
    let failure = compose(schedule(vec![written])).expect_err("the mark is the composer's");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleInvalid);
    assert_eq!(issue.pointer, "/rules/0/benefit_judged");
}

// ---- Fixtures ----

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

fn all(predicates: Vec<Value>) -> Value {
    json!({"kind": "all", "predicates": predicates})
}

fn at_least(fact_key: &str, threshold: i64) -> Value {
    json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": fact_key,
        "comparison": "greater_than_or_equal",
        "value": {"type": "integer", "value": threshold},
        "max_age_ms": HOUR_MS
    })
}

/// True once `task_id`'s latest run on the instance succeeded.
fn dependency(task_id: &str) -> Value {
    json!({"kind": "dependency_completed", "task_id": task_id, "terminal_states": ["succeeded"]})
}

fn effect(pool: &str, direction: &str, amount: Value) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

fn produce(pool: &str, amount: u64) -> Value {
    effect(pool, "produce", json!(amount))
}

fn consume(pool: &str, amount: u64) -> Value {
    effect(pool, "consume", json!(amount))
}

/// An entry whose `amount` is the pending marker.
fn pending(pool: &str, direction: &str) -> Value {
    effect(pool, direction, json!("pending"))
}

fn expected(pool: &str, milli: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": "produce",
        "expected_amount_milli": milli,
        "observation_source": "self_reported",
        "confidence_milli": 1000
    })
}

/// A task of the game on its hourly clock, marked `benefit_judged` when `judged`.
fn task(
    id: &str,
    priority: i16,
    produces: Vec<Value>,
    consumes: Vec<Value>,
    judged: bool,
) -> Value {
    let mut task = json!({
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
        "consumes": consumes,
        "produces": produces,
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
    });
    if judged {
        task["benefit_judged"] = json!(true);
    }
    task
}

/// A judged task of no effect.
fn task_value(id: &str) -> Value {
    task(id, 100, Vec::new(), Vec::new(), true)
}

/// `task` with its member `name` set to `value`.
fn with(mut task: Value, name: &str, value: Value) -> Value {
    task[name] = value;
    task
}

fn pool(id: &str, capacity: u64, projection: Value, valuation: Option<Value>) -> Value {
    let mut pool = json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": capacity,
        "projection": projection,
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    });
    if let Some(valuation) = valuation {
        pool["valuation"] = valuation;
    }
    pool
}

fn valuation(base_weight_milli: u32) -> Value {
    json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": base_weight_milli})
}

/// The five pools, in this order: valued, low, stamina, pending, bare.
fn pools() -> Value {
    let none = json!({"kind": "none"});
    json!([
        pool(VALUED, 1_000_000, none.clone(), Some(valuation(1000))),
        pool(LOW, 1_000_000, none.clone(), Some(valuation(100))),
        pool(
            STAMINA,
            100,
            json!({"amount": 1, "per_ms": 6 * MINUTE_MS}),
            Some(valuation(1000))
        ),
        pool(PENDING, 1_000, none.clone(), Some(json!("pending"))),
        pool(BARE, 1_000, none, None),
    ])
}

/// The fixture catalog, `actingcommand.scheduling.v2`, with these tasks in this order.
fn catalog(tasks: Vec<Value>) -> CompiledCatalog {
    compile_v2(tasks, pools()).unwrap_or_else(|failure| {
        panic!("the fixture catalog compiles: {:?}", failure.diagnostics())
    })
}

fn compile_v2(tasks: Vec<Value>, pools: Value) -> Result<CompiledCatalog, CatalogCompileFailure> {
    let header = json!({
        "catalog_id": "fixture.benefit",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-benefit"]
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
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", pools)),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
}

/// The V1 example catalog `catalog-a`, its tasks and pools documents changed by `mutate`.
fn compile_v1_example(
    mutate: impl FnOnce(&mut Value, &mut Value),
) -> Result<CompiledCatalog, CatalogCompileFailure> {
    let parse = |text: &str| serde_json::from_str::<Value>(text).expect("an example document");
    let mut tasks = parse(include_str!(
        "../../../../contracts/scheduling/examples/catalog-a/tasks.json"
    ));
    let mut pools = parse(include_str!(
        "../../../../contracts/scheduling/examples/catalog-a/pools.json"
    ));
    mutate(&mut tasks, &mut pools);
    compile_catalog(&CatalogSources {
        tasks: source("v1-tasks", &tasks),
        pools: source("v1-pools", &pools),
        activity: source(
            "v1-activity",
            &parse(include_str!(
                "../../../../contracts/scheduling/examples/catalog-a/activity.json"
            )),
        ),
        timeline: source(
            "v1-timeline",
            &parse(include_str!(
                "../../../../contracts/scheduling/examples/catalog-a/timeline.json"
            )),
        ),
        selection: None,
    })
}

fn refused_at(failure: &CatalogCompileFailure, code: CatalogDiagnosticCode, path: &str) -> bool {
    failure
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == code && diagnostic.json_path == path)
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-benefit/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

/// A task whose run, dispatched at `at`, ended in `terminal`.
fn state(task_id: &str, at: u64, terminal: TaskTerminalState) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: task_id.to_owned(),
        instance_id: A.to_owned(),
        last_dispatched_unix_ms: Some(at),
        eligible_since_unix_ms: None,
        terminal_state: Some(terminal),
        completed_window: None,
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
    }
}

/// One round at NOW. With `admit` every candidate is admitted; without it the admission holds
/// every candidate, and the second value lists the task ids it was asked about, in allocation
/// order.
fn round(
    catalog: &CompiledCatalog,
    tasks: Vec<TaskRuntimeSnapshot>,
    readings: Vec<ObservedFact>,
    admit: bool,
) -> (PolicyEvaluation, Vec<String>) {
    let instance = instance(A, &[]);
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-benefit".to_owned(),
        facts: readings,
        outcomes: Vec::new(),
        tasks,
        instances: vec![instance.clone()],
        priority_offsets: Vec::new(),
    };
    let resources = project_fact_pools(
        catalog,
        &facts,
        &EvaluationResources {
            pools: Vec::new(),
            hosts: vec![HostResourceSnapshot {
                host_id: instance.host_id,
                cpu_available_milli: 1_000,
                gpu_available_milli: 1_000,
                io_available_milli: 1_000,
                host_responsiveness_basis_points: 10_000,
                third_party_pressure_basis_points: 0,
                heavy_dispatch_limit: 1,
                active_heavy_dispatches: 0,
                heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
            }],
        },
    );
    let mut asked = Vec::new();
    let evaluation = evaluate_with_eligibility(
        catalog,
        &facts,
        &resources,
        EvaluationTime {
            unix_ms: NOW,
            monotonic_ms: NOW,
        },
        11,
        |intent: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
            asked.push(intent.task_id.clone());
            if admit {
                return Ok(CandidateEligibility::Eligible);
            }
            Ok(CandidateEligibility::Deferred {
                reason: DecisionReason {
                    outcome: Outcome::relayed(NOT_LOADED),
                    detail: format!("fixture admission: {NOT_LOADED}"),
                },
                next_wake_unix_ms: None,
            })
        },
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"));
    (evaluation, asked)
}

fn dispatched(evaluation: &PolicyEvaluation) -> Vec<&str> {
    evaluation
        .dispatch_intents
        .iter()
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

fn reason<'a>(decision: &'a TaskDecision, code: &str) -> &'a DecisionReason {
    decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == code)
        .unwrap_or_else(|| panic!("a {code} reason on {decision:#?}"))
}

fn last(decision: &TaskDecision) -> &Outcome {
    &decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {decision:#?}"))
        .outcome
}

/// Whether the decision carries neither benefit reason.
fn no_benefit_reason(decision: &TaskDecision) -> bool {
    !decision.reasons.iter().any(|reason| {
        reason.outcome.is(PolicyCode::BenefitUnknown)
            || reason.outcome.is(PolicyCode::PolicyBenefitNonpositive)
    })
}

// ---- The standard-pack fixture of test 9 ----

/// An effect of a `schedule.json` rule; the composer fills nothing in it but its pool's id.
fn produce_unscoped(pool: &str, amount: Value) -> Value {
    effect(pool, "produce", amount)
}

/// A `schedule.json` rule running the package of its own id, on its hourly clock.
fn rule(id: &str, produces: Vec<Value>) -> Value {
    let mut rule = task(id, 100, produces, Vec::new(), false);
    let members = rule.as_object_mut().expect("a task object");
    for member in ["scope", "entrypoint", "procedure_ref", "instance_overrides"] {
        members.shift_remove(member);
    }
    members.insert("package_id".to_owned(), json!(id));
    rule
}

/// `schedule.json` with these rules and the valued, pending and bare pools, without scopes.
fn schedule(rules: Vec<Value>) -> Value {
    let pools = pools()
        .as_array()
        .expect("the pools")
        .iter()
        .filter(|pool| [VALUED, PENDING, BARE].contains(&pool["id"].as_str().unwrap_or("")))
        .map(|pool| {
            let mut pool = pool.clone();
            pool.as_object_mut()
                .expect("a pool object")
                .shift_remove("scope");
            pool
        })
        .collect::<Vec<_>>();
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [],
        "rules": rules,
        "pools": pools,
        "activity": [],
        "timeline": [],
        "held": []
    })
}

/// Composes `schedule` for one instance of the game, whose registry holds a package per rule,
/// `rule.maint`'s a return-home maintenance package.
fn compose(schedule: Value) -> Result<StandardPackComposition, crate::StandardPackComposeFailure> {
    let packages = schedule["rules"]
        .as_array()
        .expect("the rules")
        .iter()
        .map(|rule| rule["package_id"].as_str().expect("a package").to_owned())
        .chain(["rule.maint".to_owned()])
        .collect::<std::collections::BTreeSet<_>>();
    let bundle = json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": packages
            .iter()
            .map(|package_id| json!({
                "package_id": package_id,
                "server": SERVER,
                "entry_task_id": "task",
                "path": format!("packs/{package_id}"),
                "file_count": 1,
                "byte_count": 64
            }))
            .collect::<Vec<_>>(),
        "maintenance": [{"package_id": "rule.maint", "server": SERVER, "uses": ["return_home"]}]
    });
    let bundle = serde_json::to_vec(&bundle).expect("bundle bytes");
    let root = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle.as_slice()),
            ..StandardPackRootBytes::default()
        },
        GAME,
    )
    .unwrap_or_else(|issue| panic!("the root reads: {issue}"))
    .expect("the root holds bundle.json");
    let maintenance = serde_json::from_slice::<BundleIndex>(&bundle)
        .expect("a bundle index")
        .maintenance()
        .to_vec();
    let schedule = serde_json::to_vec(&schedule).expect("schedule bytes");
    let games = std::collections::BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &schedule,
            root: &root,
            maintenance: &maintenance,
        },
    )]);
    let instances = std::collections::BTreeMap::from([(
        A.to_owned(),
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
