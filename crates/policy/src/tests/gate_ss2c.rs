// SPDX-License-Identifier: AGPL-3.0-only

// SS-2c (register §1.227; Alice's ruling 104 point 1: maintenance is startup, return home,
// recovery, readings maintenance and store update, everything else business; the approved design
// does not judge maintenance tasks by net benefit, `rd-1029/design-sch-state.md` §4.10; brief
// `rd-1029/brief-ss2c.md` tests 1 to 6; contracts/standard-pack-schedule.md, "Judged rules"):
// - a `schedule.json` rule that writes `"maintenance": true` composes without `benefit_judged`,
//   whatever its package's role, and nothing else of it changes;
// - only `true` declares: any other value is refused at the member, and the member is no V2 task
//   member;
// - a file without the member composes as SS-2b composes it;
// - the coverage counts a declared rule as not judged, and counts the declared rules apart;
// - with exactly two priority values, a rule in the other tier than its declaration says is one
//   `standard_pack_priority_tier_mismatch` warning, never a refusal.
//
// The game is neutral: one instance of one server, a registry holding a package per rule, the
// package `rule.maint` a return-home maintenance package. Every rule runs on an hourly clock due
// at NOW. Every expected value comes from the work order and the contract text.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW};
use crate::{
    BenefitCoverage, CandidateEligibility, CatalogDiagnosticCode, CatalogDocumentSource,
    CatalogSources, CompiledCatalog, DecisionReason, DispatchIntent, EvaluationFacts,
    EvaluationResources, EvaluationTime, HostResourceSnapshot, InstanceSnapshot, PolicyCode,
    PolicyEvaluation, PolicyEvaluationError, SCHEDULING_SCHEMA_VERSION_V2,
    STANDARD_PACK_OPERATION_ID, StandardPackBinding, StandardPackComposeFailure,
    StandardPackComposeInput, StandardPackComposition, StandardPackGameFiles, TaskDecision,
    compile_composed_catalog, compose_standard_pack_rules, evaluate_with_eligibility,
    project_fact_pools,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    BundleIndex, SCHEDULE_RULE_MAINTENANCE_MEMBER, STANDARD_PACK_SCHEDULE_FILE,
    StandardPackProblem, StandardPackRootBytes, parse_standard_pack_schedule,
    read_standard_pack_root,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const SERVER: &str = "server-a";
/// The return-home maintenance package of the registry (maintenance-role by `bundle.json`).
const MAINT: &str = "rule.maint";
/// Valued 1000 milli a unit (scale 1), no regeneration.
const VALUED: &str = "pool.valued";
/// The deferral the fixture's admission answers when it holds every candidate, so that the round
/// asks it about every candidate in allocation order.
const NOT_LOADED: &str = "pack_registry_not_loaded";

// Test 1: a rule with the declaration, whose package has no maintenance role, composes unmarked
// and ranks and dispatches as an unjudged candidate does: its composed task is the undeclared
// rule's without the mark (nothing else changes, its priority included); it ranks in the
// maintenance tier before the judged candidates though its priority is lower, decides as in a
// catalog without a mark (before SS-2b), and runs. A sibling rule without it is still judged: it
// declares no value, so it is `benefit_unknown` and never runs.
#[test]
fn ss2c_t1_a_declared_rule_composes_unmarked_and_runs_while_its_sibling_is_judged() {
    let rules = |declared: bool| {
        let first = rule("rule.declared", 100, Vec::new());
        vec![
            if declared { declare(first) } else { first },
            rule("rule.sibling", 300, Vec::new()),
            rule("rule.valued", 300, vec![produce(VALUED, 1)]),
        ]
    };
    let declared = composed(schedule(rules(true)));
    assert_eq!(
        marks(&declared),
        [
            ("rule.declared".to_owned(), None),
            ("rule.sibling".to_owned(), Some(true)),
            ("rule.valued".to_owned(), Some(true)),
        ]
    );

    // Nothing but the mark: the undeclared file's tasks, the declared rule's without its mark.
    let plain = composed(schedule(rules(false)));
    let mut expected = tasks_of(&plain);
    let unmarked = expected[0]
        .as_object_mut()
        .expect("a task object")
        .shift_remove("benefit_judged");
    assert_eq!(
        unmarked,
        Some(json!(true)),
        "undeclared, the rule is judged"
    );
    assert_eq!(tasks_of(&declared), expected);
    let text = String::from_utf8(declared.sources().tasks.bytes.clone()).expect("UTF-8");
    assert!(
        !text.contains(&format!("\"{SCHEDULE_RULE_MAINTENANCE_MEMBER}\"")),
        "the declaration never reaches the composed task: {text}"
    );

    // Ranking: the maintenance tier first, the sibling never asked.
    let (held, asked) = round(declared.compiled(), false);
    assert_eq!(asked, ["rule.declared", "rule.valued"], "{held:#?}");
    let (before, _) = round(&without_marks(&declared), false);
    assert_eq!(
        decision(&held, "rule.declared"),
        decision(&before, "rule.declared"),
        "the declared rule decides as in a catalog without a mark"
    );
    assert_eq!(
        decision(&held, "rule.declared")
            .rank
            .as_ref()
            .map(|rank| rank.priority),
        Some(100),
        "its own priority"
    );

    // Dispatch: the declared rule runs, unjudged; the sibling is judged and waits for values.
    let (evaluation, _) = round(declared.compiled(), true);
    assert_eq!(
        dispatched(&evaluation),
        ["rule.declared"],
        "{evaluation:#?}"
    );
    assert!(no_benefit_reason(decision(&evaluation, "rule.declared")));
    let sibling = decision(&evaluation, "rule.sibling");
    let unknown = sibling
        .reasons
        .iter()
        .find(|reason| reason.outcome.is(PolicyCode::BenefitUnknown))
        .unwrap_or_else(|| panic!("the sibling is judged: {sibling:#?}"));
    assert_eq!(
        unknown.outcome.value("benefit_gap"),
        Some(&json!("no_effects"))
    );
}

// Test 2: a declared rule whose package already has a maintenance role behaves the same: the
// declaration is redundant but harmless. The file with it composes byte for byte as the file
// without it (every document, the catalog hash and the rules digest), and both decide alike.
#[test]
fn ss2c_t2_a_declaration_on_a_maintenance_role_rule_changes_nothing() {
    let rules = |declared: bool| {
        let maint = rule(MAINT, 200, Vec::new());
        vec![
            if declared { declare(maint) } else { maint },
            rule("rule.business", 100, vec![produce(VALUED, 1)]),
        ]
    };
    let with = composed(schedule(rules(true)));
    let without = composed(schedule(rules(false)));
    assert_eq!(with.sources(), without.sources());
    assert_eq!(
        with.compiled().catalog_hash(),
        without.compiled().catalog_hash()
    );
    assert_eq!(with.rules_digest(), without.rules_digest());
    assert_eq!(
        marks(&with),
        [
            (MAINT.to_owned(), None),
            ("rule.business".to_owned(), Some(true)),
        ]
    );
    for admit in [false, true] {
        let (left, left_asked) = round(with.compiled(), admit);
        let (right, right_asked) = round(without.compiled(), admit);
        assert_eq!(left_asked, right_asked);
        assert_eq!(
            serde_json::to_vec(&left).expect("bytes"),
            serde_json::to_vec(&right).expect("bytes")
        );
    }
}

// Test 3: bad shapes are refused with the named code: `standard_pack_root_invalid`,
// `schedule_invalid` at `/rules/<i>/maintenance`, by the reader every composer runs and by the
// composer, `false` included (only `true` declares; no member is "not declared"). The member is
// `schedule.json`'s: a raw V2 task that writes it is `unknown_field`.
#[test]
fn ss2c_t3_a_declaration_of_another_shape_is_refused_at_the_member() {
    for value in [
        json!(false),
        json!("true"),
        json!(1),
        Value::Null,
        json!({}),
        json!([true]),
    ] {
        let mut second = rule("rule.business", 100, Vec::new());
        second[SCHEDULE_RULE_MAINTENANCE_MEMBER] = value.clone();
        let schedule = schedule(vec![rule(MAINT, 200, Vec::new()), second]);
        let pointer = format!("/rules/1/{SCHEDULE_RULE_MAINTENANCE_MEMBER}");

        let bytes = serde_json::to_vec(&schedule).expect("schedule bytes");
        let issue = parse_standard_pack_schedule(&bytes).expect_err("the reader refuses it");
        assert_eq!(issue.file, STANDARD_PACK_SCHEDULE_FILE, "{value}");
        assert_eq!(
            issue.problem,
            StandardPackProblem::ScheduleInvalid,
            "{value}"
        );
        assert_eq!(issue.pointer, pointer, "{value}");

        let failure = compose(schedule).expect_err("the composer refuses it");
        assert_eq!(failure.issues().len(), 1, "{value}: {failure}");
        let refused = &failure.issues()[0];
        assert_eq!(refused.game_id.as_deref(), Some(GAME), "{value}");
        assert_eq!(refused.issue.problem, StandardPackProblem::ScheduleInvalid);
        assert_eq!(refused.issue.pointer, pointer, "{value}");
    }

    // A raw V2 catalog: the member is no task member.
    let composition = composed(schedule(vec![rule("rule.business", 100, Vec::new())]));
    let mut tasks = document_of(&composition.sources().tasks);
    tasks["tasks"][0][SCHEDULE_RULE_MAINTENANCE_MEMBER] = json!(true);
    let failure = compile_composed_catalog(&CatalogSources {
        tasks: source("tasks.json", &tasks),
        ..composition.sources().clone()
    })
    .expect_err("a V2 task does not declare");
    let path = format!("/tasks/0/{SCHEDULE_RULE_MAINTENANCE_MEMBER}");
    assert!(
        failure.diagnostics().iter().any(|diagnostic| {
            diagnostic.code == CatalogDiagnosticCode::UnknownField && diagnostic.json_path == path
        }),
        "{:?}",
        failure.diagnostics()
    );
}

// Test 4: a `schedule.json` without the member composes byte for byte as SS-2b composes it: each
// rule's task is its file members after `id`, `scope`, `entrypoint` and `procedure_ref`, then an
// empty `instance_overrides`, then `benefit_judged: true` on the rules whose package is not
// maintenance-role; the catalog hash is the hash of exactly those documents. The reader keeps
// every member of such a rule, in file order, and reads it as not declared.
#[test]
fn ss2c_t4_a_schedule_without_the_member_composes_as_with_ss2b_alone() {
    let rules = vec![
        rule(MAINT, 200, Vec::new()),
        rule("rule.business", 100, vec![produce(VALUED, 1)]),
        rule("rule.none", 100, Vec::new()),
    ];
    let schedule = schedule(rules.clone());
    let read = parse_standard_pack_schedule(&serde_json::to_vec(&schedule).expect("bytes"))
        .unwrap_or_else(|issue| panic!("the file reads: {issue}"));
    for (parsed, written) in read.rules.iter().zip(&rules) {
        assert!(!parsed.maintenance, "{}", parsed.id);
        let mut body = written.as_object().expect("a rule object").clone();
        body.shift_remove("id");
        body.shift_remove("package_id");
        assert_eq!(parsed.body, body, "{}", parsed.id);
    }

    let composition = composed(schedule);
    let expected = ss2b_tasks_document(&rules, &[false, true, true]);
    assert_eq!(
        String::from_utf8(composition.sources().tasks.bytes.clone()).expect("UTF-8"),
        String::from_utf8(expected.clone()).expect("UTF-8")
    );
    let reference = compile_composed_catalog(&CatalogSources {
        tasks: CatalogDocumentSource::new("memory://ss2b/tasks.json", expected),
        ..composition.sources().clone()
    })
    .unwrap_or_else(|failure| panic!("the reference compiles: {:?}", failure.diagnostics()));
    assert_eq!(
        composition.compiled().catalog_hash(),
        reference.catalog_hash()
    );
}

// Test 5: the coverage counts a declared rule as not judged: a declared rule that declares no
// value is neither undeclared nor warned about, and the declared rules (on a business package or
// on a maintenance-role one) are counted apart.
#[test]
fn ss2c_t5_the_coverage_counts_a_declared_rule_as_not_judged() {
    let composition = composed(schedule(vec![
        declare(rule("rule.declared", 200, Vec::new())),
        declare(rule(MAINT, 200, Vec::new())),
        rule("rule.complete", 100, vec![produce(VALUED, 1)]),
        rule("rule.none", 100, Vec::new()),
    ]));
    assert_eq!(
        composition.benefit_coverage(),
        [(
            GAME.to_owned(),
            BenefitCoverage {
                complete: 1,
                pending: 0,
                undeclared: 1,
                maintenance_declared: 2,
            }
        )]
    );
    let warnings = composition.benefit_warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].is(SharedCode::StandardPackBenefitUndeclared));
    assert_eq!(warnings[0].value("task_id"), Some(&json!("rule.none")));
    assert!(composition.tier_warnings().is_empty());
}

// Test 6: with exactly two priority values, a declared rule on the lower value and a judged rule
// (no declaration, its package not maintenance-role) on the higher are one
// `standard_pack_priority_tier_mismatch` each (keys `game`, `task_id`), in file order; a declared
// rule on the higher, a judged rule on the lower and an undeclared maintenance-role rule are not.
// The root is composed as before and the composition's own warnings are unchanged. With one or
// three values there is no such warning.
#[test]
fn ss2c_t6_a_declaration_against_the_two_priority_values_warns_and_composes() {
    let two = vec![
        declare(rule("rule.declared_low", 100, Vec::new())),
        rule("rule.judged_high", 200, vec![produce(VALUED, 1)]),
        declare(rule("rule.declared_high", 200, Vec::new())),
        rule("rule.judged_low", 100, vec![produce(VALUED, 1)]),
        rule(MAINT, 100, Vec::new()),
    ];
    let composition = composed(schedule(two.clone()));
    let warnings = composition.tier_warnings();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    for (warning, rule_id) in warnings
        .iter()
        .zip(["rule.declared_low", "rule.judged_high"])
    {
        assert!(
            warning.is(SharedCode::StandardPackPriorityTierMismatch),
            "{warning:?}"
        );
        assert_eq!(warning.value("game"), Some(&json!(GAME)));
        assert_eq!(warning.value("task_id"), Some(&json!(rule_id)));
    }
    assert!(
        composition.warnings().is_empty(),
        "{:?}",
        composition.warnings()
    );
    assert_eq!(
        marks(&composition),
        [
            ("rule.declared_low".to_owned(), None),
            ("rule.judged_high".to_owned(), Some(true)),
            ("rule.declared_high".to_owned(), None),
            ("rule.judged_low".to_owned(), Some(true)),
            (MAINT.to_owned(), None),
        ]
    );

    let mut three = two.clone();
    three.push(rule("rule.third", 300, vec![produce(VALUED, 1)]));
    let composition = composed(schedule(three));
    assert!(composition.tier_warnings().is_empty());
    assert_eq!(composition.warnings().len(), 1);
    assert!(composition.warnings()[0].is(SharedCode::StandardPackPriorityTiersExceeded));

    let one = vec![
        declare(rule("rule.declared", 100, Vec::new())),
        rule("rule.judged", 100, vec![produce(VALUED, 1)]),
    ];
    assert!(composed(schedule(one)).tier_warnings().is_empty());
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

fn produce(pool: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": "produce",
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// A `schedule.json` rule running the package of its own id on its hourly clock.
fn rule(id: &str, priority: i16, produces: Vec<Value>) -> Value {
    json!({
        "id": id,
        "package_id": id,
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
        "strategic_weight_milli": 1000
    })
}

/// `rule` declaring itself maintenance.
fn declare(mut rule: Value) -> Value {
    rule[SCHEDULE_RULE_MAINTENANCE_MEMBER] = json!(true);
    rule
}

/// `schedule.json` with these rules, the valued pool and one profile active all day.
fn schedule(rules: Vec<Value>) -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [],
        "rules": rules,
        "pools": [{
            "id": VALUED,
            "capacity": 1_000_000,
            "projection": {"kind": "none"},
            "observation": {"kind": "fact", "fact_key": "resource.valued"},
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
            "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000}
        }],
        "activity": [{
            "id": "fixture-activity",
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
        }],
        "timeline": [],
        "held": []
    })
}

/// Composes `schedule` for instance A of the game: its registry holds a package per rule and
/// [`MAINT`], a return-home maintenance package.
fn compose(schedule: Value) -> Result<StandardPackComposition, StandardPackComposeFailure> {
    let packages = schedule["rules"]
        .as_array()
        .expect("the rules")
        .iter()
        .map(|rule| rule["package_id"].as_str().expect("a package").to_owned())
        .chain([MAINT.to_owned()])
        .collect::<BTreeSet<_>>();
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
            .collect::<Vec<_>>(),
        "maintenance": [{"package_id": MAINT, "server": SERVER, "uses": ["return_home"]}]
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
    let schedule = serde_json::to_vec(&schedule).expect("schedule bytes");
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

fn composed(schedule: Value) -> StandardPackComposition {
    compose(schedule).unwrap_or_else(|failure| panic!("the fixture composes: {failure}"))
}

/// Each composed task's id and `benefit_judged`, in catalog order.
fn marks(composition: &StandardPackComposition) -> Vec<(String, Option<bool>)> {
    composition
        .compiled()
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| (task.id.clone(), task.benefit_judged))
        .collect()
}

fn document_of(source: &CatalogDocumentSource) -> Value {
    serde_json::from_slice(&source.bytes).expect("a composed document")
}

/// The composed tasks as written in the tasks document.
fn tasks_of(composition: &StandardPackComposition) -> Vec<Value> {
    document_of(&composition.sources().tasks)["tasks"]
        .as_array()
        .expect("the tasks")
        .clone()
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-ss2c/{name}"),
        serde_json::to_vec_pretty(document).expect("document bytes"),
    )
}

/// The composition's catalog with every `benefit_judged` taken out: the catalog of before SS-2b.
fn without_marks(composition: &StandardPackComposition) -> CompiledCatalog {
    let mut tasks = document_of(&composition.sources().tasks);
    for task in tasks["tasks"].as_array_mut().expect("the tasks") {
        task.as_object_mut()
            .expect("a task object")
            .shift_remove("benefit_judged");
    }
    compile_composed_catalog(&CatalogSources {
        tasks: source("tasks.json", &tasks),
        ..composition.sources().clone()
    })
    .unwrap_or_else(|failure| panic!("the unmarked catalog compiles: {:?}", failure.diagnostics()))
}

/// The tasks document SS-2b's composer writes for `rules` on instance A (contract
/// "Composition" steps 3 to 5): per rule `id`, `scope`, `entrypoint`, `procedure_ref`, the rule's
/// other members in file order, an empty `instance_overrides`, then `benefit_judged: true` where
/// `judged`; the descriptor `standard-packs` version 1 with no approval.
fn ss2b_tasks_document(rules: &[Value], judged: &[bool]) -> Vec<u8> {
    let tasks = rules
        .iter()
        .zip(judged)
        .map(|(rule, judged)| {
            let written = rule.as_object().expect("a rule object");
            let mut task = Map::new();
            task.insert("id".to_owned(), written["id"].clone());
            task.insert(
                "scope".to_owned(),
                json!({"kind": "instance", "instance_id": A}),
            );
            task.insert(
                "entrypoint".to_owned(),
                json!({"operation_id": STANDARD_PACK_OPERATION_ID}),
            );
            task.insert(
                "procedure_ref".to_owned(),
                json!(format!(
                    "package.{}",
                    written["package_id"].as_str().expect("a package")
                )),
            );
            for (name, value) in written {
                if name != "id" && name != "package_id" {
                    task.insert(name.clone(), value.clone());
                }
            }
            task.insert("instance_overrides".to_owned(), json!([]));
            if *judged {
                task.insert("benefit_judged".to_owned(), json!(true));
            }
            Value::Object(task)
        })
        .collect::<Vec<_>>();
    let mut document = Map::new();
    document.insert(
        "schema_version".to_owned(),
        json!(SCHEDULING_SCHEMA_VERSION_V2),
    );
    document.insert(
        "catalog".to_owned(),
        json!({"catalog_id": "standard-packs", "catalog_version": 1, "approval_refs": []}),
    );
    document.insert("tasks".to_owned(), Value::Array(tasks));
    serde_json::to_vec_pretty(&Value::Object(document)).expect("document bytes")
}

/// One round at NOW on instance A, whose host is idle and which runs the package operation. With
/// `admit` every candidate is admitted; without it the admission holds every candidate, and the
/// second value lists the task ids it was asked about, in allocation order.
fn round(catalog: &CompiledCatalog, admit: bool) -> (PolicyEvaluation, Vec<String>) {
    let host_id = format!("host.{A}");
    let facts = EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-ss2c".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![InstanceSnapshot {
            instance_id: A.to_owned(),
            server_id: SERVER.to_owned(),
            game_id: GAME.to_owned(),
            host_id: host_id.clone(),
            available: true,
            capability_operation_ids: vec![STANDARD_PACK_OPERATION_ID.to_owned()],
            preferred_task_ids: Vec::new(),
        }],
        priority_offsets: Vec::new(),
    };
    let resources = project_fact_pools(
        catalog,
        &facts,
        &EvaluationResources {
            pools: Vec::new(),
            hosts: vec![HostResourceSnapshot {
                host_id,
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
    let mut asked = Vec::new();
    let evaluation = evaluate_with_eligibility(
        catalog,
        &facts,
        &resources,
        EvaluationTime {
            unix_ms: NOW,
            monotonic_ms: NOW,
        },
        11,
        |intent: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
            asked.push(intent.task_id.clone());
            if admit {
                return Ok(CandidateEligibility::Eligible);
            }
            Ok(CandidateEligibility::Deferred {
                reason: DecisionReason {
                    outcome: Outcome::relayed(NOT_LOADED),
                    detail: format!("fixture admission: {NOT_LOADED}"),
                },
                next_wake_unix_ms: None,
            })
        },
    )
    .unwrap_or_else(|error| panic!("the round evaluates: {error}"));
    (evaluation, asked)
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

/// Whether the decision carries neither benefit reason.
fn no_benefit_reason(decision: &TaskDecision) -> bool {
    !decision.reasons.iter().any(|reason| {
        reason.outcome.is(PolicyCode::BenefitUnknown)
            || reason.outcome.is(PolicyCode::PolicyBenefitNonpositive)
    })
}
