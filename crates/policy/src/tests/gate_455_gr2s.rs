// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 G-R2s gate tests, policy half (0.12.60; register §1.365; coordinator work order
// `rd-1114/brief-455-gr2s-irreversible-line.md`, the coordinator's two rulings of 11-14 and its
// fix round 1; contracts/resource-targets.md, "A task that can do something irreversible";
// contracts/standard-pack-schedule.md, "The irreversible mark"): a goal never raises the weight
// of a task whose package can do something irreversible. The composer marks such a task
// (`irreversible_reach`) by the whole game root's declarations and every rule that follows it by
// a chain link; the evaluator weighs a marked candidate exactly as on an instance without a target
// record, under a v1, a v2 and a v3 record alike, and changes no other candidate's number.
//
// The evaluation fixture is gate_ss1's (tasks on one instance, due at NOW; pool A: step 10, base
// 500, gap weight 100; pool B: step 1, base 7, no gap), and gate_ss6's for the run cap; the
// composition fixture is neutral rules in catalog-a's task shape. Every expected value comes from
// the contract.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    BundleIndex, GoalReferences, GoalRule, Initiator, IrreversiblePackages,
    RESOURCE_TARGETS_FACT_KEY, RuntimeChannel, StandardPackProblem, StandardPackRootBytes,
    check_standard_pack_goal_references, parse_standard_pack_goals, parse_standard_pack_schedule,
    read_standard_pack_root,
};
use serde_json::{Value, json};

use super::gate_ss1::{
    A, POOL_A, POOL_A_KEY, POOL_B_KEY, TASK_A, TASK_B, V1, V2, catalog, decision, inputs, reading,
    round, task, task_a, task_b, term, time,
};
use super::gate_ss6 as ss6;
use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::{
    CompiledCatalog, DecisionReason, EvaluationResources, FactValue, ForwardProjectionConfig,
    HostResourceSnapshot, IRREVERSIBLE_REACH_MEMBER, ObservedFact, PolicyCode, PolicyEvaluation,
    ScopeSelector, StandardPackBinding, StandardPackComposeInput, StandardPackComposition,
    StandardPackGameFiles, TargetSetter, TaskDecision, check_resource_targets,
    compose_standard_pack_rules, parse_resource_targets, project_fact_pools, project_forward,
    standard_pack_rule_followers,
};

const V3: &str = "actingcommand.resource-targets.v3";
/// When every record of these tests is checked and applied.
const CHECKED_AT: u64 = NOW - HOUR_MS;

// ---- The evaluation fixture.

/// `task` carrying the composer's irreversible mark.
fn marked(mut task: Value) -> Value {
    task[IRREVERSIBLE_REACH_MEMBER] = json!(true);
    task
}

/// `task` judged by its net benefit (SS-2b), so its benefit `B` is computed and shown.
fn judged(mut task: Value) -> Value {
    task["benefit_judged"] = json!(true);
    task
}

/// The v1 document of one target `goal-a` on pool A naming `tasks`, in `mode`.
fn v1_document(tasks: &[&str], mode: &str) -> Value {
    json!({
        "schema_version": V1,
        "instance": A,
        "valid_until_unix_ms": NOW + DAY_MS,
        "targets": [{
            "id": "goal-a",
            "resource": POOL_A,
            "condition": {"kind": "at_least", "amount": 100},
            "scale": 10,
            "importance_milli": 1000,
            "rule": "shortfall_linear",
            "apply": {"mode": mode, "weight": "score_stage"},
            "tasks": tasks
        }]
    })
}

/// The v2 target `goal-a` on pool A, at least 100, step and importance the pool's.
fn goal_a() -> Value {
    json!({
        "id": "goal-a",
        "resource": POOL_A,
        "condition": {"kind": "at_least", "amount": 100},
        "apply": {"mode": "adjust", "weight": "score_stage"}
    })
}

/// The v2 document of `targets`, lasting a day.
fn v2_document(targets: Vec<Value>) -> Value {
    json!({
        "schema_version": V2,
        "instance": A,
        "valid_until_unix_ms": NOW + DAY_MS,
        "targets": targets
    })
}

/// The v3 document of `targets`, each lasting a day on its own.
fn v3_document(targets: Vec<Value>) -> Value {
    let targets = targets
        .into_iter()
        .map(|mut target| {
            target["valid_until_unix_ms"] = json!(NOW + DAY_MS);
            target
        })
        .collect::<Vec<_>>();
    json!({"schema_version": V3, "instance": A, "targets": targets})
}

/// Instance A's `session.resource_targets` record of `document`, checked against `catalog` and
/// applied over no record at CHECKED_AT by a Lab connection, as `ApplyResourceTargets` writes it.
fn record(catalog: &CompiledCatalog, document: &Value) -> ObservedFact {
    let parsed = parse_resource_targets(&serde_json::to_vec(document).expect("document bytes"))
        .unwrap_or_else(|error| panic!("the document parses: {error:?}"));
    let composed = check_resource_targets(
        &parsed,
        catalog,
        &inputs(Vec::new(), Vec::new()),
        time(CHECKED_AT),
    )
    .unwrap_or_else(|error| panic!("the document is accepted: {error:?}"))
    .compose(
        None,
        Some(TargetSetter {
            set_unix_ms: CHECKED_AT,
            channel: RuntimeChannel::Lab,
            initiator: Some(Initiator::LabCli),
        }),
    )
    .unwrap_or_else(|error| panic!("the document composes: {error:?}"))
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
        observed_at_unix_ms: CHECKED_AT,
        expires_at_unix_ms: composed.expires_at_unix_ms,
        confidence_milli: 1000,
    }
}

/// The readings of both pools: pool A at 50 (goal-a's gap 50), pool B at 10.
fn readings() -> Vec<ObservedFact> {
    vec![reading(POOL_A_KEY, 50), reading(POOL_B_KEY, 10)]
}

/// One round over the readings and `record`, when given.
fn evaluate_with(catalog: &CompiledCatalog, record: Option<ObservedFact>) -> PolicyEvaluation {
    let mut facts = readings();
    facts.extend(record);
    round(catalog, facts, Vec::new())
}

/// Whether `reason` is the one new reason of a candidate its record is left out for.
fn left_out(reason: &DecisionReason) -> bool {
    reason
        .outcome
        .is(PolicyCode::ResourceTargetPackageIrreversible)
}

/// `decision` without its one new reason, which it carries exactly once.
fn without_the_new_reason(decision: &TaskDecision) -> TaskDecision {
    let count = decision
        .reasons
        .iter()
        .filter(|reason| left_out(reason))
        .count();
    assert_eq!(count, 1, "the new reason once: {decision:#?}");
    let mut decision = decision.clone();
    decision.reasons.retain(|reason| !left_out(reason));
    decision
}

/// `reasons` without the one new reason and without the dispatch's `decision_record`, which they
/// carry exactly once, and that record's detail split into its `targets=` field and the rest.
fn without_the_decision_record(
    reasons: &[DecisionReason],
) -> (Vec<DecisionReason>, (String, String)) {
    let records = reasons
        .iter()
        .filter(|reason| reason.outcome.is(PolicyCode::DecisionRecord))
        .collect::<Vec<_>>();
    let [record] = records.as_slice() else {
        panic!("one decision record: {reasons:#?}");
    };
    let (targets, rest) = record
        .detail
        .split_once(" related=")
        .unwrap_or_else(|| panic!("a targets field: {record:?}"));
    let kept = reasons
        .iter()
        .filter(|reason| !reason.outcome.is(PolicyCode::DecisionRecord) && !left_out(reason))
        .cloned()
        .collect();
    (kept, (targets.to_owned(), rest.to_owned()))
}

/// The reasons of the round's dispatch chain of task A on instance A, as
/// [`without_the_decision_record`] gives them.
fn dispatch_chain(evaluation: &PolicyEvaluation) -> (Vec<DecisionReason>, (String, String)) {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == TASK_A && intent.instance_id == A)
        .unwrap_or_else(|| panic!("task A is dispatched: {evaluation:#?}"));
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .unwrap_or_else(|| panic!("the dispatch's chain: {evaluation:#?}"));
    without_the_decision_record(&chain.reasons)
}

/// Task A's decision without the one new reason (which it carries once) and without its
/// dispatch's `decision_record`, with that record's detail split as
/// [`without_the_decision_record`] splits it.
fn decision_apart(decision: &TaskDecision) -> (TaskDecision, (String, String)) {
    let mut decision = without_the_new_reason(decision);
    let (reasons, record) = without_the_decision_record(&decision.reasons);
    decision.reasons = reasons;
    (decision, record)
}

/// The reasons of the score stage, which carry a candidate's own numbers: its score, its target
/// terms, its override and its resource weights.
const SCORE_STAGE: [PolicyCode; 10] = [
    PolicyCode::ScoreComputed,
    PolicyCode::ResourceTargets,
    PolicyCode::ResourceTargetApplied,
    PolicyCode::ResourceTargetOverride,
    PolicyCode::ResourceTargetSatisfied,
    PolicyCode::ResourceTargetPending,
    PolicyCode::ResourceTargetUnmapped,
    PolicyCode::ResourceTargetTasksUnevaluable,
    PolicyCode::ResourceTargetPolicyExpired,
    PolicyCode::ResourceWeights,
];

/// What a candidate's numbers are: its rank and its score-stage reasons, in order.
fn numbers(decision: &TaskDecision) -> (Option<crate::TaskRank>, Vec<DecisionReason>) {
    (
        decision.rank.clone(),
        decision
            .reasons
            .iter()
            .filter(|reason| SCORE_STAGE.iter().any(|code| reason.outcome.is(*code)))
            .cloned()
            .collect(),
    )
}

fn has(decision: &TaskDecision, code: PolicyCode) -> bool {
    decision
        .reasons
        .iter()
        .any(|reason| reason.outcome.is(code))
}

fn effective(decision: &TaskDecision) -> i64 {
    decision
        .rank
        .as_ref()
        .map(|rank| rank.effective_milli)
        .unwrap_or_else(|| panic!("a rank on {decision:#?}"))
}

// Test 1 (work order, tests 1): a marked candidate under a v1, a v2 and a v3 record equals the same
// candidate on an instance without a record. Task A, alone, judged and marked, produces 20 of pool
// A; each record has goal-a on pool A at a gap of 50 (the v1 record names task A). Its whole
// decision (state, rank with its effective score, every reason with its values and detail, the
// benefit its `rank_breakdown` names, its dispatch's scheduling values and its own wake) equals the
// one without a record, but for the one new reason, which it carries once; so does its dispatch's
// reason chain, with one stated exception (fix round 1, point 3): the `decision_record` the
// decision and the chain carry names the instance's targets as they are, its record
// (`targets=active…`, not `targets=none`), and only that field of it differs. Without the mark the
// v2 and v3 records do weigh it (term 2000, not 1000), so each record is one a goal would apply.
#[test]
fn gr2s_t1_a_marked_candidate_under_each_record_equals_it_without_a_record() {
    let catalog = catalog(&[judged(marked(task_a()))]);
    let unmarked = catalog_of_unmarked();
    let without = evaluate_with(&catalog, None);
    let base = decision(&without, TASK_A).clone();
    assert!(
        !has(&base, PolicyCode::ResourceTargetPackageIrreversible),
        "no record, no new reason: {base:#?}"
    );
    assert_eq!(term(&without, TASK_A), 1000, "the base term alone");
    let (base_chain, (base_targets, base_rest)) = dispatch_chain(&without);
    assert_eq!(base_targets, "targets=none", "no record");
    let mut base = base;
    let (reasons, base_record) = without_the_decision_record(&base.reasons);
    base.reasons = reasons;
    assert_eq!(
        base_record,
        (base_targets.clone(), base_rest.clone()),
        "the decision names the dispatch's record"
    );
    for (label, document, unmarked_term) in [
        ("v1", v1_document(&[TASK_A], "adjust"), None),
        ("v2", v2_document(vec![goal_a()]), Some(2000)),
        ("v3", v3_document(vec![goal_a()]), Some(2000)),
    ] {
        let with = evaluate_with(&catalog, Some(record(&catalog, &document)));
        let (marked_decision, decision_record) = decision_apart(decision(&with, TASK_A));
        assert_eq!(
            marked_decision, base,
            "{label}: the marked candidate is the one without a record"
        );
        let (chain, (targets, rest)) = dispatch_chain(&with);
        assert_eq!(
            decision_record,
            (targets.clone(), rest.clone()),
            "{label}: the decision names the dispatch's record"
        );
        assert_eq!(chain, base_chain, "{label}: its dispatch's reasons");
        assert!(
            targets.starts_with("targets=active") && targets != base_targets,
            "{label}: the decision record names the instance's record: {targets}"
        );
        assert_eq!(rest, base_rest, "{label}: the rest of the decision record");
        if let Some(unmarked_term) = unmarked_term {
            let weighed = evaluate_with(&unmarked, Some(record(&unmarked, &document)));
            assert_eq!(
                term(&weighed, TASK_A),
                unmarked_term,
                "{label}: the goal weighs"
            );
        }
    }
    // The v1 record weighs the unmarked task with its target score (`target=` in its detail).
    let weighed = evaluate_with(
        &unmarked,
        Some(record(&unmarked, &v1_document(&[TASK_A], "adjust"))),
    );
    assert!(
        has(
            decision(&weighed, TASK_A),
            PolicyCode::ResourceTargetApplied
        ),
        "{weighed:#?}"
    );
}

/// Test 1's catalog without the mark.
fn catalog_of_unmarked() -> CompiledCatalog {
    catalog(&[judged(task_a())])
}

// Test 2 (work order, tests 2): every unmarked candidate of the same evaluation equals what it is
// without the mark (the code of before reads no mark). Task A is marked, task B is not; under each
// record (the v1 record names both, so its shared best useful contribution keeps task A's yield)
// task B's rank and score-stage reasons equal those of the same evaluation of the catalog without
// the mark, while task A's do not (it lost the goal's part).
#[test]
fn gr2s_t2_every_unmarked_candidate_keeps_its_numbers() {
    let with_mark = catalog(&[marked(task_a()), task_b()]);
    let without_mark = catalog(&[task_a(), task_b()]);
    for (label, document) in [
        ("v1", v1_document(&[TASK_A, TASK_B], "adjust")),
        ("v2", v2_document(vec![goal_a()])),
        ("v3", v3_document(vec![goal_a()])),
    ] {
        let marked_round = evaluate_with(&with_mark, Some(record(&with_mark, &document)));
        let base_round = evaluate_with(&without_mark, Some(record(&without_mark, &document)));
        assert_eq!(
            numbers(decision(&marked_round, TASK_B)),
            numbers(decision(&base_round, TASK_B)),
            "{label}: task B keeps its numbers"
        );
        assert_ne!(
            numbers(decision(&marked_round, TASK_A)),
            numbers(decision(&base_round, TASK_A)),
            "{label}: the goal weighed task A without the mark"
        );
        assert!(
            !has(
                decision(&marked_round, TASK_B),
                PolicyCode::ResourceTargetPackageIrreversible
            ),
            "{label}: task B carries no new reason"
        );
    }
}

// Test 3 (work order, tests 3): a target that names the marked task in `tasks`, and one with
// `override`, are void for it, the reason says so, and the target's other task is unaffected.
// goal-a names tasks A and B in override mode, as a v1 and as a v2 record. Task A, marked, carries
// the new reason and neither an applied target nor an override; its effective score is the one
// without a record and its term the base 1000. Task B keeps the override and every number it has
// without the mark.
#[test]
fn gr2s_t3_named_tasks_and_an_override_are_void_for_the_marked_task_only() {
    let with_mark = catalog(&[marked(task_a()), task_b()]);
    let without_mark = catalog(&[task_a(), task_b()]);
    let mut override_a = goal_a();
    override_a["apply"] =
        json!({"mode": "override", "weight": "score_stage", "manual_offset": "supersede"});
    override_a["tasks"] = json!([TASK_A, TASK_B]);
    let no_record = evaluate_with(&with_mark, None);
    for (label, document) in [
        ("v1", v1_document(&[TASK_A, TASK_B], "override")),
        ("v2", v2_document(vec![override_a.clone()])),
    ] {
        let marked_round = evaluate_with(&with_mark, Some(record(&with_mark, &document)));
        let base_round = evaluate_with(&without_mark, Some(record(&without_mark, &document)));
        let task = decision(&marked_round, TASK_A);
        assert!(
            has(task, PolicyCode::ResourceTargetPackageIrreversible),
            "{label}: {task:#?}"
        );
        for code in [
            PolicyCode::ResourceTargetOverride,
            PolicyCode::ResourceTargetApplied,
            PolicyCode::ResourceTargets,
        ] {
            assert!(!has(task, code), "{label}: no {code:?} on {task:#?}");
        }
        assert_eq!(
            effective(task),
            effective(decision(&no_record, TASK_A)),
            "{label}: task A's effective score is the one without a record"
        );
        assert_eq!(term(&marked_round, TASK_A), 1000, "{label}: the base term");
        let other = decision(&marked_round, TASK_B);
        assert!(
            has(other, PolicyCode::ResourceTargetOverride),
            "{label}: {other:#?}"
        );
        assert_eq!(
            numbers(other),
            numbers(decision(&base_round, TASK_B)),
            "{label}: task B is unaffected"
        );
        assert!(
            has(
                decision(&base_round, TASK_A),
                PolicyCode::ResourceTargetOverride
            ),
            "{label}: without the mark the override covers task A"
        );
    }
}

/// The resources of a round over `facts`, as gate_ss1's round derives them.
fn projected(catalog: &CompiledCatalog, facts: &crate::EvaluationFacts) -> EvaluationResources {
    project_fact_pools(
        catalog,
        facts,
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
    )
}

// Test 6 (work order, tests 6): the forward projection sees the mark. Task X produces 20 of pool A
// at priority 100; task Y produces 3 of pool B at priority 110. A v2 goal on pool A with step 1
// and importance 1 000 000 lifts task X's term to its bound (1 000 000), ahead of task Y's ten
// priority levels: the projection's first step runs task X. With task X marked, its term is the
// base 1000 and the first step runs task Y.
#[test]
fn gr2s_t6_the_forward_projection_reads_the_mark() {
    let task_x = || task("task_x", 100, &[(POOL_A, 20)]);
    let task_y = || task("task_y", 110, &[("pool_b", 3)]);
    let mut strong = goal_a();
    strong["scale"] = json!(1);
    strong["importance_milli"] = json!(1_000_000);
    for (label, catalog, first) in [
        ("unmarked", catalog(&[task_x(), task_y()]), "task_x"),
        ("marked", catalog(&[marked(task_x()), task_y()]), "task_y"),
    ] {
        let mut facts = readings();
        facts.push(record(&catalog, &v2_document(vec![strong.clone()])));
        let facts = inputs(facts, Vec::new());
        let resources = projected(&catalog, &facts);
        let projection = project_forward(
            &catalog,
            &facts,
            &resources,
            time(NOW),
            11,
            ForwardProjectionConfig::for_hours(1, 4).expect("a projection config"),
        )
        .unwrap_or_else(|error| panic!("{label}: the projection runs: {error:?}"));
        let step = projection
            .steps
            .first()
            .unwrap_or_else(|| panic!("{label}: a first step: {projection:#?}"));
        assert_eq!(step.task_ids, [first], "{label}: {projection:#?}");
    }
}

// ---- The composition fixture.

const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const VALUED: &str = "pool.valued";
/// A complete rule a chain follower rides with.
const HEAD: &str = "rule.head";
/// The follower of HEAD; its package provides a capability it marks irreversible itself.
const FOLLOW: &str = "rule.follow";
/// Its package provides a capability only another package of the root marks irreversible.
const SINK: &str = "rule.sink";
/// Its package marks the capability SINK provides irreversible.
const OTHER: &str = "rule.other";
/// Its package provides a capability nobody marks.
const PLAIN: &str = "rule.plain";

/// An hourly clock.
fn clock() -> Value {
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

/// An edge: `dependency_completed` of `task_id` accepting `succeeded` alone, `since_own_run`.
fn edge(task_id: &str) -> Value {
    json!({
        "kind": "dependency_completed",
        "task_id": task_id,
        "terminal_states": ["succeeded"],
        "since_own_run": true
    })
}

/// A `schedule.json` rule running the package of its own id on `trigger`, producing one unit of
/// the valued pool when `produces`.
fn rule(id: &str, trigger: Value, produces: bool) -> Value {
    let produces = if produces {
        json!([{
            "pool_id": VALUED,
            "direction": "produce",
            "amount": 1,
            "observation_source": "scan_verified",
            "confidence_milli": 1000
        }])
    } else {
        json!([])
    };
    json!({
        "id": id,
        "package_id": id,
        "priority": 100,
        "trigger": trigger,
        "feedback_stop": {
            "kind": "outcome",
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
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
        "load_profile": {"kind": "light"},
        "strategic_weight_milli": 1000
    })
}

/// The fixture's `schedule.json`: HEAD and its follower FOLLOW, SINK, OTHER and PLAIN.
fn schedule(extra: Option<(&str, Value)>) -> Value {
    let mut rules = vec![
        rule(HEAD, clock(), true),
        rule(FOLLOW, edge(HEAD), false),
        rule(SINK, clock(), true),
        rule(OTHER, clock(), true),
        rule(PLAIN, clock(), true),
    ];
    if let Some((member, value)) = extra {
        rules[4][member] = value;
    }
    schedule_of(rules)
}

/// A `schedule.json` of `rules` and the valued pool.
fn schedule_of(rules: Vec<Value>) -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [],
        "rules": rules,
        "pools": [{
            "id": VALUED,
            "capacity": 1000,
            "projection": {"kind": "none"},
            "observation": {"kind": "fact", "fact_key": "resource.valued"},
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
            "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000}
        }],
        "activity": [],
        "timeline": [],
        "held": []
    })
}

/// What each package of the fixture provides, as a reader read it; `unread` could not be read.
fn declared(unread: Option<&str>) -> IrreversiblePackages {
    let packages: [(&str, &[(&str, bool)]); 5] = [
        (HEAD, &[]),
        (FOLLOW, &[("cap_y", true)]),
        (SINK, &[("cap_x", false)]),
        (OTHER, &[("cap_x", true)]),
        (PLAIN, &[("cap_p", false)]),
    ];
    IrreversiblePackages::new(
        provided(&packages)
            .into_iter()
            .map(|(package_id, capabilities)| {
                let read = (Some(package_id.as_str()) != unread).then_some(capabilities);
                (package_id, read)
            })
            .collect(),
    )
}

/// Each package of `packages` with the capabilities it provides, each with its mark.
fn provided(packages: &[(&str, &[(&str, bool)])]) -> BTreeMap<String, BTreeMap<String, bool>> {
    packages
        .iter()
        .map(|(package_id, capabilities)| {
            (
                (*package_id).to_owned(),
                capabilities
                    .iter()
                    .map(|(capability, marked)| ((*capability).to_owned(), *marked))
                    .collect(),
            )
        })
        .collect()
}

/// Composes `schedule` for one instance of the game, whose registry holds the package of each of
/// its rules, with `irreversible` as the root's declarations.
fn compose(
    schedule: &Value,
    irreversible: &IrreversiblePackages,
) -> Result<StandardPackComposition, crate::StandardPackComposeFailure> {
    let packages = schedule["rules"]
        .as_array()
        .expect("the fixture's rules")
        .iter()
        .map(|rule| rule["package_id"].as_str().expect("a package id"))
        .collect::<Vec<_>>();
    let bundle = serde_json::to_vec(&json!({
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
            .collect::<Vec<_>>()
    }))
    .expect("bundle bytes");
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
            irreversible,
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

/// The ids of the composed tasks that carry the mark, and the source bytes of the tasks
/// document hold the member exactly for them.
fn marked_tasks(composition: &StandardPackComposition) -> BTreeSet<String> {
    let compiled = composition
        .compiled()
        .catalog()
        .tasks
        .tasks
        .iter()
        .filter(|task| task.is_irreversible_reach())
        .map(|task| task.id.clone())
        .collect::<BTreeSet<_>>();
    let document = serde_json::from_slice::<Value>(&composition.sources().tasks.bytes)
        .expect("the composed tasks document");
    let written = document["tasks"]
        .as_array()
        .expect("the composed tasks")
        .iter()
        .filter(|task| task.get(IRREVERSIBLE_REACH_MEMBER).is_some())
        .map(|task| {
            assert_eq!(task[IRREVERSIBLE_REACH_MEMBER], json!(true), "{task}");
            task["id"].as_str().expect("a task id").to_owned()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        compiled, written,
        "the member is written exactly where it is read"
    );
    compiled
}

// Test 4 (work order, tests 4): the composer writes the member only on the tasks of irreversible
// packages and on a chain head with a riding irreversible follower. FOLLOW's package marks its
// capability itself; SINK's provides a capability only OTHER's marks (the whole-root rule); HEAD's
// provides nothing, but FOLLOW rides with it. PLAIN's capability nobody marks: PLAIN carries no
// member. A rule of `schedule.json` that writes the member itself is refused at it.
#[test]
fn gr2s_t4_the_composer_marks_irreversible_packages_and_the_chain_heads_they_ride_with() {
    let composition = compose(&schedule(None), &declared(None))
        .unwrap_or_else(|failure| panic!("the fixture composes: {failure}"));
    let [(_, followers)] = composition.benefit_followers() else {
        panic!("one game: {:?}", composition.benefit_followers());
    };
    assert_eq!(
        followers
            .iter()
            .map(|follower| (follower.task_id.as_str(), follower.rides_with.as_str()))
            .collect::<Vec<_>>(),
        [(FOLLOW, HEAD)],
        "FOLLOW rides with HEAD"
    );
    assert_eq!(
        marked_tasks(&composition),
        [HEAD, FOLLOW, SINK, OTHER]
            .map(str::to_owned)
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    assert!(composition.unreadable_packages().is_empty());
    assert!(
        composition
            .warnings()
            .iter()
            .all(|warning| !warning.is(SharedCode::StandardPackPackageUnreadable)),
        "{:?}",
        composition.warnings()
    );

    let failure = compose(
        &schedule(Some((IRREVERSIBLE_REACH_MEMBER, json!(true)))),
        &declared(None),
    )
    .expect_err("a rule that writes the member is refused");
    assert!(
        failure.issues().iter().any(|refused| {
            refused.issue.problem == StandardPackProblem::ScheduleInvalid
                && refused.issue.pointer == format!("/rules/4/{IRREVERSIBLE_REACH_MEMBER}")
        }),
        "{failure}"
    );
}

// Test 5 (work order, tests 5, the composer's half): a package of the root that could not be read
// marks every task of its game, PLAIN's included, and the composition warns once, naming it.
#[test]
fn gr2s_t5_an_unreadable_package_marks_every_task_of_its_game_and_warns_once() {
    let composition = compose(&schedule(None), &declared(Some(PLAIN)))
        .unwrap_or_else(|failure| panic!("the fixture composes: {failure}"));
    assert_eq!(
        marked_tasks(&composition),
        [HEAD, FOLLOW, SINK, OTHER, PLAIN]
            .map(str::to_owned)
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        composition.unreadable_packages(),
        [(GAME.to_owned(), vec![PLAIN.to_owned()])]
    );
    let unreadable = composition
        .warnings()
        .into_iter()
        .filter(|warning| warning.is(SharedCode::StandardPackPackageUnreadable))
        .collect::<Vec<_>>();
    assert_eq!(unreadable.len(), 1, "{unreadable:?}");
    assert_eq!(unreadable[0].value("game"), Some(&json!(GAME)));
    assert_eq!(unreadable[0].value("package_id"), Some(&json!(PLAIN)));
}

// Fix round 1, point 1 (the independent review; `standard_pack_rule_followers`): the mark follows
// every chain link the evaluator reads, not only the followers the coverage lists. Four heads, each
// a complete rule on an hourly clock:
// - H1's follower B1 writes its own `produces` (so it is no coverage follower);
// - H2's follower B2 is linked by a `dependency_completed` that also accepts a `failed` end (no
//   edge);
// - H3's follower C3 is a complete rule, and B3 follows C3 (the coverage lists B3 with C3 alone).
// B1, B2 and B3 run a package that marks its capability irreversible: H1, H2 and H3 are marked,
// with C3. H4's one follower F4 runs a package nobody marks: neither carries the member. The goal
// check reads the same lists: a `tier` entry over the four heads, each of whose packages provides
// nothing, is refused at H1, H2 and H3 (`goal_governs_irreversible`) and passes H4.
#[test]
fn gr2s_t7_the_mark_follows_every_chain_link_the_evaluator_reads() {
    let both_ends = json!({
        "kind": "dependency_completed",
        "task_id": "rule.h2",
        "terminal_states": ["succeeded", "failed"],
        "since_own_run": true
    });
    let schedule = schedule_of(vec![
        rule("rule.h1", clock(), true),
        rule("rule.b1", edge("rule.h1"), true),
        rule("rule.h2", clock(), true),
        rule("rule.b2", both_ends, false),
        rule("rule.h3", clock(), true),
        rule("rule.c3", edge("rule.h3"), true),
        rule("rule.b3", edge("rule.c3"), false),
        rule("rule.h4", clock(), true),
        rule("rule.f4", edge("rule.h4"), false),
    ]);
    let packages: [(&str, &[(&str, bool)]); 9] = [
        ("rule.h1", &[]),
        ("rule.b1", &[("cap_y", true)]),
        ("rule.h2", &[]),
        ("rule.b2", &[("cap_y", true)]),
        ("rule.h3", &[]),
        ("rule.c3", &[]),
        ("rule.b3", &[("cap_y", true)]),
        ("rule.h4", &[]),
        ("rule.f4", &[("cap_p", false)]),
    ];
    let irreversible = IrreversiblePackages::new(
        provided(&packages)
            .into_iter()
            .map(|(package_id, capabilities)| (package_id, Some(capabilities)))
            .collect(),
    );
    let composition = compose(&schedule, &irreversible)
        .unwrap_or_else(|failure| panic!("the shapes compose: {failure}"));
    let [(_, followers)] = composition.benefit_followers() else {
        panic!("one game: {:?}", composition.benefit_followers());
    };
    assert_eq!(
        followers
            .iter()
            .map(|follower| (follower.task_id.as_str(), follower.rides_with.as_str()))
            .collect::<Vec<_>>(),
        [("rule.b3", "rule.c3"), ("rule.f4", "rule.h4")],
        "the coverage lists neither B1 nor B2, and B3 with C3"
    );
    assert_eq!(
        marked_tasks(&composition),
        [
            "rule.h1", "rule.b1", "rule.h2", "rule.b2", "rule.h3", "rule.c3", "rule.b3"
        ]
        .map(str::to_owned)
        .into_iter()
        .collect::<BTreeSet<_>>()
    );

    let bytes = serde_json::to_vec(&schedule).expect("schedule bytes");
    let parsed = parse_standard_pack_schedule(&bytes)
        .unwrap_or_else(|issue| panic!("the schedule reads: {issue}"));
    let lists = standard_pack_rule_followers(&parsed);
    let mut references = GoalReferences::new(provided(&packages));
    for rule in &parsed.rules {
        references.rules.insert(
            rule.id.clone(),
            GoalRule {
                package_id: rule.package_id.clone(),
                fixed_value: true,
                followers: lists.get(&rule.id).cloned().unwrap_or_default(),
            },
        );
    }
    let goals = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.goals.v1",
        "game": GAME,
        "entries": [{
            "name": "weight", "type": "tier",
            "governs": {"rules": ["rule.h1", "rule.h2", "rule.h3", "rule.h4"]},
            "title": {"en": "Weight"}, "default": "normal",
            "options": [{"value": "normal", "title": {"en": "Normal"}, "scale_milli": 1000},
                        {"value": "more", "title": {"en": "More"}, "scale_milli": 2000}]
        }]
    }))
    .expect("goals bytes");
    let goals = parse_standard_pack_goals(&goals, GAME)
        .unwrap_or_else(|issues| panic!("the declaration reads: {issues:?}"));
    let refused = check_standard_pack_goal_references(&goals, &references)
        .iter()
        .map(|issue| {
            assert_eq!(
                issue.problem,
                StandardPackProblem::GoalGovernsIrreversible,
                "{issue:?}"
            );
            issue.pointer.clone()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        refused,
        (0..3)
            .map(|index| format!("/entries/0/governs/rules/{index}"))
            .collect::<Vec<_>>()
    );
}

// Fix round 1, point 5 (the independent review): the run cap's goal term (SS-6) reads the mark.
// gate_ss6's goal-driven task consumes 30 of pool A (reserve 100, read at 1000) and 7 of pool D
// (reserve 10, read at 80), so its spend term is 10, and produces 10 of pool B, which carries no
// base weight: unmarked, an open goal of 100 on pool B read at 45 (gap 55) hands the gap and caps
// the run at ⌈55 / 10⌉ = 6. Marked, the same task under the same record is dispatched with the
// `policy_dispatch_parameters` of an instance without a record, whole: no gap and the spend term
// alone, 10.
#[test]
fn gr2s_t8_the_run_caps_goal_term_reads_the_mark() {
    let driven = || {
        ss6::task(&ss6::TaskDecl {
            consumes: &[(ss6::POOL_A, 30), (ss6::POOL_D, 7)],
            reserves: &[(ss6::POOL_A, 100), (ss6::POOL_D, 10)],
            produces: &[(ss6::POOL_B, 10)],
            ..ss6::TaskDecl::new(ss6::TASK_A, 100)
        })
    };
    let dispatched = |catalog: &CompiledCatalog, with_record: bool| {
        let mut facts = vec![
            ss6::reading_of(ss6::POOL_A_KEY, 1000),
            ss6::reading_of(ss6::POOL_D_KEY, 80),
            ss6::reading_of(ss6::POOL_B_KEY, 45),
        ];
        if with_record {
            let document = ss6::goals(&[ss6::goal(ss6::POOL_B, 100)]);
            facts.push(ss6::stored(catalog, &document, NOW - HOUR_MS));
        }
        let evaluation = ss6::round(catalog, facts);
        ss6::parameters(&evaluation, ss6::TASK_A)
            .cloned()
            .unwrap_or_else(|| panic!("the dispatch's parameters: {evaluation:#?}"))
    };
    let unmarked = ss6::catalog(&[driven()]);
    let weighed = dispatched(&unmarked, true);
    assert_eq!(
        weighed.outcome.value("gaps_remaining"),
        Some(&json!([{"effect_index": 0, "gap_remaining": 55}])),
        "{weighed:?}"
    );
    assert_eq!(
        weighed.outcome.value("run_cap"),
        Some(&json!(6)),
        "{weighed:?}"
    );

    let marked_catalog = ss6::catalog(&[marked(driven())]);
    let without = dispatched(&marked_catalog, false);
    assert_eq!(
        without.outcome.value("run_cap"),
        Some(&json!(10)),
        "{without:?}"
    );
    assert_eq!(without.outcome.value("gaps_remaining"), None, "{without:?}");
    assert_eq!(
        dispatched(&marked_catalog, true),
        without,
        "the marked task's values are the ones without a record"
    );
}
