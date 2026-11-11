// SPDX-License-Identifier: AGPL-3.0-only

// #428 SCHED-C1f1 (register §1.303 SC1F-1 to SC1F-7; the coordinator's rulings of 11-11 14:5x on
// the nine questions of §1.303, not Alice's words; coordinator work order
// `rd-1111/brief-sched-c1f1.md`; contracts/scheduling/README.md, "A run's consumption read from a
// fact"; contracts/standard-pack-schedule.md, "A run's consumption read from a fact"):
// - the form `{"type": "fact", "fact_key": B, "fallback": F}` of a V2 `consumes` entry's `amount`,
//   each of its refusals at its pointer, and a catalog without it as before;
// - one evaluation reads a usable record of B in the instance's scope, else F, and says when a
//   record present and current is not usable (`consumption_fact_unusable`, its first reason);
// - every reader of the consumption reads that one amount;
// - a capped episode keeps its consumers key on the declared consumption (F) and its round its
//   frozen order, while the plan and the checks read the amount in effect;
// - a usable record's expiry bounds the candidate's freshness and wakes the evaluation;
// - the composer carries the form, requires `consumption_fact`, and lists the rules that write it.
//
// The catalog is neutral and V2, as SCHED-C1c's and SCHED-C1d's: tasks on one instance, the capped
// pool `pool.neutral` read from `resource.neutral`, its capacity read from
// `resource.neutral.capacity` (180), valued 1000 milli a unit (Q = 1), no regeneration, its stop
// line the capacity less 40 (140) and its hard line the capacity less 76 (104); `pool.out` valued
// 1000 milli a unit, so a consumer's value without the pool's cost is its production in milli. Task
// A reads one run's amount of the capped pool from the instance fact `resource.neutral.cost`, with
// a fallback; a run is simulated as in SCHED-C1d (read again half a minute after the dispatch, the
// next round a minute after it).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CONSUMPTION_FACT_SUBSYSTEM, CandidateEligibility, CappedMemory, CatalogCompileFailure,
    CatalogDiagnosticCode, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, ConsumptionFact, ConsumptionFactRule, DecisionReason, DispatchIntent,
    EvaluationFacts, EvaluationResources, EvaluationTime, FACT_REFERENCE_SUBSYSTEM,
    FactRefreshInputs, FactRefreshSettings, FactValue, ForwardProjectionConfig,
    HostResourceSnapshot, InstanceHolds, NoProgressGuards, ObservedFact, OwedReruns, PolicyCode,
    PolicyEvaluation, PolicyEvaluationError, RemedyPlan, RunSlots, SCHEDULING_SCHEMA_VERSION,
    SCHEDULING_SCHEMA_VERSION_V2, ScopeSelector, StandardPackBinding, StandardPackComposeInput,
    StandardPackComposition, StandardPackGameFiles, TaskDecision, TaskRuntimeSnapshot,
    TaskTerminalState, compile_catalog, compose_standard_pack_rules, evaluate_with_capped_spending,
    project_fact_pools, project_forward,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    BundleIndex, StandardPackProblem, StandardPackRootBytes, read_standard_pack_root,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// The capped pool, read from `READING_KEY`.
const POOL: &str = "pool.neutral";
const READING_KEY: &str = "resource.neutral";
/// The capacity fact of the capped pool, in its scope.
const CAPACITY_KEY: &str = "resource.neutral.capacity";
/// The instance fact task A reads one run's amount of the capped pool from.
const COST_KEY: &str = "resource.neutral.cost";
/// The produced pool, 1000 milli a unit, read from `resource.out`.
const OUT: &str = "pool.out";
const OUT_KEY: &str = "resource.out";
/// The daily reset of test 2, half an hour before the round.
const RESET_ID: &str = "reset.daily";
const RESET_AT: u64 = NOW - 1_800_000;
/// The neutral example catalog's golden identities, computed before SS-2h (as in `gate_ss2h`).
const V1_EXAMPLE_HASH: &str =
    "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c";
const V2_EXAMPLE_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";

// Test 1, the form. On the neutral example catalog (its pool's capacity a constant 120), task 0's
// `consumes` entry of the pool written as the form is refused at its pointer: in V1 `unknown_field`
// at `amount/type`; another `type`, a `fact_key` that is no identifier or no string, a `fallback`
// that is no integer or `null`, `type_mismatch` at the member; a `fallback` of 0 or 2^53,
// `limit_exceeded` at `amount/fallback`; 121, above the constant capacity, `effect_incompatible` at
// the entry; a member missing, `missing_required_field` at `amount`; `offset`, `scope` or another
// member, `unknown_field` at it; beside `expected_amount_milli`, `limit_exceeded` at the entry; on
// a `produces` entry, `effect_incompatible` at its `amount`; a second entry of the pool with the
// form, `effect_incompatible` at its `amount`. A fallback of 1 or 120 compiles, and so does the form
// beside a constant entry of the pool. The example keeps its V1 and V2 golden identities without the
// form; with it the catalog has another identity, the same on every compile, and its entry
// re-serializes as written.
#[test]
fn sched_c1f1_t1_the_form_is_refused_at_its_pointer_and_its_absence_changes_nothing() {
    let pool = "fixture-pool-a";
    let entry = |amount: Value| {
        json!({
            "pool_id": pool,
            "direction": "consume",
            "amount": amount,
            "observation_source": "scan_verified",
            "confidence_milli": 1000
        })
    };
    let read =
        |fallback: Value| json!({"type": "fact", "fact_key": COST_KEY, "fallback": fallback});
    let with_consumes = |version: &str, consumes: Value| {
        example(version, move |tasks, _| {
            tasks["tasks"][0]["consumes"] = consumes
        })
    };
    let consumes = |amount: Value| json!([entry(amount)]);
    let at = |suffix: &str| format!("/tasks/0/consumes/0{suffix}");

    let v1 = with_consumes(SCHEDULING_SCHEMA_VERSION, consumes(read(json!(3))));
    assert!(
        refused(
            &v1,
            CatalogDiagnosticCode::UnknownField,
            &at("/amount/type")
        ),
        "{:?}",
        diagnostics(&v1)
    );
    let mut offset = read(json!(3));
    offset["offset"] = json!(0);
    let mut scoped = read(json!(3));
    scoped["scope"] = json!({"kind": "instance", "instance_id": "fixture-instance-a"});
    let mut extra = read(json!(3));
    extra["margin"] = json!(1);
    let mut untyped = read(json!(3));
    untyped
        .as_object_mut()
        .expect("the form")
        .shift_remove("type");
    let mut no_key = read(json!(3));
    no_key
        .as_object_mut()
        .expect("the form")
        .shift_remove("fact_key");
    let mut no_fallback = read(json!(3));
    no_fallback
        .as_object_mut()
        .expect("the form")
        .shift_remove("fallback");
    let cases = [
        (
            "another type",
            json!({"type": "reading", "fact_key": COST_KEY, "fallback": 3}),
            CatalogDiagnosticCode::TypeMismatch,
            at("/amount/type"),
        ),
        (
            "a key that is no identifier",
            json!({"type": "fact", "fact_key": "Cost Key", "fallback": 3}),
            CatalogDiagnosticCode::TypeMismatch,
            at("/amount/fact_key"),
        ),
        (
            "a key that is no string",
            json!({"type": "fact", "fact_key": 7, "fallback": 3}),
            CatalogDiagnosticCode::TypeMismatch,
            at("/amount/fact_key"),
        ),
        (
            "a fallback that is no integer",
            read(json!("three")),
            CatalogDiagnosticCode::TypeMismatch,
            at("/amount/fallback"),
        ),
        (
            "a null fallback",
            read(Value::Null),
            CatalogDiagnosticCode::TypeMismatch,
            at("/amount/fallback"),
        ),
        (
            "a fallback of 0",
            read(json!(0)),
            CatalogDiagnosticCode::LimitExceeded,
            at("/amount/fallback"),
        ),
        (
            "a fallback of 2^53",
            read(json!(9_007_199_254_740_992_u64)),
            CatalogDiagnosticCode::LimitExceeded,
            at("/amount/fallback"),
        ),
        (
            "a fallback above the constant capacity",
            read(json!(121)),
            CatalogDiagnosticCode::EffectIncompatible,
            at(""),
        ),
        (
            "no type",
            untyped,
            CatalogDiagnosticCode::MissingRequiredField,
            at("/amount"),
        ),
        (
            "no key",
            no_key,
            CatalogDiagnosticCode::MissingRequiredField,
            at("/amount"),
        ),
        (
            "no fallback",
            no_fallback,
            CatalogDiagnosticCode::MissingRequiredField,
            at("/amount"),
        ),
        (
            "an offset",
            offset,
            CatalogDiagnosticCode::UnknownField,
            at("/amount/offset"),
        ),
        (
            "a scope",
            scoped,
            CatalogDiagnosticCode::UnknownField,
            at("/amount/scope"),
        ),
        (
            "another member",
            extra,
            CatalogDiagnosticCode::UnknownField,
            at("/amount/margin"),
        ),
    ];
    for (case, amount, code, path) in cases {
        let sources = with_consumes(SCHEDULING_SCHEMA_VERSION_V2, consumes(amount));
        assert!(
            refused(&sources, code, &path),
            "{case}: {:?}",
            diagnostics(&sources)
        );
    }
    let mut expected = entry(read(json!(3)));
    expected["expected_amount_milli"] = json!(400);
    let beside = with_consumes(SCHEDULING_SCHEMA_VERSION_V2, json!([expected]));
    assert!(
        refused(&beside, CatalogDiagnosticCode::LimitExceeded, &at("")),
        "{:?}",
        diagnostics(&beside)
    );
    let produced = example(SCHEDULING_SCHEMA_VERSION_V2, |tasks, _| {
        tasks["tasks"][0]["produces"][0]["amount"] =
            json!({"type": "fact", "fact_key": COST_KEY, "fallback": 3});
    });
    assert!(
        refused(
            &produced,
            CatalogDiagnosticCode::EffectIncompatible,
            "/tasks/0/produces/0/amount"
        ),
        "{:?}",
        diagnostics(&produced)
    );
    let twice = with_consumes(
        SCHEDULING_SCHEMA_VERSION_V2,
        json!([entry(read(json!(3))), entry(read(json!(4)))]),
    );
    assert!(
        refused(
            &twice,
            CatalogDiagnosticCode::EffectIncompatible,
            "/tasks/0/consumes/1/amount"
        ),
        "{:?}",
        diagnostics(&twice)
    );
    for compiles in [
        consumes(read(json!(1))),
        consumes(read(json!(120))),
        json!([entry(read(json!(3))), entry(json!(2))]),
    ] {
        compile_catalog(&with_consumes(
            SCHEDULING_SCHEMA_VERSION_V2,
            compiles.clone(),
        ))
        .unwrap_or_else(|failure| panic!("{compiles}: {:?}", failure.diagnostics()));
    }

    for (version, golden) in [
        (SCHEDULING_SCHEMA_VERSION, V1_EXAMPLE_HASH),
        (SCHEDULING_SCHEMA_VERSION_V2, V2_EXAMPLE_HASH),
    ] {
        let plain = compile_catalog(&example(version, |_, _| {}))
            .unwrap_or_else(|failure| panic!("{version}: {:?}", failure.diagnostics()));
        assert_eq!(plain.catalog_hash(), golden, "{version}");
    }
    let written = entry(read(json!(3)));
    let with_form = with_consumes(SCHEDULING_SCHEMA_VERSION_V2, json!([written.clone()]));
    let compiled = compile_catalog(&with_form)
        .unwrap_or_else(|failure| panic!("the form compiles: {:?}", failure.diagnostics()));
    assert_ne!(compiled.catalog_hash(), V2_EXAMPLE_HASH);
    assert_eq!(
        compile_catalog(&with_form)
            .expect("the form compiles again")
            .catalog_hash(),
        compiled.catalog_hash()
    );
    let consumed = &compiled.catalog().tasks.tasks[0].consumes[0];
    assert_eq!(
        consumed.fact_amount(),
        Some(&ConsumptionFact {
            fact_key: COST_KEY.to_owned(),
            fallback: 3,
        })
    );
    assert_eq!(consumed.units(), None);
    assert_eq!(
        serde_json::to_value(consumed).expect("the entry serializes"),
        written
    );
}

// Test 2, one evaluation. Task A reads its amount from `resource.neutral.cost` with the fallback 10
// and produces 30; read at 110 the pool is below its cap and a run of 7 or more units would leave it
// below its hard line (104), so A is held (`would_cross_hard_line`) with the amount its decision
// reads (`consumption`) and where it comes from (`consumption_source`). A record of 20: 20, `fact`,
// no warning. No record, a record past its own expiry, or one a reset (half an hour ago) invalidated:
// the fallback 10, `fallback`, no warning. A record that is present and current but at a confidence
// of 0, of another kind, or an integer out of range (0): the fallback, with
// `consumption_fact_unusable` {task_id, pool_id, fact_key, unknown_reason `fact_low_confidence` or
// `type_mismatch`, consumption 10} as the decision's first reason.
#[test]
fn sched_c1f1_t2_one_evaluation_reads_a_usable_record_or_the_fallback() {
    let cost =
        |value: FactValue, observed: u64, expires: Option<u64>, confidence: u16| ObservedFact {
            scope: ScopeSelector::Instance {
                instance_id: A.to_owned(),
            },
            fact_key: COST_KEY.to_owned(),
            value,
            observed_at_unix_ms: observed,
            expires_at_unix_ms: expires,
            confidence_milli: confidence,
        };
    let earlier = NOW - 2 * HOUR_MS;
    let cases = [
        (
            "a usable record",
            Some(cost(FactValue::Integer(20), earlier, None, 1000)),
            false,
            20,
            "fact",
            None,
        ),
        ("no record", None, false, 10, "fallback", None),
        (
            "an expired record",
            Some(cost(
                FactValue::Integer(20),
                earlier,
                Some(NOW - MINUTE_MS),
                1000,
            )),
            false,
            10,
            "fallback",
            None,
        ),
        (
            "a record a reset invalidated",
            Some(cost(FactValue::Integer(20), earlier, None, 1000)),
            true,
            10,
            "fallback",
            None,
        ),
        (
            "a record at a confidence of 0",
            Some(cost(FactValue::Integer(20), earlier, None, 0)),
            false,
            10,
            "fallback",
            Some("fact_low_confidence"),
        ),
        (
            "a record of another kind",
            Some(cost(
                FactValue::String("twenty".to_owned()),
                earlier,
                None,
                1000,
            )),
            false,
            10,
            "fallback",
            Some("type_mismatch"),
        ),
        (
            "an integer out of range",
            Some(cost(FactValue::Integer(0), earlier, None, 1000)),
            false,
            10,
            "fallback",
            Some("type_mismatch"),
        ),
    ];
    for (case, record, reset, consumption, source, unusable) in cases {
        let events = if reset {
            vec![reset_of(COST_KEY)]
        } else {
            Vec::new()
        };
        let mut world = World::new(
            catalog_with(vec![reading_driven("task_a", 10, 30)], pools(), events),
            110,
        );
        world.extra.extend(record);
        let evaluation = world.evaluate();
        assert!(
            dispatched(&evaluation).is_empty(),
            "{case}: {evaluation:#?}"
        );
        let decided = decision(&evaluation, "task_a");
        assert_eq!(
            hold(decided).as_deref(),
            Some("would_cross_hard_line"),
            "{case}"
        );
        let held = &last(decided).outcome;
        assert_eq!(
            held.value("consumption"),
            Some(&json!(consumption)),
            "{case}"
        );
        assert_eq!(
            held.value("consumption_source"),
            Some(&json!(source)),
            "{case}"
        );
        assert_eq!(
            held.value("amount_after"),
            Some(&json!(110 - consumption)),
            "{case}"
        );
        let warnings = decided
            .reasons
            .iter()
            .filter(|reason| reason.outcome.is(PolicyCode::ConsumptionFactUnusable))
            .count();
        match unusable {
            None => assert_eq!(warnings, 0, "{case}: {decided:#?}"),
            Some(unknown_reason) => {
                assert_eq!(warnings, 1, "{case}: {decided:#?}");
                let first = &decided.reasons[0].outcome;
                assert!(first.is(PolicyCode::ConsumptionFactUnusable), "{case}");
                for (key, value) in [
                    ("task_id", json!("task_a")),
                    ("pool_id", json!(POOL)),
                    ("fact_key", json!(COST_KEY)),
                    ("unknown_reason", json!(unknown_reason)),
                    ("consumption", json!(10)),
                ] {
                    assert_eq!(first.value(key), Some(&value), "{case}: {key}");
                }
            }
        }
    }
}

// Test 3, every reader reads the amount in effect. A reads 20 from its record (fallback 10) in
// each case, and every reader reads 20:
// - the net benefit: at 170 A runs with `B` = 30 000 − 20 000 = 10 000 (20 000 on the fallback);
// - the hard line: at 120 A is held, `amount_after` 100, `consumption` 20 `fact`;
// - a pool its lines cannot spend: with a record of 80, read at the cap (180) no run of 80 fits
//   above the hard line (76), so A is deferred with `pool_lines_unspendable` (`least_consumption`
//   80, `consumption` 80 `fact`), while on its fallback an episode begins and A runs; the compile
//   warning reads the fallback: 80 against constant lines (room 76) warns, 10 does not;
// - a capped round's order and plan: at 200 beside B (10 units, producing 25), A's `V ÷ c` is 1500
//   against B's 2500, so B is the head and A waits behind it (`capped_not_head`, `consumption` 20);
//   on its fallback A (3000) is the head and its `capped_spend` says `consumption` 10 `fallback`;
// - a reserve of 160 at 170: `amount_after` 150 holds A (`resource_reserve_insufficient`,
//   `consumption` 20 `fact`), where its fallback would leave 160; a reserve of 100 admits it with
//   SS-6's `run_cap` ⌊70 ÷ 20⌋ = 3 (7 on the fallback);
// - the forward projection's first step leaves the pool at 150 (160 on the fallback).
#[test]
fn sched_c1f1_t3_every_reader_reads_the_amount_in_effect() {
    let a = || reading_driven("task_a", 10, 30);
    let with_cost = |world: &mut World, value: i64| world.extra.push(cost_fact(value));

    let mut benefit = World::new(catalog(vec![a()], pools()), 170);
    let fallback = benefit.evaluate();
    with_cost(&mut benefit, 20);
    let read = benefit.evaluate();
    for (evaluation, expected) in [(&read, 10_000), (&fallback, 20_000)] {
        assert_eq!(dispatched(evaluation), ["task_a"], "{evaluation:#?}");
        let breakdown = reason(decision(evaluation, "task_a"), PolicyCode::RankBreakdown);
        assert_eq!(
            breakdown.outcome.value("benefit_milli"),
            Some(&json!(expected))
        );
    }

    let mut hard = World::new(catalog(vec![a()], pools()), 120);
    with_cost(&mut hard, 20);
    let held = hard.evaluate();
    let decided = decision(&held, "task_a");
    assert_eq!(hold(decided).as_deref(), Some("would_cross_hard_line"));
    assert_eq!(
        last(decided).outcome.value("amount_after"),
        Some(&json!(100))
    );
    assert_eq!(last(decided).outcome.value("consumption"), Some(&json!(20)));
    assert_eq!(
        last(decided).outcome.value("consumption_source"),
        Some(&json!("fact"))
    );

    let mut full = World::new(catalog(vec![a()], pools()), 180);
    let begun = full.clone_world().evaluate();
    assert_eq!(dispatched(&begun), ["task_a"], "{begun:#?}");
    let head = spend(&begun, "task_a");
    assert_eq!(head.value("consumption"), Some(&json!(10)));
    assert_eq!(head.value("consumption_source"), Some(&json!("fallback")));
    with_cost(&mut full, 80);
    let unspendable = full.evaluate();
    assert!(dispatched(&unspendable).is_empty(), "{unspendable:#?}");
    let warned = &last(decision(&unspendable, "task_a")).outcome;
    assert!(warned.is(PolicyCode::PoolLinesUnspendable), "{warned:?}");
    for (key, value) in [
        ("least_consumption", json!(80)),
        ("consumption", json!(80)),
        ("consumption_source", json!("fact")),
    ] {
        assert_eq!(warned.value(key), Some(&value), "{key}");
    }
    let constants = || {
        let mut pool = valued(POOL, 1000, 180);
        pool["stop_line"] = json!(140);
        pool["hard_line"] = json!(104);
        json!([out_pool(), pool])
    };
    let warns = catalog(vec![reading_driven("task_a", 80, 30)], constants());
    let found = warns
        .warnings()
        .iter()
        .filter(|warning| warning.code == CatalogDiagnosticCode::PoolLinesUnspendable)
        .collect::<Vec<_>>();
    let [warning] = found.as_slice() else {
        panic!("one compile warning: {:?}", warns.warnings());
    };
    assert!(
        warning
            .reason
            .contains("capacity_less_hard_line=76 least_consumption=80"),
        "{}",
        warning.reason
    );
    let quiet = catalog(vec![a()], constants());
    assert!(
        quiet
            .warnings()
            .iter()
            .all(|warning| warning.code != CatalogDiagnosticCode::PoolLinesUnspendable),
        "{:?}",
        quiet.warnings()
    );

    let two = || catalog(vec![a(), judged("task_b", 10, 25)], pools());
    let mut ordered = World::new(two(), 200);
    let on_fallback = ordered.clone_world().evaluate();
    assert_eq!(dispatched(&on_fallback), ["task_a"], "{on_fallback:#?}");
    let first = spend(&on_fallback, "task_a");
    assert_eq!(first.value("capped_rank"), Some(&json!(1)));
    assert_eq!(first.value("consumption"), Some(&json!(10)));
    assert_eq!(first.value("consumption_source"), Some(&json!("fallback")));
    with_cost(&mut ordered, 20);
    let reordered = ordered.evaluate();
    assert_eq!(dispatched(&reordered), ["task_b"], "{reordered:#?}");
    assert_eq!(
        spend(&reordered, "task_b").value("capped_rank"),
        Some(&json!(1))
    );
    assert_eq!(spend(&reordered, "task_b").value("consumption"), None);
    let waiting = decision(&reordered, "task_a");
    assert_eq!(hold(waiting).as_deref(), Some("capped_not_head"));
    assert_eq!(
        last(waiting).outcome.value("task_id"),
        Some(&json!("task_b"))
    );
    assert_eq!(last(waiting).outcome.value("consumption"), Some(&json!(20)));
    assert_eq!(
        last(waiting).outcome.value("consumption_source"),
        Some(&json!("fact"))
    );

    let reserved = |at_least: u64| {
        let mut task = a();
        task["reserves"] = json!([{"pool_id": POOL, "at_least": at_least}]);
        catalog(vec![task], pools())
    };
    let mut tight = World::new(reserved(160), 170);
    assert_eq!(
        dispatched(&tight.clone_world().evaluate()),
        ["task_a"],
        "on the fallback 160 is left"
    );
    with_cost(&mut tight, 20);
    let short = tight.evaluate();
    assert!(dispatched(&short).is_empty(), "{short:#?}");
    let insufficient = &last(decision(&short, "task_a")).outcome;
    assert!(
        insufficient.is(PolicyCode::ResourceReserveInsufficient),
        "{insufficient:?}"
    );
    for (key, value) in [
        ("amount_after", json!(150)),
        ("reserve_at_least", json!(160)),
        ("consumption", json!(20)),
        ("consumption_source", json!("fact")),
    ] {
        assert_eq!(insufficient.value(key), Some(&value), "{key}");
    }
    let mut roomy = World::new(reserved(100), 170);
    let run_cap = |evaluation: &PolicyEvaluation| {
        reason(
            decision(evaluation, "task_a"),
            PolicyCode::PolicyDispatchParameters,
        )
        .outcome
        .value("run_cap")
        .cloned()
    };
    assert_eq!(run_cap(&roomy.clone_world().evaluate()), Some(json!(7)));
    with_cost(&mut roomy, 20);
    assert_eq!(run_cap(&roomy.evaluate()), Some(json!(3)));

    let projected = catalog(vec![a()], pools());
    let first_step = |cost: Option<i64>| {
        let mut world = World::new(projected.clone(), 170);
        world.extra.push(fact(A, OUT_KEY, 0, NOW - HOUR_MS, None));
        world.extra.extend(cost.map(cost_fact));
        let facts = world.facts();
        let projection = project_forward(
            &projected,
            &facts,
            &resources(&projected, &facts),
            time(NOW),
            11,
            ForwardProjectionConfig {
                horizon_ms: HOUR_MS,
                max_steps: 2,
            },
        )
        .unwrap_or_else(|error| panic!("the projection runs: {error}"));
        let step = projection
            .steps
            .first()
            .unwrap_or_else(|| panic!("a first step: {projection:#?}"));
        assert_eq!(step.task_ids, ["task_a"], "{projection:#?}");
        step.pools
            .iter()
            .find(|pool| pool.pool_id == POOL)
            .map(|pool| pool.value)
    };
    assert_eq!(first_step(Some(20)), Some(150));
    assert_eq!(first_step(None), Some(160));
}

// Test 4, an episode under way. A (fallback 10, producing 30, `V ÷ c` 3000) and B (10 units,
// producing 40, 4000) at 200 freeze B then A; B runs as the head of episode 1, round 1.
// - B's run took 10 (190) and A's record now reads 80: the consumers key holds A's fallback, so the
//   episode and its order stay, the plan reads 80 (200 → 190 → 110), and A runs as rank 2 of round 1,
//   its `capped_spend` saying `consumption` 80 `fact`.
// - B's run took 20 (180) and A's record reads 80: the plan from the round's start still fits A, but
//   the head's check reads the new amount (180 − 80 = 100 < 104), so round 1 ends; round 2 orders by
//   the amounts then and B runs as its rank 1, while A is skipped (`capped_would_cross_hard_line`,
//   `amount_after` 90, `consumption` 80).
// - A new generation in which A's fallback is 12 ends the episode: at 190 a second one begins.
// - A was a constant 10 when B ran; a new generation that replaces it by the form with the fallback
//   10 keeps the episode, and A runs as rank 2 of round 1 on its fallback.
#[test]
fn sched_c1f1_t4_an_episode_keeps_its_consumers_key_and_its_round_its_order() {
    let tasks = |fallback: u64| {
        vec![
            reading_driven("task_a", fallback, 30),
            judged("task_b", 10, 40),
        ]
    };
    let mut world = World::new(catalog(tasks(10), pools()), 200);
    let first = world.evaluate();
    assert_eq!(dispatched(&first), ["task_b"], "{first:#?}");
    assert_eq!(
        spend(&first, "task_b").value("capped_rank"),
        Some(&json!(1))
    );

    let mut kept = world.clone_world();
    kept.run(&first, 10);
    kept.extra.push(cost_fact_at(80, kept.now - 1));
    let second = kept.evaluate();
    assert_eq!(dispatched(&second), ["task_a"], "{second:#?}");
    let next = spend(&second, "task_a");
    for (key, value) in [
        ("capped_episode", json!(1)),
        ("capped_round", json!(1)),
        ("capped_rank", json!(2)),
        ("pool_reading", json!(190)),
        ("consumption", json!(80)),
        ("consumption_source", json!("fact")),
    ] {
        assert_eq!(next.value(key), Some(&value), "{key}");
    }
    assert_eq!(kept.memory.episode(A, POOL), Some((1, 1)));

    let mut checked = world.clone_world();
    checked.run(&first, 20);
    checked.extra.push(cost_fact_at(80, checked.now - 1));
    let ended = checked.evaluate();
    assert_eq!(dispatched(&ended), ["task_b"], "{ended:#?}");
    let again = spend(&ended, "task_b");
    assert_eq!(again.value("capped_episode"), Some(&json!(1)));
    assert_eq!(again.value("capped_round"), Some(&json!(2)));
    assert_eq!(again.value("capped_rank"), Some(&json!(1)));
    let skipped = decision(&ended, "task_a");
    assert_eq!(
        hold(skipped).as_deref(),
        Some("capped_would_cross_hard_line")
    );
    assert_eq!(
        last(skipped).outcome.value("amount_after"),
        Some(&json!(90))
    );
    assert_eq!(last(skipped).outcome.value("consumption"), Some(&json!(80)));

    let mut renewed = world.clone_world();
    renewed.run(&first, 10);
    renewed.catalog = catalog(tasks(12), pools());
    let restarted = renewed.evaluate();
    assert_eq!(
        renewed.memory.episode(A, POOL).map(|(number, _)| number),
        Some(2),
        "{restarted:#?}"
    );
    assert_eq!(dispatched(&restarted), ["task_b"], "{restarted:#?}");
    assert_eq!(
        spend(&restarted, "task_b").value("capped_episode"),
        Some(&json!(2))
    );

    let constant = || vec![judged("task_a", 10, 30), judged("task_b", 10, 40)];
    let mut replaced = World::new(catalog(constant(), pools()), 200);
    let before = replaced.evaluate();
    assert_eq!(dispatched(&before), ["task_b"], "{before:#?}");
    replaced.run(&before, 10);
    replaced.catalog = catalog(tasks(10), pools());
    let after = replaced.evaluate();
    assert_eq!(dispatched(&after), ["task_a"], "{after:#?}");
    let continued = spend(&after, "task_a");
    for (key, value) in [
        ("capped_episode", json!(1)),
        ("capped_round", json!(1)),
        ("capped_rank", json!(2)),
        ("consumption", json!(10)),
        ("consumption_source", json!("fallback")),
    ] {
        assert_eq!(continued.value(key), Some(&value), "{key}");
    }
    assert_eq!(replaced.memory.episode(A, POOL), Some((1, 1)));
}

// Test 5, freshness and wakes. At 170 A reads 20 from a record that expires five minutes after the
// round: its dispatch is fresh until that expiry and no later, and the evaluation wakes a
// millisecond after it, at the latest; without the record the dispatch is bounded by nothing. At
// that wake the record has lapsed and A is judged on its fallback (`B` 20 000, not 10 000), fresh
// without a bound.
#[test]
fn sched_c1f1_t5_a_usable_records_expiry_bounds_freshness_and_wakes_the_evaluation() {
    let expiry = NOW + 5 * MINUTE_MS;
    let mut unbounded = World::new(
        catalog(vec![reading_driven("task_a", 10, 30)], pools()),
        170,
    );
    let baseline = unbounded.evaluate();
    assert_eq!(fresh_until(&baseline, "task_a"), None, "{baseline:#?}");

    let mut world = World::new(
        catalog(vec![reading_driven("task_a", 10, 30)], pools()),
        170,
    );
    world.extra.push(ObservedFact {
        expires_at_unix_ms: Some(expiry),
        ..cost_fact(20)
    });
    let bounded = world.evaluate();
    assert_eq!(
        fresh_until(&bounded, "task_a"),
        Some(expiry),
        "{bounded:#?}"
    );
    let wake = baseline
        .next_wake_unix_ms
        .map_or(expiry + 1, |wake| wake.min(expiry + 1));
    assert_eq!(bounded.next_wake_unix_ms, Some(wake));
    assert_eq!(
        reason(decision(&bounded, "task_a"), PolicyCode::RankBreakdown)
            .outcome
            .value("benefit_milli"),
        Some(&json!(10_000))
    );

    world.now = expiry + 1;
    let lapsed = world.evaluate();
    assert_eq!(dispatched(&lapsed), ["task_a"], "{lapsed:#?}");
    assert_eq!(fresh_until(&lapsed, "task_a"), None);
    assert_eq!(
        reason(decision(&lapsed, "task_a"), PolicyCode::RankBreakdown)
            .outcome
            .value("benefit_milli"),
        Some(&json!(20_000))
    );
}

// Test 6, the composer. A `schedule.json` rule whose `consumes` entry writes the form (fallback 3)
// beside `consumption_fact` in `requires` composes: the composed task carries the `amount` as
// written, with no scope, and compiles into the form; the judged rule is complete in the coverage,
// and `consumption_facts` lists it {rule_id, pool_id, fact_key, fallback}, while a rule with a
// constant amount is not listed. Without `consumption_fact` in `requires` the file is refused,
// `schedule_invalid` at `/rules/0/consumes/0/amount`; a `scope` in the form is refused by the reader
// at its member, and a fallback of 0 by the compiler, mapped back to `/rules/0/consumes/0/amount/
// fallback`.
#[test]
fn sched_c1f1_t6_the_composer_carries_the_form_and_requires_its_subsystem() {
    let form = |fallback: u64| json!({"type": "fact", "fact_key": COST_KEY, "fallback": fallback});
    let rule = |id: &str, amount: Value| {
        let mut rule = schedule_rule(id);
        rule["consumes"][0]["amount"] = amount;
        rule
    };
    let file = |amount: Value, requires: &[&str]| {
        schedule(
            vec![rule("rule.a", amount), rule("rule.b", json!(2))],
            vec![schedule_pool(OUT), schedule_pool(POOL)],
            requires,
        )
    };
    let composed = compose(&file(form(3), &[CONSUMPTION_FACT_SUBSYSTEM]))
        .unwrap_or_else(|failure| panic!("the file composes: {failure}"));
    let tasks = serde_json::from_slice::<Value>(&composed.sources().tasks.bytes).expect("tasks");
    assert_eq!(tasks["tasks"][0]["consumes"][0]["amount"], form(3));
    assert_eq!(
        composed.compiled().catalog().tasks.tasks[0].consumes[0].fact_amount(),
        Some(&ConsumptionFact {
            fact_key: COST_KEY.to_owned(),
            fallback: 3,
        })
    );
    let coverage = composed
        .benefit_coverage()
        .into_iter()
        .find(|(game, _)| game == GAME)
        .map(|(_, coverage)| coverage)
        .expect("the game's coverage");
    assert_eq!((coverage.complete, coverage.undeclared), (2, 0));
    assert_eq!(
        composed.consumption_facts(),
        [(
            GAME.to_owned(),
            vec![ConsumptionFactRule {
                rule_id: "rule.a".to_owned(),
                pool_id: POOL.to_owned(),
                fact_key: COST_KEY.to_owned(),
                fallback: 3,
            }]
        )]
    );

    let pointer_of = |failure: &crate::StandardPackComposeFailure| {
        let issue = &failure.issues()[0].issue;
        (issue.problem, issue.pointer.clone())
    };
    let unnamed = compose(&file(form(3), &[FACT_REFERENCE_SUBSYSTEM])).expect_err("requires");
    assert_eq!(
        pointer_of(&unnamed),
        (
            StandardPackProblem::ScheduleInvalid,
            "/rules/0/consumes/0/amount".to_owned()
        )
    );
    let mut scoped = form(3);
    scoped["scope"] = json!({"kind": "instance", "instance_id": A});
    let reader = compose(&file(scoped, &[CONSUMPTION_FACT_SUBSYSTEM])).expect_err("a scope");
    assert_eq!(
        pointer_of(&reader),
        (
            StandardPackProblem::ScheduleInvalid,
            "/rules/0/consumes/0/amount/scope".to_owned()
        )
    );
    let compiler = compose(&file(form(0), &[CONSUMPTION_FACT_SUBSYSTEM])).expect_err("0");
    assert!(
        compiler.issues().iter().any(|refused| {
            refused.issue.problem == StandardPackProblem::ScheduleInvalid
                && refused.issue.pointer == "/rules/0/consumes/0/amount/fallback"
        }),
        "{compiler}"
    );
    let none = compose(&file(json!(3), &[])).unwrap_or_else(|failure| panic!("{failure}"));
    assert_eq!(none.consumption_facts(), [(GAME.to_owned(), Vec::new())]);
}

// ---- The world a test drives ----

/// The catalog, the Runtime's memory of the capped episodes, the capped pool's reading, other facts
/// of the instance and the tasks' snapshots.
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    reading: i64,
    read_at: u64,
    now: u64,
    extra: Vec<ObservedFact>,
    states: Vec<TaskRuntimeSnapshot>,
}

impl World {
    /// The world at NOW, the capped pool read at `reading` a minute before.
    fn new(catalog: CompiledCatalog, reading: i64) -> Self {
        Self {
            catalog,
            memory: CappedMemory::default(),
            reading,
            read_at: NOW - MINUTE_MS,
            now: NOW,
            extra: Vec::new(),
            states: Vec::new(),
        }
    }

    fn clone_world(&self) -> Self {
        Self {
            catalog: self.catalog.clone(),
            memory: self.memory.clone(),
            reading: self.reading,
            read_at: self.read_at,
            now: self.now,
            extra: self.extra.clone(),
            states: self.states.clone(),
        }
    }

    fn facts(&self) -> EvaluationFacts {
        let mut facts = vec![
            fact(A, READING_KEY, self.reading, self.read_at, None),
            fact(A, CAPACITY_KEY, 180, NOW - HOUR_MS, None),
        ];
        facts.extend(self.extra.iter().cloned());
        EvaluationFacts {
            ledger_position: 7,
            fact_snapshot_id: "snapshot:fixture-sched-c1f1".to_owned(),
            facts,
            outcomes: Vec::new(),
            tasks: self.states.clone(),
            instances: vec![instance(A, &[])],
            priority_offsets: Vec::new(),
        }
    }

    /// One round by the Runtime's entry, with its memory, every candidate admitted.
    fn evaluate(&mut self) -> PolicyEvaluation {
        let facts = self.facts();
        let resources = resources(&self.catalog, &facts);
        evaluate_with_capped_spending::<PolicyEvaluationError>(
            &self.catalog,
            &facts,
            &resources,
            time(self.now),
            11,
            &BTreeSet::new(),
            &refresh_inputs(),
            &RemedyPlan::default(),
            &ChainHandoverInputs::default(),
            &RunSlots::default(),
            &OwedReruns::default(),
            &InstanceHolds::default(),
            &NoProgressGuards::default(),
            &mut self.memory,
            |_, _, _| Ok(false),
            |_: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
                Ok(CandidateEligibility::Eligible)
            },
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
    }

    /// Runs the round's one dispatch: it succeeds having consumed `spent`; the pool is read half a
    /// minute after the dispatch and the next round is a minute after it.
    fn run(&mut self, evaluation: &PolicyEvaluation, spent: i64) {
        let [intent] = evaluation.dispatch_intents.as_slice() else {
            panic!("one dispatch: {evaluation:#?}");
        };
        self.states.retain(|state| state.task_id != intent.task_id);
        self.states.push(TaskRuntimeSnapshot {
            task_id: intent.task_id.clone(),
            instance_id: A.to_owned(),
            last_dispatched_unix_ms: Some(self.now),
            eligible_since_unix_ms: None,
            terminal_state: Some(TaskTerminalState::Succeeded),
            completed_window: None,
            last_duration_ms: None,
            failure_streak: None,
            expected_runtime_ms: None,
            expected_runtime_runs: None,
        });
        self.reading -= spent;
        self.read_at = self.now + 30_000;
        self.now += MINUTE_MS;
    }
}

/// A's record of one run's amount, `value`, read an hour before the round, with no expiry.
fn cost_fact(value: i64) -> ObservedFact {
    cost_fact_at(value, NOW - HOUR_MS)
}

/// A's record of one run's amount, `value`, read at `observed`, with no expiry.
fn cost_fact_at(value: i64, observed: u64) -> ObservedFact {
    fact(A, COST_KEY, value, observed, None)
}

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

fn resources(catalog: &CompiledCatalog, facts: &EvaluationFacts) -> EvaluationResources {
    project_fact_pools(
        catalog,
        facts,
        &EvaluationResources {
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
        instance_holds: BTreeMap::new(),
        idle_closed: BTreeMap::new(),
    }
}

// ---- Assertions ----

fn dispatched(evaluation: &PolicyEvaluation) -> Vec<&str> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| intent.task_id.as_str())
        .collect()
}

/// The freshness bound of `task_id`'s dispatch.
fn fresh_until(evaluation: &PolicyEvaluation, task_id: &str) -> Option<u64> {
    evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id)
        .unwrap_or_else(|| panic!("a dispatch of {task_id}: {evaluation:#?}"))
        .prerequisites
        .facts_fresh_until_unix_ms
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == task_id && decision.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}: {evaluation:#?}"))
}

fn reason(decision: &TaskDecision, code: PolicyCode) -> &DecisionReason {
    decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.is(code))
        .unwrap_or_else(|| panic!("a {} reason on {decision:#?}", code.as_str()))
}

fn last(decision: &TaskDecision) -> &DecisionReason {
    decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {decision:#?}"))
}

/// The `pool_line_hold` of the decision's last reason, when it is `pool_line_held`.
fn hold(decision: &TaskDecision) -> Option<String> {
    let last = last(decision);
    if !last.outcome.is(PolicyCode::PoolLineHeld) {
        return None;
    }
    last.outcome
        .value("pool_line_hold")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The `capped_spend` reason of `task_id`'s dispatch.
fn spend<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a Outcome {
    &reason(decision(evaluation, task_id), PolicyCode::CappedSpend).outcome
}

/// Whether compiling `sources` is refused with `code` at `path`.
fn refused(sources: &CatalogSources, code: CatalogDiagnosticCode, path: &str) -> bool {
    compile_catalog(sources).err().is_some_and(|failure| {
        failure
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == code && diagnostic.json_path == path)
    })
}

/// The diagnostics of compiling `sources`, for a failed assertion's message.
fn diagnostics(sources: &CatalogSources) -> Vec<(CatalogDiagnosticCode, String)> {
    match compile_catalog(sources) {
        Ok(_) => Vec::new(),
        Err(failure) => failure
            .diagnostics()
            .iter()
            .map(|diagnostic| (diagnostic.code, diagnostic.json_path.clone()))
            .collect(),
    }
}

// ---- The catalog ----

fn effect(pool: &str, direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// Due while the capped pool holds `amount`.
fn enough(amount: u64) -> Value {
    json!({
        "kind": "resource_projection",
        "pool_id": POOL,
        "comparison": "greater_than_or_equal",
        "value": amount
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

/// An unmarked task of the game on its hourly clock, of `priority`, with no effect.
fn task_body(id: &str, priority: i16) -> Value {
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
        "expected_duration_ms": 60_000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// A judged task: `consumed` units of the capped pool a run, due while the pool holds them, and
/// `produced` units of `pool.out`.
fn judged(id: &str, consumed: u64, produced: u64) -> Value {
    let mut task = task_body(id, 100);
    task["consumes"] = json!([effect(POOL, "consume", consumed)]);
    task["produces"] = json!([effect(OUT, "produce", produced)]);
    task["benefit_judged"] = json!(true);
    task["trigger"] = enough(consumed);
    task
}

/// A judged task whose run's amount of the capped pool is read from `resource.neutral.cost`, with
/// `fallback`; due while the pool holds a unit, producing `produced` units of `pool.out`.
fn reading_driven(id: &str, fallback: u64, produced: u64) -> Value {
    let mut task = judged(id, 1, produced);
    task["consumes"][0]["amount"] =
        json!({"type": "fact", "fact_key": COST_KEY, "fallback": fallback});
    task
}

/// A pool of the instance that does not regenerate, read from `resource.<the id without
/// "pool.">`, of constant `capacity`, weighed `weight` milli a unit.
fn valued(id: &str, weight: u32, capacity: u64) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": capacity,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": weight}
    })
}

fn out_pool() -> Value {
    valued(OUT, 1000, 1_000_000)
}

/// The capacity fact plus `offset`.
fn line(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

/// The produced pool and the capped pool: its capacity read from its fact, its stop line 40 below
/// the cap and its hard line 76 below it.
fn pools() -> Value {
    let mut capped = valued(POOL, 1000, 1);
    capped["capacity"] = line(0);
    capped["stop_line"] = line(-40);
    capped["hard_line"] = line(-76);
    json!([out_pool(), capped])
}

/// The daily reset of the instance, half an hour before the round, invalidating `prefix`.
fn reset_of(prefix: &str) -> Value {
    json!({
        "id": RESET_ID,
        "scope": {"kind": "instance", "instance_id": A},
        "event_kind": "reset",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": DAY_MS,
            "anchor_ms": RESET_AT
        },
        "duration_ms": 0,
        "invalidates_fact_prefixes": [prefix],
        "validity": {"from_unix_ms": 0, "until_unix_ms": null}
    })
}

fn catalog(tasks: Vec<Value>, pools: Value) -> CompiledCatalog {
    catalog_with(tasks, pools, Vec::new())
}

fn catalog_with(tasks: Vec<Value>, pools: Value, events: Vec<Value>) -> CompiledCatalog {
    compile_v2(tasks, pools, events).unwrap_or_else(|failure| {
        panic!("the fixture catalog compiles: {:?}", failure.diagnostics())
    })
}

fn compile_v2(
    tasks: Vec<Value>,
    pools: Value,
    events: Vec<Value>,
) -> Result<CompiledCatalog, CatalogCompileFailure> {
    let header = json!({
        "catalog_id": "fixture.sched-c1f1",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-sched-c1f1"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": SCHEDULING_SCHEMA_VERSION_V2,
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
        timeline: source("timeline", &document("events", Value::Array(events))),
        selection: None,
    })
}

/// The neutral example catalog `catalog-a` declared as `version` (V2: every timeline event valid
/// from 0, as its V2 golden was computed), its tasks and pools documents changed by `edit`.
fn example(version: &str, edit: impl FnOnce(&mut Value, &mut Value)) -> CatalogSources {
    let parse = |bytes: &[u8]| -> Value { serde_json::from_slice(bytes).expect("example JSON") };
    let mut documents = [
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/tasks.json"
        )),
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/pools.json"
        )),
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/activity.json"
        )),
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/timeline.json"
        )),
    ];
    for declared in &mut documents {
        declared["schema_version"] = json!(version);
        if version == SCHEDULING_SCHEMA_VERSION_V2
            && let Some(events) = declared.get_mut("events").and_then(Value::as_array_mut)
        {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
    }
    let [mut tasks, mut pools, activity, timeline] = documents;
    edit(&mut tasks, &mut pools);
    CatalogSources {
        tasks: source("example-tasks", &tasks),
        pools: source("example-pools", &pools),
        activity: source("example-activity", &activity),
        timeline: source("example-timeline", &timeline),
        selection: None,
    }
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-sched-c1f1/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

// ---- The standard-pack fixture of test 6 ----

/// A `schedule.json` rule running the package of its own id hourly, consuming 2 of the capped pool
/// and producing one `pool.out`.
fn schedule_rule(id: &str) -> Value {
    let mut rule = task_body(id, 100);
    rule["consumes"] = json!([effect(POOL, "consume", 2)]);
    rule["produces"] = json!([effect(OUT, "produce", 1)]);
    let members = rule.as_object_mut().expect("a task object");
    for member in ["scope", "entrypoint", "procedure_ref", "instance_overrides"] {
        members.shift_remove(member);
    }
    members.insert("package_id".to_owned(), json!(id));
    rule
}

/// A `schedule.json` pool: `id` of constant capacity 240, valued, without a scope.
fn schedule_pool(id: &str) -> Value {
    let mut pool = valued(id, 1000, 240);
    pool.as_object_mut()
        .expect("a pool object")
        .shift_remove("scope");
    pool
}

/// `schedule.json` with these rules and pools, naming `requires`.
fn schedule(rules: Vec<Value>, pools: Vec<Value>, requires: &[&str]) -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": rules,
        "pools": pools,
        "activity": [],
        "timeline": [],
        "held": []
    })
}

/// Composes `schedule` for one instance of the game, whose registry holds a package per rule and
/// `rule.maint`, a return-home maintenance package; no Runtime manifest checks `requires`.
fn compose(schedule: &Value) -> Result<StandardPackComposition, crate::StandardPackComposeFailure> {
    let packages = schedule["rules"]
        .as_array()
        .expect("the rules")
        .iter()
        .map(|rule| rule["package_id"].as_str().expect("a package").to_owned())
        .chain(["rule.maint".to_owned()])
        .collect::<BTreeSet<_>>();
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
    let schedule = serde_json::to_vec(schedule).expect("schedule bytes");
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &schedule,
            root: &root,
            maintenance: &maintenance,
        },
    )]);
    let instances = BTreeMap::from([(
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
