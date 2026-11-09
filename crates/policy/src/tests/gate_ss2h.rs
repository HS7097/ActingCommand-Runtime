// SPDX-License-Identifier: AGPL-3.0-only

// SS-2h-a (register §1.288; Alice's ruling 138 of 11-07 20:4x, the capped spending order and the
// planned red line, and her answer of 11-09 16:4x relayed by the coordinator: the capped order
// starts when the reading reaches the cap, and below the cap nothing is spent for free; design
// `rd-1031/design-c424-redline-1109.md` §4.1 tests 1 and 3 to 7, with the coordinator's
// correction of 11-09: test 2 moves to SS-2h-b, test 3 checks the reference form's shape, test 7
// the full `c`; contracts/scheduling/README.md, "Net benefit within a tier", "Within tolerance",
// "Pool redline"):
// - a judged candidate's consumption costs its full weight `W·c`, whatever the pool's fill: SS-2b's
//   linear discount is retired;
// - SS-2d's rounding bound reads the whole consumption `C`;
// - a V2 pool may write `redline`, CF-5's fact reference or a constant, which compiles and is
//   carried as written; nothing reads it at evaluation yet (SS-2h-b's capped spending order will);
// - a V1 document that names it is `unknown_field`, a constant above a constant capacity
//   `limit_exceeded`;
// - a catalog without a valuation and without a red line decides and hashes as before.
//
// The catalog is neutral and V2: tasks on one instance with an hourly clock due at NOW, pools of the
// instance read from their keys. Every candidate's host is idle. Every quantity is in milli of value
// unless it says otherwise; every expected value comes from the design's numbers and the contract.

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CandidateEligibility, CatalogCompileFailure, CatalogDiagnosticCode, CatalogDocumentSource,
    CatalogSources, CompiledCatalog, DecisionReason, DispatchIntent, EvaluationFacts,
    EvaluationResources, EvaluationTime, FactReference, HostResourceSnapshot, ObservedFact,
    PolicyEvaluation, PolicyEvaluationError, PoolRedline, SCHEDULING_SCHEMA_VERSION,
    SCHEDULING_SCHEMA_VERSION_V2, SchedulingDecisionState, TaskDecision, compile_catalog,
    evaluate_with_eligibility, project_fact_pools,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
const TASK: &str = "task_judged";
/// Produced; its weight is each test's, scale 1, no regeneration.
const OUT: &str = "pool.out";
/// Consumed: 1000 milli a unit (scale 1), regenerating one unit every six minutes up to 240.
const STAMINA: &str = "pool.stamina";
/// Consumed: 1000 milli a unit (scale 1), no regeneration, capacity 240.
const STOCK: &str = "pool.stock";
/// The capacity fact a red line names, in the pools' scope.
const CAPACITY_KEY: &str = "resource.stamina.capacity";
const NONPOSITIVE: &str = "policy_benefit_nonpositive";
const WITHIN: &str = "benefit_within_tolerance";
const UNKNOWN: &str = "benefit_unknown";
/// The neutral example catalog's golden identities, computed before SS-2h-a: V1
/// (`compiler::tests::neutral_catalog_compiles_to_stable_hash_and_dry_run`) and declared V2 with
/// every timeline event valid from 0 (Workflow #361 R7, row R7-T6; `gate_386_cf4`).
const V1_EXAMPLE_HASH: &str =
    "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c";
const V2_EXAMPLE_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";

// Test 1, the fix: a regenerating pool of capacity 240 read at 190, no red line; the judged task
// consumes 20 of it at W = 1000 (Q = 1) and produces 5000 milli. SS-2b's linear discount (reserve
// line 0) cost 1000 × 20 × 50/240 = 4166.67 milli, so S = 833.33 above its ε and the candidate was
// dispatched with B = 833 (main eb1e946d, `crates/policy/src/evaluator/benefit.rs`, `judge`
// :425 and `discount` :487): there this test fails on its first assertion. SS-2h-a: below the cap
// nothing is spent for free, the cost is the full 20 000 milli, S = −15 000 and the candidate is
// `policy_benefit_nonpositive`, not dispatched.
#[test]
fn ss2h_t1_below_the_cap_consumption_costs_its_full_weight() {
    let catalog = catalog(
        vec![task(
            TASK,
            vec![produce(OUT, 1)],
            vec![consume(STAMINA, 20)],
        )],
        pools(5000),
    );
    let evaluation = round(&catalog, vec![reading(STAMINA, 190)]);
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, TASK);
    assert_eq!(held.state, SchedulingDecisionState::Deferred);
    assert_eq!(last_code(held), NONPOSITIVE, "{held:#?}");
    assert_eq!(
        last(held).outcome.value("benefit_milli"),
        Some(&json!(-15_000))
    );
    assert!(
        last(held)
            .detail
            .contains(" items=pool.out:+5000,pool.stamina:-20000"),
        "{}",
        last(held).detail
    );
}

// Test 3, the reference form's shape: a V2 pool may write `redline` as CF-5's fact reference
// `{"type": "fact", "fact_key", "offset"}`, here its own capacity fact less 40 (the line 40 below
// the read cap), and another pool a constant. Both compile, the compiled pools hold them and write
// them back as written, and a pool without one carries no member. The reference's members are
// refused as a capacity reference's are: a key that is no identifier (`type_mismatch` at
// `redline/fact_key`), an offset beyond the canonical integers (`limit_exceeded` at
// `redline/offset`), a scope (`unknown_field` at `redline/scope`).
#[test]
fn ss2h_t3_the_reference_form_compiles_and_is_carried_as_written() {
    let reference = json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": -40});
    let compiled = catalog(
        vec![task(
            TASK,
            vec![produce(OUT, 1)],
            vec![consume(STAMINA, 20)],
        )],
        lined_pools(5000, reference.clone(), json!(200)),
    );
    let compiled_pools = &compiled.catalog().pools.pools;
    let redline_of = |id: &str| {
        compiled_pools
            .iter()
            .find(|pool| pool.id == id)
            .map(|pool| pool.redline.clone())
    };
    assert_eq!(
        redline_of(STAMINA),
        Some(Some(PoolRedline::Fact(FactReference {
            fact_key: CAPACITY_KEY.to_owned(),
            offset: -40,
            scope: None,
        })))
    );
    assert_eq!(redline_of(STOCK), Some(Some(PoolRedline::Constant(200))));
    assert_eq!(redline_of(OUT), Some(None));
    let written = serde_json::to_value(compiled_pools).expect("pools JSON");
    assert_eq!(written[1]["redline"], reference);
    assert_eq!(written[2]["redline"], json!(200));
    assert!(written[0].get("redline").is_none(), "{}", written[0]);

    let mut key = reference.clone();
    key["fact_key"] = json!("Not A Key");
    let mut beyond = reference.clone();
    beyond["offset"] = json!(9_007_199_254_740_992_i64);
    let mut cross_scope = reference;
    cross_scope["scope"] = json!({"kind": "instance", "instance_id": "instance-b"});
    for (case, redline, code, pointer) in [
        (
            "reference key",
            key,
            CatalogDiagnosticCode::TypeMismatch,
            "/pools/1/redline/fact_key",
        ),
        (
            "offset beyond the canonical integers",
            beyond,
            CatalogDiagnosticCode::LimitExceeded,
            "/pools/1/redline/offset",
        ),
        (
            "cross-scope reference",
            cross_scope,
            CatalogDiagnosticCode::UnknownField,
            "/pools/1/redline/scope",
        ),
    ] {
        let failure = compile_v2(
            vec![task(TASK, vec![produce(OUT, 1)], Vec::new())],
            lined_pools(5000, redline, json!(200)),
        )
        .expect_err(case);
        assert!(
            refused_at(&failure, code, pointer),
            "{case}: {code:?} at {pointer}: {:?}",
            failure.diagnostics()
        );
    }
}

// Test 4, unchanged: a pool that does not regenerate (capacity 240, no red line) costs its full
// weight, as before SS-2h-a: 20 units at W = 1000 against 5000 milli produced, B = −15 000, read
// full (240 of 240) and unread alike, since such a pool's cost needs no reading.
#[test]
fn ss2h_t4_a_pool_that_does_not_regenerate_costs_its_full_weight_as_before() {
    let catalog = catalog(
        vec![task(TASK, vec![produce(OUT, 1)], vec![consume(STOCK, 20)])],
        pools(5000),
    );
    for (case, readings) in [("full", vec![reading(STOCK, 240)]), ("unread", Vec::new())] {
        let evaluation = round(&catalog, readings);
        assert!(
            dispatched(&evaluation).is_empty(),
            "{case}: {evaluation:#?}"
        );
        let held = decision(&evaluation, TASK);
        assert_eq!(last_code(held), NONPOSITIVE, "{case}: {held:#?}");
        assert_eq!(
            last(held).outcome.value("benefit_milli"),
            Some(&json!(-15_000)),
            "{case}"
        );
        assert!(
            last(held)
                .detail
                .contains(" items=pool.out:+5000,pool.stock:-20000"),
            "{case}: {}",
            last(held).detail
        );
    }
}

// Test 5, unchanged: a catalog with no valuation and no red line.
// - The neutral example catalog (its pool has neither) keeps its golden identities, V1 and V2.
// - A V2 catalog without valuations: its compiled pools carry no `redline` member; the maintenance
//   task (unjudged, priority 300) is dispatched and the judged task is `benefit_unknown`
//   (`valuation_missing`, the produced pool), as before; two evaluations are byte for byte equal
//   and name no red line.
// - The same catalog with a red line on two pools (a reference and a constant) decides, gives its
//   reasons, its reason chains and its next wake byte for byte as without it: nothing that decides
//   reads the member in this slice.
#[test]
fn ss2h_t5_a_catalog_without_valuation_or_red_line_decides_and_hashes_as_before() {
    for (version, golden) in [
        (SCHEDULING_SCHEMA_VERSION, V1_EXAMPLE_HASH),
        (SCHEDULING_SCHEMA_VERSION_V2, V2_EXAMPLE_HASH),
    ] {
        let example = compile_catalog(&example(version, |_| {}))
            .unwrap_or_else(|failure| panic!("{version}: {:?}", failure.diagnostics()));
        assert_eq!(example.catalog_hash(), golden, "{version}");
    }

    let tasks = || {
        vec![
            task(TASK, vec![produce(OUT, 1)], vec![consume(STAMINA, 20)]),
            maintenance("task_maint", vec![consume(STAMINA, 10)]),
        ]
    };
    let plain = catalog(tasks(), bare(pools(5000)));
    let lined = catalog(
        tasks(),
        bare(lined_pools(
            5000,
            json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": -40}),
            json!(200),
        )),
    );
    let compiled = serde_json::to_string(&plain.catalog().pools.pools).expect("pools");
    assert!(!compiled.contains("redline"), "{compiled}");

    let readings = || {
        vec![
            reading(STAMINA, 190),
            fact(A, CAPACITY_KEY, 240, NOW - MINUTE_MS, None),
        ]
    };
    let before = round(&plain, readings());
    let again = round(&plain, readings());
    assert_eq!(
        serde_json::to_vec(&before).expect("bytes"),
        serde_json::to_vec(&again).expect("bytes")
    );
    assert_eq!(dispatched(&before), ["task_maint"], "{before:#?}");
    let judged = decision(&before, TASK);
    assert_eq!(last_code(judged), UNKNOWN, "{judged:#?}");
    assert_eq!(
        last(judged).outcome.value("benefit_gap"),
        Some(&json!("valuation_missing"))
    );
    assert_eq!(last(judged).outcome.value("pool_id"), Some(&json!(OUT)));
    let text = serde_json::to_string(&before).expect("text");
    assert!(!text.contains("redline"), "{text}");

    let after = round(&lined, readings());
    assert_eq!(
        serde_json::to_vec(&before.decisions).expect("decisions"),
        serde_json::to_vec(&after.decisions).expect("decisions"),
        "the red line changes no decision"
    );
    let chains = |evaluation: &PolicyEvaluation| {
        evaluation
            .reason_chains
            .iter()
            .map(|chain| chain.reasons.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(chains(&before), chains(&after));
    assert_eq!(before.next_wake_unix_ms, after.next_wake_unix_ms);
    assert_eq!(dispatched(&after), ["task_maint"], "{after:#?}");
}

// Test 6, the refusals: a V1 document that names `redline`, as a constant or as a reference, is
// refused as `unknown_field` at the member; in V2 a constant red line above the pool's constant
// capacity (241 of 240) is `limit_exceeded` at the member, while one at the capacity compiles.
#[test]
fn ss2h_t6_a_v1_red_line_and_a_constant_above_the_cap_are_refused() {
    for (case, redline) in [
        ("a constant", json!(100)),
        (
            "a reference",
            json!({"type": "fact", "fact_key": "resource.primary", "offset": -20}),
        ),
    ] {
        let failure = compile_catalog(&example(SCHEDULING_SCHEMA_VERSION, |pools| {
            pools["pools"][0]["redline"] = redline;
        }))
        .expect_err(case);
        assert!(
            refused_at(
                &failure,
                CatalogDiagnosticCode::UnknownField,
                "/pools/0/redline"
            ),
            "{case}: {:?}",
            failure.diagnostics()
        );
    }

    let tasks = || vec![task(TASK, vec![produce(OUT, 1)], vec![consume(STOCK, 20)])];
    let reference = json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": -40});
    let failure = compile_v2(tasks(), lined_pools(5000, reference.clone(), json!(241)))
        .expect_err("above the capacity");
    assert!(
        refused_at(
            &failure,
            CatalogDiagnosticCode::LimitExceeded,
            "/pools/2/redline"
        ),
        "{:?}",
        failure.diagnostics()
    );
    assert!(
        compile_v2(tasks(), lined_pools(5000, reference, json!(240))).is_ok(),
        "a red line at the capacity compiles"
    );
}

// Test 7, SS-2d's bound reads the whole consumption. A regenerating pool full (240 of 240), 20
// units at W = 1000 consumed, one unit produced: the cost is the full 20 000 milli and ε = 1000/2000
// + 20 000/2000 = 21/2, with or without a red line on the pool (here a reference to a capacity
// fact that is missing: the cost does not read it, so neither the line nor its missing fact changes
// anything). Produced at 20 005, S = 5 is within ε: `benefit_within_tolerance` (⌊S⌋ 5,
// ⌈ε⌉ 11, unmarked). Produced at 20 011, S = 11 is above it: dispatched with B = 11. Under SS-2b's
// discount the full pool cost nothing (S = 20 005, ε = 1/2). The design's appendix A5 case 1 still
// holds: two thousandths of a unit expected at W = 2, Q = 1 carry 1/1000 + 5/4000 = 9/4000, and with
// one unit of a pool at W = 6100 per 10^6 consumed (ε 1/2 000 000) S = −21/10000 is within
// ε = 4501/2000000.
#[test]
fn ss2h_t7_the_rounding_bound_reads_the_whole_consumption() {
    let tasks = || {
        vec![task(
            TASK,
            vec![produce(OUT, 1)],
            vec![consume(STAMINA, 20)],
        )]
    };
    for lined in [false, true] {
        let with_line = |weight: u32| {
            let mut pools = pools(weight);
            if lined {
                pools[1]["redline"] =
                    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": -40});
            }
            pools
        };
        let within = round(
            &catalog(tasks(), with_line(20_005)),
            vec![reading(STAMINA, 240)],
        );
        assert!(dispatched(&within).is_empty(), "{lined}: {within:#?}");
        let held = decision(&within, TASK);
        assert_eq!(last_code(held), WITHIN, "{lined}: {held:#?}");
        let outcome = &last(held).outcome;
        assert_eq!(outcome.value("benefit_milli"), Some(&json!(5)), "{lined}");
        assert_eq!(
            outcome.value("tolerance_milli"),
            Some(&json!(11)),
            "{lined}"
        );
        assert_eq!(
            outcome.value("zero_tier"),
            Some(&json!("unmarked")),
            "{lined}"
        );
        assert!(
            last(held)
                .detail
                .contains(" sum=5/1 tolerance=21/2 items=pool.out:+20005,pool.stamina:-20000"),
            "{lined}: {}",
            last(held).detail
        );

        let above = round(
            &catalog(tasks(), with_line(20_011)),
            vec![reading(STAMINA, 240)],
        );
        assert_eq!(dispatched(&above), [TASK], "{lined}: {above:#?}");
        let breakdown = reason(decision(&above, TASK), "rank_breakdown");
        assert_eq!(
            breakdown.outcome.value("benefit_milli"),
            Some(&json!(11)),
            "{lined}"
        );
    }

    let catalog = catalog(
        vec![task(
            TASK,
            vec![expected("pool.trace", 2)],
            vec![consume("pool.dust", 1)],
        )],
        json!([
            valued("pool.trace", 2, 1),
            valued("pool.dust", 6100, 1_000_000)
        ]),
    );
    let evaluation = round(&catalog, Vec::new());
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, TASK);
    assert_eq!(last_code(held), WITHIN, "{held:#?}");
    assert!(
        last(held)
            .detail
            .contains(" sum=-21/10000 tolerance=4501/2000000 "),
        "{}",
        last(held).detail
    );
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

fn effect(pool: &str, direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

fn produce(pool: &str, amount: u64) -> Value {
    effect(pool, "produce", amount)
}

fn consume(pool: &str, amount: u64) -> Value {
    effect(pool, "consume", amount)
}

/// A production of `milli` thousandths of a unit expected per run.
fn expected(pool: &str, milli: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": "produce",
        "expected_amount_milli": milli,
        "observation_source": "self_reported",
        "confidence_milli": 1000
    })
}

/// A task of the game on its hourly clock, of priority `priority`, unmarked.
fn task_body(id: &str, priority: i16, produces: Vec<Value>, consumes: Vec<Value>) -> Value {
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
    })
}

/// A judged task (marked `benefit_judged`, SS-2b) of priority 100.
fn task(id: &str, produces: Vec<Value>, consumes: Vec<Value>) -> Value {
    let mut task = task_body(id, 100, produces, consumes);
    task["benefit_judged"] = json!(true);
    task
}

/// A maintenance task (unmarked, so never judged) of priority 300 that produces nothing.
fn maintenance(id: &str, consumes: Vec<Value>) -> Value {
    task_body(id, 300, Vec::new(), consumes)
}

/// A pool of the instance read from `resource.<the id without "pool.">`.
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

fn valuation(weight: u32, scale: u64) -> Value {
    json!({"name": "Fixture", "unit": "unit", "scale": scale, "base_weight_milli": weight})
}

/// A pool that does not regenerate, weighed `weight` milli per `scale` units.
fn valued(id: &str, weight: u32, scale: u64) -> Value {
    pool(
        id,
        1_000_000,
        json!({"kind": "none"}),
        Some(valuation(weight, scale)),
    )
}

/// The three pools, in this order: the produced pool weighed `out_weight`, the regenerating pool
/// and the stock, neither writing a red line.
fn pools(out_weight: u32) -> Value {
    json!([
        valued(OUT, out_weight, 1),
        pool(
            STAMINA,
            240,
            json!({"amount": 1, "per_ms": 6 * MINUTE_MS}),
            Some(valuation(1000, 1))
        ),
        pool(
            STOCK,
            240,
            json!({"kind": "none"}),
            Some(valuation(1000, 1))
        ),
    ])
}

/// [`pools`] with `stamina` as the regenerating pool's red line and `stock` as the stock's.
fn lined_pools(out_weight: u32, stamina: Value, stock: Value) -> Value {
    let mut pools = pools(out_weight);
    pools[1]["redline"] = stamina;
    pools[2]["redline"] = stock;
    pools
}

/// `pools` without any valuation.
fn bare(mut pools: Value) -> Value {
    for pool in pools.as_array_mut().expect("the pools") {
        pool.as_object_mut()
            .expect("a pool object")
            .shift_remove("valuation");
    }
    pools
}

/// The fixture catalog, `actingcommand.scheduling.v2`, with these tasks and pools.
fn catalog(tasks: Vec<Value>, pools: Value) -> CompiledCatalog {
    compile_v2(tasks, pools).unwrap_or_else(|failure| {
        panic!("the fixture catalog compiles: {:?}", failure.diagnostics())
    })
}

fn compile_v2(tasks: Vec<Value>, pools: Value) -> Result<CompiledCatalog, CatalogCompileFailure> {
    let header = json!({
        "catalog_id": "fixture.ss2h",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-ss2h"]
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
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
}

/// The neutral example catalog `catalog-a` declared as `version` (V2: every timeline event valid
/// from 0, as its V2 golden was computed), its pools document changed by `edit`.
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
    let [tasks, mut pools, activity, timeline] = documents;
    edit(&mut pools);
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
        format!("memory://fixture-ss2h/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

fn refused_at(failure: &CatalogCompileFailure, code: CatalogDiagnosticCode, path: &str) -> bool {
    failure
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == code && diagnostic.json_path == path)
}

/// A reading of `pool`'s key at `value`, one minute before the round (no regeneration step since).
fn reading(pool: &str, value: i64) -> ObservedFact {
    fact(
        A,
        &format!("resource.{}", &pool["pool.".len()..]),
        value,
        NOW - MINUTE_MS,
        None,
    )
}

/// One round at NOW on the instance, every candidate admitted.
fn round(catalog: &CompiledCatalog, readings: Vec<ObservedFact>) -> PolicyEvaluation {
    let instance = instance(A, &[]);
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-ss2h".to_owned(),
        facts: readings,
        outcomes: Vec::new(),
        tasks: Vec::new(),
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
    evaluate_with_eligibility(
        catalog,
        &facts,
        &resources,
        EvaluationTime {
            unix_ms: NOW,
            monotonic_ms: NOW,
        },
        11,
        |_: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
            Ok(CandidateEligibility::Eligible)
        },
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
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

fn last(decision: &TaskDecision) -> &DecisionReason {
    decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {decision:#?}"))
}

fn last_code(decision: &TaskDecision) -> &str {
    last(decision).outcome.code().as_str()
}
