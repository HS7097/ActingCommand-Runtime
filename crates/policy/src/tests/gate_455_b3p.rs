// SPDX-License-Identifier: AGPL-3.0-only

// #455 B3-P (0.12.82; register §1.522; the coordinator's rulings of 11-23, no words of the owner's
// behind them; coordinator work order `rd-1123/brief-455-b3p.md`; contracts/scheduling/README.md,
// "A goal's step and importance on a pool without a valuation"; contracts/resource-targets.md,
// "Who gives a goal its step and importance"; contracts/standard-pack-schedule.md, "`requires`"):
// a pool that declares no valuation may declare the step and the importance a goal on it takes
// (`target_defaults`).
// - the reader takes both numbers and the V2 schema lists them with the compiler's bounds; one
//   missing, each out of its bound, the member beside a valuation block or the pending marker and
//   the member in a V1 document are refused at their pointers; a `schedule.json` that writes it
//   without `pool_target_defaults` in `requires` is refused, and a Runtime that does not name the
//   subsystem refuses one that does (test 1);
// - a target without numbers of its own resolves to the pool's defaults, one with its own keeps
//   them, a valued pool gives its valuation's and a pool with neither gives none, in the
//   targetable list, the entry's check and the evaluation alike (test 2);
// - with B1, a fixed-value stage producing such a pool under a goal without numbers decides byte for
//   byte as under the same goal carrying the two numbers as its own, on this catalogue and on the
//   one main serves, the goal record's identity aside (test 3);
// - a rule without a fixed value is weighed at the pool's default step, and its benefit stays
//   unknown as for any pool without a valuation (test 4);
// - one stored goal under two catalogues that differ only in the defaults weighs by each, with
//   nothing set again (test 6);
// - a catalogue without the member keeps its golden identity (test 8).
//
// The evaluation fixture is B1's (`gate_455_b1`): judged stages of one instance consuming 30 a run
// of the capped pool `pool.neutral` (capacity 180 read from its fact, stop line 140, hard line 104,
// no regeneration, valued 1000 milli a unit) read at 150, and the lacking pool `pool.lacking`
// without a valuation, read at 10, its `target_defaults` 10 and 1000. The goal is a v3 record on
// the lacking pool, at least 40: the gap is 30 and, at the defaults, `Γ = ⌊1000·30/10⌋ = 3000`, so
// a stage producing 2 a run adds `2000·3000/(10·1000) = 600`. Names are made up; no clock is read.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{
    Initiator, RESOURCE_TARGETS_FACT_KEY, ResourceTargetsRejectionReason, RuntimeChannel,
    StandardPackProblem, StandardPackRootBytes, read_standard_pack_root,
};
use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CandidateEligibility, CappedMemory, CatalogDiagnosticCode, CatalogDocumentSource,
    CatalogSources, ChainHandoverInputs, CheckedResourceTargets, CompiledCatalog, DecisionReason,
    DispatchIntent, EvaluationFacts, EvaluationResources, EvaluationTime, FactRefreshInputs,
    FactRefreshSettings, FactValue, HostResourceSnapshot, InstanceHolds, NoProgressGuards,
    ObservedFact, OwedReruns, POOL_TARGET_DEFAULTS_SUBSYSTEM, PolicyCode, PolicyEvaluation,
    PolicyEvaluationError, PoolTargetDefaults, PoolsDocument, RemedyPlan, ResourceTargetsError,
    RunSlots, SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2, ScopeSelector,
    StandardPackBinding, StandardPackComposeInput, StandardPackComposition, StandardPackGameFiles,
    TargetSetter, TaskDecision, check_resource_targets, compile_catalog,
    compose_standard_pack_rules, evaluate_with_capped_spending, parse_resource_targets,
    project_fact_pools, targetable_resources,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// The capped pool, read from `READING_KEY`.
const POOL: &str = "pool.neutral";
const READING_KEY: &str = "resource.neutral";
/// The capacity fact of the capped pool, in its scope.
const CAPACITY_KEY: &str = "resource.neutral.capacity";
/// The lacking pool, without a valuation, read from `LACKING_KEY`.
const LACKING: &str = "pool.lacking";
const LACKING_KEY: &str = "resource.lacking";
/// A pool whose valuation gives a step of 5 and a gap weight of 700 (test 2).
const POINTS: &str = "pool.points";
const POINTS_KEY: &str = "resource.points";
/// A pool with neither a valuation nor `target_defaults` (test 2).
const BARE: &str = "pool.bare";
/// The goal on a pool: at least `AT_LEAST`.
const AT_LEAST: i64 = 40;
/// The lacking pool's `target_defaults`: the step and the importance of B1's goal.
const DEFAULTS: (u64, u64) = (10, 1000);
/// The capped pool at 150 and the lacking pool at 10, a minute before NOW.
const STAGE_READINGS: [(&str, i64); 2] = [(READING_KEY, 150), (LACKING_KEY, 10)];
/// The declared duration of every task, `E` without a snapshot.
const E: u64 = 60_000;
/// The neutral example catalog's golden identities (as in `gate_455_b1` and `gate_455_p1`).
const V1_EXAMPLE_HASH: &str =
    "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c";
const V2_EXAMPLE_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";

// Test 1, the reader. Both numbers on the example's pool compile: the compiled pool holds them, the
// pools document writes them back as written and reads back equal, and compiles again to the same
// identity (which the member is part of); the largest bounds are taken. Refused, each at its
// pointer: in a V1 document `unknown_field` at the member; one number missing,
// `missing_required_field` naming it; a scale of 0 or 2^53 and an importance of 0 or 1 000 001,
// `limit_exceeded` at the number; beside a valuation block or the pending marker, `type_mismatch` at
// the member.
#[test]
fn b3p_t1_the_reader_takes_both_numbers_and_refuses_each_fault_at_its_pointer() {
    let sources = example(SCHEDULING_SCHEMA_VERSION_V2, |declared| {
        written(declared, defaults_json(DEFAULTS));
    });
    let compiled = compile(&sources);
    let compiled_pools = &compiled.catalog().pools;
    assert_eq!(
        compiled_pools.pools[0].target_defaults,
        Some(PoolTargetDefaults {
            scale: 10,
            importance_milli: 1000
        })
    );
    let document = serde_json::to_value(compiled_pools).expect("the pools document serializes");
    assert_eq!(
        document["pools"][0]["target_defaults"],
        defaults_json(DEFAULTS)
    );
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
    let largest = example(SCHEDULING_SCHEMA_VERSION_V2, |declared| {
        written(declared, defaults_json((9_007_199_254_740_991, 1_000_000)));
    });
    assert_eq!(
        compile(&largest).catalog().pools.pools[0].target_defaults,
        Some(PoolTargetDefaults {
            scale: 9_007_199_254_740_991,
            importance_milli: 1_000_000
        })
    );

    use CatalogDiagnosticCode::{LimitExceeded, MissingRequiredField, TypeMismatch, UnknownField};
    let member = "/pools/0/target_defaults";
    let v1 = example(SCHEDULING_SCHEMA_VERSION, |declared| {
        written(declared, defaults_json(DEFAULTS));
    });
    assert!(
        refused_at(&v1, UnknownField, member),
        "a V1 document: {:?}",
        diagnostics(&v1)
    );

    for (missing, present) in [
        ("importance_milli", json!({"scale": 10})),
        ("scale", json!({"importance_milli": 1000})),
    ] {
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

    type Edit = fn(&mut Value);
    let cases: [(&str, Edit, CatalogDiagnosticCode, &str); 6] = [
        (
            "a scale of 0",
            |pool| pool["target_defaults"]["scale"] = json!(0),
            LimitExceeded,
            "/pools/0/target_defaults/scale",
        ),
        (
            "a scale of 2^53",
            |pool| pool["target_defaults"]["scale"] = json!(9_007_199_254_740_992_u64),
            LimitExceeded,
            "/pools/0/target_defaults/scale",
        ),
        (
            "an importance of 0",
            |pool| pool["target_defaults"]["importance_milli"] = json!(0),
            LimitExceeded,
            "/pools/0/target_defaults/importance_milli",
        ),
        (
            "an importance of 1 000 001",
            |pool| pool["target_defaults"]["importance_milli"] = json!(1_000_001),
            LimitExceeded,
            "/pools/0/target_defaults/importance_milli",
        ),
        (
            "beside a valuation block",
            |pool| {
                pool["valuation"] = json!({"name": "Fixture", "unit": "unit", "scale": 1,
                                           "base_weight_milli": 1000});
            },
            TypeMismatch,
            "/pools/0/target_defaults",
        ),
        (
            "beside the pending marker",
            |pool| pool["valuation"] = json!("pending"),
            TypeMismatch,
            "/pools/0/target_defaults",
        ),
    ];
    for (case, edit, code, path) in cases {
        let sources = example(SCHEDULING_SCHEMA_VERSION_V2, |declared| {
            written(declared, defaults_json(DEFAULTS));
            edit(&mut declared["pools"][0]);
        });
        assert!(
            refused_at(&sources, code, path),
            "{case}: {code:?} at {path}: {:?}",
            diagnostics(&sources)
        );
    }
}

// Test 1, the composer (ruling 6). A `schedule.json` pool that writes `target_defaults`, with
// `pool_target_defaults` in `requires`, composes: the composed pools document carries the member as
// written and the compiled pool its two numbers. Without the entry the file is refused,
// `schedule_invalid` at the pool's member. On a Runtime whose manifest does not name the subsystem
// (as no Runtime of 0.12.81 does) the file that lists it is refused at that entry
// (`schedule_requires_missing`).
#[test]
fn b3p_t1_the_composer_requires_pool_target_defaults_and_an_older_runtime_refuses_the_file() {
    let composed = compose(&schedule(&[POOL_TARGET_DEFAULTS_SUBSYSTEM]), None)
        .unwrap_or_else(|failure| panic!("the file composes: {failure}"));
    let composed_pools = serde_json::from_slice::<Value>(&composed.sources().pools.bytes)
        .expect("the composed pools document");
    assert_eq!(
        composed_pools["pools"][0]["target_defaults"],
        defaults_json(DEFAULTS),
        "{composed_pools}"
    );
    assert_eq!(
        composed.compiled().catalog().pools.pools[0].target_defaults,
        Some(PoolTargetDefaults {
            scale: 10,
            importance_milli: 1000
        })
    );

    let failure = compose(&schedule(&[]), None).expect_err("requires");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleInvalid);
    assert_eq!(issue.pointer, "/pools/0/target_defaults");

    let older = BTreeSet::from(["standard_pack_schedule".to_owned()]);
    let failure = compose(&schedule(&[POOL_TARGET_DEFAULTS_SUBSYSTEM]), Some(&older))
        .expect_err("a Runtime that does not name the subsystem");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleRequiresMissing);
    assert_eq!(issue.pointer, "/requires/0");
}

// Test 1, the schema (ruling 8). The V2 pools schema lists the member, both numbers required within
// the bounds the compiler holds them to (the goal document's), and not beside a `valuation`; the V1
// schema does not list it.
#[test]
fn b3p_t1_the_v2_pools_schema_lists_the_member_with_the_compiler_s_bounds() {
    let schema = |text: &str| -> Value { serde_json::from_str(text).expect("a schema document") };
    let v2 = schema(include_str!(
        "../../../../contracts/scheduling/v2/pools.schema.json"
    ));
    let v1 = schema(include_str!(
        "../../../../contracts/scheduling/pools.schema.json"
    ));
    assert!(
        v1["$defs"]["pool"]["properties"]
            .get("target_defaults")
            .is_none()
    );
    let pool = &v2["$defs"]["pool"];
    assert_eq!(
        pool["properties"]["target_defaults"]["$ref"],
        "#/$defs/targetDefaults"
    );
    assert_eq!(
        pool["dependentSchemas"]["target_defaults"],
        json!({"not": {"required": ["valuation"]}})
    );
    let defaults = &v2["$defs"]["targetDefaults"];
    assert_eq!(defaults["required"], json!(["scale", "importance_milli"]));
    assert_eq!(defaults["additionalProperties"], false);
    let numbers = &defaults["properties"];
    assert_eq!(numbers["scale"]["minimum"], 1);
    assert_eq!(
        numbers["scale"]["maximum"],
        crate::resource_targets::MAX_RESOURCE_TARGET_AMOUNT
    );
    assert_eq!(numbers["importance_milli"]["minimum"], 1);
    assert_eq!(
        numbers["importance_milli"]["maximum"],
        crate::MAX_RESOURCE_TARGET_MILLI
    );
}

// Test 2, resolution (ruling 3). One task produces 2 a run of the lacking pool (its defaults 10 and
// 1000), of a valued pool whose valuation gives a step of 5 and a gap weight of 700, and of a pool
// with neither. The targetable list gives the lacking pool its defaults and requires nothing, the
// valued pool its valuation's as before, and the pool with neither nothing, both required. The
// entry takes a target without numbers on the lacking pool and one with its own, and refuses one
// without numbers on the pool with neither at its `scale` (`missing_field`), as before. The
// evaluation, each pool read at 10 under a goal of at least 40 (gap 30): without numbers the lacking
// pool's target applies at step 10 and importance 1000 (`Γ` 3000); with its own 5 and 2000 at those
// (`Γ` 12 000); the valued pool's target without numbers at its valuation's 5 and 700 (`Γ` 4200).
#[test]
fn b3p_t2_a_target_takes_its_own_numbers_else_the_valuation_s_else_the_pool_s_defaults() {
    let compiled = resolution_catalog();
    let (targetable, _) =
        targetable_resources(&compiled, &bare_facts(), &instance(A, &[]), time(NOW))
            .unwrap_or_else(|error| panic!("the targetable list: {error:?}"));
    let terms = |pool: &str| {
        let row = targetable
            .iter()
            .find(|row| row.resource == pool)
            .unwrap_or_else(|| panic!("{pool} is targetable: {targetable:?}"));
        (
            row.default_scale,
            row.default_importance_milli,
            row.scale_required,
            row.importance_required,
        )
    };
    assert_eq!(terms(LACKING), (Some(10), Some(1000), false, false));
    assert_eq!(terms(POINTS), (Some(5), Some(700), false, false));
    assert_eq!(terms(BARE), (None, None, true, true));

    for (case, own) in [("without numbers", None), ("with its own", Some((5, 2000)))] {
        if let Err(error) = checked(&compiled, goal_target("goal-lacking", LACKING, own)) {
            panic!("{case}: the target is refused: {error:?}");
        }
    }
    match checked(&compiled, goal_target("goal-bare", BARE, None)) {
        Err(ResourceTargetsError::Rejected(rejection)) => assert_eq!(
            (rejection.field_path.as_str(), rejection.reason),
            (
                "/targets/0/scale",
                ResourceTargetsRejectionReason::MissingField
            )
        ),
        other => panic!("a pool with neither takes no target without numbers: {other:?}"),
    }

    let readings: [(&str, i64); 2] = [(LACKING_KEY, 10), (POINTS_KEY, 10)];
    let round = |own: Option<(u64, u64)>| {
        let goal = goal_record(
            &compiled,
            vec![
                goal_target("goal-lacking", LACKING, own),
                goal_target("goal-points", POINTS, None),
            ],
        );
        let mut world = World::new(compiled.clone(), &readings).with_goal(&goal);
        let evaluation = world.evaluate();
        reason(decision(&evaluation, "task_p"), PolicyCode::ResourceTargets)
            .detail
            .clone()
    };
    let points = "goal-points@pool.points:applied current=10 at_least=40 gap=30 step=5 \
                  importance=700 gap_weight=4200";
    assert_eq!(
        round(None),
        format!(
            "goal-lacking@pool.lacking:applied current=10 at_least=40 gap=30 step=10 \
             importance=1000 gap_weight=3000; {points}"
        )
    );
    assert_eq!(
        round(Some((5, 2000))),
        format!(
            "goal-lacking@pool.lacking:applied current=10 at_least=40 gap=30 step=5 \
             importance=2000 gap_weight=12000; {points}"
        )
    );
}

// Test 3, with B1. B1's two stages (`V` 40 000, cost 30 000; the one declared second producing 2 of
// the lacking pool a run) under a goal of at least 40 without numbers, on the catalogue whose
// lacking pool writes the defaults 10 and 1000: the decisions are byte for byte those of the same
// catalogue under the goal carrying 10 and 1000 as its own, the goal record's identity aside (the
// two records differ by the two numbers, so their `policy_sha256` does, and every `sha256:` is
// masked); the same dispatch and the same wake. That goal with its own numbers decides byte for
// byte the same on main's catalogue, whose pool writes no defaults (the case main serves). The
// producer is selected with B1's `B = 40 000 + 600 − 30 000 = 10 600`.
#[test]
fn b3p_t3_a_goal_without_numbers_decides_byte_for_byte_as_one_carrying_them() {
    let with_defaults = || stages(40_000, pools(Some(DEFAULTS)));
    let without_numbers = goal_record(&with_defaults(), vec![lacking_goal(None)]);
    let with_numbers = goal_record(&with_defaults(), vec![lacking_goal(Some(DEFAULTS))]);
    let round = |compiled: CompiledCatalog, goal: &ObservedFact| {
        let mut world = World::new(compiled, &STAGE_READINGS).with_goal(goal);
        let evaluation = world.evaluate();
        (
            serde_json::to_string(&evaluation.decisions).expect("decisions"),
            dispatched(&evaluation)
                .iter()
                .map(|task_id| (*task_id).to_owned())
                .collect::<Vec<_>>(),
            evaluation.next_wake_unix_ms,
        )
    };
    let numbered = round(with_defaults(), &with_numbers);
    assert_eq!(numbered.1, ["task_b"]);
    assert_eq!(
        round(stages(40_000, pools(None)), &with_numbers),
        numbered,
        "main's catalogue"
    );
    let unnumbered = round(with_defaults(), &without_numbers);
    assert_eq!(masked(&unnumbered.0), masked(&numbered.0));
    assert_eq!((&unnumbered.1, unnumbered.2), (&numbered.1, numbered.2));

    let mut world = World::new(with_defaults(), &STAGE_READINGS).with_goal(&without_numbers);
    let evaluation = world.evaluate();
    let winner = decision(&evaluation, "task_b");
    fixed_first(winner, 10_600, 40_000, E);
    assert_eq!(
        reason(winner, PolicyCode::ResourceTargets).detail,
        "goal-lacking@pool.lacking:applied current=10 at_least=40 gap=30 step=10 importance=1000 \
         gap_weight=3000"
    );
}

// Test 4. A judged rule without a fixed value consumes 30 of the capped pool and produces 2 of the
// lacking pool a run, under the goal without numbers: its production term is weighed at the pool's
// default step 10, `pool.lacking:r=2,per=10,base=0,gap=3000,effective=3000,term=600`. Ruling 5: the
// pool is still one without a valuation, so the rule's benefit is unknown as it is today
// (`benefit_unknown`, `valuation_missing`, the lacking pool).
#[test]
fn b3p_t4_a_rule_without_a_fixed_value_is_weighed_at_the_pool_s_default_step() {
    let compiled = catalog(
        vec![with(
            judged("task_r", 30),
            "produces",
            json!([produce(LACKING, 2)]),
        )],
        pools(Some(DEFAULTS)),
    );
    let goal = goal_record(&compiled, vec![lacking_goal(None)]);
    let mut world = World::new(compiled, &STAGE_READINGS).with_goal(&goal);
    let evaluation = world.evaluate();
    let rule = decision(&evaluation, "task_r");
    let weights = reason(rule, PolicyCode::ResourceWeights);
    assert!(
        weights.detail.ends_with(
            " term=600 items=pool.lacking:r=2,per=10,base=0,gap=3000,effective=3000,term=600"
        ),
        "{}",
        weights.detail
    );
    let unknown = &reason(rule, PolicyCode::BenefitUnknown).outcome;
    assert_eq!(
        unknown.value("benefit_gap"),
        Some(&json!("valuation_missing"))
    );
    assert_eq!(unknown.value("pool_id"), Some(&json!(LACKING)));
}

// Test 6, ruling 4 (d). The goal without numbers is stored once, under the catalogue whose lacking
// pool writes the defaults 10 and 1000: the producer's `B` is 10 600 (`Γ` 3000, its term 600). The
// next catalogue differs only in the defaults, 20 and 500: the same stored fact, set again by
// nobody, weighs by them at the next evaluation, `Γ = ⌊500·30/20⌋ = 750`, its term
// `2000·750/(20·1000) = 75`, `B` 10 075, and the producer is still selected.
#[test]
fn b3p_t6_a_goal_set_once_weighs_by_the_defaults_of_the_catalogue_in_force() {
    let first = stages(40_000, pools(Some(DEFAULTS)));
    let goal = goal_record(&first, vec![lacking_goal(None)]);
    for (case, compiled, benefit, item) in [
        (
            "the first catalogue",
            first,
            10_600,
            "goal-lacking@pool.lacking:applied current=10 at_least=40 gap=30 step=10 \
             importance=1000 gap_weight=3000",
        ),
        (
            "the next catalogue",
            stages(40_000, pools(Some((20, 500)))),
            10_075,
            "goal-lacking@pool.lacking:applied current=10 at_least=40 gap=30 step=20 \
             importance=500 gap_weight=750",
        ),
    ] {
        let mut world = World::new(compiled, &STAGE_READINGS).with_goal(&goal);
        let evaluation = world.evaluate();
        assert_eq!(
            dispatched(&evaluation),
            ["task_b"],
            "{case}: {evaluation:#?}"
        );
        let producer = decision(&evaluation, "task_b");
        fixed_first(producer, benefit, 40_000, E);
        assert_eq!(
            reason(producer, PolicyCode::ResourceTargets).detail,
            item,
            "{case}"
        );
    }
}

// Test 8, ruling 7. The neutral example catalogue, which writes no `target_defaults`, keeps its V1
// and V2 golden identities.
#[test]
fn b3p_t8_a_catalogue_without_the_member_keeps_its_golden_identity() {
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
}

// ---- The world a test drives ----

/// The catalog, the Runtime's memory of the capped episodes, the pools' readings and the
/// instance's goal record.
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    /// Each pool's reading key and reading, a minute before NOW.
    readings: Vec<(&'static str, i64)>,
    /// The instance's `session.resource_targets` record; `None` without a goal.
    goal: Option<ObservedFact>,
}

impl World {
    /// The world at NOW without a goal, the pools read as `readings` a minute before.
    fn new(compiled: CompiledCatalog, readings: &[(&'static str, i64)]) -> Self {
        Self {
            catalog: compiled,
            memory: CappedMemory::default(),
            readings: readings.to_vec(),
            goal: None,
        }
    }

    /// The same world with the goal record `goal`.
    fn with_goal(mut self, goal: &ObservedFact) -> Self {
        self.goal = Some(goal.clone());
        self
    }

    fn facts(&self) -> EvaluationFacts {
        let mut observed = vec![fact(A, CAPACITY_KEY, 180, NOW - HOUR_MS, None)];
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

    /// One round by the Runtime's entry, with its memory, every candidate admitted.
    fn evaluate(&mut self) -> PolicyEvaluation {
        let facts = self.facts();
        let resource_snapshot = resources(&self.catalog, &facts);
        evaluate_with_capped_spending::<PolicyEvaluationError>(
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
            |_, _, _| Ok(false),
            |_: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
                Ok(CandidateEligibility::Eligible)
            },
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
    }
}

/// The instance with no fact.
fn bare_facts() -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-455-b3p".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

/// A v3 target `id` on `pool`, at least `AT_LEAST`, mode `adjust`, no tasks and no lifetime; with
/// `own`, its own scale and importance.
fn goal_target(id: &str, pool: &str, own: Option<(u64, u64)>) -> Value {
    let mut target = json!({
        "id": id,
        "resource": pool,
        "condition": {"kind": "at_least", "amount": AT_LEAST},
        "apply": {"mode": "adjust", "weight": "score_stage"}
    });
    if let Some((scale, importance)) = own {
        target["scale"] = json!(scale);
        target["importance_milli"] = json!(importance);
    }
    target
}

/// The goal on the lacking pool, with `own` numbers or none.
fn lacking_goal(own: Option<(u64, u64)>) -> Value {
    goal_target("goal-lacking", LACKING, own)
}

/// The v3 document of `target` alone checked against `compiled` an hour before NOW, as the formal
/// entry checks it.
fn checked(
    compiled: &CompiledCatalog,
    target: Value,
) -> Result<CheckedResourceTargets, ResourceTargetsError> {
    let document = json!({
        "schema_version": "actingcommand.resource-targets.v3",
        "instance": A,
        "targets": [target]
    });
    let parsed = parse_resource_targets(&serde_json::to_vec(&document).expect("document bytes"))?;
    check_resource_targets(&parsed, compiled, &bare_facts(), time(NOW - HOUR_MS))
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

/// `text` with every `sha256:<hex>` written `sha256:<identity>`: the identity of the goal record a
/// decision names, which two records that differ by the two numbers do not share.
fn masked(text: &str) -> String {
    const MARK: &str = "sha256:";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(MARK) {
        let (head, tail) = rest.split_at(at + MARK.len());
        out.push_str(head);
        out.push_str("<identity>");
        let digits = tail.bytes().take_while(u8::is_ascii_hexdigit).count();
        rest = &tail[digits..];
    }
    out.push_str(rest);
    out
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

fn produce(pool: &str, amount: u64) -> Value {
    effect(pool, "produce", amount)
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

/// A judged task of the game consuming `consumed` units of the capped pool a run, due while the
/// pool holds them, of priority 100.
fn judged(id: &str, consumed: u64) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
        "priority": 100,
        "trigger": enough(consumed),
        "feedback_stop": {
            "kind": "outcome",
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [effect(POOL, "consume", consumed)],
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
        "expected_duration_ms": E,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": [],
        "benefit_judged": true
    })
}

/// A task of the game that is not judged, on an hourly clock, consuming nothing and producing
/// `produces`.
fn plain(id: &str, produces: Value) -> Value {
    let mut task = with(
        with(judged(id, 1), "consumes", json!([])),
        "produces",
        produces,
    );
    task["trigger"] = hourly();
    task.as_object_mut()
        .expect("a task object")
        .shift_remove("benefit_judged");
    task
}

fn with(mut task: Value, name: &str, value: Value) -> Value {
    task[name] = value;
    task
}

/// `task_a` and, declared after it, `task_b`: each judged, consuming 30 of the capped pool a run,
/// with the fixed value `value`; `task_b` also produces 2 of the lacking pool a run. Over
/// `pool_list`.
fn stages(value: i64, pool_list: Value) -> CompiledCatalog {
    let fixed = |task: Value| with(task, "benefit_fixed_value_milli", json!(value));
    catalog(
        vec![
            fixed(judged("task_a", 30)),
            fixed(with(
                judged("task_b", 30),
                "produces",
                json!([produce(LACKING, 2)]),
            )),
        ],
        pool_list,
    )
}

/// Test 2's catalogue: `task_p`, not judged, produces 2 a run of the lacking pool, of [`POINTS`]
/// (its valuation: step 5, base weight 100, gap weight 700) and of [`BARE`] (neither a valuation nor
/// defaults), over [`pools`] with the lacking pool's defaults.
fn resolution_catalog() -> CompiledCatalog {
    let mut pool_list = pools(Some(DEFAULTS));
    let mut points = valued(POINTS, 100, 1000);
    points["valuation"]["scale"] = json!(5);
    points["valuation"]["gap"] = json!({"rule": "shortfall_linear", "weight_milli": 700});
    let mut bare = valued(BARE, 0, 1000);
    bare.as_object_mut()
        .expect("a pool object")
        .shift_remove("valuation");
    pool_list
        .as_array_mut()
        .expect("the pools")
        .extend([points, bare]);
    catalog(
        vec![plain(
            "task_p",
            json!([produce(LACKING, 2), produce(POINTS, 2), produce(BARE, 2)]),
        )],
        pool_list,
    )
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

/// The capacity fact plus `offset`.
fn line(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

/// `{"scale": S, "importance_milli": I}`.
fn defaults_json((scale, importance_milli): (u64, u64)) -> Value {
    json!({"scale": scale, "importance_milli": importance_milli})
}

/// The capped pool, its capacity read from its fact, its stop line 40 below the cap and its hard
/// line 76 below it; and the lacking pool, [`valued`] without a valuation, with `defaults` as its
/// `target_defaults` when given (without them, main's catalogue).
fn pools(defaults: Option<(u64, u64)>) -> Value {
    let mut capped = valued(POOL, 1000, 1);
    capped["capacity"] = line(0);
    capped["stop_line"] = line(-40);
    capped["hard_line"] = line(-76);
    let mut lacking = valued(LACKING, 1000, 1000);
    lacking
        .as_object_mut()
        .expect("a pool object")
        .shift_remove("valuation");
    if let Some(defaults) = defaults {
        lacking["target_defaults"] = defaults_json(defaults);
    }
    json!([capped, lacking])
}

fn catalog(tasks: Vec<Value>, pool_list: Value) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.455-b3p",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-b3p"]
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

/// The example's pools document `declared` with its pool 0 writing `defaults` as its
/// `target_defaults`.
fn written(declared: &mut Value, defaults: Value) {
    declared["pools"][0]["target_defaults"] = defaults;
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-455-b3p/{name}.json"),
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

/// `schedule.json` with one rule on an hourly clock and the lacking pool writing the defaults (no
/// scope: the composer fills it), naming `requires`.
fn schedule(requires: &[&str]) -> Value {
    let mut rule = plain(RULE, json!([]));
    let members = rule.as_object_mut().expect("a task object");
    for member in ["scope", "entrypoint", "procedure_ref", "instance_overrides"] {
        members.shift_remove(member);
    }
    members.insert("package_id".to_owned(), json!(RULE));
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": [rule],
        "pools": [{
            "id": LACKING,
            "capacity": 1000,
            "projection": {"kind": "none"},
            "observation": {"kind": "fact", "fact_key": LACKING_KEY},
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": null,
            "target_defaults": defaults_json(DEFAULTS)
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
