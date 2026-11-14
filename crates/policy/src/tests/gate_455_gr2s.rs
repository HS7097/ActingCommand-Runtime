// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 G-R2s gate tests, policy half (0.12.60; register §1.363; coordinator work order
// `rd-1114/brief-455-gr2s-irreversible-line.md` and the coordinator's two rulings of 11-14;
// contracts/resource-targets.md, "A task that can do something irreversible";
// contracts/standard-pack-schedule.md, "The irreversible mark"): a goal never raises the weight
// of a task whose package can do something irreversible. The composer marks such a task
// (`irreversible_reach`) by the whole game root's declarations and the chain followers that ride
// with it; the evaluator weighs a marked candidate exactly as on an instance without a target
// record, under a v1, a v2 and a v3 record alike, and changes no other candidate's number.
//
// The evaluation fixture is gate_ss1's (tasks on one instance, due at NOW; pool A: step 10, base
// 500, gap weight 100; pool B: step 1, base 7, no gap); the composition fixture is neutral rules
// in catalog-a's task shape. Every expected value comes from the contract.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    BundleIndex, Initiator, IrreversiblePackages, RESOURCE_TARGETS_FACT_KEY, RuntimeChannel,
    StandardPackProblem, StandardPackRootBytes, read_standard_pack_root,
};
use serde_json::{Value, json};

use super::gate_ss1::{
    A, POOL_A, POOL_A_KEY, POOL_B_KEY, TASK_A, TASK_B, V1, V2, catalog, decision, inputs, reading,
    round, task, task_a, task_b, term, time,
};
use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::{
    CompiledCatalog, DecisionReason, EvaluationResources, FactValue, ForwardProjectionConfig,
    HostResourceSnapshot, IRREVERSIBLE_REACH_MEMBER, ObservedFact, PolicyCode, PolicyEvaluation,
    ScopeSelector, StandardPackBinding, StandardPackComposeInput, StandardPackComposition,
    StandardPackGameFiles, TargetSetter, TaskDecision, check_resource_targets,
    compose_standard_pack_rules, parse_resource_targets, project_fact_pools, project_forward,
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
// one without a record, but for the one new reason, which it carries once; without the mark the
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
    for (label, document, unmarked_term) in [
        ("v1", v1_document(&[TASK_A], "adjust"), None),
        ("v2", v2_document(vec![goal_a()]), Some(2000)),
        ("v3", v3_document(vec![goal_a()]), Some(2000)),
    ] {
        let with = evaluate_with(&catalog, Some(record(&catalog, &document)));
        assert_eq!(
            without_the_new_reason(decision(&with, TASK_A)),
            base,
            "{label}: the marked candidate is the one without a record"
        );
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
    let provides = |capabilities: &[(&str, bool)]| {
        capabilities
            .iter()
            .map(|(capability, marked)| ((*capability).to_owned(), *marked))
            .collect::<BTreeMap<_, _>>()
    };
    let packages = [
        (HEAD, provides(&[])),
        (FOLLOW, provides(&[("cap_y", true)])),
        (SINK, provides(&[("cap_x", false)])),
        (OTHER, provides(&[("cap_x", true)])),
        (PLAIN, provides(&[("cap_p", false)])),
    ];
    IrreversiblePackages::new(
        packages
            .into_iter()
            .map(|(package_id, capabilities)| {
                let read = (Some(package_id) != unread).then_some(capabilities);
                (package_id.to_owned(), read)
            })
            .collect(),
    )
}

/// Composes `schedule` for one instance of the game, whose registry holds a package per rule,
/// with `irreversible` as the root's declarations.
fn compose(
    schedule: &Value,
    irreversible: &IrreversiblePackages,
) -> Result<StandardPackComposition, crate::StandardPackComposeFailure> {
    let packages = [HEAD, FOLLOW, SINK, OTHER, PLAIN];
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
