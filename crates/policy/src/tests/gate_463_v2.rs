// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 V2 gate tests, policy half (0.12.61; register §1.382; coordinator work order
// rd-1115/brief-463-v2-variables-run.md; contracts/package-variables.md, "The run"): the values a
// selected dispatch hands its run for the variables its package declares. Per variable, the
// application's entry row of its name in the v3 record of `session.resource_targets`, judged at
// use, else the package default; a variable the root check keeps from applying reads its default
// whatever is set; the values join `policy_dispatch_parameters` as `variable_values`, with remedies
// off too; a package without variables gives the reason chains of before byte for byte.
//
// The catalog is the selection fixture's (`selection_catalog`): hourly tasks of `game-a`, due at
// NOW, on instance A. The packages and their variables are made up. Every expected value is the
// work order's or the contract's.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::{ENVELOPE_BODY_BUDGET, Outcome, Record, RemedySource};
use actingcommand_contract::{
    GoalType, NotApplyingCause, PackageVariable, RootGoalDocument, RootPackageVariables,
    RootVariablesJudged, RuntimeChannel, VariableCause, VariableProblem, VariableSource,
    judge_root_variables, parse_package_variables, parse_standard_pack_goals,
};
use serde_json::{Value, json};

use super::selection_catalog::{NOW, catalog, instance};
use crate::{
    CandidateEligibility, CompiledCatalog, DecisionReason, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactRefreshInputs, FactRefreshSettings, FactValue, HostResourceSnapshot,
    ObservedFact, PolicyCode, PolicyEvaluation, PolicyEvaluationError, RemedyInputs, RemedyPlan,
    RemedyTable, ResolvedVariable, ScopeSelector, TargetSetter, VariablePackage, VariablePlan,
    VariableSetBy, VariableTable, VariableTableTask, check_resource_targets,
    evaluate_with_remedies, parse_resource_targets, plan_remedies, plan_variable_values,
    recorded_dispatch_parameters,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const VAR: &str = "t_var";
const OTHER: &str = "t_other";
const PACKAGE: &str = "neutral.vars";
const OTHER_PACKAGE: &str = "neutral.other";
const RESOURCE_TARGETS: &str = "session.resource_targets";
const PARAMETERS: &str = "policy_dispatch_parameters";
/// When the fixture's values were set: an hour before NOW.
const SET_AT: u64 = NOW - 3_600_000;

fn options(items: &[&str]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|item| json!({"value": item, "title": {"en": item}}))
            .collect(),
    )
}

/// The declaration of the fixture's four variables: `pick` (choice of item_a and item_b, default
/// item_b), `floor` (integer 0 to 200 on steps of 10, default 0), `order` (ordered, 1 to 2 of
/// item_a, item_b, item_c, default item_b then item_a) and `extra` (switch, default false).
fn declaration() -> Value {
    json!([
        {"name": "pick", "type": "choice", "title": {"en": "Item"},
         "options": options(&["item_a", "item_b"]), "default": "item_b"},
        {"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
         "min": 0, "max": 200, "step": 10, "default": 0},
        {"name": "order", "type": "ordered", "title": {"en": "Order"},
         "options": options(&["item_a", "item_b", "item_c"]), "min_count": 1, "max_count": 2,
         "default": ["item_b", "item_a"]},
        {"name": "extra", "type": "switch", "title": {"en": "Also the extra"}, "default": false}
    ])
}

fn variables(declaration: &Value) -> Vec<PackageVariable> {
    parse_package_variables(declaration, &[]).expect("the fixture's variables")
}

/// A task of the game with `package` (`None`: its package could not be read).
fn task(task_id: &str, package: Option<VariablePackage>) -> VariableTableTask {
    VariableTableTask {
        task_id: task_id.to_owned(),
        games: BTreeSet::from([GAME.to_owned()]),
        package,
    }
}

/// The package `package_id` declaring `variables` (Workflow #455 G-SLx: no longer with what it
/// provides and what its remedies list, which the table read for the rule of irreversibility).
fn package(package_id: &str, variables: Vec<PackageVariable>) -> VariablePackage {
    VariablePackage {
        package_id: package_id.to_owned(),
        variables,
        cap_fact_keys: Vec::new(),
    }
}

/// The table of `t_var` with the four variables alone.
fn table() -> VariableTable {
    VariableTable::build(&[task(VAR, Some(package(PACKAGE, variables(&declaration()))))])
}

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

fn facts(observed: Vec<ObservedFact>) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-variables".to_owned(),
        facts: observed,
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

/// A's v3 record holding `entries` and no target, set through the Lab at [`SET_AT`], as
/// `ApplyResourceTargets` composes it over no record.
fn record(catalog: &CompiledCatalog, entries: &[(&str, Value)]) -> ObservedFact {
    let document = json!({
        "schema_version": "actingcommand.resource-targets.v3",
        "instance": A,
        "targets": [],
        "entries": entries
            .iter()
            .map(|(name, value)| json!({"name": name, "value": value}))
            .collect::<Vec<_>>()
    });
    let parsed = parse_resource_targets(&serde_json::to_vec(&document).expect("document bytes"))
        .expect("the document reads");
    let composed = check_resource_targets(&parsed, catalog, &facts(Vec::new()), time(SET_AT))
        .expect("the document checks")
        .compose(
            None,
            Some(TargetSetter {
                set_unix_ms: SET_AT,
                channel: RuntimeChannel::Lab,
                initiator: None,
            }),
        )
        .expect("the record composes")
        .expect("with a setter every document composes");
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: RESOURCE_TARGETS.to_owned(),
        value: FactValue::RecordList(
            serde_json::from_value(serde_json::to_value(&composed.rows).expect("rows"))
                .expect("record list"),
        ),
        observed_at_unix_ms: SET_AT,
        expires_at_unix_ms: composed.expires_at_unix_ms,
        confidence_milli: 1_000,
    }
}

/// The remedy plan with remedies off (`scheduling.remedy.enabled` false): empty.
fn remedies_off(catalog: &CompiledCatalog, facts: &EvaluationFacts) -> RemedyPlan {
    plan_remedies(
        catalog,
        facts,
        time(NOW),
        &RemedyInputs {
            enabled: false,
            max_per_round: 2,
            table: RemedyTable::default(),
            journal: Vec::new(),
            stops: Vec::new(),
            signals_seen: BTreeSet::new(),
            configuration: BTreeMap::new(),
            persistent_gates: Vec::new(),
        },
    )
    .expect("the plan of remedies off")
}

fn evaluate(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    plan: &RemedyPlan,
) -> PolicyEvaluation {
    let resources = EvaluationResources {
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
    };
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
        instance_facts_loaded_at_unix_ms: None,
        instance_holds: BTreeMap::new(),
        idle_closed: BTreeMap::new(),
    };
    evaluate_with_remedies::<PolicyEvaluationError>(
        catalog,
        facts,
        &resources,
        time(NOW),
        11,
        &BTreeSet::new(),
        &refresh,
        plan,
        |_| Ok(CandidateEligibility::Eligible),
    )
    .unwrap_or_else(|error| panic!("the evaluation of valid inputs: {error}"))
}

/// The reasons of the chain of the evaluation's one dispatch on A, which is of `task_id`.
fn dispatch_chain(evaluation: &PolicyEvaluation, task_id: &str) -> Vec<DecisionReason> {
    let intents = evaluation
        .dispatch_intents
        .iter()
        .filter(|intent| intent.instance_id == A)
        .collect::<Vec<_>>();
    assert_eq!(intents.len(), 1, "{:#?}", evaluation.dispatch_intents);
    assert_eq!(intents[0].task_id, task_id);
    evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intents[0].reason_chain_id)
        .expect("the intent's reason chain")
        .reasons
        .clone()
}

fn default_value(name: &str, value: Value) -> ResolvedVariable {
    ResolvedVariable {
        variable_name: name.to_owned(),
        value,
        source: VariableSource::Default,
        set_by: None,
        problem: None,
    }
}

fn lab_set() -> Option<VariableSetBy> {
    Some(VariableSetBy {
        set_unix_ms: SET_AT,
        channel: RuntimeChannel::Lab,
    })
}

/// The plan's values of `t_var` on A.
fn values_of(plan: &VariablePlan) -> Vec<ResolvedVariable> {
    plan.values(VAR, A).expect("t_var's values").to_vec()
}

// Requirements 1, 4 and 5; the work order's tests "set beats default" and "remedies off still
// resolves": `pick` set to item_a and `order` to item_c then item_a read the set values (source
// `set`, who set them, when); `floor` and `extra`, unset, read their defaults. With remedies off
// the plan's `policy_dispatch_parameters` of t_var carries `variable_values`, one record per
// variable in declaration order, and the selected dispatch's reason chain holds that reason.
#[test]
fn v2_a_set_value_beats_the_default_and_rides_the_selected_dispatch_with_remedies_off() {
    let catalog = catalog(&[(VAR, 100)], None);
    let facts = facts(vec![record(
        &catalog,
        &[
            ("pick", json!("item_a")),
            ("order", json!(["item_c", "item_a"])),
        ],
    )]);
    let planned = plan_variable_values(&catalog, &facts, &table());
    assert_eq!(
        values_of(&planned),
        vec![
            ResolvedVariable {
                variable_name: "pick".to_owned(),
                value: json!("item_a"),
                source: VariableSource::Set,
                set_by: lab_set(),
                problem: None,
            },
            default_value("floor", json!(0)),
            ResolvedVariable {
                variable_name: "order".to_owned(),
                value: json!(["item_c", "item_a"]),
                source: VariableSource::Set,
                set_by: lab_set(),
                problem: None,
            },
            default_value("extra", json!(false)),
        ]
    );
    assert!(planned.not_applied(VAR, A).is_empty());

    let off = remedies_off(&catalog, &facts);
    assert_eq!(off, RemedyPlan::default());
    let plan = off.with_variable_values(&planned);
    let reason = plan
        .dispatch_parameters(VAR, A)
        .expect("t_var's dispatch parameters")
        .clone();
    assert_eq!(reason.outcome.code().as_str(), PARAMETERS);
    let mut expected = serde_json::Map::new();
    expected.insert(
        "variable_values".to_owned(),
        json!([
            {"variable_name": "pick", "variable_source": "set", "value_json": "\"item_a\"",
             "set_unix_ms": SET_AT, "set_channel": "lab"},
            {"variable_name": "floor", "variable_source": "default", "value_json": "0"},
            {"variable_name": "order", "variable_source": "set",
             "value_json": "[\"item_c\",\"item_a\"]", "set_unix_ms": SET_AT, "set_channel": "lab"},
            {"variable_name": "extra", "variable_source": "default", "value_json": "false"}
        ]),
    );
    assert_eq!(reason.outcome.values(), &expected, "{}", reason.outcome);
    let chain = dispatch_chain(&evaluate(&catalog, &facts, &plan), VAR);
    assert!(chain.contains(&reason), "{chain:#?}");
}

// Requirement 2 and the work order's test "each kind of invalid set value": a set value the
// declaration no longer takes reads the default, its record says `default` with the word and who
// set it, and it is the pair's one value not applied, with the declaration's sha256 and (fix round
// 1) the stored value's (what the host warns once per): a choice no longer offered, a value of
// another type, a number out of range or off its step, an ordered value whose offered keys are
// fewer than min_count once the others are left out, one too short or too long of offered keys
// only, and one that repeats a key. Fix round 1 (item 7, the design's table): an ordered value
// naming a removed key beside enough offered ones reads the rest in order, as set, its record
// marked `keys_left_out`, and it is no value not applied.
#[test]
fn v2_a_set_value_that_no_longer_fits_reads_the_default_with_its_word() {
    let catalog = catalog(&[(VAR, 100)], None);
    let declared = declaration();
    let declared_variables = variables(&declared);
    let defaults = declared_variables
        .iter()
        .map(|variable| {
            (
                variable.name.clone(),
                variable
                    .default_value()
                    .expect("a variable a person may set has a default"),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let table = VariableTable::build(&[task(
        VAR,
        Some(package(PACKAGE, declared_variables.clone())),
    )]);
    for (name, set, problem) in [
        ("pick", json!("item_c"), VariableProblem::NotAnOption),
        ("pick", json!(1), VariableProblem::TypeChanged),
        ("floor", json!(210), VariableProblem::OutOfRange),
        ("floor", json!(15), VariableProblem::OffStep),
        ("floor", json!("ten"), VariableProblem::TypeChanged),
        ("extra", json!("yes"), VariableProblem::TypeChanged),
        ("order", json!(["item_d"]), VariableProblem::KeysRemoved),
        ("order", json!([]), VariableProblem::CountOutOfRange),
        (
            "order",
            json!(["item_a", "item_b", "item_c"]),
            VariableProblem::CountOutOfRange,
        ),
        (
            "order",
            json!(["item_a", "item_a"]),
            VariableProblem::KeyRepeated,
        ),
        ("order", json!("item_a"), VariableProblem::TypeChanged),
    ] {
        let case = format!("{name} = {set}");
        let facts = facts(vec![record(&catalog, &[(name, set.clone())])]);
        let plan = plan_variable_values(&catalog, &facts, &table);
        let resolved = values_of(&plan)
            .into_iter()
            .find(|resolved| resolved.variable_name == name)
            .unwrap_or_else(|| panic!("{case}: the variable's value"));
        assert_eq!(
            resolved,
            ResolvedVariable {
                variable_name: name.to_owned(),
                value: defaults[name].clone(),
                source: VariableSource::Default,
                set_by: lab_set(),
                problem: Some(problem),
            },
            "{case}"
        );
        let not_applied = plan.not_applied(VAR, A);
        assert_eq!(not_applied.len(), 1, "{case}: {not_applied:?}");
        let variable = declared_variables
            .iter()
            .find(|variable| variable.name == name)
            .expect("the declaration");
        assert_eq!(
            (
                not_applied[0].package_id.as_str(),
                not_applied[0].variable_name.as_str(),
                not_applied[0].declaration_sha256.as_str(),
                not_applied[0].value_sha256.as_str(),
                not_applied[0].problem,
            ),
            (
                PACKAGE,
                name,
                variable.declaration_sha256().as_str(),
                sha256_of(&set).as_str(),
                problem
            ),
            "{case}"
        );
        let reason = RemedyPlan::default()
            .with_variable_values(&plan)
            .dispatch_parameters(VAR, A)
            .expect("t_var's dispatch parameters")
            .clone();
        let written = reason
            .outcome
            .value("variable_values")
            .and_then(|records| {
                records
                    .as_array()?
                    .iter()
                    .find(|written| written["variable_name"] == name)
                    .cloned()
            })
            .unwrap_or_else(|| panic!("{case}: the record"));
        assert_eq!(written["variable_source"], "default", "{case}");
        assert_eq!(
            written["value_json"],
            json!(defaults[name].to_string()),
            "{case}"
        );
        assert_eq!(written["variable_problem"], problem.as_str(), "{case}");
        assert_eq!(written["set_unix_ms"], json!(SET_AT), "{case}");
        assert_eq!(written["set_channel"], "lab", "{case}");
    }

    let facts = facts(vec![record(
        &catalog,
        &[("order", json!(["item_d", "item_c"]))],
    )]);
    let plan = plan_variable_values(&catalog, &facts, &table);
    let order = values_of(&plan)
        .into_iter()
        .find(|resolved| resolved.variable_name == "order")
        .expect("order's value");
    assert_eq!(
        order,
        ResolvedVariable {
            variable_name: "order".to_owned(),
            value: json!(["item_c"]),
            source: VariableSource::Set,
            set_by: lab_set(),
            problem: Some(VariableProblem::KeysLeftOut),
        }
    );
    assert!(plan.not_applied(VAR, A).is_empty());
    let written = RemedyPlan::default()
        .with_variable_values(&plan)
        .dispatch_parameters(VAR, A)
        .expect("t_var's dispatch parameters")
        .outcome
        .value("variable_values")
        .and_then(|records| {
            records
                .as_array()?
                .iter()
                .find(|written| written["variable_name"] == "order")
                .cloned()
        })
        .expect("order's record");
    assert_eq!(
        written,
        json!({"variable_name": "order", "variable_source": VariableSource::Set.as_str(),
               "value_json": "[\"item_c\"]", "set_unix_ms": SET_AT, "set_channel": "lab",
               "variable_problem": VariableProblem::KeysLeftOut.as_str()})
    );
}

/// `sha256:<hex>` of `value`'s compact JSON, as the plan hashes a stored value.
fn sha256_of(value: &Value) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{:x}", Sha256::digest(value.to_string().as_bytes()))
}

// Requirement 3 and the work order's test "a variable that does not apply": a variable the root
// check keeps from applying reads its default whatever is set, with the word: another package of
// the game declaring `pick` differently (`declaration_conflict`) and an instance of a game the
// table never judged (`root_unchecked`). Without a set value the default is read with no word.
// This is the table without a composed standard pack; the host's gate test judges a composed root.
// Workflow #455 G-SLx (register §1.501, adapted): the table no longer reads what a package provides
// (V2 and its fix round 1 gave a package of the game that marks a capability irreversible
// `irreversible_by_root` and a task whose package could not be read `package_unreadable`), so on
// the tables of those cases, a task of the game whose package could not be read included, the set
// value is read as set, with no word.
#[test]
fn v2_a_variable_that_does_not_apply_reads_its_default_whatever_is_set() {
    let catalog = catalog(&[(VAR, 100), (OTHER, 50)], None);
    let own = variables(&declaration());
    let mut differing = declaration();
    differing[0]["default"] = json!("item_a");
    let tables = [
        (
            "a conflicting declaration",
            vec![
                task(VAR, Some(package(PACKAGE, own.clone()))),
                task(OTHER, Some(package(OTHER_PACKAGE, variables(&differing)))),
            ],
            Some(VariableProblem::DeclarationConflict),
        ),
        (
            "beside a package that declares nothing",
            vec![
                task(VAR, Some(package(PACKAGE, own.clone()))),
                task(OTHER, Some(package(OTHER_PACKAGE, Vec::new()))),
            ],
            None,
        ),
        (
            "beside a package of the game that could not be read",
            vec![
                task(VAR, Some(package(PACKAGE, own.clone()))),
                task(OTHER, None),
            ],
            None,
        ),
        (
            "beside both",
            vec![
                task(VAR, Some(package(PACKAGE, own.clone()))),
                task(OTHER, Some(package(OTHER_PACKAGE, Vec::new()))),
                task("t_unread", None),
            ],
            None,
        ),
    ];
    let set = facts(vec![record(&catalog, &[("pick", json!("item_a"))])]);
    for (case, tasks, problem) in tables {
        let table = VariableTable::build(&tasks);
        let pick = values_of(&plan_variable_values(&catalog, &set, &table))
            .into_iter()
            .find(|resolved| resolved.variable_name == "pick")
            .expect("pick's value");
        let (value, source) = match problem {
            Some(_) => (json!("item_b"), VariableSource::Default),
            None => (json!("item_a"), VariableSource::Set),
        };
        assert_eq!(
            pick,
            ResolvedVariable {
                variable_name: "pick".to_owned(),
                value,
                source,
                set_by: lab_set(),
                problem,
            },
            "{case}"
        );
        let unset = plan_variable_values(&catalog, &facts(Vec::new()), &table);
        assert_eq!(
            values_of(&unset)[0],
            default_value("pick", json!("item_b")),
            "{case}: nothing set"
        );
        assert!(unset.not_applied(VAR, A).is_empty(), "{case}");
    }

    let elsewhere = VariableTable::build(&[VariableTableTask {
        task_id: VAR.to_owned(),
        games: BTreeSet::from(["game-b".to_owned()]),
        package: Some(package(PACKAGE, own)),
    }]);
    let pick = values_of(&plan_variable_values(&catalog, &set, &elsewhere))
        .into_iter()
        .find(|resolved| resolved.variable_name == "pick")
        .expect("pick's value");
    assert_eq!(pick.source, VariableSource::Default);
    assert_eq!(pick.problem, Some(VariableProblem::RootUnchecked));
}

// Requirement 8 and the work order's test "a package without variables": a table whose package
// declares no variable plans nothing, the plan it is added to stays the plan it was, and the
// evaluation's reason chains and decisions are those of the plan alone, byte for byte; with a set
// value of the instance too.
#[test]
fn v2_a_package_without_variables_gives_the_reason_chains_of_before() {
    let catalog = catalog(&[(VAR, 100)], None);
    let facts = facts(vec![record(&catalog, &[("pick", json!("item_a"))])]);
    let table = VariableTable::build(&[task(VAR, Some(package(PACKAGE, Vec::new())))]);
    assert!(table.is_empty());
    let planned = plan_variable_values(&catalog, &facts, &table);
    assert!(planned.is_empty());
    assert!(planned.values(VAR, A).is_none());
    let before = remedies_off(&catalog, &facts);
    let after = before.clone().with_variable_values(&planned);
    assert_eq!(after, before);
    let bytes = |evaluation: &PolicyEvaluation| {
        (
            serde_json::to_vec(&evaluation.reason_chains).expect("reason chains"),
            serde_json::to_vec(&evaluation.decisions).expect("decisions"),
            serde_json::to_vec(&evaluation.dispatch_intents).expect("dispatch intents"),
        )
    };
    let plain = evaluate(&catalog, &facts, &before);
    assert_eq!(bytes(&evaluate(&catalog, &facts, &after)), bytes(&plain));
    assert!(
        dispatch_chain(&plain, VAR)
            .iter()
            .all(|reason| reason.outcome.code().as_str() != PARAMETERS),
        "no dispatch parameters"
    );
}

// Fix round 1, item 1: the record's capacity does not decide what a run reads. An `ordered` value
// of twenty keys the declaration takes, whose compact JSON is longer than 256 bytes (and within
// the 1024 the goal store keeps), is applied as set, and its record carries it whole as
// `value_json`.
#[test]
fn v2_fix1_a_value_longer_than_256_bytes_is_applied_and_recorded_whole() {
    let catalog = catalog(&[(VAR, 100)], None);
    let keys = (0..20)
        .map(|index| format!("item_{index:02}_{}", "x".repeat(12)))
        .collect::<Vec<_>>();
    let names = keys.iter().map(String::as_str).collect::<Vec<_>>();
    let declared = variables(&json!([{
        "name": "many", "type": "ordered", "title": {"en": "Many"},
        "options": options(&names), "min_count": 1, "max_count": 20,
        "default": [names[0]]
    }]));
    let table = VariableTable::build(&[task(VAR, Some(package(PACKAGE, declared)))]);
    let set = json!(names);
    let text = set.to_string();
    assert!(text.len() > 256 && text.len() <= 1024, "{}", text.len());
    let facts = facts(vec![record(&catalog, &[("many", set.clone())])]);
    let plan = plan_variable_values(&catalog, &facts, &table);
    assert_eq!(
        values_of(&plan),
        vec![ResolvedVariable {
            variable_name: "many".to_owned(),
            value: set,
            source: VariableSource::Set,
            set_by: lab_set(),
            problem: None,
        }]
    );
    assert!(plan.not_applied(VAR, A).is_empty());
    let reason = RemedyPlan::default()
        .with_variable_values(&plan)
        .dispatch_parameters(VAR, A)
        .expect("t_var's dispatch parameters")
        .clone();
    assert_eq!(
        reason.outcome.value("variable_values"),
        Some(
            &json!([{"variable_name": "many", "variable_source": "set", "value_json": text,
                      "set_unix_ms": SET_AT, "set_channel": "lab"}])
        )
    );
}

// Fix round 1, item 6: a stored entry whose row says nothing readable of who set it (its
// `set_unix_ms` or its `channel`; the v3 reader holds both to their form, so the rows are given
// here as the plan would read them) is a stored value that was not applied, never taken as not
// set: the run reads the default, the record says `default` with `entry_unreadable` and no
// setter, and it is the pair's one value not applied, with the stored value's sha256.
#[test]
fn v2_fix1_a_stored_row_whose_setter_cannot_be_read_is_not_applied_with_its_word() {
    let catalog = catalog(&[(VAR, 100)], None);
    let stored = json!("item_a");
    let plan = crate::variables::plan_with_entries(&catalog, &facts(Vec::new()), &table(), |_| {
        BTreeMap::from([(
            "pick".to_owned(),
            crate::resource_targets::StoredEntry {
                value: stored.clone(),
                set_by: None,
            },
        )])
    });
    assert_eq!(
        values_of(&plan)[0],
        ResolvedVariable {
            variable_name: "pick".to_owned(),
            value: json!("item_b"),
            source: VariableSource::Default,
            set_by: None,
            problem: Some(VariableProblem::EntryUnreadable),
        }
    );
    let not_applied = plan.not_applied(VAR, A);
    assert_eq!(not_applied.len(), 1, "{not_applied:?}");
    assert_eq!(
        (
            not_applied[0].variable_name.as_str(),
            not_applied[0].value_sha256.as_str(),
            not_applied[0].problem,
        ),
        (
            "pick",
            sha256_of(&stored).as_str(),
            VariableProblem::EntryUnreadable
        )
    );
    let written = RemedyPlan::default()
        .with_variable_values(&plan)
        .dispatch_parameters(VAR, A)
        .expect("t_var's dispatch parameters")
        .outcome
        .value("variable_values")
        .and_then(|records| records.as_array()?.first().cloned())
        .expect("pick's record");
    assert_eq!(
        written,
        json!({"variable_name": "pick", "variable_source": "default", "value_json": "\"item_b\"",
               "variable_problem": VariableProblem::EntryUnreadable.as_str()})
    );
}

// Fix round 2, items 4, 5 and 9: the contract crate's one judgement of a root's variables
// (`judge_root_variables`, which the workbench, `standard-pack check` and the table without a
// composed standard pack call) gives each name its word and the detail of its cause. A declaration
// another package writes otherwise is `declaration_conflict`, naming that package; a name an
// entry of an accepted `goals.json` takes is `goal_name_conflict`, with the entry's type; the
// others apply, each verdict listing its packages and its declaration. A `goals.json` that cannot
// be read keeps every other name from applying (`goals_unreadable`, no detail), and a refused one
// takes no name. Workflow #455 G-SLx (register §1.501, adapted): the judgement takes no rule of
// irreversibility, so the root of a lone declaring package, which fix round 2 judged
// `irreversible_by_root` (with the capability another package marks) beside an unreadable
// package and `package_unreadable` (naming it) beside it alone, lets the name apply.
#[test]
fn v2_fix2_the_root_judgement_gives_each_name_its_word_and_its_cause() {
    let root_package = |package_id: &str, declared: Value| RootPackageVariables {
        package_id: package_id.to_owned(),
        variables: variables(&declared),
        cap_fact_keys: Vec::new(),
    };
    let cause = |judged: &RootVariablesJudged, name: &str| {
        judged
            .verdicts
            .iter()
            .find(|verdict| verdict.name == name)
            .unwrap_or_else(|| panic!("the verdict of {name}"))
            .not_applying
            .clone()
    };

    let pick = [root_package(PACKAGE, json!([declaration()[0].clone()]))];
    let judged = judge_root_variables(&pick, RootGoalDocument::Absent);
    assert!(cause(&judged, "pick").is_none(), "{judged:?}");
    assert!(judged.not_applying().is_empty());

    let mut differing = declaration();
    differing[0]["default"] = json!("item_a");
    let packages = [
        root_package(PACKAGE, declaration()),
        root_package(OTHER_PACKAGE, json!([differing[0].clone()])),
    ];
    let goals = parse_standard_pack_goals(
        &serde_json::to_vec(&json!({
            "schema_version": "actingcommand.goals.v2",
            "game": "neutral",
            "entries": [{"name": "floor", "type": "heading", "title": {"en": "Floor"}}]
        }))
        .expect("goals bytes"),
        "neutral",
    )
    .unwrap_or_else(|issues| panic!("the goal document reads: {issues:?}"));
    let judged = judge_root_variables(&packages, RootGoalDocument::Accepted(&goals));
    let names = judged
        .verdicts
        .iter()
        .map(|verdict| (verdict.name.as_str(), verdict.applies()))
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            ("pick", false),
            ("floor", false),
            ("order", true),
            ("extra", true)
        ]
    );
    assert_eq!(
        judged.verdicts[0].packages,
        [PACKAGE.to_owned(), OTHER_PACKAGE.to_owned()]
    );
    assert_eq!(judged.verdicts[0].declaration, variables(&declaration())[0]);
    assert_eq!(
        cause(&judged, "pick"),
        Some(NotApplyingCause {
            problem: VariableProblem::DeclarationConflict,
            cause: VariableCause::DeclarationConflict {
                packages: vec![OTHER_PACKAGE.to_owned()],
            },
        })
    );
    assert_eq!(
        cause(&judged, "floor"),
        Some(NotApplyingCause {
            problem: VariableProblem::GoalNameConflict,
            cause: VariableCause::GoalEntry {
                goal_type: Some(GoalType::Heading),
            },
        })
    );
    assert_eq!(judged.verdicts[2].packages, [PACKAGE.to_owned()]);

    let judged = judge_root_variables(&packages, RootGoalDocument::Unreadable);
    assert_eq!(
        judged.not_applying(),
        BTreeMap::from([
            ("extra".to_owned(), VariableProblem::GoalsUnreadable),
            ("floor".to_owned(), VariableProblem::GoalsUnreadable),
            ("order".to_owned(), VariableProblem::GoalsUnreadable),
            ("pick".to_owned(), VariableProblem::DeclarationConflict),
        ])
    );
    assert_eq!(
        cause(&judged, "floor").map(|found| found.cause),
        Some(VariableCause::WordOnly)
    );
    let judged = judge_root_variables(&packages, RootGoalDocument::Refused);
    assert_eq!(
        judged.not_applying(),
        BTreeMap::from([("pick".to_owned(), VariableProblem::DeclarationConflict)])
    );
}

// Fix round 2, item 7: the record's size. A package declares sixteen `ordered` variables (the most
// a package declares), each set to a value of 1024 bytes (the most the goal store keeps; 32 keys,
// so the most quotes to escape), with 64-byte names, and its dispatch's
// `policy_dispatch_parameters` also carries 64 run parameters of 64-byte names and values (the
// most `provides` declares). Alone, the sixteen values fit the bound of an outcome record
// (`ENVELOPE_BODY_BUDGET`) and the record is the reason as it is. With the parameters the reason
// passes the bound: its recorded form carries, for the first value (the longest first; all are
// equal), the text's sha256 and length in place of the text and says so, fits the bound, and
// keeps every other value's text and the parameters; the reason itself, which the run reads,
// keeps every value whole.
#[test]
fn v2_fix2_a_reason_past_the_record_bound_is_recorded_with_hashes_and_kept_whole() {
    let catalog = catalog(&[(VAR, 100)], None);
    let keys = (0..32)
        .map(|index| {
            let filler = if index < 31 { 25 } else { 24 };
            format!("k{index:02}_{}", "x".repeat(filler))
        })
        .collect::<Vec<_>>();
    let set = json!(keys);
    let text = set.to_string();
    assert_eq!(text.len(), 1024);
    let names = (0..16)
        .map(|index| format!("v{index:02}_{}", "n".repeat(60)))
        .collect::<Vec<_>>();
    let declared = names
        .iter()
        .map(|name| {
            json!({"name": name, "type": "ordered", "title": {"en": "Many"},
                   "options": options(&keys.iter().map(String::as_str).collect::<Vec<_>>()),
                   "min_count": 1, "max_count": 32, "default": [keys[0]]})
        })
        .collect::<Vec<_>>();
    let table = VariableTable::build(&[task(
        VAR,
        Some(package(PACKAGE, variables(&json!(declared)))),
    )]);
    let plan = crate::variables::plan_with_entries(&catalog, &facts(Vec::new()), &table, |_| {
        names
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    crate::resource_targets::StoredEntry {
                        value: set.clone(),
                        set_by: lab_set(),
                    },
                )
            })
            .collect()
    });
    let size = |outcome: &Outcome| serde_json::to_vec(outcome).expect("the record form").len();

    let alone = RemedyPlan::default()
        .with_variable_values(&plan)
        .dispatch_parameters(VAR, A)
        .expect("t_var's dispatch parameters")
        .outcome
        .clone();
    assert!(size(&alone) <= ENVELOPE_BODY_BUDGET, "{}", size(&alone));
    assert_eq!(recorded_dispatch_parameters(&alone), alone);

    let parameters = (0..64).map(|index| {
        Record::new()
            .parameter_name(format!("p{index:02}_{}", "x".repeat(60)))
            .parameter_value(format!("o{index:02}_{}", "y".repeat(60)))
            .remedy_source(RemedySource::Configuration)
    });
    let mut remedies = RemedyPlan::default();
    let parameters_only =
        Outcome::new(PolicyCode::PolicyDispatchParameters).run_parameters(parameters);
    remedies.run_parameters.insert(
        (VAR.to_owned(), A.to_owned()),
        DecisionReason {
            detail: parameters_only.to_string(),
            outcome: parameters_only,
        },
    );
    let whole = remedies
        .with_variable_values(&plan)
        .dispatch_parameters(VAR, A)
        .expect("t_var's dispatch parameters")
        .outcome
        .clone();
    assert!(size(&whole) > ENVELOPE_BODY_BUDGET, "{}", size(&whole));
    let recorded = recorded_dispatch_parameters(&whole);
    assert!(
        size(&recorded) <= ENVELOPE_BODY_BUDGET,
        "{}",
        size(&recorded)
    );
    assert_eq!(
        recorded.value("run_parameters"),
        whole.value("run_parameters")
    );
    let records = recorded
        .value("variable_values")
        .and_then(Value::as_array)
        .expect("the recorded values");
    assert_eq!(records.len(), 16);
    assert_eq!(
        records[0],
        json!({"variable_name": names[0], "variable_source": "set",
               "value_sha256": sha256_of(&set), "value_bytes": 1024,
               "value_json_left_out": true, "set_unix_ms": SET_AT, "set_channel": "lab"})
    );
    for record in &records[1..] {
        assert_eq!(record["value_json"], json!(text), "{record}");
        assert!(record.get("value_sha256").is_none(), "{record}");
    }
    let kept_whole = whole
        .value("variable_values")
        .and_then(Value::as_array)
        .expect("the reason's values")
        .iter()
        .filter(|record| record["value_json"] == json!(text))
        .count();
    assert_eq!(kept_whole, 16, "the reason the run reads keeps every value");
}
