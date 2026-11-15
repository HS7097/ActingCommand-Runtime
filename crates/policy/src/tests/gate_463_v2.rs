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

use actingcommand_contract::{
    DeclaredCapabilities, PackageVariable, RuntimeChannel, VariableProblem, VariableSource,
    parse_package_variables,
};
use serde_json::{Value, json};

use super::selection_catalog::{NOW, catalog, instance};
use crate::{
    CandidateEligibility, CompiledCatalog, DecisionReason, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactRefreshInputs, FactRefreshSettings, FactValue, HostResourceSnapshot,
    ObservedFact, PolicyEvaluation, PolicyEvaluationError, RemedyInputs, RemedyPlan, RemedyTable,
    ResolvedVariable, ScopeSelector, TargetSetter, VariablePackage, VariablePlan, VariableSetBy,
    VariableTable, VariableTableTask, check_resource_targets, evaluate_with_remedies,
    parse_resource_targets, plan_remedies, plan_variable_values,
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

/// What a package provides and what its remedies list: nothing.
fn nothing() -> DeclaredCapabilities {
    DeclaredCapabilities {
        provides: BTreeMap::new(),
        remedies: BTreeSet::new(),
    }
}

/// A task of the game with `package` (`None`: its package could not be read).
fn task(task_id: &str, package: Option<VariablePackage>) -> VariableTableTask {
    VariableTableTask {
        task_id: task_id.to_owned(),
        games: BTreeSet::from([GAME.to_owned()]),
        package,
    }
}

fn package(
    package_id: &str,
    variables: Vec<PackageVariable>,
    capabilities: DeclaredCapabilities,
) -> VariablePackage {
    VariablePackage {
        package_id: package_id.to_owned(),
        variables,
        cap_fact_keys: Vec::new(),
        capabilities,
    }
}

/// The table of `t_var` with the four variables alone.
fn table() -> VariableTable {
    VariableTable::build(&[task(
        VAR,
        Some(package(PACKAGE, variables(&declaration()), nothing())),
    )])
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
// set it, and it is the pair's one value not applied, with the declaration's sha256 (what the host
// warns once per): a choice no longer offered, a value of another type, a number out of range or
// off its step, an ordered value whose offered keys are fewer than min_count once the others are
// left out, one too short or too long of offered keys only, one that repeats a key, and one whose
// text is longer than a reason carries. An ordered value naming a removed key beside offered ones
// reads the rest in order, as set.
#[test]
fn v2_a_set_value_that_no_longer_fits_reads_the_default_with_its_word() {
    let catalog = catalog(&[(VAR, 100)], None);
    let long_keys = (0..5)
        .map(|index| format!("item_{index}_{}", "x".repeat(56)))
        .collect::<Vec<_>>();
    let long_names = long_keys.iter().map(String::as_str).collect::<Vec<_>>();
    let mut declared = declaration();
    declared.as_array_mut().expect("the variables").push(json!({
        "name": "long", "type": "ordered", "title": {"en": "Long"},
        "options": options(&long_names), "min_count": 1, "max_count": 5,
        "default": [long_names[0]]
    }));
    let declared_variables = variables(&declared);
    let defaults = declared_variables
        .iter()
        .map(|variable| (variable.name.clone(), variable.default_value()))
        .collect::<BTreeMap<_, _>>();
    let table = VariableTable::build(&[task(
        VAR,
        Some(package(PACKAGE, declared_variables.clone(), nothing())),
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
        ("long", json!(long_names), VariableProblem::ValueTooLong),
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
                not_applied[0].problem,
            ),
            (
                PACKAGE,
                name,
                variable.declaration_sha256().as_str(),
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
            problem: None,
        }
    );
    assert!(plan.not_applied(VAR, A).is_empty());
}

// Requirement 3 and the work order's test "a variable that does not apply": a variable the root
// check keeps from applying reads its default whatever is set, with the word: another package of
// the game declaring `pick` differently (`declaration_conflict`), a package of the game marking
// irreversible a capability the declaring package provides unmarked (`irreversible_by_root`), a
// task of the game whose package could not be read (`irreversible_by_root`, the safe side), and an
// instance of a game the table never judged (`root_unchecked`). Without a set value the default is
// read with no word.
#[test]
fn v2_a_variable_that_does_not_apply_reads_its_default_whatever_is_set() {
    let catalog = catalog(&[(VAR, 100), (OTHER, 50)], None);
    let own = variables(&declaration());
    let mut differing = declaration();
    differing[0]["default"] = json!("item_a");
    let provides = |marked: bool| DeclaredCapabilities {
        provides: BTreeMap::from([("cap_x".to_owned(), marked)]),
        remedies: BTreeSet::new(),
    };
    let tables = [
        (
            "a conflicting declaration",
            vec![
                task(VAR, Some(package(PACKAGE, own.clone(), nothing()))),
                task(
                    OTHER,
                    Some(package(OTHER_PACKAGE, variables(&differing), nothing())),
                ),
            ],
            VariableProblem::DeclarationConflict,
        ),
        (
            "irreversible by the root",
            vec![
                task(VAR, Some(package(PACKAGE, own.clone(), provides(false)))),
                task(
                    OTHER,
                    Some(package(OTHER_PACKAGE, Vec::new(), provides(true))),
                ),
            ],
            VariableProblem::IrreversibleByRoot,
        ),
        (
            "a package of the game that could not be read",
            vec![
                task(VAR, Some(package(PACKAGE, own.clone(), nothing()))),
                task(OTHER, None),
            ],
            VariableProblem::IrreversibleByRoot,
        ),
    ];
    let set = facts(vec![record(&catalog, &[("pick", json!("item_a"))])]);
    for (case, tasks, problem) in tables {
        let table = VariableTable::build(&tasks);
        let pick = values_of(&plan_variable_values(&catalog, &set, &table))
            .into_iter()
            .find(|resolved| resolved.variable_name == "pick")
            .expect("pick's value");
        assert_eq!(
            pick,
            ResolvedVariable {
                variable_name: "pick".to_owned(),
                value: json!("item_b"),
                source: VariableSource::Default,
                set_by: lab_set(),
                problem: Some(problem),
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
        package: Some(package(PACKAGE, own, nothing())),
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
    let table = VariableTable::build(&[task(VAR, Some(package(PACKAGE, Vec::new(), nothing())))]);
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
