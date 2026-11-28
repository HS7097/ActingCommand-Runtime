// SPDX-License-Identifier: AGPL-3.0-only

// #455 SL2 (0.12.101; register §1.547; the coordinator's rulings of 11-28, after the owner's words
// that a goal never changes the weights of the business layer and that a line on a saved currency
// has no date; coordinator work order `rd-1123/brief-455-sl2.md`; contracts/resource-targets.md,
// "A pool's line"): a stored target on a pool that writes `spend_line` is that pool's line. It
// carries `at_least` alone and needs no producer, and it never weighs what produces the pool.
// - the stored document: a line alone is taken, its condition computed, with no task of the
//   instance producing the pool; a `scale`, an `importance_milli` or `tasks` written on it is
//   `unknown_field` at that member, v2 and v3 alike; the target with its own two terms that a
//   build before this one took on such a pool is refused at its `scale` now, and taken as before on
//   the same pool without the member; a v1 target there is refused at its `scale` (test 2);
// - it never weighs: with a producer and two consumers of the pool, every evaluation with a line
//   stored (v3, v3 past its own lifetime, v2, and a target with its own two terms stored by an
//   earlier build) equals, whole, the one without a record, the needs computed from the records
//   stacked or not; the consumer without a fixed value is judged as on main, `pool.currency:-0`
//   (test 3);
// - a catalogue without `spend_line` and its stored targets read, list and judge as on main: the
//   evaluation's reading of the records is the view's, the targetable list compared whole, and the
//   goal with its own terms on the pool without the member weighs its producer as before (test 6);
// - a reader that knows no line, as a build before this one is, reads the line as a target on a
//   pool with neither a valuation nor `target_defaults`: it weighs nothing and says
//   `valuation_missing` on its producer, or `no_producing_task` on every candidate without one,
//   and the record's other goal still applies (test 7).
//
// The fixture: one instance, the window open all day, judged tasks on an hourly clock; the pool
// whose spending a line governs (`pool.currency`, no valuation, capacity 100 000) read at 500 and a
// valued pool (`pool.neutral`, 1000 milli a unit at the step 1, gap weight 100, capacity 10 000)
// read at 5000, both a minute before NOW. Names are made up; no clock is read.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{
    Initiator, RESOURCE_TARGETS_FACT_KEY, ResourceTargetCondition, ResourceTargetConditionState,
    ResourceTargetObservation, ResourceTargetsRejection, ResourceTargetsRejectionReason,
    RuntimeChannel, TargetableResource,
};
use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::resource_targets::{instance_target_policies, weighed_target_policies};
use crate::{
    CandidateEligibility, CappedMemory, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CheckedResourceTargets, CompiledCatalog, DecisionReason, DispatchIntent, EvaluationFacts,
    EvaluationResources, EvaluationTime, FactRefreshInputs, FactRefreshSettings, FactValue,
    HostResourceSnapshot, InstanceHolds, InstancePoolLines, InstanceRecordNeeds, LeftOutFacts,
    NoProgressGuards, ObservedFact, OwedReruns, PolicyCode, PolicyEvaluation,
    PolicyEvaluationError, PoolNeed, PoolNeedKind, RecordNeeds, RemedyPlan, ResourceTargetsError,
    RunSlots, SCHEDULING_SCHEMA_VERSION_V2, ScopeSelector, TargetSetter, TaskDecision,
    check_resource_targets, compile_catalog, evaluate_with_left_out_facts, parse_resource_targets,
    project_fact_pools, targetable_resources,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// The pool whose spending a line governs (when the catalogue writes the member), without a
/// valuation, read from `CURRENCY_KEY`.
const CURRENCY: &str = "pool.currency";
const CURRENCY_KEY: &str = "resource.currency";
/// A valued pool, 1000 milli a unit at the step 1 with the gap weight 100, read from `VALUED_KEY`.
const VALUED: &str = "pool.neutral";
const VALUED_KEY: &str = "resource.neutral";
/// SL1's three numbers: `price_milli`, `scale`, `income_floor_milli_per_day`.
const LINE: (u64, u64, u64) = (33_333, 6000, 71_429);
/// The declared duration of every task, `E` without a snapshot.
const E: u64 = 60_000;
/// The sha256 the fixture's needs name as their tables'.
const SHA: &str = "fixture-tables-sl2";
type Reason = ResourceTargetsRejectionReason;

// Test 2, the stored document (ruling 1). On the catalogue whose currency writes `spend_line` and
// no task of which produces it: the line `{at_least: 800}` alone is taken in a v3 and in a v2
// document, its condition computed from the reading (500, the gap 300); with a `scale`, an
// `importance_milli` or `tasks` written on it, each is `unknown_field` at that member, v3 and v2
// alike. With a task producing the pool: the target with its own `scale` and `importance_milli`,
// which the builds since SL1 took (they read no `spend_line` in this check), is `unknown_field` at
// its `scale`, changed on purpose; on the same catalogue without the member it is taken, and the
// line alone is `missing_field` at its `scale`, both as on main. A v1 target on the pool, which
// always writes its own terms and tasks, is `unknown_field` at its `scale`.
#[test]
fn sl2_t2_a_stored_line_carries_at_least_alone_and_needs_no_producer() {
    let facts = readings();
    let unproduced = catalog(tasks(false), pools(true));
    for (label, document) in [
        ("v3", v3(vec![line_target("line", 800)])),
        ("v2", v2(vec![line_target("line", 800)])),
    ] {
        let accepted = checked(&unproduced, &document, &facts)
            .unwrap_or_else(|error| panic!("{label}: the line is taken: {error:?}"));
        assert_eq!(
            accepted.conditions,
            [ResourceTargetCondition {
                target_id: "line".to_owned(),
                resource: CURRENCY.to_owned(),
                fact_key: CURRENCY_KEY.to_owned(),
                state: ResourceTargetConditionState::Computed {
                    current: 500,
                    observed_at_unix_ms: NOW - MINUTE_MS,
                    gap: 300,
                },
            }],
            "{label}"
        );
    }
    for (member, value) in [
        ("scale", json!(10)),
        ("importance_milli", json!(1000)),
        ("tasks", json!(["task_spend"])),
    ] {
        let mut target = line_target("line", 800);
        target[member] = value;
        for (label, document) in [("v3", v3(vec![target.clone()])), ("v2", v2(vec![target]))] {
            assert_eq!(
                refusal(checked(&unproduced, &document, &facts)),
                (format!("/targets/0/{member}"), Reason::UnknownField),
                "{label} {member}"
            );
        }
    }

    let produced = catalog(tasks(true), pools(true));
    let bare = catalog(tasks(true), pools(false));
    let own = v3(vec![own_terms("own", 800)]);
    assert_eq!(
        refusal(checked(&produced, &own, &facts)),
        ("/targets/0/scale".to_owned(), Reason::UnknownField)
    );
    assert!(
        checked(&bare, &own, &facts).is_ok(),
        "the pool without the member"
    );
    assert_eq!(
        refusal(checked(&bare, &v3(vec![line_target("line", 800)]), &facts)),
        ("/targets/0/scale".to_owned(), Reason::MissingField)
    );
    assert_eq!(
        refusal(checked(&produced, &v1_own_terms(), &facts)),
        ("/targets/0/scale".to_owned(), Reason::UnknownField)
    );
}

// Test 3, it never weighs (rulings 3 and 4). The catalogue: `task_earn` (`V` 800) produces 50 of
// the currency, `task_spend` (`V` 50 000) consumes 120 of it and reserves 10, `task_lone` (no fixed
// value) consumes 120 of it alone, `task_neutral` produces 2 of the valued pool. Without a record:
// `task_spend` heads the round with `run_cap` 4 (⌊(500 − 10) / 120⌋), `task_earn` is `B = 800`,
// `task_lone` is `policy_benefit_nonpositive` with the item `pool.currency:-0` (0.12.99's
// judgement), and no decision carries a `resource_targets` reason. Each of these records equals
// that evaluation whole: the line of 800 in a v3 record, the same with its own lifetime past
// before NOW, the line in a v2 document of a day's lifetime, and a target with its own `scale` 10
// and `importance_milli` 1000 on the pool, as an earlier build stored it (read as the line now).
// With the needs computed from the records (30 covered on the currency, 3 landing on the valued
// pool) the line record and no record evaluate alike too.
#[test]
fn sl2_t3_a_line_weighs_nothing_and_its_consumers_are_judged_as_before() {
    let lined = || catalog(tasks(true), pools(true));
    let without = World::new(lined()).evaluate();
    assert_eq!(
        dispatched(&without).first(),
        Some(&"task_spend"),
        "{without:#?}"
    );
    let spend = decision(&without, "task_spend");
    assert_eq!(
        reason(spend, PolicyCode::PolicyDispatchParameters)
            .outcome
            .value("run_cap"),
        Some(&json!(4)),
        "{spend:#?}"
    );
    assert_eq!(
        reason(
            decision(&without, "task_earn"),
            PolicyCode::BenefitFixedValue
        )
        .outcome
        .value("benefit_milli"),
        Some(&json!(800))
    );
    assert_eq!(
        reason(
            decision(&without, "task_lone"),
            PolicyCode::PolicyBenefitNonpositive
        )
        .detail,
        "benefit_milli=0 expected_duration_ms=60000 items=pool.currency:-0"
    );
    assert!(
        without
            .decisions
            .iter()
            .flat_map(|entry| &entry.reasons)
            .all(|given| !given.outcome.is(PolicyCode::ResourceTargets)),
        "{without:#?}"
    );

    let mut lapsing = line_target("lapsed", 800);
    lapsing["valid_until_unix_ms"] = json!(NOW - MINUTE_MS);
    let earlier = goal_record(
        &catalog(tasks(true), pools(false)),
        &v3(vec![own_terms("own", 800)]),
    );
    for (label, record) in [
        (
            "v3",
            goal_record(&lined(), &v3(vec![line_target("line", 800)])),
        ),
        ("v3 lapsed", goal_record(&lined(), &v3(vec![lapsing]))),
        (
            "v2",
            goal_record(&lined(), &v2(vec![line_target("line", 800)])),
        ),
        ("own terms stored earlier", earlier),
    ] {
        assert_eq!(
            World::new(lined()).with_record(record).evaluate(),
            without,
            "{label}"
        );
    }

    let line = goal_record(&lined(), &v3(vec![line_target("line", 800)]));
    assert_eq!(
        World::new(lined())
            .with_record(line)
            .with_needs()
            .evaluate(),
        World::new(lined()).with_needs().evaluate()
    );
}

// Test 6, byte for byte (ruling 6). On the catalogue without `spend_line`, a stored v3 record of a
// goal on the valued pool and a target with its own `scale` 10 and `importance_milli` 1000 on the
// currency: the evaluation's reading of the records is the view's, whole (main's evaluation read
// the view's); the targetable list is, whole, main's (each pool with its producers and reading, the
// valued pool its valuation's 1 and 100, the currency no default and both required); and the
// target with its own terms weighs its producer as on main: `Γ = ⌊1000·300/10⌋ = 30 000`, the goal
// term `50 000·30 000/(10·1000) = 150 000`, `B = 800 + 150 000`.
#[test]
fn sl2_t6_a_catalogue_without_spend_line_and_its_stored_targets_read_as_on_main() {
    let bare = || catalog(tasks(true), pools(false));
    let record = goal_record(
        &bare(),
        &v3(vec![
            own_terms("own", 800),
            goal_target("points", VALUED, 6000),
        ]),
    );
    let world = World::new(bare()).with_record(record);
    let facts = world.facts();
    assert_eq!(
        weighed_target_policies(&world.catalog, &facts, time(NOW)),
        instance_target_policies(&facts, time(NOW))
    );
    let (targetable, not_targetable) =
        targetable_resources(&world.catalog, &facts, &facts.instances[0], time(NOW))
            .unwrap_or_else(|error| panic!("the targetable list: {error:?}"));
    let observed = |current| ResourceTargetObservation {
        current: Some(current),
        observed_at_unix_ms: Some(NOW - MINUTE_MS),
        pending: None,
    };
    assert_eq!(
        targetable,
        [
            TargetableResource {
                resource: CURRENCY.to_owned(),
                fact_key: CURRENCY_KEY.to_owned(),
                producing_tasks: vec!["task_earn".to_owned()],
                default_scale: None,
                default_importance_milli: None,
                scale_required: true,
                importance_required: true,
                observation: observed(500),
            },
            TargetableResource {
                resource: VALUED.to_owned(),
                fact_key: VALUED_KEY.to_owned(),
                producing_tasks: vec!["task_neutral".to_owned()],
                default_scale: Some(1),
                default_importance_milli: Some(100),
                scale_required: false,
                importance_required: false,
                observation: observed(5000),
            },
        ]
    );
    assert!(not_targetable.is_empty(), "{not_targetable:?}");
    let evaluation = world.evaluate();
    assert_eq!(
        reason(
            decision(&evaluation, "task_earn"),
            PolicyCode::BenefitFixedValue
        )
        .outcome
        .value("benefit_milli"),
        Some(&json!(150_800)),
        "{evaluation:#?}"
    );
}

// Test 7, what a reader that knows no line does (ruling 7). The record the set writes, the line of
// 800 beside a goal of 6000 on the valued pool, taken on the catalogue with the member, is read on
// the same catalogue without it: the older reader's condition, a target with no terms of its own on
// a pool with neither a valuation nor `target_defaults` (no reader before this build reads
// `spend_line` in the goal's weighing). It evaluates; the line weighs nothing (`task_earn` still
// `B = 800`) and says so on its producer (`unresolved why=valuation_missing gap_weight=0`); the
// valued pool's goal still applies on its producer. With no task producing the currency every
// candidate says `resource_target_tasks_unevaluable` with `line@pool.currency:no_producing_task`,
// and the other goal still applies.
#[test]
fn sl2_t7_a_reader_that_knows_no_line_ignores_it_and_keeps_the_other_goals() {
    let record = goal_record(
        &catalog(tasks(true), pools(true)),
        &v3(vec![
            line_target("line", 800),
            goal_target("points", VALUED, 6000),
        ]),
    );
    let older = World::new(catalog(tasks(true), pools(false)))
        .with_record(record.clone())
        .evaluate();
    let earn = decision(&older, "task_earn");
    assert_eq!(
        reason(earn, PolicyCode::ResourceTargets).detail,
        "line@pool.currency:unresolved why=valuation_missing gap_weight=0"
    );
    assert_eq!(
        reason(earn, PolicyCode::BenefitFixedValue)
            .outcome
            .value("benefit_milli"),
        Some(&json!(800))
    );
    let applied = "points@pool.neutral:applied current=5000 at_least=6000 gap=1000";
    let neutral = reason(
        decision(&older, "task_neutral"),
        PolicyCode::ResourceTargets,
    );
    assert!(neutral.detail.starts_with(applied), "{}", neutral.detail);

    let unproduced = World::new(catalog(tasks(false), pools(false)))
        .with_record(record)
        .evaluate();
    let neutral = decision(&unproduced, "task_neutral");
    assert_eq!(
        reason(neutral, PolicyCode::ResourceTargetTasksUnevaluable).detail,
        "line@pool.currency:no_producing_task"
    );
    let targets = reason(neutral, PolicyCode::ResourceTargets);
    assert!(targets.detail.starts_with(applied), "{}", targets.detail);
}

// ---- The world a test drives ----

/// The catalog, the instance's stored goal record and the needs its records compute.
struct World {
    catalog: CompiledCatalog,
    /// The instance's `session.resource_targets` record; `None` without one.
    record: Option<ObservedFact>,
    /// The needs the host computed from the instance's records; `None` without records.
    needs: Option<RecordNeeds>,
}

impl World {
    fn new(compiled: CompiledCatalog) -> Self {
        Self {
            catalog: compiled,
            record: None,
            needs: None,
        }
    }

    /// The same world with the record `record`.
    fn with_record(mut self, record: ObservedFact) -> Self {
        self.record = Some(record);
        self
    }

    /// The same world with records whose needs are 30 of the currency, covered by its holding,
    /// and 3 landing on the valued pool, of the tables [`SHA`], set an hour before NOW.
    fn with_needs(mut self) -> Self {
        let mut computed = RecordNeeds::default();
        computed.give(
            A,
            InstanceRecordNeeds::new(
                Some(SHA.to_owned()),
                NOW - HOUR_MS,
                BTreeMap::from([
                    (
                        CURRENCY.to_owned(),
                        PoolNeed {
                            need: 30,
                            kind: PoolNeedKind::Covered,
                        },
                    ),
                    (
                        VALUED.to_owned(),
                        PoolNeed {
                            need: 3,
                            kind: PoolNeedKind::Lands,
                        },
                    ),
                ]),
                Vec::new(),
                None,
            ),
        );
        self.needs = Some(computed);
        self
    }

    /// The pools' readings a minute before NOW and the record.
    fn facts(&self) -> EvaluationFacts {
        let mut facts = readings();
        facts.facts.extend(self.record.clone());
        facts
    }

    /// One round at NOW by the Runtime's entry, with a fresh memory, the needs beside the left-out
    /// facts, every candidate admitted.
    fn evaluate(&self) -> PolicyEvaluation {
        let facts = self.facts();
        let resource_snapshot = resources(&self.catalog, &facts);
        let mut left_out = LeftOutFacts::default();
        if let Some(computed) = &self.needs {
            left_out.give_record_needs(computed.clone());
        }
        let mut memory = CappedMemory::default();
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
            &mut memory,
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
        fact_snapshot_id: "snapshot:fixture-455-sl2".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

/// The instance with the currency read at 500 and the valued pool at 5000, a minute before NOW.
fn readings() -> EvaluationFacts {
    EvaluationFacts {
        facts: vec![
            fact(A, CURRENCY_KEY, 500, NOW - MINUTE_MS, None),
            fact(A, VALUED_KEY, 5000, NOW - MINUTE_MS, None),
        ],
        ..bare_facts()
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

// ---- The goal documents ----

/// A target `id` on `pool`, at least `at_least`, mode `adjust`, without numbers or tasks of its
/// own and without a lifetime.
fn goal_target(id: &str, pool: &str, at_least: u64) -> Value {
    json!({
        "id": id,
        "resource": pool,
        "condition": {"kind": "at_least", "amount": at_least},
        "apply": {"mode": "adjust", "weight": "score_stage"}
    })
}

/// The line `id` of `at_least` on the currency: a target that carries nothing but its amount.
fn line_target(id: &str, at_least: u64) -> Value {
    goal_target(id, CURRENCY, at_least)
}

/// A target `id` on the currency of `at_least` with its own `scale` 10 and `importance_milli`
/// 1000.
fn own_terms(id: &str, at_least: u64) -> Value {
    let mut target = line_target(id, at_least);
    target["scale"] = json!(10);
    target["importance_milli"] = json!(1000);
    target
}

/// The v3 document of `targets` for the instance.
fn v3(targets: Vec<Value>) -> Value {
    json!({
        "schema_version": "actingcommand.resource-targets.v3",
        "instance": A,
        "targets": targets
    })
}

/// The v2 document of `targets` for the instance, valid for a day after NOW.
fn v2(targets: Vec<Value>) -> Value {
    json!({
        "schema_version": "actingcommand.resource-targets.v2",
        "instance": A,
        "valid_until_unix_ms": NOW + DAY_MS,
        "targets": targets
    })
}

/// A v1 document of one target on the currency with its own terms, naming `task_earn`.
fn v1_own_terms() -> Value {
    json!({
        "schema_version": "actingcommand.resource-targets.v1",
        "instance": A,
        "valid_until_unix_ms": NOW + DAY_MS,
        "targets": [{
            "id": "old",
            "resource": CURRENCY,
            "condition": {"kind": "at_least", "amount": 800},
            "scale": 10,
            "importance_milli": 1000,
            "rule": "shortfall_linear",
            "apply": {"mode": "adjust", "weight": "score_stage"},
            "tasks": ["task_earn"]
        }]
    })
}

/// The entry's check of `document` against `compiled` and `facts` at NOW.
fn checked(
    compiled: &CompiledCatalog,
    document: &Value,
    facts: &EvaluationFacts,
) -> Result<CheckedResourceTargets, ResourceTargetsError> {
    let parsed = parse_resource_targets(&serde_json::to_vec(document).expect("document bytes"))
        .unwrap_or_else(|error| panic!("the document parses: {error:?}"));
    check_resource_targets(&parsed, compiled, facts, time(NOW))
}

/// The pointer and the reason of a refused check.
fn refusal(
    result: Result<CheckedResourceTargets, ResourceTargetsError>,
) -> (String, ResourceTargetsRejectionReason) {
    match result {
        Err(ResourceTargetsError::Rejected(rejection)) => {
            let ResourceTargetsRejection {
                field_path,
                reason: why,
                ..
            } = *rejection;
            (field_path, why)
        }
        other => panic!("a refusal: {other:?}"),
    }
}

/// The record `document` writes, checked against `compiled` and stored as the formal entry stores
/// it for a Lab connection, an hour before NOW.
fn goal_record(compiled: &CompiledCatalog, document: &Value) -> ObservedFact {
    let parsed = parse_resource_targets(&serde_json::to_vec(document).expect("document bytes"))
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

/// A judged task of the game on an hourly clock with `consumes` and `produces`, of priority 100.
fn judged(id: &str, consumes: Value, produces: Value) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
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

/// The tasks: `task_earn` (when `producer`) of `V` 800 producing 50 of the currency;
/// `task_spend` of `V` 50 000 consuming 120 of it and reserving 10; `task_lone`, no fixed value,
/// consuming 120 of it alone; `task_neutral` producing 2 of the valued pool.
fn tasks(producer: bool) -> Vec<Value> {
    let mut spend = fixed(
        judged("task_spend", json!([consume(CURRENCY, 120)]), json!([])),
        50_000,
    );
    spend["reserves"] = json!([{"pool_id": CURRENCY, "at_least": 10}]);
    let mut listed = Vec::new();
    if producer {
        listed.push(fixed(
            judged("task_earn", json!([]), json!([produce(CURRENCY, 50)])),
            800,
        ));
    }
    listed.extend([
        spend,
        judged("task_lone", json!([consume(CURRENCY, 120)]), json!([])),
        judged("task_neutral", json!([]), json!([produce(VALUED, 2)])),
    ]);
    listed
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

/// The currency, writing [`LINE`] as its `spend_line` when `lined` (without it, a pool with
/// neither a valuation nor `target_defaults`), and the valued pool.
fn pools(lined: bool) -> Value {
    let mut currency = declared_pool(CURRENCY, 100_000);
    if lined {
        let (price_milli, scale, floor) = LINE;
        currency["spend_line"] = json!({"price_milli": price_milli, "scale": scale,
                                        "income_floor_milli_per_day": floor});
    }
    let mut valued = declared_pool(VALUED, 10_000);
    valued["valuation"] = json!({"name": "Fixture", "unit": "unit", "scale": 1,
                                 "base_weight_milli": 1000,
                                 "gap": {"rule": "shortfall_linear", "weight_milli": 100}});
    json!([currency, valued])
}

fn catalog(task_list: Vec<Value>, pool_list: Value) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.455-sl2",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-sl2"]
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
    let source = |name: &str, declared: &Value| {
        CatalogDocumentSource::new(
            format!("memory://fixture-455-sl2/{name}.json"),
            serde_json::to_vec(declared).expect("fixture document bytes"),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(task_list))),
        pools: source("pools", &document("pools", pool_list)),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the catalog compiles: {:?}", failure.diagnostics()))
}
