// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #500 SV-2 (0.12.81; register §1.511; contracts/standard-pack-schedule.md, "A rule's
// variable values"; contracts/package-variables.md, "The run"; coordinator work order
// rd-1123/brief-500-sv2.md, its rulings 1 to 3, 4's plan, 8 and its tests 1 to 6 and 9), the
// policy half: a rule of `schedule.json` carries values for its package's variables.
// - two rules of one package, each with its own value for the same variable: each pair's record
//   hands its rule's value with the source `rule`, and a variable a rule does not carry resolves as
//   before, set on the application, else the default (tests 1, 2);
// - a value set on the application equal to one rule's and other than the other's holds the other
//   back by that variable, and none once cleared (tests 3, 4, the plan's half);
// - a name the package does not declare, and a value its declaration refuses, one case for each of
//   the four kinds, and an empty member, are refused `schedule_invalid` at the member (test 5);
// - the member without `rule_variables` in `requires` is refused at the member; with it and given
//   subsystems that do not name it, `schedule_requires_missing` at that entry (test 6);
// - a root none of whose rules carries the member composes to its golden bytes (test 9);
// - fix round 1: a value a task carries that the declaration in force no longer takes holds the
//   task's pair back by that variable, with the judgement's word.
// A made-up game; every package, variable and value is a placeholder. No clock is read.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{
    DeclaredCapabilities, IrreversiblePackages, PackageVariable, RuntimeChannel,
    STANDARD_PACK_SCHEDULE_FILE, StandardPackProblem, StandardPackRootBytes, StandardPackRootIssue,
    VariableProblem, parse_package_variables, read_standard_pack_root,
};
use serde_json::{Map, Value, json};

use crate::resource_targets::StoredEntry;
use crate::{
    EvaluationFacts, InstanceSnapshot, RULE_VARIABLES_SUBSYSTEM, RefusedRuleValue, RemedyPlan,
    STANDARD_PACK_OPERATION_ID, StandardPackBinding, StandardPackComposeFailure,
    StandardPackComposeInput, StandardPackComposition, StandardPackGameFiles, VariablePackage,
    VariablePlan, VariableSetBy, VariableTable, VariableTableTask, compose_standard_pack_rules,
};

const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const INSTANCE: &str = "fixture-instance-a";
/// The one package, which declares the four variables of [`declaration`].
const PACKAGE: &str = "pack.vars";
/// Its two rules: the first carries `pick` = `item_a`, the second `pick` = `item_b`.
const RULE_A: &str = "pack.vars.a";
const RULE_B: &str = "pack.vars.b";
/// When the fixture's values were set.
const SET_AT: u64 = 1_000;

fn options(items: &[&str]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|item| json!({"value": item, "title": {"en": item}}))
            .collect(),
    )
}

/// The package's variables: `pick` (a choice of item_a, item_b and item_c, default item_c),
/// `count` (an integer 0 to 10 on steps of 2, default 0), `extra` (a switch, default false) and
/// `order` (ordered, one or two of item_a and item_b, default item_a).
fn declaration() -> Value {
    json!([
        {"name": "pick", "type": "choice", "title": {"en": "Item"},
         "options": options(&["item_a", "item_b", "item_c"]), "default": "item_c"},
        {"name": "count", "type": "integer", "title": {"en": "Count"},
         "min": 0, "max": 10, "step": 2, "default": 0},
        {"name": "extra", "type": "switch", "title": {"en": "Extra"}, "default": false},
        {"name": "order", "type": "ordered", "title": {"en": "Order"},
         "options": options(&["item_a", "item_b"]), "min_count": 1, "max_count": 2,
         "default": ["item_a"]}
    ])
}

fn variables() -> Vec<PackageVariable> {
    parse_package_variables(&declaration(), &[]).expect("the fixture's variables")
}

/// A `schedule.json` rule `id` of [`PACKAGE`] on an hourly clock, carrying `carried` when given.
fn rule(id: &str, carried: Option<Value>) -> Value {
    let mut written = json!({
        "id": id,
        "package_id": PACKAGE,
        "priority": 100,
        "trigger": {
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": 3_600_000,
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
        "consumes": [],
        "produces": [],
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": 86_400_000,
        "yield_points": [],
        "expected_duration_ms": 60000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "light"},
        "strategic_weight_milli": 1000
    });
    if let Some(carried) = carried {
        written["variables"] = carried;
    }
    written
}

/// A `schedule.json` of `rules`, naming `requires`.
fn schedule(rules: Vec<Value>, requires: &[&str]) -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": rules
    })
}

/// The fixture's two rules, each with its own `pick`, naming `rule_variables`.
fn two_rules() -> Value {
    schedule(
        vec![
            rule(RULE_A, Some(json!({"pick": "item_a"}))),
            rule(RULE_B, Some(json!({"pick": "item_b"}))),
        ],
        &[RULE_VARIABLES_SUBSYSTEM],
    )
}

/// Composes the `schedule.json` `document` for one instance of the game, whose registry holds
/// `packages`, each read declaring [`declaration`]'s variables, with the subsystems `subsystems`
/// when given.
fn compose_with(
    packages: &[&str],
    document: &Value,
    subsystems: Option<&BTreeSet<String>>,
) -> Result<StandardPackComposition, StandardPackComposeFailure> {
    let bundle = json!({
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
    let irreversible = IrreversiblePackages::new(
        root.registry
            .package_ids()
            .map(|package_id| {
                (
                    package_id.to_owned(),
                    Some(DeclaredCapabilities {
                        provides: BTreeMap::new(),
                        remedies: BTreeSet::new(),
                        variables: Some(variables()),
                    }),
                )
            })
            .collect(),
    );
    let bytes = serde_json::to_vec(document).expect("schedule bytes");
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &bytes,
            root: &root,
            maintenance: &[],
            irreversible: &irreversible,
            catalog: None,
        },
    )]);
    let instances = BTreeMap::from([(
        INSTANCE.to_owned(),
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

/// [`compose_with`] of the fixture's one package and no Runtime manifest.
fn compose(document: &Value) -> Result<StandardPackComposition, StandardPackComposeFailure> {
    compose_with(&[PACKAGE], document, None)
}

/// The first refusal of composing `document` with `subsystems`, as the composer names it.
fn first_issue(document: &Value, subsystems: Option<&BTreeSet<String>>) -> StandardPackRootIssue {
    let failure = compose_with(&[PACKAGE], document, subsystems).expect_err("the root is refused");
    let first = failure.issues().first().expect("an issue");
    assert_eq!(first.game_id.as_deref(), Some(GAME), "{failure}");
    first.issue.clone()
}

/// A refusal of `schedule.json` with the document's own word at `pointer`.
fn invalid_at(pointer: &str) -> StandardPackRootIssue {
    StandardPackRootIssue {
        file: STANDARD_PACK_SCHEDULE_FILE,
        problem: StandardPackProblem::ScheduleInvalid,
        pointer: pointer.to_owned(),
    }
}

/// The one instance, bound to the game on its server, as the composed tasks' scope names it.
fn facts() -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-rule-variables".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![InstanceSnapshot {
            instance_id: INSTANCE.to_owned(),
            server_id: SERVER.to_owned(),
            game_id: GAME.to_owned(),
            host_id: "fixture-host-a".to_owned(),
            available: true,
            capability_operation_ids: vec![STANDARD_PACK_OPERATION_ID.to_owned()],
            preferred_task_ids: Vec::new(),
        }],
        priority_offsets: Vec::new(),
    }
}

/// The table of the two rules' tasks, each running [`PACKAGE`] with its four variables.
fn table() -> VariableTable {
    let task = |task_id: &str| VariableTableTask {
        task_id: task_id.to_owned(),
        games: BTreeSet::from([GAME.to_owned()]),
        package: Some(VariablePackage {
            package_id: PACKAGE.to_owned(),
            variables: variables(),
            cap_fact_keys: Vec::new(),
        }),
    };
    VariableTable::build(&[task(RULE_A), task(RULE_B)])
}

/// The plan of [`two_rules`]' composition with the application's entry rows `entries`, each set
/// through the Lab at [`SET_AT`].
fn plan(entries: &[(&str, Value)]) -> VariablePlan {
    let composition =
        compose(&two_rules()).unwrap_or_else(|failure| panic!("the rules compose: {failure}"));
    let stored = entries
        .iter()
        .map(|(name, value)| {
            (
                (*name).to_owned(),
                StoredEntry {
                    value: value.clone(),
                    set_by: Some(VariableSetBy {
                        set_unix_ms: SET_AT,
                        channel: RuntimeChannel::Lab,
                    }),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    crate::variables::plan_with_entries(composition.compiled(), &facts(), &table(), |_| {
        stored.clone()
    })
}

/// The `variable_values` a dispatch of `task_id` on the instance carries in `plan`.
fn records(plan: &VariablePlan, task_id: &str) -> Value {
    RemedyPlan::default()
        .with_variable_values(plan)
        .dispatch_parameters(task_id, INSTANCE)
        .unwrap_or_else(|| panic!("{task_id}'s dispatch parameters"))
        .outcome
        .value("variable_values")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The three variables neither rule carries, as a dispatch hands them with nothing set: each
/// package default.
fn defaults() -> [Value; 3] {
    [
        json!({"variable_name": "count", "variable_source": "default", "value_json": "0"}),
        json!({"variable_name": "extra", "variable_source": "default", "value_json": "false"}),
        json!({"variable_name": "order", "variable_source": "default",
               "value_json": "[\"item_a\"]"}),
    ]
}

/// Tests 1 and 2 (rulings 1 and 3): the two rules of one package compose, each task carrying its
/// rule's value; with nothing set on the application each dispatch hands its run its own rule's
/// `pick` with the source `rule` and every other variable its default, as before. With `count` set
/// on the application, which neither rule carries, both hand the set value with who set it, and
/// each still its own `pick`. A value a rule carries is no value not applied.
#[test]
fn sv2_t1_two_rules_of_one_package_hand_their_runs_their_own_values() {
    let composition =
        compose(&two_rules()).unwrap_or_else(|failure| panic!("the rules compose: {failure}"));
    let tasks = &composition.compiled().catalog().tasks.tasks;
    let carried = |id: &str| {
        tasks
            .iter()
            .find(|task| task.id == id)
            .unwrap_or_else(|| panic!("the task {id}: {tasks:?}"))
            .variables
            .clone()
    };
    assert_eq!(
        carried(RULE_A),
        Some(BTreeMap::from([("pick".to_owned(), json!("item_a"))]))
    );
    assert_eq!(
        carried(RULE_B),
        Some(BTreeMap::from([("pick".to_owned(), json!("item_b"))]))
    );

    let nothing_set = plan(&[]);
    for (task_id, own) in [(RULE_A, "\"item_a\""), (RULE_B, "\"item_b\"")] {
        let mut expected = vec![json!({"variable_name": "pick", "variable_source": "rule",
                                       "value_json": own})];
        expected.extend(defaults());
        assert_eq!(
            records(&nothing_set, task_id),
            Value::Array(expected),
            "{task_id}"
        );
        assert!(nothing_set.not_applied(task_id, INSTANCE).is_empty());
        assert_eq!(nothing_set.excluded_by(task_id, INSTANCE), None);
    }
    assert_eq!(
        nothing_set.carried(INSTANCE),
        BTreeSet::from(["pick"]),
        "the variable the rules carry"
    );

    let count_set = plan(&[("count", json!(4))]);
    for (task_id, own) in [(RULE_A, "\"item_a\""), (RULE_B, "\"item_b\"")] {
        let [_, extra, order] = defaults();
        assert_eq!(
            records(&count_set, task_id),
            json!([
                {"variable_name": "pick", "variable_source": "rule", "value_json": own},
                {"variable_name": "count", "variable_source": "set", "value_json": "4",
                 "set_unix_ms": SET_AT, "set_channel": "lab"},
                extra,
                order
            ]),
            "{task_id}"
        );
        assert_eq!(count_set.excluded_by(task_id, INSTANCE), None);
    }
}

/// Tests 3 and 4, the plan's half (ruling 4): with `pick` set to `item_a` on the application, the
/// rule that carries `item_a` hands it (source `rule`, with who set the application's value) and
/// is not held back, while the rule that carries `item_b` is held back by `pick`; a value set for
/// another variable holds neither back; cleared, neither is held back. The goal page's own
/// resolution of `pick` is the set value either way.
#[test]
fn sv2_t3_a_set_value_holds_back_the_rule_that_carries_another() {
    let set_a = plan(&[("pick", json!("item_a"))]);
    assert_eq!(set_a.excluded_by(RULE_A, INSTANCE), None);
    assert_eq!(set_a.excluded_by(RULE_B, INSTANCE), Some("pick"));
    let mut expected = vec![json!({"variable_name": "pick", "variable_source": "rule",
                                   "value_json": "\"item_a\"", "set_unix_ms": SET_AT,
                                   "set_channel": "lab"})];
    expected.extend(defaults());
    assert_eq!(records(&set_a, RULE_A), Value::Array(expected));
    for task_id in [RULE_A, RULE_B] {
        let resolved = set_a
            .resolutions(task_id, INSTANCE)
            .unwrap_or_else(|| panic!("{task_id}'s resolutions"));
        assert_eq!(resolved[0].variable_name, "pick");
        assert_eq!(resolved[0].value, json!("item_a"), "{task_id}");
        assert_eq!(
            resolved[0].source,
            actingcommand_contract::VariableSource::Set
        );
    }

    let set_b = plan(&[("pick", json!("item_b"))]);
    assert_eq!(set_b.excluded_by(RULE_A, INSTANCE), Some("pick"));
    assert_eq!(set_b.excluded_by(RULE_B, INSTANCE), None);

    let other_set = plan(&[("extra", json!(true))]);
    let cleared = plan(&[]);
    for task_id in [RULE_A, RULE_B] {
        assert_eq!(other_set.excluded_by(task_id, INSTANCE), None, "{task_id}");
        assert_eq!(cleared.excluded_by(task_id, INSTANCE), None, "{task_id}");
    }
}

/// Test 5 (ruling 1): a rule that names a variable its package does not declare, or carries a
/// value the declaration refuses, is refused where the root is composed, `schedule_invalid` at
/// the member, one case for each of the four kinds (a `choice` that is no option, an `integer`
/// out of range and one off its step, a `switch` that is no boolean, an `ordered` value of a key
/// the declaration does not offer, which it would take only in part, and one of fewer keys than
/// `min_count`), and an empty member, or one that is no object, at the member itself.
#[test]
fn sv2_t5_an_undeclared_name_or_a_refused_value_is_refused_at_the_member() {
    let cases = [
        (
            "an undeclared name",
            json!({"other": 1}),
            "/rules/1/variables/other",
        ),
        (
            "a choice",
            json!({"pick": "item_z"}),
            "/rules/1/variables/pick",
        ),
        (
            "an integer out of range",
            json!({"count": 12}),
            "/rules/1/variables/count",
        ),
        (
            "an integer off its step",
            json!({"count": 3}),
            "/rules/1/variables/count",
        ),
        (
            "a switch",
            json!({"extra": "yes"}),
            "/rules/1/variables/extra",
        ),
        (
            "an ordered key not offered",
            json!({"order": ["item_a", "item_z"]}),
            "/rules/1/variables/order",
        ),
        (
            "an ordered count",
            json!({"order": []}),
            "/rules/1/variables/order",
        ),
        ("an empty member", json!({}), "/rules/1/variables"),
        ("no object", json!(["pick"]), "/rules/1/variables"),
    ];
    for (case, carried, pointer) in cases {
        let refused = schedule(
            vec![
                rule(RULE_A, Some(json!({"pick": "item_a"}))),
                rule(RULE_B, Some(carried)),
            ],
            &[RULE_VARIABLES_SUBSYSTEM],
        );
        assert_eq!(first_issue(&refused, None), invalid_at(pointer), "{case}");
    }
}

/// Test 6 (ruling 2): a rule that carries the member in a file whose `requires` does not name
/// `rule_variables` is refused at the member; with the name and subsystems that do not name it
/// (a Runtime that does not know it), the file is refused `schedule_requires_missing` at that
/// entry; subsystems that name it compose the file.
#[test]
fn sv2_t6_the_member_needs_rule_variables_in_requires() {
    let unnamed = schedule(
        vec![
            rule(RULE_A, None),
            rule(RULE_B, Some(json!({"pick": "item_b"}))),
        ],
        &[],
    );
    assert_eq!(
        first_issue(&unnamed, None),
        invalid_at("/rules/1/variables")
    );

    let older = BTreeSet::from(["standard_pack_schedule".to_owned()]);
    assert_eq!(
        first_issue(&two_rules(), Some(&older)),
        StandardPackRootIssue {
            file: STANDARD_PACK_SCHEDULE_FILE,
            problem: StandardPackProblem::ScheduleRequiresMissing,
            pointer: "/requires/0".to_owned(),
        }
    );

    let named = BTreeSet::from([RULE_VARIABLES_SUBSYSTEM.to_owned()]);
    compose_with(&[PACKAGE], &two_rules(), Some(&named))
        .unwrap_or_else(|failure| panic!("a Runtime that names it composes: {failure}"));
}

/// Test 9 (ruling 8, must not change): the neutral example catalog `catalog-a`'s task as the rule
/// of a root none of whose rules carries `variables` composes to its golden tasks document, each
/// member in its place, as on main; its compiled task carries no `variables`, and (fix round 1:
/// now asserted, as the review asked) a dispatch of it, its package declaring no variable, has no
/// pair in the plan and carries no `policy_dispatch_parameters` record.
#[test]
fn sv2_t9_a_root_without_the_member_keeps_its_golden_bytes() {
    let parse = |text: &str| -> Value { serde_json::from_str(text).expect("example JSON") };
    let tasks = parse(include_str!(
        "../../../../contracts/scheduling/examples/catalog-a/tasks.json"
    ));
    let pools = parse(include_str!(
        "../../../../contracts/scheduling/examples/catalog-a/pools.json"
    ));
    let mut example: Map<String, Value> = tasks["tasks"][0].as_object().cloned().expect("the task");
    for member in ["scope", "entrypoint", "procedure_ref", "instance_overrides"] {
        example.shift_remove(member);
    }
    if let Some(Value::Object(fact)) = example
        .get_mut("trigger")
        .and_then(|trigger| trigger.get_mut("predicates"))
        .and_then(|predicates| predicates.get_mut(1))
    {
        fact.shift_remove("scope");
    }
    example.insert("yield_points".to_owned(), json!([]));
    example.insert("package_id".to_owned(), json!("fixture.observe"));
    let mut fixture_pool = pools["pools"][0].as_object().cloned().expect("the pool");
    fixture_pool.shift_remove("scope");
    let plain = json!({
        "schema_version": "actingcommand.schedule.v1",
        "rules": [example],
        "pools": [fixture_pool]
    });
    let composed = compose_with(&["fixture.observe"], &plain, None)
        .unwrap_or_else(|failure| panic!("the example composes: {failure}"));
    let golden = json!({
        "schema_version": "actingcommand.scheduling.v2",
        "catalog": {"catalog_id": "standard-packs", "catalog_version": 1, "approval_refs": []},
        "tasks": [{
            "id": "fixture.observe",
            "scope": {"kind": "instance", "instance_id": INSTANCE},
            "entrypoint": {"operation_id": "operation.package.run"},
            "procedure_ref": "package.fixture.observe",
            "priority": 100,
            "trigger": {
                "kind": "any",
                "predicates": [
                    {
                        "kind": "clock",
                        "schedule": {
                            "kind": "interval",
                            "clock_source": {"kind": "local"},
                            "every_ms": 3_600_000,
                            "anchor_ms": 0
                        }
                    },
                    {
                        "kind": "fact",
                        "fact_key": "resource.primary",
                        "comparison": "less_than",
                        "value": {"type": "integer", "value": 20},
                        "max_age_ms": 900_000,
                        "scope": {"kind": "instance", "instance_id": INSTANCE}
                    }
                ]
            },
            "feedback_stop": {
                "kind": "outcome",
                "task_id": "fixture.observe",
                "outcome_key": "completed",
                "comparison": "eq",
                "value": {"type": "boolean", "value": true}
            },
            "consumes": [],
            "produces": [{
                "pool_id": "fixture-pool-a",
                "direction": "produce",
                "amount": 1,
                "observation_source": "scan_verified",
                "confidence_milli": 1000
            }],
            "on_failure": {
                "action": "continue",
                "retry_limit": 2,
                "retry_backoff_ms": 30000,
                "escalation_threshold": 3
            },
            "sensitive": false,
            "next_run_clamp_ms": 86_400_000,
            "yield_points": [],
            "expected_duration_ms": 60000,
            "cooldown_ms": 1000,
            "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
            "loop_budget": {
                "daily_limit": 24,
                "window_iteration_limit": 4,
                "max_runtime_ms": 300_000
            },
            "strategic_weight_milli": 1000,
            "instance_overrides": [],
            "benefit_judged": true
        }]
    });
    assert_eq!(
        String::from_utf8_lossy(&composed.sources().tasks.bytes),
        String::from_utf8_lossy(&serde_json::to_vec_pretty(&golden).expect("golden bytes"))
    );
    let compiled = &composed.compiled().catalog().tasks.tasks;
    assert_eq!(compiled.len(), 1);
    assert_eq!(compiled[0].variables, None);
    let serialized = serde_json::to_value(&compiled[0]).expect("the compiled task serializes");
    assert!(
        serialized.get("variables").is_none(),
        "no member is added: {serialized}"
    );
    let without = VariableTable::build(&[VariableTableTask {
        task_id: "fixture.observe".to_owned(),
        games: BTreeSet::from([GAME.to_owned()]),
        package: Some(VariablePackage {
            package_id: "fixture.observe".to_owned(),
            variables: Vec::new(),
            cap_fact_keys: Vec::new(),
        }),
    }]);
    let planned =
        crate::variables::plan_with_entries(composed.compiled(), &facts(), &without, |_| {
            BTreeMap::new()
        });
    assert!(planned.is_empty(), "{planned:?}");
    assert!(
        RemedyPlan::default()
            .with_variable_values(&planned)
            .dispatch_parameters("fixture.observe", INSTANCE)
            .is_none(),
        "no record"
    );
}

/// Fix round 1 (the review's required fix; package-variables.md, "Carried by the rule"): the plan
/// judges each value a task carries under the declaration in force (the table's), by the
/// composer's own judgement. The two rules composed under [`declaration`], the package is replaced
/// by one whose `pick` no longer offers `item_b`: the pair of the rule that carries `item_b` is
/// held back by `pick` with `not_an_option`, the one that carries `item_a` is not. Under the
/// declaration as composed, neither is.
#[test]
fn sv2_t10_a_rule_value_the_declaration_in_force_refuses_holds_its_rule_back() {
    let composition =
        compose(&two_rules()).unwrap_or_else(|failure| panic!("the rules compose: {failure}"));
    let mut replaced = declaration();
    replaced[0]["options"] = options(&["item_a", "item_c"]);
    let narrowed = parse_package_variables(&replaced, &[]).expect("the replaced variables");
    let task = |task_id: &str| VariableTableTask {
        task_id: task_id.to_owned(),
        games: BTreeSet::from([GAME.to_owned()]),
        package: Some(VariablePackage {
            package_id: PACKAGE.to_owned(),
            variables: narrowed.clone(),
            cap_fact_keys: Vec::new(),
        }),
    };
    let in_force = VariableTable::build(&[task(RULE_A), task(RULE_B)]);
    let narrowed_plan =
        crate::variables::plan_with_entries(composition.compiled(), &facts(), &in_force, |_| {
            BTreeMap::new()
        });
    assert_eq!(
        narrowed_plan.rule_value_refused(RULE_B, INSTANCE),
        Some(&RefusedRuleValue {
            package_id: PACKAGE.to_owned(),
            variable_name: "pick".to_owned(),
            problem: Some(VariableProblem::NotAnOption),
        })
    );
    assert_eq!(narrowed_plan.rule_value_refused(RULE_A, INSTANCE), None);

    let as_composed = plan(&[]);
    for task_id in [RULE_A, RULE_B] {
        assert_eq!(
            as_composed.rule_value_refused(task_id, INSTANCE),
            None,
            "{task_id}"
        );
    }
}
