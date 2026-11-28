// SPDX-License-Identifier: AGPL-3.0-only

// #455 SL1 (0.12.97; register §1.540; the coordinator's rulings of 11-28, after the owner's words
// that such a currency has a line; coordinator work order `rd-1123/brief-455-sl1.md`;
// contracts/scheduling/README.md, "A pool whose spending a line governs" and "Net benefit within a
// tier"; contracts/standard-pack-schedule.md, "`requires`"): a pool that declares no valuation may
// declare that a line governs its spending (`spend_line`); a judged task that consumes such a pool
// is judged, and while no line is set the consumption costs nothing.
// - the reader takes the three numbers and the V2 schema lists them with the compiler's bounds;
//   each missing, each out of its bound, another member, the member beside a valuation block, the
//   pending marker or `target_defaults` and the member in a V1 document are refused at their
//   pointers; a `schedule.json` that writes it without `pool_spend_line` in `requires` is refused,
//   and a Runtime that does not name the subsystem refuses one that does (test 1);
// - a judged task with a fixed value that consumes such a pool is `B = V` and dispatched, and
//   `benefit_unknown` `valuation_missing` on the same catalogue without the member; beside a valued
//   pool only the valued one costs and bounds `ε`, the item `:-0` at its place; without a fixed
//   value a production term is the benefit; a pending amount on it is `entry_pending`, a producer
//   of it `valuation_missing`, a consumer of it alone `policy_benefit_nonpositive` (test 2);
// - the reserve gate holds a consumer whose reading is short as it holds any (test 3);
// - a catalogue without the member keeps its golden identity (test 4).
//
// The evaluation fixture: one instance, the window open all day, judged tasks on an hourly clock;
// the pool whose spending a line governs (`pool.currency`, no valuation, capacity 100 000) read at
// 500 and a valued pool (`pool.neutral`, 1000 milli a unit at the step 1, capacity 10 000, no
// lines) read at 5000, both a minute before NOW. Names are made up; no clock is read.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{StandardPackProblem, StandardPackRootBytes, read_standard_pack_root};
use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CandidateEligibility, CappedMemory, CatalogDiagnosticCode, CatalogDocumentSource,
    CatalogSources, ChainHandoverInputs, CompiledCatalog, DecisionReason, DispatchIntent,
    EvaluationFacts, EvaluationResources, EvaluationTime, FactRefreshInputs, FactRefreshSettings,
    HostResourceSnapshot, InstanceHolds, MAX_CANONICAL_INTEGER, NoProgressGuards, OwedReruns,
    POOL_SPEND_LINE_SUBSYSTEM, PolicyCode, PolicyEvaluation, PolicyEvaluationError, PoolSpendLine,
    PoolsDocument, RemedyPlan, RunSlots, SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2,
    StandardPackBinding, StandardPackComposeInput, StandardPackComposition, StandardPackGameFiles,
    TaskDecision, compile_catalog, compose_standard_pack_rules, evaluate_with_capped_spending,
    project_fact_pools,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// The pool whose spending a line governs, without a valuation, read from `CURRENCY_KEY`.
const CURRENCY: &str = "pool.currency";
const CURRENCY_KEY: &str = "resource.currency";
/// A valued pool, 1000 milli a unit at the step 1, without lines, read from `VALUED_KEY`.
const VALUED: &str = "pool.neutral";
const VALUED_KEY: &str = "resource.neutral";
/// The work order's three numbers: `price_milli`, `scale`, `income_floor_milli_per_day`.
const LINE: (u64, u64, u64) = (33_333, 6000, 71_429);
/// The largest safe integer, the bound of each number.
const SAFE: u64 = 9_007_199_254_740_991;
/// The pool whose spending a line governs at 500 and the valued pool at 5000.
const READINGS: [(&str, i64); 2] = [(CURRENCY_KEY, 500), (VALUED_KEY, 5000)];
/// The declared duration of every task, `E` without a snapshot.
const E: u64 = 60_000;
/// The neutral example catalog's golden identities (as in `gate_455_b1` and `gate_455_b3p`).
const V1_EXAMPLE_HASH: &str =
    "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c";
const V2_EXAMPLE_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";

// Test 1, the reader. The three numbers on the example's pool compile: the compiled pool holds
// them, the pools document writes them back as written and reads back equal, and compiles again to
// the same identity (which the member is part of); the largest bounds and the least are taken.
// Refused, each at its pointer: in a V1 document `unknown_field` at the member; each number
// missing, `missing_required_field` naming it; a price or a scale of 0 or 2^53 and an income floor
// of 2^53, `limit_exceeded` at the number; another member, `unknown_field` naming it; beside a
// valuation block, the pending marker or `target_defaults`, `type_mismatch` at the member.
#[test]
fn sl1_t1_the_reader_takes_the_three_numbers_and_refuses_each_fault_at_its_pointer() {
    let sources = example(SCHEDULING_SCHEMA_VERSION_V2, |declared| {
        written(declared, line_json(LINE));
    });
    let compiled = compile(&sources);
    let compiled_pools = &compiled.catalog().pools;
    assert_eq!(compiled_pools.pools[0].spend_line, Some(spend_line(LINE)));
    let document = serde_json::to_value(compiled_pools).expect("the pools document serializes");
    assert_eq!(document["pools"][0]["spend_line"], line_json(LINE));
    let read = serde_json::from_value::<PoolsDocument>(document.clone())
        .expect("the pools document reads back");
    assert_eq!(&read, compiled_pools);
    let mut again = sources.clone();
    again.pools.bytes = serde_json::to_vec(&document).expect("the document's bytes");
    assert_eq!(compile(&again).catalog_hash(), compiled.catalog_hash());
    assert_ne!(
        compiled.catalog_hash(),
        V2_EXAMPLE_HASH,
        "the member is hashed"
    );
    for (case, numbers) in [
        ("the largest", (SAFE, SAFE, SAFE)),
        ("the least", (1, 1, 0)),
    ] {
        let bounded = example(SCHEDULING_SCHEMA_VERSION_V2, |declared| {
            written(declared, line_json(numbers));
        });
        assert_eq!(
            compile(&bounded).catalog().pools.pools[0].spend_line,
            Some(spend_line(numbers)),
            "{case}"
        );
    }

    use CatalogDiagnosticCode::{LimitExceeded, MissingRequiredField, TypeMismatch, UnknownField};
    let member = "/pools/0/spend_line";
    let v1 = example(SCHEDULING_SCHEMA_VERSION, |declared| {
        written(declared, line_json(LINE));
    });
    assert!(
        refused_at(&v1, UnknownField, member),
        "a V1 document: {:?}",
        diagnostics(&v1)
    );

    for missing in ["price_milli", "scale", "income_floor_milli_per_day"] {
        let mut present = line_json(LINE);
        present
            .as_object_mut()
            .expect("the member's object")
            .shift_remove(missing);
        let sources = example(SCHEDULING_SCHEMA_VERSION_V2, |declared| {
            written(declared, present);
        });
        let failure = compile_catalog(&sources).expect_err("a number is missing");
        let diagnostic = &failure.diagnostics()[0];
        assert_eq!(
            diagnostic.code, MissingRequiredField,
            "{missing}: {diagnostic:?}"
        );
        assert!(diagnostic.reason.contains(missing), "{diagnostic:?}");
        assert!(diagnostic.json_path.starts_with(member), "{diagnostic:?}");
    }

    let extra = example(SCHEDULING_SCHEMA_VERSION_V2, |declared| {
        written(declared, line_json(LINE));
        declared["pools"][0]["spend_line"]["extra_milli"] = json!(1);
    });
    let failure = compile_catalog(&extra).expect_err("another member");
    let diagnostic = &failure.diagnostics()[0];
    assert_eq!(diagnostic.code, UnknownField, "{diagnostic:?}");
    assert!(diagnostic.reason.contains("extra_milli"), "{diagnostic:?}");
    assert!(
        diagnostic.json_path.starts_with("/pools/0"),
        "{diagnostic:?}"
    );

    type Edit = fn(&mut Value);
    let cases: [(&str, Edit, CatalogDiagnosticCode, &str); 8] = [
        (
            "a price of 0",
            |pool| pool["spend_line"]["price_milli"] = json!(0),
            LimitExceeded,
            "/pools/0/spend_line/price_milli",
        ),
        (
            "a price of 2^53",
            |pool| pool["spend_line"]["price_milli"] = json!(9_007_199_254_740_992_u64),
            LimitExceeded,
            "/pools/0/spend_line/price_milli",
        ),
        (
            "a scale of 0",
            |pool| pool["spend_line"]["scale"] = json!(0),
            LimitExceeded,
            "/pools/0/spend_line/scale",
        ),
        (
            "a scale of 2^53",
            |pool| pool["spend_line"]["scale"] = json!(9_007_199_254_740_992_u64),
            LimitExceeded,
            "/pools/0/spend_line/scale",
        ),
        (
            "an income floor of 2^53",
            |pool| {
                pool["spend_line"]["income_floor_milli_per_day"] = json!(9_007_199_254_740_992_u64);
            },
            LimitExceeded,
            "/pools/0/spend_line/income_floor_milli_per_day",
        ),
        (
            "beside a valuation block",
            |pool| {
                pool["valuation"] = json!({"name": "Fixture", "unit": "unit", "scale": 1,
                                           "base_weight_milli": 1000});
            },
            TypeMismatch,
            "/pools/0/spend_line",
        ),
        (
            "beside the pending marker",
            |pool| pool["valuation"] = json!("pending"),
            TypeMismatch,
            "/pools/0/spend_line",
        ),
        (
            "beside target_defaults",
            |pool| pool["target_defaults"] = json!({"scale": 10, "importance_milli": 1000}),
            TypeMismatch,
            "/pools/0/spend_line",
        ),
    ];
    for (case, edit, code, path) in cases {
        let sources = example(SCHEDULING_SCHEMA_VERSION_V2, |declared| {
            written(declared, line_json(LINE));
            edit(&mut declared["pools"][0]);
        });
        assert!(
            refused_at(&sources, code, path),
            "{case}: {code:?} at {path}: {:?}",
            diagnostics(&sources)
        );
    }
}

// Test 1, the composer (ruling 6). A `schedule.json` pool that writes `spend_line`, with
// `pool_spend_line` in `requires`, composes: the composed pools document carries the member as
// written and the compiled pool its three numbers. Without the entry the file is refused,
// `schedule_invalid` at the pool's member. On a Runtime whose manifest does not name the subsystem
// (as no Runtime of 0.12.96 does) the file that lists it is refused at that entry
// (`schedule_requires_missing`).
#[test]
fn sl1_t1_the_composer_requires_pool_spend_line_and_an_older_runtime_refuses_the_file() {
    let composed = compose(&schedule(&[POOL_SPEND_LINE_SUBSYSTEM]), None)
        .unwrap_or_else(|failure| panic!("the file composes: {failure}"));
    let composed_pools = serde_json::from_slice::<Value>(&composed.sources().pools.bytes)
        .expect("the composed pools document");
    assert_eq!(
        composed_pools["pools"][0]["spend_line"],
        line_json(LINE),
        "{composed_pools}"
    );
    assert_eq!(
        composed.compiled().catalog().pools.pools[0].spend_line,
        Some(spend_line(LINE))
    );

    let failure = compose(&schedule(&[]), None).expect_err("requires");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleInvalid);
    assert_eq!(issue.pointer, "/pools/0/spend_line");

    let older = BTreeSet::from(["standard_pack_schedule".to_owned()]);
    let failure = compose(&schedule(&[POOL_SPEND_LINE_SUBSYSTEM]), Some(&older))
        .expect_err("a Runtime that does not name the subsystem");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleRequiresMissing);
    assert_eq!(issue.pointer, "/requires/0");
}

// Test 1, the schema (ruling 8). The V2 pools schema lists the member, its three numbers required
// within the bounds the compiler holds them to, and not beside a `valuation` or `target_defaults`;
// the V1 schema does not list it.
#[test]
fn sl1_t1_the_v2_pools_schema_lists_the_member_with_the_compiler_s_bounds() {
    let schema = |text: &str| -> Value { serde_json::from_str(text).expect("a schema document") };
    let v2 = schema(include_str!(
        "../../../../contracts/scheduling/v2/pools.schema.json"
    ));
    let v1 = schema(include_str!(
        "../../../../contracts/scheduling/pools.schema.json"
    ));
    assert!(
        v1["$defs"]["pool"]["properties"]
            .get("spend_line")
            .is_none()
    );
    let pool = &v2["$defs"]["pool"];
    assert_eq!(
        pool["properties"]["spend_line"]["$ref"],
        "#/$defs/spendLine"
    );
    assert_eq!(
        pool["dependentSchemas"]["spend_line"],
        json!({"not": {"anyOf": [{"required": ["valuation"]}, {"required": ["target_defaults"]}]}})
    );
    let declared = &v2["$defs"]["spendLine"];
    assert_eq!(
        declared["required"],
        json!(["price_milli", "scale", "income_floor_milli_per_day"])
    );
    assert_eq!(declared["additionalProperties"], false);
    for (field, least) in [
        ("price_milli", 1),
        ("scale", 1),
        ("income_floor_milli_per_day", 0),
    ] {
        let number = &declared["properties"][field];
        assert_eq!(number["minimum"], least, "{field}");
        assert_eq!(number["maximum"], MAX_CANONICAL_INTEGER, "{field}");
    }
}

// Test 2, the judgement (ruling 3). A judged task of `V` 500 that consumes 120 of the pool whose
// spending a line governs: `B = V = 500`, dispatched, its `rank_breakdown` naming B and V. The same
// catalogue with the member taken off the pool is today's decision: `benefit_unknown`,
// `valuation_missing`, the pool, nothing dispatched.
#[test]
fn sl1_t2_a_fixed_value_consumer_is_judged_at_its_value_and_without_the_member_as_today() {
    let task = || {
        fixed(
            judged("task_v", json!([consume(CURRENCY, 120)]), json!([])),
            500,
        )
    };
    let evaluation = evaluate(&catalog(vec![task()], pools(true)), &READINGS);
    assert_eq!(dispatched(&evaluation), ["task_v"], "{evaluation:#?}");
    let winner = decision(&evaluation, "task_v");
    fixed_first(winner, 500, 500, E);
    let breakdown = reason(winner, PolicyCode::RankBreakdown);
    assert_eq!(breakdown.outcome.value("benefit_milli"), Some(&json!(500)));
    assert!(
        breakdown
            .detail
            .ends_with(" benefit_milli=500 expected_duration_ms=60000 fixed_value_milli=500"),
        "{}",
        breakdown.detail
    );

    let evaluation = evaluate(&catalog(vec![task()], pools(false)), &READINGS);
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    unknown_on(
        decision(&evaluation, "task_v"),
        "valuation_missing",
        CURRENCY,
    );
}

// Test 2, beside a valued pool. The task consumes 120 of the pool whose spending a line governs,
// declared first, and 30 of the valued pool (W 1000, Q 1: cost 30 000, its share of `ε` 15). With
// `V` 50 000: `B = 50 000 − 30 000 = 20 000`, dispatched. With `V` 30 010: `S = 10`, within the
// valued pool's `ε` alone, so `benefit_within_tolerance` with `sum=10/1 tolerance=15/1` and the
// items `pool.currency:-0,pool.neutral:-30000`, the declared 0 at its place.
#[test]
fn sl1_t2_beside_a_valued_pool_only_the_valued_one_costs_and_bounds() {
    let task = |value| {
        fixed(
            judged(
                "task_w",
                json!([consume(CURRENCY, 120), consume(VALUED, 30)]),
                json!([]),
            ),
            value,
        )
    };
    let evaluation = evaluate(&catalog(vec![task(50_000)], pools(true)), &READINGS);
    assert_eq!(dispatched(&evaluation), ["task_w"], "{evaluation:#?}");
    let winner = decision(&evaluation, "task_w");
    fixed_first(winner, 20_000, 50_000, E);
    assert_eq!(
        reason(winner, PolicyCode::RankBreakdown)
            .outcome
            .value("benefit_milli"),
        Some(&json!(20_000))
    );

    let evaluation = evaluate(&catalog(vec![task(30_010)], pools(true)), &READINGS);
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let entry = decision(&evaluation, "task_w");
    fixed_first(entry, 10, 30_010, E);
    assert_eq!(
        reason(entry, PolicyCode::BenefitWithinTolerance).detail,
        "benefit_milli=10 tolerance_milli=15 zero_tier=unmarked sum=10/1 tolerance=15/1 \
         items=pool.currency:-0,pool.neutral:-30000"
    );
}

// Test 2, without a fixed value. A judged task that produces 2 of the valued pool (its term
// `2000·1000/(1·1000) = 2000`, `ε` 1) and consumes 120 of the pool whose spending a line governs:
// `B = 2000`, the production term, dispatched, and no `benefit_fixed_value`.
#[test]
fn sl1_t2_without_a_fixed_value_the_benefit_is_the_production_term() {
    let task = judged(
        "task_p",
        json!([consume(CURRENCY, 120)]),
        json!([produce(VALUED, 2)]),
    );
    let evaluation = evaluate(&catalog(vec![task], pools(true)), &READINGS);
    assert_eq!(dispatched(&evaluation), ["task_p"], "{evaluation:#?}");
    let winner = decision(&evaluation, "task_p");
    let breakdown = reason(winner, PolicyCode::RankBreakdown);
    assert_eq!(breakdown.outcome.value("benefit_milli"), Some(&json!(2000)));
    assert!(
        breakdown
            .detail
            .ends_with(" benefit_milli=2000 expected_duration_ms=60000"),
        "{}",
        breakdown.detail
    );
    assert!(
        !winner
            .reasons
            .iter()
            .any(|given| given.outcome.is(PolicyCode::BenefitFixedValue)),
        "{winner:#?}"
    );
}

// Test 2, the rest of ruling 3. In one catalogue: a task of `V` 500 whose consumption of the pool
// is the pending marker, `benefit_unknown` `entry_pending`; a task without a fixed value that
// produces 2 of the pool, `valuation_missing` as for any pool without a valuation; a task without a
// fixed value whose only effect is a consumption of 120 of the pool, `B = 0` exactly (`S = ε = 0`):
// `policy_benefit_nonpositive` with `benefit_milli` 0 and the item `pool.currency:-0`, not within
// tolerance. Nothing is dispatched.
#[test]
fn sl1_t2_a_pending_amount_a_producer_and_a_lone_consumer_are_judged_as_ruled() {
    let pending = fixed(
        judged(
            "task_q",
            json!([effect(CURRENCY, "consume", json!("pending"))]),
            json!([]),
        ),
        500,
    );
    let producer = judged("task_r", json!([]), json!([produce(CURRENCY, 2)]));
    let lone = judged("task_s", json!([consume(CURRENCY, 120)]), json!([]));
    let evaluation = evaluate(
        &catalog(vec![pending, producer, lone], pools(true)),
        &READINGS,
    );
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    unknown_on(decision(&evaluation, "task_q"), "entry_pending", CURRENCY);
    unknown_on(
        decision(&evaluation, "task_r"),
        "valuation_missing",
        CURRENCY,
    );
    let entry = decision(&evaluation, "task_s");
    let nonpositive = reason(entry, PolicyCode::PolicyBenefitNonpositive);
    assert_eq!(nonpositive.outcome.value("benefit_milli"), Some(&json!(0)));
    assert_eq!(
        nonpositive.detail,
        "benefit_milli=0 expected_duration_ms=60000 items=pool.currency:-0"
    );
    assert!(
        !entry
            .reasons
            .iter()
            .any(|given| given.outcome.is(PolicyCode::BenefitWithinTolerance)),
        "{entry:#?}"
    );
}

// Test 3, ruling 4: the reserve gate. A task of `V` 500 that consumes 120 of the pool and reserves
// 10 of it: read at 100, it passes the benefit gate with `B = 500` and the reserve gate holds it,
// `resource_reserve_insufficient` on the pool with `amount_after` −20, as it holds a consumer of
// any pool; read at 200 it is dispatched.
#[test]
fn sl1_t3_the_reserve_gate_holds_a_consumer_whose_reading_is_short_as_for_any_pool() {
    let task = || {
        fixed(
            with(
                judged("task_v", json!([consume(CURRENCY, 120)]), json!([])),
                "reserves",
                json!([{"pool_id": CURRENCY, "at_least": 10}]),
            ),
            500,
        )
    };
    let short = evaluate(
        &catalog(vec![task()], pools(true)),
        &[(CURRENCY_KEY, 100), (VALUED_KEY, 5000)],
    );
    assert!(dispatched(&short).is_empty(), "{short:#?}");
    let entry = decision(&short, "task_v");
    fixed_first(entry, 500, 500, E);
    let hold = &reason(entry, PolicyCode::ResourceReserveInsufficient).outcome;
    assert_eq!(hold.value("pool_id"), Some(&json!(CURRENCY)), "{entry:#?}");
    assert_eq!(hold.value("amount_after"), Some(&json!(-20)), "{entry:#?}");

    let enough = evaluate(
        &catalog(vec![task()], pools(true)),
        &[(CURRENCY_KEY, 200), (VALUED_KEY, 5000)],
    );
    assert_eq!(dispatched(&enough), ["task_v"], "{enough:#?}");
}

// Test 4, ruling 7. The neutral example catalogue, which writes no `spend_line`, keeps its V1 and
// V2 golden identities; with the member its identity differs.
#[test]
fn sl1_t4_a_catalogue_without_the_member_keeps_its_golden_identity() {
    for (version, golden) in [
        (SCHEDULING_SCHEMA_VERSION, V1_EXAMPLE_HASH),
        (SCHEDULING_SCHEMA_VERSION_V2, V2_EXAMPLE_HASH),
    ] {
        assert_eq!(
            compile(&example(version, |_| {})).catalog_hash(),
            golden,
            "{version}"
        );
    }
    let lined = example(SCHEDULING_SCHEMA_VERSION_V2, |declared| {
        written(declared, line_json(LINE));
    });
    assert_ne!(compile(&lined).catalog_hash(), V2_EXAMPLE_HASH);
}

// ---- The round a test drives ----

/// One round at NOW by the Runtime's entry, with a fresh memory, every candidate admitted, the
/// pools read as `readings` a minute before.
fn evaluate(compiled: &CompiledCatalog, readings: &[(&str, i64)]) -> PolicyEvaluation {
    let facts = EvaluationFacts {
        facts: readings
            .iter()
            .map(|(key, reading)| fact(A, key, *reading, NOW - MINUTE_MS, None))
            .collect(),
        ..bare_facts()
    };
    let resource_snapshot = resources(compiled, &facts);
    let mut memory = CappedMemory::default();
    evaluate_with_capped_spending::<PolicyEvaluationError>(
        compiled,
        &facts,
        &resource_snapshot,
        time(NOW),
        11,
        &BTreeSet::new(),
        &refresh_inputs(),
        &RemedyPlan::default(),
        &ChainHandoverInputs::default(),
        &RunSlots::default(),
        &OwedReruns::default(),
        &InstanceHolds::default(),
        &NoProgressGuards::default(),
        &mut memory,
        |_, _, _| Ok(false),
        |_: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
            Ok(CandidateEligibility::Eligible)
        },
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

/// The instance with no fact.
fn bare_facts() -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-455-sl1".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

fn resources(compiled: &CompiledCatalog, facts: &EvaluationFacts) -> EvaluationResources {
    project_fact_pools(
        compiled,
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

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|entry| entry.task_id == task_id && entry.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}: {evaluation:#?}"))
}

fn reason(entry: &TaskDecision, code: PolicyCode) -> &DecisionReason {
    entry
        .reasons
        .iter()
        .find(|given| given.outcome.is(code))
        .unwrap_or_else(|| panic!("a {} reason on {entry:#?}", code.as_str()))
}

/// The decision carries `benefit_unknown` with the gap `gap` on `pool`.
fn unknown_on(entry: &TaskDecision, gap: &str, pool: &str) {
    let unknown = &reason(entry, PolicyCode::BenefitUnknown).outcome;
    assert_eq!(
        unknown.value("benefit_gap"),
        Some(&json!(gap)),
        "{entry:#?}"
    );
    assert_eq!(unknown.value("pool_id"), Some(&json!(pool)), "{entry:#?}");
}

/// The decision opens with exactly one `benefit_fixed_value`, of `B`, `V` and `E`.
fn fixed_first(entry: &TaskDecision, benefit: i64, value: i64, expected_ms: u64) {
    let first = entry
        .reasons
        .first()
        .unwrap_or_else(|| panic!("a reason on {entry:#?}"));
    assert!(
        first.outcome.is(PolicyCode::BenefitFixedValue),
        "{entry:#?}"
    );
    assert_eq!(
        first.outcome.value("benefit_milli"),
        Some(&json!(benefit)),
        "{entry:#?}"
    );
    assert_eq!(
        first.outcome.value("fixed_value_milli"),
        Some(&json!(value))
    );
    assert_eq!(
        first.outcome.value("expected_duration_ms"),
        Some(&json!(expected_ms))
    );
    let count = entry
        .reasons
        .iter()
        .filter(|given| given.outcome.is(PolicyCode::BenefitFixedValue))
        .count();
    assert_eq!(count, 1, "{entry:#?}");
}

// ---- The catalog ----

fn effect(pool: &str, direction: &str, amount: Value) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

fn consume(pool: &str, amount: u64) -> Value {
    effect(pool, "consume", json!(amount))
}

fn produce(pool: &str, amount: u64) -> Value {
    effect(pool, "produce", json!(amount))
}

/// Due every hour (one minute past the hour at NOW).
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

/// A judged task of the game on an hourly clock with `consumes` and `produces`, of priority 100.
fn judged(id: &str, consumes: Value, produces: Value) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
        "priority": 100,
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
        "expected_duration_ms": E,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": [],
        "benefit_judged": true
    })
}

fn with(mut task: Value, name: &str, value: Value) -> Value {
    task[name] = value;
    task
}

/// `task` with the fixed value `value`.
fn fixed(task: Value, value: i64) -> Value {
    with(task, "benefit_fixed_value_milli", json!(value))
}

/// A pool of the instance that does not regenerate, read from `resource.<the id without
/// "pool.">`, of constant `capacity`, without a valuation.
fn declared_pool(id: &str, capacity: u64) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": capacity,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    })
}

/// The pool whose spending a line governs, writing [`LINE`] as its `spend_line` when `lined`
/// (without it, main's reading of the same pool), and the valued pool.
fn pools(lined: bool) -> Value {
    let mut currency = declared_pool(CURRENCY, 100_000);
    if lined {
        currency["spend_line"] = line_json(LINE);
    }
    let mut valued = declared_pool(VALUED, 10_000);
    valued["valuation"] =
        json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000});
    json!([currency, valued])
}

/// `{"price_milli": P, "scale": S, "income_floor_milli_per_day": F}`.
fn line_json((price_milli, scale, floor): (u64, u64, u64)) -> Value {
    json!({"price_milli": price_milli, "scale": scale, "income_floor_milli_per_day": floor})
}

/// The compiled member of [`line_json`]'s numbers.
fn spend_line((price_milli, scale, floor): (u64, u64, u64)) -> PoolSpendLine {
    PoolSpendLine {
        price_milli,
        scale,
        income_floor_milli_per_day: floor,
    }
}

fn catalog(tasks: Vec<Value>, pool_list: Value) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.455-sl1",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-sl1"]
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
    compile(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", pool_list)),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
}

/// The neutral example catalog `catalog-a` declared as `version` (V2: every timeline event valid
/// from 0, as its V2 golden was computed), its pools document edited by `edit`.
fn example(version: &str, edit: impl FnOnce(&mut Value)) -> CatalogSources {
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
    let [tasks, mut example_pools, activity, timeline] = documents;
    edit(&mut example_pools);
    CatalogSources {
        tasks: source("example-tasks", &tasks),
        pools: source("example-pools", &example_pools),
        activity: source("example-activity", &activity),
        timeline: source("example-timeline", &timeline),
        selection: None,
    }
}

/// The example's pools document `declared` with its pool 0 writing `line` as its `spend_line`.
fn written(declared: &mut Value, line: Value) {
    declared["pools"][0]["spend_line"] = line;
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-455-sl1/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

fn compile(sources: &CatalogSources) -> CompiledCatalog {
    compile_catalog(sources)
        .unwrap_or_else(|failure| panic!("the catalog compiles: {:?}", failure.diagnostics()))
}

/// Whether compiling `sources` is refused with `code` at `path`.
fn refused_at(sources: &CatalogSources, code: CatalogDiagnosticCode, path: &str) -> bool {
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

// ---- The composer fixture of test 1 ----

const SERVER: &str = "server-a";
/// The one rule, running the package of its own id.
const RULE: &str = "rule.plain";

/// `schedule.json` with one rule on an hourly clock, consuming nothing, and the pool whose spending
/// a line governs (no scope: the composer fills it), naming `requires`.
fn schedule(requires: &[&str]) -> Value {
    let mut rule = judged(RULE, json!([]), json!([]));
    let members = rule.as_object_mut().expect("a task object");
    for member in [
        "scope",
        "entrypoint",
        "procedure_ref",
        "instance_overrides",
        "benefit_judged",
    ] {
        members.shift_remove(member);
    }
    members.insert("package_id".to_owned(), json!(RULE));
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": [rule],
        "pools": [{
            "id": CURRENCY,
            "capacity": 100_000,
            "projection": {"kind": "none"},
            "observation": {"kind": "fact", "fact_key": CURRENCY_KEY},
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": null,
            "spend_line": line_json(LINE)
        }],
        "activity": [],
        "timeline": [],
        "held": []
    })
}

/// Composes the `schedule.json` `file` for the instance [`A`], whose game root registers the rule's
/// package; with `subsystems`, a Runtime's manifest checks `requires`.
fn compose(
    file: &Value,
    subsystems: Option<&BTreeSet<String>>,
) -> Result<StandardPackComposition, crate::StandardPackComposeFailure> {
    let bundle = json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": [{
            "package_id": RULE,
            "server": SERVER,
            "entry_task_id": "task",
            "path": format!("packs/{RULE}"),
            "file_count": 1,
            "byte_count": 64
        }]
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
    let file_bytes = serde_json::to_vec(file).expect("schedule bytes");
    let irreversible = super::readable_root(&root);
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &file_bytes,
            root: &root,
            maintenance: &[],
            irreversible: &irreversible,
            catalog: None,
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
        subsystems,
        catalog_version: 1,
    })
}
