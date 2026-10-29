// SPDX-License-Identifier: AGPL-3.0-only

// SS-6 gate tests, policy half (0.12.31; register §1.218; coordinator work order
// `rd-1029/brief-ss6.md` tests 1 to 5, 7 and 8; design `rd-1029/design-sch-state.md` §4.3 and the
// SS-6 entry of §5; `contracts/remedies.md` section 4): a selected dispatch of a task that reserves
// a pool it consumes, or that produces a pool an active goal covers, carries on
// `policy_dispatch_parameters`, with no opt-in, its spend values: per such `consumes` entry `i`
// the allowance (the pool's projection at the evaluation less the task's reserve, at least 0), per
// such `produces` entry `j` the goal's gap, and `run_cap`, the least over the reserved consumed
// pools of ⌊allowance / declared consumption per run⌋, at most ⌈gap / declared production per
// run⌉ for a produced pool without base weight whose gap is open. Alice's ruling 100 (#386
// comment 6490879952): dispatch follows live account state, there is no "run task X N times";
// ruling 106 (10-29 20:3x): until time joins the model the reserve line is the backstop. Every
// expected value comes from the work order's formulas.
//
// The catalog is neutral: tasks on one instance, each triggered by an hourly clock that is due at
// NOW, and four pools of the instance that take their amount from readings of their keys and do
// not regenerate: `pool_a` and `pool_d` are spent, `pool_b` has no valuation (a goal drives it
// alone) and `pool_c` has a base weight.

use actingcommand_contract::outcome::Outcome;
use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::resource_targets::{ReserveView, reserve_views, spend_values};
use crate::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, DecisionReason, DecisionReasonChain,
    EvaluationFacts, EvaluationResources, EvaluationTime, FactValue, HostResourceSnapshot,
    ObservedFact, PolicyCode, PolicyEvaluation, ScopeSelector, TaskDecision, TaskSpec,
    check_resource_targets, compile_catalog, evaluate, parse_resource_targets, project_fact_pools,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const TASK_A: &str = "task_a";
const TASK_B: &str = "task_b";
const TASK_N: &str = "task_n";
/// Spent; no valuation.
const POOL_A: &str = "pool_a";
const POOL_A_KEY: &str = "resource.pool_a";
/// Produced toward a goal; no valuation, so no base weight.
const POOL_B: &str = "pool_b";
const POOL_B_KEY: &str = "resource.pool_b";
/// Produced; valued at `base_weight_milli` 500.
const POOL_C: &str = "pool_c";
const POOL_C_KEY: &str = "resource.pool_c";
/// Spent; no valuation.
const POOL_D: &str = "pool_d";
const POOL_D_KEY: &str = "resource.pool_d";
const V2: &str = "actingcommand.resource-targets.v2";
const PARAMETERS: &str = "policy_dispatch_parameters";
const RESERVE_INSUFFICIENT: &str = "resource_reserve_insufficient";
/// The values SS-6 adds to `policy_dispatch_parameters`.
const SS6_KEYS: [&str; 3] = ["spend_allowances", "gaps_remaining", "run_cap"];
/// The values Workflow #411 B8 puts on it.
const B8_KEYS: [&str; 5] = [
    "next_occurrence_unix_ms",
    "window_end_unix_ms",
    "last_in_window",
    "last_daily_slot",
    "daily_remaining",
];

// Work order test 1: the allowance is the pool's projection less the task's reserve line, floored
// at 0, keyed by the entry's position in the task's own `consumes`. Task A consumes 5 of pool D
// (no reserve) and 30 of pool A, which must keep 100. Read at 1000, pool A allows 900 (30 runs);
// read at 130, it allows 30 (one run, which leaves exactly the reserve). Pool D, consumed but not
// reserved, gets no allowance. Below the line the allowance is 0, never negative: pool A projected
// at 60 against its reserve of 100.
#[test]
fn ss6_t1_the_allowance_is_the_projection_less_the_reserve_floored_at_zero() {
    let declared = TaskDecl {
        consumes: &[(POOL_D, 5), (POOL_A, 30)],
        reserves: &[(POOL_A, 100)],
        ..TaskDecl::new(TASK_A, 100)
    };
    let catalog = catalog(&[task(&declared)]);
    for (reading, allowance, run_cap) in [(1000, 900, 30), (130, 30, 1)] {
        let evaluation = round(&catalog, vec![reading_of(POOL_A_KEY, reading)]);
        let reason = parameters(&evaluation, TASK_A)
            .unwrap_or_else(|| panic!("reading {reading}: {PARAMETERS}: {evaluation:#?}"));
        assert_eq!(
            reason.outcome.value("spend_allowances"),
            Some(&json!([{"effect_index": 1, "spend_allowance": allowance}])),
            "reading {reading}: {reason:?}"
        );
        assert_eq!(
            reason.outcome.value("run_cap"),
            Some(&json!(run_cap)),
            "reading {reading}: {reason:?}"
        );
        assert_eq!(
            reason.outcome.value("gaps_remaining"),
            None,
            "no goal: {reason:?}"
        );
    }

    let spec = task_spec(&catalog, TASK_A);
    let views = projected_views(spec, 60);
    let below = spend_values(spec, Some(views.as_slice()), &[], |_| None);
    assert_eq!(below.spend_allowances, [(1, 0)], "{below:?}");
    assert_eq!(below.run_cap, Some(0), "{below:?}");
}

// Work order test 2: a task without `reserves` gets no allowance. Task B consumes 30 of pool A
// without reserving it and opts in to Workflow #411 B8's values; task A, ranked first, must keep
// 2000 of the pool read at 1000 and is held. Task B is dispatched: its `policy_dispatch_parameters`
// carries B8's values only, no allowance and no run cap.
#[test]
fn ss6_t2_without_reserves_there_is_no_allowance() {
    let held = TaskDecl {
        consumes: &[(POOL_A, 30)],
        reserves: &[(POOL_A, 2000)],
        ..TaskDecl::new(TASK_A, 200)
    };
    let free = TaskDecl {
        consumes: &[(POOL_A, 30)],
        dispatch_parameters: true,
        ..TaskDecl::new(TASK_B, 100)
    };
    let catalog = catalog(&[task(&held), task(&free)]);
    let evaluation = round(&catalog, vec![reading_of(POOL_A_KEY, 1000)]);
    assert_eq!(dispatched(&evaluation), [TASK_B], "{evaluation:#?}");
    assert_eq!(
        last_reason(decision(&evaluation, TASK_A))
            .outcome
            .code()
            .as_str(),
        RESERVE_INSUFFICIENT
    );
    let reason = parameters(&evaluation, TASK_B)
        .unwrap_or_else(|| panic!("B8's {PARAMETERS}: {evaluation:#?}"));
    for key in SS6_KEYS {
        assert_eq!(reason.outcome.value(key), None, "{key}: {reason:?}");
    }
}

// Work order test 3: without an active goal there is no `gap_remaining`. Task A consumes 30 of
// pool A (reserve 100, read at 1000) and produces 10 of pool B per run, read at 45. With no target
// record, with a withdrawn one and with an expired one its dispatch carries the allowance but no
// gap; under an active v2 goal of 100 on pool B it carries the gap 55 at the entry's position in
// its `produces`.
#[test]
fn ss6_t3_without_an_active_goal_there_is_no_gap_remaining() {
    let declared = TaskDecl {
        consumes: &[(POOL_A, 30)],
        reserves: &[(POOL_A, 100)],
        produces: &[(POOL_B, 10)],
        ..TaskDecl::new(TASK_A, 100)
    };
    let catalog = catalog(&[task(&declared)]);
    let expired = stored(
        &catalog,
        &json!({
            "schema_version": V2,
            "instance": A,
            "valid_until_unix_ms": NOW - HOUR_MS,
            "targets": [goal(POOL_B, 100)],
        }),
        NOW - 2 * HOUR_MS,
    );
    let withdrawn = stored(
        &catalog,
        &json!({"schema_version": V2, "instance": A, "targets": []}),
        NOW - HOUR_MS,
    );
    let active = stored(&catalog, &goals(&[goal(POOL_B, 100)]), NOW - HOUR_MS);
    for (label, record, gaps) in [
        ("no record", None, None),
        ("withdrawn", Some(withdrawn), None),
        ("expired", Some(expired), None),
        (
            "active",
            Some(active),
            Some(json!([{"effect_index": 0, "gap_remaining": 55}])),
        ),
    ] {
        let mut facts = vec![reading_of(POOL_A_KEY, 1000), reading_of(POOL_B_KEY, 45)];
        facts.extend(record);
        let evaluation = round(&catalog, facts);
        let reason = parameters(&evaluation, TASK_A)
            .unwrap_or_else(|| panic!("{label}: {PARAMETERS}: {evaluation:#?}"));
        assert_eq!(
            reason.outcome.value("spend_allowances"),
            Some(&json!([{"effect_index": 0, "spend_allowance": 900}])),
            "{label}: {reason:?}"
        );
        assert_eq!(
            reason.outcome.value("gaps_remaining"),
            gaps.as_ref(),
            "{label}: {reason:?}"
        );
    }
}

// Work order test 4: `run_cap` is the least over the reserved consumed pools, and a goal-driven
// produced pool's gap caps it. Task A consumes 30 of pool A (reserve 100, read at 1000: 900, 30
// runs) and 7 of pool D (reserve 10, read at 80: 70, 10 runs): the cap is 10. It also produces 10
// of pool B (no base weight) and 10 of pool C (base weight 500) per run:
// - a goal of 100 on pool B read at 45 leaves a gap of 55: ⌈55 / 10⌉ = 6 runs cap it at 6;
// - the same goal on pool C, which has a base weight, caps nothing: 10; its gap is still handed;
// - pool B's goal met (read at 150, gap 0) caps nothing either (ruling 102: a met goal only stops
//   adding score): 10.
#[test]
fn ss6_t4_run_cap_is_the_least_over_pools_and_a_goal_driven_gap_caps_it() {
    let declared = TaskDecl {
        consumes: &[(POOL_A, 30), (POOL_D, 7)],
        reserves: &[(POOL_A, 100), (POOL_D, 10)],
        produces: &[(POOL_B, 10), (POOL_C, 10)],
        ..TaskDecl::new(TASK_A, 100)
    };
    let catalog = catalog(&[task(&declared)]);
    let spent = || vec![reading_of(POOL_A_KEY, 1000), reading_of(POOL_D_KEY, 80)];
    for (label, goal_pool, reading_b, reading_c, gaps, run_cap) in [
        ("no goal", None, 45, 45, None, 10),
        (
            "an open goal on the pool without base weight",
            Some(POOL_B),
            45,
            45,
            Some(json!([{"effect_index": 0, "gap_remaining": 55}])),
            6,
        ),
        (
            "an open goal on the valued pool",
            Some(POOL_C),
            45,
            45,
            Some(json!([{"effect_index": 1, "gap_remaining": 55}])),
            10,
        ),
        (
            "a met goal on the pool without base weight",
            Some(POOL_B),
            150,
            45,
            Some(json!([{"effect_index": 0, "gap_remaining": 0}])),
            10,
        ),
    ] {
        let mut facts = spent();
        facts.push(reading_of(POOL_B_KEY, reading_b));
        facts.push(reading_of(POOL_C_KEY, reading_c));
        if let Some(pool) = goal_pool {
            facts.push(stored(&catalog, &goals(&[goal(pool, 100)]), NOW - HOUR_MS));
        }
        let evaluation = round(&catalog, facts);
        let reason = parameters(&evaluation, TASK_A)
            .unwrap_or_else(|| panic!("{label}: {PARAMETERS}: {evaluation:#?}"));
        assert_eq!(
            reason.outcome.value("spend_allowances"),
            Some(&json!([
                {"effect_index": 0, "spend_allowance": 900},
                {"effect_index": 1, "spend_allowance": 70}
            ])),
            "{label}: {reason:?}"
        );
        assert_eq!(
            reason.outcome.value("gaps_remaining"),
            gaps.as_ref(),
            "{label}: {reason:?}"
        );
        assert_eq!(
            reason.outcome.value("run_cap"),
            Some(&json!(run_cap)),
            "{label}: {reason:?}"
        );
    }
}

// Work order test 5: an allowance of 0 coincides with the candidate being blocked at the reserve
// gate. Task A consumes 30 of pool A per run and must keep 100. For each reading the allowance
// is computed from the reserve view the gate judges by (the same projection at the same instant):
// every reading whose allowance is 0 holds the candidate with `resource_reserve_insufficient`, and
// the candidate runs exactly when the allowance covers one run (reading 130 and above), carrying
// that allowance.
#[test]
fn ss6_t5_an_allowance_of_zero_coincides_with_the_reserve_gate_holding_the_candidate() {
    let declared = TaskDecl {
        consumes: &[(POOL_A, 30)],
        reserves: &[(POOL_A, 100)],
        ..TaskDecl::new(TASK_A, 100)
    };
    let catalog = catalog(&[task(&declared)]);
    let spec = task_spec(&catalog, TASK_A);
    for reading in [0_u64, 60, 99, 100, 101, 129, 130, 131, 400] {
        let views = projected_views(spec, reading);
        let allowance =
            spend_values(spec, Some(views.as_slice()), &[], |_| None).spend_allowances[0].1;
        assert_eq!(allowance, reading.saturating_sub(100), "reading {reading}");
        let evaluation = round(
            &catalog,
            vec![reading_of(
                POOL_A_KEY,
                i64::try_from(reading).expect("a reading"),
            )],
        );
        let runs = !dispatched(&evaluation).is_empty();
        assert_eq!(runs, allowance >= 30, "reading {reading}: {evaluation:#?}");
        if allowance == 0 {
            assert!(!runs, "reading {reading}: an allowance of 0 never runs");
        }
        if runs {
            let reason = parameters(&evaluation, TASK_A)
                .unwrap_or_else(|| panic!("reading {reading}: {evaluation:#?}"));
            assert_eq!(
                reason.outcome.value("spend_allowances"),
                Some(&json!([{"effect_index": 0, "spend_allowance": allowance}])),
                "reading {reading}: {reason:?}"
            );
        } else {
            assert_eq!(
                last_reason(decision(&evaluation, TASK_A))
                    .outcome
                    .code()
                    .as_str(),
                RESERVE_INSUFFICIENT,
                "reading {reading}: {evaluation:#?}"
            );
        }
    }
}

// Work order test 7: a task that declares none of these (no reserve, no production toward a goal)
// gets the dispatch reason chain of before. Task N neither reserves nor produces; task A, ranked
// first, reserves pool A (2000, read at 1000) and produces pool B under an active goal, and is
// held, so the evaluation computes spend values. Task N is dispatched: without the opt-in its chain
// has no `policy_dispatch_parameters`, as before; with Workflow #411 B8's opt-in the reason is,
// byte for byte, the one B8 builds from its own values, and no reason of the chain names an SS-6
// value.
#[test]
fn ss6_t7_a_task_that_declares_none_of_these_keeps_its_dispatch_chain() {
    let held = TaskDecl {
        consumes: &[(POOL_A, 30)],
        reserves: &[(POOL_A, 2000)],
        produces: &[(POOL_B, 10)],
        ..TaskDecl::new(TASK_A, 200)
    };
    for opt_in in [false, true] {
        let none = TaskDecl {
            dispatch_parameters: opt_in,
            ..TaskDecl::new(TASK_N, 100)
        };
        let catalog = catalog(&[task(&held), task(&none)]);
        let record = stored(&catalog, &goals(&[goal(POOL_B, 100)]), NOW - HOUR_MS);
        let evaluation = round(
            &catalog,
            vec![
                reading_of(POOL_A_KEY, 1000),
                reading_of(POOL_B_KEY, 45),
                record,
            ],
        );
        assert_eq!(dispatched(&evaluation), [TASK_N], "{evaluation:#?}");
        let chain = dispatched_chain(&evaluation, TASK_N).expect("task N's dispatch chain");
        for reason in &chain.reasons {
            for key in SS6_KEYS {
                assert_eq!(reason.outcome.value(key), None, "{key}: {reason:?}");
            }
        }
        let carried = parameters(&evaluation, TASK_N);
        if !opt_in {
            assert!(carried.is_none(), "{chain:#?}");
            continue;
        }
        let reason = carried.unwrap_or_else(|| panic!("B8's {PARAMETERS}: {chain:#?}"));
        let expected = b8_reason(reason);
        assert_eq!(reason, &expected);
        assert_eq!(
            serde_json::to_vec(&reason.outcome).expect("reason bytes"),
            serde_json::to_vec(&expected.outcome).expect("expected bytes")
        );
        assert!(
            reason
                .outcome
                .values()
                .keys()
                .all(|key| B8_KEYS.contains(&key.as_str())),
            "{reason:?}"
        );
    }
}

// Work order test 8: incomplete declarations give no `run_cap`, and overflow-sized inputs neither
// panic nor wrap.
// - A goal-driven produced pool declared by `expected_amount_milli` (no integer per-run amount)
//   with an open gap: allowance and gap are handed, `run_cap` is not.
// - A reserved pool whose projection is unknown: no allowance and no `run_cap`.
// - A projection of u64::MAX with a reserve of 0 and a consumption of 1: the allowance and the cap
//   are u64::MAX and are recorded as the key table's largest integer, i64::MAX; a gap of u64::MAX
//   is recorded the same way, and against a production of u64::MAX twice (a saturating sum) it
//   caps the runs at ⌈u64::MAX / u64::MAX⌉ = 1.
#[test]
fn ss6_t8_incomplete_declarations_give_no_run_cap_and_overflow_neither_panics_nor_wraps() {
    let milli = task_spec_of(task_with_production(
        &[(POOL_A, 30)],
        &[(POOL_A, 100)],
        json!([production(POOL_B, json!({"expected_amount_milli": 5000}))]),
    ));
    let views = [view(POOL_A, 100, 30, Some(1000))];
    let spend = spend_values(&milli, Some(views.as_slice()), &[], |pool| {
        (pool == POOL_B).then_some(60)
    });
    assert_eq!(spend.spend_allowances, [(0, 900)], "{spend:?}");
    assert_eq!(spend.gaps_remaining, [(0, 60)], "{spend:?}");
    assert_eq!(spend.run_cap, None, "{spend:?}");

    let unknown = [view(POOL_A, 100, 30, None)];
    let spend = spend_values(&milli, Some(unknown.as_slice()), &[], |_| None);
    assert!(spend.spend_allowances.is_empty(), "{spend:?}");
    assert_eq!(spend.run_cap, None, "{spend:?}");

    let huge = task_spec_of(task_with_production(
        &[(POOL_A, 1)],
        &[(POOL_A, 0)],
        json!([
            production(POOL_B, json!({"amount": u64::MAX})),
            production(POOL_B, json!({"amount": u64::MAX}))
        ]),
    ));
    let views = [view(POOL_A, 0, 1, Some(u64::MAX))];
    let uncapped = spend_values(&huge, Some(views.as_slice()), &[], |_| None);
    assert_eq!(uncapped.spend_allowances, [(0, u64::MAX)], "{uncapped:?}");
    assert_eq!(uncapped.run_cap, Some(u64::MAX), "{uncapped:?}");
    let recorded = uncapped.add_to(Outcome::new(PolicyCode::PolicyDispatchParameters));
    assert_eq!(
        recorded.value("spend_allowances"),
        Some(&json!([{"effect_index": 0, "spend_allowance": i64::MAX}]))
    );
    assert_eq!(recorded.value("run_cap"), Some(&json!(i64::MAX)));

    let capped = spend_values(&huge, Some(views.as_slice()), &[], |pool| {
        (pool == POOL_B).then_some(u64::MAX)
    });
    assert_eq!(capped.run_cap, Some(1), "{capped:?}");
    let recorded = capped.add_to(Outcome::new(PolicyCode::PolicyDispatchParameters));
    assert_eq!(
        recorded.value("gaps_remaining"),
        Some(&json!([
            {"effect_index": 0, "gap_remaining": i64::MAX},
            {"effect_index": 1, "gap_remaining": i64::MAX}
        ]))
    );
    assert_eq!(recorded.value("run_cap"), Some(&json!(1)));
}

// ---- Fixture.

/// One task of the fixture: its id, priority, consumption, reserves and production per pool, and
/// whether it opts in to Workflow #411 B8's values.
struct TaskDecl {
    id: &'static str,
    priority: i16,
    consumes: &'static [(&'static str, u64)],
    reserves: &'static [(&'static str, u64)],
    produces: &'static [(&'static str, u64)],
    dispatch_parameters: bool,
}

impl TaskDecl {
    const fn new(id: &'static str, priority: i16) -> Self {
        Self {
            id,
            priority,
            consumes: &[],
            reserves: &[],
            produces: &[],
            dispatch_parameters: false,
        }
    }
}

/// One `produces` entry of `pool_id` with its quantity member (`amount` or
/// `expected_amount_milli`).
fn production(pool_id: &str, quantity: Value) -> Value {
    let mut entry = json!({
        "pool_id": pool_id,
        "direction": "produce",
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    });
    for (key, value) in quantity.as_object().expect("a quantity member") {
        entry[key] = value.clone();
    }
    entry
}

fn task(declared: &TaskDecl) -> Value {
    let produces = declared
        .produces
        .iter()
        .map(|(pool_id, amount)| production(pool_id, json!({"amount": amount})))
        .collect::<Vec<_>>();
    let mut task = task_with_production(declared.consumes, declared.reserves, json!(produces));
    task["id"] = json!(declared.id);
    task["procedure_ref"] = json!(format!("procedure.{}", declared.id));
    task["feedback_stop"]["task_id"] = json!(declared.id);
    task["priority"] = json!(declared.priority);
    if declared.dispatch_parameters {
        task["dispatch_parameters"] = json!(true);
    }
    task
}

/// A task `task_a` with these consumptions and reserves (no member without reserves) and this
/// `produces` list.
fn task_with_production(
    consumes: &[(&str, u64)],
    reserves: &[(&str, u64)],
    produces: Value,
) -> Value {
    let consumes = consumes
        .iter()
        .map(|(pool_id, amount)| {
            json!({
                "pool_id": pool_id,
                "direction": "consume",
                "amount": amount,
                "observation_source": "scan_verified",
                "confidence_milli": 1000
            })
        })
        .collect::<Vec<_>>();
    let mut task = json!({
        "id": TASK_A,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{TASK_A}"),
        "priority": 100,
        "trigger": {
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": HOUR_MS,
                "anchor_ms": 0
            }
        },
        "feedback_stop": {
            "kind": "outcome",
            "task_id": TASK_A,
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
        "loop_budget": {"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    });
    if !reserves.is_empty() {
        task["reserves"] = json!(
            reserves
                .iter()
                .map(|(pool_id, at_least)| json!({"pool_id": pool_id, "at_least": at_least}))
                .collect::<Vec<_>>()
        );
    }
    task
}

fn pool(id: &str, fact_key: &str, valuation: Option<Value>) -> Value {
    let mut pool = json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": 100000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": fact_key},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    });
    if let Some(valuation) = valuation {
        pool["valuation"] = valuation;
    }
    pool
}

/// The fixture catalog, `actingcommand.scheduling.v2`, with these tasks and the four pools.
fn catalog(tasks: &[Value]) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.spend-allowance",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-spend-allowance"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let pools = json!([
        pool(POOL_A, POOL_A_KEY, None),
        pool(POOL_B, POOL_B_KEY, None),
        pool(
            POOL_C,
            POOL_C_KEY,
            Some(json!({"name": "Material C", "unit": "piece", "scale": 10,
                "base_weight_milli": 500,
                "gap": {"rule": "shortfall_linear", "weight_milli": 100}}))
        ),
        pool(POOL_D, POOL_D_KEY, None),
    ]);
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
        tasks: source("tasks", &document("tasks", Value::Array(tasks.to_vec()))),
        pools: source("pools", &document("pools", pools)),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

/// A v2 goal on `pool` to hold at least `at_least`, step 1 and importance 1000 of its own (pool B
/// has no valuation to take them from).
fn goal(pool: &str, at_least: u64) -> Value {
    json!({
        "id": format!("goal-{pool}"),
        "resource": pool,
        "condition": {"kind": "at_least", "amount": at_least},
        "scale": 1,
        "importance_milli": 1000,
        "apply": {"mode": "adjust", "weight": "score_stage"}
    })
}

/// An active v2 document of `targets`, valid for a day.
fn goals(targets: &[Value]) -> Value {
    json!({
        "schema_version": V2,
        "instance": A,
        "valid_until_unix_ms": NOW + DAY_MS,
        "targets": targets,
    })
}

/// The record the formal entry stores for `document`, checked against `catalog` at `checked_at`.
fn stored(catalog: &CompiledCatalog, document: &Value, checked_at: u64) -> ObservedFact {
    let parsed = parse_resource_targets(&serde_json::to_vec(document).expect("document bytes"))
        .unwrap_or_else(|error| panic!("the fixture document parses: {error:?}"));
    let checked = check_resource_targets(&parsed, catalog, &inputs(Vec::new()), time(checked_at))
        .unwrap_or_else(|error| panic!("the fixture document is accepted: {error:?}"));
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: actingcommand_contract::RESOURCE_TARGETS_FACT_KEY.to_owned(),
        value: FactValue::RecordList(
            serde_json::from_value(serde_json::to_value(&checked.rows).expect("rows"))
                .expect("record list"),
        ),
        observed_at_unix_ms: checked_at,
        expires_at_unix_ms: document["valid_until_unix_ms"].as_u64(),
        confidence_milli: 1000,
    }
}

/// A reading of a pool's key on the instance, a minute before NOW, fresh for a day.
fn reading_of(fact_key: &str, value: i64) -> ObservedFact {
    fact(A, fact_key, value, NOW - 60_000, Some(NOW + DAY_MS))
}

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

fn inputs(facts: Vec<ObservedFact>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-spend-allowance".to_owned(),
        facts,
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

/// One round at NOW. The pools' values are the ones the Runtime derives from the readings; the
/// instance has its own host and every candidate is admitted.
fn round(catalog: &CompiledCatalog, facts: Vec<ObservedFact>) -> PolicyEvaluation {
    let facts = inputs(facts);
    let resources = project_fact_pools(
        catalog,
        &facts,
        &EvaluationResources {
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
        },
    );
    evaluate(catalog, &facts, &resources, time(NOW), 11)
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

/// The tasks the round dispatches on the instance, by task id.
fn dispatched(evaluation: &PolicyEvaluation) -> Vec<&'static str> {
    [TASK_A, TASK_B, TASK_N]
        .into_iter()
        .filter(|task_id| {
            evaluation
                .dispatch_intents
                .iter()
                .any(|intent| intent.task_id == *task_id && intent.instance_id == A)
        })
        .collect()
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == task_id && decision.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}: {evaluation:#?}"))
}

/// The reason that settled a decision: its last one.
fn last_reason(decision: &TaskDecision) -> &DecisionReason {
    decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {decision:#?}"))
}

/// The reason chain of the evaluation's dispatch of `task_id`, if it has one.
fn dispatched_chain<'a>(
    evaluation: &'a PolicyEvaluation,
    task_id: &str,
) -> Option<&'a DecisionReasonChain> {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id && intent.instance_id == A)?;
    evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
}

/// The one `policy_dispatch_parameters` of `task_id`'s dispatch chain, if it has one.
fn parameters<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> Option<&'a DecisionReason> {
    let chain = dispatched_chain(evaluation, task_id)?;
    let reasons = chain
        .reasons
        .iter()
        .filter(|reason| reason.outcome.code().as_str() == PARAMETERS)
        .collect::<Vec<_>>();
    match reasons.as_slice() {
        [] => None,
        [reason] => Some(reason),
        _ => panic!("one {PARAMETERS} on the chain: {chain:#?}"),
    }
}

/// The reason Workflow #411 B8 builds from the schedule values `reason` carries: a fresh
/// `policy_dispatch_parameters` with each value set in B8's order, its detail the outcome's text.
fn b8_reason(reason: &DecisionReason) -> DecisionReason {
    let value = |key: &str| reason.outcome.value(key);
    let mut outcome = Outcome::new(PolicyCode::PolicyDispatchParameters);
    if let Some(next) = value("next_occurrence_unix_ms").and_then(Value::as_u64) {
        outcome = outcome.next_occurrence_unix_ms(next);
    }
    if let Some(end) = value("window_end_unix_ms").and_then(Value::as_u64) {
        outcome = outcome.window_end_unix_ms(end);
    }
    if let Some(last) = value("last_in_window").and_then(Value::as_bool) {
        outcome = outcome.last_in_window(last);
    }
    if let Some(last) = value("last_daily_slot").and_then(Value::as_bool) {
        outcome = outcome.last_daily_slot(last);
    }
    if let Some(remaining) = value("daily_remaining").and_then(Value::as_u64) {
        outcome = outcome.daily_remaining(remaining);
    }
    DecisionReason {
        detail: outcome.to_string(),
        outcome,
    }
}

/// The compiled catalog's task `task_id`.
fn task_spec<'a>(catalog: &'a CompiledCatalog, task_id: &str) -> &'a TaskSpec {
    catalog
        .catalog()
        .tasks
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .unwrap_or_else(|| panic!("the fixture task {task_id}"))
}

/// A task as the catalog document declares it, outside any catalog (no compiler bound applies).
fn task_spec_of(task: Value) -> TaskSpec {
    serde_json::from_value(task).expect("a task declaration")
}

/// The reserve views the gate judges `task` by when each of its reserved pools is projected at
/// `projected`.
fn projected_views(task: &TaskSpec, projected: u64) -> Vec<ReserveView> {
    reserve_views(task, |_| Ok(Some((projected, None))))
        .expect("the reserve views")
        .expect("the task declares reserves")
}

/// The reserve view of `pool_id`, `at_least` and `consumes` per run, projected at `projected`.
fn view(pool_id: &str, at_least: u64, consumes: u64, projected: Option<u64>) -> ReserveView {
    ReserveView {
        pool_id: pool_id.to_owned(),
        at_least,
        consumes,
        after: projected
            .and_then(|projected| i64::try_from(i128::from(projected) - i128::from(consumes)).ok()),
        projected,
        next_change_unix_ms: None,
    }
}
