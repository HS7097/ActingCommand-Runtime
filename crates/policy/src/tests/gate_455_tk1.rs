// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 TK-1 (0.12.89; register §1.532; contracts/package-variables.md, "A variable a rule
// gives"; contracts/standard-pack-schedule.md, "A rule's variable values"), the policy half: a
// package declares `pick` with `given_by: "rule"`, so only a rule gives it.
// - two rules of the package, each carrying its own `pick`, are both candidates with nothing
//   stored, each pair's record handing its rule's value with the source `rule`; a value stored
//   under `pick` before it became this kind (an orphan) holds neither back and is not read, where
//   the same stored value holds the other rule back while `pick` is a person's variable (t1);
// - a rule of the package that carries no value for `pick` (another variable only, or no member)
//   is refused `schedule_invalid` at its `variables` (t2);
// - the rules composed while `pick` was a person's variable, the package replaced by one whose
//   `pick` only a rule gives: the pair of the rule that carries no `pick` is held back by it, the
//   other is not (t3).
// A made-up game; every package, variable and value is a placeholder. No clock is read.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{
    DeclaredCapabilities, IrreversiblePackages, PackageVariable, RuntimeChannel,
    STANDARD_PACK_SCHEDULE_FILE, StandardPackProblem, StandardPackRootBytes, StandardPackRootIssue,
    parse_package_variables, read_standard_pack_root,
};
use serde_json::{Value, json};

use crate::resource_targets::StoredEntry;
use crate::{
    EvaluationFacts, InstanceSnapshot, MissingRuleValue, RULE_VARIABLES_SUBSYSTEM, RemedyPlan,
    STANDARD_PACK_OPERATION_ID, StandardPackBinding, StandardPackComposeFailure,
    StandardPackComposeInput, StandardPackComposition, StandardPackGameFiles, VariablePackage,
    VariablePlan, VariableSetBy, VariableTable, VariableTableTask, compose_standard_pack_rules,
};

const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const INSTANCE: &str = "fixture-instance-a";
/// The one package, which declares the two variables of [`declaration`].
const PACKAGE: &str = "pack.shop";
/// Its two rules: the first carries `pick` = `item_a`, the second `pick` = `item_b`.
const RULE_A: &str = "pack.shop.a";
const RULE_B: &str = "pack.shop.b";
/// When the fixture's stored values were set.
const SET_AT: u64 = 1_000;

/// The package's variables: `pick`, a choice of item_a and item_b, which only a rule gives when
/// `rule_given` (no default) and is a person's variable with the default item_b otherwise, and
/// `extra`, a switch with the default false.
fn declaration(rule_given: bool) -> Value {
    let mut pick = json!({"name": "pick", "type": "choice", "title": {"en": "Item"},
                          "options": [{"value": "item_a", "title": {"en": "Item A"}},
                                      {"value": "item_b", "title": {"en": "Item B"}}]});
    if rule_given {
        pick["given_by"] = json!("rule");
    } else {
        pick["default"] = json!("item_b");
    }
    json!([pick, {"name": "extra", "type": "switch", "title": {"en": "Extra"}, "default": false}])
}

fn variables(rule_given: bool) -> Vec<PackageVariable> {
    parse_package_variables(&declaration(rule_given), &[]).expect("the fixture's variables")
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
/// [`PACKAGE`], read declaring `declared`.
fn compose(
    document: &Value,
    declared: &[PackageVariable],
) -> Result<StandardPackComposition, StandardPackComposeFailure> {
    let bundle = json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": [{
            "package_id": PACKAGE,
            "server": SERVER,
            "entry_task_id": "task",
            "path": format!("packs/{PACKAGE}"),
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
    let irreversible = IrreversiblePackages::new(
        root.registry
            .package_ids()
            .map(|package_id| {
                (
                    package_id.to_owned(),
                    Some(DeclaredCapabilities {
                        provides: BTreeMap::new(),
                        remedies: BTreeSet::new(),
                        variables: Some(declared.to_vec()),
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
        subsystems: None,
        catalog_version: 1,
    })
}

/// The first refusal of composing `document` with [`PACKAGE`] declaring `declared`.
fn first_issue(document: &Value, declared: &[PackageVariable]) -> StandardPackRootIssue {
    let failure = compose(document, declared).expect_err("the root is refused");
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
        fact_snapshot_id: "snapshot:fixture-rule-given".to_owned(),
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

/// The table of the two rules' tasks, each running [`PACKAGE`] declaring `declared` (the
/// declaration in force).
fn table(declared: &[PackageVariable]) -> VariableTable {
    let task = |task_id: &str| VariableTableTask {
        task_id: task_id.to_owned(),
        games: BTreeSet::from([GAME.to_owned()]),
        package: Some(VariablePackage {
            package_id: PACKAGE.to_owned(),
            variables: declared.to_vec(),
            cap_fact_keys: Vec::new(),
        }),
    };
    VariableTable::build(&[task(RULE_A), task(RULE_B)])
}

/// The plan of `composition` under the declaration in force `declared`, with the application's
/// entry rows `entries`, each set through the Lab at [`SET_AT`].
fn plan(
    composition: &StandardPackComposition,
    declared: &[PackageVariable],
    entries: &[(&str, Value)],
) -> VariablePlan {
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
    crate::variables::plan_with_entries(composition.compiled(), &facts(), &table(declared), |_| {
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

/// Rulings 2 and 7: with `pick` only a rule gives, the two rules compose and each pair's record
/// hands its own rule's `pick` (source `rule`, no setter) and `extra` its default; nothing is held
/// back with nothing stored. A value stored under `pick` (an orphan) is not read: no pair is held
/// back by it, no record names a setter, nothing is a value not applied, and the page's own
/// resolutions hold `extra` alone. While `pick` is a person's variable the same stored value holds
/// the other rule back (SV-2), so the difference is the declaration's member.
#[test]
fn tk1_t1_two_rules_each_carrying_its_own_value_are_both_candidates_with_nothing_stored() {
    let rule_given = variables(true);
    let composition = compose(&two_rules(), &rule_given)
        .unwrap_or_else(|failure| panic!("the rules compose: {failure}"));
    let nothing = plan(&composition, &rule_given, &[]);
    let orphan = plan(&composition, &rule_given, &[("pick", json!("item_a"))]);
    for (task_id, own) in [(RULE_A, "\"item_a\""), (RULE_B, "\"item_b\"")] {
        let expected = json!([
            {"variable_name": "pick", "variable_source": "rule", "value_json": own},
            {"variable_name": "extra", "variable_source": "default", "value_json": "false"}
        ]);
        for (case, planned) in [("nothing stored", &nothing), ("an orphan stored", &orphan)] {
            assert_eq!(records(planned, task_id), expected, "{task_id}, {case}");
            assert_eq!(
                planned.excluded_by(task_id, INSTANCE),
                None,
                "{task_id}, {case}"
            );
            assert_eq!(
                planned.rule_value_missing(task_id, INSTANCE),
                None,
                "{task_id}, {case}"
            );
            assert_eq!(
                planned.rule_value_refused(task_id, INSTANCE),
                None,
                "{task_id}, {case}"
            );
            assert!(
                planned.not_applied(task_id, INSTANCE).is_empty(),
                "{task_id}, {case}"
            );
            let resolved = planned
                .resolutions(task_id, INSTANCE)
                .unwrap_or_else(|| panic!("{task_id}'s resolutions, {case}"));
            assert_eq!(
                resolved
                    .iter()
                    .map(|resolved| resolved.variable_name.as_str())
                    .collect::<Vec<_>>(),
                ["extra"],
                "{task_id}, {case}"
            );
        }
    }

    let person_s = variables(false);
    let before = compose(&two_rules(), &person_s)
        .unwrap_or_else(|failure| panic!("the rules compose: {failure}"));
    let stored = plan(&before, &person_s, &[("pick", json!("item_a"))]);
    assert_eq!(stored.excluded_by(RULE_A, INSTANCE), None);
    assert_eq!(stored.excluded_by(RULE_B, INSTANCE), Some("pick"));
}

/// Ruling 3: a rule of the package that carries no value for `pick` is refused `schedule_invalid`
/// at its `variables`, whether it carries another variable's value or no member at all; a rule of
/// a package whose `pick` is a person's variable may carry none.
#[test]
fn tk1_t2_a_rule_that_carries_no_value_for_it_is_refused_at_its_variables() {
    let rule_given = variables(true);
    let other_only = schedule(
        vec![
            rule(RULE_A, Some(json!({"pick": "item_a"}))),
            rule(RULE_B, Some(json!({"extra": true}))),
        ],
        &[RULE_VARIABLES_SUBSYSTEM],
    );
    assert_eq!(
        first_issue(&other_only, &rule_given),
        invalid_at("/rules/1/variables")
    );
    let no_member = schedule(vec![rule(RULE_A, None)], &[]);
    assert_eq!(
        first_issue(&no_member, &rule_given),
        invalid_at("/rules/0/variables")
    );
    compose(&no_member, &variables(false))
        .unwrap_or_else(|failure| panic!("a person's variable needs no rule value: {failure}"));
}

/// Ruling 4, the plan's half: the rules composed while `pick` was a person's variable (the first
/// carrying `item_a`, the second nothing), the package replaced by one whose `pick` only a rule
/// gives: the pair of the rule that carries no `pick` is held back by it, naming the package and
/// the variable, and the other is not; under the declaration as composed neither is.
#[test]
fn tk1_t3_a_rule_without_the_value_the_declaration_in_force_asks_is_held_back() {
    let composed_under = variables(false);
    let composition = compose(
        &schedule(
            vec![
                rule(RULE_A, Some(json!({"pick": "item_a"}))),
                rule(RULE_B, None),
            ],
            &[RULE_VARIABLES_SUBSYSTEM],
        ),
        &composed_under,
    )
    .unwrap_or_else(|failure| panic!("the rules compose: {failure}"));
    let in_force = variables(true);
    let replaced = plan(&composition, &in_force, &[]);
    assert_eq!(
        replaced.rule_value_missing(RULE_B, INSTANCE),
        Some(&MissingRuleValue {
            package_id: PACKAGE.to_owned(),
            variable_name: "pick".to_owned(),
        })
    );
    assert_eq!(replaced.rule_value_missing(RULE_A, INSTANCE), None);

    let as_composed = plan(&composition, &composed_under, &[]);
    for task_id in [RULE_A, RULE_B] {
        assert_eq!(
            as_composed.rule_value_missing(task_id, INSTANCE),
            None,
            "{task_id}"
        );
    }
}
