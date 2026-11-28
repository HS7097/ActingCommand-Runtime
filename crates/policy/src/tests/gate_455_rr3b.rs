// SPDX-License-Identifier: AGPL-3.0-only

// #455 RR3b (0.12.98; register §1.542; the owner's ruling of 11-28 that what the characters in
// training lack is pushed hard and what a standing stock lacks gently, the rulings 1 to 8 and A1
// the coordinator's of the same day; contracts/resource-targets.md, "Resource weights (v2)";
// contracts/scheduling/README.md, "A goal's gap in two parts" and "Fixed values written in the
// pack"; contracts/standard-pack-schedule.md, "`requires`"): a pool may write `goal_tiers`, and a
// goal's gap on it is weighed in two parts over one step.
// - the reader takes both numbers and the V2 schema lists them with the compiler's bounds; each
//   missing, each out of its bound, another member, the member beside `spend_line` and in a V1
//   document are refused at their pointers; beside a valuation, beside `target_defaults` and alone
//   it is taken; a `schedule.json` that writes it without `pool_goal_tiers` in `requires` is
//   refused, and a Runtime that does not name the subsystem refuses one that does (test 1);
// - the need's part by its own importance, the stock's capped, the holding to the characters
//   first (test 2); without the member the whole gap as on main (test 3);
// - a need of two outranks a stock of 150, in the business tier and at the head of a capped round
//   (test 4); a run is told the need's part while it is open, then the stock's (test 5);
// - beside a fixed value a need makes a negative rule positive and a stock does not (test 6);
// - a goal set once weighs by the member's numbers of the catalogue in force (test 7);
// - the `resource_targets` item names the two parts only on a pool with the member (test 8);
// - a catalogue without the member keeps its golden identity (test 10);
// - ruling A1: below the trigger line of a regenerating pool the task consumes, a goal term of a
//   pool with the member counts; only the others are left out (tests a, b and c).
//
// The numbers of the work order throughout: the gap step `S` 10 and the importance `I` 1000 (the
// pools' `target_defaults`), `I_n` 100 000 and `C` 40 (their `goal_tiers`), a need of 30 and a stock
// of 200. The fixture: one instance, judged tasks on an hourly clock; the capped pool
// `pool.neutral` (valued 1000 milli a unit at the step 1, no regeneration, its capacity read from
// its fact, 1000, its stop line 40 below and its hard line 900 below it); the regenerating pool
// `pool.regen` (valued alike, one unit in six minutes, capacity 240, no lines); the pools
// `pool.tiered` and `pool.stocked`, without a valuation, writing the defaults and the member; the
// pool `pool.plain`, writing the defaults alone. Names are made up; no clock is read.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    Initiator, RESOURCE_TARGETS_FACT_KEY, RuntimeChannel, StandardPackProblem,
    StandardPackRootBytes, read_standard_pack_root,
};
use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::resource_targets::SpendValues;
use crate::{
    CandidateEligibility, CappedMemory, CatalogDiagnosticCode, CatalogDocumentSource,
    CatalogSources, ChainHandoverInputs, CompiledCatalog, DecisionReason, DispatchIntent,
    EvaluationFacts, EvaluationResources, EvaluationTime, FactRefreshInputs, FactRefreshSettings,
    FactValue, HostResourceSnapshot, InstanceHolds, InstancePoolLines, InstanceRecordNeeds,
    LeftOutFacts, MAX_RESOURCE_TARGET_MILLI, NoProgressGuards, ObservedFact, OwedReruns,
    POOL_GOAL_TIERS_SUBSYSTEM, PolicyCode, PolicyEvaluation, PolicyEvaluationError, PoolGoalTiers,
    PoolNeed, PoolNeedKind, PoolsDocument, RecordNeeds, RemedyPlan, RunSlots,
    SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2, ScopeSelector, StandardPackBinding,
    StandardPackComposeInput, StandardPackComposition, StandardPackGameFiles, TargetSetter,
    TaskDecision, check_resource_targets, compile_catalog, compose_standard_pack_rules,
    evaluate_with_left_out_facts, parse_resource_targets, project_fact_pools,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// The capped pool, read from `READING_KEY`, its capacity from `CAPACITY_KEY`.
const POOL: &str = "pool.neutral";
const READING_KEY: &str = "resource.neutral";
const CAPACITY_KEY: &str = "resource.neutral.capacity";
/// The capped pool's capacity.
const CAPACITY: i64 = 1000;
/// The regenerating pool without lines, read from `REGEN_KEY`.
const REGEN: &str = "pool.regen";
const REGEN_KEY: &str = "resource.regen";
/// A pool that writes the member, read from `TIERED_KEY`.
const TIERED: &str = "pool.tiered";
const TIERED_KEY: &str = "resource.tiered";
/// A second pool that writes the member, read from `STOCKED_KEY` (test 4).
const STOCKED: &str = "pool.stocked";
const STOCKED_KEY: &str = "resource.stocked";
/// A pool that writes the defaults and not the member, read from `PLAIN_KEY`.
const PLAIN: &str = "pool.plain";
const PLAIN_KEY: &str = "resource.plain";
/// `S` and `I`: the defaults of every goal on the three pools without a valuation.
const DEFAULTS: (u64, u64) = (10, 1000);
/// `I_n` and `C`: the member's numbers.
const TIERS: (u64, u64) = (100_000, 40);
/// The work order's need and stock.
const NEED: u64 = 30;
const STOCK: u64 = 200;
/// The sha256 the fixture's needs name as their tables'.
const SHA: &str = "fixture-tables-a";
/// The declared duration of every task, `E` without a snapshot.
const E: u64 = 60_000;
/// Ruling 11 of #455 B1: how the detail of a reason judged without some goal terms ends.
const UNCOUNTED: &str = " goal_terms=not_counted_below_trigger_line";
/// The neutral example catalog's golden identities (as in `gate_455_b3p` and `gate_455_sl1`).
const V1_EXAMPLE_HASH: &str =
    "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c";
const V2_EXAMPLE_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";

// Test 1, the reader. Both numbers on the example's pool compile: the compiled pool holds them, the
// pools document writes them back as written and reads back equal, and compiles again to the same
// identity, which the member is part of; the largest bounds and the least (a cap of 0) are taken;
// beside a valuation and beside `target_defaults` it is taken too. Refused, each at its pointer: in
// a V1 document `unknown_field` at the member; each number missing, `missing_required_field` naming
// it; an importance of 0 or 1 000 001 and a cap of 1 000 001, `limit_exceeded` at the number;
// another member, `unknown_field` naming it; beside `spend_line`, `type_mismatch` at the member.
#[test]
fn rr3b_t1_the_reader_takes_both_numbers_and_refuses_each_fault_at_its_pointer() {
    let sources = example(SCHEDULING_SCHEMA_VERSION_V2, |declared| {
        written(declared, tiers_json(TIERS));
    });
    let compiled = compile(&sources);
    let compiled_pools = &compiled.catalog().pools;
    assert_eq!(compiled_pools.pools[0].goal_tiers, Some(goal_tiers(TIERS)));
    let document = serde_json::to_value(compiled_pools).expect("the pools document serializes");
    assert_eq!(document["pools"][0]["goal_tiers"], tiers_json(TIERS));
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
    type Beside = fn(&mut Value);
    let taken: [(&str, (u64, u64), Beside); 4] = [
        ("the largest", (1_000_000, 1_000_000), |_| {}),
        ("the least", (1, 0), |_| {}),
        ("beside a valuation", TIERS, |pool| {
            pool["valuation"] = json!({"name": "Fixture", "unit": "unit", "scale": 1,
                                       "base_weight_milli": 1000});
        }),
        ("beside target_defaults", TIERS, |pool| {
            pool["target_defaults"] = defaults_json(DEFAULTS);
        }),
    ];
    for (case, numbers, beside) in taken {
        let accepted = example(SCHEDULING_SCHEMA_VERSION_V2, |declared| {
            written(declared, tiers_json(numbers));
            beside(&mut declared["pools"][0]);
        });
        assert_eq!(
            compile(&accepted).catalog().pools.pools[0].goal_tiers,
            Some(goal_tiers(numbers)),
            "{case}"
        );
    }

    use CatalogDiagnosticCode::{LimitExceeded, MissingRequiredField, TypeMismatch, UnknownField};
    let member = "/pools/0/goal_tiers";
    let v1 = example(SCHEDULING_SCHEMA_VERSION, |declared| {
        written(declared, tiers_json(TIERS));
    });
    assert!(
        refused_at(&v1, UnknownField, member),
        "a V1 document: {:?}",
        diagnostics(&v1)
    );

    for missing in ["need_importance_milli", "stock_cap_milli"] {
        let mut present = tiers_json(TIERS);
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
        written(declared, tiers_json(TIERS));
        declared["pools"][0]["goal_tiers"]["extra_milli"] = json!(1);
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
    let cases: [(&str, Edit, CatalogDiagnosticCode, &str); 4] = [
        (
            "an importance of 0",
            |pool| pool["goal_tiers"]["need_importance_milli"] = json!(0),
            LimitExceeded,
            "/pools/0/goal_tiers/need_importance_milli",
        ),
        (
            "an importance of 1 000 001",
            |pool| pool["goal_tiers"]["need_importance_milli"] = json!(1_000_001),
            LimitExceeded,
            "/pools/0/goal_tiers/need_importance_milli",
        ),
        (
            "a cap of 1 000 001",
            |pool| pool["goal_tiers"]["stock_cap_milli"] = json!(1_000_001),
            LimitExceeded,
            "/pools/0/goal_tiers/stock_cap_milli",
        ),
        (
            "beside spend_line",
            |pool| {
                pool["spend_line"] = json!({"price_milli": 33333, "scale": 6000,
                                            "income_floor_milli_per_day": 71429});
            },
            TypeMismatch,
            "/pools/0/goal_tiers",
        ),
    ];
    for (case, edit, code, path) in cases {
        let sources = example(SCHEDULING_SCHEMA_VERSION_V2, |declared| {
            written(declared, tiers_json(TIERS));
            edit(&mut declared["pools"][0]);
        });
        assert!(
            refused_at(&sources, code, path),
            "{case}: {code:?} at {path}: {:?}",
            diagnostics(&sources)
        );
    }
}

// Test 1, the composer (ruling 6). A `schedule.json` pool that writes `goal_tiers`, with
// `pool_goal_tiers` in `requires`, composes: the composed pools document carries the member as
// written and the compiled pool its two numbers. Without the entry the file is refused,
// `schedule_invalid` at the pool's member. On a Runtime whose manifest does not name the subsystem
// (as no Runtime of 0.12.97 does) the file that lists it is refused at that entry
// (`schedule_requires_missing`).
#[test]
fn rr3b_t1_the_composer_requires_pool_goal_tiers_and_an_older_runtime_refuses_the_file() {
    let composed = compose(&schedule(&[POOL_GOAL_TIERS_SUBSYSTEM]), None)
        .unwrap_or_else(|failure| panic!("the file composes: {failure}"));
    let composed_pools = serde_json::from_slice::<Value>(&composed.sources().pools.bytes)
        .expect("the composed pools document");
    assert_eq!(
        composed_pools["pools"][0]["goal_tiers"],
        tiers_json(TIERS),
        "{composed_pools}"
    );
    assert_eq!(
        composed.compiled().catalog().pools.pools[0].goal_tiers,
        Some(goal_tiers(TIERS))
    );

    let failure = compose(&schedule(&[]), None).expect_err("requires");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleInvalid);
    assert_eq!(issue.pointer, "/pools/0/goal_tiers");

    let older = BTreeSet::from(["standard_pack_schedule".to_owned()]);
    let failure = compose(&schedule(&[POOL_GOAL_TIERS_SUBSYSTEM]), Some(&older))
        .expect_err("a Runtime that does not name the subsystem");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleRequiresMissing);
    assert_eq!(issue.pointer, "/requires/0");
}

// Test 1, the schema. The V2 pools schema lists the member, its two numbers required within the
// bounds the compiler holds them to, and not beside `spend_line`; the V1 schema does not list it.
#[test]
fn rr3b_t1_the_v2_pools_schema_lists_the_member_with_the_compiler_s_bounds() {
    let schema = |text: &str| -> Value { serde_json::from_str(text).expect("a schema document") };
    let v2 = schema(include_str!(
        "../../../../contracts/scheduling/v2/pools.schema.json"
    ));
    let v1 = schema(include_str!(
        "../../../../contracts/scheduling/pools.schema.json"
    ));
    assert!(
        v1["$defs"]["pool"]["properties"]
            .get("goal_tiers")
            .is_none()
    );
    let pool = &v2["$defs"]["pool"];
    assert_eq!(
        pool["properties"]["goal_tiers"]["$ref"],
        "#/$defs/goalTiers"
    );
    assert_eq!(
        pool["dependentSchemas"]["goal_tiers"],
        json!({"not": {"required": ["spend_line"]}})
    );
    let declared = &v2["$defs"]["goalTiers"];
    assert_eq!(
        declared["required"],
        json!(["need_importance_milli", "stock_cap_milli"])
    );
    assert_eq!(declared["additionalProperties"], false);
    for (field, least) in [("need_importance_milli", 1), ("stock_cap_milli", 0)] {
        let number = &declared["properties"][field];
        assert_eq!(number["minimum"], least, "{field}");
        assert_eq!(number["maximum"], MAX_RESOURCE_TARGET_MILLI, "{field}");
    }
}

// Test 2, the weight (rulings 2 and 3). A stored goal of 200 on the pool with the member and a need
// of 30 stacked on it (`at_least` 230), weighed over the one step 10:
// - held 50: the 30 the characters need are covered, the stock is counted on the 20 left, `g_n` 0,
//   `g_s` 180, `Γ = min(18 000, 40) = 40`;
// - held 10: `g_n` 20, `g_s` 200, `Γ_n = 200 000`, `Γ_s = 40`, `Γ = 200 040`;
// - held 230: satisfied, as today;
// - no records, held 50: `g_s` 150, `Γ = 40`;
// - a need of 30 and no stored goal, held 10: the computed goal of 30, `g_n` 20, `Γ = 200 000`;
// - `I_n` 1 000 000 and a need gap of 30 (held 0): `Γ_n` capped at 1 000 000, `Γ` too, ` capped`.
// `gap` and `gap_weight` are the totals, and the producer's term reads `Γ`.
#[test]
fn rr3b_t2_the_need_and_the_stock_are_weighed_apart_over_one_step() {
    let compiled = || single(pools(Some(TIERS)));
    let stored = goal_record(&compiled(), vec![goal_target("goal-t", TIERED, STOCK)]);
    let item_of = |mut world: World| {
        let evaluation = world.evaluate();
        reason(decision(&evaluation, "task_t"), PolicyCode::ResourceTargets)
            .detail
            .clone()
    };
    let parts = "need_importance=100000";
    for (case, held, needs, item) in [
        (
            "held 50",
            50,
            true,
            format!(
                "goal-t@pool.tiered:applied current=50 at_least=230 gap=180 step=10 importance=1000 \
                 gap_weight=40 need_gap=0 {parts} need_weight=0 stock_gap=180 stock_weight=40 \
                 stock_cap=40"
            ),
        ),
        (
            "held 10",
            10,
            true,
            format!(
                "goal-t@pool.tiered:applied current=10 at_least=230 gap=220 step=10 importance=1000 \
                 gap_weight=200040 need_gap=20 {parts} need_weight=200000 stock_gap=200 \
                 stock_weight=40 stock_cap=40"
            ),
        ),
        (
            "held 230",
            230,
            true,
            "goal-t@pool.tiered:satisfied current=230 at_least=230 gap=0 gap_weight=0".to_owned(),
        ),
        (
            "no records, held 50",
            50,
            false,
            format!(
                "goal-t@pool.tiered:applied current=50 at_least=200 gap=150 step=10 importance=1000 \
                 gap_weight=40 need_gap=0 {parts} need_weight=0 stock_gap=150 stock_weight=40 \
                 stock_cap=40"
            ),
        ),
    ] {
        let mut world =
            World::new(compiled(), &[(READING_KEY, 500), (TIERED_KEY, held)]).with_goal(&stored);
        if needs {
            world = world.with_needs(&[(TIERED, NEED)]);
        }
        assert_eq!(item_of(world), item, "{case}");
    }

    let computed = World::new(compiled(), &[(READING_KEY, 500), (TIERED_KEY, 10)])
        .with_needs(&[(TIERED, NEED)]);
    assert_eq!(
        item_of(computed),
        format!(
            "goal_records@pool.tiered:applied current=10 at_least=30 gap=20 step=10 \
             importance=1000 gap_weight=200000 need_gap=20 {parts} need_weight=200000 stock_gap=0 \
             stock_weight=0 stock_cap=40"
        )
    );

    let strong = || single(pools(Some((1_000_000, 40))));
    let mut capped = World::new(strong(), &[(READING_KEY, 500), (TIERED_KEY, 0)])
        .with_goal(&goal_record(
            &strong(),
            vec![goal_target("goal-t", TIERED, STOCK)],
        ))
        .with_needs(&[(TIERED, NEED)]);
    let evaluation = capped.evaluate();
    let producer = decision(&evaluation, "task_t");
    assert_eq!(
        reason(producer, PolicyCode::ResourceTargets).detail,
        "goal-t@pool.tiered:applied current=0 at_least=230 gap=230 step=10 importance=1000 \
         gap_weight=1000000 capped need_gap=30 need_importance=1000000 need_weight=1000000 \
         stock_gap=200 stock_weight=40 stock_cap=40"
    );
    let weights = &reason(producer, PolicyCode::ResourceWeights).detail;
    assert!(
        weights.ends_with(
            " items=pool.tiered:r=1,per=10,base=0,gap=1000000,effective=1000000,term=100000"
        ),
        "{weights}"
    );
}

// Test 3, a pool without the member (ruling 3's last sentence). The same catalogue with the member
// taken off: held 10 the whole gap 220 weighs `⌊1000·220/10⌋ = 22 000`, held 50 the gap 180 weighs
// 18 000, main's numbers, and the item is main's text, without the two parts.
#[test]
fn rr3b_t3_without_the_member_the_whole_gap_is_weighed_as_on_main() {
    let compiled = || single(pools(None));
    let stored = goal_record(&compiled(), vec![goal_target("goal-t", TIERED, STOCK)]);
    for (held, item) in [
        (
            10,
            "goal-t@pool.tiered:applied current=10 at_least=230 gap=220 step=10 importance=1000 \
             gap_weight=22000",
        ),
        (
            50,
            "goal-t@pool.tiered:applied current=50 at_least=230 gap=180 step=10 importance=1000 \
             gap_weight=18000",
        ),
    ] {
        let mut world = World::new(compiled(), &[(READING_KEY, 500), (TIERED_KEY, held)])
            .with_goal(&stored)
            .with_needs(&[(TIERED, NEED)]);
        let evaluation = world.evaluate();
        assert_eq!(
            reason(decision(&evaluation, "task_t"), PolicyCode::ResourceTargets).detail,
            item,
            "held {held}"
        );
    }
}

// Test 4, the order (rulings 1 and 3). `task_s` and, declared after it, `task_n`, each of `V`
// 10 000 consuming 1 of the capped pool a run (cost 1000); `task_s` produces 1 of `pool.stocked`,
// under a stored goal of 150 and held 0 (no need: a stock gap of 150), `task_n` 1 of `pool.tiered`,
// on which the records need 2 and nothing is stored (a need gap of 2, no stock gap). With the
// member on both pools: `Γ` 40 and 20 000, the terms 4 and 2000, so `task_n` is 11 000 against
// 9 004 and goes first. With the member taken off both (today's behaviour): `Γ` 15 000 and 200, the
// terms 1500 and 20, so `task_s` is 10 500 against 9 020 and goes first. Read at its capacity the
// capped pool's episode begins, and the same pair heads it by the same terms: `task_n` with the
// member, `task_s` without.
#[test]
fn rr3b_t4_a_need_of_two_outranks_a_stock_of_150_and_heads_the_capped_round() {
    let pair = |tiers: Option<(u64, u64)>| {
        catalog(
            vec![
                fixed(
                    judged(
                        "task_s",
                        json!([consume(POOL, 1)]),
                        json!([produce(STOCKED, 1)]),
                    ),
                    10_000,
                ),
                fixed(
                    judged(
                        "task_n",
                        json!([consume(POOL, 1)]),
                        json!([produce(TIERED, 1)]),
                    ),
                    10_000,
                ),
            ],
            pools(tiers),
        )
    };
    let world = |tiers: Option<(u64, u64)>, reading: i64| {
        let compiled = pair(tiers);
        let stored = goal_record(&compiled, vec![goal_target("goal-s", STOCKED, 150)]);
        World::new(
            compiled,
            &[(READING_KEY, reading), (TIERED_KEY, 0), (STOCKED_KEY, 0)],
        )
        .with_goal(&stored)
        .with_needs(&[(TIERED, 2)])
    };
    let term_of = |evaluation: &PolicyEvaluation, task_id: &str| {
        reason(decision(evaluation, task_id), PolicyCode::ResourceWeights)
            .detail
            .clone()
    };

    let mut tiered = world(Some(TIERS), 500);
    let evaluation = tiered.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_n"], "{evaluation:#?}");
    fixed_first(decision(&evaluation, "task_n"), 11_000, 10_000, E);
    fixed_first(decision(&evaluation, "task_s"), 9_004, 10_000, E);
    let need_term = term_of(&evaluation, "task_n");
    assert!(
        need_term.ends_with(
            " term=2000 items=pool.tiered:r=1,per=10,base=0,gap=20000,effective=20000,term=2000"
        ),
        "{need_term}"
    );
    let stock_term = term_of(&evaluation, "task_s");
    assert!(
        stock_term
            .ends_with(" term=4 items=pool.stocked:r=1,per=10,base=0,gap=40,effective=40,term=4"),
        "{stock_term}"
    );

    let mut plain = world(None, 500);
    let evaluation = plain.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_s"], "{evaluation:#?}");
    fixed_first(decision(&evaluation, "task_s"), 10_500, 10_000, E);
    fixed_first(decision(&evaluation, "task_n"), 9_020, 10_000, E);

    for (case, tiers, head, waits) in [
        ("with the member", Some(TIERS), "task_n", "task_s"),
        ("without the member", None, "task_s", "task_n"),
    ] {
        let mut round = world(tiers, CAPACITY);
        let evaluation = round.evaluate();
        assert_eq!(dispatched(&evaluation), [head], "{case}: {evaluation:#?}");
        assert_eq!(
            spend(&evaluation, head).value("capped_rank"),
            Some(&json!(1)),
            "{case}"
        );
        assert_eq!(
            hold(decision(&evaluation, waits)).as_deref(),
            Some("capped_not_head"),
            "{case}"
        );
    }
}

// Test 5, the runs (ruling 4). `task_t` of `V` 10 000 consumes 1 of the capped pool read at 500
// (its hard line 100: 400 runs above it) and produces 1 of the pool with the member, one unit a run,
// under a stored goal of 150 with the records' need of 30. Held 25: a need gap of 5 and a stock gap
// of 150, so the run is told 5 (`gap_remaining` 5, `run_cap` min(400, 5) = 5); held 30, the need
// met: 150 and 150; the pool without the member, held 25: today's 155 and 155.
#[test]
fn rr3b_t5_a_run_is_told_the_need_s_part_while_it_is_open_then_the_stock_s() {
    for (case, tiers, held, told) in [
        ("a need gap of 5", Some(TIERS), 25, 5),
        ("the need met", Some(TIERS), 30, 150),
        ("without the member", None, 25, 155),
    ] {
        let compiled = single(pools(tiers));
        let stored = goal_record(&compiled, vec![goal_target("goal-t", TIERED, 150)]);
        let mut world = World::new(compiled, &[(READING_KEY, 500), (TIERED_KEY, held)])
            .with_goal(&stored)
            .with_needs(&[(TIERED, NEED)]);
        let evaluation = world.evaluate();
        assert_eq!(
            dispatched(&evaluation),
            ["task_t"],
            "{case}: {evaluation:#?}"
        );
        spend_values_are(&evaluation, "task_t", told, told);
    }
}

// Test 6, beside a fixed value (B1's goal term). `task_t` of `V` 900 consumes 1 of the capped pool
// (below its capacity; it does not regenerate) at a cost of 1000, so it is −100 of its own, and
// produces 1 of the pool with the member under the stored goal of 200. With the need of 30 and 10
// held, `Γ = 200 040`, its term 20 004: `B = 19 904`, dispatched. With no records and 50 held the
// stock's part alone, `Γ = 40`, its term 4: `B = −96`, `policy_benefit_nonpositive`.
#[test]
fn rr3b_t6_a_need_makes_a_negative_rule_positive_and_a_stock_does_not() {
    let compiled = || single_valued(900, pools(Some(TIERS)));
    let stored = goal_record(&compiled(), vec![goal_target("goal-t", TIERED, STOCK)]);
    let mut need = World::new(compiled(), &[(READING_KEY, 500), (TIERED_KEY, 10)])
        .with_goal(&stored)
        .with_needs(&[(TIERED, NEED)]);
    let evaluation = need.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_t"], "{evaluation:#?}");
    fixed_first(decision(&evaluation, "task_t"), 19_904, 900, E);

    let mut stock =
        World::new(compiled(), &[(READING_KEY, 500), (TIERED_KEY, 50)]).with_goal(&stored);
    let evaluation = stock.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let waits = decision(&evaluation, "task_t");
    fixed_first(waits, -96, 900, E);
    assert_eq!(
        reason(waits, PolicyCode::PolicyBenefitNonpositive).detail,
        "benefit_milli=-96 expected_duration_ms=60000 items=pool.tiered:+4,pool.neutral:-1000"
    );
}

// Test 7, ruling 7's last sentence. The stored goal of 200 and the need of 30, held 10, under the
// catalogue whose member writes 100 000 and 40 (`Γ` 200 040) and then under the next one, which
// differs only in the member's numbers, 50 000 and 20: the same stored fact, set again by nobody,
// weighs by them, `Γ_n = ⌊50 000·20/10⌋ = 100 000`, `Γ_s = 20`, `Γ = 100 020`.
#[test]
fn rr3b_t7_a_goal_set_once_weighs_by_the_numbers_of_the_catalogue_in_force() {
    let stored = goal_record(
        &single(pools(Some(TIERS))),
        vec![goal_target("goal-t", TIERED, STOCK)],
    );
    for (case, tiers, item) in [
        (
            "the first catalogue",
            TIERS,
            "goal-t@pool.tiered:applied current=10 at_least=230 gap=220 step=10 importance=1000 \
             gap_weight=200040 need_gap=20 need_importance=100000 need_weight=200000 \
             stock_gap=200 stock_weight=40 stock_cap=40",
        ),
        (
            "the next catalogue",
            (50_000, 20),
            "goal-t@pool.tiered:applied current=10 at_least=230 gap=220 step=10 importance=1000 \
             gap_weight=100020 need_gap=20 need_importance=50000 need_weight=100000 \
             stock_gap=200 stock_weight=20 stock_cap=20",
        ),
    ] {
        let mut world = World::new(
            single(pools(Some(tiers))),
            &[(READING_KEY, 500), (TIERED_KEY, 10)],
        )
        .with_goal(&stored)
        .with_needs(&[(TIERED, NEED)]);
        let evaluation = world.evaluate();
        assert_eq!(
            reason(decision(&evaluation, "task_t"), PolicyCode::ResourceTargets).detail,
            item,
            "{case}"
        );
    }
}

// Test 8, what a reader sees (ruling 5). One task produces 1 of the pool with the member and 1 of
// the pool without it, each under a stored goal of 200 and held 50, no records: the item of the
// first names its two parts after main's text, the item of the second is main's text alone.
#[test]
fn rr3b_t8_the_item_names_the_two_parts_only_on_a_pool_with_the_member() {
    let compiled = || {
        catalog(
            vec![fixed(
                judged(
                    "task_t",
                    json!([consume(POOL, 1)]),
                    json!([produce(TIERED, 1), produce(PLAIN, 1)]),
                ),
                10_000,
            )],
            pools(Some(TIERS)),
        )
    };
    let stored = goal_record(
        &compiled(),
        vec![
            goal_target("goal-t", TIERED, STOCK),
            goal_target("goal-p", PLAIN, STOCK),
        ],
    );
    let mut world = World::new(
        compiled(),
        &[(READING_KEY, 500), (TIERED_KEY, 50), (PLAIN_KEY, 50)],
    )
    .with_goal(&stored);
    let evaluation = world.evaluate();
    assert_eq!(
        reason(decision(&evaluation, "task_t"), PolicyCode::ResourceTargets).detail,
        "goal-t@pool.tiered:applied current=50 at_least=200 gap=150 step=10 importance=1000 \
         gap_weight=40 need_gap=0 need_importance=100000 need_weight=0 stock_gap=150 \
         stock_weight=40 stock_cap=40; goal-p@pool.plain:applied current=50 at_least=200 \
         gap=150 step=10 importance=1000 gap_weight=15000"
    );
}

// Test 10, ruling 7. The neutral example catalogue, which writes no `goal_tiers`, keeps its V1 and
// V2 golden identities; with the member its identity differs.
#[test]
fn rr3b_t10_a_catalogue_without_the_member_keeps_its_golden_identity() {
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
    let tiered = example(SCHEDULING_SCHEMA_VERSION_V2, |declared| {
        written(declared, tiers_json(TIERS));
    });
    assert_ne!(compile(&tiered).catalog_hash(), V2_EXAMPLE_HASH);
}

// Ruling A1, test (a). `task_r` of `V` 998 consumes 1 of the regenerating pool (cost 1000, read at
// 100, below its trigger line, its capacity 240), so it is −2 of its own, and produces 1 of the
// pool with the member under the stored goal of 200, held 50: the stock's capped term 4 counts
// there, `B = 2` beyond its rounding bound (0.55), and it is dispatched below the line. With the
// member taken off, today's ruling 9: the term (1500) is not counted below the line, and `task_r`
// waits `policy_benefit_nonpositive` −2, its items the consumption's alone and its detail ending
// ` goal_terms=not_counted_below_trigger_line`.
#[test]
fn rr3b_a1_a_below_the_trigger_line_a_stock_s_capped_term_counts() {
    let world = |tiers: Option<(u64, u64)>| {
        let compiled = regenerating_rule(998, json!([produce(TIERED, 1)]), tiers);
        let stored = goal_record(&compiled, vec![goal_target("goal-t", TIERED, STOCK)]);
        World::new(compiled, &[(REGEN_KEY, 100), (TIERED_KEY, 50)]).with_goal(&stored)
    };
    let mut tiered = world(Some(TIERS));
    let evaluation = tiered.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_r"], "{evaluation:#?}");
    fixed_first(decision(&evaluation, "task_r"), 2, 998, E);

    let mut plain = world(None);
    let evaluation = plain.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let waits = decision(&evaluation, "task_r");
    fixed_first(waits, -2, 998, E);
    assert_eq!(
        reason(waits, PolicyCode::PolicyBenefitNonpositive).detail,
        format!("benefit_milli=-2 expected_duration_ms=60000 items=pool.regen:-1000{UNCOUNTED}")
    );
}

// Ruling A1, test (b). `task_r` of `V` 900 is −100 of its own below the regenerating pool's line.
// With the stock's part alone (the stored goal of 200, held 50) its term 4 counts: `B = −96`, not
// positive there, the ordinary reason, its items naming the counted term and no note. With a need
// gap of 2 (the records need 52, held 50; the stock's part 200 beside it) `Γ = 20 000 + 40`, its
// term 2004: `B = 1904`, dispatched there.
#[test]
fn rr3b_a1_b_below_the_trigger_line_a_need_lifts_a_rule_a_stock_does_not() {
    let compiled = || regenerating_rule(900, json!([produce(TIERED, 1)]), Some(TIERS));
    let stored = goal_record(&compiled(), vec![goal_target("goal-t", TIERED, STOCK)]);
    let mut stock =
        World::new(compiled(), &[(REGEN_KEY, 100), (TIERED_KEY, 50)]).with_goal(&stored);
    let evaluation = stock.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let waits = decision(&evaluation, "task_r");
    fixed_first(waits, -96, 900, E);
    assert_eq!(
        reason(waits, PolicyCode::PolicyBenefitNonpositive).detail,
        "benefit_milli=-96 expected_duration_ms=60000 items=pool.tiered:+4,pool.regen:-1000"
    );

    let mut need = World::new(compiled(), &[(REGEN_KEY, 100), (TIERED_KEY, 50)])
        .with_goal(&stored)
        .with_needs(&[(TIERED, 52)]);
    let evaluation = need.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_r"], "{evaluation:#?}");
    fixed_first(decision(&evaluation, "task_r"), 1_904, 900, E);
}

// Ruling A1, test (c). `task_r` of `V` 900 produces 1 of the pool with the member and 1 of the pool
// without it, each under a stored goal of 200 and held 50 (terms 4 and 1500; `B` 1404 with both).
// Below the regenerating pool's line the gate reads the first and leaves the second out: `B = −96`,
// `policy_benefit_nonpositive`, its items naming the counted term and the consumption, its detail
// ending with the existing note.
#[test]
fn rr3b_a1_c_only_the_goal_terms_of_pools_without_the_member_are_left_out() {
    let compiled = || {
        regenerating_rule(
            900,
            json!([produce(TIERED, 1), produce(PLAIN, 1)]),
            Some(TIERS),
        )
    };
    let stored = goal_record(
        &compiled(),
        vec![
            goal_target("goal-t", TIERED, STOCK),
            goal_target("goal-p", PLAIN, STOCK),
        ],
    );
    let mut world = World::new(
        compiled(),
        &[(REGEN_KEY, 100), (TIERED_KEY, 50), (PLAIN_KEY, 50)],
    )
    .with_goal(&stored);
    let evaluation = world.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let waits = decision(&evaluation, "task_r");
    fixed_first(waits, -96, 900, E);
    assert_eq!(
        reason(waits, PolicyCode::PolicyBenefitNonpositive).detail,
        format!(
            "benefit_milli=-96 expected_duration_ms=60000 items=pool.tiered:+4,pool.regen:-1000\
             {UNCOUNTED}"
        )
    );
}

// ---- The world a test drives ----

/// The catalog, the Runtime's memory of the capped episodes, the pools' readings, the instance's
/// goal record and the needs its records compute.
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    /// Each pool's reading key and reading, a minute before NOW.
    readings: Vec<(&'static str, i64)>,
    /// The instance's `session.resource_targets` record; `None` without a goal.
    goal: Option<ObservedFact>,
    /// The needs the host computed from the instance's records; `None` without records.
    needs: Option<RecordNeeds>,
}

impl World {
    /// The world at NOW without a goal and without records, the pools read as `readings` a
    /// minute before and the capped pool's capacity an hour before.
    fn new(compiled: CompiledCatalog, readings: &[(&'static str, i64)]) -> Self {
        Self {
            catalog: compiled,
            memory: CappedMemory::default(),
            readings: readings.to_vec(),
            goal: None,
            needs: None,
        }
    }

    /// The same world with the goal record `goal`.
    fn with_goal(mut self, goal: &ObservedFact) -> Self {
        self.goal = Some(goal.clone());
        self
    }

    /// The same world with records whose computed needs land on `landing` (pool, need), of the
    /// tables [`SHA`], set an hour before NOW, of no expiry.
    fn with_needs(mut self, landing: &[(&str, u64)]) -> Self {
        let mut computed = RecordNeeds::default();
        computed.give(
            A,
            InstanceRecordNeeds::new(
                Some(SHA.to_owned()),
                NOW - HOUR_MS,
                landing
                    .iter()
                    .map(|(pool, need)| {
                        (
                            (*pool).to_owned(),
                            PoolNeed {
                                need: *need,
                                kind: PoolNeedKind::Lands,
                            },
                        )
                    })
                    .collect(),
                Vec::new(),
                None,
            ),
        );
        self.needs = Some(computed);
        self
    }

    fn facts(&self) -> EvaluationFacts {
        let mut observed = vec![fact(A, CAPACITY_KEY, CAPACITY, NOW - HOUR_MS, None)];
        observed.extend(
            self.readings
                .iter()
                .map(|(key, reading)| fact(A, key, *reading, NOW - MINUTE_MS, None)),
        );
        observed.extend(self.goal.clone());
        EvaluationFacts {
            facts: observed,
            ..bare_facts()
        }
    }

    /// One round by the Runtime's entry, with its memory, the needs beside the left-out facts,
    /// every candidate admitted.
    fn evaluate(&mut self) -> PolicyEvaluation {
        let facts = self.facts();
        let resource_snapshot = resources(&self.catalog, &facts);
        let mut left_out = LeftOutFacts::default();
        if let Some(computed) = &self.needs {
            left_out.give_record_needs(computed.clone());
        }
        evaluate_with_left_out_facts::<PolicyEvaluationError>(
            &self.catalog,
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
            &mut self.memory,
            &InstancePoolLines::default(),
            &left_out,
            |_, _, _| Ok(false),
            |_: &DispatchIntent| Ok(CandidateEligibility::Eligible),
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
    }
}

/// The instance with no fact.
fn bare_facts() -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-455-rr3b".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

/// A v3 target `id` on `pool`, at least `at_least`, mode `adjust`, without numbers of its own (the
/// pool's defaults give them), no tasks and no lifetime.
fn goal_target(id: &str, pool: &str, at_least: u64) -> Value {
    json!({
        "id": id,
        "resource": pool,
        "condition": {"kind": "at_least", "amount": at_least},
        "apply": {"mode": "adjust", "weight": "score_stage"}
    })
}

/// The v3 goal record of `targets`, checked against `compiled` and stored as the formal entry
/// stores it for a Lab connection, an hour before NOW.
fn goal_record(compiled: &CompiledCatalog, targets: Vec<Value>) -> ObservedFact {
    let document = json!({
        "schema_version": "actingcommand.resource-targets.v3",
        "instance": A,
        "targets": targets
    });
    let parsed = parse_resource_targets(&serde_json::to_vec(&document).expect("document bytes"))
        .unwrap_or_else(|error| panic!("the goal parses: {error:?}"));
    let set_at = NOW - HOUR_MS;
    let accepted = check_resource_targets(&parsed, compiled, &bare_facts(), time(set_at))
        .unwrap_or_else(|error| panic!("the goal is accepted: {error:?}"));
    let composed = accepted
        .compose(
            None,
            Some(TargetSetter {
                set_unix_ms: set_at,
                channel: RuntimeChannel::Lab,
                initiator: Some(Initiator::LabCli),
            }),
        )
        .unwrap_or_else(|error| panic!("the goal composes: {error:?}"))
        .expect("with a setter every document composes");
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: RESOURCE_TARGETS_FACT_KEY.to_owned(),
        value: FactValue::RecordList(
            serde_json::from_value(serde_json::to_value(&composed.rows).expect("rows"))
                .expect("record list"),
        ),
        observed_at_unix_ms: set_at,
        expires_at_unix_ms: composed.expires_at_unix_ms,
        confidence_milli: 1000,
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

/// The `pool_line_hold` of the decision's `pool_line_held` reason, when it has one.
fn hold(entry: &TaskDecision) -> Option<String> {
    entry
        .reasons
        .iter()
        .find(|given| given.outcome.is(PolicyCode::PoolLineHeld))
        .and_then(|held| held.outcome.value("pool_line_hold"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The `capped_spend` reason of `task_id`'s dispatch.
fn spend<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a Outcome {
    &reason(decision(evaluation, task_id), PolicyCode::CappedSpend).outcome
}

/// `task_id`'s dispatch is handed `gap` at its production entry 0 and `run_cap`, as SS-6 builds
/// them (`policy_dispatch_parameters`).
fn spend_values_are(evaluation: &PolicyEvaluation, task_id: &str, gap: u64, run_cap: u64) {
    let given = &reason(
        decision(evaluation, task_id),
        PolicyCode::PolicyDispatchParameters,
    )
    .outcome;
    let built = SpendValues {
        spend_allowances: Vec::new(),
        gaps_remaining: vec![(0, gap)],
        run_cap: Some(run_cap),
    }
    .add_to(Outcome::new(PolicyCode::PolicyDispatchParameters));
    for key in ["gaps_remaining", "run_cap"] {
        assert_eq!(given.value(key), built.value(key), "{key}: {given:?}");
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

fn consume(pool: &str, amount: u64) -> Value {
    effect(pool, "consume", amount)
}

fn produce(pool: &str, amount: u64) -> Value {
    effect(pool, "produce", amount)
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

/// `task` with the fixed value `value`.
fn fixed(mut task: Value, value: i64) -> Value {
    task["benefit_fixed_value_milli"] = json!(value);
    task
}

/// `task_t` of `V` `value`, consuming 1 of the capped pool and producing 1 of the pool with the
/// member a run, over `pool_list`.
fn single_valued(value: i64, pool_list: Value) -> CompiledCatalog {
    catalog(
        vec![fixed(
            judged(
                "task_t",
                json!([consume(POOL, 1)]),
                json!([produce(TIERED, 1)]),
            ),
            value,
        )],
        pool_list,
    )
}

/// [`single_valued`] of `V` 10 000.
fn single(pool_list: Value) -> CompiledCatalog {
    single_valued(10_000, pool_list)
}

/// `task_r` of `V` `value`, consuming 1 of the regenerating pool and producing `produces` a run,
/// over [`pools`] with `tiers`.
fn regenerating_rule(value: i64, produces: Value, tiers: Option<(u64, u64)>) -> CompiledCatalog {
    catalog(
        vec![fixed(
            judged("task_r", json!([consume(REGEN, 1)]), produces),
            value,
        )],
        pools(tiers),
    )
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

/// [`declared_pool`] valued 1000 milli a unit at the step 1.
fn valued(id: &str, capacity: u64) -> Value {
    let mut pool = declared_pool(id, capacity);
    pool["valuation"] =
        json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000});
    pool
}

/// The capacity fact plus `offset`.
fn line(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

/// `{"scale": S, "importance_milli": I}`.
fn defaults_json((scale, importance_milli): (u64, u64)) -> Value {
    json!({"scale": scale, "importance_milli": importance_milli})
}

/// `{"need_importance_milli": I_n, "stock_cap_milli": C}`.
fn tiers_json((need_importance_milli, stock_cap_milli): (u64, u64)) -> Value {
    json!({"need_importance_milli": need_importance_milli, "stock_cap_milli": stock_cap_milli})
}

/// The compiled member of [`tiers_json`]'s numbers.
fn goal_tiers((need_importance_milli, stock_cap_milli): (u64, u64)) -> PoolGoalTiers {
    PoolGoalTiers {
        need_importance_milli,
        stock_cap_milli,
    }
}

/// The fixture's pools: the capped pool (its capacity read from its fact, the stop line 40 and the
/// hard line 900 below it), the regenerating pool (one unit in six minutes, capacity 240, no
/// lines), the two pools without a valuation that write [`DEFAULTS`] and, when `tiers` is given,
/// `goal_tiers` of those numbers (without them, main's reading of the same pools), and the pool
/// that writes [`DEFAULTS`] alone.
fn pools(tiers: Option<(u64, u64)>) -> Value {
    let mut capped = valued(POOL, 1);
    capped["capacity"] = line(0);
    capped["stop_line"] = line(-40);
    capped["hard_line"] = line(-900);
    let mut regen = valued(REGEN, 240);
    regen["projection"] = json!({"amount": 1, "per_ms": 6 * MINUTE_MS});
    let defaulted = |id: &str| {
        let mut pool = declared_pool(id, 1_000_000);
        pool["target_defaults"] = defaults_json(DEFAULTS);
        pool
    };
    let mut tiered = defaulted(TIERED);
    let mut stocked = defaulted(STOCKED);
    if let Some(numbers) = tiers {
        tiered["goal_tiers"] = tiers_json(numbers);
        stocked["goal_tiers"] = tiers_json(numbers);
    }
    json!([capped, regen, tiered, stocked, defaulted(PLAIN)])
}

fn catalog(tasks: Vec<Value>, pool_list: Value) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.455-rr3b",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-rr3b"]
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

/// The example's pools document `declared` with its pool 0 writing `tiers` as its `goal_tiers`.
fn written(declared: &mut Value, tiers: Value) {
    declared["pools"][0]["goal_tiers"] = tiers;
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-455-rr3b/{name}.json"),
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

/// `schedule.json` with one rule on an hourly clock, consuming nothing, and a pool without a
/// valuation that writes the member (no scope: the composer fills it), naming `requires`.
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
            "id": TIERED,
            "capacity": 1_000_000,
            "projection": {"kind": "none"},
            "observation": {"kind": "fact", "fact_key": TIERED_KEY},
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": null,
            "goal_tiers": tiers_json(TIERS)
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
