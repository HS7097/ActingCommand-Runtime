// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 G-R2x gate tests, policy half (0.12.77; register §1.499; coordinator work order
// `rd-1121/brief-455-gr2x.md`; contracts/resource-targets.md, "A task that can do something
// irreversible"; contracts/standard-pack-schedule.md, "The irreversible mark"). G-R2s (0.12.60,
// register §1.369) is withdrawn: a goal raises the weight of every task that produces its pool, a
// package that can do something irreversible included. The composer writes `irreversible_reach`
// on no task, whatever makes a package irreversible (its own declaration, a remedy, a rule it
// follows, an unreadable package of its root); a task that carries it, as a catalog generation of
// 0.12.60 to 0.12.76 does, is weighed with its instance's target record as every task is, and no
// other candidate's number moves. The void of a game whose root cannot be composed (G-R2s round
// 2) stays: t10 is G-R2s's, unchanged.
//
// The evaluation fixture is gate_ss1's (tasks on one instance, due at NOW; pool A: step 10, base
// 500, gap weight 100; pool B: step 1, base 7, no gap), and gate_ss6's for the run cap; the
// composition fixture is neutral rules in catalog-a's task shape. Every expected value comes from
// the contract.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    BundleIndex, DeclaredCapabilities, Initiator, IrreversiblePackages, RESOURCE_TARGETS_FACT_KEY,
    RuntimeChannel, StandardPackProblem, StandardPackRootBytes, read_standard_pack_root,
};
use serde_json::{Value, json};

use super::gate_ss1::{
    A, POOL_A, POOL_A_KEY, POOL_B_KEY, TASK_A, TASK_B, V1, V2, catalog, decision, inputs, reading,
    round, task, task_a, task_b, term, time,
};
use super::gate_ss6 as ss6;
use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::{
    CandidateEligibility, CappedMemory, CatalogDocumentSource, ChainHandoverInputs,
    CompiledCatalog, DecisionReason, DispatchIntent, EvaluationResources, FactRefreshInputs,
    FactRefreshSettings, FactValue, ForwardProjectionConfig, HostResourceSnapshot,
    IRREVERSIBLE_REACH_MEMBER, InstanceHolds, InstancePoolLines, LeftOutFacts, NoProgressGuards,
    ObservedFact, OwedReruns, PolicyCode, PolicyEvaluation, PolicyEvaluationError, RemedyPlan,
    RunSlots, ScopeSelector, StandardPackBinding, StandardPackComposeInput,
    StandardPackComposition, StandardPackGameFiles, TargetSetter, TaskDecision,
    check_resource_targets, compile_composed_catalog, compose_standard_pack_rules,
    evaluate_with_left_out_facts, parse_resource_targets, project_fact_pools, project_forward,
};

const V3: &str = "actingcommand.resource-targets.v3";
/// When every record of these tests is checked and applied.
const CHECKED_AT: u64 = NOW - HOUR_MS;
/// The code G-R2s gave a candidate whose record it left out, retired in 0.12.77.
const RETIRED: &str = "resource_target_package_irreversible";

// ---- The evaluation fixture.

/// `task` carrying the irreversible mark, as a generation of 0.12.60 to 0.12.76 carries it.
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

/// Whether `decision` carries the retired code of G-R2s.
fn carries_retired(decision: &TaskDecision) -> bool {
    decision
        .reasons
        .iter()
        .any(|reason| reason.outcome.code().as_str() == RETIRED)
}

/// The reasons of the round's dispatch chain of `task_id` on instance A.
fn dispatch_reasons<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a [DecisionReason] {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id && intent.instance_id == A)
        .unwrap_or_else(|| panic!("{task_id} is dispatched: {evaluation:#?}"));
    &evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .unwrap_or_else(|| panic!("the dispatch's chain: {evaluation:#?}"))
        .reasons
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

// Test 1 (work order, tests 1): on an instance with a v2 record and one with a v3 record, a
// candidate whose task carries the mark is weighed number for number as the same candidate without
// it. Task A, alone and judged, produces 20 of pool A; each record has goal-a on pool A at a gap of
// 50. Under both records the candidate of the catalog whose task A carries the mark and the one of
// the catalog without it have the same whole decision (state, rank with its effective score, every
// reason with its values and detail, the benefit its `rank_breakdown` names, its own wake) and the
// same dispatch reasons (the `policy_dispatch_parameters` with its `gap_remaining` included); the
// goal weighs both (term 2000, the base alone being 1000) and neither carries the retired code.
// The run cap's goal term (gate_ss6's goal-driven task: spend term 10, an open goal of 100 on pool
// B read at 45) is 6 for the task with the mark as without it. The forward projection reads the
// task alike too (G-R2s's t6 fixture: a strong goal lifts task X, marked, to the first step).
// Every way a package counted as irreversible (its own declaration, a remedy, a rule it follows,
// an unreadable package of its root) reached the evaluator only as this member; t4 shows that the
// composer writes it for none of them, so their catalogs are the one without a declaration.
#[test]
fn gr2x_t1_a_task_that_carried_the_mark_is_weighed_as_without_it() {
    let carried = catalog(&[judged(marked(task_a()))]);
    let plain = catalog(&[judged(task_a())]);
    for (label, document) in [
        ("v2", v2_document(vec![goal_a()])),
        ("v3", v3_document(vec![goal_a()])),
    ] {
        let with_mark = evaluate_with(&carried, Some(record(&carried, &document)));
        let without = evaluate_with(&plain, Some(record(&plain, &document)));
        assert_eq!(term(&without, TASK_A), 2000, "{label}: the goal weighs it");
        assert_eq!(term(&with_mark, TASK_A), 2000, "{label}: with the mark too");
        let decided = decision(&with_mark, TASK_A);
        assert_eq!(
            decided,
            decision(&without, TASK_A),
            "{label}: the whole decision"
        );
        assert!(!carries_retired(decided), "{label}: {decided:#?}");
        assert_eq!(
            dispatch_reasons(&with_mark, TASK_A),
            dispatch_reasons(&without, TASK_A),
            "{label}: its dispatch's reasons"
        );
        assert!(
            dispatch_reasons(&with_mark, TASK_A)
                .iter()
                .all(|reason| reason.outcome.code().as_str() != RETIRED),
            "{label}"
        );
    }

    let driven = || {
        ss6::task(&ss6::TaskDecl {
            consumes: &[(ss6::POOL_A, 30), (ss6::POOL_D, 7)],
            reserves: &[(ss6::POOL_A, 100), (ss6::POOL_D, 10)],
            produces: &[(ss6::POOL_B, 10)],
            ..ss6::TaskDecl::new(ss6::TASK_A, 100)
        })
    };
    let parameters = |catalog: &CompiledCatalog| {
        let document = ss6::goals(&[ss6::goal(ss6::POOL_B, 100)]);
        let facts = vec![
            ss6::reading_of(ss6::POOL_A_KEY, 1000),
            ss6::reading_of(ss6::POOL_D_KEY, 80),
            ss6::reading_of(ss6::POOL_B_KEY, 45),
            ss6::stored(catalog, &document, NOW - HOUR_MS),
        ];
        let evaluation = ss6::round(catalog, facts);
        ss6::parameters(&evaluation, ss6::TASK_A)
            .cloned()
            .unwrap_or_else(|| panic!("the dispatch's parameters: {evaluation:#?}"))
    };
    let with_mark = parameters(&ss6::catalog(&[marked(driven())]));
    assert_eq!(
        with_mark.outcome.value("gaps_remaining"),
        Some(&json!([{"effect_index": 0, "gap_remaining": 55}])),
        "{with_mark:?}"
    );
    assert_eq!(
        with_mark.outcome.value("run_cap"),
        Some(&json!(6)),
        "{with_mark:?}"
    );
    assert_eq!(
        with_mark,
        parameters(&ss6::catalog(&[driven()])),
        "the dispatch's values of the task without the mark"
    );

    let task_x = || task("task_x", 100, &[(POOL_A, 20)]);
    let task_y = || task("task_y", 110, &[("pool_b", 3)]);
    let mut strong = goal_a();
    strong["scale"] = json!(1);
    strong["importance_milli"] = json!(1_000_000);
    for (label, catalog) in [
        ("without", catalog(&[task_x(), task_y()])),
        ("with the mark", catalog(&[marked(task_x()), task_y()])),
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
        assert_eq!(step.task_ids, ["task_x"], "{label}: {projection:#?}");
    }
}

// Test 2 (work order, tests 2): a v1 record that names a task carrying the mark applies to it as
// to any named task. Task A, carrying the mark or not, is named by goal-a in `adjust` mode (task A
// alone, judged, as G-R2s's t1 weighed it) and in `override` mode (with task B, both named, as
// G-R2s's t3 weighed them): its whole decision (its applied target or its override, its effective
// score, its reasons and its own wake) is the one of the task without the mark, it carries
// `resource_target_applied` or `resource_target_override`, and not the retired code.
#[test]
fn gr2x_t2_a_v1_record_that_names_a_task_that_carried_the_mark_applies_to_it() {
    for (mode, code, carried, plain, named) in [
        (
            "adjust",
            PolicyCode::ResourceTargetApplied,
            catalog(&[judged(marked(task_a()))]),
            catalog(&[judged(task_a())]),
            vec![TASK_A],
        ),
        (
            "override",
            PolicyCode::ResourceTargetOverride,
            catalog(&[marked(task_a()), task_b()]),
            catalog(&[task_a(), task_b()]),
            vec![TASK_A, TASK_B],
        ),
    ] {
        let document = v1_document(&named, mode);
        let with_mark = evaluate_with(&carried, Some(record(&carried, &document)));
        let without = evaluate_with(&plain, Some(record(&plain, &document)));
        let decided = decision(&with_mark, TASK_A);
        assert!(has(decided, code), "{mode}: {decided:#?}");
        assert!(!carries_retired(decided), "{mode}: {decided:#?}");
        assert_eq!(
            decided,
            decision(&without, TASK_A),
            "{mode}: the decision of the task without the mark, its wake included"
        );
    }
}

// Test 3 (work order, tests 3): every candidate whose task never carried the mark keeps its numbers
// and reasons. Task A carries the mark, task B does not; under a v1 record naming both, a v2 and a
// v3 record of goal-a, task B's rank and score-stage reasons equal those of the catalog without the
// mark, as G-R2s's t2 pinned them on main (the record's own resolution, which task B reads, is the
// one G-R2s left as it was; this slice changes no line it reads), and under v2 and v3 its term is
// the contract's: 30 of pool A at W = 500 + 500 over step 10, 3000, and 3 of pool B at its base 7,
// 21: 3021. Task A's numbers now equal those of the catalog without the mark too, and neither
// task carries the retired code. On an instance without a record, every decision of the two
// catalogs is the same.
#[test]
fn gr2x_t3_every_other_candidate_keeps_its_numbers() {
    let with_mark = catalog(&[marked(task_a()), task_b()]);
    let without_mark = catalog(&[task_a(), task_b()]);
    for (label, document, term_b) in [
        ("v1", v1_document(&[TASK_A, TASK_B], "adjust"), None),
        ("v2", v2_document(vec![goal_a()]), Some(3021)),
        ("v3", v3_document(vec![goal_a()]), Some(3021)),
    ] {
        let marked_round = evaluate_with(&with_mark, Some(record(&with_mark, &document)));
        let base_round = evaluate_with(&without_mark, Some(record(&without_mark, &document)));
        for task_id in [TASK_A, TASK_B] {
            assert_eq!(
                numbers(decision(&marked_round, task_id)),
                numbers(decision(&base_round, task_id)),
                "{label}: {task_id} keeps its numbers"
            );
            assert!(
                !carries_retired(decision(&marked_round, task_id)),
                "{label}: {task_id}"
            );
        }
        if let Some(term_b) = term_b {
            assert_eq!(term(&marked_round, TASK_B), term_b, "{label}");
        }
    }
    assert_eq!(
        evaluate_with(&with_mark, None).decisions,
        evaluate_with(&without_mark, None).decisions,
        "no record: every decision"
    );
}

/// One round at NOW over the readings of both pools, as [`round`] gives it, through the entry
/// point that takes the instances whose targets are void (`left_out`), with the fact refresh at
/// its starting defaults and no reader.
fn round_with_left_out(catalog: &CompiledCatalog, left_out: &LeftOutFacts) -> PolicyEvaluation {
    let facts = inputs(readings(), Vec::new());
    let resources = projected(catalog, &facts);
    let refresh = FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: true,
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
    };
    evaluate_with_left_out_facts::<PolicyEvaluationError>(
        catalog,
        &facts,
        &resources,
        time(NOW),
        11,
        &BTreeSet::new(),
        &refresh,
        &RemedyPlan::default(),
        &ChainHandoverInputs::default(),
        &RunSlots::default(),
        &OwedReruns::default(),
        &InstanceHolds::default(),
        &NoProgressGuards::default(),
        &mut CappedMemory::default(),
        &InstancePoolLines::default(),
        left_out,
        |_, _, _| Ok(false),
        |_: &DispatchIntent| Ok(CandidateEligibility::Eligible),
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

// G-R2s fix round 4 (the coordinator's work order of 11-14, item 2), unchanged by G-R2x: the void
// changes a decision only on an instance that holds a target record (the score stage leaves a
// record out only there), so only there does it enter the decision identity. Task A on instance A,
// which holds no record, under both pools' readings: the round with instance A's targets void, as
// while its game's root cannot be composed, has byte for byte the dispatches' identities
// (`decision_id` and reason chain id) of the round with no game voided, and is that round whole.
#[test]
fn gr2s_t10_a_voided_instance_without_a_record_keeps_its_decision_identity() {
    let catalog = catalog(&[task_a()]);
    let identities = |evaluation: &PolicyEvaluation| {
        evaluation
            .dispatch_intents
            .iter()
            .map(|intent| {
                (
                    intent.task_id.clone(),
                    intent.instance_id.clone(),
                    intent.decision_id.clone(),
                    intent.reason_chain_id.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let plain = round_with_left_out(&catalog, &LeftOutFacts::default());
    assert!(
        plain
            .dispatch_intents
            .iter()
            .any(|intent| intent.task_id == TASK_A && intent.instance_id == A),
        "task A is dispatched: {plain:#?}"
    );
    let mut voided = LeftOutFacts::default();
    voided.void_targets(A);
    let under_void = round_with_left_out(&catalog, &voided);
    assert_eq!(
        identities(&under_void),
        identities(&plain),
        "the identities of no void"
    );
    assert_eq!(under_void, plain, "the whole round of no void");
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
/// The rules G-R2s marked under [`declared`] with every package read.
const MARKED_BY_GR2S: [&str; 4] = [HEAD, FOLLOW, SINK, OTHER];

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

/// The fixture's `schedule.json`: HEAD and its follower FOLLOW, SINK, OTHER and PLAIN; `extra`
/// writes one member on PLAIN's rule.
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

/// One package of the fixture as a reader reads it: its id, what it provides (each capability with
/// its mark) and what its remedies list.
type Declared<'a> = (&'a str, &'a [(&'a str, bool)], &'a [&'a str]);

/// The fixture's packages as a reader read them, each with what it provides (each capability with
/// its mark) and what its remedies list; `unread` could not be read.
fn root_of(packages: [Declared<'_>; 5], unread: Option<&str>) -> IrreversiblePackages {
    IrreversiblePackages::new(
        packages
            .iter()
            .map(|(package_id, provides, remedies)| {
                let declaration = DeclaredCapabilities {
                    provides: provides
                        .iter()
                        .map(|(capability, marked)| ((*capability).to_owned(), *marked))
                        .collect(),
                    remedies: remedies
                        .iter()
                        .map(|capability| (*capability).to_owned())
                        .collect(),
                };
                let read = (Some(*package_id) != unread).then_some(declaration);
                ((*package_id).to_owned(), read)
            })
            .collect::<BTreeMap<_, _>>(),
    )
}

/// G-R2s's fixture declarations: FOLLOW's package marks `cap_y` itself, SINK's provides `cap_x`,
/// which OTHER's marks (the whole-root rule), HEAD's provides nothing but FOLLOW follows it, and
/// PLAIN's provides `cap_p`, which nobody marks; no remedy. `unread` could not be read.
fn declared(unread: Option<&str>) -> IrreversiblePackages {
    root_of(
        [
            (HEAD, &[], &[]),
            (FOLLOW, &[("cap_y", true)], &[]),
            (SINK, &[("cap_x", false)], &[]),
            (OTHER, &[("cap_x", true)], &[]),
            (PLAIN, &[("cap_p", false)], &[]),
        ],
        unread,
    )
}

/// Every package read, none irreversible: the root as G-R2s left it unmarked, and as it is
/// composed by every release.
fn nothing_irreversible() -> IrreversiblePackages {
    root_of(
        [
            (HEAD, &[], &[]),
            (FOLLOW, &[("cap_y", false)], &[]),
            (SINK, &[("cap_x", false)], &[]),
            (OTHER, &[("cap_q", false)], &[]),
            (PLAIN, &[("cap_p", false)], &[]),
        ],
        None,
    )
}

/// PLAIN irreversible by a remedy alone: its remedy lists `cap_x`, which OTHER's package marks.
fn by_a_remedy() -> IrreversiblePackages {
    root_of(
        [
            (HEAD, &[], &[]),
            (FOLLOW, &[("cap_y", false)], &[]),
            (SINK, &[], &[]),
            (OTHER, &[("cap_x", true)], &[]),
            (PLAIN, &[("cap_p", false)], &["cap_x"]),
        ],
        None,
    )
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

/// No composed task carries the member: neither the compiled catalog nor the tasks document the
/// composer wrote.
fn assert_no_member(composition: &StandardPackComposition, label: &str) {
    let tasks = &composition.compiled().catalog().tasks.tasks;
    assert_eq!(tasks.len(), 5, "{label}: every rule is composed");
    assert!(
        tasks.iter().all(|task| task.irreversible_reach.is_none()),
        "{label}: {tasks:#?}"
    );
    let document = serde_json::from_slice::<Value>(&composition.sources().tasks.bytes)
        .expect("the composed tasks document");
    assert!(
        document["tasks"]
            .as_array()
            .expect("the composed tasks")
            .iter()
            .all(|task| task.get(IRREVERSIBLE_REACH_MEMBER).is_none()),
        "{label}: {document}"
    );
}

/// The four documents' bytes, the catalog hash and the rules digest of `composition`.
fn composed_form(composition: &StandardPackComposition) -> (Vec<Vec<u8>>, String, String) {
    let sources = composition.sources();
    (
        [
            &sources.tasks,
            &sources.pools,
            &sources.activity,
            &sources.timeline,
        ]
        .iter()
        .map(|source| source.bytes.clone())
        .collect(),
        composition.compiled().catalog_hash().to_owned(),
        composition.rules_digest().to_owned(),
    )
}

// Test 4 (work order, tests 4): the composer writes the member on no task, whatever makes a
// package irreversible, and the composition of a root is otherwise what it was. The fixture root is
// composed with every package read and none irreversible (what every release composes for it,
// G-R2s writing no member there; `gate_rl1a`'s unchanged G-R2s test pins such a composition to
// the as-is documents and their hash), and with each declaration that made G-R2s mark tasks: its
// own declaration, the whole root's and a rule's follower (G-R2s's fixture, which marked HEAD,
// FOLLOW, SINK and OTHER), a remedy that lists an irreversible capability (PLAIN), and an
// unreadable package (PLAIN, which marked every task). Each is irreversible by the one rule, which
// the goal check still holds; each composes no member, the same four documents byte for byte, the
// same catalog hash and the same rules digest as the root without a declaration, and the same
// warnings, but for the unreadable package's one `standard_pack_package_unreadable` naming it
// (ruling 4). A rule of `schedule.json` that writes the member itself is still refused at it.
#[test]
fn gr2x_t4_the_composer_writes_no_mark_and_composes_what_it_composed_without_one() {
    let none = nothing_irreversible();
    let base = compose(&schedule(None), &none)
        .unwrap_or_else(|failure| panic!("the fixture composes: {failure}"));
    assert_no_member(&base, "nothing irreversible");
    assert!(base.unreadable_packages().is_empty());

    let own = declared(None);
    for package_id in [FOLLOW, SINK, OTHER] {
        assert!(own.counts_irreversible(package_id), "{package_id}");
    }
    assert!(!own.counts_irreversible(HEAD), "HEAD only by its follower");
    assert!(!own.counts_irreversible(PLAIN), "nobody marks cap_p");
    let remedy = by_a_remedy();
    assert_eq!(remedy.package(PLAIN), Some(true), "by its remedy");
    let unread = declared(Some(PLAIN));
    assert!(
        [HEAD, FOLLOW, SINK, OTHER, PLAIN]
            .iter()
            .all(|package_id| unread.counts_irreversible(package_id)),
        "the safe side"
    );
    for (label, irreversible) in [
        ("own, whole root and follower", own),
        ("remedy", remedy),
        ("unreadable", unread),
    ] {
        let composition = compose(&schedule(None), &irreversible)
            .unwrap_or_else(|failure| panic!("{label}: the fixture composes: {failure}"));
        assert_no_member(&composition, label);
        assert_eq!(
            composed_form(&composition),
            composed_form(&base),
            "{label}: the documents, the hash and the digest of the root without a declaration"
        );
        let mut expected = base.warnings();
        if !irreversible.unreadable().is_empty() {
            expected.push(
                actingcommand_contract::outcome::Outcome::new(
                    SharedCode::StandardPackPackageUnreadable,
                )
                .game(GAME)
                .package_id(PLAIN),
            );
        }
        assert_eq!(composition.warnings(), expected, "{label}");
    }

    let failure = compose(
        &schedule(Some((IRREVERSIBLE_REACH_MEMBER, json!(true)))),
        &none,
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

// Test 5 (work order, tests 5; ruling 5, the stored form): a catalog generation 0.12.76 composed
// for a root with an irreversible package (the fixture root under G-R2s's declarations: this
// build's composition with `irreversible_reach: true` on HEAD, FOLLOW, SINK and OTHER, as G-R2s's
// composer wrote it) is read by this build: the composed catalog's compiler takes it, the member is
// kept as read on exactly those tasks and in the tasks the catalog hash is computed over, so a
// stored generation still compiles to the hash its manifest records; that hash is not the one this
// build composes for the root, so the first check after a deploy activates a new generation for
// such a root. That the member has no effect is t1's.
#[test]
fn gr2x_t5_a_generation_composed_with_the_mark_is_read_and_its_hash_kept() {
    let composition = compose(&schedule(None), &declared(None))
        .unwrap_or_else(|failure| panic!("the fixture composes: {failure}"));
    let mut document = serde_json::from_slice::<Value>(&composition.sources().tasks.bytes)
        .expect("the composed tasks document");
    for task in document["tasks"]
        .as_array_mut()
        .expect("the composed tasks")
    {
        let id = task["id"].as_str().expect("a task id").to_owned();
        if MARKED_BY_GR2S.contains(&id.as_str()) {
            task[IRREVERSIBLE_REACH_MEMBER] = json!(true);
        }
    }
    let mut stored = composition.sources().clone();
    stored.tasks = CatalogDocumentSource::new(
        stored.tasks.source_uri.clone(),
        serde_json::to_vec(&document).expect("the stored tasks document"),
    );
    let compiled = compile_composed_catalog(&stored).unwrap_or_else(|failure| {
        panic!(
            "a generation of 0.12.76 compiles: {:?}",
            failure.diagnostics()
        )
    });
    let read = compiled
        .catalog()
        .tasks
        .tasks
        .iter()
        .filter(|task| task.irreversible_reach == Some(true))
        .map(|task| task.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(read, BTreeSet::from(MARKED_BY_GR2S), "kept as read");
    let hashed = serde_json::to_value(&compiled.catalog().tasks).expect("the hashed tasks");
    assert_eq!(
        hashed["tasks"]
            .as_array()
            .expect("the hashed tasks")
            .iter()
            .filter(|task| task[IRREVERSIBLE_REACH_MEMBER] == json!(true))
            .count(),
        MARKED_BY_GR2S.len(),
        "the catalog hash is computed over the member"
    );
    assert_ne!(
        compiled.catalog_hash(),
        composition.compiled().catalog_hash(),
        "a root whose tasks carried the mark composes another hash"
    );
}
